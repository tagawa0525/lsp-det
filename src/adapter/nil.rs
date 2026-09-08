//! The mapping for nil (oxalica/nil) (M26, ADR 0021 decision C row for nil;
//! research/nil-readiness-measurement.md).
//!
//! Identified by `serverInfo.name` "nil" exactly; the version is `serverInfo.version`
//! ("2026-07-23" for the tested build).
//!
//! - **readiness**: starts `initializing`. nil reads flake.lock right after `initialized`, but
//!   that read carries no signal of its own (research doc, "startup and index-dependent requests" section): the
//!   first observable event is the begin of one of three fixed-string `$/progress` tokens --
//!   `nil/loadNixosOptionsProgress` (once the `nixpkgs` input's store path is resolved),
//!   `nil/loadInputFlakeProgress` (opt-in `nix.flake.autoEvalInputs`), and
//!   `nil/flakeArchiveProgress` (after a `window/showMessageRequest` answer starts fetching a
//!   missing input) -- each preceded by its own `window/workDoneProgress/create`. A begin adds
//!   its token to the set of unfinished loads and moves readiness to `indexing`; the matching
//!   end removes it, and readiness becomes `ready` only once the set is empty. A later begin
//!   after `ready` -- a reload nil starts on its own after a `workspace/didChangeWatchedFiles`
//!   Changed for flake.lock, or a `didOpen` / `didChange` of flake.nix -- goes back to
//!   `indexing`. Other tokens, `report` values, and ends of tokens not in the open set are
//!   ignored: an end of an unknown token must never make the state `ready` by itself. A
//!   workspace with no flake, no flake.lock, no `nixpkgs` input, or no store path for it sends
//!   no begin at all, and this mapping stays `initializing` (choice (a) of the record's
//!   "mapping (design) and open points" section, like clangd without a compilation database, ADR 0020
//!   decision (a) -- the alternative, `unknown` until the first begin, is not implemented; the
//!   choice between them is pending the user's decision)
//! - **no prediction** (`observe_client` is not implemented): the only 7.0 method that depends
//!   on flake information is `textDocument/definition` on an input, and unlike nixd, nil does
//!   not hold it -- it answers from a snapshot right away. Right after flake.lock's own
//!   (signal-less) read, that answer is already complete regardless of whether the NixOS
//!   options load is still running (that load affects completion and hover, not this request;
//!   research doc, "startup and index-dependent requests" section), so once a begin has been observed the answer
//!   can be trusted; a request sent in the brief window before the first begin (before
//!   flake.lock is read) answers empty instead -- a real gap this mapping does not predict its
//!   way around. It is covered by the observer's own hold (spec chapter 9), driven by this
//!   mapping's readiness staying `initializing` until that first begin, for a client that has
//!   not declared the protocol itself. `textDocument/references` is limited to the requesting
//!   document's own uses (Nix name resolution does not follow `import` across files, ADR 0021
//!   decision D), so there is nothing else index-dependent to predict from `didChange` /
//!   `workspace/didChangeWatchedFiles` (nil registers and reads the latter for flake.nix and
//!   flake.lock itself, and re-emits begin / end on a change)
//! - **health**: `window/showMessage` type 1 -> `error`, type 2 -> `warning` (only the type is
//!   read; the message text is not parsed). The begin of any of the three tokens -> `ok`
//!   (flake.lock was read and the input exists, or fetching one that was missing has started).
//!   Types 3 and 4 are ignored
//!
//! `guarantees()` is `notifications_only()` for every version, for the same reason as nixd
//! (ADR 0021 decision D): `references` is limited to the requesting document, a scope spec
//! chapter 5's `coverage.scope` has no name for yet. ADR 0021 decision E leaves the question of
//! naming that scope to the maintainer; until it is answered no guarantee is declared for any
//! version, so there is no `TESTED_VERSIONS` here (research doc's "mapping (design)" section).

use serde::Deserialize;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{Health, Readiness, ServerState, ServerStateProvider};

/// The name nil calls itself in `InitializeResult.serverInfo.name`, already lowercased for the
/// case-insensitive comparison [`super::select`] does.
pub const SERVER_NAME: &str = "nil";

const PROGRESS_METHOD: &str = "$/progress";
const SHOW_MESSAGE_METHOD: &str = "window/showMessage";

/// LSP `MessageType.Error`.
const SHOW_MESSAGE_ERROR: u8 = 1;
/// LSP `MessageType.Warning`.
const SHOW_MESSAGE_WARNING: u8 = 2;

/// The three fixed `$/progress` tokens nil sends for the phases that read flake information:
/// NixOS options evaluation, input flake evaluation (opt-in `nix.flake.autoEvalInputs`), and
/// fetching a flake archive (after a `window/showMessageRequest` answer). flake.lock's own read
/// has no signal (research doc, "startup and index-dependent requests" section): the first observable event is
/// one of these three begins.
const KNOWN_TOKENS: &[&str] = &[
    "nil/loadNixosOptionsProgress",
    "nil/loadInputFlakeProgress",
    "nil/flakeArchiveProgress",
];

#[derive(Deserialize)]
struct ProgressParams {
    token: String,
    value: ProgressValue,
}

#[derive(Deserialize)]
struct ProgressValue {
    kind: String,
}

#[derive(Deserialize)]
struct ShowMessageParams {
    #[serde(rename = "type")]
    kind: u8,
    message: String,
}

pub struct NilAdapter {
    state: ServerState,
    /// Tokens among [`KNOWN_TOKENS`] that began and have not yet ended. `ready` only once this
    /// is empty.
    open: Vec<String>,
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
            open: Vec::new(),
        }
    }

    // TODO(GREEN): look at TESTED_VERSIONS and declare document_only(&[]) for a tested version
    // (ADR 0021 decision E, answer (b)).
    pub fn for_version(_version: Option<&str>) -> Self {
        Self::new()
    }

    fn on_progress(&mut self, params: ProgressParams) -> Option<ServerState> {
        let ProgressParams { token, value } = params;
        match value.kind.as_str() {
            "begin" => {
                if !KNOWN_TOKENS.contains(&token.as_str()) {
                    return None;
                }
                self.open.push(token);
                let next = ServerState {
                    readiness: Readiness::Indexing,
                    health: Health::Ok,
                    message: None,
                };
                if next == self.state {
                    return None;
                }
                self.state = next;
                Some(self.state.clone())
            }
            "end" => {
                let index = self.open.iter().position(|t| *t == token)?;
                self.open.remove(index);
                if !self.open.is_empty() {
                    return None;
                }
                self.state.readiness = Readiness::Ready;
                Some(self.state.clone())
            }
            _ => None,
        }
    }

    fn on_show_message(&mut self, params: ShowMessageParams) -> Option<ServerState> {
        let health = match params.kind {
            SHOW_MESSAGE_ERROR => Health::Error,
            SHOW_MESSAGE_WARNING => Health::Warning,
            _ => return None,
        };
        let next = ServerState {
            health,
            message: Some(params.message),
            ..self.state.clone()
        };
        if next == self.state {
            return None;
        }
        self.state = next;
        Some(self.state.clone())
    }
}

impl Mapping for NilAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// Never a guarantee, whatever the version (see the module documentation: ADR 0021
    /// decision E is pending).
    fn guarantees(&self) -> ServerStateProvider {
        ServerStateProvider::notifications_only()
    }

    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() {
            return None;
        }
        match view.method()? {
            PROGRESS_METHOD => {
                #[derive(Deserialize)]
                struct Envelope {
                    params: ProgressParams,
                }
                let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
                self.on_progress(envelope.params)
            }
            SHOW_MESSAGE_METHOD => {
                #[derive(Deserialize)]
                struct Envelope {
                    params: ShowMessageParams,
                }
                let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
                self.on_show_message(envelope.params)
            }
            _ => None,
        }
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

    #[test]
    fn declares_a_guarantee_only_for_the_tested_version() {
        let tested = NilAdapter::for_version(Some("2026-07-23"));
        assert_eq!(tested.guarantees(), ServerStateProvider::document_only(&[]));
        let untested = NilAdapter::for_version(Some("2026-07-22"));
        assert_eq!(
            untested.guarantees(),
            ServerStateProvider::notifications_only()
        );
        let unversioned = NilAdapter::new();
        assert_eq!(
            unversioned.guarantees(),
            ServerStateProvider::notifications_only()
        );
    }

    #[test]
    fn a_tested_guarantee_declares_no_freshness() {
        let tested = NilAdapter::for_version(Some("2026-07-23"));
        let json = serde_json::to_string(&tested.guarantees()).unwrap();
        assert!(
            !json.contains("freshness"),
            "nil must not declare freshness: {json}"
        );
        assert!(
            json.contains("coverage"),
            "nil must declare coverage: {json}"
        );
    }
}
