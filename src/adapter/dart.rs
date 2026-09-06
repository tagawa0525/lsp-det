//! The Dart analysis server mapping (M21, ADR 0020 decision C row for Dart;
//! research/dart-readiness-measurement.md).
//!
//! Identified by `serverInfo.name` "Dart SDK LSP Analysis Server"; the version is
//! `serverInfo.version`.
//!
//! - **readiness**: starts `initializing`. `$/progress` with the fixed token `"ANALYZING"`
//!   (title "Analyzing…"): begin -> `indexing`, end -> `ready`. The pair repeats on every
//!   analysis round (a `didChange`, an on-disk change, or even nothing to analyze right after
//!   `initialized`, measured 2 ms), the same shape as rust-analyzer's `quiescent`. The token is
//!   re-created with `window/workDoneProgress/create` before every round; lsp-det already
//!   answers that request generically regardless of the client's own declaration
//!   (`proxy.rs`), and this mapping never reads create requests at all, so a repeated create for
//!   the same token is not a problem here
//! - **no prediction** (`observe_client` is not implemented): the server itself holds a request
//!   until the analysis it depends on completes (`requireResolvedUnit` in
//!   `handler_references.dart`), so a request made during `indexing` is answered complete once
//!   `ready`, never with an empty or partial result. There is therefore nothing for the
//!   observer to predict from `textDocument/didChange` or `workspace/didChangeWatchedFiles`.
//!   Dart also does not read the `workspace/didChangeWatchedFiles` lsp-det forwards or stands
//!   in for (ADR 0015): it does its own file watching and answers the notification with a
//!   `window/showMessage` (type 1) "Unknown method workspace/didChangeWatchedFiles"
//!   (docs/upstream-submissions.md has a proposal to accept it silently)
//! - **health**: no signal. `unknown` (spec 8.2 item 3)
//!
//! `coverage` / `freshness` are declared only for versions ([`TESTED_VERSIONS`]) for which
//! conformance tests 7.1 / 7.2 / 7.3 were run against a real Dart analysis server and passed
//! (spec 8.2 item 5).

use serde::Deserialize;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{ALL_FILE_CHANGES, Readiness, ServerState, ServerStateProvider};

/// The name Dart calls itself in `InitializeResult.serverInfo.name`, already lowercased for the
/// case-insensitive comparison [`super::select`] does.
pub const SERVER_NAME: &str = "dart sdk lsp analysis server";

const PROGRESS_METHOD: &str = "$/progress";
/// The fixed token of the analysis progress (`pkg/analysis_server`'s LSP handlers).
const ANALYZING_TOKEN: &str = "ANALYZING";

/// Versions for which conformance tests 7.1 / 7.2 / 7.3 were run against a real Dart analysis
/// server and passed. Matched by exact equality against `serverInfo.version`. No guarantee is
/// declared for a version not in the list. When adding one, run
/// `cargo test --test conformance -- --ignored dart` against that version first (declaring a
/// guarantee that cannot be kept violates spec 5.1).
///
/// Record of versions passed: 3.13.0 (nixpkgs `dart`, flake.nix `servers`), 2026-09-06,
/// 3 consecutive runs.
pub const TESTED_VERSIONS: &[&str] = &["3.13.0"];

#[derive(Deserialize)]
struct ProgressParams {
    token: String,
    value: ProgressValue,
}

#[derive(Deserialize)]
struct ProgressValue {
    kind: String,
}

pub struct DartAdapter {
    /// Whether the announced version is in [`TESTED_VERSIONS`]. The condition for declaring a
    /// guarantee.
    version_is_tested: bool,
    state: ServerState,
}

impl Default for DartAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl DartAdapter {
    /// For a Dart analysis server that does not announce a version. Declares no guarantee.
    pub fn new() -> Self {
        Self::for_version(None)
    }

    /// Looks at `serverInfo.version` and declares a guarantee if it is a tested version.
    pub fn for_version(version: Option<&str>) -> Self {
        let version_is_tested = version.is_some_and(|v| TESTED_VERSIONS.contains(&v));
        DartAdapter {
            version_is_tested,
            state: ServerState::initializing(),
        }
    }
}

impl Mapping for DartAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// The guarantee to declare (spec chapter 5). Declared only for [`TESTED_VERSIONS`] (spec
    /// 8.2 item 5): the server holds a request until the analysis it depends on completes
    /// (verified by conformance tests 7.1 / 7.2 against a real server), and incorporates
    /// on-disk changes through its own file watching even though the
    /// `workspace/didChangeWatchedFiles` stand-in (ADR 0015) is not read (verified by 7.3).
    fn guarantees(&self) -> ServerStateProvider {
        if self.version_is_tested {
            ServerStateProvider::workspace(&[], &ALL_FILE_CHANGES)
        } else {
            ServerStateProvider::notifications_only()
        }
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
        if envelope.params.token != ANALYZING_TOKEN {
            return None;
        }
        match envelope.params.value.kind.as_str() {
            "begin" => self.state.readiness = Readiness::Indexing,
            "end" => self.state.readiness = Readiness::Ready,
            _ => return None,
        }
        Some(self.state.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;
    use crate::state::Health;

    fn progress(token: &str, kind: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"$/progress","params":{{"token":"{token}","value":{{"kind":"{kind}"}}}}}}"#
        )
    }

    fn feed(adapter: &mut DartAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    #[test]
    fn starts_initializing() {
        let adapter = DartAdapter::new();
        assert_eq!(adapter.initial_state().readiness, Readiness::Initializing);
    }

    #[test]
    fn a_begin_of_the_analyzing_token_is_indexing_and_its_end_is_ready() {
        let mut m = DartAdapter::new();
        let state = feed(&mut m, &progress("ANALYZING", "begin"))
            .expect("a begin of the analysis token is a signal");
        assert_eq!(state.readiness, Readiness::Indexing);
        assert_eq!(state.health, Health::Unknown);
        let state =
            feed(&mut m, &progress("ANALYZING", "end")).expect("the matching end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn repeats_on_every_later_round() {
        let mut m = DartAdapter::new();
        feed(&mut m, &progress("ANALYZING", "begin"));
        feed(&mut m, &progress("ANALYZING", "end"));
        let state = feed(&mut m, &progress("ANALYZING", "begin"))
            .expect("a later begin (a new analysis round) is a signal");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state = feed(&mut m, &progress("ANALYZING", "end")).expect("its end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn ignores_other_tokens() {
        let mut m = DartAdapter::new();
        assert!(feed(&mut m, &progress("some-other-token", "begin")).is_none());
        assert!(feed(&mut m, &progress("some-other-token", "end")).is_none());
        assert_eq!(m.initial_state().readiness, Readiness::Initializing);
    }

    #[test]
    fn ignores_a_progress_that_happens_to_use_the_analyzing_token_as_a_request() {
        let mut m = DartAdapter::new();
        let as_request = r#"{"jsonrpc":"2.0","id":1,"method":"$/progress","params":{"token":"ANALYZING","value":{"kind":"begin"}}}"#;
        let view = peek(as_request.as_bytes()).unwrap();
        assert!(m.interpret(&view, as_request.as_bytes()).is_none());
    }

    #[test]
    fn health_stays_unknown_regardless_of_readiness() {
        let mut m = DartAdapter::new();
        assert_eq!(
            feed(&mut m, &progress("ANALYZING", "begin"))
                .unwrap()
                .health,
            Health::Unknown
        );
        assert_eq!(
            feed(&mut m, &progress("ANALYZING", "end")).unwrap().health,
            Health::Unknown
        );
    }

    #[test]
    fn declares_a_guarantee_only_for_the_tested_version() {
        let tested = DartAdapter::for_version(Some("3.13.0"));
        // freshness for `didChange` only: on-disk changes are noticed by the server's own
        // watcher asynchronously, with no signal before its `ANALYZING` begin (see the
        // module documentation).
        assert_eq!(
            tested.guarantees(),
            ServerStateProvider::workspace(&[], &[])
        );
        let untested = DartAdapter::for_version(Some("3.12.0"));
        assert_eq!(
            untested.guarantees(),
            ServerStateProvider::notifications_only()
        );
        let unversioned = DartAdapter::new();
        assert_eq!(
            unversioned.guarantees(),
            ServerStateProvider::notifications_only()
        );
    }
}
