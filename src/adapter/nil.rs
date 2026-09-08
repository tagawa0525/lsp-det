//! The mapping for nil (oxalica/nil) (M26, ADR 0021 decision C row for nil;
//! research/nil-readiness-measurement.md).
//!
//! Identified by `serverInfo.name` "nil" exactly; the version is `serverInfo.version`
//! ("2026-07-23" for the tested build).

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{ServerState, ServerStateProvider};

/// The name nil calls itself in `InitializeResult.serverInfo.name`, already lowercased for the
/// case-insensitive comparison [`super::select`] does.
pub const SERVER_NAME: &str = "nil";

pub struct NilAdapter {
    // RED (M26): held only for the tests below to read directly; GREEN's `on_progress` /
    // `on_show_message` (the fixed-token `$/progress` and `window/showMessage` rules) read and
    // update it too, which will make this field's ordinary (non-test) use non-dead.
    #[allow(dead_code)]
    state: ServerState,
}

impl Default for NilAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl NilAdapter {
    pub fn new() -> Self {
        NilAdapter {
            state: ServerState::initializing(),
        }
    }
}

impl Mapping for NilAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// Never a guarantee, whatever the version (ADR 0021 decision E is pending).
    fn guarantees(&self) -> ServerStateProvider {
        ServerStateProvider::notifications_only()
    }

    /// RED (M26): the fixed-token `$/progress` and `window/showMessage` rules are not
    /// implemented yet. Reads nothing.
    fn interpret(&mut self, _view: &MessageView, _body: &[u8]) -> Option<ServerState> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;
    use crate::state::{Health, Readiness};

    /// The three fixed `$/progress` tokens a real nil was observed to send
    /// (research/nil-readiness-measurement.md): NixOS options evaluation, input flake
    /// evaluation (opt-in `nix.flake.autoEvalInputs`), and fetching a flake archive (after a
    /// `window/showMessageRequest` answer). flake.lock's own read carries no signal.
    const LOAD_NIXOS_OPTIONS_TOKEN: &str = "nil/loadNixosOptionsProgress";
    const LOAD_INPUT_FLAKE_TOKEN: &str = "nil/loadInputFlakeProgress";
    const FLAKE_ARCHIVE_TOKEN: &str = "nil/flakeArchiveProgress";

    fn progress(token: &str, kind: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"$/progress","params":{{"token":"{token}","value":{{"kind":"{kind}"}}}}}}"#
        )
    }

    fn show_message(kind: u8, message: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"window/showMessage","params":{{"type":{kind},"message":"{message}"}}}}"#
        )
    }

    fn feed(adapter: &mut NilAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    #[test]
    fn starts_initializing() {
        let m = NilAdapter::new();
        assert_eq!(m.initial_state().readiness, Readiness::Initializing);
        assert_eq!(m.initial_state().health, Health::Unknown);
    }

    #[test]
    fn a_begin_of_a_known_token_is_indexing_and_its_end_is_ready() {
        let mut m = NilAdapter::new();
        let state = feed(&mut m, &progress(LOAD_NIXOS_OPTIONS_TOKEN, "begin"))
            .expect("a begin of a known token is a signal");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state = feed(&mut m, &progress(LOAD_NIXOS_OPTIONS_TOKEN, "end"))
            .expect("the matching end, with nothing else open, is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn two_known_tokens_the_first_end_keeps_indexing_and_the_second_is_ready() {
        let mut m = NilAdapter::new();
        feed(&mut m, &progress(LOAD_NIXOS_OPTIONS_TOKEN, "begin"));
        feed(&mut m, &progress(LOAD_INPUT_FLAKE_TOKEN, "begin"));
        assert!(
            feed(&mut m, &progress(LOAD_NIXOS_OPTIONS_TOKEN, "end")).is_none(),
            "the input-flake token is still open"
        );
        assert_eq!(m.state.readiness, Readiness::Indexing);
        let state = feed(&mut m, &progress(LOAD_INPUT_FLAKE_TOKEN, "end"))
            .expect("the last open token ended");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn a_later_begin_after_ready_reindexes_and_its_end_is_ready_again() {
        let mut m = NilAdapter::new();
        feed(&mut m, &progress(FLAKE_ARCHIVE_TOKEN, "begin"));
        feed(&mut m, &progress(FLAKE_ARCHIVE_TOKEN, "end"));
        assert_eq!(m.state.readiness, Readiness::Ready);
        // A reload after a `didChangeWatchedFiles` Changed for flake.lock, or a `didOpen` /
        // `didChange` of flake.nix (research doc, run 8).
        let state = feed(&mut m, &progress(FLAKE_ARCHIVE_TOKEN, "begin"))
            .expect("a begin while ready moves back to indexing");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state =
            feed(&mut m, &progress(FLAKE_ARCHIVE_TOKEN, "end")).expect("its end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn unknown_tokens_are_ignored_in_both_kinds() {
        let mut m = NilAdapter::new();
        assert!(feed(&mut m, &progress("something/else", "begin")).is_none());
        assert!(feed(&mut m, &progress("something/else", "end")).is_none());
        assert_eq!(m.state.readiness, Readiness::Initializing);
    }

    #[test]
    fn an_end_of_an_unknown_token_does_not_make_a_known_one_ready_early() {
        let mut m = NilAdapter::new();
        feed(&mut m, &progress(LOAD_NIXOS_OPTIONS_TOKEN, "begin"));
        assert!(
            feed(&mut m, &progress("something/else", "end")).is_none(),
            "an end of a token that never began must not be read"
        );
        assert_eq!(
            m.state.readiness,
            Readiness::Indexing,
            "an end of an unrelated token must not move readiness"
        );
    }

    #[test]
    fn a_report_is_ignored() {
        let mut m = NilAdapter::new();
        feed(&mut m, &progress(LOAD_NIXOS_OPTIONS_TOKEN, "begin"));
        assert!(feed(&mut m, &progress(LOAD_NIXOS_OPTIONS_TOKEN, "report")).is_none());
        assert_eq!(m.state.readiness, Readiness::Indexing);
    }

    #[test]
    fn ignores_a_progress_that_happens_to_use_the_token_as_a_request() {
        let mut m = NilAdapter::new();
        let as_request = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"$/progress","params":{{"token":"{LOAD_NIXOS_OPTIONS_TOKEN}","value":{{"kind":"begin"}}}}}}"#
        );
        let view = peek(as_request.as_bytes()).unwrap();
        assert!(m.interpret(&view, as_request.as_bytes()).is_none());
        assert_eq!(m.state.readiness, Readiness::Initializing);
    }

    #[test]
    fn show_message_error_and_warning_move_health() {
        let mut m = NilAdapter::new();
        let state = feed(
            &mut m,
            &show_message(2, "Some flake inputs are not available"),
        )
        .expect("type 2 is a signal");
        assert_eq!(state.health, Health::Warning);
        let state = feed(&mut m, &show_message(1, "Failed to load flake workspace"))
            .expect("type 1 is a signal");
        assert_eq!(state.health, Health::Error);
    }

    #[test]
    fn show_message_info_and_log_are_ignored() {
        let mut m = NilAdapter::new();
        assert!(
            feed(
                &mut m,
                &show_message(3, "Some flake inputs are not available. Fetch them now?")
            )
            .is_none()
        );
        assert!(feed(&mut m, &show_message(4, "just a log")).is_none());
        assert_eq!(m.state.health, Health::Unknown);
    }

    #[test]
    fn a_known_begin_moves_health_to_ok() {
        let mut m = NilAdapter::new();
        feed(&mut m, &show_message(1, "Failed to load flake workspace"));
        let state = feed(&mut m, &progress(LOAD_NIXOS_OPTIONS_TOKEN, "begin"))
            .expect("a known begin is a signal");
        assert_eq!(state.health, Health::Ok);
        assert_eq!(state.readiness, Readiness::Indexing);
    }

    #[test]
    fn health_changes_are_reported_even_when_readiness_does_not_move() {
        let mut m = NilAdapter::new();
        feed(&mut m, &progress(LOAD_NIXOS_OPTIONS_TOKEN, "begin"));
        feed(&mut m, &progress(LOAD_NIXOS_OPTIONS_TOKEN, "end"));
        assert_eq!(m.state.readiness, Readiness::Ready);
        let state = feed(
            &mut m,
            &show_message(2, "Some flake inputs are not available"),
        )
        .expect("a health-only change is still a signal");
        assert_eq!(state.readiness, Readiness::Ready, "readiness did not move");
        assert_eq!(state.health, Health::Warning);
    }

    #[test]
    fn declares_notifications_only_regardless_of_version() {
        // No `TESTED_VERSIONS`, no `for_version`: nil's version is never looked at, because no
        // guarantee is declared for any version until ADR 0021 decision E is answered.
        assert_eq!(
            NilAdapter::new().guarantees(),
            ServerStateProvider::notifications_only()
        );
    }
}
