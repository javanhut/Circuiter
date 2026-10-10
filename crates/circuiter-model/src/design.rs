//! A design is the set of libraries visible to a project, with cell resolution and the
//! instance hierarchy built on top.

use crate::cell::{CellDef, CellRef, ViewKind};
use crate::error::{ModelError, Result};
use crate::library::Library;
use crate::project::Project;
use std::collections::BTreeMap;
use std::fmt;

/// The built-in standard library, always present.
pub const STD_LIBRARY: &str = "std";

/// A fully qualified cell identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CellId {
    pub library: String,
    pub cell: String,
}

impl CellId {
    pub fn new(library: &str, cell: &str) -> Self {
        CellId { library: library.to_owned(), cell: cell.to_owned() }
    }
}

impl fmt::Display for CellId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.library, self.cell)
    }
}

#[derive(Debug, Clone)]
pub struct Design {
    libraries: BTreeMap<String, Library>,
    /// View precedence when an instance doesn't force one. Schematic first means a
    /// cell's own transistor-level design is used over its SPICE model when both exist.
    pub view_order: Vec<ViewKind>,
    /// The project's library, if this design belongs to a project.
    pub project_library: Option<String>,
}

impl Default for Design {
    fn default() -> Self {
        Design {
            libraries: BTreeMap::new(),
            view_order: vec![ViewKind::Schematic, ViewKind::Spice],
            project_library: None,
        }
    }
}

impl Design {
    pub fn new() -> Self {
        Design::default()
    }

    /// Builds the design for a project: the standard library, the project's own library
    /// and every library listed in its manifest.
    pub fn for_project(project: &Project, std: Library) -> Result<Self> {
        let mut design = Design::new();
        design.add_library(std)?;
        design.add_library(project.library.clone())?;
        for lib in project.load_libraries()? {
            design.add_library(lib)?;
        }
        design.project_library = Some(project.name().to_owned());
        Ok(design)
    }

    pub fn add_library(&mut self, lib: Library) -> Result<()> {
        if self.libraries.contains_key(&lib.name) {
            return Err(ModelError::Invalid(format!("library `{}` is loaded twice", lib.name)));
        }
        self.libraries.insert(lib.name.clone(), lib);
        Ok(())
    }

    pub fn library(&self, name: &str) -> Option<&Library> {
        self.libraries.get(name)
    }

    pub fn libraries(&self) -> impl Iterator<Item = &Library> {
        self.libraries.values()
    }

    pub fn cell(&self, id: &CellId) -> Option<&CellDef> {
        self.library(&id.library)?.cell(&id.cell)
    }

    pub fn library_mut(&mut self, name: &str) -> Option<&mut Library> {
        self.libraries.get_mut(name)
    }

    pub fn cell_mut(&mut self, id: &CellId) -> Option<&mut CellDef> {
        self.libraries.get_mut(&id.library)?.cells.get_mut(&id.cell)
    }

    /// Whether `ancestor`'s schematic hierarchy contains `cell` (or is `cell`). Placing
    /// `ancestor` inside `cell` would then make the design recursive.
    pub fn contains(&self, ancestor: &CellId, cell: &CellId) -> bool {
        let mut stack = vec![ancestor.clone()];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(id) = stack.pop() {
            if &id == cell {
                return true;
            }
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(schematic) = self.cell(&id).and_then(|d| d.schematic.as_ref()) {
                for inst in &schematic.instances {
                    if let Some((child, _)) = self.resolve(&inst.cell, &id.library) {
                        stack.push(child);
                    }
                }
            }
        }
        false
    }

    /// Resolves a reference made from a cell in `context_library`. Unqualified names
    /// refer to that same library.
    pub fn resolve(&self, r: &CellRef, context_library: &str) -> Option<(CellId, &CellDef)> {
        let library = r.library.as_deref().unwrap_or(context_library);
        let id = CellId::new(library, &r.cell);
        let def = self.cell(&id)?;
        Some((id, def))
    }

    /// The view used to elaborate `def`, honouring an instance's forced view.
    pub fn select_view(&self, def: &CellDef, forced: Option<ViewKind>) -> Option<ViewKind> {
        match forced {
            Some(v) => def.has_view(v).then_some(v),
            None => self.view_order.iter().copied().find(|v| def.has_view(*v)),
        }
    }

    /// Elaborates the instance hierarchy under `top`.
    ///
    /// Fails on unresolved cells or recursive instantiation; run the checker first for
    /// complete diagnostics.
    pub fn hierarchy(&self, top: &CellId) -> Result<HierNode> {
        let def = self
            .cell(top)
            .ok_or_else(|| ModelError::Invalid(format!("top cell `{top}` not found")))?;
        let mut stack = vec![top.clone()];
        self.elaborate(None, top.clone(), def, None, &mut stack)
    }

    fn elaborate(
        &self,
        instance: Option<String>,
        id: CellId,
        def: &CellDef,
        forced: Option<ViewKind>,
        stack: &mut Vec<CellId>,
    ) -> Result<HierNode> {
        let view = self.select_view(def, forced);
        let mut children = Vec::new();
        if view == Some(ViewKind::Schematic) {
            let schematic = def.schematic.as_ref().expect("selected view exists");
            for inst in &schematic.instances {
                let (child_id, child_def) =
                    self.resolve(&inst.cell, &id.library).ok_or_else(|| {
                        ModelError::Invalid(format!(
                            "{id}: instance {} uses unknown cell `{}`",
                            inst.name, inst.cell
                        ))
                    })?;
                if stack.contains(&child_id) {
                    return Err(ModelError::Invalid(format!(
                        "recursive instantiation: {} -> {child_id}",
                        stack.iter().map(ToString::to_string).collect::<Vec<_>>().join(" -> ")
                    )));
                }
                stack.push(child_id.clone());
                children.push(self.elaborate(
                    Some(inst.name.clone()),
                    child_id,
                    child_def,
                    inst.view,
                    stack,
                )?);
                stack.pop();
            }
        }
        Ok(HierNode { instance, cell: id, view, children })
    }
}

/// One node of the elaborated instance tree.
#[derive(Debug, Clone, PartialEq)]
pub struct HierNode {
    /// Instance name within the parent; `None` for the top cell.
    pub instance: Option<String>,
    pub cell: CellId,
    /// The view this instance is elaborated with; `None` if the cell has no usable view.
    pub view: Option<ViewKind>,
    pub children: Vec<HierNode>,
}

impl HierNode {
    /// Counts simulated leaf instances (those not elaborated further), e.g. the
    /// transistors in a CPU. Global-net symbols such as ground have no view and don't count.
    pub fn leaf_count(&self) -> usize {
        if self.children.is_empty() {
            usize::from(self.instance.is_some() && self.view.is_some())
        } else {
            self.children.iter().map(HierNode::leaf_count).sum()
        }
    }
}
