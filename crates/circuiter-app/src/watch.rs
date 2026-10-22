//! Notices when another program changes a project's files (an assistant working through
//! `circuiter mcp`, a text editor), so the open project can take the changes in.
//!
//! A thread looks at the files' sizes and modification times twice a second and wakes
//! the window when they change; the window then merges what changed.

use eframe::egui;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How often the files are looked at.
const INTERVAL: Duration = Duration::from_millis(500);

#[derive(Debug)]
pub struct Watch {
    paths: Vec<PathBuf>,
    changed: Arc<AtomicBool>,
    /// Report a change on the next look even if nothing changed: the last merge failed,
    /// most likely on a file caught half written.
    again: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl Watch {
    /// Starts watching `paths` (files, or directories with everything under them).
    pub fn start(paths: Vec<PathBuf>, ctx: egui::Context) -> Watch {
        let changed = Arc::new(AtomicBool::new(false));
        let again = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (c, a, s, watched) = (changed.clone(), again.clone(), stop.clone(), paths.clone());
        std::thread::spawn(move || {
            let mut last = stamp(&watched);
            while !s.load(Ordering::Relaxed) {
                std::thread::sleep(INTERVAL);
                let now = stamp(&watched);
                if now != last || a.swap(false, Ordering::Relaxed) {
                    last = now;
                    c.store(true, Ordering::Relaxed);
                    ctx.request_repaint();
                }
            }
        });
        Watch { paths, changed, again, stop }
    }

    /// Whether this watches exactly `paths`.
    pub fn watches(&self, paths: &[PathBuf]) -> bool {
        self.paths == paths
    }

    /// Whether the files changed since the last call.
    pub fn take(&self) -> bool {
        self.changed.swap(false, Ordering::Relaxed)
    }

    /// Reports a change again on the next look.
    pub fn retry(&self) {
        self.again.store(true, Ordering::Relaxed);
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A hash of the names, sizes and modification times of every file under `paths`.
pub fn stamp(paths: &[PathBuf]) -> u64 {
    let mut h = DefaultHasher::new();
    for path in paths {
        visit(path, &mut h);
    }
    h.finish()
}

fn visit(path: &Path, h: &mut DefaultHasher) {
    let Ok(meta) = std::fs::metadata(path) else { return };
    path.hash(h);
    if meta.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else { return };
        let mut entries: Vec<PathBuf> = entries.filter_map(|e| Some(e.ok()?.path())).collect();
        entries.sort();
        for entry in entries {
            visit(&entry, h);
        }
    } else {
        meta.len().hash(h);
        meta.modified().ok().hash(h);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stamp_follows_the_files() {
        let dir = tempfile::tempdir().unwrap();
        let cells = dir.path().join("cells");
        std::fs::create_dir_all(cells.join("amp")).unwrap();
        let paths = vec![cells.clone()];
        let empty = stamp(&paths);
        std::fs::write(cells.join("amp/cell.ron"), "()").unwrap();
        let one = stamp(&paths);
        assert_ne!(empty, one);
        assert_eq!(one, stamp(&paths));
        std::fs::write(cells.join("amp/cell.ron"), "(description: \"x\")").unwrap();
        assert_ne!(one, stamp(&paths));
    }
}
