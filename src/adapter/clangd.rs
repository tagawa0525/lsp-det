//! The mapping for clangd (M24, ADR 0020 decision C row for clangd;
//! research/clangd-readiness-measurement.md).
//!
//! Identified by `serverInfo.name` "clangd" (case-insensitive); the version is the whole
//! `serverInfo.version` string ("clangd version 21.1.8 linux x86_64-unknown-linux-gnu" for the
//! tested build). The platform is baked into that string, so a build for a different platform
//! at the same clangd release is a different, untested version.
//!
//! - **readiness**: starts `initializing`. LLVM's source (`ClangdLSPServer.cpp`,
//!   `onBackgroundIndexProgress`, added by D73218) sends `window/workDoneProgress/create` +
//!   `$/progress` with the fixed token `"backgroundIndexProgress"` (title "indexing") only once
//!   a compilation database (`compile_commands.json`) is found at the first `didOpen`: begin ->
//!   `indexing`, `report` is ignored, end -> `ready`. The pair repeats whenever the background
//!   index queue goes from empty to non-empty again (a later begin while `ready` -> `indexing`,
//!   its end -> `ready`). Other tokens are ignored.
//!
//!   Without a compilation database the token never arrives at all, so this mapping's starting
//!   `initializing` never moves and cross-file requests stay held indefinitely (a client-side
//!   timeout). This was an open question (research doc's mapping section) with three options:
//!   (a) keep `initializing` (correct for the ordinary case where a database exists; times out
//!   when it does not), (b) start from `unknown` instead (leaks an empty answer in the
//!   measured ~2 ms gap between `didOpen` and a begin even when a database IS present, and
//!   `unknown` cannot be told apart from "index already ran and found nothing missing"), (c)
//!   have the observer look at the filesystem for a database (cannot reproduce
//!   `--compile-commands-dir` or `.clangd`'s `CompileFlags`, the same "reproducing the server's
//!   own logic" trap as Nextflow's workspace scan). The maintainer decided (a), approved in
//!   ADR 0020's addendum
//! - **no prediction** (`observe_client` is not implemented): during begin..end, `references`
//!   answers an empty array and then a growing partial set as the index fills in (measured on a
//!   402-file fixture: 0 -> 17 -> 117 -> 217 -> 316 -> 400) -- the silent lie this protocol
//!   exists to remove, and exactly what the begin/end hold fixes. But a `didChange` on an open
//!   document has a measured 40-80 ms stale window with no signal marking its end, and neither
//!   `workspace/didChangeWatchedFiles` (clangd never registers it) nor any on-disk change is
//!   ever incorporated, so there is nothing for the observer to predict from either
//! - **health**: no signal. `unknown` (spec 8.2 item 3)
//!
//! `coverage: {scope: "workspace", incomplete: {}}` is declared only for versions
//! ([`TESTED_VERSIONS`]) for which conformance tests 7.1 / 7.2 were run against a real clangd
//! and passed (spec 8.2 item 5). No `freshness` is ever declared: the stale window after
//! `didChange` has no completion signal, and on-disk changes are never incorporated (7.3 is not
//! applicable to this mapping and is not written as a conformance test).

use serde::Deserialize;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{Readiness, ServerState, ServerStateProvider};

/// The name clangd calls itself in `InitializeResult.serverInfo.name`, already lowercased for
/// the case-insensitive comparison [`super::select`] does.
pub const SERVER_NAME: &str = "clangd";

const PROGRESS_METHOD: &str = "$/progress";
/// The fixed token of the background-index progress (`ClangdLSPServer.cpp`,
/// `onBackgroundIndexProgress`).
const BACKGROUND_INDEX_TOKEN: &str = "backgroundIndexProgress";

/// Versions for which conformance tests 7.1 / 7.2 were run against a real clangd and passed.
/// Matched by exact equality against `serverInfo.version`, which includes the platform
/// (`"clangd version 21.1.8 linux x86_64-unknown-linux-gnu"`); a build for another platform is
/// a different, untested version even at the same clangd release. No guarantee is declared for
/// a version not in the list. When adding one, run
/// `cargo test --test conformance -- --ignored clangd` against that version first (declaring a
/// guarantee that cannot be kept violates spec 5.1).
///
/// Record of versions passed: "clangd version 21.1.8 linux x86_64-unknown-linux-gnu" (nixpkgs
/// `clang-tools` 21.1.8, flake.nix `servers`), 2026-09-07.
pub const TESTED_VERSIONS: &[&str] = &["clangd version 21.1.8 linux x86_64-unknown-linux-gnu"];

#[derive(Deserialize)]
struct ProgressParams {
    token: String,
    value: ProgressValue,
}

#[derive(Deserialize)]
struct ProgressValue {
    kind: String,
}

pub struct ClangdAdapter {
    /// Whether the announced version is in [`TESTED_VERSIONS`]. The condition for declaring a
    /// guarantee.
    version_is_tested: bool,
    state: ServerState,
}

impl Default for ClangdAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ClangdAdapter {
    /// For a clangd that does not announce a version. Declares no guarantee.
    pub fn new() -> Self {
        Self::for_version(None)
    }

    /// Looks at `serverInfo.version` and declares a guarantee if it is a tested version.
    pub fn for_version(version: Option<&str>) -> Self {
        let version_is_tested = version.is_some_and(|v| TESTED_VERSIONS.contains(&v));
        ClangdAdapter {
            version_is_tested,
            state: ServerState::initializing(),
        }
    }
}

impl Mapping for ClangdAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// The guarantee to declare (spec chapter 5). Declared only for [`TESTED_VERSIONS`] (spec
    /// 8.2 item 5): the observer's begin/end hold on `$/progress` keeps every `references`
    /// answer complete once `ready` (verified by conformance tests 7.1 / 7.2 against a real
    /// server). No `freshness`: neither a `didChange`'s completion nor any on-disk change has a
    /// signal.
    fn guarantees(&self) -> ServerStateProvider {
        if self.version_is_tested {
            ServerStateProvider::coverage_only(&[])
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
        if envelope.params.token != BACKGROUND_INDEX_TOKEN {
            return None;
        }
        match envelope.params.value.kind.as_str() {
            "begin" => self.state.readiness = Readiness::Indexing,
            "end" => self.state.readiness = Readiness::Ready,
            // report: nothing this mapping reads from (spec chapter 8's rule against
            // per-request progress does not apply here -- this is a workspace-wide index, not a
            // per-request one -- but the percentage itself carries no boundary this mapping can
            // use).
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

    fn feed(adapter: &mut ClangdAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    #[test]
    fn starts_initializing() {
        let m = ClangdAdapter::new();
        assert_eq!(m.state.readiness, Readiness::Initializing);
        assert_eq!(m.state.health, Health::Unknown);
    }

    #[test]
    fn a_begin_of_the_background_index_token_is_indexing_and_its_end_is_ready() {
        let mut m = ClangdAdapter::new();
        let state = feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "begin"))
            .expect("a begin of the background index token is a signal");
        assert_eq!(state.readiness, Readiness::Indexing);
        assert_eq!(state.health, Health::Unknown);
        let state = feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "end"))
            .expect("the matching end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(m.state.readiness, Readiness::Ready);
    }

    #[test]
    fn report_is_ignored() {
        let mut m = ClangdAdapter::new();
        feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "begin"));
        assert!(
            feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "report")).is_none(),
            "report must not be read"
        );
        assert_eq!(
            m.state.readiness,
            Readiness::Indexing,
            "a report must not move readiness away from indexing"
        );
    }

    #[test]
    fn a_later_begin_after_ready_reindexes_and_its_end_is_ready_again() {
        let mut m = ClangdAdapter::new();
        feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "begin"));
        feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "end"));
        assert_eq!(m.state.readiness, Readiness::Ready);
        let state = feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "begin"))
            .expect("a begin while ready (the queue non-empty again) is a signal");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state =
            feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "end")).expect("its end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn ignores_other_tokens() {
        let mut m = ClangdAdapter::new();
        assert!(feed(&mut m, &progress("some-other-token", "begin")).is_none());
        assert!(feed(&mut m, &progress("some-other-token", "end")).is_none());
        assert_eq!(m.state.readiness, Readiness::Initializing);
    }

    #[test]
    fn ignores_a_progress_that_happens_to_use_the_token_as_a_request() {
        let mut m = ClangdAdapter::new();
        let as_request = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"$/progress","params":{{"token":"{BACKGROUND_INDEX_TOKEN}","value":{{"kind":"begin"}}}}}}"#
        );
        let view = peek(as_request.as_bytes()).unwrap();
        assert!(m.interpret(&view, as_request.as_bytes()).is_none());
        assert_eq!(m.state.readiness, Readiness::Initializing);
    }

    #[test]
    fn health_stays_unknown_regardless_of_readiness() {
        let mut m = ClangdAdapter::new();
        assert_eq!(
            feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "begin"))
                .unwrap()
                .health,
            Health::Unknown
        );
        assert_eq!(
            feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "end"))
                .unwrap()
                .health,
            Health::Unknown
        );
    }

    #[test]
    fn without_any_signal_readiness_stays_initializing() {
        // No compilation database: the token never arrives at all (decision (a), ADR 0020
        // addendum M24). This mapping does not distinguish that from "not yet begun".
        let m = ClangdAdapter::new();
        assert_eq!(m.state.readiness, Readiness::Initializing);
    }

    #[test]
    fn declares_a_guarantee_only_for_the_tested_version() {
        let tested = ClangdAdapter::for_version(Some(
            "clangd version 21.1.8 linux x86_64-unknown-linux-gnu",
        ));
        assert_eq!(tested.guarantees(), ServerStateProvider::coverage_only(&[]));
        let untested = ClangdAdapter::for_version(Some(
            "clangd version 20.1.0 linux x86_64-unknown-linux-gnu",
        ));
        assert_eq!(
            untested.guarantees(),
            ServerStateProvider::notifications_only()
        );
        let different_platform =
            ClangdAdapter::for_version(Some("clangd version 21.1.8 darwin arm64-apple-darwin"));
        assert_eq!(
            different_platform.guarantees(),
            ServerStateProvider::notifications_only(),
            "a different platform build is a different, untested version"
        );
        let unversioned = ClangdAdapter::new();
        assert_eq!(
            unversioned.guarantees(),
            ServerStateProvider::notifications_only()
        );
    }

    #[test]
    fn a_tested_guarantee_declares_no_freshness() {
        let tested = ClangdAdapter::for_version(Some(
            "clangd version 21.1.8 linux x86_64-unknown-linux-gnu",
        ));
        let json = serde_json::to_string(&tested.guarantees()).unwrap();
        assert!(
            !json.contains("freshness"),
            "clangd must not declare freshness: {json}"
        );
        assert!(
            json.contains("coverage"),
            "clangd must declare coverage: {json}"
        );
    }
}
