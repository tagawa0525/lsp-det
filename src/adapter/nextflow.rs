//! The mapping for Nextflow's language server (M12, ADR 0019 decision F;
//! research/nextflow-readiness-measurement.md).
//!
//! The server (nextflow-io/language-server, measured with the release 26.04.3) has no readiness
//! vocabulary, and what it does say is misleading:
//!
//! - **identity**: it returns no `serverInfo` and logs nothing at startup. The only thing in
//!   the protocol that names it is `executeCommandProvider.commands` in `InitializeResult`
//!   (`nextflow.server.previewDag` and three more, all prefixed `nextflow.server.`)
//! - **"Initializing"** (`$/progress`, token `"initialize"`) is not the workspace scan. It
//!   only swaps the configuration and clears the caches (5 ms). It is sent only for a
//!   `workspace/didChangeConfiguration` that differs from the server's defaults; without one
//!   the server never initializes its services and answers everything with empty results
//! - **the scan** runs on the first update after that (triggered by a `didOpen` /
//!   `didChange` / `didClose` after a 1-second debounce, or synchronously by a completion or
//!   formatting request), with no signal of its own. Until it has run, `references` answers
//!   `[]` (measured 22 empty answers over 2 seconds after the "Initializing" end). Its only
//!   visible trace is `publishDiagnostics` for **every** `*.nf` under the workspace folders
//!   (diagnostics are published even when empty). Diagnostics for a file the client did not
//!   open are not enough: the update after a `didOpen` also diagnoses the included modules
//!   of the opened script (measured). So this mapping walks the workspace folders itself, with
//!   the server's own rules (`*.nf`; a path is excluded when it equals a configured pattern or
//!   ends with `/` + pattern, for directories and files alike; symlinks are not followed), and
//!   is `ready` when every script in that set has been diagnosed after the "Initializing" end
//! - **`references` does not synchronize** with the debounced update: after a `didChange` it
//!   answers the old result for 1 second (measured 9 stale answers). Every update diagnoses
//!   the changed documents (`ScriptAstCache.analyze` keeps the changed URIs), so a `didOpen`
//!   / `didChange` / `didClose` of a document under a workspace folder predicts `indexing`
//!   until that document's diagnostics (ADR 0014 addendum decision D). A document outside
//!   every folder falls to the server's default service, which is never initialized and never
//!   parses it: nothing to predict
//! - **watched files**: `workspace/didChangeWatchedFiles` only logs. Created / Changed are
//!   never incorporated (measured over 30 seconds), so there is no prediction and
//!   `freshness.fileChanges` would be empty. A Deleted removes the script from the scan set
//!   (a file gone before the scan is never diagnosed)
//! - **a second, different configuration** re-runs "Initializing", publishes empty
//!   diagnostics for the cached files **inside** the token (the clearing), and drops the
//!   caches; the scan then needs a trigger again. Diagnostics inside the token are not counted
//! - **health**: no signal. `unknown` (spec 8.2 item 3)
//!
//! **No guarantee is declared** (`serverStateProvider: {}`). The rules above are those of
//! 26.04.3 (its source and the measurement), and 7.2 / 7.3 item 1 pass through this mapping
//! with that release, but the version is not observable in the protocol, so a guarantee
//! cannot be scoped to the versions the conformance suite passed on (spec 8.2 item 5).
//!
//! A client that never sends a differing configuration stays `initializing`; the hold log says
//! so. The mapping does not inject a configuration: ADR 0019 decision G lets an observer
//! enable a *signal*, not stand in for the server's initialization. The root fix belongs
//! upstream (initialize on `initialized`, return `serverInfo`, report the scan as progress).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{Readiness, ServerState, ServerStateProvider};
use crate::uri::uri_to_path;

/// The name this mapping is selected by (the server has no `serverInfo.name`; this is the
/// repository's name).
pub const SERVER_NAME: &str = "nextflow-language-server";
const COMMAND_PREFIX: &str = "nextflow.server.";
const PROGRESS_METHOD: &str = "$/progress";
const DIAGNOSTICS_METHOD: &str = "textDocument/publishDiagnostics";
const CONFIGURATION_METHOD: &str = "workspace/didChangeConfiguration";
const WATCHED_FILES_METHOD: &str = "workspace/didChangeWatchedFiles";
const INITIALIZING_TITLE: &str = "Initializing";
const SCRIPT_SUFFIX: &str = ".nf";
const DOCUMENT_METHODS: [&str; 3] = [
    "textDocument/didOpen",
    "textDocument/didChange",
    "textDocument/didClose",
];

/// Whether an `InitializeResult` (`result`) is that of Nextflow's language server: it declares
/// an `executeCommandProvider` whose commands are prefixed `nextflow.server.`.
pub fn is_nextflow_initialize_result(result: &Value) -> bool {
    result["capabilities"]["executeCommandProvider"]["commands"]
        .as_array()
        .is_some_and(|commands| {
            !commands.is_empty()
                && commands
                    .iter()
                    .all(|c| c.as_str().is_some_and(|c| c.starts_with(COMMAND_PREFIX)))
        })
}

#[derive(Debug, Deserialize)]
struct ProgressParams {
    token: Value,
    value: ProgressValue,
}

#[derive(Debug, Deserialize)]
struct ProgressValue {
    kind: String,
    #[serde(default)]
    title: Option<String>,
}

/// The server's exclusion rule (`nextflow.util.PathUtils.isExcluded`): the path string equals
/// a pattern, or ends with `/` + pattern. On Windows the server's paths are `\`-separated, so
/// the rule never matches there and nothing is excluded; this mirrors that rather than
/// excluding files the server scans (the scan set must not be smaller than the server's).
fn is_excluded(path: &Path, patterns: &[String]) -> bool {
    let text = path.to_string_lossy();
    patterns
        .iter()
        .any(|pattern| text.as_ref() == pattern || text.ends_with(&format!("/{pattern}")))
}

/// The server's `getWorkspaceFiles` for scripts: every `*.nf` under the folder, skipping
/// excluded directories and files, not following symlinks (`Files.walkFileTree` defaults).
fn visit(dir: &Path, exclude: &[String], scripts: &mut BTreeSet<PathBuf>) {
    if is_excluded(dir, exclude) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            visit(&path, exclude, scripts);
        } else if !is_excluded(&path, exclude) && path.to_string_lossy().ends_with(SCRIPT_SUFFIX) {
            scripts.insert(path);
        }
    }
}

fn workspace_scripts(folders: &[PathBuf], exclude: &[String]) -> BTreeSet<PathBuf> {
    let mut scripts = BTreeSet::new();
    for folder in folders {
        visit(folder, exclude, &mut scripts);
    }
    scripts
}

pub struct NextflowAdapter {
    state: ServerState,
    /// The client's `workspaceFolders`. Only these are scanned by the server (a lone `rootUri`
    /// is not).
    folders: Vec<PathBuf>,
    /// `nextflow.files.exclude` of the last configuration that carried it (the server's default
    /// is empty).
    exclude: Vec<String>,
    /// The token of the open "Initializing", if one is open.
    initializing: Option<Value>,
    /// An "Initializing" has ended at least once (the services are initialized).
    initialized: bool,
    /// Scripts the scan has yet to diagnose.
    expected: BTreeSet<PathBuf>,
    /// Documents whose parse after a `didOpen` / `didChange` / `didClose` has yet to be
    /// diagnosed.
    pending: BTreeSet<PathBuf>,
}

impl Default for NextflowAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl NextflowAdapter {
    pub fn new() -> Self {
        NextflowAdapter {
            state: ServerState::initializing(),
            folders: Vec::new(),
            exclude: Vec::new(),
            initializing: None,
            initialized: false,
            expected: BTreeSet::new(),
            pending: BTreeSet::new(),
        }
    }

    fn readiness(&self) -> Readiness {
        if self.initializing.is_some() || !self.initialized || !self.expected.is_empty() {
            Readiness::Initializing
        } else if !self.pending.is_empty() {
            Readiness::Indexing
        } else {
            Readiness::Ready
        }
    }

    /// The state after the bookkeeping, if readiness moved.
    fn moved(&mut self) -> Option<ServerState> {
        let readiness = self.readiness();
        if readiness == self.state.readiness {
            return None;
        }
        self.state.readiness = readiness;
        Some(self.state.clone())
    }

    fn under_a_folder(&self, path: &Path) -> bool {
        self.folders.iter().any(|folder| path.starts_with(folder))
    }

    fn on_progress(&mut self, params: ProgressParams) -> Option<ServerState> {
        let ProgressParams { token, value } = params;
        match value.kind.as_str() {
            "begin" if value.title.as_deref() == Some(INITIALIZING_TITLE) => {
                self.initializing = Some(token);
            }
            "end" if self.initializing.as_ref() == Some(&token) => {
                self.initializing = None;
                self.initialized = true;
                self.expected = workspace_scripts(&self.folders, &self.exclude);
            }
            _ => return None,
        }
        self.moved()
    }

    fn on_diagnostics(&mut self, uri: &str) -> Option<ServerState> {
        if self.initializing.is_some() {
            // The clearing of the cached files inside the token, not a parse.
            return None;
        }
        let path = uri_to_path(uri)?;
        self.expected.remove(&path);
        self.pending.remove(&path);
        self.moved()
    }

    fn on_configuration(&mut self, settings: &Value) {
        if let Some(patterns) = settings["nextflow"]["files"]["exclude"].as_array() {
            self.exclude = patterns
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
        }
    }

    fn on_document(&mut self, uri: &str) -> Option<ServerState> {
        let path = uri_to_path(uri)?;
        if !self.under_a_folder(&path) {
            return None;
        }
        self.pending.insert(path);
        self.moved()
    }

    fn on_watched_files(&mut self, changes: &[WatchedChange]) -> Option<ServerState> {
        for change in changes {
            // 3 = Deleted (LSP FileChangeType). Created / Changed are not incorporated.
            if change.kind == 3
                && let Some(path) = uri_to_path(&change.uri)
            {
                self.expected.remove(&path);
                self.pending.remove(&path);
            }
        }
        self.moved()
    }
}

#[derive(Deserialize)]
struct WatchedChange {
    uri: String,
    #[serde(rename = "type")]
    kind: u8,
}

impl Mapping for NextflowAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// Never a guarantee: the version is not observable (see the module documentation).
    fn guarantees(&self) -> ServerStateProvider {
        ServerStateProvider::notifications_only()
    }

    fn learn_workspace_folders(&mut self, folders: &[PathBuf]) {
        self.folders = folders.to_vec();
    }

    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() {
            return None;
        }
        match view.method()? {
            PROGRESS_METHOD => {
                #[derive(Deserialize)]
                struct Envelope {
                    params: ProgressParams,
                }
                let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
                self.on_progress(envelope.params)
            }
            DIAGNOSTICS_METHOD => {
                #[derive(Deserialize)]
                struct Params {
                    uri: String,
                }
                #[derive(Deserialize)]
                struct Envelope {
                    params: Params,
                }
                let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
                self.on_diagnostics(&envelope.params.uri)
            }
            _ => None,
        }
    }

    fn observe_client(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() {
            return None;
        }
        let method = view.method()?;
        if method == CONFIGURATION_METHOD {
            let value = serde_json::from_slice::<Value>(body).ok()?;
            self.on_configuration(&value["params"]["settings"]);
            None
        } else if DOCUMENT_METHODS.contains(&method) {
            #[derive(Deserialize)]
            struct Document {
                uri: String,
            }
            #[derive(Deserialize)]
            struct Params {
                #[serde(rename = "textDocument")]
                text_document: Document,
            }
            #[derive(Deserialize)]
            struct Envelope {
                params: Params,
            }
            let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
            self.on_document(&envelope.params.text_document.uri)
        } else if method == WATCHED_FILES_METHOD {
            #[derive(Deserialize)]
            struct Params {
                changes: Vec<WatchedChange>,
            }
            #[derive(Deserialize)]
            struct Envelope {
                params: Params,
            }
            let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
            self.on_watched_files(&envelope.params.changes)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;
    use crate::uri::path_to_uri;

    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "lsp-det-nextflow-adapter-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("modules")).unwrap();
            std::fs::create_dir_all(root.join("work/ab")).unwrap();
            std::fs::write(root.join("main.nf"), "").unwrap();
            std::fs::write(root.join("modules/greet.nf"), "").unwrap();
            std::fs::write(root.join("nextflow.config"), "").unwrap();
            std::fs::write(root.join("work/ab/stale.nf"), "").unwrap();
            Fixture { root }
        }

        fn file(&self, name: &str) -> PathBuf {
            self.root.join(name)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn feed(adapter: &mut NextflowAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    fn observe(adapter: &mut NextflowAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.observe_client(&view, body.as_bytes())
    }

    fn progress(kind: &str) -> String {
        let title = if kind == "begin" {
            r#","title":"Initializing""#
        } else {
            ""
        };
        format!(
            r#"{{"jsonrpc":"2.0","method":"$/progress","params":{{"token":"initialize","value":{{"kind":"{kind}"{title}}}}}}}"#
        )
    }

    fn diagnostics(path: &Path) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{{"uri":"{}","diagnostics":[]}}}}"#,
            path_to_uri(path)
        )
    }

    fn configuration(exclude: &[&str]) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"workspace/didChangeConfiguration","params":{{"settings":{{"nextflow":{{"errorReportingMode":"errors","files":{{"exclude":{}}}}}}}}}}}"#,
            serde_json::to_string(exclude).unwrap()
        )
    }

    fn document(method: &str, path: &Path) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/{method}","params":{{"textDocument":{{"uri":"{}"}}}}}}"#,
            path_to_uri(path)
        )
    }

    fn adapter_for(fixture: &Fixture) -> NextflowAdapter {
        let mut adapter = NextflowAdapter::new();
        adapter.learn_workspace_folders(std::slice::from_ref(&fixture.root));
        adapter
    }

    #[test]
    fn recognizes_the_server_by_its_commands() {
        let result = serde_json::json!({"capabilities": {"executeCommandProvider": {
            "commands": ["nextflow.server.previewDag", "nextflow.server.previewWorkspace"]
        }}});
        assert!(is_nextflow_initialize_result(&result));
        let other = serde_json::json!({"capabilities": {"executeCommandProvider": {
            "commands": ["nextflow.server.previewDag", "rust-analyzer.runSingle"]
        }}});
        assert!(!is_nextflow_initialize_result(&other));
        assert!(!is_nextflow_initialize_result(
            &serde_json::json!({"capabilities": {"executeCommandProvider": {"commands": []}}})
        ));
        assert!(!is_nextflow_initialize_result(
            &serde_json::json!({"capabilities": {}})
        ));
    }

    #[test]
    fn excludes_like_the_server() {
        let patterns = vec!["work".to_string(), "stale.nf".to_string()];
        assert!(is_excluded(Path::new("/p/work"), &patterns));
        assert!(is_excluded(Path::new("/p/a/work"), &patterns));
        assert!(!is_excluded(Path::new("/p/workspace"), &patterns));
        assert!(!is_excluded(Path::new("/p/work/x"), &patterns));
        assert!(is_excluded(Path::new("/p/m/stale.nf"), &patterns));
        assert!(!is_excluded(Path::new("/p/m/x.nf"), &[]));
    }

    /// Whether `work/ab/stale.nf` is left out by the exclude pattern `work` on this platform
    /// (the server's rule matches `/`-separated paths only).
    fn work_is_excluded_here() -> bool {
        !cfg!(windows)
    }

    #[test]
    fn walks_the_scripts_under_the_folders_minus_the_excludes() {
        let fixture = Fixture::new("walk");
        let all = workspace_scripts(std::slice::from_ref(&fixture.root), &[]);
        assert_eq!(
            all,
            BTreeSet::from([
                fixture.file("main.nf"),
                fixture.file("modules/greet.nf"),
                fixture.file("work/ab/stale.nf"),
            ])
        );
        let excluded =
            workspace_scripts(std::slice::from_ref(&fixture.root), &["work".to_string()]);
        let mut expected =
            BTreeSet::from([fixture.file("main.nf"), fixture.file("modules/greet.nf")]);
        if !work_is_excluded_here() {
            expected.insert(fixture.file("work/ab/stale.nf"));
        }
        assert_eq!(excluded, expected);
        assert!(workspace_scripts(&[], &[]).is_empty());
    }

    #[test]
    fn ready_only_after_every_script_of_the_scan_is_diagnosed() {
        let fixture = Fixture::new("scan");
        let mut m = adapter_for(&fixture);
        assert!(observe(&mut m, &configuration(&["work"])).is_none());
        assert!(
            feed(&mut m, &progress("begin")).is_none(),
            "already initializing"
        );
        // The clearing inside the token is not a parse.
        assert!(feed(&mut m, &diagnostics(&fixture.file("main.nf"))).is_none());
        assert!(
            feed(&mut m, &progress("end")).is_none(),
            "nothing scanned yet"
        );
        assert!(feed(&mut m, &diagnostics(&fixture.file("modules/greet.nf"))).is_none());
        if !work_is_excluded_here() {
            assert!(feed(&mut m, &diagnostics(&fixture.file("main.nf"))).is_none());
            assert_eq!(
                feed(&mut m, &diagnostics(&fixture.file("work/ab/stale.nf")))
                    .expect("the last script completes the scan")
                    .readiness,
                Readiness::Ready
            );
        } else {
            assert_eq!(
                feed(&mut m, &diagnostics(&fixture.file("main.nf")))
                    .expect("the last script completes the scan")
                    .readiness,
                Readiness::Ready
            );
        }
        // A later configuration that differs restarts everything, including the walk.
        assert!(observe(&mut m, &configuration(&[])).is_none());
        assert_eq!(
            feed(&mut m, &progress("begin")).unwrap().readiness,
            Readiness::Initializing
        );
        feed(&mut m, &progress("end"));
        feed(&mut m, &diagnostics(&fixture.file("main.nf")));
        assert!(
            feed(&mut m, &diagnostics(&fixture.file("modules/greet.nf"))).is_none(),
            "work/ab/stale.nf is part of the scan without the exclude"
        );
        assert_eq!(
            feed(&mut m, &diagnostics(&fixture.file("work/ab/stale.nf")))
                .unwrap()
                .readiness,
            Readiness::Ready
        );
    }

    #[test]
    fn nothing_to_scan_is_ready_at_the_initializing_end() {
        let mut m = NextflowAdapter::new();
        feed(&mut m, &progress("begin"));
        assert_eq!(
            feed(&mut m, &progress("end")).unwrap().readiness,
            Readiness::Ready
        );
    }

    #[test]
    fn a_document_notification_predicts_indexing_until_its_diagnostics() {
        let fixture = Fixture::new("predict");
        let mut m = adapter_for(&fixture);
        feed(&mut m, &progress("begin"));
        feed(&mut m, &progress("end"));
        feed(&mut m, &diagnostics(&fixture.file("main.nf")));
        feed(&mut m, &diagnostics(&fixture.file("modules/greet.nf")));
        feed(&mut m, &diagnostics(&fixture.file("work/ab/stale.nf")));
        assert_eq!(m.state.readiness, Readiness::Ready);
        for method in ["didOpen", "didChange", "didClose"] {
            assert_eq!(
                observe(&mut m, &document(method, &fixture.file("main.nf")))
                    .unwrap()
                    .readiness,
                Readiness::Indexing,
                "{method}"
            );
            assert_eq!(
                feed(&mut m, &diagnostics(&fixture.file("main.nf")))
                    .unwrap()
                    .readiness,
                Readiness::Ready,
                "{method}"
            );
        }
        // Outside every folder: the server never parses it.
        assert!(
            observe(
                &mut m,
                &document("didOpen", &std::env::temp_dir().join("elsewhere.nf"))
            )
            .is_none()
        );
    }

    #[test]
    fn a_deleted_script_leaves_the_scan_and_other_watched_changes_do_nothing() {
        let fixture = Fixture::new("watched");
        let mut m = adapter_for(&fixture);
        observe(&mut m, &configuration(&["work"]));
        feed(&mut m, &progress("begin"));
        feed(&mut m, &progress("end"));
        feed(&mut m, &diagnostics(&fixture.file("main.nf")));
        if !work_is_excluded_here() {
            feed(&mut m, &diagnostics(&fixture.file("work/ab/stale.nf")));
        }
        let greet = path_to_uri(&fixture.file("modules/greet.nf"));
        let created = path_to_uri(&fixture.file("c.nf"));
        assert!(
            observe(
                &mut m,
                &format!(
                    r#"{{"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{{"changes":[{{"uri":"{created}","type":1}},{{"uri":"{greet}","type":2}}]}}}}"#
                )
            )
            .is_none()
        );
        assert_eq!(
            observe(
                &mut m,
                &format!(
                    r#"{{"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{{"changes":[{{"uri":"{greet}","type":3}}]}}}}"#
                )
            )
            .unwrap()
            .readiness,
            Readiness::Ready
        );
    }

    #[test]
    fn never_declares_a_guarantee_and_has_no_health() {
        let m = NextflowAdapter::new();
        assert_eq!(m.guarantees(), ServerStateProvider::notifications_only());
        assert_eq!(m.initial_state(), ServerState::initializing());
    }
}
