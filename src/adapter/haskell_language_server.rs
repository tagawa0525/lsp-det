//! The mapping for haskell-language-server (M15, ADR 0019 decision F;
//! research/haskell-language-server-readiness-measurement.md).
//!
//! HLS (measured with 2.13.0.0) has no readiness vocabulary an observer can read:
//!
//! - **identity**: it returns no `serverInfo` and logs nothing at startup on the protocol. The
//!   only thing that names it is `executeCommandProvider.commands` in `InitializeResult`,
//!   pid-prefixed (`"<pid>:ghcide-type-lenses:typesignature.add"` and the like); a command
//!   whose plugin segment starts with `ghcide-` is HLS. The version is not observable
//! - **readiness is `unknown`** (spec 8.2 item 3, 8.4 item 1). Its `$/progress` sessions
//!   ("Processing" per typecheck kick, "Indexing" per batch written to the hiedb, "Setting up
//!   …" for the cradle) are created by the lsp library only when a session lasts longer than
//!   1 second (`optProgressStartDelay`), and every kick or drained index batch restarts the
//!   session, so tokens rarely appear and cover only slices of the work: 202 modules indexed
//!   over 8 seconds showed one 0.7-second token, while `references` answered 13 different,
//!   growing partial results. With `--test` (delay 0) the "Indexing" tokens open and close per
//!   file with gaps in which no token is open and the results are still incomplete. The absence
//!   of a token means neither "done" nor "not started", so the observer must not stay at
//!   `initializing` nor claim `ready` on an end. Mapping an open token to `indexing` was
//!   rejected too: it would only go back to `unknown` on the end, a hold with no release value
//! - **health**: a cradle that cannot load (hie-bios fails to run the build tool) shows up as a
//!   diagnostic with `source: "cradle"` and severity 1 on the opened file, after which every
//!   request is answered with empty results. That is the "broken server answering with
//!   successes" lie, and the diagnostic is the signal: `error` with the first line of the
//!   message while any file carries one; back to `unknown` when the same file's diagnostics
//!   arrive without it (nothing observable says `ok`: "Setting up" is time-gated and
//!   `ghcide/cradle/loaded` exists only under `--test`)
//! - **guarantees**: none (`{}`). Readiness is not observed, so nothing of 7.2 / 7.3 is
//!   promised
//!
//! The root fix belongs upstream: `serverInfo`, and a completion signal for the index (a total
//! next to `ghcide/reference/ready`, or this protocol) outside test mode; the 1-second gate is
//! a UI choice and not a readiness vocabulary.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{Health, ServerState, ServerStateProvider};

/// The name this mapping is selected by (the server has no `serverInfo.name`; this is the
/// executable's name).
pub const SERVER_NAME: &str = "haskell-language-server";
const DIAGNOSTICS_METHOD: &str = "textDocument/publishDiagnostics";
const CRADLE_SOURCE: &str = "cradle";
/// LSP `DiagnosticSeverity.Error`.
const SEVERITY_ERROR: u8 = 1;
const GHCIDE_PLUGIN_PREFIX: &str = "ghcide-";

/// Whether an `InitializeResult` (`result`) is HLS's: it declares an `executeCommandProvider`
/// with a pid-prefixed command of a `ghcide-…` plugin.
pub fn is_hls_initialize_result(result: &Value) -> bool {
    result["capabilities"]["executeCommandProvider"]["commands"]
        .as_array()
        .is_some_and(|commands| {
            commands
                .iter()
                .filter_map(Value::as_str)
                .any(is_ghcide_command)
        })
}

/// `"<pid>:ghcide-<plugin>:<command>"`.
fn is_ghcide_command(command: &str) -> bool {
    let Some((pid, rest)) = command.split_once(':') else {
        return false;
    };
    !pid.is_empty()
        && pid.bytes().all(|b| b.is_ascii_digit())
        && rest.starts_with(GHCIDE_PLUGIN_PREFIX)
}

#[derive(Deserialize)]
struct Diagnostic {
    #[serde(default)]
    source: Option<String>,
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

pub struct HaskellLanguageServerAdapter {
    state: ServerState,
    /// The files whose latest diagnostics carry a cradle failure, with the failure's first
    /// line.
    cradle_failures: BTreeMap<String, String>,
}

impl Default for HaskellLanguageServerAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl HaskellLanguageServerAdapter {
    pub fn new() -> Self {
        HaskellLanguageServerAdapter {
            state: ServerState::unobserved(),
            cradle_failures: BTreeMap::new(),
        }
    }

    fn on_diagnostics(&mut self, params: DiagnosticsParams) -> Option<ServerState> {
        let failure = params.diagnostics.iter().find(|d| {
            d.source.as_deref() == Some(CRADLE_SOURCE) && d.severity == Some(SEVERITY_ERROR)
        });
        match failure {
            Some(d) => {
                let first_line = d.message.lines().next().unwrap_or("").trim().to_string();
                self.cradle_failures.insert(params.uri, first_line);
            }
            None => {
                self.cradle_failures.remove(&params.uri);
            }
        }
        let next = match self.cradle_failures.values().next() {
            Some(message) => ServerState {
                health: Health::Error,
                readiness: self.state.readiness,
                message: Some(message.clone()),
            },
            None => ServerState::unobserved(),
        };
        if next == self.state {
            return None;
        }
        self.state = next;
        Some(self.state.clone())
    }
}

impl Mapping for HaskellLanguageServerAdapter {
    /// Unknown on both axes (spec 8.4 item 1): readiness is never observed, and health not yet.
    fn initial_state(&self) -> ServerState {
        ServerState::unobserved()
    }

    /// Never a guarantee (see the module documentation).
    fn guarantees(&self) -> ServerStateProvider {
        ServerStateProvider::notifications_only()
    }

    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() || view.method() != Some(DIAGNOSTICS_METHOD) {
            return None;
        }
        #[derive(Deserialize)]
        struct Envelope {
            params: DiagnosticsParams,
        }
        let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
        self.on_diagnostics(envelope.params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;
    use crate::state::Readiness;

    fn feed(adapter: &mut HaskellLanguageServerAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    fn diagnostics(uri: &str, items: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{{"uri":"{uri}","diagnostics":[{items}]}}}}"#
        )
    }

    const CRADLE: &str = r#"{"source":"cradle","severity":1,"range":{"start":{"line":0,"character":0},"end":{"line":1,"character":0}},"message":"Failed to run cabal v2-repl 'lib:x' in directory /p\nConsult the logs"}"#;
    const TYPE_ERROR: &str = r#"{"source":"typecheck","severity":1,"range":{"start":{"line":0,"character":0},"end":{"line":1,"character":0}},"message":"Variable not in scope"}"#;

    #[test]
    fn recognizes_the_server_by_its_pid_prefixed_ghcide_commands() {
        let hls = serde_json::json!({"capabilities": {"executeCommandProvider": {
            "commands": ["4242:eval:evalCommand", "4242:ghcide-type-lenses:typesignature.add"]
        }}});
        assert!(is_hls_initialize_result(&hls));
        let other = serde_json::json!({"capabilities": {"executeCommandProvider": {
            "commands": ["nextflow.server.previewDag", "ghcide-something"]
        }}});
        assert!(!is_hls_initialize_result(&other));
        assert!(!is_hls_initialize_result(
            &serde_json::json!({"capabilities": {"executeCommandProvider": {"commands": []}}})
        ));
        assert!(!is_hls_initialize_result(
            &serde_json::json!({"capabilities": {}})
        ));
    }

    #[test]
    fn starts_unknown_on_both_axes_and_declares_nothing() {
        let m = HaskellLanguageServerAdapter::new();
        assert_eq!(m.initial_state(), ServerState::unobserved());
        assert_eq!(m.guarantees(), ServerStateProvider::notifications_only());
    }

    #[test]
    fn a_cradle_failure_is_error_health_until_the_file_is_diagnosed_without_it() {
        let mut m = HaskellLanguageServerAdapter::new();
        let state = feed(&mut m, &diagnostics("file:///p/src/A.hs", CRADLE))
            .expect("a cradle failure moves health");
        assert_eq!(state.health, Health::Error);
        assert_eq!(state.readiness, Readiness::Unknown);
        assert_eq!(
            state.message.as_deref(),
            Some("Failed to run cabal v2-repl 'lib:x' in directory /p")
        );
        // A second file with the same failure changes nothing to notify.
        assert!(feed(&mut m, &diagnostics("file:///p/src/B.hs", CRADLE)).is_none());
        // One file recovering is not enough while the other still carries it.
        assert!(feed(&mut m, &diagnostics("file:///p/src/A.hs", TYPE_ERROR)).is_none());
        let state = feed(&mut m, &diagnostics("file:///p/src/B.hs", ""))
            .expect("the last failure clearing moves health");
        assert_eq!(state, ServerState::unobserved());
    }

    #[test]
    fn other_diagnostics_and_progress_move_nothing() {
        let mut m = HaskellLanguageServerAdapter::new();
        assert!(feed(&mut m, &diagnostics("file:///p/src/A.hs", TYPE_ERROR)).is_none());
        // A cradle warning is not a failure.
        assert!(
            feed(
                &mut m,
                &diagnostics(
                    "file:///p/src/A.hs",
                    r#"{"source":"cradle","severity":2,"range":{"start":{"line":0,"character":0},"end":{"line":1,"character":0}},"message":"Loading…"}"#
                )
            )
            .is_none()
        );
        assert!(
            feed(
                &mut m,
                r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":1,"value":{"kind":"begin","title":"Indexing"}}}"#
            )
            .is_none()
        );
        assert_eq!(m.state, ServerState::unobserved());
    }
}
