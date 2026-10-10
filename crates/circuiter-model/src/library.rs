//! Libraries: named collections of cells.
//!
//! On disk a library is a directory with one sub-directory per cell, optionally sorted
//! into folders. A directory holding a `cell.ron` is a cell; any other directory is a
//! folder:
//!
//! ```text
//! cells/
//!   gates/
//!     nand/
//!       cell.ron      interface: ports, parameters, metadata (required)
//!       symbol.ron    symbol view
//!       spice.ron     SPICE view
//!   top/
//!     cell.ron
//!     schematic.ron   schematic view
//! ```
//!
//! Folders are only for organising: a cell is referred to by its name alone, so names
//! are unique within a library and a cell can move between folders freely. A folder
//! stays until it is deleted, even with nothing in it yet.

use crate::cell::{Cell, CellDef, validate_folder, validate_name};
use crate::error::{ModelError, Result};
use crate::format::{from_ron, to_ron, write_file};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const CELLS_DIR: &str = "cells";

const CELL_FILE: &str = "cell.ron";
const SYMBOL_FILE: &str = "symbol.ron";
const SCHEMATIC_FILE: &str = "schematic.ron";
const SPICE_FILE: &str = "spice.ron";
const DIGITAL_FILE: &str = "digital.ron";
const VIEW_FILES: [&str; 5] = [CELL_FILE, SYMBOL_FILE, SCHEMATIC_FILE, SPICE_FILE, DIGITAL_FILE];

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Library {
    pub name: String,
    pub cells: BTreeMap<String, CellDef>,
    /// Folders that exist in their own right, such as ones just made and still empty.
    /// Folders holding cells exist anyway; see [`Library::all_folders`].
    pub folders: BTreeSet<String>,
}

impl Library {
    pub fn new(name: impl Into<String>) -> Self {
        Library { name: name.into(), cells: BTreeMap::new(), folders: BTreeSet::new() }
    }

    pub fn cell(&self, name: &str) -> Option<&CellDef> {
        self.cells.get(name)
    }

    pub fn insert(&mut self, def: CellDef) {
        self.cells.insert(def.name.clone(), def);
    }

    /// Every folder, each listed once, parents before children.
    pub fn all_folders(&self) -> Vec<String> {
        let mut all = BTreeSet::new();
        let named = self.cells.values().map(|d| &d.folder).chain(&self.folders);
        for folder in named.filter(|f| !f.is_empty()) {
            let mut at = 0;
            while let Some(i) = folder[at..].find('/') {
                all.insert(folder[..at + i].to_owned());
                at += i + 1;
            }
            all.insert(folder.clone());
        }
        all.into_iter().collect()
    }

    /// Whether `folder` or any folder inside it holds a cell.
    pub fn folder_has_cells(&self, folder: &str) -> bool {
        self.cells.values().any(|d| in_folder(&d.folder, folder))
    }

    /// Builds a library from `(relative path, contents)` pairs laid out as under
    /// [`CELLS_DIR`]. Paths outside `cells/` and files without a `.ron` extension are
    /// ignored, so libraries can carry READMEs and other assets.
    pub fn from_files<'a>(
        name: &str,
        files: impl IntoIterator<Item = (&'a Path, &'a str)>,
    ) -> Result<Self> {
        validate_name(name).map_err(|e| ModelError::Invalid(format!("library name: {e}")))?;
        // (cell, folder) → files
        let mut grouped: BTreeMap<(String, String), Vec<(&Path, &str)>> = BTreeMap::new();
        for (path, text) in files {
            let parts: Option<Vec<&str>> =
                path.components().map(|c| c.as_os_str().to_str()).collect();
            let Some(parts) = parts else { continue };
            let [CELLS_DIR, folder @ .., cell, file] = parts.as_slice() else { continue };
            if file.ends_with(".ron") {
                grouped.entry((cell.to_string(), folder.join("/"))).or_default().push((path, text));
            }
        }

        let mut lib = Library::new(name);
        for ((cell_name, folder), files) in grouped {
            if let Some(other) = lib.cell(&cell_name) {
                return Err(ModelError::Invalid(format!(
                    "cell `{cell_name}` is in two folders, `{}/` and `{folder}/`",
                    other.folder
                )));
            }
            validate_folder(&folder).map_err(ModelError::Invalid)?;
            let mut def = parse_cell(&cell_name, &files)?;
            def.folder = folder;
            lib.insert(def);
        }
        Ok(lib)
    }

    /// Loads a library from a directory on disk. Every directory that isn't a cell is
    /// a folder, empty or not.
    pub fn load_dir(name: &str, root: &Path) -> Result<Self> {
        let cells = root.join(CELLS_DIR);
        let mut files: Vec<(PathBuf, String)> = Vec::new();
        let mut folders = BTreeSet::new();
        let mut dirs = vec![cells.clone()];
        while let Some(dir) = dirs.pop() {
            if !dir.is_dir() {
                continue;
            }
            let is_cell = dir.join(CELL_FILE).is_file();
            if !is_cell && dir != cells {
                // Directories named unlike folders (`.cache`, `my notes`) are someone
                // else's business.
                match folder_of(&cells, &dir) {
                    Some(folder) if validate_folder(&folder).is_ok() => {
                        folders.insert(folder);
                    }
                    _ => continue,
                }
            }
            for entry in sorted_entries(&dir)? {
                if entry.is_dir() {
                    // Inside a cell, only cells (in folders of their own) are looked for.
                    if !is_cell || entry.join(CELL_FILE).is_file() {
                        dirs.push(entry);
                    }
                } else if entry.extension().is_some_and(|e| e == "ron") {
                    let text =
                        std::fs::read_to_string(&entry).map_err(|e| ModelError::io(&entry, e))?;
                    let rel = entry.strip_prefix(root).unwrap_or(&entry).to_path_buf();
                    files.push((rel, text));
                }
            }
        }
        let mut lib =
            Library::from_files(name, files.iter().map(|(p, t)| (p.as_path(), t.as_str())))
                .map_err(|e| prefix_path(e, root))?;
        lib.folders = folders;
        Ok(lib)
    }

    /// Where a cell's files go under a library directory: `cells/<folder>/<cell>`.
    pub fn cell_dir(def: &CellDef) -> PathBuf {
        let mut dir = PathBuf::from(CELLS_DIR);
        dir.extend(def.folder.split('/').filter(|f| !f.is_empty()));
        dir.push(&def.name);
        dir
    }

    /// Writes every cell to `root/cells/<folder>/<cell>/`, and makes every folder. View
    /// files for views a cell no longer has are removed, as are the files a cell left
    /// behind when it moved to another folder, and deleted folders once empty; other
    /// files are left alone.
    pub fn save_dir(&self, root: &Path) -> Result<()> {
        let folders = self.all_folders();
        for folder in &folders {
            validate_folder(folder).map_err(ModelError::Invalid)?;
            let dir = root.join(CELLS_DIR).join(folder);
            std::fs::create_dir_all(&dir).map_err(|e| ModelError::io(&dir, e))?;
        }
        for def in self.cells.values() {
            validate_folder(&def.folder).map_err(ModelError::Invalid)?;
            let dir = root.join(Library::cell_dir(def));
            let cell_path = dir.join(CELL_FILE);
            write_file(&cell_path, &to_ron(&cell_path, &def.cell)?)?;
            save_view(&dir.join(SYMBOL_FILE), def.symbol.as_ref())?;
            save_view(&dir.join(SCHEMATIC_FILE), def.schematic.as_ref())?;
            save_view(&dir.join(SPICE_FILE), def.spice.as_ref())?;
            save_view(&dir.join(DIGITAL_FILE), def.digital.as_ref())?;
        }
        self.tidy(root, &folders)
    }

    /// Removes the view files of this library's cells found anywhere but where they
    /// belong, then directories left empty that are neither cells nor folders.
    fn tidy(&self, root: &Path, folders: &[String]) -> Result<()> {
        let cells = root.join(CELLS_DIR);
        // Every directory under `cells/`, parents before children.
        let mut all = Vec::new();
        let mut dirs = vec![cells.clone()];
        while let Some(dir) = dirs.pop() {
            for entry in sorted_entries(&dir)?.into_iter().filter(|e| e.is_dir()) {
                all.push(entry.clone());
                dirs.push(entry);
            }
        }
        for dir in &all {
            let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            let moved = dir.join(CELL_FILE).is_file()
                && self.cell(name).is_some_and(|d| root.join(Library::cell_dir(d)) != *dir);
            if moved {
                for file in VIEW_FILES {
                    remove_if_present(&dir.join(file))?;
                }
            }
        }
        // Children first, so emptied parents go too. `remove_dir` refuses anything with
        // files left in it, such as notes someone keeps next to their cells.
        for dir in all.iter().rev() {
            let is_cell = self.cells.values().any(|d| root.join(Library::cell_dir(d)) == *dir);
            let is_folder = folder_of(&cells, dir).is_some_and(|f| folders.contains(&f));
            if !is_cell && !is_folder {
                let _ = std::fs::remove_dir(dir);
            }
        }
        Ok(())
    }
}

/// Whether a cell in folder `cell_folder` is inside `folder` (at any depth).
pub fn in_folder(cell_folder: &str, folder: &str) -> bool {
    folder.is_empty()
        || cell_folder == folder
        || cell_folder.strip_prefix(folder).is_some_and(|rest| rest.starts_with('/'))
}

/// The folder path of directory `dir` under `cells`: `analog/filters`.
fn folder_of(cells: &Path, dir: &Path) -> Option<String> {
    let rel = dir.strip_prefix(cells).ok()?;
    let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
    Some(parts?.join("/"))
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
            DIGITAL_FILE => def.digital = Some(from_ron(path, text)?),
            other => {
                return Err(ModelError::parse(
                    path,
                    format!(
                        "unknown view file `{other}`; expected {CELL_FILE}, {SYMBOL_FILE}, {SCHEMATIC_FILE}, {SPICE_FILE} or {DIGITAL_FILE}"
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
        None => remove_if_present(path),
    }
}

fn remove_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(ModelError::io(path, e)),
        _ => Ok(()),
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
