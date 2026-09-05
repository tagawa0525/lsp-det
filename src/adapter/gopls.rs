//! The gopls mapping (v0.1-design.md 5.2).
//!
//! gopls has no readiness vocabulary. It is synthesized from `$/progress`
//! (confirmed in the gopls source `server/general.go`, `server/diagnostics.go`,
//! and `progress/progress.go`):
//!
//! - **readiness**: at initialization, a progress with title "Setting up
//!   workspace" begins for each workspace folder, and after
//!   `AwaitInitialized` it ends with "Finished loading packages." (or, on a
//!   load failure, ends with "Error loading packages: ..."). Tokens are
//!   random, so they are identified by title, and once every token
//!   remembered at begin has ended, it is `ready`. The same title also
//!   appears when a folder is added via `didChangeWorkspaceFolders`, so
//!   seeing a begin reverts it to `indexing` (re-arming). It does not appear
//!   for a go.mod change
//! - **health**: `ok` once the first load ends with "Finished loading
//!   packages.", `error` if it ends with "Error loading packages:". After
//!   that, a progress with title "Error loading workspace"
//!   (`WorkspaceLoadFailure`) beginning means `error` (with a message), a
//!   report updates the message, and ending with "Done." reverts it to `ok`
//!
//! `coverage` / `freshness` are declared only for versions
//! ([`TESTED_VERSIONS`]) for which conformance tests 7.2 / 7.3 were run
//! against a real gopls and passed (design 5.2, spec 8.2 item 5).

use serde::Deserialize;
use serde_json::Value;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{ALL_FILE_CHANGES, Health, Readiness, ServerState, ServerStateProvider};

const PROGRESS_METHOD: &str = "$/progress";
/// The initial load (`addFolders` in `general.go`).
const WORKSPACE_SETUP_TITLE: &str = "Setting up workspace";
/// A workspace load failure (`WorkspaceLoadFailure` in `diagnostics.go`).
const WORKSPACE_LOAD_FAILURE_TITLE: &str = "Error loading workspace";
/// The start of the end message on a folder load failure (`general.go`).
const FAILED_LOAD_PREFIX: &str = "Error loading packages";

/// Versions for which conformance tests 7.2 / 7.3 were run against a real gopls and passed.
/// Matched by exact equality against the identity string normalized by [`gopls_version`]
/// (`X.Y.Z` with the `v` stripped).
///
/// No guarantee is declared for a version not in the list. When adding one, run
/// `cargo test --test conformance -- --ignored gopls_` against that version first (declaring a
/// guarantee that cannot be kept violates spec 5.1).
///
/// Record of versions passed: v0.23.0 (nixpkgs, go1.26.7), 2026-09-03, 5 consecutive runs.
pub const TESTED_VERSIONS: &[&str] = &["0.23.0"];

/// Extracts the version (`X.Y.Z`) from gopls's `serverInfo.version`.
///
/// gopls announces itself as a JSON-stringified build info (`debug.BuildInfo`). The version is
/// its top-level `"Version"` (`v0.23.0`). `Main.Version` cannot be used because it is `(devel)`
/// in a nix build. If it is not JSON, the string itself is treated as the version. Leading and
/// trailing whitespace and a leading `v` are dropped, and only the `X.Y.Z` shape is accepted
/// (`(devel)` and the like become `None`).
pub fn gopls_version(version: &str) -> Option<String> {
    let raw = match serde_json::from_str::<Value>(version) {
        Ok(Value::Object(info)) => info.get("Version")?.as_str()?.to_string(),
        _ => version.to_string(),
    };
    let trimmed = raw.trim();
    let stripped = trimmed.strip_prefix('v').unwrap_or(trimmed);
    let mut parts = stripped.split('.');
    let is_semver = parts
        .by_ref()
        .take(3)
        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && stripped.matches('.').count() == 2;
    is_semver.then(|| stripped.to_string())
}

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
    #[serde(default)]
    message: Option<String>,
}

pub struct GoplsAdapter {
    /// Whether the announced version is in [`TESTED_VERSIONS`]. The condition for declaring a
    /// guarantee.
    version_is_tested: bool,
    state: ServerState,
    /// Tokens of "Setting up workspace" that have begun and are awaiting end.
    loading: Vec<Value>,
    /// The end message of a folder that failed during this load (since `loading` last grew from
    /// empty). If there is even one, this round's result cannot be trusted.
    failed_in_round: Option<String>,
    /// The token of an "Error loading workspace" that is in progress (begun).
    failure: Option<Value>,
}

impl Default for GoplsAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl GoplsAdapter {
    /// For a gopls that does not announce a version (or whose version cannot be read).
    /// Declares no guarantee.
    pub fn new() -> Self {
        Self::for_version(None)
    }

    /// Looks at `serverInfo.version` and declares a guarantee if it is a tested version.
    pub fn for_version(version: Option<&str>) -> Self {
        let version_is_tested = version
            .and_then(gopls_version)
            .is_some_and(|v| TESTED_VERSIONS.contains(&v.as_str()));
        GoplsAdapter {
            version_is_tested,
            state: ServerState::initializing(),
            loading: Vec::new(),
            failed_in_round: None,
            failure: None,
        }
    }

    fn on_progress(&mut self, params: ProgressParams) -> Option<ServerState> {
        let ProgressParams { token, value } = params;
        match value.kind.as_str() {
            "begin" => match value.title.as_deref() {
                Some(WORKSPACE_SETUP_TITLE) => {
                    if self.loading.is_empty() {
                        // A new load round. The previous failure is not carried over.
                        self.failed_in_round = None;
                    }
                    self.loading.push(token);
                    self.state.readiness = Readiness::Indexing;
                }
                Some(WORKSPACE_LOAD_FAILURE_TITLE) => {
                    self.failure = Some(token);
                    self.state.health = Health::Error;
                    self.state.message = value.message;
                }
                _ => return None,
            },
            "report" => {
                if self.failure.as_ref() != Some(&token) {
                    return None;
                }
                if value.message.is_some() {
                    self.state.message = value.message;
                }
            }
            "end" if self.failure.as_ref() == Some(&token) => {
                self.failure = None;
                self.state.health = Health::Ok;
                self.state.message = None;
            }
            "end" => {
                let index = self.loading.iter().position(|t| *t == token)?;
                self.loading.remove(index);
                let failed = value
                    .message
                    .as_deref()
                    .is_some_and(|m| m.starts_with(FAILED_LOAD_PREFIX));
                if failed {
                    // The attempt has ended but the result cannot be trusted (spec chapter 6
                    // item 5).
                    self.failed_in_round = value.message;
                }
                if self.loading.is_empty() {
                    // ready only once every folder's load has ended. health is also decided
                    // only here (claiming ok in the middle would be an unobserved claim). If
                    // even one failed this round, error; if all succeeded, ok based on the
                    // observed success (the previous failure is not carried over). But it stays
                    // error while "Error loading workspace" is in progress (begun).
                    self.state.readiness = Readiness::Ready;
                    if let Some(message) = &self.failed_in_round {
                        self.state.health = Health::Error;
                        self.state.message = Some(message.clone());
                    } else if self.failure.is_none() {
                        self.state.health = Health::Ok;
                        self.state.message = None;
                    }
                }
            }
            _ => return None,
        }
        Some(self.state.clone())
    }
}

impl Mapping for GoplsAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// gopls takes a snapshot per request and incorporates `didChange` overlays. This has been
    /// confirmed by running conformance tests 7.2 (completeness) and 7.3 (cross-file freshness)
    /// against a real gopls (the gopls_* ignored tests in tests/conformance.rs). It can be
    /// declared only for versions the tests have passed on.
    fn guarantees(&self) -> ServerStateProvider {
        if self.version_is_tested {
            ServerStateProvider::workspace(&[("workspace/symbol", 100)], &ALL_FILE_CHANGES)
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
        self.on_progress(envelope.params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;

    fn status(health: &str, quiescent: bool) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{{"health":"{health}","quiescent":{quiescent},"message":null}}}}"#
        )
    }

    // --- gopls ---------------------------------------------------------------

    fn progress(token: &str, value: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"$/progress","params":{{"token":"{token}","value":{value}}}}}"#
        )
    }

    fn setup_begin(token: &str) -> String {
        progress(
            token,
            r#"{"kind":"begin","title":"Setting up workspace","message":"Loading packages...","cancellable":false}"#,
        )
    }

    fn setup_end(token: &str, message: &str) -> String {
        progress(token, &format!(r#"{{"kind":"end","message":"{message}"}}"#))
    }

    fn gopls_interpret(adapter: &mut GoplsAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    #[test]
    fn gopls_begin_of_workspace_setup_means_indexing() {
        let mut adapter = GoplsAdapter::new();
        let state = gopls_interpret(&mut adapter, &setup_begin("1")).expect("begin is a signal");
        assert_eq!(state.readiness, Readiness::Indexing);
        assert_eq!(
            state.health,
            Health::Unknown,
            "begin does not speak to health"
        );
    }

    #[test]
    fn gopls_end_of_workspace_setup_means_ready_and_ok() {
        let mut adapter = GoplsAdapter::new();
        gopls_interpret(&mut adapter, &setup_begin("1"));
        let state = gopls_interpret(&mut adapter, &setup_end("1", "Finished loading packages."))
            .expect("end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(state.health, Health::Ok);
    }

    #[test]
    fn gopls_waits_for_all_tokens() {
        let mut adapter = GoplsAdapter::new();
        gopls_interpret(&mut adapter, &setup_begin("a"));
        gopls_interpret(&mut adapter, &setup_begin("b"));
        let after_a = gopls_interpret(&mut adapter, &setup_end("a", "Finished loading packages."));
        assert!(
            after_a
                .as_ref()
                .is_none_or(|s| s.readiness == Readiness::Indexing),
            "claimed ready on the first end: {after_a:?}"
        );
        let after_b = gopls_interpret(&mut adapter, &setup_end("b", "Finished loading packages."))
            .expect("last end is a signal");
        assert_eq!(after_b.readiness, Readiness::Ready);
    }

    #[test]
    fn gopls_successful_reload_after_a_failed_load_restores_ok() {
        // When a reload triggered by, say, adding a folder succeeds, health reverts to ok based
        // on the observed success (per Copilot's feedback).
        let mut adapter = GoplsAdapter::new();
        gopls_interpret(&mut adapter, &setup_begin("1"));
        let failed = gopls_interpret(&mut adapter, &setup_end("1", "Error loading packages: x"))
            .expect("end is a signal");
        assert_eq!(failed.health, Health::Error);

        gopls_interpret(&mut adapter, &setup_begin("2"));
        let state = gopls_interpret(&mut adapter, &setup_end("2", "Finished loading packages."))
            .expect("end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(
            state.health,
            Health::Ok,
            "reverts to ok on a successful reload"
        );
        assert_eq!(state.message, None);
    }

    #[test]
    fn gopls_a_round_with_one_failed_folder_stays_error() {
        // If even one folder fails within the same load, it stays error even when a later
        // folder succeeds (the result cannot be trusted).
        let mut adapter = GoplsAdapter::new();
        gopls_interpret(&mut adapter, &setup_begin("a"));
        gopls_interpret(&mut adapter, &setup_begin("b"));
        gopls_interpret(&mut adapter, &setup_end("a", "Error loading packages: x"));
        let state = gopls_interpret(&mut adapter, &setup_end("b", "Finished loading packages."))
            .expect("last end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(state.health, Health::Error);
        assert_eq!(state.message.as_deref(), Some("Error loading packages: x"));
    }

    #[test]
    fn gopls_end_of_an_unknown_token_is_ignored() {
        // Only tokens remembered at begin count. It does not become ready on the end of some
        // other progress.
        let mut adapter = GoplsAdapter::new();
        assert!(
            gopls_interpret(
                &mut adapter,
                &setup_end("stray", "Finished loading packages.")
            )
            .is_none()
        );
    }

    #[test]
    fn gopls_failed_load_is_ready_but_error() {
        let mut adapter = GoplsAdapter::new();
        gopls_interpret(&mut adapter, &setup_begin("1"));
        let state = gopls_interpret(
            &mut adapter,
            &setup_end("1", "Error loading packages: no Go files"),
        )
        .expect("end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(state.health, Health::Error);
        assert_eq!(
            state.message.as_deref(),
            Some("Error loading packages: no Go files")
        );
    }

    #[test]
    fn gopls_workspace_load_failure_progress_drives_health() {
        let mut adapter = GoplsAdapter::new();
        gopls_interpret(&mut adapter, &setup_begin("1"));
        gopls_interpret(&mut adapter, &setup_end("1", "Finished loading packages."));

        let begin = progress(
            "e",
            r#"{"kind":"begin","title":"Error loading workspace","message":"err: go.mod file not found","cancellable":false}"#,
        );
        let state = gopls_interpret(&mut adapter, &begin).expect("failure begin is a signal");
        assert_eq!(state.health, Health::Error);
        assert_eq!(state.message.as_deref(), Some("err: go.mod file not found"));
        assert_eq!(
            state.readiness,
            Readiness::Ready,
            "does not change readiness"
        );

        let report = progress("e", r#"{"kind":"report","message":"err: still broken"}"#);
        let state = gopls_interpret(&mut adapter, &report).expect("report updates the message");
        assert_eq!(state.health, Health::Error);
        assert_eq!(state.message.as_deref(), Some("err: still broken"));

        let end = progress("e", r#"{"kind":"end","message":"Done."}"#);
        let state = gopls_interpret(&mut adapter, &end).expect("failure end is a signal");
        assert_eq!(state.health, Health::Ok);
    }

    #[test]
    fn gopls_ignores_other_progress_titles_and_other_methods() {
        let mut adapter = GoplsAdapter::new();
        let diag = progress(
            "d",
            r#"{"kind":"begin","title":"Calculating diagnostics","message":"..."}"#,
        );
        assert!(gopls_interpret(&mut adapter, &diag).is_none());
        assert!(
            gopls_interpret(
                &mut adapter,
                &progress("d", r#"{"kind":"end","message":"Done."}"#)
            )
            .is_none()
        );
        assert!(
            gopls_interpret(&mut adapter, &status("ok", true)).is_none(),
            "does not read rust-analyzer's vocabulary"
        );
    }

    /// The string the nix-built gopls v0.23.0 announced (measured 2026-09-03).
    const GOPLS_VERSION_JSON: &str = r#"{"GoVersion":"go1.27.0","Path":"golang.org/x/tools/gopls","Main":{"Path":"golang.org/x/tools/gopls","Version":"(devel)"},"Deps":[{"Path":"golang.org/x/tools","Version":"v0.47.1-0.20260707181000-a299dadba899"}],"Settings":[{"Key":"GOOS","Value":"linux"}],"Version":"v0.23.0"}"#;

    #[test]
    fn gopls_reads_the_version_out_of_the_build_info_json() {
        // serverInfo.version is JSON-stringified build info. The top-level "Version" is the
        // version; Main.Version is "(devel)" in a nix build.
        assert_eq!(gopls_version(GOPLS_VERSION_JSON).as_deref(), Some("0.23.0"));
        assert_eq!(gopls_version("v0.23.0").as_deref(), Some("0.23.0"));
        assert_eq!(gopls_version("0.23.0").as_deref(), Some("0.23.0"));
        assert_eq!(gopls_version(" v0.23.0 ").as_deref(), Some("0.23.0"));
        assert_eq!(gopls_version("(devel)"), None, "accepts nothing but X.Y.Z");
        assert_eq!(gopls_version("0.23"), None);
        assert_eq!(gopls_version(""), None);
    }

    #[test]
    fn gopls_declares_guarantees_only_for_versions_the_conformance_suite_passed_on() {
        // Spec 8.2 item 5. 7.2 / 7.3 were run against a real gopls v0.23.0 and passed (the
        // gopls_* ignored tests in tests/conformance.rs). No guarantee is declared otherwise.
        assert_eq!(
            GoplsAdapter::for_version(Some(GOPLS_VERSION_JSON)).guarantees(),
            ServerStateProvider::workspace(&[("workspace/symbol", 100)], &ALL_FILE_CHANGES)
        );
        for untested in [
            Some("v0.22.0"),
            Some("(devel)"),
            Some("1.98.0 (fake)"),
            None,
        ] {
            assert_eq!(
                GoplsAdapter::for_version(untested).guarantees(),
                ServerStateProvider::notifications_only(),
                "declared a guarantee for untested version {untested:?}"
            );
        }
    }
}
