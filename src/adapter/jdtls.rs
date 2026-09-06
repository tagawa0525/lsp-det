//! The mapping for jdtls (Eclipse JDT Language Server) (M23, ADR 0020 decision C row for jdtls;
//! research/jdtls-readiness-measurement.md).
//!
//! Identified by `serverInfo.name` "JDT Language Server (Standard)"; the version is
//! `serverInfo.version` (nixpkgs 1.60.0 calls itself "1.60.0-SNAPSHOT").
//!
//! - **readiness**: starts `initializing`. `language/status` (`{type, message}`, the
//!   `ServiceStatus` enum) `type: "ServiceReady"` -> `ready`. `$/progress` is not mapped:
//!   "Building" is compilation for diagnostics, not the index, and JDT's search itself waits
//!   for the index with `WAIT_UNTIL_READY_TO_SEARCH` (`BasicSearchEngine.findMatches`), so
//!   mapping progress would only delay a result that is already complete once `ServiceReady`
//!   fires
//! - **no prediction** (`observe_client` is not implemented): the server itself holds a
//!   request until the index it depends on is ready (measured: a `references` sent at 1.04s,
//!   before `ServiceReady` at 1.12s, answers complete only once the search can run), so there
//!   is nothing for the observer to predict from `textDocument/didChange` or
//!   `workspace/didChangeWatchedFiles`
//! - **health**: `language/status` `type: "ProjectStatus"` message "OK" -> `ok`, "WARNING" ->
//!   `warning` (`ProjectsManager.reportProjectsStatus`: the maximum severity of the project's
//!   problem markers); `type: "Error"` -> `error`. "WARNING" was not observed in the measured
//!   fixture (`reportProjectsStatus` runs before the build marks a broken classpath).
//!   Additionally, `textDocument/publishDiagnostics` on a URI that does not end with ".java"
//!   (the project resource itself or a build file; measured: a missing library in `.classpath`
//!   shows as severity 1 "Project 'x' is missing required library: '…'" on the project's own
//!   URI) with a severity-1 diagnostic -> `warning`, reverting to whatever `ProjectStatus` /
//!   `Error` last reported once that URI's diagnostics are empty again. An `Error` status
//!   always wins over a project-URI diagnostic warning (a hard failure outranks "partly
//!   functional")
//!
//! `coverage` / `freshness` are declared only for versions ([`TESTED_VERSIONS`]) for which
//! conformance tests 7.1 / 7.2 / 7.3 were run against a real jdtls and passed (spec 8.2 item 5).

use std::collections::BTreeMap;

use serde::Deserialize;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{ALL_FILE_CHANGES, Health, Readiness, ServerState, ServerStateProvider};

/// The name jdtls calls itself in `InitializeResult.serverInfo.name`, already lowercased for
/// the case-insensitive comparison [`super::select`] does.
pub const SERVER_NAME: &str = "jdt language server (standard)";

const STATUS_METHOD: &str = "language/status";
const DIAGNOSTICS_METHOD: &str = "textDocument/publishDiagnostics";
/// LSP `DiagnosticSeverity.Error`.
const SEVERITY_ERROR: u8 = 1;
const JAVA_EXTENSION: &str = ".java";

/// Versions for which conformance tests 7.1 / 7.2 / 7.3 were run against a real jdtls and
/// passed. Matched by exact equality against `serverInfo.version`. No guarantee is declared for
/// a version not in the list. When adding one, run
/// `cargo test --test conformance -- --ignored jdtls` against that version first (declaring a
/// guarantee that cannot be kept violates spec 5.1).
///
/// Record of versions passed: 1.60.0-SNAPSHOT (nixpkgs `jdt-language-server` 1.60.0, flake.nix
/// `servers`), 2026-09-06.
pub const TESTED_VERSIONS: &[&str] = &["1.60.0-SNAPSHOT"];

#[derive(Deserialize)]
struct StatusParams {
    #[serde(rename = "type")]
    status_type: String,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct Diagnostic {
    #[serde(default)]
    severity: Option<u8>,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct DiagnosticsParams {
    uri: String,
    #[serde(default)]
    diagnostics: Vec<Diagnostic>,
}

pub struct JdtlsAdapter {
    /// Whether the announced version is in [`TESTED_VERSIONS`]. The condition for declaring a
    /// guarantee.
    version_is_tested: bool,
    state: ServerState,
    /// What the last `ProjectStatus` / `Error` `language/status` reported. `health` reverts to
    /// this once every project-URI diagnostic warning clears.
    status_health: Health,
    status_message: Option<String>,
    /// Non-`.java` URIs (the project resource, a build file) whose latest diagnostics carry a
    /// severity-1 problem, with its message.
    project_diagnostic_warnings: BTreeMap<String, String>,
}

impl Default for JdtlsAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl JdtlsAdapter {
    /// For a jdtls that does not announce a version. Declares no guarantee.
    pub fn new() -> Self {
        Self::for_version(None)
    }

    /// Looks at `serverInfo.version` and declares a guarantee if it is a tested version.
    pub fn for_version(version: Option<&str>) -> Self {
        let version_is_tested = version.is_some_and(|v| TESTED_VERSIONS.contains(&v));
        JdtlsAdapter {
            version_is_tested,
            state: ServerState::initializing(),
            status_health: Health::Unknown,
            status_message: None,
            project_diagnostic_warnings: BTreeMap::new(),
        }
    }

    /// Recomputes `health` from `status_health` and `project_diagnostic_warnings`, and emits
    /// the new state if it changed. `Error` from `language/status` always wins over a
    /// project-URI diagnostic warning.
    fn recompute_health(&mut self) -> Option<ServerState> {
        let (health, message) = if self.status_health == Health::Error {
            (Health::Error, self.status_message.clone())
        } else if let Some(message) = self.project_diagnostic_warnings.values().next() {
            (Health::Warning, Some(message.clone()))
        } else {
            (self.status_health, self.status_message.clone())
        };
        let next = ServerState {
            health,
            readiness: self.state.readiness,
            message,
        };
        if next == self.state {
            return None;
        }
        self.state = next;
        Some(self.state.clone())
    }

    fn on_status(&mut self, params: StatusParams) -> Option<ServerState> {
        match params.status_type.as_str() {
            "ServiceReady" => {
                if self.state.readiness == Readiness::Ready {
                    return None;
                }
                self.state.readiness = Readiness::Ready;
                Some(self.state.clone())
            }
            "ProjectStatus" => {
                self.status_health = match params.message.as_str() {
                    "OK" => Health::Ok,
                    "WARNING" => Health::Warning,
                    // An unrecognized ProjectStatus message: nothing to read from it.
                    _ => return None,
                };
                self.status_message =
                    (self.status_health == Health::Warning).then(|| params.message.clone());
                self.recompute_health()
            }
            "Error" => {
                self.status_health = Health::Error;
                self.status_message = Some(params.message);
                self.recompute_health()
            }
            // Starting, Started, Message: nothing this mapping reads from.
            _ => None,
        }
    }

    fn on_diagnostics(&mut self, params: DiagnosticsParams) -> Option<ServerState> {
        if params.uri.to_ascii_lowercase().ends_with(JAVA_EXTENSION) {
            // A regular source file's own diagnostics are not the project-URI health signal.
            return None;
        }
        let failure = params
            .diagnostics
            .iter()
            .find(|d| d.severity == Some(SEVERITY_ERROR));
        match failure {
            Some(d) => {
                self.project_diagnostic_warnings
                    .insert(params.uri.clone(), d.message.clone());
            }
            None => {
                self.project_diagnostic_warnings.remove(&params.uri);
            }
        }
        self.recompute_health()
    }
}

impl Mapping for JdtlsAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// The guarantee to declare (spec chapter 5). Declared only for [`TESTED_VERSIONS`] (spec
    /// 8.2 item 5): the server holds a request until the index it depends on is ready (verified
    /// by conformance tests 7.1 / 7.2 against a real server), and incorporates `didChange` and
    /// on-disk changes (verified by 7.3).
    fn guarantees(&self) -> ServerStateProvider {
        if self.version_is_tested {
            ServerStateProvider::workspace(&[], &ALL_FILE_CHANGES)
        } else {
            ServerStateProvider::notifications_only()
        }
    }

    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() {
            return None;
        }
        match view.method() {
            Some(STATUS_METHOD) => {
                #[derive(Deserialize)]
                struct Envelope {
                    params: StatusParams,
                }
                let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
                self.on_status(envelope.params)
            }
            Some(DIAGNOSTICS_METHOD) => {
                #[derive(Deserialize)]
                struct Envelope {
                    params: DiagnosticsParams,
                }
                let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
                self.on_diagnostics(envelope.params)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;

    fn feed(adapter: &mut JdtlsAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    fn status(status_type: &str, message: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"language/status","params":{{"type":"{status_type}","message":"{message}"}}}}"#
        )
    }

    fn diagnostics(uri: &str, items: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{{"uri":"{uri}","diagnostics":[{items}]}}}}"#
        )
    }

    const MISSING_LIBRARY: &str = r#"{"severity":1,"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"message":"Project 'x' is missing required library: 'missing.jar'"}"#;
    const CANNOT_FIND_SYMBOL: &str = r#"{"severity":1,"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"message":"cannot find symbol"}"#;

    #[test]
    fn starts_initializing() {
        let adapter = JdtlsAdapter::new();
        assert_eq!(adapter.initial_state().readiness, Readiness::Initializing);
        assert_eq!(adapter.initial_state().health, Health::Unknown);
    }

    #[test]
    fn service_ready_moves_readiness_to_ready() {
        let mut m = JdtlsAdapter::new();
        let state = feed(&mut m, &status("ServiceReady", "ServiceReady"))
            .expect("ServiceReady is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn other_status_types_do_not_move_readiness() {
        let mut m = JdtlsAdapter::new();
        assert!(feed(&mut m, &status("Starting", "Init...")).is_none());
        assert!(feed(&mut m, &status("Started", "Ready")).is_none());
        assert!(feed(&mut m, &status("Message", "some message")).is_none());
        assert_eq!(m.initial_state().readiness, Readiness::Initializing);
    }

    #[test]
    fn project_status_ok_and_warning_move_health() {
        let mut m = JdtlsAdapter::new();
        let state = feed(&mut m, &status("ProjectStatus", "OK")).expect("OK is a signal");
        assert_eq!(state.health, Health::Ok);
        assert_eq!(state.readiness, Readiness::Initializing);
        let state = feed(&mut m, &status("ProjectStatus", "WARNING")).expect("WARNING is a signal");
        assert_eq!(state.health, Health::Warning);
    }

    #[test]
    fn error_status_moves_health_to_error_and_wins_over_a_project_diagnostic_warning() {
        let mut m = JdtlsAdapter::new();
        feed(&mut m, &status("ProjectStatus", "OK"));
        feed(&mut m, &diagnostics("file:///fixture", MISSING_LIBRARY));
        let state = feed(&mut m, &status("Error", "internal error")).expect("Error is a signal");
        assert_eq!(state.health, Health::Error);
        assert_eq!(state.message.as_deref(), Some("internal error"));
    }

    #[test]
    fn non_java_diagnostics_with_severity_one_are_warning_and_revert() {
        let mut m = JdtlsAdapter::new();
        feed(&mut m, &status("ProjectStatus", "OK"));
        let state = feed(&mut m, &diagnostics("file:///fixture", MISSING_LIBRARY))
            .expect("a project-URI severity-1 diagnostic moves health");
        assert_eq!(state.health, Health::Warning);
        assert_eq!(
            state.message.as_deref(),
            Some("Project 'x' is missing required library: 'missing.jar'")
        );
        let state = feed(&mut m, &diagnostics("file:///fixture", ""))
            .expect("the diagnostic clearing reverts health");
        assert_eq!(state.health, Health::Ok);
    }

    #[test]
    fn java_diagnostics_are_ignored() {
        let mut m = JdtlsAdapter::new();
        feed(&mut m, &status("ProjectStatus", "OK"));
        assert!(
            feed(
                &mut m,
                &diagnostics("file:///fixture/src/app/F0.java", CANNOT_FIND_SYMBOL)
            )
            .is_none(),
            "a .java file's own diagnostics must not move health"
        );
    }

    #[test]
    fn progress_is_ignored() {
        let mut m = JdtlsAdapter::new();
        assert!(
            feed(
                &mut m,
                r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":"some-uuid","value":{"kind":"begin","title":"Building"}}}"#
            )
            .is_none()
        );
        assert_eq!(m.initial_state().readiness, Readiness::Initializing);
    }

    #[test]
    fn health_and_readiness_changes_preserve_each_other() {
        let mut m = JdtlsAdapter::new();
        let state = feed(&mut m, &status("ServiceReady", "ServiceReady")).unwrap();
        assert_eq!(state.readiness, Readiness::Ready);
        let state = feed(&mut m, &status("ProjectStatus", "WARNING")).unwrap();
        assert_eq!(state.health, Health::Warning);
        assert_eq!(
            state.readiness,
            Readiness::Ready,
            "a health change must not move readiness"
        );
        // The reverse: a readiness change (there is none after ServiceReady in this mapping,
        // but a repeated ServiceReady must not disturb health either).
        assert!(
            feed(&mut m, &status("ServiceReady", "ServiceReady")).is_none(),
            "an unchanged ServiceReady must not re-notify"
        );
    }

    #[test]
    fn declares_a_guarantee_only_for_the_tested_version() {
        let tested = JdtlsAdapter::for_version(Some("1.60.0-SNAPSHOT"));
        assert_eq!(
            tested.guarantees(),
            ServerStateProvider::workspace(&[], &ALL_FILE_CHANGES)
        );
        let untested = JdtlsAdapter::for_version(Some("1.59.0-SNAPSHOT"));
        assert_eq!(
            untested.guarantees(),
            ServerStateProvider::notifications_only()
        );
        let unversioned = JdtlsAdapter::new();
        assert_eq!(
            unversioned.guarantees(),
            ServerStateProvider::notifications_only()
        );
    }
}
