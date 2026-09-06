//! Holding `ServerState` and its transitions (v0.1-design.md 4.2, ADR 0008).
//!
//! The mapping (the interpretation of upstream messages) and the holding of the state are
//! separated. The mapping cannot be selected until the upstream calls itself a name, so until
//! then both axes are `unknown`. What the server calls itself comes from
//! `InitializeResult.serverInfo`, or, for an upstream that does not return it, from the
//! `window/logMessage` at startup (ADR 0011 decision A). If the name is known, switch to the
//! mapping; otherwise report `unknown` honestly, as it is (spec 8.2 item 3).

use crate::adapter::{self, Mapping};
use crate::initialize::ServerInfo;
use crate::peek::MessageView;
#[cfg(test)]
use crate::state::{ALL_FILE_CHANGES, FileChangeType};
use crate::state::{ServerState, ServerStateProvider};

pub struct Tracker {
    state: ServerState,
    adapter: Option<Box<dyn Mapping>>,
    /// What the server called itself, which was the basis for selecting the mapping.
    identity: Option<ServerInfo>,
    /// The upstream called itself a name in `serverInfo`, but there was no known mapping. From
    /// then on there is no need to look for the name in the upstream's notifications.
    named_but_unknown: bool,
    /// The `initializationOptions` of the client's `initialize`. The mapping is selected after
    /// the upstream calls itself a name, so hold it until then and hand it to the selected
    /// mapping.
    initialization_options: Option<serde_json::Value>,
    /// The `workspaceFolders` of the client's `initialize`, held for the same reason.
    workspace_folders: Vec<std::path::PathBuf>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

impl Tracker {
    /// Until the upstream calls itself a name there is no mapping, and both axes are `unknown`.
    pub fn new() -> Self {
        Tracker {
            state: ServerState::unobserved(),
            adapter: None,
            identity: None,
            named_but_unknown: false,
            initialization_options: None,
            workspace_folders: Vec::new(),
        }
    }

    /// Remember the client's `initialize` (hand `initializationOptions` and `workspaceFolders`
    /// to the mapping).
    pub fn remember_initialize(&mut self, body: &[u8]) {
        self.workspace_folders = crate::initialize::workspace_folders(body);
        if let Some(adapter) = self.adapter.as_mut() {
            adapter.learn_workspace_folders(&self.workspace_folders);
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
            return;
        };
        let options = value["params"]["initializationOptions"].clone();
        if options.is_null() {
            return;
        }
        if let Some(adapter) = self.adapter.as_mut() {
            adapter.learn_initialization_options(&options);
        }
        self.initialization_options = Some(options);
    }

    /// Observe a message in the client-to-upstream direction and update the state.
    /// Returns the new state only when there was a change that requires a notification.
    pub fn observe_client(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        let adapter = self.adapter.as_mut()?;
        let next = adapter.observe_client(view, body)?;
        self.apply(next)
    }

    /// The upstream called itself a name, but there is no known mapping (settled as `unknown`
    /// on both axes). The transparent path may skip the peek. It cannot be skipped while there
    /// is no name yet (what the server calls itself can arrive after the `initialize`
    /// response).
    pub fn upstream_is_unmapped(&self) -> bool {
        self.adapter.is_none() && self.named_but_unknown
    }

    /// What the server called itself, which was the basis for selecting the mapping
    /// (`serverInfo` or the startup log).
    pub fn identity(&self) -> Option<&ServerInfo> {
        self.identity.as_ref()
    }

    /// The upstream called itself a name in `InitializeResult`. If the name is known, select
    /// the mapping and move to its starting state (the `initializing` of "right after
    /// initialize"). If there is no name, or it is not known, do nothing.
    ///
    /// The starting state and the guarantees are asked of the mapping's values. Equating
    /// "there is a mapping" with rust-analyzer would mean rewriting the match when gopls is
    /// added in M4.
    pub fn select_mapping(&mut self, server_info: Option<&ServerInfo>) -> Option<ServerState> {
        let server_info = server_info?;
        if self.adapter.is_none() && adapter::select(&server_info.name, None).is_none() {
            self.named_but_unknown = true;
        }
        if let Some(current) = &self.identity
            && current.name.eq_ignore_ascii_case(&server_info.name)
        {
            // The same mapping has already been selected from the startup log (basedpyright
            // calls itself a name in both). Reselecting would lose the observations read after
            // the startup log (the count of "Starting service instance"), so keep the mapping
            // and tell it the new name (how to update the version used as the basis for the
            // guarantees is up to the mapping).
            if let Some(adapter) = self.adapter.as_mut() {
                adapter.learn_identity(server_info);
            }
            self.identity = Some(server_info.clone());
            return Some(self.state.clone());
        }
        self.adopt(server_info.clone())
    }

    fn adopt(&mut self, identity: ServerInfo) -> Option<ServerState> {
        let mut adapter = adapter::select(&identity.name, identity.version.as_deref())?;
        if let Some(options) = &self.initialization_options {
            adapter.learn_initialization_options(options);
        }
        adapter.learn_workspace_folders(&self.workspace_folders);
        self.state = adapter.initial_state();
        self.adapter = Some(adapter);
        self.identity = Some(identity);
        Some(self.state.clone())
    }

    pub fn state(&self) -> &ServerState {
        &self.state
    }

    /// Whether there is something to read from the upstream's messages (whether there is an
    /// adapter). If not, the transparent path can skip the peek.
    pub fn observes_upstream(&self) -> bool {
        self.adapter.is_some()
    }

    /// The guarantees to declare in `InitializeResult` (spec chapter 5).
    /// Without a mapping, a declaration of no guarantees (`{}`).
    pub fn provider(&self) -> ServerStateProvider {
        // The guarantees are asked of the mapping (spec 8.2 item 5). Which version of which
        // name is used as the basis is up to the mapping (`Mapping::learn_identity`).
        self.adapter
            .as_ref()
            .map_or(ServerStateProvider::notifications_only(), |adapter| {
                adapter.guarantees()
            })
    }

    /// Observe a message in the upstream-to-client direction and update the state.
    /// Returns the new state only when there was a change that requires a notification
    /// (spec 4.2).
    ///
    /// Without an adapter, nothing is read. Only the adapter knows rust-analyzer's vocabulary,
    /// and reading on our own without one would misread a same-named notification from another
    /// server.
    pub fn observe_upstream(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        let Some(adapter) = self.adapter.as_mut() else {
            // There is no mapping yet. If the upstream called itself a name in its startup
            // log, select by that (ADR 0011 decision A-2). If `serverInfo` comes later,
            // reselect by it. The selection only places the starting state and is not a change
            // to notify. The same as when selected by `serverInfo`. The startup log arrives
            // before the `initialize` response, and LSP forbids server-to-client notifications
            // (except logMessage etc.) before the response.
            let identity = adapter::identity_from_notification(view, body)?;
            self.adopt(identity)?;
            // The announcement itself can be a signal (Sorbet: the first
            // `sorbet/showOperation` is the outer start of a nested pair). Let the mapping
            // read it too; a mapping whose announcement is a mere log ignores it.
            let adapter = self.adapter.as_mut()?;
            let next = adapter.interpret(view, body)?;
            return self.apply(next);
        };
        let next = adapter.interpret(view, body)?;
        self.apply(next)
    }

    /// Take in the new state, and return it if the change requires a notification.
    /// A change of `message` alone is not notified, but the state is updated.
    fn apply(&mut self, next: ServerState) -> Option<ServerState> {
        let notifiable = next.notifiable_change_from(&self.state);
        self.state = next;
        notifiable.then(|| self.state.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;
    use crate::state::{Health, Readiness};

    fn observe(tracker: &mut Tracker, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        tracker.observe_upstream(&view, body.as_bytes())
    }

    fn status(health: &str, quiescent: bool) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{{"health":"{health}","quiescent":{quiescent},"message":null}}}}"#
        )
    }

    /// A serverInfo that calls itself a version that passed the conformance tests
    /// (adapter::TESTED_VERSIONS).
    fn info(name: &str) -> ServerInfo {
        ServerInfo {
            name: name.to_string(),
            version: Some("1.98.0 (88d9e12 2026-08-18)".to_string()),
        }
    }

    fn with_adapter() -> Tracker {
        let mut tracker = Tracker::new();
        tracker.select_mapping(Some(&info("rust-analyzer")));
        tracker
    }

    fn without_adapter() -> Tracker {
        let mut tracker = Tracker::new();
        tracker.select_mapping(Some(&info("fake-lsp-server")));
        tracker
    }

    // --- Selection from the startup log (ADR 0011 decision A) ------------------------------

    const PYRIGHT_STARTUP: &str = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"Pyright language server 1.1.412 starting"}}"#;
    const PYRIGHT_STARTED: &str = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"Starting service instance \"pyfix\""}}"#;
    const PYRIGHT_FOUND: &str = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"Found 2 source files"}}"#;

    #[test]
    fn a_startup_log_selects_the_mapping_before_the_upstream_answers_initialize() {
        // pyright returns no serverInfo. Select the mapping by the name in the startup log and
        // move to the starting state (initializing). The selection is not a change to notify
        // (the same as when selected by serverInfo. The startup log arrives before the
        // initialize response, and LSP forbids server-to-client notifications before the
        // response).
        let mut tracker = Tracker::new();
        assert!(
            observe(&mut tracker, PYRIGHT_STARTUP).is_none(),
            "the selection does not notify"
        );
        let state = tracker.state();
        assert_eq!(state.readiness, Readiness::Initializing);
        assert_eq!(state.health, Health::Unknown);
        assert!(tracker.observes_upstream());
        assert_eq!(tracker.identity().map(|i| i.name.as_str()), Some("pyright"));

        // The mapping reads the notifications that follow.
        observe(&mut tracker, PYRIGHT_STARTED);
        let ready =
            observe(&mut tracker, PYRIGHT_FOUND).expect("ready when the enumeration completes");
        assert_eq!(ready.readiness, Readiness::Ready);
    }

    #[test]
    fn an_initialize_result_without_server_info_keeps_the_mapping_from_the_startup_log() {
        let mut tracker = Tracker::new();
        observe(&mut tracker, PYRIGHT_STARTUP);
        assert!(
            tracker.select_mapping(None).is_none(),
            "does not reselect without a name"
        );
        assert!(tracker.observes_upstream());
        assert_eq!(tracker.state().readiness, Readiness::Initializing);
    }

    #[test]
    fn server_info_is_the_stronger_identity_and_reselects() {
        // basedpyright emits both the startup log and serverInfo. When serverInfo comes,
        // reselect by it (it points to the same mapping, so the state stays the starting state).
        let mut tracker = Tracker::new();
        let based = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"basedpyright language server 1.39.8 starting"}}"#;
        observe(&mut tracker, based);
        let state = tracker
            .select_mapping(Some(&ServerInfo {
                name: "basedpyright".to_string(),
                version: Some("1.39.8".to_string()),
            }))
            .expect("reselects by serverInfo");
        assert_eq!(state.readiness, Readiness::Initializing);
        assert!(tracker.observes_upstream());
    }

    #[test]
    fn reselecting_the_same_mapping_from_server_info_keeps_what_it_observed() {
        // basedpyright calls itself a name in the startup log, emits "Starting service
        // instance", and then answers initialize with serverInfo. If the counted folders are
        // discarded when reselecting by serverInfo, the completion log has nothing to count
        // against and it never becomes ready (observed with real basedpyright 1.39.8).
        let mut tracker = Tracker::new();
        let based = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"basedpyright language server 1.39.8 starting"}}"#;
        observe(&mut tracker, based);
        observe(&mut tracker, PYRIGHT_STARTED);
        tracker.select_mapping(Some(&ServerInfo {
            name: "basedpyright".to_string(),
            version: Some("1.39.8".to_string()),
        }));
        let ready =
            observe(&mut tracker, PYRIGHT_FOUND).expect("ready when the counted folders complete");
        assert_eq!(ready.readiness, Readiness::Ready);
    }

    #[test]
    fn server_info_updates_the_declared_guarantees_even_when_the_mapping_is_kept() {
        // Even if the startup log omits the version, declare the guarantees if serverInfo
        // calls itself a tested version (pointed out by Copilot). The guarantees are a function
        // of what the server calls itself (name and version), and the observations (the counted
        // folders) are kept.
        let mut tracker = Tracker::new();
        let unversioned = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"basedpyright language server starting"}}"#;
        observe(&mut tracker, unversioned);
        assert_eq!(
            tracker.provider(),
            ServerStateProvider::notifications_only()
        );
        observe(&mut tracker, PYRIGHT_STARTED);

        tracker.select_mapping(Some(&ServerInfo {
            name: "basedpyright".to_string(),
            version: Some("1.39.8".to_string()),
        }));
        assert_eq!(
            tracker.provider(),
            ServerStateProvider::workspace(&[], &[FileChangeType::Changed]),
            "the guarantees were not redeclared from the serverInfo version"
        );
        let ready = observe(&mut tracker, PYRIGHT_FOUND).expect("the observations are kept");
        assert_eq!(ready.readiness, Readiness::Ready);
    }

    #[test]
    fn typescript_language_server_keeps_the_engine_version_as_the_guarantee_basis() {
        // The upstream change that adds serverInfo to typescript-language-server calls itself
        // by the wrapper's own version (6.0.0). What the guarantees depend on is the version of
        // the analysis engine (TypeScript), which appears in the startup log and in
        // $/typescriptVersion. The serverInfo version does not replace the basis for the
        // guarantees (which version is the basis is up to the mapping).
        let mut tracker = Tracker::new();
        let startup = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"Using Typescript version (user-setting) 5.9.3 from path \"/x/tsserver.js\""}}"#;
        observe(&mut tracker, startup);
        assert_eq!(
            tracker.provider(),
            ServerStateProvider::workspace(&[], &[FileChangeType::Changed])
        );
        tracker.select_mapping(Some(&ServerInfo {
            name: "typescript-language-server".to_string(),
            version: Some("6.0.0".to_string()),
        }));
        assert_eq!(
            tracker.provider(),
            ServerStateProvider::workspace(&[], &[FileChangeType::Changed]),
            "the guarantees were dropped because of the wrapper's version"
        );
    }

    #[test]
    fn the_same_name_in_another_case_keeps_the_mapping_too() {
        // The upstream change that adds serverInfo to pyright calls itself by the productName
        // "Pyright". It is the same mapping as the one read as "pyright" from the startup log,
        // so do not reselect, and keep the observations.
        let mut tracker = Tracker::new();
        observe(&mut tracker, PYRIGHT_STARTUP);
        observe(&mut tracker, PYRIGHT_STARTED);
        tracker.select_mapping(Some(&ServerInfo {
            name: "Pyright".to_string(),
            version: Some("1.1.412".to_string()),
        }));
        assert_eq!(
            tracker.provider(),
            ServerStateProvider::workspace(&[], &[FileChangeType::Changed])
        );
        let ready =
            observe(&mut tracker, PYRIGHT_FOUND).expect("ready when the counted folders complete");
        assert_eq!(ready.readiness, Readiness::Ready);
    }

    #[test]
    fn a_different_name_in_server_info_replaces_the_mapping() {
        // If the name differs, serverInfo is stronger. Discard the startup log's mapping and
        // reselect.
        let mut tracker = Tracker::new();
        observe(&mut tracker, PYRIGHT_STARTUP);
        let state = tracker
            .select_mapping(Some(&info("rust-analyzer")))
            .expect("reselects by serverInfo");
        assert_eq!(state.readiness, Readiness::Initializing);
        assert!(
            observe(&mut tracker, &status("ok", true)).is_some(),
            "rust-analyzer's mapping"
        );
        assert_eq!(
            tracker.identity().map(|i| i.name.as_str()),
            Some("rust-analyzer")
        );
    }

    #[test]
    fn ordinary_logs_do_not_select_a_mapping() {
        let mut tracker = Tracker::new();
        assert!(observe(&mut tracker, PYRIGHT_FOUND).is_none());
        assert!(!tracker.observes_upstream());
        assert_eq!(tracker.state(), &ServerState::unobserved());
    }

    #[test]
    fn a_startup_log_does_not_replace_a_mapping_chosen_from_server_info() {
        // If there is already a mapping, do not reselect by the name in the startup log
        // (serverInfo is stronger).
        let mut tracker = with_adapter();
        assert!(observe(&mut tracker, PYRIGHT_STARTUP).is_none());
        assert!(
            observe(&mut tracker, &status("ok", true)).is_some(),
            "still rust-analyzer's mapping"
        );
    }

    // --- Starting state ------------------------------------------------------------------

    #[test]
    fn starts_unobserved_before_the_upstream_names_itself() {
        // The mapping is selected by serverInfo. Until then nothing has been observed.
        assert_eq!(Tracker::new().state(), &ServerState::unobserved());
    }

    #[test]
    fn selecting_a_known_mapping_moves_to_initializing() {
        // readiness is initializing, because "right after initialize" is a known phase.
        // health is unknown, because nothing has been observed until the first serverStatus
        // arrives (spec 8.2 item 2). Claiming ok would be an assertion without observation.
        let mut tracker = Tracker::new();
        let state = tracker
            .select_mapping(Some(&info("rust-analyzer")))
            .expect("a known name moves to the starting state");
        assert_eq!(state.readiness, Readiness::Initializing);
        assert_eq!(state.health, Health::Unknown);
        assert!(tracker.observes_upstream());
    }

    #[test]
    fn the_first_status_replaces_the_unknown_health() {
        let mut tracker = with_adapter();
        let changed = observe(&mut tracker, &status("ok", false)).expect("state should change");
        assert_eq!(changed.health, Health::Ok);
    }

    #[test]
    fn stays_unobserved_when_the_name_is_unknown_or_absent() {
        // Spec 8.2 item 3: a server with no signal is unknown on both axes. Start neither
        // from initializing nor from ok.
        let mut tracker = Tracker::new();
        assert!(tracker.select_mapping(None).is_none());
        assert!(tracker.select_mapping(Some(&info("ccls"))).is_none());
        assert_eq!(tracker.state(), &ServerState::unobserved());
        assert!(!tracker.observes_upstream());
    }

    #[test]
    fn declares_the_adapter_guarantees_or_no_guarantees() {
        assert_eq!(
            with_adapter().provider(),
            ServerStateProvider::workspace(&[("workspace/symbol", 128)], &ALL_FILE_CHANGES)
        );
        assert_eq!(
            without_adapter().provider(),
            ServerStateProvider::notifications_only()
        );
    }

    // --- Transitions (with an adapter) ----------------------------------------------------

    #[test]
    fn applies_what_the_adapter_reads() {
        let mut tracker = with_adapter();
        let changed = observe(&mut tracker, &status("ok", true)).expect("state should change");
        assert_eq!(changed.readiness, Readiness::Ready);
        assert_eq!(tracker.state().readiness, Readiness::Ready);
    }

    #[test]
    fn repeating_the_same_state_does_not_notify_again() {
        let mut tracker = with_adapter();
        assert!(observe(&mut tracker, &status("ok", true)).is_some());
        assert!(observe(&mut tracker, &status("ok", true)).is_none());
    }

    #[test]
    fn losing_quiescence_returns_to_indexing() {
        // Reindexing (v0.1-design 4.3, spec chapter 6 item 3). A regression of readiness is
        // also a change of the two axes, so it is notified.
        let mut tracker = with_adapter();
        observe(&mut tracker, &status("ok", true));
        let changed = observe(&mut tracker, &status("ok", false)).expect("re-index should notify");
        assert_eq!(changed.readiness, Readiness::Indexing);
    }

    #[test]
    fn a_message_only_change_updates_state_without_notifying() {
        // Spec 4.2: notify only when the two axes change. But message is updated so that it
        // appears in the next serverState response.
        let mut tracker = with_adapter();
        observe(&mut tracker, &status("ok", false));

        let with_message = r#"{"method":"experimental/serverStatus","params":{"health":"ok","quiescent":false,"message":"loading crates"}}"#;
        assert!(observe(&mut tracker, with_message).is_none());
        assert_eq!(tracker.state().message.as_deref(), Some("loading crates"));
    }

    // --- Transitions (without an adapter) -------------------------------------------------

    #[test]
    fn does_not_interpret_upstream_status_without_an_adapter() {
        // Only the adapter knows rust-analyzer's vocabulary. Reading on our own without one
        // would misread a same-named notification from another server.
        let mut tracker = without_adapter();
        assert!(observe(&mut tracker, &status("ok", true)).is_none());
        assert_eq!(tracker.state(), &ServerState::unobserved());
    }

    #[test]
    fn the_notification_that_selected_the_mapping_is_also_read_by_it() {
        // Sorbet's identity is its first `sorbet/showOperation`, and that very notification is
        // a readiness signal (the outer `SlowPathBlocking` start of a nested pair). Measured
        // startup order: SlowPathBlocking start, Indexing start, Indexing end, SlowPathBlocking
        // end. If the tracker swallowed the identity message, the mapping would count only the
        // inner pair and declare `ready` at the inner end, while the server is still
        // typechecking.
        fn operation(name: &str, status: &str) -> String {
            format!(
                r#"{{"jsonrpc":"2.0","method":"sorbet/showOperation","params":{{"operationName":"{name}","description":"...","status":"{status}"}}}}"#
            )
        }
        let mut tracker = Tracker::new();
        assert!(
            observe(&mut tracker, &operation("SlowPathBlocking", "start")).is_none(),
            "the selection does not notify"
        );
        assert_eq!(tracker.identity().map(|i| i.name.as_str()), Some("sorbet"));
        assert!(observe(&mut tracker, &operation("Indexing", "start")).is_none());
        assert!(
            observe(&mut tracker, &operation("Indexing", "end")).is_none(),
            "the outer operation is still open: not ready yet"
        );
        assert_eq!(tracker.state().readiness, Readiness::Initializing);
        let ready = observe(&mut tracker, &operation("SlowPathBlocking", "end"))
            .expect("ready when the outer operation ends");
        assert_eq!(ready.readiness, Readiness::Ready);
    }
}
