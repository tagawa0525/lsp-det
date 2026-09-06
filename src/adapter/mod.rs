//! Mappings (adapters, v0.1-design.md chapter 5). They map a language
//! server's own vocabulary onto the server state protocol.
//!
//! A mapping's only role is **interpreting upstream messages**. State
//! retention and duplicate suppression belong to [`crate::tracker::Tracker`].
//! They are kept separate because the upstream side exists even without a
//! mapping, reporting `unknown` on both axes (spec 8.2 item 3).
//!
//! A mapping is chosen by the name the upstream calls itself in
//! `InitializeResult.serverInfo.name` ([`select`]), or, for a server that returns no
//! `serverInfo`, by its startup notification ([`identity_from_notification`]) or by what its
//! `InitializeResult` declares ([`identity_from_initialize_result`]). The mapping is also
//! responsible for compensating for the coarseness of a language server's
//! vocabulary, so the downstream side only ever sees spec values (ADR 0009
//! decision D-6).

pub mod crystalline;
pub mod dart;
pub mod expert;
pub mod gleam;
pub mod gopls;
pub mod haskell_language_server;
pub mod haxe_language_server;
pub mod jdtls;
pub mod metals;
pub mod nextflow;
pub mod pyright;
pub mod rust_analyzer;
pub mod typescript_language_server;

pub use gopls::{GoplsAdapter, TESTED_VERSIONS as GOPLS_TESTED_VERSIONS};
pub use pyright::{
    BASEDPYRIGHT_TESTED_VERSIONS, PyrightAdapter, TESTED_VERSIONS as PYRIGHT_TESTED_VERSIONS,
};
pub use rust_analyzer::{
    RustAnalyzerAdapter, SERVER_STATUS_METHOD, TESTED_VERSIONS as RUST_ANALYZER_TESTED_VERSIONS,
};
pub use typescript_language_server::{
    TESTED_VERSIONS as TYPESCRIPT_LANGUAGE_SERVER_TESTED_VERSIONS, TypescriptLanguageServerAdapter,
};

use crate::initialize::ServerInfo;
use crate::peek::MessageView;
#[cfg(test)]
use crate::state::ALL_FILE_CHANGES;
use crate::state::{ServerState, ServerStateProvider};

/// A mapping that reads `ServerState` from a language server's own vocabulary.
pub trait Mapping {
    /// The state right after connecting to the upstream (the moment the mapping is chosen).
    fn initial_state(&self) -> ServerState;
    /// The guarantee to declare in `InitializeResult` (spec chapter 5). Declare it only for
    /// versions the conformance tests have passed on (spec 8.2 item 5).
    fn guarantees(&self) -> ServerStateProvider;
    /// Reads the state the upstream is reporting from an upstream-to-client message.
    /// `None` if there is nothing to read (the state does not move).
    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState>;
    /// After a mapping is chosen, another identity announcement from the same upstream
    /// (`InitializeResult.serverInfo`) arrived. The mapping decides how to update the version
    /// its guarantee is based on. For pyright, the serverInfo version is the product version
    /// itself, but for typescript-language-server, the serverInfo version is the wrapper's
    /// version, not the version of the analysis engine (TypeScript) the guarantee depends on.
    /// Does nothing by default.
    fn learn_identity(&mut self, info: &ServerInfo) {
        let _ = info;
    }
    /// The client's `initialize` `initializationOptions`. Read by servers (rust-analyzer) whose
    /// declared cap (`coverage.incomplete`) changes with settings. Does nothing by default.
    fn learn_initialization_options(&mut self, options: &serde_json::Value) {
        let _ = options;
    }
    /// The `workspaceFolders` of the client's `initialize`. Read by a mapping (Nextflow) that
    /// reconstructs the set of files the server scans. Does nothing by default.
    fn learn_workspace_folders(&mut self, folders: &[std::path::PathBuf]) {
        let _ = folders;
    }
    /// Observes a client-to-upstream message. Predicting the start of reindexing from a
    /// notification is allowed only for a mapping that has measured that a completion signal is
    /// always sent (ADR 0014 addendum decision D). Reads nothing by default.
    fn observe_client(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        let _ = (view, body);
        None
    }
}

/// The union of client capabilities every known mapping needs (design 4.2).
///
/// A mapping is chosen by `InitializeResult.serverInfo.name`, but the injection has to happen
/// before `initialize` is sent to the upstream. So all of them are injected regardless of who
/// the upstream turns out to be (ADR 0009 decision D-3). Both are merely a permission to send a
/// notification.
///
/// - `experimental.serverStatusNotification`: rust-analyzer. Without declaring it,
///   `experimental/serverStatus` is never sent at all
/// - `window.workDoneProgress`: gopls (M4). Without declaring it, it falls back to
///   `window/showMessage` instead of `$/progress`
pub const CLIENT_CAPABILITIES_FOR_ALL_MAPPINGS: &[&str] = &[
    "experimental.serverStatusNotification",
    "window.workDoneProgress",
];

/// The mapping corresponding to the name the upstream calls itself. `None` if it is not known
/// (the upstream side reports `unknown` on both axes. spec 8.2 item 3).
///
/// The name comparison is case-insensitive. `serverInfo.name` is a free-form display string
/// and LSP defines no comparison rule; the same server can appear as both "Pyright"
/// (productName) and "pyright" (the startup log key).
pub fn select(server_name: &str, version: Option<&str>) -> Option<Box<dyn Mapping>> {
    let key = server_name.to_ascii_lowercase();
    match key.as_str() {
        "rust-analyzer" => Some(Box::new(RustAnalyzerAdapter::for_version(version))),
        "gopls" => Some(Box::new(GoplsAdapter::for_version(version))),
        "metals" => Some(Box::new(metals::MetalsAdapter::for_version(version))),
        dart::SERVER_NAME => Some(Box::new(dart::DartAdapter::for_version(version))),
        jdtls::SERVER_NAME => Some(Box::new(jdtls::JdtlsAdapter::for_version(version))),
        // The version is not looked at: Expert declares no guarantee for any version.
        "expert" => Some(Box::new(expert::ExpertAdapter::new())),
        // The version is not observable: Nextflow's language server declares no guarantee.
        nextflow::SERVER_NAME => Some(Box::new(nextflow::NextflowAdapter::new())),
        // Readiness is not observable at all, the version neither: no guarantee.
        haskell_language_server::SERVER_NAME => Some(Box::new(
            haskell_language_server::HaskellLanguageServerAdapter::new(),
        )),
        // The version is not observable: crystalline declares no guarantee for any version.
        crystalline::SERVER_NAME => Some(Box::new(crystalline::CrystallineAdapter::new())),
        // The version is not observable: Gleam declares no guarantee for any version.
        gleam::SERVER_NAME => Some(Box::new(gleam::GleamAdapter::new())),
        // The version never appears on the protocol: no guarantee.
        haxe_language_server::SERVER_NAME => Some(Box::new(
            haxe_language_server::HaxeLanguageServerAdapter::new(),
        )),
        "pyright" | "basedpyright" => Some(Box::new(PyrightAdapter::for_identity(&key, version))),
        typescript_language_server::SERVER_NAME => Some(Box::new(
            TypescriptLanguageServerAdapter::for_version(version),
        )),
        _ => None,
    }
}

/// The identity of an upstream that returns no `serverInfo` and announces nothing at startup,
/// read from what its `InitializeResult` declares. Nextflow's language server is known only
/// by its `executeCommandProvider.commands` (`nextflow.server.*`), haskell-language-server by
/// its pid-prefixed ones (`<pid>:ghcide-…`); neither version is observable. `None` when `serverInfo` is present (that is the identity) or nothing is
/// recognized.
pub fn identity_from_initialize_result(body: &[u8]) -> Option<ServerInfo> {
    let root = serde_json::from_slice::<serde_json::Value>(body).ok()?;
    let result = root.get("result")?;
    if result.get("serverInfo").is_some_and(|info| !info.is_null()) {
        return None;
    }
    let name = if nextflow::is_nextflow_initialize_result(result) {
        nextflow::SERVER_NAME
    } else if haskell_language_server::is_hls_initialize_result(result) {
        haskell_language_server::SERVER_NAME
    } else {
        return None;
    };
    Some(ServerInfo {
        name: name.to_string(),
        version: None,
    })
}

/// The identity announcement of an upstream that does not return `serverInfo` (ADR 0011
/// decision A-2).
///
/// Reads the identity announcement an upstream sends on its own at startup from an
/// upstream-to-client notification. That is pyright's `window/logMessage` ("Pyright language
/// server 1.1.412 starting"), typescript-language-server's own `$/typescriptVersion`, and
/// crystalline's `window/logMessage` ("\"[workspace] Found projects:…", version never observable),
/// Gleam's `$/progress` begin of the dependency download ("Downloading Gleam dependencies"), and
/// haxe-language-server's `window/logMessage` ("Haxe Path: …", sent only after
/// `workspace/didChangeConfiguration`, with no version in it).
/// `None` if it is not an identity announcement. No generic recognition mechanism is built
/// (a mapping that needs one adds its own recognition).
pub fn identity_from_notification(view: &MessageView, body: &[u8]) -> Option<ServerInfo> {
    if !view.is_notification() {
        return None;
    }
    match view.method() {
        Some("window/logMessage") => {
            #[derive(serde::Deserialize)]
            struct Envelope {
                params: LogMessage,
            }
            #[derive(serde::Deserialize)]
            struct LogMessage {
                message: String,
            }
            let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
            let message = &envelope.params.message;
            pyright::startup_identity(message)
                .or_else(|| typescript_language_server::startup_identity(message))
                .or_else(|| {
                    crystalline::identity_from_log(message).then(|| ServerInfo {
                        name: crystalline::SERVER_NAME.to_string(),
                        version: None,
                    })
                })
                .or_else(|| {
                    haxe_language_server::identity_from_log(message).then(|| ServerInfo {
                        name: haxe_language_server::SERVER_NAME.to_string(),
                        version: None,
                    })
                })
        }
        Some("$/typescriptVersion") => {
            #[derive(serde::Deserialize)]
            struct Envelope {
                params: serde_json::Value,
            }
            let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
            typescript_language_server::identity_from_typescript_version(&envelope.params)
        }
        Some("$/progress") if gleam::is_dependency_progress_begin(body) => Some(ServerInfo {
            name: gleam::SERVER_NAME.to_string(),
            version: None,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_gopls_by_its_server_info_name() {
        assert!(select("gopls", Some("v0.20.0")).is_some());
    }

    #[test]
    fn selects_pyright_and_basedpyright_by_their_server_info_names() {
        // basedpyright calls itself "basedpyright" in serverInfo. The upstream change adding
        // serverInfo to pyright calls itself by its productName ("Pyright"), so the name
        // comparison is case-insensitive (serverInfo.name is a free-form display string and
        // LSP defines no comparison rule).
        assert!(select("basedpyright", Some("1.39.8")).is_some());
        assert!(select("pyright", None).is_some());
        assert!(
            select("Pyright", None).is_some(),
            "case-insensitive comparison"
        );
        assert!(select("Rust-Analyzer", None).is_some());
        assert!(select("GOPLS", None).is_some());
    }

    #[test]
    fn identifies_typescript_language_server_by_its_startup_log() {
        use crate::peek::peek;
        let body = br#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"Using Typescript version (user-setting) 5.9.3 from path \"/x/tsserver.js\""}}"#;
        let view = peek(body).unwrap();
        let identity = identity_from_notification(&view, body)
            .expect("a startup log is an identity announcement");
        assert_eq!(identity.name, "typescript-language-server");
        assert_eq!(identity.version.as_deref(), Some("5.9.3"));
    }

    #[test]
    fn identifies_typescript_language_server_by_its_typescript_version_notification() {
        use crate::peek::peek;
        let body = br#"{"jsonrpc":"2.0","method":"$/typescriptVersion","params":{"version":"5.9.3","source":"user-setting"}}"#;
        let view = peek(body).unwrap();
        let identity = identity_from_notification(&view, body)
            .expect("its own notification is an identity announcement");
        assert_eq!(identity.name, "typescript-language-server");
        assert_eq!(identity.version.as_deref(), Some("5.9.3"));
        assert!(select(&identity.name, identity.version.as_deref()).is_some());
    }

    #[test]
    fn identifies_pyright_by_its_startup_log() {
        use crate::peek::peek;
        let body = br#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"Pyright language server 1.1.412 starting"}}"#;
        let view = peek(body).unwrap();
        let identity = identity_from_notification(&view, body)
            .expect("a startup log is an identity announcement");
        assert_eq!(identity.name, "pyright");
        assert_eq!(identity.version.as_deref(), Some("1.1.412"));
        assert!(select(&identity.name, identity.version.as_deref()).is_some());
    }

    #[test]
    fn other_notifications_and_requests_are_not_identities() {
        use crate::peek::peek;
        for body in [
            &br#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"Found 2 source files"}}"#[..],
            br#"{"jsonrpc":"2.0","method":"window/showMessage","params":{"type":3,"message":"Pyright language server 1.1.412 starting"}}"#,
            br#"{"jsonrpc":"2.0","id":7,"method":"window/workDoneProgress/create","params":{"token":"t"}}"#,
            br#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"ok","quiescent":true}}"#,
            br#"{"jsonrpc":"2.0","id":0,"result":{"capabilities":{}}}"#,
        ] {
            let view = peek(body).unwrap();
            assert!(
                identity_from_notification(&view, body).is_none(),
                "not an identity announcement: {}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn selects_rust_analyzer_by_its_server_info_name() {
        assert!(select("rust-analyzer", None).is_some());
        for unknown in ["fake-lsp-server", "", "clangd", "rust-analyzer-proxy"] {
            assert!(
                select(unknown, None).is_none(),
                "not a known name: {unknown:?}"
            );
        }
    }

    #[test]
    fn declares_guarantees_only_for_versions_the_conformance_suite_passed_on() {
        // Spec 8.2 item 5 (ADR 0009 decision D-5): the guarantee an observer can declare is
        // limited to versions the conformance tests 7.2 / 7.3 have passed on. lsp-det cannot
        // guarantee rust-analyzer's internals; it only has the observation that a test passed.
        let tested = select("rust-analyzer", Some("1.98.0 (88d9e12 2026-08-18)")).unwrap();
        assert_eq!(
            tested.guarantees(),
            ServerStateProvider::workspace(&[("workspace/symbol", 128)], &ALL_FILE_CHANGES)
        );

        for untested in [
            Some("1.97.0 (abcdef1 2026-07-01)"),
            Some("0.3.2600-standalone"),
            Some("garbage"),
            None,
        ] {
            let adapter = select("rust-analyzer", untested).unwrap();
            assert_eq!(
                adapter.guarantees(),
                ServerStateProvider::notifications_only(),
                "declared a guarantee for untested version {untested:?}"
            );
        }
    }
}
