//! The typescript-language-server mapping (M6 of ADR 0010 decision B, design 5.3).
//!
//! typescript-language-server has no readiness vocabulary, and does not
//! return `InitializeResult.serverInfo` either. The signals are as follows
//! (per the source `src/ts-client.ts`, `src/lsp-server.ts`, and the
//! measurement in
//! research/typescript-language-server-readiness-measurement.md):
//!
//! - **what the server calls itself**: right after the `initialize`
//!   response, it sends its own `$/typescriptVersion` notification
//!   `{version, source}`. This notification is specific to
//!   typescript-language-server, so it is used to select the mapping
//!   ([`identity_from_typescript_version`]). The version is tsserver's
//!   (TypeScript's) version; typescript-language-server's own version never
//!   appears on the wire
//! - **readiness**: on tsserver's `projectLoadingStart`, a `$/progress` with
//!   title "Initializing JS/TS language features…" begins, and it ends on
//!   `projectLoadingFinish` etc. Projects are loaded one at a time as files
//!   are opened, and a new begin starts only after ending the previous
//!   progress. Once every token remembered at begin has ended, it is
//!   `ready`. It is also reissued on a tsconfig change
//! - **health**: `ok` on the first end (a successful load was observed). If
//!   tsserver crashes, "[tsserver] Exited. Code: N. Signal: S" appears in a
//!   `window/logMessage` (error). The language server itself survives and
//!   returns an empty array as success, so this log is what drives `error`.
//!   There is no restart, so it does not revert
//!
//! `coverage` / `freshness` are declared only for versions
//! ([`TESTED_VERSIONS`]) for which conformance tests 7.2 / 7.3 were run
//! against a real server and passed (ADR 0009 decision D-5).

use serde::Deserialize;
use serde_json::Value;

use super::Mapping;
use crate::initialize::ServerInfo;
use crate::peek::MessageView;
use crate::state::{FileChangeType, Health, Readiness, ServerState, ServerStateProvider};

const PROGRESS_METHOD: &str = "$/progress";
const LOG_MESSAGE_METHOD: &str = "window/logMessage";
/// The notification specific to typescript-language-server. Arrives after the `initialize`
/// response.
const TYPESCRIPT_VERSION_METHOD: &str = "$/typescriptVersion";
/// Project loading (`ServerInitializingIndicator` in `ts-client.ts`).
const PROJECT_LOAD_TITLE: &str = "Initializing JS/TS language features…";
/// tsserver exiting (`onExit` in `ts-client.ts`). Prefixed with the "[lspserver] [tsclient] "
/// tag.
const TSSERVER_EXITED: &str = "[tsserver] Exited. Code:";
/// The fixed phrase of the startup log (`initialize` in `lsp-server.ts`).
const STARTUP_PREFIX: &str = "Using Typescript version (";
const STARTUP_INFIX: &str = ") ";
const STARTUP_SUFFIX_START: &str = " from path ";

/// The name used as an identity announcement in place of `serverInfo`.
pub const SERVER_NAME: &str = "typescript-language-server";

/// The **TypeScript (tsserver) version** for which conformance tests 7.2 / 7.3 have passed.
///
/// typescript-language-server's own version never appears on the wire, so this is the only
/// thing that can be matched against what the server calls itself (the `version` in
/// `$/typescriptVersion`). No guarantee is declared for a version not in the list. When adding
/// one, run `cargo test --test conformance -- --ignored typescript_language_server_` against
/// that version first (declaring a guarantee that cannot be kept violates spec 5.1).
///
/// Record of versions passed: TypeScript 5.9.3 (typescript-language-server 5.3.0, node 24.19.0,
/// all from nixpkgs), 2026-09-03, 5 consecutive runs.
pub const TESTED_VERSIONS: &[&str] = &["5.9.3"];

/// Reads the identity announcement from the `window/logMessage` (info) that arrives before the
/// `initialize` response.
///
/// `initialize` in `lsp-server.ts` emits an info
/// `Using Typescript version (${source}) ${version} from path "${path}"` before returning its
/// response. `$/typescriptVersion` arrives after the response, so declaring a guarantee in
/// `InitializeResult` requires selecting the mapping from this one first. The wording is
/// specific to typescript-language-server. `None` for any other wording.
pub fn startup_identity(message: &str) -> Option<ServerInfo> {
    let rest = message.strip_prefix(STARTUP_PREFIX)?;
    let (_source, rest) = rest.split_once(STARTUP_INFIX)?;
    let (version, _path) = rest.split_once(STARTUP_SUFFIX_START)?;
    let version = version.trim();
    if version.is_empty() || version.contains(' ') {
        return None;
    }
    Some(ServerInfo {
        name: SERVER_NAME.to_string(),
        version: Some(version.to_string()),
    })
}

/// Reads the identity announcement from the params of `$/typescriptVersion`.
pub fn identity_from_typescript_version(params: &Value) -> Option<ServerInfo> {
    let params = params.as_object()?;
    let version = params
        .get("version")
        .and_then(Value::as_str)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    Some(ServerInfo {
        name: SERVER_NAME.to_string(),
        version,
    })
}

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

#[derive(Deserialize)]
struct LogMessageParams {
    message: String,
}

/// The typescript-language-server mapping.
pub struct TypescriptLanguageServerAdapter {
    /// Whether the announced version is in [`TESTED_VERSIONS`]. The condition for declaring a
    /// guarantee.
    version_is_tested: bool,
    state: ServerState,
    /// Tokens of a project load that have begun and are awaiting end.
    loading: Vec<Value>,
}

impl Default for TypescriptLanguageServerAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl TypescriptLanguageServerAdapter {
    /// For an upstream that does not announce a version. Declares no guarantee.
    pub fn new() -> Self {
        Self::for_version(None)
    }

    /// Looks at the announced version (TypeScript's version) and declares a guarantee if it is
    /// a tested one.
    pub fn for_version(version: Option<&str>) -> Self {
        let version_is_tested = version.is_some_and(|v| TESTED_VERSIONS.contains(&v.trim()));
        TypescriptLanguageServerAdapter {
            version_is_tested,
            state: ServerState::initializing(),
            loading: Vec::new(),
        }
    }

    fn on_progress(&mut self, params: ProgressParams) -> Option<ServerState> {
        let ProgressParams { token, value } = params;
        match value.kind.as_str() {
            "begin" if value.title.as_deref() == Some(PROJECT_LOAD_TITLE) => {
                self.loading.push(token);
                self.state.readiness = Readiness::Indexing;
            }
            "end" => {
                let index = self.loading.iter().position(|t| *t == token)?;
                self.loading.remove(index);
                if !self.loading.is_empty() {
                    return None;
                }
                self.state.readiness = Readiness::Ready;
                // A successful load was observed. But an end after tsserver has crashed (the
                // indicator resetting) is not a success. There is no restart, so it is not
                // reverted.
                if self.state.health != Health::Error {
                    self.state.health = Health::Ok;
                }
            }
            _ => return None,
        }
        Some(self.state.clone())
    }

    fn on_log(&mut self, message: &str) -> Option<ServerState> {
        if !message.contains(TSSERVER_EXITED) {
            return None;
        }
        self.state.health = Health::Error;
        self.state.message = Some(message.to_string());
        Some(self.state.clone())
    }
}

impl Mapping for TypescriptLanguageServerAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    fn guarantees(&self) -> ServerStateProvider {
        if self.version_is_tested {
            ServerStateProvider::workspace(&[], &[FileChangeType::Changed])
        } else {
            ServerStateProvider::notifications_only()
        }
    }

    /// The serverInfo version is the wrapper's (typescript-language-server's) version, not the
    /// TypeScript version the guarantee depends on. The basis is taken from the startup log and
    /// `$/typescriptVersion`, so this does nothing.
    fn learn_identity(&mut self, info: &ServerInfo) {
        let _ = info;
    }

    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() {
            return None;
        }
        match view.method() {
            Some(TYPESCRIPT_VERSION_METHOD) => {
                // The analysis engine's version. The basis for when the startup log did not
                // appear (due to settings). Updated only when a version is present (the basis
                // is not discarded by a notification with no version).
                #[derive(Deserialize)]
                struct Envelope {
                    params: Value,
                }
                if let Ok(envelope) = serde_json::from_slice::<Envelope>(body)
                    && let Some(identity) = identity_from_typescript_version(&envelope.params)
                    && let Some(version) = identity.version.as_deref()
                {
                    self.version_is_tested = Self::for_version(Some(version)).version_is_tested;
                }
                None
            }
            Some(PROGRESS_METHOD) => {
                #[derive(Deserialize)]
                struct Envelope {
                    params: ProgressParams,
                }
                let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
                self.on_progress(envelope.params)
            }
            Some(LOG_MESSAGE_METHOD) => {
                #[derive(Deserialize)]
                struct Envelope {
                    params: LogMessageParams,
                }
                let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
                self.on_log(&envelope.params.message)
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
    use serde_json::json;

    fn progress(token: &str, value: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"$/progress","params":{{"token":"{token}","value":{value}}}}}"#
        )
    }

    fn load_begin(token: &str) -> String {
        progress(
            token,
            r#"{"kind":"begin","title":"Initializing JS/TS language features…"}"#,
        )
    }

    fn load_end(token: &str) -> String {
        progress(token, r#"{"kind":"end"}"#)
    }

    fn log(kind: u8, message: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"window/logMessage","params":{{"type":{kind},"message":"{message}"}}}}"#
        )
    }

    /// The measured wording (SIGKILL).
    const EXITED: &str = "[lspserver] [tsclient] [tsserver] Exited. Code: null. Signal: SIGKILL";

    fn interpret(adapter: &mut TypescriptLanguageServerAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    // --- what the server calls itself -------------------------------------------

    #[test]
    fn reads_the_startup_log_as_the_identity_before_initialize_completes() {
        // The exact wording measured. The only identity announcement that arrives before the
        // initialize response.
        let identity = startup_identity(
            r#"Using Typescript version (user-setting) 5.9.3 from path "/nix/store/x/lib/tsserver.js""#,
        )
        .expect("a startup log is an identity announcement");
        assert_eq!(identity.name, SERVER_NAME);
        assert_eq!(identity.version.as_deref(), Some("5.9.3"));

        let bundled = startup_identity(r#"Using Typescript version (bundled) 5.9.3 from path "x""#)
            .expect("still an identity announcement even with a different source");
        assert_eq!(bundled.version.as_deref(), Some("5.9.3"));
    }

    #[test]
    fn other_log_lines_are_not_identities() {
        for other in [
            "Pyright language server 1.1.412 starting",
            "Using (workspace) TypeScript 5.9.3 instead - x",
            "[lspserver] Killing TS Server",
            "Using Typescript version",
            "",
        ] {
            assert!(
                startup_identity(other).is_none(),
                "not an identity announcement: {other:?}"
            );
        }
    }

    #[test]
    fn reads_the_typescript_version_as_the_identity() {
        // The exact params measured. The name is the mapping key; the version is tsserver's.
        let identity = identity_from_typescript_version(
            &json!({"version": "5.9.3", "source": "user-setting"}),
        )
        .expect("$/typescriptVersion is an identity announcement");
        assert_eq!(identity.name, SERVER_NAME);
        assert_eq!(identity.version.as_deref(), Some("5.9.3"));
    }

    #[test]
    fn a_typescript_version_without_a_version_string_still_names_the_server() {
        let identity = identity_from_typescript_version(&json!({"source": "bundled"}))
            .expect("still an identity announcement without a version");
        assert_eq!(identity.name, SERVER_NAME);
        assert_eq!(identity.version, None);
        assert!(
            identity_from_typescript_version(&json!("5.9.3")).is_none(),
            "does not read anything but an object"
        );
    }

    // --- readiness -------------------------------------------------------------

    #[test]
    fn starts_initializing_with_unknown_health() {
        let state = TypescriptLanguageServerAdapter::new().initial_state();
        assert_eq!(state.readiness, Readiness::Initializing);
        assert_eq!(state.health, Health::Unknown);
    }

    #[test]
    fn begin_of_a_project_load_means_indexing() {
        let mut adapter = TypescriptLanguageServerAdapter::new();
        let state = interpret(&mut adapter, &load_begin("1")).expect("begin is a signal");
        assert_eq!(state.readiness, Readiness::Indexing);
        assert_eq!(
            state.health,
            Health::Unknown,
            "begin does not speak to health"
        );
    }

    #[test]
    fn end_of_the_project_load_means_ready_and_ok() {
        let mut adapter = TypescriptLanguageServerAdapter::new();
        interpret(&mut adapter, &load_begin("1"));
        let state = interpret(&mut adapter, &load_end("1")).expect("end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(state.health, Health::Ok, "a successful load was observed");
    }

    #[test]
    fn sequential_project_loads_rearm() {
        // Opening a second project: end of the first -> begin of the second -> end.
        let mut adapter = TypescriptLanguageServerAdapter::new();
        interpret(&mut adapter, &load_begin("a"));
        interpret(&mut adapter, &load_end("a"));
        let state =
            interpret(&mut adapter, &load_begin("b")).expect("the reload's begin is a signal");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state = interpret(&mut adapter, &load_end("b")).expect("end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn waits_for_every_open_token() {
        // Since loading is sequential, two should not open at once, but if they do, all are
        // waited for.
        let mut adapter = TypescriptLanguageServerAdapter::new();
        interpret(&mut adapter, &load_begin("a"));
        interpret(&mut adapter, &load_begin("b"));
        let after_a = interpret(&mut adapter, &load_end("a"));
        assert!(
            after_a
                .as_ref()
                .is_none_or(|s| s.readiness != Readiness::Ready),
            "claimed ready on the first end: {after_a:?}"
        );
        assert_eq!(
            interpret(&mut adapter, &load_end("b"))
                .expect("the last end is a signal")
                .readiness,
            Readiness::Ready
        );
    }

    #[test]
    fn end_of_an_unknown_token_is_ignored() {
        let mut adapter = TypescriptLanguageServerAdapter::new();
        assert!(interpret(&mut adapter, &load_end("stray")).is_none());
    }

    // --- health ----------------------------------------------------------------

    #[test]
    fn a_tsserver_exit_log_means_error_with_the_log_as_message() {
        let mut adapter = TypescriptLanguageServerAdapter::new();
        interpret(&mut adapter, &load_begin("1"));
        interpret(&mut adapter, &load_end("1"));
        let state = interpret(&mut adapter, &log(1, EXITED)).expect("a crash is a signal");
        assert_eq!(state.health, Health::Error);
        assert_eq!(state.message.as_deref(), Some(EXITED));
        assert_eq!(
            state.readiness,
            Readiness::Ready,
            "does not change readiness"
        );
    }

    #[test]
    fn a_later_end_does_not_restore_ok_after_a_crash() {
        // tsserver is not restarted. Even when the indicator resets (ends) on a crash, it is
        // not reverted to ok.
        let mut adapter = TypescriptLanguageServerAdapter::new();
        interpret(&mut adapter, &load_begin("1"));
        interpret(&mut adapter, &log(1, EXITED));
        let state = interpret(&mut adapter, &load_end("1"));
        assert!(
            state.as_ref().is_none_or(|s| s.health == Health::Error),
            "reverted to ok on the end after a crash: {state:?}"
        );
    }

    #[test]
    fn ignores_other_logs_progress_and_other_vocabularies() {
        let mut adapter = TypescriptLanguageServerAdapter::new();
        for other in [
            log(3, "Using Typescript version (user-setting) 5.9.3 from path x"),
            log(3, "[lspserver] Killing TS Server"),
            log(1, "some other error"),
            progress("d", r#"{"kind":"begin","title":"Finding references"}"#),
            progress("d", r#"{"kind":"end"}"#),
            r#"{"jsonrpc":"2.0","method":"$/typescriptVersion","params":{"version":"5.9.3","source":"user-setting"}}"#.to_string(),
            r#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"ok","quiescent":true,"message":null}}"#.to_string(),
        ] {
            assert!(
                interpret(&mut adapter, &other).is_none(),
                "the state moved on an unrelated message: {other}"
            );
        }
    }

    // --- guarantee ---------------------------------------------------------------

    #[test]
    fn a_typescript_version_notification_without_a_version_keeps_the_guarantee_basis() {
        // After a tested version is settled by the startup log, the basis is not discarded even
        // if $/typescriptVersion arrives lacking a version (per Copilot's feedback). If a
        // version is present, it updates the basis.
        let mut adapter = TypescriptLanguageServerAdapter::for_version(Some("5.9.3"));
        assert_eq!(
            adapter.guarantees(),
            ServerStateProvider::workspace(&[], &[FileChangeType::Changed])
        );
        interpret(
            &mut adapter,
            r#"{"jsonrpc":"2.0","method":"$/typescriptVersion","params":{"source":"bundled"}}"#,
        );
        assert_eq!(
            adapter.guarantees(),
            ServerStateProvider::workspace(&[], &[FileChangeType::Changed]),
            "discarded the basis on a notification with no version"
        );
        interpret(
            &mut adapter,
            r#"{"jsonrpc":"2.0","method":"$/typescriptVersion","params":{"version":"5.9.2","source":"bundled"}}"#,
        );
        assert_eq!(
            adapter.guarantees(),
            ServerStateProvider::notifications_only(),
            "a notification with a version updates the basis"
        );
    }

    #[test]
    fn declares_guarantees_only_for_typescript_versions_the_conformance_suite_passed_on() {
        // 7.2 / 7.3 were run against typescript-language-server 5.3.0 + TypeScript 5.9.3 and
        // passed. Only TypeScript's version appears in the identity announcement.
        assert_eq!(
            TypescriptLanguageServerAdapter::for_version(Some("5.9.3")).guarantees(),
            ServerStateProvider::workspace(&[], &[FileChangeType::Changed])
        );
        for version in [Some("5.9.2"), Some("5.3.0"), Some("garbage"), None] {
            assert_eq!(
                TypescriptLanguageServerAdapter::for_version(version).guarantees(),
                ServerStateProvider::notifications_only(),
                "declared a guarantee for unmeasured version {version:?}"
            );
        }
    }
}
