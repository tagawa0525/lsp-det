//! The mapping for nixd (M25, ADR 0021 decision C row for nixd;
//! research/nixd-readiness-measurement.md).
//!
//! Identified by `serverInfo.name` "nixd" exactly; the version is `serverInfo.version`
//! ("2.9.2" for the tested build).
//!
//! - **readiness**: starts `initializing`. Right after the `initialize` response, nixd sends
//!   `window/workDoneProgress/create` and then a `$/progress` begin for every evaluation it
//!   starts -- nixpkgs entries and NixOS options run in parallel from the start (2 by default,
//!   more with user configuration). Tokens are `rand()` integers (JSON numbers, never
//!   strings). A begin whose title starts with "evaluating " adds its token to the set of
//!   unfinished evaluations and moves readiness to `indexing`; the matching end removes it,
//!   and readiness becomes `ready` only once the set is empty (several evaluations run in
//!   parallel, so one ending is not enough). A later begin after `ready` -- a re-evaluation
//!   nixd starts after a `workspace/configuration` answer changes an expression, driven by
//!   `didChangeConfiguration` -- goes back to `indexing`, and `ready` again once every token
//!   from that round has ended too. Begins with other titles, `report` values (nixd sends
//!   none), and ends of tokens not in the open set are ignored: an end of an unknown token
//!   must never make the state `ready` by itself
//! - **no prediction** (`observe_client` is not implemented): nixd's controller holds the RPC
//!   to the evaluation worker for an index-dependent `definition` (the only 7.0 method that
//!   depends on an evaluation; `references` is document-local, see below) until the evaluation
//!   it depends on finishes, so a request made while evaluating is answered complete once the
//!   round ends, never with an empty or partial result. There is therefore nothing for the
//!   observer to predict from `textDocument/didChange` -- and nixd does not read
//!   `workspace/didChangeWatchedFiles` at all (no `client/registerCapability`; a notification
//!   sent anyway logs "unhandled notification" to stderr)
//! - **health**: no signal. A failed evaluation's end carries the same "evaluated ..." message
//!   as a successful one (only stderr, which is not part of the protocol, distinguishes them),
//!   and a dead evaluation worker takes the whole nixd process down with it (SIGPIPE on the
//!   next RPC to it) rather than reporting anything on the protocol first, so `unknown`
//!   (spec 8.2 item 3)
//!
//! `guarantees()` is `notifications_only()` for every version: nixd's `references` answers
//! only the requesting document's own uses (Nix name resolution does not follow `import`
//! across files), a scope spec chapter 5's `coverage.scope` has no name for yet (both
//! `"workspace"` and `"openDocuments"` would be false), so no `coverage` can be declared
//! honestly. ADR 0021 decision E leaves the question of naming that scope to the maintainer;
//! until it is answered no guarantee is declared for any version, so there is no
//! `TESTED_VERSIONS` here (research doc's "mapping (design)" section).

use serde::Deserialize;
use serde_json::Value;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{Readiness, ServerState, ServerStateProvider};

/// The name nixd calls itself in `InitializeResult.serverInfo.name`, already lowercased for
/// the case-insensitive comparison [`super::select`] does.
pub const SERVER_NAME: &str = "nixd";

const PROGRESS_METHOD: &str = "$/progress";
/// The prefix common to every evaluation's begin title ("evaluating nixpkgs entries",
/// "evaluating nixos options", and, with user configuration, "evaluating nixos" or
/// "evaluating <name>"). A title that does not start with this is unrelated progress this
/// mapping does not read (nixd sends no other `$/progress` at all, but nothing guarantees
/// that stays true).
const EVALUATING_PREFIX: &str = "evaluating ";

#[derive(Deserialize)]
struct ProgressParams {
    token: Value,
    value: ProgressValue,
}

#[derive(Deserialize)]
struct ProgressValue {
    kind: String,
    #[serde(default)]
    title: Option<String>,
}

pub struct NixdAdapter {
    state: ServerState,
    /// Tokens of evaluations that began with a title starting with "evaluating " and have not
    /// yet ended. Several run in parallel (2 by default, more with configuration), so `ready`
    /// only once this is empty.
    open: Vec<Value>,
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
                if !value.title.as_deref()?.starts_with(EVALUATING_PREFIX) {
                    return None;
                }
                self.open.push(token);
                self.state.readiness = Readiness::Indexing;
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
}

impl Mapping for NixdAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// Never a guarantee, whatever the version (see the module documentation: ADR 0021
    /// decision E is pending).
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

    #[test]
    fn declares_a_guarantee_only_for_the_tested_version() {
        let tested = NixdAdapter::for_version(Some("2.9.2"));
        assert_eq!(tested.guarantees(), ServerStateProvider::document_only(&[]));
        let untested = NixdAdapter::for_version(Some("2.9.1"));
        assert_eq!(
            untested.guarantees(),
            ServerStateProvider::notifications_only()
        );
        let unversioned = NixdAdapter::new();
        assert_eq!(
            unversioned.guarantees(),
            ServerStateProvider::notifications_only()
        );
    }

    #[test]
    fn a_tested_guarantee_declares_no_freshness() {
        let tested = NixdAdapter::for_version(Some("2.9.2"));
        let json = serde_json::to_string(&tested.guarantees()).unwrap();
        assert!(
            !json.contains("freshness"),
            "nixd must not declare freshness: {json}"
        );
        assert!(
            json.contains("coverage"),
            "nixd must declare coverage: {json}"
        );
    }
}
