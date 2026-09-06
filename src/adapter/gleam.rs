//! The Gleam mapping (M19, ADR 0019 decision F; research/gleam-readiness-measurement.md).
//!
//! Gleam's language server (measured with 1.18.1) returns no `serverInfo` and announces
//! nothing at startup. The only thing that names it is `$/progress` with the fixed token
//! `"downloading-dependencies"`: a begin whose title is "Downloading Gleam dependencies" is
//! sent right after `initialized`, even when there is nothing to download (measured 12 ms).
//! The version is not observable anywhere in the protocol.
//!
//! - **readiness**: the dependency-download token's begin → end is the only readiness
//!   vocabulary. The first begin does not move the state past the mapping's starting
//!   `initializing` (measured: it always happens, so its absence is not "not started" either;
//!   the observer simply waits for the matching end). Its end → `ready`. The engine is
//!   recreated on a `gleam.toml` change (`workspace/didChangeWatchedFiles`), and the same
//!   token begins and ends again; a begin while `ready` → `indexing`, its end → `ready` again
//! - **no prediction**: a `textDocument/didChange` is incorporated synchronously inside request
//!   processing (measured), so there is nothing to predict. A watched-file Changed for
//!   `gleam.toml` recreates the engine but, after the token completes, `references` answers
//!   empty instead of the incorporated change (measured; likely a 1.18.1 bug, not traced to a
//!   root cause in the source) — predicting `indexing` from it would advertise a completion
//!   that does not arrive, so it is not read at all (ADR 0014 addendum decision D)
//! - **health**: no signal. `unknown` (spec 8.2 item 3)
//!
//! **No guarantee is declared for any version** (`serverStateProvider: {}`): the version never
//! appears in the protocol, so a guarantee cannot be scoped to tested versions (spec 8.2
//! item 5).

use serde::Deserialize;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{Readiness, ServerState, ServerStateProvider};

/// The name this mapping is selected by (Gleam has no `serverInfo.name`; this is the
/// executable's name).
pub const SERVER_NAME: &str = "gleam";
const PROGRESS_METHOD: &str = "$/progress";
/// The fixed token of the dependency-download progress (`language-server/src/progress.rs`).
const DEPENDENCY_TOKEN: &str = "downloading-dependencies";
/// The begin title of the dependency-download progress. The only identity announcement Gleam
/// makes on the protocol.
pub const DEPENDENCY_PROGRESS_TITLE: &str = "Downloading Gleam dependencies";

#[derive(Deserialize)]
struct ProgressParams {
    token: String,
    value: ProgressValue,
}

#[derive(Deserialize)]
struct ProgressValue {
    kind: String,
    #[serde(default)]
    title: Option<String>,
}

/// Whether `body` is a `$/progress` notification whose `value` is a begin with the dependency
/// download's title. Used both to identify the upstream as Gleam and, in unit tests, to pin the
/// wording down.
pub fn is_dependency_progress_begin(body: &[u8]) -> bool {
    #[derive(Deserialize)]
    struct Envelope {
        params: ProgressParams,
    }
    let Ok(envelope) = serde_json::from_slice::<Envelope>(body) else {
        return false;
    };
    envelope.params.value.kind == "begin"
        && envelope.params.value.title.as_deref() == Some(DEPENDENCY_PROGRESS_TITLE)
}

pub struct GleamAdapter {
    state: ServerState,
}

impl Default for GleamAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl GleamAdapter {
    pub fn new() -> Self {
        GleamAdapter {
            state: ServerState::initializing(),
        }
    }

    fn on_progress(&mut self, value: ProgressValue) -> Option<ServerState> {
        match value.kind.as_str() {
            "begin" => {
                if self.state.readiness != Readiness::Ready {
                    // The first round: measured to always happen, so this does not move the
                    // state past the mapping's starting `initializing`.
                    return None;
                }
                self.state.readiness = Readiness::Indexing;
            }
            "end" => {
                self.state.readiness = Readiness::Ready;
            }
            _ => return None,
        }
        Some(self.state.clone())
    }
}

impl Mapping for GleamAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// Never a guarantee (see the module documentation).
    fn guarantees(&self) -> ServerStateProvider {
        ServerStateProvider::notifications_only()
    }

    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() || view.method() != Some(PROGRESS_METHOD) {
            return None;
        }
        #[derive(Deserialize)]
        struct Envelope {
            params: ProgressParams,
        }
        let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
        if envelope.params.token != DEPENDENCY_TOKEN {
            return None;
        }
        self.on_progress(envelope.params.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;
    use crate::state::Health;

    fn progress(token: &str, kind: &str, title: Option<&str>) -> String {
        let title = title
            .map(|t| format!(r#","title":"{t}""#))
            .unwrap_or_default();
        format!(
            r#"{{"jsonrpc":"2.0","method":"$/progress","params":{{"token":"{token}","value":{{"kind":"{kind}"{title}}}}}}}"#
        )
    }

    fn feed(adapter: &mut GleamAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    #[test]
    fn a_first_begin_does_not_move_past_initializing_and_its_end_is_ready() {
        let mut m = GleamAdapter::new();
        assert_eq!(m.initial_state().readiness, Readiness::Initializing);
        assert!(
            feed(
                &mut m,
                &progress(
                    "downloading-dependencies",
                    "begin",
                    Some(DEPENDENCY_PROGRESS_TITLE)
                )
            )
            .is_none(),
            "a first begin is not an observable change"
        );
        let state = feed(&mut m, &progress("downloading-dependencies", "end", None))
            .expect("the end of the first round is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(state.health, Health::Unknown);
    }

    #[test]
    fn a_begin_after_ready_reindexes_and_its_end_is_ready_again() {
        let mut m = GleamAdapter::new();
        feed(&mut m, &progress("downloading-dependencies", "begin", None));
        feed(&mut m, &progress("downloading-dependencies", "end", None));
        let state = feed(&mut m, &progress("downloading-dependencies", "begin", None))
            .expect("a begin while ready (the engine recreated) is a signal");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state = feed(&mut m, &progress("downloading-dependencies", "end", None))
            .expect("the end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn a_different_token_is_ignored() {
        let mut m = GleamAdapter::new();
        assert!(feed(&mut m, &progress("some-other-token", "begin", None)).is_none());
        assert!(feed(&mut m, &progress("some-other-token", "end", None)).is_none());
        assert_eq!(m.initial_state().readiness, Readiness::Initializing);
    }

    #[test]
    fn never_declares_a_guarantee() {
        assert_eq!(
            GleamAdapter::new().guarantees(),
            ServerStateProvider::notifications_only()
        );
    }

    #[test]
    fn recognizes_the_dependency_progress_begin_by_its_title() {
        assert!(is_dependency_progress_begin(
            progress(
                "downloading-dependencies",
                "begin",
                Some(DEPENDENCY_PROGRESS_TITLE)
            )
            .as_bytes()
        ));
        assert!(!is_dependency_progress_begin(
            progress("downloading-dependencies", "end", None).as_bytes()
        ));
        assert!(!is_dependency_progress_begin(
            progress("downloading-dependencies", "begin", Some("Something else")).as_bytes()
        ));
    }
}
