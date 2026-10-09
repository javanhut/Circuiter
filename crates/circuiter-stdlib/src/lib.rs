//! Circuiter's built-in `std` library: passives, sources, semiconductors and ground.
//!
//! The cells are ordinary library files under `library/`, embedded at compile time, so
//! they use the same format as user libraries and can be copied and modified.

use circuiter_model::{Library, Result, STD_LIBRARY};
use include_dir::{Dir, DirEntry, include_dir};

static LIBRARY_DIR: Dir<'static> = include_dir!("$CARGO_MANIFEST_DIR/library");

/// Loads the standard library.
pub fn load() -> Result<Library> {
    let mut files = Vec::new();
    collect(&LIBRARY_DIR, &mut files);
    Library::from_files(STD_LIBRARY, files.iter().map(|(p, t)| (*p, *t)))
}

/// The standard library. Its validity is covered by this crate's tests.
pub fn library() -> Library {
    load().expect("built-in standard library is valid")
}

fn collect<'a>(dir: &'a Dir<'a>, out: &mut Vec<(&'a std::path::Path, &'a str)>) {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(d) => collect(d, out),
            DirEntry::File(f) => {
                if let Some(text) = f.contents_utf8() {
                    out.push((f.path(), text));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use circuiter_model::check::check_libraries;
    use circuiter_model::{Design, ViewKind};

    #[test]
    fn standard_library_loads_and_passes_checks() {
        let lib = load().unwrap();
        assert!(lib.cells.len() >= 12, "only {} cells loaded", lib.cells.len());
        let mut design = Design::new();
        design.add_library(lib).unwrap();
        let diagnostics = check_libraries(&design, &[STD_LIBRARY]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
    }

    /// The library files must be in canonical form, so that editing a cell in Circuiter
    /// and saving it produces a minimal diff. Run with `CIRCUITER_BLESS=1` to rewrite them.
    #[test]
    fn library_files_are_canonical() {
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("library");
        let lib = load().unwrap();
        if std::env::var_os("CIRCUITER_BLESS").is_some() {
            lib.save_dir(&source).unwrap();
            return;
        }
        let out = tempfile::tempdir().unwrap();
        lib.save_dir(out.path()).unwrap();
        let mut stale = Vec::new();
        for def in lib.cells.values() {
            let rel = std::path::Path::new("cells").join(&def.name);
            for entry in std::fs::read_dir(out.path().join(&rel)).unwrap() {
                let file = rel.join(entry.unwrap().file_name());
                let expected = std::fs::read_to_string(out.path().join(&file)).unwrap();
                let actual = std::fs::read_to_string(source.join(&file)).unwrap_or_default();
                if expected != actual {
                    stale.push(file.display().to_string());
                }
            }
        }
        assert!(stale.is_empty(), "not canonical (run with CIRCUITER_BLESS=1): {stale:?}");
    }

    #[test]
    fn every_cell_is_placeable_and_simulatable() {
        for def in library().cells.values() {
            assert!(def.symbol.is_some(), "{} has no symbol", def.name);
            assert!(
                def.cell.global_net.is_some() || def.has_view(ViewKind::Spice),
                "{} has no SPICE view",
                def.name
            );
        }
    }
}
