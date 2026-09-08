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
//! `coverage: {scope: "document", incomplete: {}}` is declared only for versions
//! ([`TESTED_VERSIONS`]) for which the conformance test of 7.2 item 1 was run against a real nil
//! and passed (spec 8.2 item 5): `references` is limited to the requesting document, for the
//! same reason as nixd (ADR 0021 decision D), which spec chapter 5's `"document"` scope names
//! (ADR 0021 decision E, answer (b)). No `freshness` is ever declared: 7.3's guarantee is
//! inherently cross-file, which cannot be constructed for a document-local server (`didChange`
//! is already covered by LSP's own ordering guarantee).

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

/// Versions for which the conformance test of 7.2 item 1 was run against a real nil and passed.
/// Matched by exact equality against `serverInfo.version`. No guarantee is declared for a
/// version not in the list. When adding one, run
/// `cargo test --test conformance -- --ignored nil_` against that version first (declaring a
/// guarantee that cannot be kept violates spec 5.1).
///
/// Record of versions passed: "2026-07-23" (nixpkgs `nil`, flake.nix `servers`), 2026-09-08.
pub const TESTED_VERSIONS: &[&str] = &["2026-07-23"];

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
    /// Whether the announced version is in [`TESTED_VERSIONS`]. The condition for declaring a
    /// guarantee.
    version_is_tested: bool,
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
    /// For a nil that does not announce a version. Declares no guarantee.
    pub fn new() -> Self {
        Self::for_version(None)
    }

    /// Looks at `serverInfo.version` and declares a guarantee if it is a tested version.
    pub fn for_version(version: Option<&str>) -> Self {
        let version_is_tested = version.is_some_and(|v| TESTED_VERSIONS.contains(&v));
        NilAdapter {
            version_is_tested,
            state: ServerState::initializing(),
            open: Vec::new(),
        }
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

    /// The guarantee to declare (spec chapter 5). Declared only for [`TESTED_VERSIONS`] (spec
    /// 8.2 item 5): `references` is limited to the requesting document (ADR 0021 decision E,
    /// answer (b)). No `freshness`: 7.3 needs a cross-file query, which cannot be constructed
    /// for a document-local server.
    fn guarantees(&self) -> ServerStateProvider {
        if self.version_is_tested {
            ServerStateProvider::document_only(&[])
        } else {
            ServerStateProvider::notifications_only()
        }
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
    use std::path::PathBuf;

    use serde_json::Value;

    use super::*;
    use crate::peek::peek;
    use crate::state::{Health, Readiness};

    /// A throwaway directory for a fake workspace folder, cleaned up on drop. No dependency is
    /// added for this (`CLAUDE.md`'s absolute constraint): `std::env::temp_dir()` plus a name
    /// unique to the test and the process.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("lsp-det-nil-adapter-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("cannot create the temporary directory");
            TempDir { path }
        }

        /// Writes a `flake.lock` whose root node's inputs are exactly `names`.
        fn write_flake_lock(&self, names: &[&str]) {
            let mut inputs = serde_json::Map::new();
            for name in names {
                inputs.insert((*name).to_string(), Value::String(format!("{name}-node")));
            }
            let value = serde_json::json!({
                "nodes": {"root": {"inputs": Value::Object(inputs)}},
                "root": "root",
                "version": 7,
            });
            std::fs::write(
                self.path.join("flake.lock"),
                serde_json::to_string_pretty(&value).unwrap(),
            )
            .unwrap();
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

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

    // ADR 0021 addendum 2026-09-09, decision (b): a workspace whose `flake.lock` root node has
    // no `nixpkgs` input never sends a `$/progress` begin, so this mapping starts `unknown`
    // instead of `initializing` there.

    #[test]
    fn a_workspace_folder_with_a_nixpkgs_flake_lock_starts_initializing() {
        let dir = TempDir::new("with-nixpkgs");
        dir.write_flake_lock(&["nixpkgs"]);
        let mut m = NilAdapter::new();
        m.learn_workspace_folders(std::slice::from_ref(&dir.path));
        assert_eq!(m.initial_state().readiness, Readiness::Initializing);
        assert_eq!(m.initial_state().health, Health::Unknown);
    }

    #[test]
    fn a_workspace_folder_without_a_flake_lock_starts_unknown() {
        let dir = TempDir::new("no-flake-lock");
        let mut m = NilAdapter::new();
        m.learn_workspace_folders(std::slice::from_ref(&dir.path));
        assert_eq!(m.initial_state().readiness, Readiness::Unknown);
    }

    #[test]
    fn a_flake_lock_whose_root_has_no_nixpkgs_input_starts_unknown() {
        let dir = TempDir::new("no-nixpkgs-input");
        dir.write_flake_lock(&["some-other-input"]);
        let mut m = NilAdapter::new();
        m.learn_workspace_folders(std::slice::from_ref(&dir.path));
        assert_eq!(m.initial_state().readiness, Readiness::Unknown);
    }

    #[test]
    fn a_type_2_show_message_before_any_begin_moves_readiness_to_unknown_too() {
        let mut m = NilAdapter::new();
        let state = feed(
            &mut m,
            &show_message(2, "Some flake inputs are not available"),
        )
        .expect("a type 2 message is a signal");
        assert_eq!(state.readiness, Readiness::Unknown);
        assert_eq!(state.health, Health::Warning);
    }

    #[test]
    fn unknown_from_a_workspace_without_a_nixpkgs_flake_lock_still_indexes_and_becomes_ready_on_a_begin()
     {
        let dir = TempDir::new("no-flake-lock-then-begin");
        let mut m = NilAdapter::new();
        m.learn_workspace_folders(std::slice::from_ref(&dir.path));
        assert_eq!(m.initial_state().readiness, Readiness::Unknown);
        let state = feed(&mut m, &progress(LOAD_NIXOS_OPTIONS_TOKEN, "begin"))
            .expect("a begin is a signal even from a workspace that started unknown");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state = feed(&mut m, &progress(LOAD_NIXOS_OPTIONS_TOKEN, "end"))
            .expect("the matching end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn unknown_from_a_pre_begin_show_message_still_indexes_and_becomes_ready_on_a_begin() {
        let mut m = NilAdapter::new();
        feed(
            &mut m,
            &show_message(2, "Some flake inputs are not available"),
        );
        assert_eq!(m.state.readiness, Readiness::Unknown);
        let state = feed(&mut m, &progress(LOAD_NIXOS_OPTIONS_TOKEN, "begin"))
            .expect("a begin is a signal even from unknown");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state = feed(&mut m, &progress(LOAD_NIXOS_OPTIONS_TOKEN, "end"))
            .expect("the matching end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }
}
