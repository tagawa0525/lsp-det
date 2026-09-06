//! The mapping for crystalline (Crystal's language server, M17, ADR 0019 decision F;
//! research/crystalline-readiness-measurement.md).
//!
//! crystalline (measured with 0.18.0) has no readiness vocabulary of its own, but the moment it
//! becomes ready to answer is announced plainly:
//!
//! - **identity**: it returns no `serverInfo` and its `InitializeResult` declares nothing that
//!   names it (only `textDocumentSync` and five plain providers). The only thing that does is a
//!   `window/logMessage` sent right after `initialized`, `"[workspace] Found projects:\n<path>"`
//!   (the leading double quote is part of the server's own heredoc), sent only when a `shard.yml`
//!   project was found under the root. The version never appears in the protocol
//! - **readiness**: `initializing` until the log `"LSP server is ready."`, then `ready`. This is
//!   the only readiness signal; a root without a `shard.yml` project never sends either log, so
//!   the mapping is never selected and the connection reports `unknown` on both axes (spec 8.2
//!   item 3)
//! - **`$/progress`** (token `"workspace/compile/N"`) is a per-request compilation: every
//!   `textDocument/definition` (and the other four providers) waits for its own `compile` to
//!   finish before answering, so the token is a side effect of the request, not a readiness
//!   signal (spec chapter 8's rule against mapping per-request progress to readiness). It is not
//!   read
//! - **health**: no signal. `"Completed with errors."` on a compilation end is a diagnostic on
//!   the user's code, not a server health observation, so health stays `unknown`
//! - **guarantees**: none (`{}`). The version is not observable anywhere in the protocol, so a
//!   guarantee cannot be scoped to versions the conformance suite passed on (spec 8.2 item 5),
//!   even though the per-request compile makes every answered request complete on its own
//!
//! The root fix belongs upstream: `serverInfo`, an error response (rather than an empty result)
//! for a request whose compilation failed, and registering `didChangeWatchedFiles` to invalidate
//! the result cache for files that are not open.

use serde::Deserialize;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{ServerState, ServerStateProvider};

/// The name this mapping is selected by (the server has no `serverInfo.name`; this is the
/// executable's name).
pub const SERVER_NAME: &str = "crystalline";
const LOG_MESSAGE_METHOD: &str = "window/logMessage";
/// The startup log naming a found `shard.yml` project (the leading `"` is the server's own
/// heredoc, kept verbatim so the match is exact).
const FOUND_PROJECTS_PREFIX: &str = "\"[workspace] Found projects:";
const READY_MESSAGE: &str = "LSP server is ready.";

/// Whether a `window/logMessage` message is crystalline's startup announcement that a
/// `shard.yml` project was found under the root. `false` for any other wording, including the
/// "LSP server is ready." log by itself (a root with no project never gets this one).
pub fn identity_from_log(message: &str) -> bool {
    message.starts_with(FOUND_PROJECTS_PREFIX)
}

#[derive(Deserialize)]
struct LogMessageParams {
    message: String,
}

/// The crystalline mapping.
pub struct CrystallineAdapter {
    state: ServerState,
}

impl Default for CrystallineAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl CrystallineAdapter {
    pub fn new() -> Self {
        CrystallineAdapter {
            state: ServerState::initializing(),
        }
    }

    fn on_log(&mut self, message: &str) -> Option<ServerState> {
        if message != READY_MESSAGE || self.state.readiness == crate::state::Readiness::Ready {
            return None;
        }
        self.state.readiness = crate::state::Readiness::Ready;
        Some(self.state.clone())
    }
}

impl Mapping for CrystallineAdapter {
    /// `initializing` (spec 8.4 item 1 does not apply: the mapping is selected only once the
    /// startup log named a project, so readiness starts observed).
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// Never a guarantee: the version is not observable (see the module documentation).
    fn guarantees(&self) -> ServerStateProvider {
        ServerStateProvider::notifications_only()
    }

    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() || view.method() != Some(LOG_MESSAGE_METHOD) {
            return None;
        }
        #[derive(Deserialize)]
        struct Envelope {
            params: LogMessageParams,
        }
        let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
        self.on_log(&envelope.params.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;
    use crate::state::{Health, Readiness};

    fn log(message: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"window/logMessage","params":{{"type":3,"message":"{message}"}}}}"#
        )
    }

    fn interpret(adapter: &mut CrystallineAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    #[test]
    fn recognizes_the_found_projects_log_and_nothing_else() {
        assert!(identity_from_log(
            "\"[workspace] Found projects:\n/p/fixture"
        ));
        for other in [
            "LSP server is ready.",
            "[workspace] Found projects:",
            "Flags for project fixture: []",
            "",
        ] {
            assert!(
                !identity_from_log(other),
                "not the startup announcement: {other:?}"
            );
        }
    }

    #[test]
    fn starts_initializing_with_unknown_health_and_declares_nothing() {
        let adapter = CrystallineAdapter::new();
        let state = adapter.initial_state();
        assert_eq!(state.readiness, Readiness::Initializing);
        assert_eq!(state.health, Health::Unknown);
        assert_eq!(
            adapter.guarantees(),
            ServerStateProvider::notifications_only()
        );
    }

    #[test]
    fn the_ready_log_moves_readiness_to_ready_once() {
        let mut adapter = CrystallineAdapter::new();
        let state = interpret(&mut adapter, &log("LSP server is ready."))
            .expect("the ready log is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(state.health, Health::Unknown);
        // Idempotent: a second one changes nothing to notify.
        assert!(interpret(&mut adapter, &log("LSP server is ready.")).is_none());
    }

    #[test]
    fn per_request_compilation_progress_and_other_logs_move_nothing() {
        let mut adapter = CrystallineAdapter::new();
        for other in [
            log("\\\"[workspace] Found projects:\\n/p/fixture"),
            log("Flags for project fixture: []"),
            r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":"workspace/compile/0","value":{"kind":"begin","title":"Building project"}}}"#.to_string(),
            r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":"workspace/compile/0","value":{"kind":"end","message":"Completed with errors."}}}"#.to_string(),
        ] {
            assert!(
                interpret(&mut adapter, &other).is_none(),
                "the state moved on an unrelated message: {other}"
            );
        }
        assert_eq!(adapter.state, ServerState::initializing());
    }
}
