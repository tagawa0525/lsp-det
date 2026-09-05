//! The rust-analyzer mapping (v0.1-design.md 5.1).
//!
//! rust-analyzer sends `{health, quiescent, message}` in an
//! `experimental/serverStatus` notification (`lsp/ext.rs`). `quiescent` is in
//! substance `is_fully_ready()` = workspace load complete and cache priming
//! not running.
//!
//! It reverts to `false` only when the workspace configuration changes
//! (`Cargo.toml`, switching branches, etc.), and **not on ordinary source
//! edits**. The measurement and its structural basis are in ADR 0007 and
//! docs/research/rust-analyzer-quiescent-measurement.md. Consequently, flap
//! countermeasures (smoothing, debouncing) are unnecessary.
//!
//! Failure arrives via `health`. A workspace load failure is
//! `{health: error, quiescent: true}` (`current_status()`). Per spec chapter
//! 6 item 5, it is mapped onto `health`, not `readiness`.

use serde::Deserialize;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{ALL_FILE_CHANGES, Health, Readiness, ServerState, ServerStateProvider};

/// The method name of the readiness notification rust-analyzer sends.
pub const SERVER_STATUS_METHOD: &str = "experimental/serverStatus";

/// The params of `experimental/serverStatus`.
///
/// `health` is received as a dedicated enum rather than `state::Health` because spec 8.1 states
/// that "a server must not send `unknown`." If the upstream sends it anyway, parsing fails and
/// the state does not change.
#[derive(Debug, Deserialize)]
struct ServerStatusParams {
    health: UpstreamHealth,
    quiescent: bool,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum UpstreamHealth {
    Ok,
    Warning,
    Error,
}

impl From<UpstreamHealth> for Health {
    fn from(value: UpstreamHealth) -> Self {
        match value {
            UpstreamHealth::Ok => Health::Ok,
            UpstreamHealth::Warning => Health::Warning,
            UpstreamHealth::Error => Health::Error,
        }
    }
}

/// Versions for which conformance tests 7.2 / 7.3 were run against a real rust-analyzer and
/// passed. Matched by exact equality against the leading token (before any whitespace) of
/// `serverInfo.version`.
///
/// The rustup distribution calls itself `1.98.0 (88d9e12 2026-08-18)`, while the nixpkgs build
/// calls itself `2026-08-03`. Since the formats differ, these are not interpreted as semver;
/// the identity strings are kept in the list as-is. lsp-det cannot guarantee rust-analyzer's
/// internals; it only has the observation that a test passed (spec 8.2 item 5). No guarantee is
/// declared for a version not in the list. When adding one, run
/// `cargo test --test conformance -- --ignored` against that version first (declaring a
/// guarantee that cannot be kept violates spec 5.1).
///
/// Record of versions passed:
/// - `1.98.0 (88d9e12 2026-08-18)` (rustup stable), 2026-08-29 and 2026-09-03
/// - `2026-08-03` (nixpkgs, flake.nix dev environment), 2026-09-03
pub const TESTED_VERSIONS: &[&str] = &["1.98.0", "2026-08-03"];

/// The leading token of `serverInfo.version`. Drops a trailing hash or date.
fn leading_token(version: &str) -> &str {
    version.split_whitespace().next().unwrap_or("")
}

/// The message rust-analyzer attaches to `warning` when it finds no project at all
/// (`current_status()` in `reload.rs`). This is the only thing available to distinguish it, so
/// it is matched as a string. Fragile, but kept within the range of [`TESTED_VERSIONS`].
const MISSING_WORKSPACE_MESSAGE: &str = "Failed to discover workspace.";

pub struct RustAnalyzerAdapter {
    /// Whether the announced version is in [`TESTED_VERSIONS`]. The condition for declaring a
    /// guarantee.
    version_is_tested: bool,
    /// Whether an unparseable status has already been logged once (to avoid repeated logging).
    warned_unparseable: bool,
    /// The cap on `workspace/symbol`. Defaults to 128. Changed by the client's
    /// `initializationOptions.workspace.symbol.search.limit`.
    workspace_symbol_limit: u64,
    /// The last health read. Used when predicting from a notification moves only readiness.
    last_health: Health,
}

/// The default cap rust-analyzer has for `workspace/symbol` (`workspace_symbol_search_limit`
/// in `config.rs`).
const DEFAULT_WORKSPACE_SYMBOL_LIMIT: u64 = 128;

/// Whether this is a file rust-analyzer registers for watching via
/// `client/registerCapability` (`**/*.rs`, `**/Cargo.{toml,lock}`, `**/rust-analyzer.toml`).
/// A Created / Deleted event for one of these is always followed by `quiescent: false → true`
/// (per the addendum to research/disk-edit-propagation-measurement.md).
fn is_watched_file(uri: &str) -> bool {
    // Judged by the last component of the URI. A Windows file URI can arrive `\\`-separated.
    let name = uri.rsplit(['/', '\\']).next().unwrap_or(uri);
    name.ends_with(".rs") || matches!(name, "Cargo.toml" | "Cargo.lock" | "rust-analyzer.toml")
}

impl Default for RustAnalyzerAdapter {
    fn default() -> Self {
        Self::for_version(None)
    }
}

impl RustAnalyzerAdapter {
    /// For a rust-analyzer that does not announce a version (or whose version cannot be read).
    /// Declares no guarantee.
    pub fn new() -> Self {
        Self::default()
    }

    /// Looks at `serverInfo.version` and declares a guarantee if it is a tested version.
    pub fn for_version(version: Option<&str>) -> Self {
        let version_is_tested =
            version.is_some_and(|v| TESTED_VERSIONS.contains(&leading_token(v)));
        RustAnalyzerAdapter {
            version_is_tested,
            warned_unparseable: false,
            workspace_symbol_limit: DEFAULT_WORKSPACE_SYMBOL_LIMIT,
            last_health: Health::Unknown,
        }
    }

    /// Whether the announced version is within the range the conformance tests have passed on.
    pub fn version_is_tested(&self) -> bool {
        self.version_is_tested
    }
}

impl Mapping for RustAnalyzerAdapter {
    /// The state right after connecting to the upstream. rust-analyzer reports nothing until it
    /// sends its first `serverStatus`, after the `initialize` response.
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// The guarantee to declare in `InitializeResult` (spec chapter 5).
    ///
    /// rust-analyzer satisfies both guarantees. This has been confirmed by running conformance
    /// test suite specs 7.2 (completeness) and 7.3 (cross-file freshness) against a real
    /// rust-analyzer (the two `#[ignore]`d tests in tests/conformance.rs). However, it can be
    /// declared only for versions the tests have passed on ([`TESTED_VERSIONS`]) (spec 8.2 item
    /// 5). Outside that range, only state notifications are promised.
    fn guarantees(&self) -> ServerStateProvider {
        if self.version_is_tested {
            ServerStateProvider::workspace(
                &[("workspace/symbol", self.workspace_symbol_limit)],
                &ALL_FILE_CHANGES,
            )
        } else {
            ServerStateProvider::notifications_only()
        }
    }

    fn learn_initialization_options(&mut self, options: &serde_json::Value) {
        if let Some(limit) = options["workspace"]["symbol"]["search"]["limit"].as_u64() {
            self.workspace_symbol_limit = limit;
        }
    }

    /// Predicts `indexing` from a Created / Deleted notification (ADR 0014 addendum decision
    /// D). Only takes effect for watched files. A Changed event is not followed by a signal
    /// (the in-flight request is simply refused with -32801), so it is not predicted from.
    fn observe_client(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() || view.method() != Some("workspace/didChangeWatchedFiles") {
            return None;
        }
        let changes = parse_watched_file_changes(body)?;
        // FileChangeType: 1 = Created, 2 = Changed, 3 = Deleted.
        let reindexes = changes
            .iter()
            .any(|change| matches!(change.kind, 1 | 3) && is_watched_file(&change.uri));
        reindexes.then_some(ServerState {
            health: self.last_health,
            readiness: Readiness::Indexing,
            message: None,
        })
    }

    /// Reads the state the upstream is reporting from an upstream-to-client message. `None`
    /// (the state does not move) for anything other than `experimental/serverStatus`, and for
    /// an unreadable status.
    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() || view.method() != Some(SERVER_STATUS_METHOD) {
            return None;
        }

        let Some(params) = parse_status_params(body) else {
            // A status of unknown shape does not move the state. Keeping the previous state is
            // safer than letting one broken message wrongly advance readiness.
            //
            // But it must not be silently dropped. If the upstream changes the shape of params,
            // every message becomes unreadable and the state freezes at its last value. The
            // downstream side then keeps holding cross-workspace requests (there is no time
            // limit on holding), which cannot be diagnosed without a reason in the log. Logged
            // once to avoid repeated logging.
            if !self.warned_unparseable {
                self.warned_unparseable = true;
                eprintln!(
                    "lsp-det: cannot parse {SERVER_STATUS_METHOD} params; \
                     keeping the previous state (further occurrences are not logged)"
                );
            }
            return None;
        };

        // Compensates for the coarseness of the vocabulary (design 5.1). Since a missing
        // project makes cross-workspace queries non-functional, rust-analyzer's warning is
        // mapped to error.
        let mut health: Health = params.health.into();
        if health == Health::Warning
            && params
                .message
                .as_deref()
                .is_some_and(|m| m.contains(MISSING_WORKSPACE_MESSAGE))
        {
            health = Health::Error;
        }

        self.last_health = health;
        Some(ServerState {
            health,
            readiness: if params.quiescent {
                Readiness::Ready
            } else {
                Readiness::Indexing
            },
            message: params.message,
        })
    }
}

struct WatchedFileChange {
    uri: String,
    /// The value of LSP's `FileChangeType` (1..=3). A change with a value outside this range is
    /// dropped.
    kind: u64,
}

/// The `changes` (uri and FileChangeType) of `workspace/didChangeWatchedFiles`.
fn parse_watched_file_changes(body: &[u8]) -> Option<Vec<WatchedFileChange>> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let changes = value["params"]["changes"].as_array()?;
    Some(
        changes
            .iter()
            .filter_map(|change| {
                Some(WatchedFileChange {
                    uri: change["uri"].as_str()?.to_string(),
                    kind: change["type"]
                        .as_u64()
                        .filter(|kind| (1..=3).contains(kind))?,
                })
            })
            .collect(),
    )
}

/// Extracts `params` and reads it as `ServerStatusParams`.
/// A missing `params`, a type mismatch, or an unknown `health` value all result in `None`.
fn parse_status_params(body: &[u8]) -> Option<ServerStatusParams> {
    #[derive(Deserialize)]
    struct Envelope {
        params: ServerStatusParams,
    }

    serde_json::from_slice::<Envelope>(body)
        .ok()
        .map(|envelope| envelope.params)
}

#[cfg(test)]
mod tests {
    #[test]
    fn watched_files_are_recognised_with_either_path_separator() {
        assert!(super::is_watched_file("file:///w/src/c.rs"));
        assert!(super::is_watched_file("file:///C:/w/Cargo.toml"));
        assert!(super::is_watched_file("file:///C:\\w\\Cargo.lock"));
        assert!(!super::is_watched_file("file:///w/notes.txt"));
        assert!(!super::is_watched_file(
            "file:///w/src/rust-analyzer.toml.bak"
        ));
    }

    use super::*;
    use crate::peek::peek;

    fn interpret(adapter: &mut RustAnalyzerAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    fn status(health: &str, quiescent: bool) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{{"health":"{health}","quiescent":{quiescent},"message":null}}}}"#
        )
    }

    #[test]
    fn declares_guarantees_for_the_nixpkgs_build_the_suite_passed_on() {
        // The nixpkgs build calls itself by a date (`2026-08-03`). 7.2 / 7.3 were run against
        // this version too and passed (flake.nix dev environment, 2026-09-03). Version
        // identification is not limited to semver; the leading token of the identity string is
        // matched against the list of tested versions.
        let tested = crate::adapter::select("rust-analyzer", Some("2026-08-03")).unwrap();
        assert_eq!(
            tested.guarantees(),
            ServerStateProvider::workspace(&[("workspace/symbol", 128)], &ALL_FILE_CHANGES)
        );
        let untested = crate::adapter::select("rust-analyzer", Some("2026-08-04")).unwrap();
        assert_eq!(
            untested.guarantees(),
            ServerStateProvider::notifications_only()
        );
    }

    #[test]
    fn maps_a_missing_workspace_warning_to_error() {
        // Design 5.1: when rust-analyzer finds no project at all, it emits warning and
        // "Failed to discover workspace." (`current_status()` in reload.rs). Cross-workspace
        // queries do not function, so this is mapped to error. The message string is the only
        // thing available to distinguish it.
        let mut adapter = RustAnalyzerAdapter::new();
        let body = r#"{"method":"experimental/serverStatus","params":{"health":"warning","quiescent":true,"message":"Failed to discover workspace.\nConsider adding the `Cargo.toml` of the workspace to the [`linkedProjects`](https://rust-analyzer.github.io/book/configuration.html#linkedProjects) setting.\n\n"}}"#;
        let state = interpret(&mut adapter, body).unwrap();
        assert_eq!(state.health, Health::Error);
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn maps_the_missing_workspace_warning_to_error_even_after_other_warnings() {
        // current_status() concatenates warning text. It is found even when not at the start.
        let mut adapter = RustAnalyzerAdapter::new();
        let body = r#"{"method":"experimental/serverStatus","params":{"health":"warning","quiescent":true,"message":"Auto-reloading is disabled and the workspace has changed, a manual workspace reload is required.\n\nFailed to discover workspace.\n"}}"#;
        assert_eq!(interpret(&mut adapter, body).unwrap().health, Health::Error);
    }

    #[test]
    fn keeps_other_warnings_as_warning() {
        let mut adapter = RustAnalyzerAdapter::new();
        let body = r#"{"method":"experimental/serverStatus","params":{"health":"warning","quiescent":true,"message":"Failed to run build scripts of some packages.\n\n"}}"#;
        assert_eq!(
            interpret(&mut adapter, body).unwrap().health,
            Health::Warning
        );
    }

    #[test]
    fn a_non_quiescent_status_means_indexing() {
        let mut adapter = RustAnalyzerAdapter::new();
        let state = interpret(&mut adapter, &status("ok", false)).expect("status is readable");
        assert_eq!(state.readiness, Readiness::Indexing);
    }

    #[test]
    fn a_quiescent_status_means_ready() {
        let mut adapter = RustAnalyzerAdapter::new();
        let state = interpret(&mut adapter, &status("ok", true)).expect("status is readable");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn carries_health_through_unchanged() {
        // Failure arrives via health (spec chapter 6 item 5). Even with error, quiescent is
        // read independently.
        for (upstream, expected) in [
            ("ok", Health::Ok),
            ("warning", Health::Warning),
            ("error", Health::Error),
        ] {
            let mut adapter = RustAnalyzerAdapter::new();
            let state = interpret(&mut adapter, &status(upstream, true)).unwrap();
            assert_eq!(state.health, expected);
            assert_eq!(state.readiness, Readiness::Ready);
        }
    }

    #[test]
    fn carries_the_human_message_through() {
        let mut adapter = RustAnalyzerAdapter::new();
        let body = r#"{"method":"experimental/serverStatus","params":{"health":"warning","quiescent":false,"message":"build scripts need rebuilding"}}"#;
        let state = interpret(&mut adapter, body).unwrap();
        assert_eq!(
            state.message.as_deref(),
            Some("build scripts need rebuilding")
        );
    }

    #[test]
    fn ignores_unrelated_notifications() {
        let mut adapter = RustAnalyzerAdapter::new();
        let progress = r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":"x","value":{"kind":"end"}}}"#;
        assert!(interpret(&mut adapter, progress).is_none());
    }

    #[test]
    fn ignores_a_request_that_happens_to_use_the_status_method_name() {
        // serverStatus is a notification, not a request.
        let mut adapter = RustAnalyzerAdapter::new();
        let as_request = r#"{"jsonrpc":"2.0","id":1,"method":"experimental/serverStatus","params":{"health":"ok","quiescent":true}}"#;
        assert!(interpret(&mut adapter, as_request).is_none());
    }

    #[test]
    fn ignores_a_status_whose_params_do_not_parse() {
        let mut adapter = RustAnalyzerAdapter::new();
        let missing_quiescent =
            r#"{"method":"experimental/serverStatus","params":{"health":"ok"}}"#;
        assert!(interpret(&mut adapter, missing_quiescent).is_none());
    }

    #[test]
    fn refuses_observer_only_health_values_claimed_by_the_upstream() {
        // Spec 8.1: a server must not send unknown. dead is not a value of this protocol
        // (spec chapter 3).
        for claimed in ["dead", "unknown"] {
            let mut adapter = RustAnalyzerAdapter::new();
            let body = format!(
                r#"{{"method":"experimental/serverStatus","params":{{"health":"{claimed}","quiescent":true}}}}"#
            );
            assert!(
                interpret(&mut adapter, &body).is_none(),
                "must not accept {claimed} from the upstream"
            );
        }
    }
}
