//! The mapping for nixd (M25, ADR 0021 decision C row for nixd;
//! research/nixd-readiness-measurement.md).
//!
//! Identified by `serverInfo.name` "nixd" exactly; the version is `serverInfo.version`
//! ("2.9.2" for the tested build).

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{ServerState, ServerStateProvider};

/// The name nixd calls itself in `InitializeResult.serverInfo.name`, already lowercased for
/// the case-insensitive comparison [`super::select`] does.
pub const SERVER_NAME: &str = "nixd";

pub struct NixdAdapter {
    // RED (M25): held only for the tests below to read directly; GREEN's `on_progress` (the
    // "evaluating " rule) reads and updates it too, which will make this field's ordinary
    // (non-test) use non-dead.
    #[allow(dead_code)]
    state: ServerState,
}

impl Default for NixdAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl NixdAdapter {
    pub fn new() -> Self {
        NixdAdapter {
            state: ServerState::initializing(),
        }
    }
}

impl Mapping for NixdAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// Never a guarantee, whatever the version (ADR 0021 decision E is pending).
    fn guarantees(&self) -> ServerStateProvider {
        ServerStateProvider::notifications_only()
    }

    /// RED (M25): the "evaluating " `$/progress` rule is not implemented yet. Reads nothing.
    fn interpret(&mut self, _view: &MessageView, _body: &[u8]) -> Option<ServerState> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;
    use crate::state::{Health, Readiness};

    /// The two fixed token values a real nixd was observed to send in the first run
    /// (research/nixd-readiness-measurement.md); any JSON-number token works, these are just
    /// realistic values.
    const NIXPKGS_TOKEN: i64 = 1804289383;
    const NIXOS_OPTIONS_TOKEN: i64 = 846930886;

    fn progress(token: i64, kind: &str, title: Option<&str>) -> String {
        let title = title
            .map(|t| format!(r#","title":"{t}""#))
            .unwrap_or_default();
        format!(
            r#"{{"jsonrpc":"2.0","method":"$/progress","params":{{"token":{token},"value":{{"kind":"{kind}"{title}}}}}}}"#
        )
    }

    fn feed(adapter: &mut NixdAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    #[test]
    fn starts_initializing() {
        let m = NixdAdapter::new();
        assert_eq!(m.initial_state().readiness, Readiness::Initializing);
        assert_eq!(m.initial_state().health, Health::Unknown);
    }

    #[test]
    fn a_begin_of_an_evaluating_token_is_indexing_and_its_end_is_ready() {
        let mut m = NixdAdapter::new();
        let state = feed(
            &mut m,
            &progress(NIXPKGS_TOKEN, "begin", Some("evaluating nixpkgs entries")),
        )
        .expect("a begin whose title starts with \"evaluating \" is a signal");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state = feed(&mut m, &progress(NIXPKGS_TOKEN, "end", None))
            .expect("the matching end, with nothing else open, is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn two_parallel_tokens_the_first_end_keeps_indexing_and_the_second_is_ready() {
        let mut m = NixdAdapter::new();
        feed(
            &mut m,
            &progress(NIXPKGS_TOKEN, "begin", Some("evaluating nixpkgs entries")),
        );
        feed(
            &mut m,
            &progress(
                NIXOS_OPTIONS_TOKEN,
                "begin",
                Some("evaluating nixos options"),
            ),
        );
        assert!(
            feed(&mut m, &progress(NIXPKGS_TOKEN, "end", None)).is_none(),
            "the nixos options token is still open"
        );
        assert_eq!(m.state.readiness, Readiness::Indexing);
        let state = feed(&mut m, &progress(NIXOS_OPTIONS_TOKEN, "end", None))
            .expect("the last open token ended");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn a_later_begin_after_ready_reindexes_and_its_end_is_ready_again() {
        let mut m = NixdAdapter::new();
        feed(
            &mut m,
            &progress(NIXPKGS_TOKEN, "begin", Some("evaluating nixpkgs entries")),
        );
        feed(&mut m, &progress(NIXPKGS_TOKEN, "end", None));
        assert_eq!(m.state.readiness, Readiness::Ready);
        // A re-evaluation after `didChangeConfiguration` (research doc, run 6).
        let state = feed(
            &mut m,
            &progress(NIXPKGS_TOKEN, "begin", Some("evaluating nixpkgs entries")),
        )
        .expect("a begin while ready moves back to indexing");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state =
            feed(&mut m, &progress(NIXPKGS_TOKEN, "end", None)).expect("its end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn ends_of_unknown_tokens_are_ignored() {
        let mut m = NixdAdapter::new();
        feed(
            &mut m,
            &progress(NIXPKGS_TOKEN, "begin", Some("evaluating nixpkgs entries")),
        );
        assert!(
            feed(&mut m, &progress(999, "end", None)).is_none(),
            "an end of a token that never began must not be read"
        );
        assert_eq!(
            m.state.readiness,
            Readiness::Indexing,
            "an end of an unknown token must not make the state ready by itself"
        );
    }

    #[test]
    fn other_titles_are_ignored() {
        let mut m = NixdAdapter::new();
        assert!(
            feed(
                &mut m,
                &progress(NIXPKGS_TOKEN, "begin", Some("Something else"))
            )
            .is_none()
        );
        assert!(
            feed(&mut m, &progress(NIXPKGS_TOKEN, "begin", None)).is_none(),
            "a begin with no title at all is not an evaluation begin"
        );
        assert_eq!(m.state.readiness, Readiness::Initializing);
    }

    #[test]
    fn ignores_a_progress_that_happens_to_use_the_token_as_a_request() {
        let mut m = NixdAdapter::new();
        let as_request = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"$/progress","params":{{"token":{NIXPKGS_TOKEN},"value":{{"kind":"begin","title":"evaluating nixpkgs entries"}}}}}}"#
        );
        let view = peek(as_request.as_bytes()).unwrap();
        assert!(m.interpret(&view, as_request.as_bytes()).is_none());
        assert_eq!(m.state.readiness, Readiness::Initializing);
    }

    #[test]
    fn health_stays_unknown_regardless_of_readiness() {
        let mut m = NixdAdapter::new();
        assert_eq!(
            feed(
                &mut m,
                &progress(NIXPKGS_TOKEN, "begin", Some("evaluating nixpkgs entries"))
            )
            .unwrap()
            .health,
            Health::Unknown
        );
        assert_eq!(
            feed(&mut m, &progress(NIXPKGS_TOKEN, "end", None))
                .unwrap()
                .health,
            Health::Unknown
        );
    }

    #[test]
    fn declares_notifications_only_regardless_of_version() {
        // No `TESTED_VERSIONS`, no `for_version`: nixd's version is never looked at, because
        // no guarantee is declared for any version until ADR 0021 decision E is answered.
        assert_eq!(
            NixdAdapter::new().guarantees(),
            ServerStateProvider::notifications_only()
        );
    }
}
