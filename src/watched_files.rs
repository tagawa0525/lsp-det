//! `workspace/didChangeWatchedFiles` の代行 (設計 4.3、ADR 0015 決定 A)。
//!
//! capability `workspace.didChangeWatchedFiles` を宣言せず、通知も送らない
//! クライアント (Claude Code) に代わって、7.0 のリクエストが届くたびに
//! ワークスペースのファイルの mtime を前回と比べ、差を Created / Changed /
//! Deleted として 1 つの通知にまとめて上流へ送る。時計は使わず、引き金は
//! 要求そのもの。
//!
//! 列挙は `git ls-files --cached --others --exclude-standard` に任せる
//! (追跡中 + 無視されていない未追跡)。`.gitignore` の解釈も、言語ごとの
//! 拡張子の一覧も持たない。git 管理外のルートでは代行しない。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use crate::framing::RawMessage;
use crate::uri::path_to_uri;

/// LSP の `FileChangeType`。
const CREATED: u8 = 1;
const CHANGED: u8 = 2;
const DELETED: u8 = 3;

pub struct WatchedFiles {
    /// git 管理下のルート (そうでないルートは起動時に除く)。
    roots: Vec<PathBuf>,
    /// 前回の一覧 (パス → mtime)。
    snapshot: BTreeMap<PathBuf, SystemTime>,
}

impl WatchedFiles {
    /// ルートごとに `git ls-files` を試し、使えるルートで最初の一覧を取る。
    /// 使えるルートがなければ `None` (代行しない)。
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

    /// 前回からの差を通知にする。差がなければ `None`。`git` が一時的に失敗
    /// したときも `None` で、一覧は更新しない (失敗を「全部消えた」と
    /// 誤認して Deleted を大量に送らないため)。
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
            .expect("通知は常にシリアライズできる"),
        })
    }

    /// 全ルートの一覧。どれかのルートで `git ls-files` が失敗したら `None`。
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

/// `git ls-files --cached --others --exclude-standard -z` の一覧。git が
/// 動かない・ルートが work tree でないなら `None`。
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

    /// 費用の実測 (ADR 0015)。大きな git 管理下のワークスペースで、最初の
    /// 一覧と 2 回目の走査 (変更なし) にかかる時間を出す。
    #[test]
    #[ignore = "reference/zed (1935 ファイル) が要る。ローカル専用"]
    fn measures_the_scan_cost_on_a_large_workspace() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("reference/zed");
        let started = std::time::Instant::now();
        let mut watched = WatchedFiles::new(std::slice::from_ref(&root)).expect("git 管理下");
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
        let mut watched = WatchedFiles::new(std::slice::from_ref(&root)).expect("git 管理下");
        assert!(watched.changes_since_last_scan().is_none(), "変更なし");

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
