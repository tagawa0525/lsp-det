//! The Expert (Elixir) mapping (M10, ADR 0019 decision F;
//! research/expert-readiness-measurement.md).
//!
//! Expert has no readiness vocabulary of its own. It is synthesized from the `$/progress`
//! titles of the engine start and the build (measured with Expert 0.1.9):
//!
//! - **readiness**: a begin of "… Starting engine node", "… Preparing engine" (the project
//!   name is prefixed), "Building …", "Indexing source code", or "Loading search index" means
//!   `indexing`. `ready` needs no token of those titles open AND the last token that ended was
//!   one of the two index phases: a build is followed by "Indexing source code" (a fresh index)
//!   or by "Loading search index" (a persisted index of the same project; measured, no
//!   "Indexing source code" follows it on a warm start). The measured 1-second gap between the
//!   engine start and the build (no token open, `references` answered with `[]`) is not ready.
//!   "Finding Completion Candidates" is request processing and is not looked at
//! - **no prediction**: Expert registers `**/*.{ex,exs}` for `workspace/didChangeWatchedFiles`
//!   but neither a Created nor a Changed leads to a build (measured), so there is no completion
//!   signal to predict against (ADR 0014 addendum decision D)
//! - **health**: no signal. `unknown` (spec 8.2 item 3)
//!
//! Before the engine is initialized, Expert answers `references` with an empty array while
//! logging that it ignored the request; the hold during `initializing` / `indexing` is what
//! removes that lie.
//!
//! **No guarantee is declared for any version** (`serverStateProvider: {}`). After a build,
//! Expert loads the persisted search index ("Loading search index") and, only if that index
//! is empty or stale, rebuilds it ("Indexing source code") about 50 ms later; in between, a
//! fresh project answers `references` and `workspace/symbol` with empty results. Nothing in
//! Expert's vocabulary tells the observer at the end of the load whether a rebuild follows
//! (the "backend reports empty / stale" log stays inside the engine), so an observer cannot
//! promise `coverage` at `ready` without a clock. The readiness mapping still removes the
//! 25-second stretch of empty answers before the engine is up. There is no list of tested
//! versions here on purpose: no version can pass 7.2 until Expert exposes the decision
//! (spec 8.2 item 5), so `guarantees` is unconditional.

use serde::Deserialize;
use serde_json::Value;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{Readiness, ServerState, ServerStateProvider};

const PROGRESS_METHOD: &str = "$/progress";
const ENGINE_NODE_SUFFIX: &str = "Starting engine node";
const ENGINE_PREPARE_SUFFIX: &str = "Preparing engine";
const BUILDING_PREFIX: &str = "Building ";
const INDEXING_TITLE: &str = "Indexing source code";
const LOADING_INDEX_TITLE: &str = "Loading search index";

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

/// The kinds of token the startup consists of. Only an index phase closes a round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Engine,
    Building,
    Indexing,
}

fn phase_of(title: &str) -> Option<Phase> {
    if title == INDEXING_TITLE || title == LOADING_INDEX_TITLE {
        Some(Phase::Indexing)
    } else if title.starts_with(BUILDING_PREFIX) {
        Some(Phase::Building)
    } else if title.ends_with(ENGINE_NODE_SUFFIX) || title.ends_with(ENGINE_PREPARE_SUFFIX) {
        Some(Phase::Engine)
    } else {
        None
    }
}

pub struct ExpertAdapter {
    state: ServerState,
    /// Open tokens of the startup phases.
    open: Vec<(Value, Phase)>,
    /// The last token that ended was an index phase ("Indexing source code" or "Loading search
    /// index").
    last_ended_indexing: bool,
}

impl Default for ExpertAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ExpertAdapter {
    pub fn new() -> Self {
        ExpertAdapter {
            state: ServerState::initializing(),
            open: Vec::new(),
            last_ended_indexing: false,
        }
    }

    fn on_progress(&mut self, params: ProgressParams) -> Option<ServerState> {
        let ProgressParams { token, value } = params;
        match value.kind.as_str() {
            "begin" => {
                let phase = phase_of(value.title.as_deref()?)?;
                self.open.push((token, phase));
                self.state.readiness = Readiness::Indexing;
            }
            "end" => {
                let index = self.open.iter().position(|(t, _)| *t == token)?;
                let (_, phase) = self.open.remove(index);
                self.last_ended_indexing = phase == Phase::Indexing;
                if self.open.is_empty() && self.last_ended_indexing {
                    self.state.readiness = Readiness::Ready;
                }
            }
            _ => return None,
        }
        Some(self.state.clone())
    }
}

impl Mapping for ExpertAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// Never a guarantee, whatever the version (see the module documentation).
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
        self.on_progress(envelope.params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;

    fn progress(token: i64, kind: &str, title: Option<&str>) -> String {
        let title = title
            .map(|t| format!(r#","title":"{t}""#))
            .unwrap_or_default();
        format!(
            r#"{{"jsonrpc":"2.0","method":"$/progress","params":{{"token":{token},"value":{{"kind":"{kind}"{title}}}}}}}"#
        )
    }

    fn feed(adapter: &mut ExpertAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    fn readiness_after(adapter: &mut ExpertAdapter, body: &str) -> Readiness {
        feed(adapter, body)
            .expect("a phase token moves the state")
            .readiness
    }

    #[test]
    fn ready_only_after_the_indexing_end_with_nothing_open() {
        let mut m = ExpertAdapter::new();
        assert_eq!(
            readiness_after(
                &mut m,
                &progress(1, "begin", Some("[fixture] Starting engine node"))
            ),
            Readiness::Indexing
        );
        feed(
            &mut m,
            &progress(2, "begin", Some("[fixture] Preparing engine")),
        );
        feed(&mut m, &progress(2, "end", None));
        // The gap: the engine is up, no token is open, the build has not started.
        assert_eq!(
            readiness_after(&mut m, &progress(1, "end", None)),
            Readiness::Indexing
        );
        feed(&mut m, &progress(3, "begin", Some("Building fixture")));
        assert_eq!(
            readiness_after(&mut m, &progress(3, "end", None)),
            Readiness::Indexing,
            "a build alone does not complete a round"
        );
        feed(&mut m, &progress(4, "begin", Some("Indexing source code")));
        assert_eq!(
            readiness_after(&mut m, &progress(4, "end", None)),
            Readiness::Ready
        );
        // A later build reindexes.
        assert_eq!(
            readiness_after(&mut m, &progress(5, "begin", Some("Building fixture"))),
            Readiness::Indexing
        );
        feed(&mut m, &progress(5, "end", None));
        feed(&mut m, &progress(6, "begin", Some("Indexing source code")));
        assert_eq!(
            readiness_after(&mut m, &progress(6, "end", None)),
            Readiness::Ready
        );
    }

    #[test]
    fn request_processing_titles_are_ignored() {
        let mut m = ExpertAdapter::new();
        assert!(
            feed(
                &mut m,
                &progress(9, "begin", Some("Finding Completion Candidates"))
            )
            .is_none()
        );
        assert!(feed(&mut m, &progress(9, "end", None)).is_none());
    }

    #[test]
    fn a_loaded_index_completes_a_round_like_a_fresh_one() {
        // Measured on a warm start: "Building" then "Loading search index" with no
        // "Indexing source code" after it.
        let mut m = ExpertAdapter::new();
        feed(&mut m, &progress(3, "begin", Some("Building fixture")));
        assert_eq!(
            readiness_after(&mut m, &progress(3, "end", None)),
            Readiness::Indexing
        );
        feed(&mut m, &progress(6, "begin", Some("Loading search index")));
        assert_eq!(
            readiness_after(&mut m, &progress(6, "end", None)),
            Readiness::Ready
        );
    }

    #[test]
    fn never_declares_a_guarantee() {
        assert_eq!(
            ExpertAdapter::new().guarantees(),
            ServerStateProvider::notifications_only()
        );
    }
}
