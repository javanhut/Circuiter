//! Projects: a directory holding a `circuiter.toml` manifest and the project's own cell
//! library. The project library is named after the project.
//!
//! ```toml
//! [project]
//! name = "cmos_inverter"
//! top = "testbench"
//!
//! [libraries]
//! analog = { path = "../shared/analog" }
//! ```

use crate::cell::{Cell, CellDef, validate_name};
use crate::error::{ModelError, Result};
use crate::format::write_file;
use crate::library::Library;
use crate::schematic::Schematic;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const MANIFEST_FILE: &str = "circuiter.toml";

/// Library names that projects may not use.
pub const RESERVED_LIBRARY_NAMES: &[&str] = &[crate::design::STD_LIBRARY];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub project: ProjectInfo,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub libraries: BTreeMap<String, LibraryEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
    /// The top-level cell, simulated and laid out by default.
    pub top: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryEntry {
    /// Library directory, relative to the project root.
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub root: PathBuf,
    pub manifest: Manifest,
    pub library: Library,
}

impl Project {
    /// Creates a new project in `root` with an empty top-level schematic.
    pub fn create(root: &Path, name: &str) -> Result<Self> {
        let manifest_path = root.join(MANIFEST_FILE);
        if manifest_path.exists() {
            return Err(ModelError::Invalid(format!("{} already exists", manifest_path.display())));
        }
        let manifest = Manifest {
            project: ProjectInfo {
                name: name.to_owned(),
                top: "top".to_owned(),
                description: String::new(),
            },
            libraries: BTreeMap::new(),
        };
        validate_manifest(&manifest).map_err(|e| ModelError::parse(&manifest_path, e))?;

        let mut library = Library::new(name);
        library.insert(CellDef {
            name: "top".to_owned(),
            cell: Cell { description: "Top-level design".to_owned(), ..Cell::default() },
            schematic: Some(Schematic::default()),
            ..CellDef::default()
        });

        let project = Project { root: root.to_path_buf(), manifest, library };
        project.save()?;
        Ok(project)
    }

    pub fn load(root: &Path) -> Result<Self> {
        let manifest_path = root.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|e| ModelError::io(&manifest_path, e))?;
        let manifest: Manifest =
            toml::from_str(&text).map_err(|e| ModelError::parse(&manifest_path, e))?;
        validate_manifest(&manifest).map_err(|e| ModelError::parse(&manifest_path, e))?;
        let library = Library::load_dir(&manifest.project.name, root)?;
        Ok(Project { root: root.to_path_buf(), manifest, library })
    }

    pub fn save(&self) -> Result<()> {
        let manifest_path = self.root.join(MANIFEST_FILE);
        let text = toml::to_string_pretty(&self.manifest)
            .map_err(|e| ModelError::parse(&manifest_path, e))?;
        write_file(&manifest_path, &text)?;
        self.library.save_dir(&self.root)
    }

    /// Finds the project containing `start` by searching it and its ancestors for a manifest.
    pub fn find_root(start: &Path) -> Option<PathBuf> {
        start.ancestors().find(|dir| dir.join(MANIFEST_FILE).is_file()).map(Path::to_path_buf)
    }

    pub fn name(&self) -> &str {
        &self.manifest.project.name
    }

    /// Loads the external libraries listed in the manifest.
    pub fn load_libraries(&self) -> Result<Vec<Library>> {
        self.manifest
            .libraries
            .iter()
            .map(|(name, entry)| Library::load_dir(name, &self.root.join(&entry.path)))
            .collect()
    }
}

fn validate_manifest(m: &Manifest) -> std::result::Result<(), String> {
    let name = &m.project.name;
    validate_name(name).map_err(|e| format!("project name: {e}"))?;
    validate_name(&m.project.top).map_err(|e| format!("top cell: {e}"))?;
    if RESERVED_LIBRARY_NAMES.contains(&name.as_str()) {
        return Err(format!("project name `{name}` is reserved"));
    }
    for lib in m.libraries.keys() {
        validate_name(lib).map_err(|e| format!("library name: {e}"))?;
        if RESERVED_LIBRARY_NAMES.contains(&lib.as_str()) || lib == name {
            return Err(format!("library name `{lib}` is reserved or clashes with the project"));
        }
    }
    Ok(())
}
