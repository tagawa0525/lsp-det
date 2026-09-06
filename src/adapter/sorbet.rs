//! The Sorbet mapping (M22, ADR 0020 decision C row for Sorbet;
//! research/sorbet-readiness-measurement.md).
//!
//! Sorbet (measured with sorbet-static 0.6.13485) returns no `serverInfo` and announces
//! nothing at startup by itself. The only thing that names it is `sorbet/showOperation`
//! (`{operationName, description, status: "start" | "end"}`), sent only when the client passes
//! `initializationOptions.supportsOperationNotifications: true` -- an opt-in lsp-det injects
//! itself only when it is the one that launched the `sorbet` (or `srb`) command
//! ([`super::INITIALIZATION_OPTIONS_BY_COMMAND`], ADR 0020 decision D). A client is of course
//! free to pass the option itself; the injection only fills the gap for one that does not.
//! Sorbet's version never appears anywhere in the protocol.
//!
//! - **readiness**: starts `initializing`. Operations that are not tied to a request --
//!   `Indexing`, `SlowPathBlocking`, `SlowPathNonBlocking`, `FastPath`, and anything else that
//!   is not in [`REQUEST_TIED_OPERATIONS`] -- are counted: a `start` increments the count of
//!   open operations and, once the state has already reached `ready` once (a later round),
//!   moves to `indexing`; an `end` decrements it, and once the count returns to zero the state
//!   becomes `ready`. Operations nest (measured: `Indexing` inside `SlowPathBlocking`), hence
//!   the counter rather than a plain begin/end pair. An `end` that would take the count below
//!   zero is ignored (defensive: not observed, but symmetric with the rest of the corpus's
//!   handling of an out-of-order end). Operations tied to a request --
//!   [`REQUEST_TIED_OPERATIONS`] -- are the processing of a cross-file request itself and are
//!   not read as readiness (ADR 0019 decision G: per-request progress is not readiness)
//! - **no prediction** (`observe_client` is not implemented): the server holds a request until
//!   the operation it depends on ends by itself (measured: no empty or partial answer is ever
//!   observed), so there is nothing for the observer to predict from `didChange` or
//!   `workspace/didChangeWatchedFiles`
//! - **health**: no signal. `unknown` (spec 8.2 item 3). Sorbet's own documentation
//!   (`server-status.md`) says cross-file requests answer only from Idle, matching the
//!   readiness reading above
//!
//! **No guarantee is declared for any version** (`serverStateProvider: {}`, ADR 0020 decision
//! E): the version never appears in the protocol, so a guarantee cannot be scoped to tested
//! versions (spec 8.2 item 5).

use serde::Deserialize;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{Readiness, ServerState, ServerStateProvider};

/// The name this mapping is selected by. Sorbet has no `serverInfo.name`; this is the identity
/// [`super::identity_from_notification`] reports for a `sorbet/showOperation` notification.
pub const SERVER_NAME: &str = "sorbet";

/// The notification whose method alone is Sorbet's identity announcement
/// ([`super::identity_from_notification`]).
pub const SHOW_OPERATION_METHOD: &str = "sorbet/showOperation";

/// Operation names that are the processing of a cross-file request itself, not readiness
/// (measured; `website/docs/lsp.md`).
const REQUEST_TIED_OPERATIONS: &[&str] = &["References", "SymbolSearch", "Rename", "MoveMethod"];

#[derive(Deserialize)]
struct ShowOperationParams {
    #[serde(rename = "operationName")]
    operation_name: String,
    status: String,
}

pub struct SorbetAdapter {
    state: ServerState,
    /// The count of open (not-yet-ended) operations that are not tied to a request.
    open_operations: u32,
}

impl Default for SorbetAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl SorbetAdapter {
    pub fn new() -> Self {
        SorbetAdapter {
            state: ServerState::initializing(),
            open_operations: 0,
        }
    }

    fn on_operation(&mut self, operation_name: &str, status: &str) -> Option<ServerState> {
        if REQUEST_TIED_OPERATIONS.contains(&operation_name) {
            return None;
        }
        match status {
            "start" => {
                self.open_operations += 1;
                if self.state.readiness == Readiness::Ready {
                    // Only a later round (already saw a ready) moves visibly to indexing; the
                    // first round stays initializing until the count returns to zero.
                    self.state.readiness = Readiness::Indexing;
                }
            }
            "end" => {
                if self.open_operations == 0 {
                    // Would take the count below zero: not observed, ignored defensively.
                    return None;
                }
                self.open_operations -= 1;
                if self.open_operations == 0 {
                    self.state.readiness = Readiness::Ready;
                }
            }
            _ => return None,
        }
        Some(self.state.clone())
    }
}

impl Mapping for SorbetAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// Never a guarantee (ADR 0020 decision E; see the module documentation).
    fn guarantees(&self) -> ServerStateProvider {
        ServerStateProvider::notifications_only()
    }

    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() || view.method() != Some(SHOW_OPERATION_METHOD) {
            return None;
        }
        #[derive(Deserialize)]
        struct Envelope {
            params: ShowOperationParams,
        }
        let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
        self.on_operation(&envelope.params.operation_name, &envelope.params.status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;
    use crate::state::Health;

    fn operation(name: &str, status: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"sorbet/showOperation","params":{{"operationName":"{name}","description":"{name}...","status":"{status}"}}}}"#
        )
    }

    fn feed(adapter: &mut SorbetAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    #[test]
    fn starts_initializing() {
        let adapter = SorbetAdapter::new();
        assert_eq!(adapter.initial_state().readiness, Readiness::Initializing);
    }

    #[test]
    fn a_single_start_and_its_end_reach_ready_on_the_first_round() {
        let mut m = SorbetAdapter::new();
        let state = feed(&mut m, &operation("Indexing", "start"))
            .expect("a start is a signal even while it does not move readiness");
        assert_eq!(
            state.readiness,
            Readiness::Initializing,
            "the first round stays initializing until the count returns to zero"
        );
        let state = feed(&mut m, &operation("Indexing", "end"))
            .expect("the end that empties the count is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(state.health, Health::Unknown);
    }

    #[test]
    fn nested_operations_only_become_ready_once_the_outermost_ends() {
        let mut m = SorbetAdapter::new();
        feed(&mut m, &operation("SlowPathBlocking", "start"));
        let state = feed(&mut m, &operation("Indexing", "start"))
            .expect("a nested start is still a signal");
        assert_eq!(
            state.readiness,
            Readiness::Initializing,
            "still the first round, and the outer operation is still open"
        );
        let state = feed(&mut m, &operation("Indexing", "end"))
            .expect("the inner end is a signal even though the count does not reach zero");
        assert_eq!(
            state.readiness,
            Readiness::Initializing,
            "the outer SlowPathBlocking is still open"
        );
        let state = feed(&mut m, &operation("SlowPathBlocking", "end"))
            .expect("the outer end that empties the count is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn a_later_start_reindexes_and_its_end_is_ready_again() {
        let mut m = SorbetAdapter::new();
        feed(&mut m, &operation("Indexing", "start"));
        feed(&mut m, &operation("Indexing", "end"));
        let state = feed(&mut m, &operation("SlowPathNonBlocking", "start"))
            .expect("a start after ready (a later round) is a signal");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state =
            feed(&mut m, &operation("SlowPathNonBlocking", "end")).expect("its end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn ignores_request_tied_operations() {
        let mut m = SorbetAdapter::new();
        feed(&mut m, &operation("Indexing", "start"));
        feed(&mut m, &operation("Indexing", "end"));
        for name in ["References", "SymbolSearch", "Rename", "MoveMethod"] {
            assert!(
                feed(&mut m, &operation(name, "start")).is_none(),
                "a request-tied operation's start moved the state: {name}"
            );
            assert!(
                feed(&mut m, &operation(name, "end")).is_none(),
                "a request-tied operation's end moved the state: {name}"
            );
        }
        assert_eq!(m.state.readiness, Readiness::Ready, "unaffected throughout");
    }

    #[test]
    fn ignores_an_end_that_would_take_the_count_below_zero() {
        let mut m = SorbetAdapter::new();
        assert!(
            feed(&mut m, &operation("Indexing", "end")).is_none(),
            "an end with nothing open must not be treated as reaching zero"
        );
        assert_eq!(m.state.readiness, Readiness::Initializing);
        // The count is still well-formed afterward: a real start/end pair works normally.
        feed(&mut m, &operation("Indexing", "start"));
        let state = feed(&mut m, &operation("Indexing", "end")).expect("a matched end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn ignores_other_methods() {
        let mut m = SorbetAdapter::new();
        let unrelated = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"anything"}}"#;
        let view = peek(unrelated.as_bytes()).unwrap();
        assert!(m.interpret(&view, unrelated.as_bytes()).is_none());
    }

    #[test]
    fn ignores_a_show_operation_that_happens_to_be_sent_as_a_request() {
        let mut m = SorbetAdapter::new();
        let as_request = r#"{"jsonrpc":"2.0","id":1,"method":"sorbet/showOperation","params":{"operationName":"Indexing","description":"x","status":"start"}}"#;
        let view = peek(as_request.as_bytes()).unwrap();
        assert!(m.interpret(&view, as_request.as_bytes()).is_none());
    }

    #[test]
    fn health_stays_unknown_regardless_of_readiness() {
        let mut m = SorbetAdapter::new();
        assert_eq!(
            feed(&mut m, &operation("Indexing", "start"))
                .unwrap()
                .health,
            Health::Unknown
        );
        assert_eq!(
            feed(&mut m, &operation("Indexing", "end")).unwrap().health,
            Health::Unknown
        );
    }

    #[test]
    fn never_declares_a_guarantee() {
        assert_eq!(
            SorbetAdapter::new().guarantees(),
            ServerStateProvider::notifications_only()
        );
    }
}
