//! Libraries: named collections of cells.
//!
//! On disk a library is a directory with one sub-directory per cell:
//!
//! ```text
//! cells/
//!   inverter/
//!     cell.ron        interface: ports, parameters, metadata (required)
//!     symbol.ron      symbol view
//!     schematic.ron   schematic view
//!     spice.ron       SPICE view
//! ```

use crate::cell::{Cell, CellDef, validate_name};
use crate::error::{ModelError, Result};
use crate::format::{from_ron, to_ron, write_file};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const CELLS_DIR: &str = "cells";

const CELL_FILE: &str = "cell.ron";
const SYMBOL_FILE: &str = "symbol.ron";
const SCHEMATIC_FILE: &str = "schematic.ron";
const SPICE_FILE: &str = "spice.ron";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Library {
    pub name: String,
    pub cells: BTreeMap<String, CellDef>,
}

impl Library {
    pub fn new(name: impl Into<String>) -> Self {
        Library { name: name.into(), cells: BTreeMap::new() }
    }

    pub fn cell(&self, name: &str) -> Option<&CellDef> {
        self.cells.get(name)
    }

    pub fn insert(&mut self, def: CellDef) {
        self.cells.insert(def.name.clone(), def);
    }

    /// Builds a library from `(relative path, contents)` pairs laid out as under
    /// [`CELLS_DIR`]. Paths outside `cells/` and files without a `.ron` extension are
    /// ignored, so libraries can carry READMEs and other assets.
    pub fn from_files<'a>(
        name: &str,
        files: impl IntoIterator<Item = (&'a Path, &'a str)>,
    ) -> Result<Self> {
        validate_name(name).map_err(|e| ModelError::Invalid(format!("library name: {e}")))?;
        let mut grouped: BTreeMap<String, Vec<(&Path, &str)>> = BTreeMap::new();
        for (path, text) in files {
            let mut parts = path.components();
            let top = parts.next().and_then(|c| c.as_os_str().to_str());
            let cell = parts.next().and_then(|c| c.as_os_str().to_str());
            let file = parts.next().and_then(|c| c.as_os_str().to_str());
            let (Some(CELLS_DIR), Some(cell), Some(file), None) = (top, cell, file, parts.next())
            else {
                continue;
            };
            if file.ends_with(".ron") {
                grouped.entry(cell.to_owned()).or_default().push((path, text));
            }
        }

        let mut lib = Library::new(name);
        for (cell_name, files) in grouped {
            lib.insert(parse_cell(&cell_name, &files)?);
        }
        Ok(lib)
    }

    /// Loads a library from a directory on disk.
    pub fn load_dir(name: &str, root: &Path) -> Result<Self> {
        let cells_dir = root.join(CELLS_DIR);
        let mut files: Vec<(PathBuf, String)> = Vec::new();
        if cells_dir.is_dir() {
            for cell_dir in sorted_entries(&cells_dir)? {
                if !cell_dir.is_dir() {
                    continue;
                }
                for file in sorted_entries(&cell_dir)? {
                    if file.extension().is_some_and(|e| e == "ron") {
                        let text =
                            std::fs::read_to_string(&file).map_err(|e| ModelError::io(&file, e))?;
                        let rel = file.strip_prefix(root).unwrap_or(&file).to_path_buf();
                        files.push((rel, text));
                    }
                }
            }
        }
        Library::from_files(name, files.iter().map(|(p, t)| (p.as_path(), t.as_str())))
            .map_err(|e| prefix_path(e, root))
    }

    /// Writes every cell to `root/cells/<cell>/`. View files for views a cell no longer
    /// has are removed; other files are left alone.
    pub fn save_dir(&self, root: &Path) -> Result<()> {
        for def in self.cells.values() {
            let dir = root.join(CELLS_DIR).join(&def.name);
            let cell_path = dir.join(CELL_FILE);
            write_file(&cell_path, &to_ron(&cell_path, &def.cell)?)?;
            save_view(&dir.join(SYMBOL_FILE), def.symbol.as_ref())?;
            save_view(&dir.join(SCHEMATIC_FILE), def.schematic.as_ref())?;
            save_view(&dir.join(SPICE_FILE), def.spice.as_ref())?;
        }
        Ok(())
    }
}

fn parse_cell(name: &str, files: &[(&Path, &str)]) -> Result<CellDef> {
    let mut def = CellDef { name: name.to_owned(), ..CellDef::default() };
    let mut have_cell = false;
    for &(path, text) in files {
        let file = path.file_name().and_then(|f| f.to_str()).unwrap_or_default();
        match file {
            CELL_FILE => {
                def.cell = from_ron::<Cell>(path, text)?;
                have_cell = true;
            }
            SYMBOL_FILE => def.symbol = Some(from_ron(path, text)?),
            SCHEMATIC_FILE => def.schematic = Some(from_ron(path, text)?),
            SPICE_FILE => def.spice = Some(from_ron(path, text)?),
            other => {
                return Err(ModelError::parse(
                    path,
                    format!(
                        "unknown view file `{other}`; expected {CELL_FILE}, {SYMBOL_FILE}, {SCHEMATIC_FILE} or {SPICE_FILE}"
                    ),
                ));
            }
        }
    }
    if !have_cell {
        let dir = files.first().and_then(|(p, _)| p.parent()).unwrap_or(Path::new(name));
        return Err(ModelError::parse(dir, format!("cell `{name}` has no {CELL_FILE}")));
    }
    validate_name(name).map_err(|e| ModelError::Invalid(format!("cell name: {e}")))?;
    Ok(def)
}

fn save_view<T: serde::Serialize>(path: &Path, view: Option<&T>) -> Result<()> {
    match view {
        Some(v) => write_file(path, &to_ron(path, v)?),
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(ModelError::io(path, e)),
            _ => Ok(()),
        },
    }
}

fn sorted_entries(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = std::fs::read_dir(dir)
        .map_err(|e| ModelError::io(dir, e))?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|e| ModelError::io(dir, e))?;
    entries.sort();
    Ok(entries)
}

/// Makes paths in errors from [`Library::from_files`] absolute again.
fn prefix_path(err: ModelError, root: &Path) -> ModelError {
    match err {
        ModelError::Parse { path, message } if path.is_relative() => {
            ModelError::Parse { path: root.join(path), message }
        }
        other => other,
    }
}
