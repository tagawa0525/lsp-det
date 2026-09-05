//! Standing in for `workspace/didChangeWatchedFiles` (design 4.3, ADR 0015 decision A).
//!
//! On behalf of a client (Claude Code) that neither declares the capability
//! `workspace.didChangeWatchedFiles` nor sends the notification, every time a 7.0 request
//! arrives, compare the mtimes of the workspace files with the previous time, and send the
//! difference upstream as one notification of Created / Changed / Deleted. No clock is used;
//! the trigger is the request itself.
//!
//! Enumeration is left to `git ls-files --cached --others --exclude-standard`
//! (tracked + untracked but not ignored). There is no interpretation of `.gitignore` and no
//! per-language list of extensions. Roots outside git management are not stood in for.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use crate::framing::RawMessage;
use crate::uri::path_to_uri;

/// LSP's `FileChangeType`.
const CREATED: u8 = 1;
const CHANGED: u8 = 2;
const DELETED: u8 = 3;

pub struct WatchedFiles {
    /// Roots under git management (other roots are excluded at startup).
    roots: Vec<PathBuf>,
    /// The previous listing (path -> mtime).
    snapshot: BTreeMap<PathBuf, SystemTime>,
}

impl WatchedFiles {
    /// Tries `git ls-files` per root and takes the first listing on the usable roots.
    /// `None` if no root is usable (no stand-in).
    pub fn new(roots: &[PathBuf]) -> Option<Self> {
        let usable: Vec<PathBuf> = roots
            .iter()
            .filter(|root| match list_files(root) {
                Some(_) => true,
                None => {
                    eprintln!(
                        "lsp-det: {} is not a git work tree; not standing in for \
                         workspace/didChangeWatchedFiles there",
                        root.display()
                    );
                    false
                }
            })
            .cloned()
            .collect();
        if usable.is_empty() {
            return None;
        }
        let mut stand_in = WatchedFiles {
            roots: usable,
            snapshot: BTreeMap::new(),
        };
        stand_in.snapshot = stand_in.scan()?;
        Some(stand_in)
    }

    /// Turns the difference since the previous time into a notification. `None` if there is no
    /// difference. Also `None` when `git` fails transiently, and the listing is not updated
    /// (so that the failure is not mistaken for "everything is gone" and a flood of Deleted is
    /// not sent).
    pub fn changes_since_last_scan(&mut self) -> Option<RawMessage> {
        let current = self.scan()?;
        let mut changes: Vec<serde_json::Value> = Vec::new();
        for (path, mtime) in &current {
            match self.snapshot.get(path) {
                None => changes.push(change(path, CREATED)),
                Some(previous) if previous != mtime => changes.push(change(path, CHANGED)),
                Some(_) => {}
            }
        }
        for path in self.snapshot.keys() {
            if !current.contains_key(path) {
                changes.push(change(path, DELETED));
            }
        }
        self.snapshot = current;
        if changes.is_empty() {
            return None;
        }
        Some(RawMessage {
            body: serde_json::to_vec(&serde_json::json!({
                "jsonrpc": "2.0",
                "method": "workspace/didChangeWatchedFiles",
                "params": {"changes": changes},
            }))
            .expect("the notification is always serializable"),
        })
    }

    /// The listing of all roots. `None` if `git ls-files` fails on any root.
    fn scan(&self) -> Option<BTreeMap<PathBuf, SystemTime>> {
        let mut files = BTreeMap::new();
        for root in &self.roots {
            for relative in list_files(root)? {
                let path = root.join(relative);
                if let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) {
                    files.insert(path, mtime);
                }
            }
        }
        Some(files)
    }
}

fn change(path: &Path, kind: u8) -> serde_json::Value {
    serde_json::json!({"uri": path_to_uri(path), "type": kind})
}

/// The listing from `git ls-files --cached --others --exclude-standard -z`. `None` if git does
/// not run or the root is not a work tree.
fn list_files(root: &Path) -> Option<Vec<PathBuf>> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|entry| !entry.is_empty())
            .map(|entry| PathBuf::from(String::from_utf8_lossy(entry).into_owned()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("lsp-det-watched-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// Measurement of the cost (ADR 0015). On a large git-managed workspace, prints the time
    /// taken by the first listing and by the second scan (no changes).
    #[test]
    #[ignore = "needs reference/zed (1935 files). Local only"]
    fn measures_the_scan_cost_on_a_large_workspace() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("reference/zed");
        let started = std::time::Instant::now();
        let mut watched = WatchedFiles::new(std::slice::from_ref(&root)).expect("under git");
        let first = started.elapsed();
        let files = watched.snapshot.len();
        let started = std::time::Instant::now();
        assert!(watched.changes_since_last_scan().is_none());
        let second = started.elapsed();
        eprintln!("watched_files: {files} files; first scan {first:?}; rescan {second:?}");
    }

    #[test]
    fn a_directory_without_git_is_not_watched() {
        let root = temp_root("nogit");
        assert!(WatchedFiles::new(std::slice::from_ref(&root)).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reports_created_changed_and_deleted_files_once() {
        let root = temp_root("git");
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .current_dir(&root)
                .status()
                .unwrap()
                .success()
        );
        std::fs::write(root.join("a.rs"), "a").unwrap();
        let mut watched = WatchedFiles::new(std::slice::from_ref(&root)).expect("under git");
        assert!(watched.changes_since_last_scan().is_none(), "no changes");

        std::fs::write(root.join("b.rs"), "b").unwrap();
        let notification = watched.changes_since_last_scan().expect("Created");
        let value: serde_json::Value = serde_json::from_slice(&notification.body).unwrap();
        assert_eq!(value["method"], "workspace/didChangeWatchedFiles");
        assert_eq!(value["params"]["changes"][0]["type"], 1);
        assert!(
            value["params"]["changes"][0]["uri"]
                .as_str()
                .unwrap()
                .ends_with("/b.rs")
        );

        std::fs::remove_file(root.join("b.rs")).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&watched.changes_since_last_scan().unwrap().body).unwrap();
        assert_eq!(value["params"]["changes"][0]["type"], 3);
        let _ = std::fs::remove_dir_all(&root);
    }
}
