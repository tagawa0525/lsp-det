//! The pyright / basedpyright mapping (ADR 0011, design 5.3).
//!
//! pyright has no readiness vocabulary, and does not return
//! `InitializeResult.serverInfo` either (basedpyright does). Both are read
//! from `window/logMessage` (per the pyright source `languageServerBase.ts`,
//! `sourceEnumerator.ts`, `referencesProvider.ts`, and the measurement in
//! research/pyright-readiness-measurement.md):
//!
//! - **what the server calls itself**: right at startup, the constructor
//!   sends an info "Pyright language server 1.1.412 starting" (basedpyright
//!   sends "basedpyright language server 1.39.8 starting"). This is not
//!   suppressed, since it happens before settings are loaded, and arrives
//!   before the `initialize` response. [`startup_identity`]
//! - **readiness**: a cross-workspace request scans the list of tracked
//!   files, and that list is enumerated gradually via a timer. Enumeration
//!   completion is signaled by an info "Found N source files" or "No source
//!   files found." "Starting service instance \"name\"" is emitted per
//!   workspace folder, and enumeration is also per folder, so completions
//!   are counted up to that number before `ready`. A re-enumeration start
//!   "Searching for source files" (log level, not delivered by default)
//!   reverts it to `indexing`
//! - **health**: there is no signal. It stays `unknown` (spec 8.2 item 2). A
//!   crash is conveyed by the connection closing
//! - `$/progress` is the parsing progress of an open file, a separate matter
//!   from the completeness of a cross-workspace request, so it is not read
//!   (ADR 0011 decision B-4)
//!
//! `coverage` / `freshness` are declared only for versions
//! ([`TESTED_VERSIONS`]) for which conformance tests 7.2 / 7.3 were run
//! against a real pyright and passed (ADR 0009 decision D-5).

use serde::Deserialize;

use super::Mapping;
use crate::initialize::ServerInfo;
use crate::peek::MessageView;
use crate::state::{FileChangeType, Readiness, ServerState, ServerStateProvider};

const LOG_MESSAGE_METHOD: &str = "window/logMessage";
/// The fixed phrase between productName and version in the startup log (the constructor in
/// `languageServerBase.ts`).
const STARTUP_INFIX: &str = " language server ";
const STARTUP_SUFFIX: &str = " starting";
/// The start of an `AnalyzerService` per workspace folder (`languageServerBase.ts`).
const SERVICE_STARTED_PREFIX: &str = "Starting service instance ";
/// File enumeration completion (`_finish()` in `sourceEnumerator.ts`).
const ENUMERATION_FOUND_PREFIX: &str = "Found ";
const ENUMERATION_FOUND_SUFFIX_ONE: &str = " source file";
const ENUMERATION_FOUND_SUFFIX_MANY: &str = " source files";
const ENUMERATION_EMPTY: &str = "No source files found.";
/// Re-enumeration start (the constructor in `sourceEnumerator.ts`, log level).
const ENUMERATION_STARTED: &str = "Searching for source files";

/// Versions for which conformance tests 7.2 / 7.3 were run against a real pyright and passed.
/// Matched by exact equality against what the server calls itself (the version in the startup
/// log; pyright does not return `serverInfo`).
///
/// No guarantee is declared for a version not in the list. When adding one, run
/// `cargo test --test conformance -- --ignored pyright_` against that version first (declaring
/// a guarantee that cannot be kept violates spec 5.1).
///
/// Record of versions passed: 1.1.412 (nixpkgs), 2026-09-03, 5 consecutive runs.
pub const TESTED_VERSIONS: &[&str] = &["1.1.412"];

/// The same, for basedpyright. Its version numbers are a separate series from pyright's, so it
/// has its own separate list. It calls itself via `serverInfo.version` (the same version also
/// appears in the startup log).
///
/// Record of versions passed: 1.39.8 (nixpkgs, derived from pyright 1.1.410), 2026-09-03, 5
/// consecutive runs.
pub const BASEDPYRIGHT_TESTED_VERSIONS: &[&str] = &["1.39.8"];

/// Reads the startup log's identity announcement.
///
/// A pyright-family server sends `${productName} language server ${version} starting`.
/// productName is "Pyright" or "basedpyright". The mapping key is normalized to lowercase
/// "pyright" / "basedpyright" (basedpyright calls itself "basedpyright" in `serverInfo.name`;
/// the name comparison is case-insensitive). The version can be omitted, in which case this is
/// `None`. `None` for any other wording.
pub fn startup_identity(message: &str) -> Option<ServerInfo> {
    let (product, rest) = message.split_once(STARTUP_INFIX)?;
    let name = match product {
        "Pyright" | "pyright" => "pyright",
        "basedpyright" => "basedpyright",
        _ => return None,
    };
    // The version comes from `serverOptions.version && serverOptions.version + ' '`, so it can
    // be omitted. In that case rest is just "starting".
    let version = match rest {
        r if r == STARTUP_SUFFIX.trim_start() => None,
        r => {
            let v = r.strip_suffix(STARTUP_SUFFIX)?;
            if v.is_empty() || v.contains(' ') {
                return None;
            }
            Some(v.to_string())
        }
    };
    Some(ServerInfo {
        name: name.to_string(),
        version,
    })
}

/// The pyright / basedpyright mapping.
pub struct PyrightAdapter {
    /// Whether the announced version is in [`TESTED_VERSIONS`]. The condition for declaring a
    /// guarantee.
    version_is_tested: bool,
    state: ServerState,
    /// The count of "Starting service instance" seen (= the number of folders being waited on
    /// for enumeration).
    instances: usize,
    /// The count of enumeration completion logs seen. `ready` once it catches up to
    /// `instances`.
    completed: usize,
}

impl Default for PyrightAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl PyrightAdapter {
    /// For a pyright that does not announce a version. Declares no guarantee.
    pub fn new() -> Self {
        Self::for_identity("pyright", None)
    }

    /// Looks at the announced name and version and declares a guarantee if it is a tested
    /// version for that product.
    pub fn for_identity(name: &str, version: Option<&str>) -> Self {
        let tested: &[&str] = match name {
            "pyright" => TESTED_VERSIONS,
            "basedpyright" => BASEDPYRIGHT_TESTED_VERSIONS,
            _ => &[],
        };
        let version_is_tested = version.is_some_and(|v| tested.contains(&v.trim()));
        PyrightAdapter {
            version_is_tested,
            state: ServerState::initializing(),
            instances: 0,
            completed: 0,
        }
    }

    fn on_log(&mut self, message: &str) -> Option<ServerState> {
        if message.starts_with(SERVICE_STARTED_PREFIX) {
            // Enumeration of a new folder begins. If it was ready, revert to indexing
            // (didChangeWorkspaceFolders). Left as-is while initializing.
            self.instances += 1;
            if self.state.readiness == Readiness::Ready {
                self.state.readiness = Readiness::Indexing;
            }
        } else if message == ENUMERATION_STARTED {
            // Re-enumeration (log level, not delivered at the default logLevel).
            if self.instances == 0 {
                return None;
            }
            self.completed = self.completed.saturating_sub(1);
            self.state.readiness = Readiness::Indexing;
        } else if is_enumeration_complete(message) {
            if self.completed >= self.instances {
                // A completion with nothing to count against. Not grounds for claiming ready.
                return None;
            }
            self.completed += 1;
            if self.completed == self.instances {
                self.state.readiness = Readiness::Ready;
            }
        } else {
            return None;
        }
        Some(self.state.clone())
    }
}

/// "Found N source file(s)" or "No source files found."
fn is_enumeration_complete(message: &str) -> bool {
    if message == ENUMERATION_EMPTY {
        return true;
    }
    let Some(rest) = message.strip_prefix(ENUMERATION_FOUND_PREFIX) else {
        return false;
    };
    let count = rest
        .strip_suffix(ENUMERATION_FOUND_SUFFIX_MANY)
        .or_else(|| rest.strip_suffix(ENUMERATION_FOUND_SUFFIX_ONE));
    count.is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

#[derive(Deserialize)]
struct LogMessageParams {
    message: String,
}

impl Mapping for PyrightAdapter {
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

    /// The serverInfo version is the product version itself (even if the startup log omitted
    /// the version, a guarantee is declared if serverInfo announces a tested version).
    fn learn_identity(&mut self, info: &ServerInfo) {
        let refreshed =
            PyrightAdapter::for_identity(&info.name.to_ascii_lowercase(), info.version.as_deref());
        self.version_is_tested = refreshed.version_is_tested;
    }

    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() || view.method() != Some(LOG_MESSAGE_METHOD) {
            return None;
        }
        #[derive(Deserialize)]
        struct Envelope {
            params: LogMessageParams,
        }
        let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
        self.on_log(&envelope.params.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;
    use crate::state::{Health, Readiness};

    fn log(kind: u8, message: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"window/logMessage","params":{{"type":{kind},"message":"{message}"}}}}"#
        )
    }

    fn info(message: &str) -> String {
        log(3, message)
    }

    fn interpret(adapter: &mut PyrightAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    fn started(adapter: &mut PyrightAdapter, folder: &str) -> Option<ServerState> {
        interpret(
            adapter,
            &info(&format!("Starting service instance \\\"{folder}\\\"")),
        )
    }

    // --- what the server calls itself -------------------------------------------

    #[test]
    fn reads_the_name_and_version_out_of_the_startup_log() {
        // The exact wording measured (research/pyright-readiness-measurement.md).
        let pyright = startup_identity("Pyright language server 1.1.412 starting")
            .expect("pyright's startup log is an identity announcement");
        assert_eq!(pyright.name, "pyright");
        assert_eq!(pyright.version.as_deref(), Some("1.1.412"));

        let based = startup_identity("basedpyright language server 1.39.8 starting")
            .expect("basedpyright's startup log is an identity announcement");
        assert_eq!(based.name, "basedpyright");
        assert_eq!(based.version.as_deref(), Some("1.39.8"));
    }

    #[test]
    fn the_startup_log_may_omit_the_version() {
        // The version can be omitted, since it comes from
        // `serverOptions.version && serverOptions.version + ' '`.
        let identity = startup_identity("Pyright language server starting")
            .expect("still an identity announcement without a version");
        assert_eq!(identity.name, "pyright");
        assert_eq!(identity.version, None);
    }

    #[test]
    fn other_log_lines_are_not_identities() {
        for other in [
            "Server root directory: file:///nix/store/x/dist",
            "Starting service instance \"pyfix\"",
            "Found 2 source files",
            "rust-analyzer 1.98.0 starting",
            "language server starting",
            "",
        ] {
            assert!(
                startup_identity(other).is_none(),
                "not an identity announcement: {other:?}"
            );
        }
    }

    // --- readiness -------------------------------------------------------------

    #[test]
    fn starts_initializing_with_unknown_health() {
        let adapter = PyrightAdapter::new();
        let state = adapter.initial_state();
        assert_eq!(state.readiness, Readiness::Initializing);
        assert_eq!(state.health, Health::Unknown);
    }

    #[test]
    fn enumeration_of_the_only_folder_means_ready() {
        let mut adapter = PyrightAdapter::new();
        started(&mut adapter, "pyfix");
        let state =
            interpret(&mut adapter, &info("Found 2 source files")).expect("completion is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(
            state.health,
            Health::Unknown,
            "enumeration completion is not an observation of health"
        );
    }

    #[test]
    fn no_source_files_is_also_a_completion() {
        let mut adapter = PyrightAdapter::new();
        started(&mut adapter, "empty");
        let state = interpret(&mut adapter, &info("No source files found."))
            .expect("completion is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn waits_for_every_workspace_folder() {
        // "Starting service instance" and a completion log each appear once per folder.
        let mut adapter = PyrightAdapter::new();
        started(&mut adapter, "one");
        started(&mut adapter, "two");
        let after_one = interpret(&mut adapter, &info("Found 400 source files"));
        assert!(
            after_one
                .as_ref()
                .is_none_or(|s| s.readiness != Readiness::Ready),
            "claimed ready on the completion of one folder: {after_one:?}"
        );
        let after_two = interpret(&mut adapter, &info("Found 1200 source files"))
            .expect("the last completion is a signal");
        assert_eq!(after_two.readiness, Readiness::Ready);
    }

    #[test]
    fn a_folder_added_after_ready_rearms() {
        // didChangeWorkspaceFolders starts a new service instance.
        let mut adapter = PyrightAdapter::new();
        started(&mut adapter, "one");
        interpret(&mut adapter, &info("Found 1 source file"));
        let state = started(&mut adapter, "two").expect("starting a new folder is a signal");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state =
            interpret(&mut adapter, &info("Found 3 source files")).expect("completion is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn reenumeration_start_rearms_when_it_is_visible() {
        // "Searching for source files" is log level and not delivered by default, but when it
        // is delivered, it reverts to indexing as the start of re-enumeration.
        let mut adapter = PyrightAdapter::new();
        started(&mut adapter, "one");
        interpret(&mut adapter, &info("Found 1 source file"));
        let state = interpret(&mut adapter, &log(4, "Searching for source files"))
            .expect("the start of re-enumeration is a signal");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state =
            interpret(&mut adapter, &info("Found 2 source files")).expect("completion is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn a_completion_without_a_started_instance_is_ignored() {
        // Does not become ready on a completion log with nothing to count against.
        let mut adapter = PyrightAdapter::new();
        assert!(interpret(&mut adapter, &info("Found 2 source files")).is_none());
    }

    #[test]
    fn ignores_other_logs_progress_and_other_vocabularies() {
        let mut adapter = PyrightAdapter::new();
        started(&mut adapter, "one");
        for other in [
            info("Pyright language server 1.1.412 starting"),
            info("Server root directory: file:///x"),
            info("Assuming Python version 3.14.7.final.0"),
            info("Auto-excluding **/node_modules"),
            log(1, "some error"),
            r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":"t","value":{"kind":"begin","title":"Finding references"}}}"#.to_string(),
            r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":"t","value":{"kind":"end"}}}"#.to_string(),
            r#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"ok","quiescent":true,"message":null}}"#.to_string(),
            r#"{"jsonrpc":"2.0","method":"window/showMessage","params":{"type":3,"message":"Found 2 source files"}}"#.to_string(),
        ] {
            assert!(
                interpret(&mut adapter, &other).is_none(),
                "the state moved on an unrelated message: {other}"
            );
        }
    }

    #[test]
    fn health_never_leaves_unknown() {
        let mut adapter = PyrightAdapter::new();
        started(&mut adapter, "one");
        let state = interpret(&mut adapter, &info("Found 1 source file")).unwrap();
        assert_eq!(state.health, Health::Unknown);
        assert_eq!(state.message, None);
    }

    // --- guarantee ---------------------------------------------------------------

    #[test]
    fn declares_guarantees_only_for_versions_the_conformance_suite_passed_on() {
        // Spec 8.2 item 5. 7.2 / 7.3 were run against a real pyright 1.1.412 and a real
        // basedpyright 1.39.8 and passed (the pyright_* ignored tests in tests/conformance.rs).
        // Since version numbers are a separate series per product, the list is kept per
        // product too.
        assert_eq!(
            PyrightAdapter::for_identity("pyright", Some("1.1.412")).guarantees(),
            ServerStateProvider::workspace(&[], &[FileChangeType::Changed])
        );
        assert_eq!(
            PyrightAdapter::for_identity("basedpyright", Some("1.39.8")).guarantees(),
            ServerStateProvider::workspace(&[], &[FileChangeType::Changed])
        );
        for (name, version) in [
            ("pyright", Some("1.1.400")),
            ("pyright", Some("1.39.8")),
            ("pyright", None),
            ("basedpyright", Some("1.1.412")),
            ("basedpyright", Some("1.39.7")),
            ("basedpyright", None),
            ("other", Some("1.1.412")),
        ] {
            assert_eq!(
                PyrightAdapter::for_identity(name, version).guarantees(),
                ServerStateProvider::notifications_only(),
                "declared a guarantee for unmeasured {name} {version:?}"
            );
        }
    }
}
