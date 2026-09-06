//! Conformance test suite for the upstream side of the server state protocol
//! (docs/spec/server-state.md chapter 7 and 8.4).
//!
//! This makes chapter 7 of the spec (server obligations) and 8.4 (observer
//! conformance requirements) executable, and the subject under test can be
//! anything that is "a command that speaks LSP over stdio". lsp-det is only
//! the first subject (v0.1-design.md chapter 6).
//!
//! Each test name corresponds to a spec item number. If the spec changes,
//! this file should fail.
//!
//! 7.2 (coverage) and 7.3 (freshness) only make sense when the subject
//! declares a guarantee. Running lsp-det + fake upstream comes after the
//! downstream side (M3). Downstream conformance requirements (spec 9.1)
//! are handled by a separate suite.

mod support;

use std::time::Duration;

use lsp_det::state::{Health, Readiness};
use serde_json::{Value, json};
use support::{ConformanceClient, ServerUnderTest};

/// The observation window used to confirm that something does NOT arrive.
const NEGATIVE_WINDOW: Duration = Duration::from_millis(750);

fn client(declare_server_state: bool) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_upstream();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(declare_server_state);
    (client, result)
}

// ---------------------------------------------------------------------------
// chapter 5: capability
// ---------------------------------------------------------------------------

#[test]
fn spec_5_declares_the_server_state_provider_capability() {
    let (mut client, result) = client(true);
    let provider = &result["result"]["capabilities"]["experimental"]["serverStateProvider"];
    assert!(
        !provider.is_null(),
        "InitializeResult is missing experimental.serverStateProvider: {result}"
    );
    client.shutdown();
}

#[test]
fn spec_5_keeps_the_upstream_capabilities_intact() {
    // This adds a declaration; it must not replace the upstream's own declarations.
    let (mut client, result) = client(true);
    let capabilities = &result["result"]["capabilities"];
    assert_eq!(capabilities["hoverProvider"], json!(true));
    assert_eq!(capabilities["referencesProvider"], json!(true));
    assert_eq!(
        capabilities["experimental"]["fakeUpstreamMarker"],
        json!(true),
        "the upstream's experimental was lost: {capabilities}"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// 7.1 declaration without a guarantee
// ---------------------------------------------------------------------------

#[test]
fn spec_7_1_1_answers_server_state_right_after_initialize() {
    // The fake upstream has not emitted a signal yet, so the upstream side
    // reports initializing, corresponding to "right after initialize"
    // (spec 8.2 item 2). Not being ready is not a spec requirement (it is a
    // precondition of chapter 7); it is a fact about this subject.
    let (mut client, _) = client(true);
    let state = client.server_state();
    assert_eq!(state.readiness, Readiness::Initializing);
    assert_eq!(state.health, Health::Unknown);
    client.shutdown();
}

#[test]
fn spec_7_1_1_answers_server_state_even_without_the_client_declaration() {
    // Spec 5.2: the request is answered regardless of whether it was declared.
    let (mut client, _) = client(false);
    let state = client.server_state();
    assert_eq!(state.readiness, Readiness::Initializing);
    client.shutdown();
}

#[test]
fn spec_7_1_2_sends_state_changed_when_the_client_declared() {
    let (mut client, _) = client(true);
    client.make_upstream_emit_status("ok", true);
    let state = client.await_state_changed();
    assert_eq!(state.readiness, Readiness::Ready);
    assert_eq!(state.health, Health::Ok);
    client.shutdown();
}

#[test]
fn spec_7_1_2_stays_silent_when_the_client_did_not_declare() {
    let (mut client, _) = client(false);
    client.make_upstream_emit_status("ok", true);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "must not send serverStateChanged to a client that did not declare it"
    );
    // The state itself is still tracked, so the request returns the new value.
    assert_eq!(client.server_state().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn spec_7_1_3_observes_ready_then_indexing_then_ready() {
    let (mut client, _) = client(true);

    client.make_upstream_emit_status("ok", true);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);

    // Reindexing corresponding to a dependency change.
    client.make_upstream_emit_status("ok", false);
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);

    client.make_upstream_emit_status("ok", true);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);

    client.shutdown();
}

// ---------------------------------------------------------------------------
// chapter 4: method semantics
// ---------------------------------------------------------------------------

#[test]
fn spec_4_2_does_not_repeat_a_notification_for_an_unchanged_state() {
    let (mut client, _) = client(true);
    client.make_upstream_emit_status("ok", true);
    client.await_state_changed();

    client.make_upstream_emit_status("ok", true);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "must not notify when neither axis has changed"
    );
    client.shutdown();
}

#[test]
fn spec_4_1_does_not_forward_the_state_request_upstream() {
    // This is a method the upstream side answers by itself; the upstream does not know this protocol.
    let (mut client, _) = client(true);
    client.server_state();
    let seen = client.upstream_methods_seen();
    assert!(
        !seen.iter().any(|m| m == "experimental/serverState"),
        "forwarded experimental/serverState to the upstream: {seen:?}"
    );
    client.shutdown();
}

#[test]
fn spec_8_2_7_closes_the_connection_without_a_notification_when_the_upstream_disappears() {
    // Spec 8.2 item 7: process disappearance is not a value of this protocol.
    // The relay answers any unanswered requests with an error, then closes
    // the connection, and EOF propagates to the downstream. It does not
    // send a notification meaning "dead".
    let (mut client, _) = client(true);
    client.make_upstream_emit_status("ok", true);
    client.await_state_changed();

    client.notify("exit", json!(null));
    assert!(
        client.expect_silence_until_closed("experimental/serverStateChanged"),
        "notified upstream disappearance via serverStateChanged (violates spec 8.2 item 7)"
    );
}

#[test]
fn spec_7_1_4_reports_an_index_failure_as_health_error() {
    // Failure is expressed via health, not readiness (spec chapter 6 item 5).
    // rust-analyzer sends a workspace load failure as {health: error, quiescent: true}.
    let (mut client, _) = client(true);
    client.make_upstream_emit_status("error", true);
    let state = client.await_state_changed();
    assert_eq!(state.health, Health::Error);
    client.shutdown();
}

// ---------------------------------------------------------------------------
// Mapping selection and capability injection (design 4.2, ADR 0009 decision D-2/D-3)
//
// The mapping is chosen by the name the upstream calls itself in
// InitializeResult.serverInfo.name. Since initialize must be sent to the
// upstream before it calls itself anything, the capabilities for every
// known mapping are injected unconditionally.
// ---------------------------------------------------------------------------

#[test]
fn selects_the_mapping_from_the_server_info_name() {
    // The default subject has the fake upstream call itself rust-analyzer. There is no --adapter.
    let (mut client, result) = client(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"coverage": {"scope": "workspace", "incomplete": {"workspace/symbol": 128}}, "freshness": {"fileChanges": ["Created", "Changed", "Deleted"]}}),
        "the rust-analyzer mapping was not chosen for an upstream that calls itself rust-analyzer: {result}"
    );
    client.make_upstream_emit_status("ok", true);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn spec_8_2_5_declares_no_guarantees_for_an_untested_version() {
    // Spec 8.2 item 5: a guarantee is declared only for the range of
    // versions that passed conformance tests. For an out-of-range version,
    // or an upstream that does not report its version, only the state
    // notification is promised.
    for version in ["1.97.0 (old)", "none"] {
        let server =
            ServerUnderTest::lsp_det_with_fake_upstream_flags(&["--server-version", version]);
        let mut client = ConformanceClient::start(&server);
        let result = client.initialize(true);
        assert_eq!(
            result["result"]["capabilities"]["experimental"]["serverStateProvider"],
            json!({}),
            "declared a guarantee for untested version {version:?}: {result}"
        );
        // The mapping itself still works (the state is still tracked).
        client.make_upstream_emit_status("ok", true);
        assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
        client.shutdown();
    }
}

#[test]
fn maps_the_missing_workspace_warning_of_rust_analyzer_to_error() {
    // Design 5.1: the mapping compensates for the coarseness of a language
    // server's vocabulary. A "project not found" warning is a malfunction
    // for cross-workspace queries, so it is mapped to error.
    let (mut client, _) = client(true);
    client.make_upstream_emit_status_with_message(
        "warning",
        true,
        "Failed to discover workspace.\nConsider adding the `Cargo.toml` ...",
    );
    let state = client.await_state_changed();
    assert_eq!(state.health, Health::Error);
    client.shutdown();
}

#[test]
fn injects_the_capabilities_of_every_known_mapping_unconditionally() {
    // serverInfo is only known from the initialize response. Injection is
    // needed before that, so the capabilities for every known mapping are
    // injected regardless of who the upstream turns out to be.
    for server in [
        ServerUnderTest::lsp_det_with_fake_upstream(),
        ServerUnderTest::lsp_det_without_adapter(),
    ] {
        let mut client = ConformanceClient::start(&server);
        client.initialize(true);
        let capabilities = client.upstream_client_capabilities();
        assert_eq!(
            capabilities["experimental"]["serverStatusNotification"],
            json!(true),
            "the declaration for rust-analyzer was not injected: {capabilities}"
        );
        assert_eq!(
            capabilities["window"]["workDoneProgress"],
            json!(true),
            "the declaration for gopls was not injected: {capabilities}"
        );
        // Injection only sets the two target keys to true; the client's other
        // declarations (hover, etc.) are left intact.
        assert_eq!(capabilities["textDocument"]["hover"], json!({}));
        client.shutdown();
    }
}

#[test]
fn answers_work_done_progress_create_itself_when_the_client_did_not_declare_it() {
    // A request that originates from the injected window.workDoneProgress is
    // answered directly by lsp-det with success, rather than forwarded to the
    // client (design 4.2). A client that did not declare it would return
    // MethodNotFound (confirmed with Serena).
    let server = ServerUnderTest::lsp_det_with_fake_upstream_flags(&["--request-progress-create"]);
    let mut client = ConformanceClient::start(&server);
    client.initialize(true);
    assert!(
        client.expect_no_notification("window/workDoneProgress/create", NEGATIVE_WINDOW),
        "forwarded window/workDoneProgress/create to a client that did not declare it"
    );
    assert!(
        client.upstream_progress_create_answered(),
        "did not answer the upstream's window/workDoneProgress/create"
    );
    client.shutdown();
}

#[test]
fn forwards_work_done_progress_create_when_the_client_declared_it() {
    // A request based on a capability the client originally declared is passed through.
    let server = ServerUnderTest::lsp_det_with_fake_upstream_flags(&["--request-progress-create"]);
    let mut client = ConformanceClient::start(&server);
    client.initialize_with_capabilities(json!({
        "window": {"workDoneProgress": true},
        "experimental": {"serverState": true},
    }));
    assert!(
        client
            .await_notification("window/workDoneProgress/create")
            .is_some(),
        "window/workDoneProgress/create did not reach the client that declared it"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// The gopls mapping (design 5.2)
//
// gopls has no vocabulary for readiness. The upstream side synthesizes
// readiness from the begin/end of "Setting up workspace" in `$/progress`,
// and health from the begin/end of "Error loading workspace".
// ---------------------------------------------------------------------------

fn gopls_client(declare_server_state: bool) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_gopls();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(declare_server_state);
    (client, result)
}

#[test]
fn gopls_spec_8_2_5_declares_no_guarantees_for_an_untested_version() {
    // The fake gopls's default version (1.98.0 (fake)) can be read, but it
    // is outside the range of gopls::TESTED_VERSIONS, so no guarantee is declared.
    let (mut client, result) = gopls_client(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared an unmeasured guarantee for gopls: {result}"
    );
    client.shutdown();
}

#[test]
fn gopls_spec_5_declares_the_measured_guarantees_for_a_tested_version() {
    // 7.2 / 7.3 were run against real gopls v0.23.0 and passed (gopls_* ignored).
    let server =
        ServerUnderTest::lsp_det_with_upstream_flags("gopls", &["--server-version", "v0.23.0"]);
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"coverage": {"scope": "workspace", "incomplete": {"workspace/symbol": 100}}, "freshness": {"fileChanges": ["Created", "Changed", "Deleted"]}}),
        "did not declare a guarantee for a measured version: {result}"
    );
    client.shutdown();
}

#[test]
fn gopls_spec_7_1_1_starts_initializing_with_unknown_health() {
    let (mut client, _) = gopls_client(true);
    let state = client.server_state();
    assert_eq!(state.readiness, Readiness::Initializing);
    assert_eq!(state.health, Health::Unknown);
    client.shutdown();
}

#[test]
fn gopls_spec_7_1_2_becomes_ready_when_the_workspace_load_ends() {
    let (mut client, _) = gopls_client(true);
    client.make_upstream_begin_workspace_load("1234");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    client.make_upstream_end_workspace_load("1234", "Finished loading packages.");
    let state = client.await_state_changed();
    assert_eq!(state.readiness, Readiness::Ready);
    assert_eq!(
        state.health,
        Health::Ok,
        "health is ok when the load succeeds"
    );
    client.shutdown();
}

#[test]
fn gopls_waits_for_every_workspace_folder() {
    // One progress is emitted per folder. It is not ready until all of them finish.
    let (mut client, _) = gopls_client(true);
    client.make_upstream_begin_workspace_load("a");
    client.make_upstream_begin_workspace_load("b");
    client.await_state_changed();
    client.make_upstream_end_workspace_load("a", "Finished loading packages.");
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "declared ready when only one folder finished"
    );
    client.make_upstream_end_workspace_load("b", "Finished loading packages.");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn gopls_spec_7_1_3_rearms_when_a_folder_is_added() {
    // didChangeWorkspaceFolders causes "Setting up workspace" to be re-emitted.
    let (mut client, _) = gopls_client(true);
    client.make_upstream_begin_workspace_load("1");
    client.await_state_changed();
    client.make_upstream_end_workspace_load("1", "Finished loading packages.");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);

    client.make_upstream_begin_workspace_load("2");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    client.make_upstream_end_workspace_load("2", "Finished loading packages.");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn gopls_spec_7_1_4_reports_a_workspace_load_failure_as_health_error() {
    let (mut client, _) = gopls_client(true);
    client.make_upstream_begin_workspace_load("1");
    client.await_state_changed();
    client.make_upstream_emit_progress(json!({
        "token": "err",
        "value": {"kind": "begin", "title": "Error loading workspace", "message": "err: go.mod file not found", "cancellable": false}
    }));
    let state = client.await_state_changed();
    assert_eq!(state.health, Health::Error);
    assert_eq!(
        state.message.as_deref(),
        Some("err: go.mod file not found"),
        "attaches the failure message"
    );

    // Recovery: ends with "Done.".
    client.make_upstream_emit_progress(json!({
        "token": "err",
        "value": {"kind": "end", "message": "Done."}
    }));
    assert_eq!(client.await_state_changed().health, Health::Ok);
    client.shutdown();
}

#[test]
fn gopls_reports_a_failed_load_as_health_error() {
    // A folder load failure ends with "Error loading packages: ...".
    // The attempt is over, so it is ready; the result is not trustworthy, so it is error
    // (spec chapter 6 item 5).
    let (mut client, _) = gopls_client(true);
    client.make_upstream_begin_workspace_load("1");
    client.await_state_changed();
    client.make_upstream_end_workspace_load("1", "Error loading packages: no Go files");
    let state = client.await_state_changed();
    assert_eq!(state.readiness, Readiness::Ready);
    assert_eq!(state.health, Health::Error);
    client.shutdown();
}

#[test]
fn gopls_ignores_unrelated_progress() {
    // A progress with a different title, such as diagnostics or govulncheck, does not touch readiness.
    let (mut client, _) = gopls_client(true);
    client.make_upstream_emit_progress(json!({
        "token": "diag",
        "value": {"kind": "begin", "title": "Calculating diagnostics", "message": "..."}
    }));
    client.make_upstream_emit_progress(json!({
        "token": "diag",
        "value": {"kind": "end", "message": "Done."}
    }));
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "state moved because of an unrelated progress"
    );
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    client.shutdown();
}

// ---------------------------------------------------------------------------
// The pyright mapping (ADR 0011, design 5.3)
//
// pyright has no vocabulary for readiness and does not return `serverInfo`
// either. The upstream side chooses the mapping from what the startup log
// calls the server, and synthesizes readiness from the completion of file
// enumeration in `window/logMessage` ("Found N source files" /
// "No source files found."). There is no signal for health, so it stays unknown.
// ---------------------------------------------------------------------------

fn pyright_client(declare_server_state: bool) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_pyright();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(declare_server_state);
    (client, result)
}

#[test]
fn pyright_is_identified_by_its_startup_log_when_server_info_is_absent() {
    // Without serverInfo there would be no mapping (both axes unknown), but
    // the pyright mapping is chosen from what the startup log calls the
    // server, and it is in the starting state (initializing).
    let (mut client, result) = pyright_client(true);
    assert!(
        !result["result"]["capabilities"]["experimental"]["serverStateProvider"].is_null(),
        "the upstream side has no declaration: {result}"
    );
    assert!(
        result["result"]["serverInfo"].is_null(),
        "the premise is broken: the fake pyright should not return serverInfo: {result}"
    );
    let state = client.server_state();
    assert_eq!(
        state.readiness,
        Readiness::Initializing,
        "no mapping was chosen"
    );
    assert_eq!(state.health, Health::Unknown);
    client.shutdown();
}

#[test]
fn basedpyright_is_identified_by_its_server_info() {
    // basedpyright returns serverInfo. The same mapping is chosen even without a startup log.
    let server = ServerUnderTest::lsp_det_with_upstream_flags(
        "basedpyright",
        &["--server-version", "1.39.8"],
    );
    let mut client = ConformanceClient::start(&server);
    client.initialize(true);
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    client.shutdown();
}

#[test]
fn pyright_spec_8_2_5_declares_no_guarantees_for_an_untested_version() {
    // If the version in the startup log is not in pyright::TESTED_VERSIONS, no guarantee is declared.
    let server = ServerUnderTest::lsp_det_with_upstream_flags(
        "none",
        &["--startup-log", "Pyright language server 1.1.400 starting"],
    );
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared an unmeasured guarantee for pyright: {result}"
    );
    client.shutdown();
}

#[test]
fn pyright_spec_5_declares_the_measured_guarantees_for_a_tested_version() {
    // 7.2 / 7.3 were run against real pyright 1.1.412 and passed (pyright_* ignored).
    // The version is read from the startup log (there is no serverInfo).
    let (mut client, result) = pyright_client(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"coverage": {"scope": "workspace", "incomplete": {}}, "freshness": {"fileChanges": ["Changed"]}}),
        "did not declare a guarantee for a measured version: {result}"
    );
    client.shutdown();
}

#[test]
fn basedpyright_spec_5_declares_the_measured_guarantees_for_a_tested_version() {
    // 7.2 / 7.3 were run against real basedpyright 1.39.8 and passed. The version is read from serverInfo.
    let server = ServerUnderTest::lsp_det_with_upstream_flags(
        "basedpyright",
        &["--server-version", "1.39.8"],
    );
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"coverage": {"scope": "workspace", "incomplete": {}}, "freshness": {"fileChanges": ["Changed"]}}),
        "did not declare a guarantee for a measured version: {result}"
    );
    client.shutdown();
}

#[test]
fn pyright_spec_7_1_2_becomes_ready_when_enumeration_completes() {
    let (mut client, _) = pyright_client(true);
    client.make_upstream_start_service_instance("pyfix");
    client.make_upstream_finish_enumeration("Found 2 source files");
    let state = client.await_state_changed();
    assert_eq!(state.readiness, Readiness::Ready);
    assert_eq!(
        state.health,
        Health::Unknown,
        "enumeration completion is not an observation of health"
    );
    client.shutdown();
}

#[test]
fn pyright_no_source_files_is_also_a_completion() {
    let (mut client, _) = pyright_client(true);
    client.make_upstream_start_service_instance("empty");
    client.make_upstream_finish_enumeration("No source files found.");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn pyright_waits_for_every_workspace_folder() {
    // "Starting service instance" and a completion log are each emitted once per folder.
    let (mut client, _) = pyright_client(true);
    client.make_upstream_start_service_instance("one");
    client.make_upstream_start_service_instance("two");
    client.make_upstream_finish_enumeration("Found 400 source files");
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "declared ready when only one folder finished"
    );
    client.make_upstream_finish_enumeration("Found 1200 source files");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn pyright_spec_7_1_3_rearms_when_a_folder_is_added() {
    let (mut client, _) = pyright_client(true);
    client.make_upstream_start_service_instance("one");
    client.make_upstream_finish_enumeration("Found 1 source file");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);

    client.make_upstream_start_service_instance("two");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    client.make_upstream_finish_enumeration("Found 3 source files");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn pyright_rearms_on_reenumeration_when_the_log_is_visible() {
    // "Searching for source files" is at log level (type 4). It does not
    // arrive by default, but when it does arrive it marks the start of re-enumeration.
    let (mut client, _) = pyright_client(true);
    client.make_upstream_start_service_instance("one");
    client.make_upstream_finish_enumeration("Found 1 source file");
    client.await_state_changed();

    client.make_upstream_emit_log_message(4, "Searching for source files");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    client.make_upstream_finish_enumeration("Found 2 source files");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn pyright_ignores_progress_and_other_logs() {
    // $/progress from analyzing an open file, and other logs, do not touch readiness.
    let (mut client, _) = pyright_client(true);
    client.make_upstream_start_service_instance("one");
    client.make_upstream_emit_log_message(3, "Assuming Python version 3.14.7.final.0");
    client.make_upstream_emit_progress(json!({
        "token": "t",
        "value": {"kind": "begin", "title": ""}
    }));
    client.make_upstream_emit_progress(json!({
        "token": "t",
        "value": {"kind": "end"}
    }));
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "state moved because of an unrelated message"
    );
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    client.shutdown();
}

#[test]
fn pyright_logs_are_forwarded_to_the_client_unchanged() {
    // The mapping only reads the log; it still reaches the client unchanged (verbatim forwarding).
    let (mut client, _) = pyright_client(false);
    client.make_upstream_start_service_instance("one");
    client.make_upstream_finish_enumeration("Found 2 source files");
    let found = client
        .await_notification("window/logMessage")
        .expect("the log did not arrive");
    assert!(
        found["message"].as_str().is_some(),
        "the log's shape changed: {found}"
    );
    client.shutdown();
}
// ---------------------------------------------------------------------------
// No mapping (spec 8.2 item 3, 8.4 item 1)
//
// There is no way to observe readiness, so both axes are honestly reported as unknown.
// ---------------------------------------------------------------------------

fn client_without_adapter(declare_server_state: bool) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_without_adapter();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(declare_server_state);
    (client, result)
}

#[test]
fn spec_5_declares_without_guarantees_when_there_is_no_adapter() {
    let (mut client, result) = client_without_adapter(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "with no mapping, a declaration without a guarantee (true) is made: {result}"
    );
    client.shutdown();
}

#[test]
fn spec_8_4_1_reports_unknown_on_both_axes_without_an_adapter() {
    let (mut client, _) = client_without_adapter(true);
    let state = client.server_state();
    assert_eq!(state.readiness, Readiness::Unknown);
    assert_eq!(state.health, Health::Unknown);
    client.shutdown();
}

#[test]
fn does_not_interpret_the_upstream_status_without_an_adapter() {
    // Even if an upstream that calls itself a name with no known mapping
    // sends an rust-analyzer-style serverStatus, it is not read. This
    // avoids misreading another server's identically named notification.
    let (mut client, _) = client_without_adapter(true);
    client.make_upstream_emit_status("ok", true);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "readiness must not move without an adapter"
    );
    assert_eq!(client.server_state().readiness, Readiness::Unknown);
    client.shutdown();
}

#[test]
fn an_upstream_that_dies_before_answering_initialize_does_not_hang_the_client_without_an_adapter() {
    // Even without an adapter, a death during the handshake is not hidden.
    let server =
        ServerUnderTest::lsp_det_without_adapter_flags(&["--exit-before-initialize-result"]);
    let mut client = ConformanceClient::start(&server);
    let response = client.initialize_raw(true);
    assert!(
        response.get("error").is_some(),
        "the upstream disappeared without answering initialize, but no error was returned either: {response}"
    );
}

#[test]
fn spec_8_2_7_closes_the_connection_without_a_notification_without_an_adapter() {
    // Same without a mapping. Instead of emitting "dead", the connection is closed and EOF conveys it.
    let (mut client, _) = client_without_adapter(true);
    client.notify("exit", json!(null));
    assert!(
        client.expect_silence_until_closed("experimental/serverStateChanged"),
        "notified upstream disappearance via serverStateChanged (violates spec 8.2 item 7)"
    );
}

// ---------------------------------------------------------------------------
// When the upstream itself declares the protocol (spec 8.2 item 6, 8.4 item 2)
//
// The upstream side becomes the identity mapping. It adds no declaration,
// forwards requests, and emits no notification of its own. This avoids two
// streams with different senders flowing over the same connection.
// ---------------------------------------------------------------------------

#[test]
fn spec_8_4_2_asks_the_conformant_upstream_only_after_initialized() {
    // Under the identity mapping, the upstream side queries the initial
    // state itself (design 4.2). That query must happen after the client's
    // `initialized` has been forwarded. LSP allows a server to refuse other
    // requests until `initialized`, and rust-analyzer actually exits for a
    // protocol violation (observed with an upstream patch).
    let server = ServerUnderTest::lsp_det_without_adapter_flags(&[
        "--declare-server-state-provider",
        "--require-initialized-before-requests",
    ]);
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"freshness": {"fileChanges": ["Changed"]}}),
        "rewrote the upstream's declaration: {result}"
    );
    let state = client.server_state();
    assert_eq!(
        state.message.as_deref(),
        Some("answered by upstream"),
        "queried before initialized and crashed the upstream"
    );
    client.shutdown();
}

#[test]
fn spec_8_4_2_defers_to_a_conformant_upstream_without_an_adapter() {
    let server =
        ServerUnderTest::lsp_det_without_adapter_flags(&["--declare-server-state-provider"]);
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);

    // The upstream's declaration is passed through unchanged (not overwritten
    // by a declaration without a guarantee).
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"freshness": {"fileChanges": ["Changed"]}}),
        "rewrote the upstream's declaration: {result}"
    );

    // The request reaches the upstream and the upstream's answer comes back.
    let state = client.server_state();
    assert_eq!(state.message.as_deref(), Some("answered by upstream"));
    assert!(
        client
            .upstream_methods_seen()
            .iter()
            .any(|m| m == "experimental/serverState"),
        "did not forward experimental/serverState to the upstream"
    );

    // The upstream side emits no notification of its own (the upstream is the sender).
    client.notify("exit", json!(null));
    assert!(
        client.expect_silence_until_closed("experimental/serverStateChanged"),
        "the upstream side, which should have been the identity mapping, emitted a notification"
    );
}

#[test]
fn a_false_upstream_declaration_is_not_a_declaration() {
    // `serverStateProvider: false` means "does not provide it". It must not
    // switch to the identity mapping; the upstream side leaves its own
    // declaration in place and answers on its own.
    let server =
        ServerUnderTest::lsp_det_without_adapter_flags(&["--declare-server-state-provider-false"]);
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "treated false as a declaration and switched to the identity mapping: {result}"
    );
    assert_eq!(client.server_state().readiness, Readiness::Unknown);
    client.shutdown();
}

#[test]
fn does_not_emit_its_own_notifications_under_deferral_with_an_adapter() {
    // Emitting a serverStateChanged of its own during the identity mapping
    // would create two streams alongside the upstream's notification
    // (spec 8.2 item 6). Even if the mapping reads a live transition, it must not emit one.
    let server =
        ServerUnderTest::lsp_det_with_fake_upstream_flags(&["--declare-server-state-provider"]);
    let mut client = ConformanceClient::start(&server);
    client.initialize(true);
    client.make_upstream_emit_status("ok", true);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "the upstream side emitted its own notification during the identity mapping"
    );
    client.shutdown();
}

#[test]
fn spec_8_4_2_defers_to_a_conformant_upstream_even_with_an_adapter() {
    // The mapping exists to compensate for the upstream's vocabulary. If the
    // upstream speaks this protocol itself, the mapping is unnecessary, and
    // the upstream side's declaration must not hide the upstream's own declaration.
    let server =
        ServerUnderTest::lsp_det_with_fake_upstream_flags(&["--declare-server-state-provider"]);
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"freshness": {"fileChanges": ["Changed"]}})
    );
    assert_eq!(
        client.server_state().message.as_deref(),
        Some("answered by upstream")
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// The boundary around the handshake
//
// The mapping is chosen from serverInfo in InitializeResult, so an upstream
// state notification before that cannot be interpreted. LSP restricts
// server-initiated notifications to after InitializeResult (only
// showMessage / logMessage / telemetry are allowed before it), so this is
// within the spec's scope. What is constrained here is how initialize
// failure and retry are handled.
// ---------------------------------------------------------------------------

#[test]
fn an_initialize_error_does_not_end_the_handshake() {
    // An error response to initialize is not completion of the handshake.
    // The client can retry, and the second InitializeResult is the real handshake.
    let server = ServerUnderTest::lsp_det_with_fake_upstream_flags(&["--fail-first-initialize"]);
    let mut client = ConformanceClient::start(&server);

    let first = client.initialize_raw(true);
    assert!(
        first.get("error").is_some(),
        "the fake upstream should fail the first attempt"
    );

    let second = client.initialize(true);
    assert!(
        !second["result"]["capabilities"]["experimental"]["serverStateProvider"].is_null(),
        "the retried initialize has no serverStateProvider: {second}"
    );
    client.shutdown();
}

#[test]
fn death_during_a_retried_initialize_is_still_closed_with_an_error() {
    // The first attempt errors; the second disappears without answering.
    // The second must not be left hanging either (spec 8.2 item 7).
    let server = ServerUnderTest::lsp_det_with_fake_upstream_flags(&[
        "--fail-first-initialize",
        "--exit-before-initialize-result",
    ]);
    let mut client = ConformanceClient::start(&server);
    let first = client.initialize_raw(true);
    assert!(first.get("error").is_some());

    let second = client.initialize_raw(true);
    assert!(
        second.get("error").is_some(),
        "the upstream disappeared during the retry, but no error was returned either: {second}"
    );
}

#[test]
fn an_answered_initialize_is_not_answered_again_when_the_upstream_dies() {
    // Once an error response to initialize has been returned, that id is no
    // longer hanging. Even if the upstream disappears afterward, the same id
    // must not be answered twice (JSON-RPC is one request, one response).
    let server = ServerUnderTest::lsp_det_with_fake_upstream_flags(&[
        "--fail-first-initialize",
        "--exit-after-initialize-error",
    ]);
    let mut client = ConformanceClient::start(&server);
    let first = client.initialize_raw(true);
    assert!(
        first.get("error").is_some(),
        "the fake upstream should fail the first attempt"
    );
    assert!(
        client.expect_no_response_until_closed(),
        "answered an already-answered initialize a second time when the upstream disappeared"
    );
}

#[test]
fn an_upstream_that_dies_before_answering_initialize_does_not_hang_the_client() {
    // Crash at startup. Spec 8.2 item 7: answer any unanswered request with
    // an error before closing the connection.
    let server =
        ServerUnderTest::lsp_det_with_fake_upstream_flags(&["--exit-before-initialize-result"]);
    let mut client = ConformanceClient::start(&server);

    // Close the hanging initialize with an error. If it went silent and only
    // returned EOF, the client would wait for the response forever.
    let response = client.initialize_raw(true);
    assert!(
        response.get("error").is_some(),
        "the upstream disappeared without answering initialize, but no error was returned either: {response}"
    );
}

// ---------------------------------------------------------------------------
// Real server integration (local only. Not part of CI — v0.1-design.md chapter 6)
// ---------------------------------------------------------------------------

/// Negative control. Plain rust-analyzer does not implement this protocol,
/// so the suite must judge it non-conforming. A suite that always passes
/// measures nothing, so this confirms that "what should fail, fails".
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn a_server_without_the_extension_is_detected_as_non_conforming() {
    let server = ServerUnderTest {
        program: "rust-analyzer".into(),
        args: vec![],
        root: support::repo_root(),
    };
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);

    assert!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"].is_null(),
        "plain rust-analyzer declares serverStateProvider. Either the upstream implemented it, \
         or the wrong subject is under test: {result}"
    );
}

/// 7.2 coverage. A requirement that only makes sense against a real server.
///
/// Once it has declared `ready`, a cross-workspace query must not be
/// incomplete (missing a cross-file usage site). An empty response from an
/// incomplete index is exactly the "silent lie" this project sets out to eliminate.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn spec_7_2_coverage_through_lsp_det_with_real_rust_analyzer() {
    let project = support::TempCargoProject::with_cross_file_reference("coverage");
    let a = project.file("a.rs");
    let b = project.file("b.rs");

    let mut client = ConformanceClient::start(&real_rust_analyzer(&project));
    client.initialize(true);
    client.wait_until_ready();
    client.did_open(&a, "rust");
    client.did_open(&b, "rust");

    // The known-complete result ahead of time: the call on line 4 of b.rs (line 3, 0-indexed).
    let found = references_in(&mut client, &a, &b);
    assert!(
        found
            .iter()
            .any(|location| location["range"]["start"]["line"] == 3),
        "missed the call in b.rs while declaring ready (completeness violation): {found:#?}"
    );

    client.shutdown();
}

/// 7.3 freshness. A requirement that only makes sense against a real server.
///
/// This measures **across files**, as spec 7.3 requires. File B is changed,
/// and we check whether a cross-workspace query rooted at a symbol in a
/// separate file A reflects B's change. Measuring within a single file would
/// pass on LSP's processing-order guarantee alone and verify nothing about
/// freshness (spec chapter 6 item 2).
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn spec_7_3_cross_file_freshness_through_lsp_det_with_real_rust_analyzer() {
    let project = support::TempCargoProject::with_cross_file_reference("freshness");
    let a = project.file("a.rs");
    let b = project.file("b.rs");

    let mut client = ConformanceClient::start(&real_rust_analyzer(&project));
    client.initialize(true);
    client.wait_until_ready();

    client.did_open(&a, "rust");
    client.did_open(&b, "rust");

    // Precondition: a reference from b.rs to the target in a.rs is visible.
    // The count depends on how it is counted (whether `use` counts as a
    // reference), so this only checks "is there a reference pointing at b.rs".
    let before = references_in(&mut client, &a, &b);
    assert!(
        !before.is_empty(),
        "the premise is broken: a reference from b.rs should be visible"
    );

    // Remove the call from B. This is the didChange that spec 6.2 covers.
    client.did_change(&b, 2, support::B_WITHOUT_CALL);

    // Query while still ready. If it is ready, the change must already be incorporated.
    let state = client.server_state();
    assert_eq!(
        state.readiness,
        Readiness::Ready,
        "if it is no longer ready at this point, that is a readiness problem, not a freshness one"
    );

    let after = references_in(&mut client, &a, &b);
    assert!(
        after.is_empty(),
        "returned a reference that should have been removed while declaring ready (freshness violation): {after:#?}"
    );

    client.shutdown();
}

// ---------------------------------------------------------------------------
// The typescript-language-server mapping (ADR 0010 decision B's M6, design 5.3)
//
// It returns no serverInfo and has no vocabulary for readiness either. The
// upstream side chooses the mapping from the specific `$/typescriptVersion`
// notification, and synthesizes readiness from the $/progress of
// "Initializing JS/TS language features…" and health from the
// "[tsserver] Exited. Code:" log.
// ---------------------------------------------------------------------------

fn tsls_client(declare_server_state: bool) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_typescript_language_server();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(declare_server_state);
    (client, result)
}

#[test]
fn typescript_language_server_is_identified_by_its_typescript_version_notification() {
    // What the server calls itself arrives after the initialize response.
    // The mapping is chosen at that point, and it is in the starting state.
    let (mut client, result) = tsls_client(true);
    assert!(
        result["result"]["serverInfo"].is_null(),
        "the premise is broken: the fake typescript-language-server should not return serverInfo: {result}"
    );
    // What the server calls itself arrives after the response, so wait for it
    // (it arrives before the first state query).
    client.make_upstream_begin_project_load("1");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    client.shutdown();
}

#[test]
fn typescript_language_server_is_identified_by_its_startup_log_before_initialize_completes() {
    // "Using Typescript version …" arrives before the initialize response.
    // The mapping is chosen by the time of the response, and a state query
    // right afterward returns initializing.
    let (mut client, result) = tsls_client(true);
    assert!(
        !result["result"]["capabilities"]["experimental"]["serverStateProvider"].is_null(),
        "the upstream side has no declaration: {result}"
    );
    assert_eq!(
        client.server_state().readiness,
        Readiness::Initializing,
        "the mapping was not chosen by the time of the response"
    );
    client.shutdown();
}

#[test]
fn typescript_language_server_is_identified_by_the_version_notification_alone() {
    // Even without a startup log (by configuration), the mapping can be chosen from $/typescriptVersion alone.
    let server = ServerUnderTest::lsp_det_with_upstream_flags(
        "none",
        &["--startup-typescript-version", "5.9.3"],
    );
    let mut client = ConformanceClient::start(&server);
    client.initialize(true);
    client.make_upstream_begin_project_load("1");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    client.shutdown();
}

#[test]
fn typescript_language_server_is_identified_even_when_the_client_declares_progress() {
    // When the client declares window.workDoneProgress, the upstream side
    // takes a path that skips peeking after the handshake. It must not skip
    // that while the mapping is still unchosen.
    let server = ServerUnderTest::lsp_det_with_fake_typescript_language_server();
    let mut client = ConformanceClient::start(&server);
    client.initialize_with_capabilities(json!({
        "window": {"workDoneProgress": true},
        "experimental": {"serverState": true}
    }));
    client.make_upstream_begin_project_load("1");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    client.shutdown();
}

#[test]
fn typescript_language_server_spec_8_2_5_declares_no_guarantees_for_an_untested_version() {
    let server = ServerUnderTest::lsp_det_with_upstream_flags(
        "none",
        &[
            "--startup-log",
            r#"Using Typescript version (fake) 5.9.2 from path "/fake/tsserver.js""#,
            "--startup-typescript-version",
            "5.9.2",
        ],
    );
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared an unmeasured guarantee: {result}"
    );
    client.shutdown();
}

#[test]
fn typescript_language_server_spec_5_declares_the_measured_guarantees_for_a_tested_version() {
    // 7.2 / 7.3 were run against the real server (TypeScript 5.9.3) and
    // passed. The version is read from the startup log, so it is available
    // in time for the initialize response.
    let (mut client, result) = tsls_client(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"coverage": {"scope": "workspace", "incomplete": {}}, "freshness": {"fileChanges": ["Changed"]}}),
        "did not declare a guarantee for a measured version: {result}"
    );
    client.shutdown();
}

#[test]
fn typescript_language_server_spec_7_1_2_becomes_ready_when_the_project_load_ends() {
    let (mut client, _) = tsls_client(true);
    client.make_upstream_begin_project_load("1");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    client.make_upstream_end_project_load("1");
    let state = client.await_state_changed();
    assert_eq!(state.readiness, Readiness::Ready);
    assert_eq!(
        state.health,
        Health::Ok,
        "health is ok when the load succeeds"
    );
    client.shutdown();
}

#[test]
fn typescript_language_server_spec_7_1_3_rearms_on_the_next_project_load() {
    // Re-emitted for a second project (or a tsconfig change).
    let (mut client, _) = tsls_client(true);
    client.make_upstream_begin_project_load("1");
    client.await_state_changed();
    client.make_upstream_end_project_load("1");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);

    client.make_upstream_begin_project_load("2");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    client.make_upstream_end_project_load("2");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn typescript_language_server_spec_7_1_4_reports_a_tsserver_exit_as_health_error() {
    let (mut client, _) = tsls_client(true);
    client.make_upstream_begin_project_load("1");
    client.await_state_changed();
    client.make_upstream_end_project_load("1");
    client.await_state_changed();

    client.make_upstream_emit_log_message(
        1,
        "[lspserver] [tsclient] [tsserver] Exited. Code: null. Signal: SIGKILL",
    );
    let state = client.await_state_changed();
    assert_eq!(state.health, Health::Error);
    assert!(
        state
            .message
            .as_deref()
            .is_some_and(|m| m.contains("Exited. Code: null")),
        "attaches the failure message: {state:?}"
    );
    client.shutdown();
}

#[test]
fn typescript_language_server_ignores_unrelated_progress_and_logs() {
    let (mut client, _) = tsls_client(true);
    client.make_upstream_emit_progress(json!({
        "token": "r",
        "value": {"kind": "begin", "title": "Finding references"}
    }));
    client.make_upstream_emit_progress(json!({"token": "r", "value": {"kind": "end"}}));
    client.make_upstream_emit_log_message(
        3,
        "Using Typescript version (user-setting) 5.9.3 from path x",
    );
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "state moved because of an unrelated message"
    );
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    client.shutdown();
}

// ---------------------------------------------------------------------------
// Real pyright / basedpyright integration (local only. Not part of CI — v0.1-design.md chapter 6)
//
// Requires pyright-langserver and basedpyright-langserver on PATH (flake.nix).
// ---------------------------------------------------------------------------

/// A subject that launches real pyright via lsp-det.
fn real_pyright(project: &support::TempPyProject, command: &str) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec!["--".to_string(), command.to_string(), "--stdio".to_string()],
        root: project.root.clone(),
    }
}

/// Returns only the references to `target` in `a.py` that point at `file`.
fn py_references_in(
    client: &mut ConformanceClient,
    a: &std::path::Path,
    file: &std::path::Path,
) -> Vec<Value> {
    let wanted = support::file_uri(file);
    client
        .references(a, 0, 4)
        .into_iter()
        .filter(|location| location["uri"] == Value::String(wanted.clone()))
        .collect()
}

/// Via pyright. The mapping is chosen from the startup log, and it becomes ready when enumeration completes (ADR 0011).
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn pyright_spec_7_1_through_lsp_det_with_real_pyright() {
    let project = support::TempPyProject::with_cross_file_reference("readiness");
    let mut client = ConformanceClient::start(&real_pyright(&project, "pyright-langserver"));
    let result = client.initialize_with_root(true, &project.root);
    assert!(
        result["result"]["serverInfo"].is_null(),
        "the premise is broken: pyright started returning serverInfo: {result}"
    );
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"coverage": {"scope": "workspace", "incomplete": {}}, "freshness": {"fileChanges": ["Changed"]}}),
        "no guarantee is declared for the measured version of real pyright: {result}"
    );
    let state = client.server_state();
    assert_ne!(
        state.readiness,
        Readiness::Unknown,
        "the mapping was not chosen from the startup log"
    );
    client.wait_until_ready();
    assert_eq!(
        client.server_state().health,
        Health::Unknown,
        "pyright has no signal for health"
    );
    client.shutdown();
}

/// Via basedpyright. The same mapping is chosen from serverInfo.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn pyright_spec_7_1_through_lsp_det_with_real_basedpyright() {
    let project = support::TempPyProject::with_cross_file_reference("based");
    let mut client = ConformanceClient::start(&real_pyright(&project, "basedpyright-langserver"));
    let result = client.initialize_with_root(true, &project.root);
    assert_eq!(
        result["result"]["serverInfo"]["name"],
        json!("basedpyright")
    );
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"coverage": {"scope": "workspace", "incomplete": {}}, "freshness": {"fileChanges": ["Changed"]}}),
        "no guarantee is declared for the measured version of real basedpyright: {result}"
    );
    assert_ne!(client.server_state().readiness, Readiness::Unknown);
    client.wait_until_ready();
    client.shutdown();
}

/// Measures 7.2 completeness against real pyright. The basis for the declaration.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn pyright_spec_7_2_coverage_through_lsp_det_with_real_pyright() {
    py_coverage_with("pyright-langserver", "coverage");
}

/// Measures 7.2 completeness against real basedpyright. The basis for the declaration.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn pyright_spec_7_2_coverage_through_lsp_det_with_real_basedpyright() {
    py_coverage_with("basedpyright-langserver", "based-coverage");
}

fn py_coverage_with(command: &str, tag: &str) {
    let project = support::TempPyProject::with_cross_file_reference(tag);
    let a = project.file("a.py");
    let b = project.file("b.py");

    let mut client = ConformanceClient::start(&real_pyright(&project, command));
    client.initialize_with_root(true, &project.root);
    client.wait_until_ready();
    client.did_open(&a, "python");

    let found = py_references_in(&mut client, &a, &b);
    assert!(
        found
            .iter()
            .any(|location| location["range"]["start"]["line"] == 4),
        "missed the call in b.py while declaring ready (completeness violation): {found:#?}"
    );
    client.shutdown();
}

/// Measures 7.3 freshness against real pyright (cross-file). The basis for the declaration.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn pyright_spec_7_3_cross_file_freshness_through_lsp_det_with_real_pyright() {
    py_freshness_with("pyright-langserver", "freshness");
}

/// Measures 7.3 freshness against real basedpyright. The basis for the declaration.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn pyright_spec_7_3_cross_file_freshness_through_lsp_det_with_real_basedpyright() {
    py_freshness_with("basedpyright-langserver", "based-freshness");
}

fn py_freshness_with(command: &str, tag: &str) {
    let project = support::TempPyProject::with_cross_file_reference(tag);
    let a = project.file("a.py");
    let b = project.file("b.py");

    let mut client = ConformanceClient::start(&real_pyright(&project, command));
    client.initialize_with_root(true, &project.root);
    client.wait_until_ready();
    client.did_open(&a, "python");
    client.did_open(&b, "python");

    let before = py_references_in(&mut client, &a, &b);
    assert!(
        !before.is_empty(),
        "the premise is broken: a reference from b.py should be visible"
    );

    client.did_change(&b, 2, support::PY_B_WITHOUT_CALL);
    assert_eq!(client.server_state().readiness, Readiness::Ready);

    let after = py_references_in(&mut client, &a, &b);
    assert!(
        after.is_empty(),
        "returned a reference that should have been removed while declaring ready (freshness violation): {after:#?}"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// Real typescript-language-server integration (local only. Not part of CI — v0.1-design.md chapter 6)
//
// Requires typescript-language-server and tsserver (typescript) on PATH (flake.nix).
// ---------------------------------------------------------------------------

/// A subject that launches real typescript-language-server via lsp-det.
fn real_tsls(project: &support::TempTsProject) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec![
            "--".to_string(),
            "typescript-language-server".to_string(),
            "--stdio".to_string(),
        ],
        root: project.root.clone(),
    }
}

/// Returns only the references to `target` in `a.ts` that point at `file`.
fn ts_references_in(
    client: &mut ConformanceClient,
    a: &std::path::Path,
    file: &std::path::Path,
) -> Vec<Value> {
    let wanted = support::file_uri(file);
    client
        .references(a, 0, 16)
        .into_iter()
        .filter(|location| location["uri"] == Value::String(wanted.clone()))
        .collect()
}

/// Opening a file loads the project, progressing initializing -> indexing -> ready.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn typescript_language_server_spec_7_1_through_lsp_det_with_real_server() {
    let project = support::TempTsProject::with_cross_file_reference("readiness");
    let mut client = ConformanceClient::start(&real_tsls(&project));
    let result = client.initialize_with_root(true, &project.root);
    assert!(
        result["result"]["serverInfo"].is_null(),
        "the premise is broken: typescript-language-server started returning serverInfo: {result}"
    );
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"coverage": {"scope": "workspace", "incomplete": {}}, "freshness": {"fileChanges": ["Changed"]}}),
        "no guarantee is declared for the measured version of the real server: {result}"
    );
    client.did_open(&project.file("a.ts"), "typescript");
    client.wait_until_ready();
    assert_eq!(client.server_state().health, Health::Ok);
    client.shutdown();
}

/// 7.2 completeness. The basis for the declaration.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn typescript_language_server_spec_7_2_coverage_through_lsp_det_with_real_server() {
    let project = support::TempTsProject::with_cross_file_reference("coverage");
    let a = project.file("a.ts");
    let b = project.file("b.ts");

    let mut client = ConformanceClient::start(&real_tsls(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&a, "typescript");
    client.wait_until_ready();

    let found = ts_references_in(&mut client, &a, &b);
    assert!(
        found
            .iter()
            .any(|location| location["range"]["start"]["line"] == 3),
        "missed the call in b.ts while declaring ready (completeness violation): {found:#?}"
    );
    client.shutdown();
}

/// 7.3 freshness (cross-file). The basis for the declaration. Spec chapter 10's expectation is "freshness not possible".
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn typescript_language_server_spec_7_3_cross_file_freshness_through_lsp_det_with_real_server() {
    let project = support::TempTsProject::with_cross_file_reference("freshness");
    let a = project.file("a.ts");
    let b = project.file("b.ts");

    let mut client = ConformanceClient::start(&real_tsls(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&a, "typescript");
    client.did_open(&b, "typescript");
    client.wait_until_ready();

    let before = ts_references_in(&mut client, &a, &b);
    assert!(
        !before.is_empty(),
        "the premise is broken: a reference from b.ts should be visible"
    );

    client.did_change(&b, 2, support::TS_B_WITHOUT_CALL);
    assert_eq!(client.server_state().readiness, Readiness::Ready);

    let after = ts_references_in(&mut client, &a, &b);
    assert!(
        after.is_empty(),
        "returned a reference that should have been removed while declaring ready (freshness violation): {after:#?}"
    );
    client.shutdown();
}

/// A tsconfig change re-triggers the load, going through indexing and back to ready.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn typescript_language_server_rearms_on_tsconfig_change_with_real_server() {
    let project = support::TempTsProject::with_cross_file_reference("tsconfig");
    let mut client = ConformanceClient::start(&real_tsls(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&project.file("a.ts"), "typescript");
    client.wait_until_ready();

    let tsconfig = project.file("tsconfig.json");
    std::fs::write(
        &tsconfig,
        support::TSCONFIG.replace("\"strict\":true", "\"strict\":false"),
    )
    .unwrap();
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": support::file_uri(&tsconfig), "type": 2}]}),
    );
    let observed = client
        .await_notification_within("experimental/serverStateChanged", Duration::from_secs(8))
        .expect(
            "readiness did not move on the tsconfig change (contradicts our reading of the source)",
        );
    assert_eq!(observed["readiness"], json!("indexing"));
    client.wait_until_ready();
    client.shutdown();
}

/// Keeps polling `experimental/serverState` and returns the state once it
/// satisfies the condition. For a client that does not receive
/// notifications (no declaration). The cap is a value generous enough to
/// cover a real server's startup and loading; it is not used to judge the subject.
fn poll_state_until(
    client: &mut ConformanceClient,
    done: impl Fn(&lsp_det::state::ServerState) -> bool,
) -> lsp_det::state::ServerState {
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let state = client.server_state();
        if done(&state) {
            return state;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the condition is still not satisfied after waiting 20 seconds: {state:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Killing tsserver leaves the language server alive returning empty
/// responses. The upstream side sets error from the "[tsserver] Exited. Code:"
/// log, and the downstream side rejects references.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn typescript_language_server_tsserver_crash_becomes_health_error_with_real_server() {
    // The downstream side stands in only for a client that does not declare
    // this protocol (ADR 0002 decision 3). Since it does not declare, no
    // notification arrives, so the state is tracked by querying.
    let project = support::TempTsProject::with_cross_file_reference("crash");
    let a = project.file("a.ts");
    let mut client = ConformanceClient::start(&real_tsls(&project));
    client.initialize_with_root(false, &project.root);
    client.did_open(&a, "typescript");
    let state = poll_state_until(&mut client, |s| s.readiness == Readiness::Ready);
    assert_eq!(state.health, Health::Ok, "the premise is broken: {state:?}");

    let killed = support::kill_descendants_matching(client.server_pid(), "tsserver");
    assert!(
        !killed.is_empty(),
        "the tsserver grandchild process was not found"
    );

    let state = poll_state_until(&mut client, |s| s.health == Health::Error);
    assert!(
        state
            .message
            .as_deref()
            .is_some_and(|m| m.contains("Exited. Code:")),
        "the reason for the crash is not attached: {state:?}"
    );

    let id = client.send_request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": support::file_uri(&a)},
            "position": {"line": 0, "character": 16},
            "context": {"includeDeclaration": false},
        }),
    );
    let response = client.await_response_to(id);
    assert!(
        !response["error"].is_null(),
        "forwarded a broken server's success-looking response unchanged: {response}"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// Real gopls integration (local only. Not part of CI — v0.1-design.md chapter 6)
//
// Requires gopls and the go toolchain on PATH.
// ---------------------------------------------------------------------------

/// A subject that launches real gopls via lsp-det.
fn real_gopls(project: &support::TempGoProject) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec!["--".to_string(), "gopls".to_string()],
        root: project.root.clone(),
    }
}

/// Via gopls. Observes the transition from initializing to ready (the mapping in design 5.2).
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn gopls_spec_7_1_through_lsp_det_with_real_gopls() {
    let project = support::TempGoProject::with_cross_file_reference("readiness");
    let mut client = ConformanceClient::start(&real_gopls(&project));
    let result = client.initialize_with_root(true, &project.root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"coverage": {"scope": "workspace", "incomplete": {"workspace/symbol": 100}}, "freshness": {"fileChanges": ["Created", "Changed", "Deleted"]}}),
        "no guarantee is declared for the measured version of real gopls: {result}"
    );
    assert_ne!(client.server_state().readiness, Readiness::Ready);
    client.wait_until_ready();
    assert_eq!(client.server_state().health, Health::Ok);
    client.shutdown();
}

/// Measures 7.2 completeness against real gopls. The basis for the declaration (design 5.2).
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn gopls_spec_7_2_coverage_through_lsp_det_with_real_gopls() {
    let project = support::TempGoProject::with_cross_file_reference("coverage");
    let a = project.file("a.go");
    let b = project.file("b.go");

    let mut client = ConformanceClient::start(&real_gopls(&project));
    client.initialize_with_root(true, &project.root);
    client.wait_until_ready();
    client.did_open(&a, "go");
    client.did_open(&b, "go");

    let found = go_references_in(&mut client, &a, &b);
    assert!(
        found
            .iter()
            .any(|location| location["range"]["start"]["line"] == 3),
        "missed the call in b.go while declaring ready (completeness violation): {found:#?}"
    );
    client.shutdown();
}

/// Measures 7.3 freshness against real gopls (cross-file). The basis for the declaration (design 5.2).
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn gopls_spec_7_3_cross_file_freshness_through_lsp_det_with_real_gopls() {
    let project = support::TempGoProject::with_cross_file_reference("freshness");
    let a = project.file("a.go");
    let b = project.file("b.go");

    let mut client = ConformanceClient::start(&real_gopls(&project));
    client.initialize_with_root(true, &project.root);
    client.wait_until_ready();
    client.did_open(&a, "go");
    client.did_open(&b, "go");

    let before = go_references_in(&mut client, &a, &b);
    assert!(
        !before.is_empty(),
        "the premise is broken: a reference from b.go should be visible"
    );

    client.did_change(&b, 2, support::GO_B_WITHOUT_CALL);
    assert_eq!(client.server_state().readiness, Readiness::Ready);

    let after = go_references_in(&mut client, &a, &b);
    assert!(
        after.is_empty(),
        "returned a reference that should have been removed while declaring ready (freshness violation): {after:#?}"
    );
    client.shutdown();
}

/// Whether a go.mod change re-emits "Setting up workspace" (measured for design 5.2).
///
/// In gopls's source, re-emission only happens on didChangeWorkspaceFolders,
/// not on a go.mod change. This confirms that reading against a real server.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn gopls_does_not_reemit_workspace_setup_on_go_mod_change() {
    let project = support::TempGoProject::with_cross_file_reference("gomod");
    let mut client = ConformanceClient::start(&real_gopls(&project));
    client.initialize_with_root(true, &project.root);
    client.wait_until_ready();

    // Change go.mod on disk (an edit by Claude Code is a disk write).
    let go_mod = project.file("go.mod");
    std::fs::write(&go_mod, "module fixture\n\ngo 1.21\n\n// touched\n").unwrap();
    client.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": support::file_uri(&go_mod), "type": 2}]}),
    );

    let observed =
        client.await_notification_within("experimental/serverStateChanged", Duration::from_secs(8));
    assert!(
        observed.is_none(),
        "readiness moved on the go.mod change (contradicts our reading of the source): {observed:?}"
    );
    assert_eq!(client.server_state().readiness, Readiness::Ready);
    client.shutdown();
}

/// Returns only the references to `Target` in `a.go` that point at `file`.
fn go_references_in(
    client: &mut ConformanceClient,
    a: &std::path::Path,
    file: &std::path::Path,
) -> Vec<Value> {
    let wanted = support::file_uri(file);
    client
        .references(a, 2, 5)
        .into_iter()
        .filter(|location| location["uri"] == Value::String(wanted.clone()))
        .collect()
}

/// A subject that launches real rust-analyzer via lsp-det.
fn real_rust_analyzer(project: &support::TempCargoProject) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec!["--".to_string(), "rust-analyzer".to_string()],
        root: project.root.clone(),
    }
}

/// Returns only the references to symbol `target` in `a` that point at `file`.
fn references_in(
    client: &mut ConformanceClient,
    a: &std::path::Path,
    file: &std::path::Path,
) -> Vec<Value> {
    let wanted = support::file_uri(file);
    client
        .references(a, 0, 7)
        .into_iter()
        .filter(|location| location["uri"] == Value::String(wanted.clone()))
        .collect()
}

/// Real rust-analyzer via lsp-det. Observes the transition from initializing to ready.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn spec_7_1_through_lsp_det_with_real_rust_analyzer() {
    let server = ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec!["--".to_string(), "rust-analyzer".to_string()],
        root: support::repo_root(),
    };
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert!(!result["result"]["capabilities"]["experimental"]["serverStateProvider"].is_null());

    // 7.1 item 1: it is not ready right after initialize.
    assert_ne!(client.server_state().readiness, Readiness::Ready);

    // The real server becomes ready at its own pace. Bails out if health breaks.
    client.wait_until_ready();
    client.shutdown();
}

// ---------------------------------------------------------------------------
// 7.3 item 2: freshness of an on-disk change (workspace/didChangeWatchedFiles) (ADR 0014)
//
// The files being changed (the caller and the new file) are not opened.
// Opening them would take the didOpen path and would not verify an on-disk
// change. The starting point is the file that defines the symbol.
// ---------------------------------------------------------------------------

/// Fake upstream: the notification starts reindexing (quiescent: false), and
/// once it finishes, the result incorporates the change. The upstream side
/// conveys indexing -> ready using its existing signal.
#[test]
fn spec_7_3_2_watched_file_changes_are_reflected_after_reindexing_with_a_fake_upstream() {
    let server = ServerUnderTest::lsp_det_with_fake_upstream_flags(&[
        "--references-depend-on-readiness",
        "--reindex-on-watched-files",
    ]);
    let mut client = ConformanceClient::start(&server);
    client.initialize(true);
    client.make_upstream_emit_status("ok", true);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    let root = support::repo_root();
    let before = client.references(&root.join("src/a.rs"), 0, 7).len();

    client.did_change_watched_files(&[(&root.join("src/c.rs"), 1)]);
    assert_eq!(
        client.await_state_changed().readiness,
        Readiness::Indexing,
        "reindexing started by the notification is not conveyed as indexing"
    );
    client.make_upstream_emit_status("ok", true);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);

    let after = client.references(&root.join("src/a.rs"), 0, 7).len();
    assert_eq!(
        after,
        before + 1,
        "the response after ready does not incorporate the notified change"
    );
    client.shutdown();
}

/// The common procedure across real servers. Opens the defining file `def`,
/// changes the caller `caller` on disk without opening it (Changed), creates
/// a new file `new_file` (Created), then deletes it (Deleted). `extra` is a
/// file sent as Changed at the same time (Rust's lib.rs).
#[allow(clippy::too_many_arguments)] // it is more readable to list the procedure's arguments as they are
fn watched_file_changes_are_reflected(
    client: &mut ConformanceClient,
    def: &std::path::Path,
    caller: &std::path::Path,
    two_calls: &str,
    new_file: &std::path::Path,
    new_file_text: &str,
    extra: Option<(&std::path::Path, &str)>,
    references_in: fn(&mut ConformanceClient, &std::path::Path, &std::path::Path) -> Vec<Value>,
    client_notifies: bool,
) {
    let before = references_in(client, def, caller);
    assert!(
        !before.is_empty(),
        "the premise is broken: a reference from the caller should be visible"
    );

    // Changed: add one more call.
    std::fs::write(caller, two_calls).unwrap();
    if client_notifies {
        client.did_change_watched_files(&[(caller, 2)]);
    }
    client.wait_until_ready();
    let after_change = references_in(client, def, caller);
    assert_eq!(
        after_change.len(),
        before.len() + 1,
        "did not return the call added on disk while declaring ready (freshness violation): {after_change:#?}"
    );

    // Created: also call from the new file.
    std::fs::write(new_file, new_file_text).unwrap();
    let mut changes = vec![(new_file, 1u8)];
    if let Some((path, text)) = extra {
        std::fs::write(path, text).unwrap();
        changes.push((path, 2));
    }
    if client_notifies {
        client.did_change_watched_files(&changes);
    }
    client.wait_until_ready();
    let in_new_file = references_in(client, def, new_file);
    assert!(
        !in_new_file.is_empty(),
        "did not return the reference from the new file while declaring ready (freshness violation)"
    );

    // Deleted: delete the new file (7.3 item 4). For Rust, lib.rs is also restored.
    std::fs::remove_file(new_file).unwrap();
    let mut changes = vec![(new_file, 3u8)];
    if let Some((path, _)) = extra {
        std::fs::write(path, "pub mod a;\npub mod b;\n").unwrap();
        changes.push((path, 2));
    }
    if client_notifies {
        client.did_change_watched_files(&changes);
    }
    client.wait_until_ready();
    let after_delete = references_in(client, def, new_file);
    assert!(
        after_delete.is_empty(),
        "returned a reference from the deleted file while declaring ready (freshness violation): {after_delete:#?}"
    );
}

#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn spec_7_3_2_watched_file_changes_through_lsp_det_with_real_rust_analyzer() {
    let project = support::TempCargoProject::with_cross_file_reference("watched");
    let mut client = ConformanceClient::start(&real_rust_analyzer(&project));
    client.initialize(true);
    client.wait_until_ready();
    client.did_open(&project.file("a.rs"), "rust");
    watched_file_changes_are_reflected(
        &mut client,
        &project.file("a.rs"),
        &project.file("b.rs"),
        support::B_WITH_TWO_CALLS,
        &project.file("c.rs"),
        support::C_RS_WITH_CALL,
        Some((&project.file("lib.rs"), support::LIB_RS_WITH_C)),
        references_in,
        true,
    );
    client.shutdown();
}

#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn gopls_spec_7_3_2_watched_file_changes_through_lsp_det_with_real_gopls() {
    let project = support::TempGoProject::with_cross_file_reference("watched");
    let mut client = ConformanceClient::start(&real_gopls(&project));
    client.initialize_with_root(true, &project.root);
    client.wait_until_ready();
    client.did_open(&project.file("a.go"), "go");
    watched_file_changes_are_reflected(
        &mut client,
        &project.file("a.go"),
        &project.file("b.go"),
        support::GO_B_WITH_TWO_CALLS,
        &project.file("c.go"),
        support::GO_C_WITH_CALL,
        None,
        go_references_in,
        true,
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// ADR 0014 addendum decision D: the rust-analyzer mapping predicts indexing
// from a Created / Deleted notification, and reverts on quiescent: true. It
// does not predict for Changed or for a file outside the watched set.
// ---------------------------------------------------------------------------

#[test]
fn rust_analyzer_mapping_predicts_indexing_from_a_created_notification() {
    let (mut client, _) = client(true);
    client.make_upstream_emit_status("ok", true);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);

    let root = support::repo_root();
    client.did_change_watched_files(&[(&root.join("src/c.rs"), 1)]);
    let predicted = client.await_state_changed();
    assert_eq!(
        predicted.readiness,
        Readiness::Indexing,
        "does not predict indexing from a Created notification"
    );
    assert_eq!(
        predicted.health,
        Health::Ok,
        "the prediction only moves readiness"
    );

    // Reverts on the upstream's completion signal.
    client.make_upstream_emit_status("ok", true);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn rust_analyzer_mapping_predicts_indexing_from_a_deleted_cargo_toml_notification() {
    let (mut client, _) = client(true);
    client.make_upstream_emit_status("ok", true);
    client.await_state_changed();

    let root = support::repo_root();
    client.did_change_watched_files(&[(&root.join("Cargo.toml"), 3)]);
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    client.shutdown();
}

#[test]
fn rust_analyzer_mapping_does_not_predict_for_changed_or_unwatched_files() {
    let (mut client, _) = client(true);
    client.make_upstream_emit_status("ok", true);
    client.await_state_changed();

    let root = support::repo_root();
    // Changed produces no signal (an in-flight request is simply refused with -32801).
    client.did_change_watched_files(&[(&root.join("src/b.rs"), 2)]);
    // Created for a file outside the watched set also produces no signal.
    client.did_change_watched_files(&[(&root.join("notes.txt"), 1)]);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "must not predict for a notification that produces no signal (would stay stuck in indexing)"
    );
    assert_eq!(client.server_state().readiness, Readiness::Ready);
    client.shutdown();
}

// ---------------------------------------------------------------------------
// ADR 0016: a declaration names what is missing. The cap is read from initializationOptions
// ---------------------------------------------------------------------------

#[test]
fn rust_analyzer_mapping_reads_the_workspace_symbol_limit_from_initialization_options() {
    let server = ServerUnderTest::lsp_det_with_fake_upstream();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize_with_initialization_options(
        true,
        json!({"workspace": {"symbol": {"search": {"limit": 1000}}}}),
    );
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"]["coverage"]["incomplete"],
        json!({"workspace/symbol": 1000}),
        "the cap the client raised is not reflected in the declaration: {result}"
    );
    client.shutdown();
}

/// 7.2 item 2: a method listed under incomplete returns up to its cap, and a
/// method not listed returns everything. Measured with a fixture that has 300 matching symbols.
fn workspace_symbol_count_through(client: &mut ConformanceClient) -> usize {
    let response = client.request("workspace/symbol", json!({"query": "wsymprobe"}));
    response["result"].as_array().map_or(0, |items| {
        items
            .iter()
            .filter(|item| {
                item["name"]
                    .as_str()
                    .is_some_and(|n| n.to_ascii_lowercase().contains("wsymprobe"))
            })
            .count()
    })
}

#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn spec_7_2_2_rust_analyzer_returns_the_declared_limit_for_workspace_symbol() {
    let project = support::TempCargoProject::with_many_symbols("limit", 300);
    let mut client = ConformanceClient::start(&real_rust_analyzer(&project));
    let result = client.initialize(true);
    client.wait_until_ready();
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"]["coverage"]["incomplete"]
            ["workspace/symbol"],
        json!(128)
    );
    assert_eq!(
        workspace_symbol_count_through(&mut client),
        128,
        "does not match the declared cap"
    );
    client.shutdown();
}

#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn spec_7_2_2_gopls_returns_the_declared_limit_for_workspace_symbol() {
    let project = support::TempGoProject::with_many_symbols("limit", 300);
    let mut client = ConformanceClient::start(&real_gopls(&project));
    let result = client.initialize_with_root(true, &project.root);
    client.wait_until_ready();
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"]["coverage"]["incomplete"]
            ["workspace/symbol"],
        json!(100)
    );
    assert_eq!(
        workspace_symbol_count_through(&mut client),
        100,
        "does not match the declared cap"
    );
    client.shutdown();
}

#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn spec_7_2_2_pyright_returns_every_workspace_symbol() {
    let project = support::TempPyProject::with_many_symbols("limit", 300);
    let mut client = ConformanceClient::start(&real_pyright(&project, "pyright-langserver"));
    client.initialize_with_root(true, &project.root);
    client.wait_until_ready();
    assert_eq!(
        workspace_symbol_count_through(&mut client),
        300,
        "capped even though it is not listed under incomplete"
    );
    client.shutdown();
}

#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn spec_7_2_2_typescript_language_server_returns_every_workspace_symbol() {
    let project = support::TempTsProject::with_many_symbols("limit", 300);
    let mut client = ConformanceClient::start(&real_tsls(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&project.file("s0.ts"), "typescript");
    client.wait_until_ready();
    assert_eq!(
        workspace_symbol_count_through(&mut client),
        300,
        "capped even though it is not listed under incomplete"
    );
    client.shutdown();
}

/// 7.3 item 2 only (for a mapping whose fileChanges is ["Changed"]): changes
/// the caller on disk and sends Changed. Created / Deleted are not tried since they are not declared.
fn changed_file_is_reflected(
    client: &mut ConformanceClient,
    def: &std::path::Path,
    caller: &std::path::Path,
    two_calls: &str,
    references_in: fn(&mut ConformanceClient, &std::path::Path, &std::path::Path) -> Vec<Value>,
    client_notifies: bool,
) {
    let before = references_in(client, def, caller);
    assert!(
        !before.is_empty(),
        "the premise is broken: a reference from the caller should be visible"
    );
    std::fs::write(caller, two_calls).unwrap();
    if client_notifies {
        client.did_change_watched_files(&[(caller, 2)]);
    }
    client.wait_until_ready();
    let after = references_in(client, def, caller);
    assert_eq!(
        after.len(),
        before.len() + 1,
        "did not return the call added on disk while declaring ready (freshness violation): {after:#?}"
    );
}

#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn pyright_spec_7_3_2_changed_file_through_lsp_det_with_real_pyright() {
    let project = support::TempPyProject::with_cross_file_reference("changed");
    let mut client = ConformanceClient::start(&real_pyright(&project, "pyright-langserver"));
    client.initialize_with_root(true, &project.root);
    client.wait_until_ready();
    client.did_open(&project.file("a.py"), "python");
    changed_file_is_reflected(
        &mut client,
        &project.file("a.py"),
        &project.file("b.py"),
        support::PY_B_WITH_TWO_CALLS,
        py_references_in,
        true,
    );
    client.shutdown();
}

#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn typescript_language_server_spec_7_3_2_changed_file_through_lsp_det_with_real_server() {
    let project = support::TempTsProject::with_cross_file_reference("changed");
    let mut client = ConformanceClient::start(&real_tsls(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&project.file("a.ts"), "typescript");
    client.wait_until_ready();
    changed_file_is_reflected(
        &mut client,
        &project.file("a.ts"),
        &project.file("b.ts"),
        support::TS_B_WITH_TWO_CALLS,
        ts_references_in,
        true,
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// ADR 0015 decision A: lsp-det sends didChangeWatchedFiles standing in for a
// client that neither declares nor sends it. Runs the same procedure as
// above, with the client sending no notification and the fixture placed under git.
// ---------------------------------------------------------------------------

#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn stand_in_spec_7_3_2_through_lsp_det_with_real_rust_analyzer() {
    let project = support::TempCargoProject::with_cross_file_reference("standin");
    support::git_init(&project.root);
    let mut client = ConformanceClient::start(&real_rust_analyzer(&project));
    client.initialize(true);
    client.wait_until_ready();
    client.did_open(&project.file("a.rs"), "rust");
    watched_file_changes_are_reflected(
        &mut client,
        &project.file("a.rs"),
        &project.file("b.rs"),
        support::B_WITH_TWO_CALLS,
        &project.file("c.rs"),
        support::C_RS_WITH_CALL,
        Some((&project.file("lib.rs"), support::LIB_RS_WITH_C)),
        references_in,
        false,
    );
    client.shutdown();
}

#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn stand_in_spec_7_3_2_through_lsp_det_with_real_gopls() {
    let project = support::TempGoProject::with_cross_file_reference("standin");
    support::git_init(&project.root);
    let mut client = ConformanceClient::start(&real_gopls(&project));
    client.initialize_with_root(true, &project.root);
    client.wait_until_ready();
    client.did_open(&project.file("a.go"), "go");
    watched_file_changes_are_reflected(
        &mut client,
        &project.file("a.go"),
        &project.file("b.go"),
        support::GO_B_WITH_TWO_CALLS,
        &project.file("c.go"),
        support::GO_C_WITH_CALL,
        None,
        go_references_in,
        false,
    );
    client.shutdown();
}

#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn stand_in_spec_7_3_2_changed_file_through_lsp_det_with_real_pyright() {
    let project = support::TempPyProject::with_cross_file_reference("standin");
    support::git_init(&project.root);
    let mut client = ConformanceClient::start(&real_pyright(&project, "pyright-langserver"));
    client.initialize_with_root(true, &project.root);
    client.wait_until_ready();
    client.did_open(&project.file("a.py"), "python");
    changed_file_is_reflected(
        &mut client,
        &project.file("a.py"),
        &project.file("b.py"),
        support::PY_B_WITH_TWO_CALLS,
        py_references_in,
        false,
    );
    client.shutdown();
}

#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn stand_in_spec_7_3_2_changed_file_through_lsp_det_with_real_server() {
    let project = support::TempTsProject::with_cross_file_reference("standin");
    support::git_init(&project.root);
    let mut client = ConformanceClient::start(&real_tsls(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&project.file("a.ts"), "typescript");
    client.wait_until_ready();
    changed_file_is_reflected(
        &mut client,
        &project.file("a.ts"),
        &project.file("b.ts"),
        support::TS_B_WITH_TWO_CALLS,
        ts_references_in,
        false,
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// Metals (M9, ADR 0019 decision F). The mapping in research/metals-readiness-measurement.md:
// readiness from the `$/progress` titles of the build import ("… bspConfig", "Importing build",
// "Indexing", "Compiling …"); the initial import completes only with an "Indexing" end (the
// gaps between tokens are not ready), after which a "Compiling" end is ready too; a watched
// change of a source predicts indexing until the next compile end, a created / deleted source
// or a build file until the next "Indexing" end; health from the `level` of `metals/status`
// with `statusType: "module"`.
// ---------------------------------------------------------------------------

fn metals_client(declare_server_state: bool) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_metals();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(declare_server_state);
    (client, result)
}

fn metals_progress(client: &mut ConformanceClient, token: &str, kind: &str, title: Option<&str>) {
    let mut value = json!({"kind": kind});
    if let Some(title) = title {
        value["title"] = json!(title);
    }
    client.make_upstream_emit_progress(json!({"token": token, "value": value}));
}

fn metals_module_status(client: &mut ConformanceClient, level: &str, text: &str, tooltip: &str) {
    client.make_upstream_emit_notification(
        "metals/status",
        json!({"text": text, "level": level, "show": true, "tooltip": tooltip, "statusType": "module"}),
    );
}

#[test]
fn metals_is_selected_and_moves_readiness_with_the_import_progress() {
    let (mut client, _) = metals_client(true);
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    metals_progress(&mut client, "i", "begin", Some("Importing build"));
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    metals_progress(&mut client, "i", "end", None);
    metals_progress(&mut client, "x", "begin", Some("Indexing"));
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "moved while still indexing"
    );
    metals_progress(&mut client, "x", "end", None);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn metals_does_not_claim_ready_in_the_gap_before_the_first_indexing_end() {
    // Measured: "scala-cli bspConfig" ends 10 seconds before "Importing build" begins, with no
    // token open in between (research/metals-readiness-measurement.md).
    let (mut client, _) = metals_client(true);
    metals_progress(&mut client, "b", "begin", Some("scala-cli bspConfig"));
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    metals_progress(&mut client, "b", "end", None);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "claimed ready in the gap between bspConfig and Importing build"
    );
    assert_eq!(client.server_state().readiness, Readiness::Indexing);
    client.shutdown();
}

#[test]
fn metals_completes_a_compile_only_after_the_initial_import() {
    // Measured: a changed source only recompiles ("Compiling …" with no import round after
    // it), so after the import a compile end is ready. Before the first "Indexing" end it is
    // not (the initial import is still under way).
    let (mut client, _) = metals_client(true);
    metals_progress(
        &mut client,
        "c0",
        "begin",
        Some("Compiling fixture_309b29d35b"),
    );
    client.await_state_changed();
    metals_progress(&mut client, "c0", "end", None);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "claimed ready on a compile before the initial import completed"
    );
    metals_progress(&mut client, "i", "begin", Some("Importing build"));
    metals_progress(&mut client, "i", "end", None);
    metals_progress(&mut client, "x", "begin", Some("Indexing"));
    metals_progress(&mut client, "x", "end", None);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    metals_progress(
        &mut client,
        "c1",
        "begin",
        Some("Compiling fixture_309b29d35b"),
    );
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    metals_progress(&mut client, "c1", "end", None);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn metals_ignores_presentation_compiler_progress_and_metals_status_bar_text() {
    let (mut client, _) = metals_client(true);
    metals_progress(&mut client, "x", "begin", Some("Indexing"));
    client.await_state_changed();
    metals_progress(&mut client, "x", "end", None);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    metals_progress(
        &mut client,
        "p",
        "begin",
        Some("Loading presentation compiler"),
    );
    metals_progress(&mut client, "p", "end", None);
    client.make_upstream_emit_notification(
        "metals/status",
        json!({"text": " Indexing complete!", "level": "info", "show": true, "statusType": "metals"}),
    );
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "moved because of the presentation compiler or the status bar text"
    );
    client.shutdown();
}

#[test]
fn metals_health_comes_from_the_level_of_the_module_status() {
    // Measured with a broken build definition: the import progress runs as usual, then
    // `metals/status {level: "error", text: "no target", tooltip: "No build target for file found."}`
    // and every references answer is empty.
    let (mut client, _) = metals_client(true);
    assert_eq!(client.server_state().health, Health::Unknown);
    metals_module_status(&mut client, "info", "importing...", "");
    assert_eq!(client.await_state_changed().health, Health::Ok);
    metals_module_status(
        &mut client,
        "error",
        "no target",
        "No build target for file found.",
    );
    let state = client.await_state_changed();
    assert_eq!(state.health, Health::Error);
    assert!(
        state
            .message
            .as_deref()
            .is_some_and(|m| m.contains("No build target")),
        "the reason is not attached: {state:?}"
    );
    metals_module_status(&mut client, "warn", "fixture", "1 warning");
    assert_eq!(client.await_state_changed().health, Health::Warning);
    client.shutdown();
}

#[test]
fn metals_predicts_indexing_from_a_watched_scala_file_change() {
    // The measured gap between the client's notification and the first progress begin is
    // 0.15-0.33 s with no token open (ADR 0014 addendum decision D applied to Metals).
    let (mut client, _) = metals_client(true);
    metals_progress(&mut client, "x", "begin", Some("Indexing"));
    client.await_state_changed();
    metals_progress(&mut client, "x", "end", None);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    let root = support::repo_root();
    // A changed source: the next compile end reverts it.
    client.did_change_watched_files(&[(&root.join("B.scala"), 2)]);
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    metals_progress(
        &mut client,
        "c",
        "begin",
        Some("Compiling fixture_309b29d35b"),
    );
    metals_progress(&mut client, "c", "end", None);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    // A created source changes the build: a compile does not revert it, the next Indexing
    // end does (measured: Compiling, then Importing build and Indexing).
    client.did_change_watched_files(&[(&root.join("C.scala"), 1)]);
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    metals_progress(
        &mut client,
        "c2",
        "begin",
        Some("Compiling fixture_309b29d35b"),
    );
    metals_progress(&mut client, "c2", "end", None);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "reverted a predicted build change on a compile end"
    );
    metals_progress(&mut client, "x2", "begin", Some("Indexing"));
    metals_progress(&mut client, "x2", "end", None);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    // A build file too; a file outside the watched set does not predict.
    client.did_change_watched_files(&[(&root.join("project.scala"), 2)]);
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    metals_progress(&mut client, "x3", "begin", Some("Indexing"));
    metals_progress(&mut client, "x3", "end", None);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.did_change_watched_files(&[(&root.join("README.md"), 2)]);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "predicted indexing from a file outside the watched set"
    );
    client.shutdown();
}

#[test]
fn metals_spec_8_2_5_declares_no_guarantees_for_an_untested_version() {
    let (mut client, result) = metals_client(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee for the fake Metals's untested version: {result}"
    );
    client.shutdown();
}

// --- real Metals (local only) ---------------------------------------------

fn real_metals(project: &support::TempScalaProject) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec!["--".to_string(), "metals".to_string()],
        root: project.root.clone(),
    }
}

/// Returns only the references to `target` in `A.scala` that point at `file`.
fn scala_references_in(
    client: &mut ConformanceClient,
    a: &std::path::Path,
    file: &std::path::Path,
) -> Vec<Value> {
    let wanted = support::file_uri(file);
    client
        .references(a, 1, 6)
        .into_iter()
        .filter(|location| location["uri"] == Value::String(wanted.clone()))
        .collect()
}

/// Via Metals. Observes the transition from initializing to ready.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn metals_spec_7_1_through_lsp_det_with_real_metals() {
    let project = support::TempScalaProject::with_cross_file_reference("readiness");
    let mut client = ConformanceClient::start(&real_metals(&project));
    let result = client.initialize_with_root(true, &project.root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"coverage": {"scope": "workspace", "incomplete": {}}, "freshness": {"fileChanges": []}}),
        "no guarantee is declared for the measured version of real Metals: {result}"
    );
    assert_ne!(client.server_state().readiness, Readiness::Ready);
    client.wait_until_ready();
    // The module status that carries health arrives on its own schedule: measured 7 ms
    // after the "Indexing" end (research/metals-readiness-measurement.md). An observer may
    // be ready with health still unknown (spec 8.2 item 2).
    let state = poll_state_until(&mut client, |s| s.health != Health::Unknown);
    assert_eq!(
        state.health,
        Health::Ok,
        "health after the module status: {state:?}"
    );
    client.shutdown();
}

/// Measures 7.2 coverage against real Metals.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn metals_spec_7_2_coverage_through_lsp_det_with_real_metals() {
    let project = support::TempScalaProject::with_cross_file_reference("coverage");
    let a = project.file("A.scala");
    let b = project.file("B.scala");
    let mut client = ConformanceClient::start(&real_metals(&project));
    client.initialize_with_root(true, &project.root);
    client.wait_until_ready();
    client.did_open(&a, "scala");
    let found = scala_references_in(&mut client, &a, &b);
    assert!(
        found
            .iter()
            .any(|location| location["range"]["start"]["line"] == 1),
        "missed the reference in B.scala while declaring ready (coverage violation): {found:#?}"
    );
    client.shutdown();
}

/// Measures 7.3 item 1 (didChange, cross-file) against real Metals.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn metals_spec_7_3_cross_file_freshness_through_lsp_det_with_real_metals() {
    let project = support::TempScalaProject::with_cross_file_reference("freshness");
    let a = project.file("A.scala");
    let b = project.file("B.scala");
    let mut client = ConformanceClient::start(&real_metals(&project));
    client.initialize_with_root(true, &project.root);
    client.wait_until_ready();
    client.did_open(&a, "scala");
    client.did_open(&b, "scala");
    let before = scala_references_in(&mut client, &a, &b);
    assert!(
        !before.is_empty(),
        "the premise is broken: a reference from B.scala should be visible"
    );
    client.did_change(&b, 2, support::SCALA_B_WITHOUT_CALL);
    assert_eq!(client.server_state().readiness, Readiness::Ready);
    let after = scala_references_in(&mut client, &a, &b);
    assert!(
        after.is_empty(),
        "returned a reference that should have been removed while declaring ready (freshness violation): {after:#?}"
    );
    client.shutdown();
}

/// Measures the `workspace/symbol` cap of real Metals (the basis for `coverage.incomplete`).
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn spec_7_2_2_metals_workspace_symbol_count() {
    let project = support::TempScalaProject::with_many_symbols("limit", 300);
    let mut client = ConformanceClient::start(&real_metals(&project));
    client.initialize_with_root(true, &project.root);
    client.wait_until_ready();
    assert_eq!(
        workspace_symbol_count_through(&mut client),
        300,
        "capped even though it is not listed under incomplete"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// Expert (Elixir. M10, ADR 0019 decision F). The mapping in research/expert-readiness-measurement.md:
// readiness from the `$/progress` titles of the engine start and the build ("… Starting engine
// node", "… Preparing engine", "Building …", "Indexing source code"); ready only when no token is
// open AND the last token that ended was an index phase ("Indexing source code", or "Loading
// search index" on a warm start; the 1-second gap between the engine start and the build is
// not ready); no prediction (watched-file changes are not incorporated by Expert); no health
// signal.
// ---------------------------------------------------------------------------

fn expert_client(declare_server_state: bool) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_expert();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(declare_server_state);
    (client, result)
}

fn expert_progress(client: &mut ConformanceClient, token: i64, kind: &str, title: Option<&str>) {
    let mut value = json!({"kind": kind});
    if let Some(title) = title {
        value["title"] = json!(title);
    }
    client.make_upstream_emit_progress(json!({"token": token, "value": value}));
}

#[test]
fn expert_is_selected_and_moves_readiness_with_the_engine_start_and_the_build() {
    let (mut client, _) = expert_client(true);
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    expert_progress(
        &mut client,
        1,
        "begin",
        Some("[fixture] Starting engine node"),
    );
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    expert_progress(&mut client, 2, "begin", Some("[fixture] Preparing engine"));
    expert_progress(&mut client, 2, "end", None);
    expert_progress(&mut client, 1, "end", None);
    // The measured gap: engine up, build not started, no token open.
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "claimed ready in the gap between the engine start and the build"
    );
    expert_progress(&mut client, 3, "begin", Some("Building fixture"));
    expert_progress(&mut client, 3, "end", None);
    expert_progress(&mut client, 4, "begin", Some("Indexing source code"));
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "moved while still building"
    );
    expert_progress(&mut client, 4, "end", None);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn expert_ignores_request_processing_progress() {
    let (mut client, _) = expert_client(true);
    expert_progress(&mut client, 4, "begin", Some("Indexing source code"));
    client.await_state_changed();
    expert_progress(&mut client, 4, "end", None);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    expert_progress(
        &mut client,
        9,
        "begin",
        Some("Finding Completion Candidates"),
    );
    expert_progress(&mut client, 9, "end", None);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "moved because of request-processing progress"
    );
    // A persisted index loaded on a warm start completes a round like a fresh one.
    expert_progress(&mut client, 10, "begin", Some("Building fixture"));
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    expert_progress(&mut client, 10, "end", None);
    expert_progress(&mut client, 11, "begin", Some("Loading search index"));
    expert_progress(&mut client, 11, "end", None);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn expert_reindexes_on_a_new_build_and_does_not_predict_from_watched_files() {
    let (mut client, _) = expert_client(true);
    expert_progress(&mut client, 4, "begin", Some("Indexing source code"));
    client.await_state_changed();
    expert_progress(&mut client, 4, "end", None);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    // Expert does not incorporate watched-file changes, so there is no completion signal to
    // predict against (ADR 0014 addendum decision D).
    let root = support::repo_root();
    client.did_change_watched_files(&[(&root.join("lib/c.ex"), 1)]);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "predicted indexing although Expert does not react to watched files"
    );
    expert_progress(&mut client, 5, "begin", Some("Building fixture"));
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    expert_progress(&mut client, 5, "end", None);
    expert_progress(&mut client, 6, "begin", Some("Indexing source code"));
    expert_progress(&mut client, 6, "end", None);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    assert_eq!(
        client.server_state().health,
        Health::Unknown,
        "Expert has no health signal"
    );
    client.shutdown();
}

#[test]
fn expert_spec_8_2_5_declares_no_guarantees_for_an_untested_version() {
    let (mut client, result) = expert_client(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee for the fake Expert's untested version: {result}"
    );
    client.shutdown();
}

// --- real Expert (local only) ---------------------------------------------

fn real_expert(project: &support::TempMixProject) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec![
            "--".to_string(),
            "expert".to_string(),
            "--stdio".to_string(),
        ],
        root: project.root.clone(),
    }
}

/// Via Expert. Observes the transition from initializing to ready.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn expert_spec_7_1_through_lsp_det_with_real_expert() {
    let project = support::TempMixProject::with_cross_file_reference("readiness");
    let mut client = ConformanceClient::start(&real_expert(&project));
    let result = client.initialize_with_root(true, &project.root);
    // No guarantee for Expert: after "Loading search index" a fresh project may still be
    // about to rebuild the index, with empty answers in between and no signal
    // (research/expert-readiness-measurement.md). 7.2 / 7.3 are therefore not run.
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee Expert's vocabulary cannot keep: {result}"
    );
    assert_ne!(client.server_state().readiness, Readiness::Ready);
    client.wait_until_ready();
    assert_eq!(
        client.server_state().health,
        Health::Unknown,
        "Expert has no health signal"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// Nextflow's language server (M12, ADR 0019 decision F). The mapping in
// research/nextflow-readiness-measurement.md: the server returns no `serverInfo` and is known
// by `executeCommandProvider.commands` (`nextflow.server.*`); "Initializing" is a reset, not the
// scan, and the scan's completion is visible only as `publishDiagnostics` for every `*.nf`
// under the workspace folders (minus the configured excludes), so the observer walks the
// folders itself; a `didOpen` / `didChange` / `didClose` predicts `indexing` until the
// diagnostics of that document; watched-file changes are not incorporated; no health signal;
// no guarantee (the version is not observable).
// ---------------------------------------------------------------------------

fn nextflow_client(root: &std::path::Path) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_nextflow();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize_with_root(true, root);
    (client, result)
}

/// The settings Serena sends: a value that differs from the server's defaults, which is what
/// makes the real server initialize its services.
fn nextflow_configure(client: &mut ConformanceClient, exclude: &[&str]) {
    client.notify(
        "workspace/didChangeConfiguration",
        json!({"settings": {"nextflow": {
            "errorReportingMode": "errors",
            "files": {"exclude": exclude},
        }}}),
    );
}

fn nextflow_initializing(client: &mut ConformanceClient, kind: &str) {
    let value = if kind == "begin" {
        json!({"kind": "begin", "title": "Initializing", "message": "Initializing workspace..."})
    } else {
        json!({"kind": kind})
    };
    client.make_upstream_emit_progress(json!({"token": "initialize", "value": value}));
}

fn nextflow_diagnostics(client: &mut ConformanceClient, path: &std::path::Path) {
    client.make_upstream_emit_notification(
        "textDocument/publishDiagnostics",
        json!({"uri": support::file_uri(path), "diagnostics": []}),
    );
}

/// A barrier: a round trip through the fake upstream. Everything the fake was told to emit
/// before it has been read by lsp-det by the time the answer comes back, so a client
/// notification sent after this is observed after those emissions.
fn nextflow_sync(client: &mut ConformanceClient) {
    let _ = client.upstream_methods_seen();
}

/// Configures, runs the "Initializing" token, and diagnoses both scripts: the fake's way to
/// ready.
fn nextflow_reach_ready(client: &mut ConformanceClient, project: &support::TempNextflowProject) {
    nextflow_configure(client, &["work", ".nextflow"]);
    nextflow_initializing(client, "begin");
    nextflow_initializing(client, "end");
    nextflow_diagnostics(client, &project.file("modules/greet.nf"));
    nextflow_diagnostics(client, &project.file("main.nf"));
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
}

#[test]
fn nextflow_is_selected_by_its_commands_and_stays_initializing_until_every_script_is_diagnosed() {
    let project = support::TempNextflowProject::with_cross_file_reference("select");
    let (mut client, _) = nextflow_client(&project.root);
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    nextflow_configure(&mut client, &["work", ".nextflow"]);
    nextflow_initializing(&mut client, "begin");
    // Inside the token the real server clears the diagnostics of the files it had cached.
    // That is not a parse.
    nextflow_diagnostics(&mut client, &project.file("main.nf"));
    nextflow_initializing(&mut client, "end");
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "moved at the Initializing end although nothing has been scanned"
    );
    nextflow_diagnostics(&mut client, &project.file("modules/greet.nf"));
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "claimed ready before every script under the workspace folder was diagnosed"
    );
    nextflow_diagnostics(&mut client, &project.file("main.nf"));
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn nextflow_predicts_indexing_from_a_document_notification_until_its_diagnostics() {
    let project = support::TempNextflowProject::with_cross_file_reference("predict");
    let main = project.file("main.nf");
    let (mut client, _) = nextflow_client(&project.root);
    nextflow_reach_ready(&mut client, &project);
    client.did_open(&main, "nextflow");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    nextflow_diagnostics(&mut client, &main);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.did_change(&main, 2, support::NF_MAIN_WITHOUT_CALL);
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    nextflow_diagnostics(&mut client, &main);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.did_close(&main);
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    nextflow_diagnostics(&mut client, &main);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    // A document outside every workspace folder falls to the server's default service, which
    // is never initialized and never parses it: nothing to predict.
    let outside = std::env::temp_dir().join(format!(
        "lsp-det-conformance-nextflow-outside-{}.nf",
        std::process::id()
    ));
    std::fs::write(&outside, support::NF_GREET).unwrap();
    client.did_open(&outside, "nextflow");
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "predicted a parse of a document outside the workspace folders"
    );
    let _ = std::fs::remove_file(&outside);
    client.shutdown();
}

#[test]
fn nextflow_does_not_predict_from_watched_files_and_has_no_health_signal() {
    let project = support::TempNextflowProject::with_cross_file_reference("watched");
    let main = project.file("main.nf");
    let greet = project.file("modules/greet.nf");
    let (mut client, _) = nextflow_client(&project.root);
    nextflow_configure(&mut client, &["work", ".nextflow"]);
    nextflow_initializing(&mut client, "begin");
    nextflow_initializing(&mut client, "end");
    nextflow_diagnostics(&mut client, &main);
    nextflow_sync(&mut client);
    // A script deleted after the walk is never diagnosed by the scan: it leaves the set.
    client.did_change_watched_files(&[(&greet, 3)]);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    // Created / Changed are not incorporated by the server (measured), so there is no
    // completion signal to predict against (ADR 0014 addendum decision D).
    client.did_change_watched_files(&[(&project.file("c.nf"), 1), (&main, 2)]);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "predicted indexing although the server ignores watched files"
    );
    assert_eq!(
        client.server_state().health,
        Health::Unknown,
        "Nextflow's language server has no health signal"
    );
    client.shutdown();
}

#[test]
fn nextflow_spec_8_2_5_declares_no_guarantees() {
    let project = support::TempNextflowProject::with_cross_file_reference("declare");
    let (mut client, result) = nextflow_client(&project.root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee for a server whose version is not observable: {result}"
    );
    client.shutdown();
}

#[test]
fn nextflow_is_ready_at_the_initializing_end_when_there_is_nothing_to_scan() {
    let project = support::TempNextflowProject::without_scripts("empty");
    let (mut client, _) = nextflow_client(&project.root);
    nextflow_configure(&mut client, &["work", ".nextflow"]);
    nextflow_initializing(&mut client, "begin");
    nextflow_initializing(&mut client, "end");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

/// The server's exclude rule matches `/`-separated paths only, so on Windows nothing is
/// excluded and the mapping mirrors that (`work/ab/stale.nf` stays in the scan set).
#[test]
fn nextflow_honours_the_exclude_patterns_of_the_configuration() {
    let project = support::TempNextflowProject::with_cross_file_reference("exclude");
    std::fs::create_dir_all(project.file("work/ab")).unwrap();
    std::fs::write(project.file("work/ab/stale.nf"), support::NF_GREET).unwrap();
    let (mut client, _) = nextflow_client(&project.root);
    // With `work` excluded the server never diagnoses `work/ab/stale.nf`.
    if cfg!(windows) {
        nextflow_configure(&mut client, &["work", ".nextflow"]);
        nextflow_initializing(&mut client, "begin");
        nextflow_initializing(&mut client, "end");
        nextflow_diagnostics(&mut client, &project.file("modules/greet.nf"));
        nextflow_diagnostics(&mut client, &project.file("main.nf"));
        nextflow_diagnostics(&mut client, &project.file("work/ab/stale.nf"));
        assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    } else {
        nextflow_reach_ready(&mut client, &project);
    }
    // Without the exclude it is part of the scan.
    nextflow_configure(&mut client, &[]);
    nextflow_initializing(&mut client, "begin");
    assert_eq!(
        client.await_state_changed().readiness,
        Readiness::Initializing
    );
    nextflow_initializing(&mut client, "end");
    nextflow_diagnostics(&mut client, &project.file("modules/greet.nf"));
    nextflow_diagnostics(&mut client, &project.file("main.nf"));
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "claimed ready without the script under work/ that the new configuration includes"
    );
    nextflow_diagnostics(&mut client, &project.file("work/ab/stale.nf"));
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

// --- real Nextflow language server (local only) ------------------------------

fn real_nextflow(project: &support::TempNextflowProject) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec!["--".to_string(), "nextflow-language-server".to_string()],
        root: project.root.clone(),
    }
}

/// The references to `GREET` (declared in `modules/greet.nf`) that point at `file`.
fn nextflow_references_in(
    client: &mut ConformanceClient,
    greet: &std::path::Path,
    file: &std::path::Path,
) -> Vec<Value> {
    let wanted = support::file_uri(file);
    let (line, character) = support::NF_GREET_DECLARATION;
    client
        .references(greet, line, character)
        .into_iter()
        .filter(|location| location["uri"] == Value::String(wanted.clone()))
        .collect()
}

/// Via the real server. Observes the transition from initializing to ready. The scan needs a
/// trigger after the configuration (a `didOpen`; measured).
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn nextflow_spec_7_1_through_lsp_det_with_real_nextflow() {
    let project = support::TempNextflowProject::with_cross_file_reference("readiness");
    let mut client = ConformanceClient::start(&real_nextflow(&project));
    let result = client.initialize_with_root(true, &project.root);
    // No guarantee: the version is not observable (research/nextflow-readiness-measurement.md).
    // 7.2 / 7.3 are still measured below, as what the mapping would keep.
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee for a server whose version is not observable: {result}"
    );
    assert_ne!(client.server_state().readiness, Readiness::Ready);
    nextflow_configure(&mut client, &["work", ".nextflow"]);
    client.did_open(&project.file("modules/greet.nf"), "nextflow");
    client.wait_until_ready();
    assert_eq!(
        client.server_state().health,
        Health::Unknown,
        "Nextflow's language server has no health signal"
    );
    client.shutdown();
}

/// Without a configuration that differs from the server's defaults, the real server never
/// initializes its services and answers everything with empty results (measured). The
/// mapping stays initializing rather than letting that through.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn nextflow_stays_initializing_without_a_configuration_through_real_nextflow() {
    let project = support::TempNextflowProject::with_cross_file_reference("unconfigured");
    let mut client = ConformanceClient::start(&real_nextflow(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&project.file("modules/greet.nf"), "nextflow");
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", Duration::from_secs(4)),
        "moved without a configuration"
    );
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    client.shutdown();
}

/// Measures 7.2 coverage against the real server: references across 200 scripts, complete
/// at the first answer after ready.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn nextflow_spec_7_2_coverage_through_lsp_det_with_real_nextflow() {
    let project = support::TempNextflowProject::with_many_calls("coverage", 200);
    let greet = project.file("modules/greet.nf");
    let mut client = ConformanceClient::start(&real_nextflow(&project));
    client.initialize_with_root(true, &project.root);
    nextflow_configure(&mut client, &["work", ".nextflow"]);
    client.did_open(&greet, "nextflow");
    client.wait_until_ready();
    let (line, character) = support::NF_GREET_DECLARATION;
    let found = client.references(&greet, line, character);
    // Each script contributes the include and the call; main.nf the same.
    assert_eq!(
        found.len(),
        2 * 200 + 2,
        "missed references while declaring ready (coverage violation)"
    );
    client.shutdown();
}

/// Measures 7.3 item 1 (didChange of an open document, cross-file) against the real server.
/// `references` does not synchronize with the debounced update (measured 1 second stale), so
/// only the hold makes it fresh.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn nextflow_spec_7_3_cross_file_freshness_through_lsp_det_with_real_nextflow() {
    let project = support::TempNextflowProject::with_cross_file_reference("freshness");
    let greet = project.file("modules/greet.nf");
    let main = project.file("main.nf");
    let mut client = ConformanceClient::start(&real_nextflow(&project));
    client.initialize_with_root(true, &project.root);
    nextflow_configure(&mut client, &["work", ".nextflow"]);
    client.did_open(&greet, "nextflow");
    client.did_open(&main, "nextflow");
    client.wait_until_ready();
    let before = nextflow_references_in(&mut client, &greet, &main);
    assert!(
        before
            .iter()
            .any(|location| location["range"]["start"]["line"] == support::NF_MAIN_CALL_LINE),
        "the premise is broken: the call in main.nf should be visible: {before:#?}"
    );
    client.did_change(&main, 2, support::NF_MAIN_WITHOUT_CALL);
    // The mapping predicts `indexing` until the diagnostics of main.nf. This client declares
    // `experimental.serverState`, so nothing is held on its behalf: it waits itself (spec
    // chapter 9), as it did before the first query.
    assert_eq!(client.server_state().readiness, Readiness::Indexing);
    client.wait_until_ready();
    let after = nextflow_references_in(&mut client, &greet, &main);
    assert!(
        after
            .iter()
            .all(|location| location["range"]["start"]["line"] != support::NF_MAIN_CALL_LINE),
        "returned the removed call while declaring ready (freshness violation): {after:#?}"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// haskell-language-server (M15, ADR 0019 decision F). The mapping in
// research/haskell-language-server-readiness-measurement.md: the server returns no `serverInfo`
// and is known by its pid-prefixed `executeCommandProvider.commands` (`<pid>:ghcide-…`); its
// `$/progress` tokens are suppressed by a 1-second delay in the lsp library and cover only
// slices of the indexing, and `references` answers partial results that grow while the index
// is written, so readiness is `unknown` (spec 8.2 item 3); a cradle failure shows up as a
// diagnostic with `source: "cradle"`, which is the health signal; no guarantee.
// ---------------------------------------------------------------------------

fn hls_client(root: &std::path::Path) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_haskell_language_server();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize_with_root(true, root);
    (client, result)
}

fn hls_diagnostics(client: &mut ConformanceClient, path: &std::path::Path, diagnostics: Value) {
    client.make_upstream_emit_notification(
        "textDocument/publishDiagnostics",
        json!({"uri": support::file_uri(path), "diagnostics": diagnostics}),
    );
}

#[test]
fn hls_is_selected_by_its_pid_prefixed_commands_and_readiness_stays_unknown() {
    let project = support::TempCabalProject::with_cross_file_reference("select");
    let (mut client, result) = hls_client(&project.root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee for a server whose readiness cannot be observed: {result}"
    );
    // Spec 8.4 item 1: unknown on both axes right after initialize.
    let state = client.server_state();
    assert_eq!(state.readiness, Readiness::Unknown);
    assert_eq!(state.health, Health::Unknown);
    // The time-gated tokens say nothing about the index: they move nothing.
    client.make_upstream_emit_progress(
        json!({"token": 1, "value": {"kind": "begin", "title": "Indexing", "percentage": 0}}),
    );
    client.make_upstream_emit_progress(
        json!({"token": 2, "value": {"kind": "begin", "title": "Processing", "message": "1/2"}}),
    );
    client.make_upstream_emit_progress(json!({"token": 1, "value": {"kind": "end"}}));
    client.make_upstream_emit_progress(json!({"token": 2, "value": {"kind": "end"}}));
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "read readiness out of tokens the server suppresses by time"
    );
    assert_eq!(client.server_state().readiness, Readiness::Unknown);
    client.shutdown();
}

#[test]
fn hls_maps_a_cradle_failure_diagnostic_to_error_health() {
    let project = support::TempCabalProject::with_cross_file_reference("cradle");
    let a = project.file("src/A.hs");
    let (mut client, _) = hls_client(&project.root);
    let range = json!({"start": {"line": 0, "character": 0}, "end": {"line": 1, "character": 0}});
    hls_diagnostics(
        &mut client,
        &a,
        json!([{"source": "cradle", "severity": 1, "range": range,
                "message": "Failed to run cabal v2-repl 'lib:doesnotexist' in directory /p\nConsult the logs"}]),
    );
    let state = client.await_state_changed();
    assert_eq!(state.health, Health::Error);
    assert_eq!(
        state.message.as_deref(),
        Some("Failed to run cabal v2-repl 'lib:doesnotexist' in directory /p")
    );
    assert_eq!(state.readiness, Readiness::Unknown);
    // An ordinary type error is not a cradle failure, and diagnostics without the cradle
    // error mean the cradle loaded: nothing positive is observable, so back to unknown.
    hls_diagnostics(
        &mut client,
        &a,
        json!([{"source": "typecheck", "severity": 1, "range": range, "message": "Variable not in scope"}]),
    );
    let state = client.await_state_changed();
    assert_eq!(state.health, Health::Unknown);
    assert_eq!(state.message, None);
    client.shutdown();
}

// --- real haskell-language-server (local only) --------------------------------

fn real_hls(project: &support::TempCabalProject) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec![
            "--".to_string(),
            "haskell-language-server-wrapper".to_string(),
            "--lsp".to_string(),
        ],
        root: project.root.clone(),
    }
}

/// Via the real server: nothing is held (readiness is unknown), the declaration is `{}`, and
/// health stays unknown on a loadable project.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn hls_real_readiness_is_unknown_and_nothing_is_held() {
    let project = support::TempCabalProject::with_cross_file_reference("unknown");
    let a = project.file("src/A.hs");
    let mut client = ConformanceClient::start(&real_hls(&project));
    let result = client.initialize_with_root(true, &project.root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee for a server whose readiness cannot be observed: {result}"
    );
    client.did_open(&a, "haskell");
    let (line, character) = support::HS_TARGET_DECLARATION;
    // Answered (possibly incomplete, as the spec's `unknown` allows), not held.
    let _ = client.references(&a, line, character);
    let state = client.server_state();
    assert_eq!(state.readiness, Readiness::Unknown);
    assert_eq!(state.health, Health::Unknown);
    client.shutdown();
}

/// Via the real server with a cradle that cannot load: health becomes `error`.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn hls_real_broken_cradle_is_error_health() {
    let project = support::TempCabalProject::with_broken_cradle("broken");
    let a = project.file("src/A.hs");
    let mut client = ConformanceClient::start(&real_hls(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&a, "haskell");
    let state = poll_state_until(&mut client, |s| s.health != Health::Unknown);
    assert_eq!(state.health, Health::Error, "{state:?}");
    assert!(
        state
            .message
            .as_deref()
            .is_some_and(|m| m.starts_with("Failed to run cabal")),
        "{state:?}"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// pyrefly (M16, ADR 0019 decision F). No mapping: the initial index is silent on the protocol
// (its start and end go to stderr only), "Pyrefly: Rechecking" covers only the typecheck of
// open files, and neither readiness nor health has a signal
// (research/pyrefly-readiness-measurement.md). lsp-det has no mapping for "pyrefly-lsp" and
// reports unknown on both axes (spec 8.2 item 3, 8.4 item 1).
// ---------------------------------------------------------------------------

fn real_pyrefly(project: &support::TempPyreflyProject) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec!["--".to_string(), "pyrefly".to_string(), "lsp".to_string()],
        root: project.root.clone(),
    }
}

/// Via the real server: unknown on both axes, `{}` declared, and `references` answered rather
/// than held.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn pyrefly_real_is_unknown_on_both_axes_and_nothing_is_held() {
    let project = support::TempPyreflyProject::with_cross_file_reference("unknown");
    let a = project.file("pkg/a.py");
    let mut client = ConformanceClient::start(&real_pyrefly(&project));
    let result = client.initialize_with_root(true, &project.root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee for a server with no mapping: {result}"
    );
    let state = client.server_state();
    assert_eq!(state.readiness, Readiness::Unknown);
    assert_eq!(state.health, Health::Unknown);
    client.did_open(&a, "python");
    let (line, character) = support::PYREFLY_TARGET_DECLARATION;
    // Answered (possibly cancelled with -32800 or incomplete, as `unknown` allows), not held.
    let _ = client.references(&a, line, character);
    assert_eq!(client.server_state().readiness, Readiness::Unknown);
    client.shutdown();
}

// ---------------------------------------------------------------------------
// crystalline (M17, ADR 0019 decision F). The mapping in
// docs/research/crystalline-readiness-measurement.md: the server returns no `serverInfo` and is
// known only by the startup `window/logMessage` `"[workspace] Found projects:` (the leading
// double quote is part of the message; sent only when a `shard.yml` project was found). Readiness moves from `initializing` to `ready` on the log "LSP
// server is ready.". `$/progress` is a per-request compilation (title "Building project") that a
// request waits on synchronously before answering, so it is not readiness and is not read. No
// health signal; no guarantee (the version does not appear in the protocol).
// ---------------------------------------------------------------------------

fn crystalline_client(root: &std::path::Path) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_crystalline();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize_with_root(true, root);
    (client, result)
}

/// A barrier: a round trip through the fake upstream. Everything the fake was told to emit
/// before this call has already been read by lsp-det by the time the answer comes back (the same
/// technique as `nextflow_sync`), so a query sent after this observes the effect of the log
/// message even though selecting a mapping is not itself a notification.
fn crystalline_sync(client: &mut ConformanceClient) {
    let _ = client.upstream_methods_seen();
}

#[test]
fn crystalline_is_selected_by_its_startup_log_and_becomes_ready_on_the_ready_log() {
    let project = support::TempCrystalProject::new("select");
    let (mut client, _) = crystalline_client(&project.root);
    let state = client.server_state();
    assert_eq!(state.readiness, Readiness::Unknown);
    assert_eq!(state.health, Health::Unknown);

    client.make_upstream_emit_log_message(3, "\"[workspace] Found projects:\n/p/fixture");
    crystalline_sync(&mut client);
    assert_eq!(
        client.server_state().readiness,
        Readiness::Initializing,
        "the startup log did not select the mapping"
    );

    client.make_upstream_emit_log_message(3, "LSP server is ready.");
    let state = client.await_state_changed();
    assert_eq!(state.readiness, Readiness::Ready);
    assert_eq!(state.health, Health::Unknown);
    client.shutdown();
}

#[test]
fn crystalline_ignores_per_request_compilation_progress() {
    let project = support::TempCrystalProject::new("progress");
    let (mut client, _) = crystalline_client(&project.root);
    client.make_upstream_emit_log_message(3, "\"[workspace] Found projects:\n/p/fixture");
    client.make_upstream_emit_log_message(3, "LSP server is ready.");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);

    client.make_upstream_emit_progress(json!({
        "token": "workspace/compile/0",
        "value": {"kind": "begin", "title": "Building project"}
    }));
    client.make_upstream_emit_progress(json!({
        "token": "workspace/compile/0",
        "value": {"kind": "end", "message": "Completed with errors."}
    }));
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "read readiness out of a per-request compilation token"
    );
    assert_eq!(client.server_state().health, Health::Unknown);
    client.shutdown();
}

#[test]
fn crystalline_spec_8_2_5_declares_no_guarantees() {
    let project = support::TempCrystalProject::new("guarantee");
    let (mut client, result) = crystalline_client(&project.root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee for a server whose version is not observable: {result}"
    );
    client.shutdown();
}

#[test]
fn crystalline_without_a_project_stays_unknown() {
    let project = support::TempCrystalProject::new("noproject");
    let (mut client, _) = crystalline_client(&project.root);
    client.make_upstream_emit_log_message(3, "LSP server is ready.");
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "selected the mapping without the \"Found projects:\" log"
    );
    let state = client.server_state();
    assert_eq!(state.readiness, Readiness::Unknown);
    assert_eq!(state.health, Health::Unknown);
    client.shutdown();
}

// --- real crystalline (local only) --------------------------------

fn real_crystalline(project: &support::TempCrystalProject) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec!["--".to_string(), "crystalline".to_string()],
        root: project.root.clone(),
    }
}

/// Via the real server: `ready` comes from the startup log, no guarantee is declared, and
/// `textDocument/definition` (crystalline has no `references`) answers across files.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn crystalline_real_becomes_ready_from_the_startup_log_and_answers_definition() {
    let project = support::TempCrystalProject::new("real");
    let fixture_cr = project.file("src/fixture.cr");
    let b_cr = project.file("src/b.cr");
    let mut client = ConformanceClient::start(&real_crystalline(&project));
    let result = client.initialize_with_root(true, &project.root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee for a server whose version is not observable: {result}"
    );
    let state = poll_state_until(&mut client, |s| s.readiness == Readiness::Ready);
    assert_eq!(state.health, Health::Unknown);

    client.did_open(&fixture_cr, "crystal");
    let response = client.request(
        "textDocument/definition",
        json!({
            "textDocument": {"uri": support::file_uri(&fixture_cr)},
            "position": {"line": 3, "character": 5},
        }),
    );
    let locations = response["result"].as_array().cloned().unwrap_or_default();
    assert!(
        !locations.is_empty(),
        "definition returned nothing: {response}"
    );
    assert_eq!(
        locations[0]["uri"],
        Value::String(support::file_uri(&b_cr)),
        "definition did not point at b.cr: {response}"
    );
    assert_eq!(locations[0]["range"]["start"]["line"], json!(2));
    client.shutdown();
}

// ---------------------------------------------------------------------------
// Gleam (M19, ADR 0019 decision F). The mapping in
// research/gleam-readiness-measurement.md: no `serverInfo`, identified by the `$/progress`
// begin title "Downloading Gleam dependencies" (token `"downloading-dependencies"`), which is
// sent right after `initialized` even when there is nothing to download (measured 12 ms); begin
// → `indexing` after the first round (or stays `initializing` the first time), end → `ready`.
// `didChange` and watched-file changes are not incorporated by the mapping (a `didChange` is
// synchronous inside request processing; a watched-file Changed recreates the engine but breaks
// `references` instead of completing it, so it is not predicted). No health signal. No
// guarantee is declared (`{}`): the version never appears in the protocol.
// ---------------------------------------------------------------------------

const GLEAM_DEPENDENCY_TOKEN: &str = "downloading-dependencies";
const GLEAM_DEPENDENCY_TITLE: &str = "Downloading Gleam dependencies";

fn gleam_client(root: &std::path::Path) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_gleam();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize_with_root(true, root);
    (client, result)
}

/// Emits the fixed-token dependency-download progress a real Gleam sends
/// (research/gleam-readiness-measurement.md).
fn gleam_dependency_progress(client: &mut ConformanceClient, kind: &str) {
    let mut value = json!({"kind": kind});
    if kind == "begin" {
        value["title"] = json!(GLEAM_DEPENDENCY_TITLE);
        value["cancellable"] = json!(false);
    }
    client.make_upstream_emit_progress(json!({"token": GLEAM_DEPENDENCY_TOKEN, "value": value}));
}

#[test]
fn gleam_is_selected_by_the_dependency_progress_and_becomes_ready_at_its_end() {
    let project = support::TempGleamProject::with_cross_file_reference("select");
    let (mut client, _) = gleam_client(&project.root);
    // Both axes unknown before the upstream calls itself anything.
    let state = client.server_state();
    assert_eq!(state.readiness, Readiness::Unknown);
    assert_eq!(state.health, Health::Unknown);
    gleam_dependency_progress(&mut client, "begin");
    // Selecting a mapping is not itself a notified change: use a round trip through the fake
    // upstream as a synchronization barrier before reading the state by request.
    let _ = client.upstream_methods_seen();
    assert_eq!(
        client.server_state().readiness,
        Readiness::Initializing,
        "a first begin does not move past the mapping's starting state"
    );
    gleam_dependency_progress(&mut client, "end");
    let state = client.await_state_changed();
    assert_eq!(state.readiness, Readiness::Ready);
    assert_eq!(state.health, Health::Unknown);
    client.shutdown();
}

#[test]
fn gleam_reindexes_when_the_engine_is_recreated() {
    let project = support::TempGleamProject::with_cross_file_reference("reindex");
    let (mut client, _) = gleam_client(&project.root);
    gleam_dependency_progress(&mut client, "begin");
    let _ = client.upstream_methods_seen();
    gleam_dependency_progress(&mut client, "end");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    // The engine is recreated (a `gleam.toml` change) and downloads dependencies again.
    gleam_dependency_progress(&mut client, "begin");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    gleam_dependency_progress(&mut client, "end");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn gleam_does_not_predict_from_document_or_watched_file_changes() {
    let project = support::TempGleamProject::with_cross_file_reference("no-predict");
    let (mut client, _) = gleam_client(&project.root);
    gleam_dependency_progress(&mut client, "begin");
    let _ = client.upstream_methods_seen();
    gleam_dependency_progress(&mut client, "end");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    let a = project.file("src/a.gleam");
    client.did_change(&a, 2, support::GLEAM_A);
    let gleam_toml = project.file("gleam.toml");
    client.did_change_watched_files(&[(&gleam_toml, 2)]);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "predicted a state change from a didChange or a watched-file change"
    );
    client.shutdown();
}

#[test]
fn gleam_spec_8_2_5_declares_no_guarantees() {
    let project = support::TempGleamProject::with_cross_file_reference("guarantees");
    let (mut client, result) = gleam_client(&project.root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee Gleam's vocabulary cannot keep: {result}"
    );
    client.shutdown();
}

// --- real Gleam (local only) -----------------------------------------------

fn real_gleam(project: &support::TempGleamProject) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec!["--".to_string(), "gleam".to_string(), "lsp".to_string()],
        root: project.root.clone(),
    }
}

/// Via the real server: no guarantee is declared, `ready` is reached, and health stays
/// `unknown` (no signal).
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn gleam_spec_7_1_through_lsp_det_with_real_gleam() {
    let project = support::TempGleamProject::with_cross_file_reference("readiness");
    let a = project.file("src/a.gleam");
    let mut client = ConformanceClient::start(&real_gleam(&project));
    let result = client.initialize_with_root(true, &project.root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee for a server whose version never appears in the protocol: {result}"
    );
    client.did_open(&a, "gleam");
    let (line, character) = support::GLEAM_TARGET_DECLARATION;
    // The real Gleam identifies itself only through a `$/progress` sent after `initialized`
    // (research/gleam-readiness-measurement.md), an unavoidable round trip that a bare
    // `experimental/serverState` request can race ahead of. The first `references` cannot:
    // Gleam answers no request before the token ends and compiles synchronously inside request
    // handling (measured), so waiting on its response synchronizes this test past that gap.
    let _ = client.references(&a, line, character);
    client.wait_until_ready();
    assert_eq!(
        client.server_state().health,
        Health::Unknown,
        "Gleam has no health signal"
    );
    client.shutdown();
}

/// Via the real server: a `didChange` on an open file that adds a cross-file call is
/// incorporated into `references` (spec 7.3 item 1). Gleam may include the declaration itself
/// in the result, so the check is on the count increasing rather than on the exact set.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn gleam_spec_7_3_cross_file_freshness_through_lsp_det_with_real_gleam() {
    let project = support::TempGleamProject::with_cross_file_reference("freshness");
    let a = project.file("src/a.gleam");
    let b = project.file("src/b.gleam");
    let mut client = ConformanceClient::start(&real_gleam(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&a, "gleam");
    client.did_open(&b, "gleam");
    let (line, character) = support::GLEAM_TARGET_DECLARATION;
    // The first `references` synchronizes past the async gap between `initialized` and the
    // dependency-download progress (see gleam_spec_7_1_through_lsp_det_with_real_gleam).
    let before = client.references(&a, line, character).len();
    client.wait_until_ready();
    client.did_change(&b, 2, support::GLEAM_B_WITH_TWO_CALLS);
    let after = client.references(&a, line, character).len();
    assert_eq!(
        after,
        before + 1,
        "the added call in b.gleam was not incorporated into references"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// haxe-language-server (M20, ADR 0019 decision F). The mapping in
// research/haxe-language-server-readiness-measurement.md: the server returns no `serverInfo`
// and names itself only through a `window/logMessage` "Haxe Path: " sent after
// `workspace/didChangeConfiguration` (which is also what starts the underlying compiler);
// `$/progress` reuses one title format ("Haxe: " + name + "...") for both the startup work
// ("Building Cache", "Parsing Classpaths", "Building Refactoring Cache…", concurrent) and
// per-request work ("Collecting Diagnostics", "Performing Refactor/Rename Operation…"), and the
// names are fixed and disjoint, so only the startup titles move readiness: `ready` needs none
// of them open; health comes from `window/showMessage` (type 1, "Haxe version check failed" /
// "Invalid compiler argument"), the "Haxe connected!" log, and `haxe/haxeKeepsCrashing`; no
// guarantee (the version never appears on the protocol, and an open document's `didChange` is
// not incorporated into other files' `references`, only a `didSave` is).
// ---------------------------------------------------------------------------

fn haxe_client(root: &std::path::Path) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_haxe_language_server();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize_with_root(true, root);
    (client, result)
}

/// The settings a client has to send for haxe-language-server to start its compiler
/// (`settings.haxe` can be empty).
fn haxe_configure(client: &mut ConformanceClient) {
    client.notify(
        "workspace/didChangeConfiguration",
        json!({"settings": {"haxe": {}}}),
    );
}

fn haxe_progress(client: &mut ConformanceClient, token: i64, kind: &str, title: Option<&str>) {
    let mut value = json!({"kind": kind});
    if let Some(title) = title {
        value["title"] = json!(title);
    }
    client.make_upstream_emit_progress(json!({"token": token, "value": value}));
}

fn haxe_log(client: &mut ConformanceClient, message: &str) {
    client.make_upstream_emit_log_message(4, message);
}

#[test]
fn haxe_is_selected_by_the_haxe_path_log_and_becomes_ready_when_the_startup_tokens_end() {
    let project = support::TempHaxeProject::with_cross_file_reference("select");
    let (mut client, _) = haxe_client(&project.root);
    // Nothing said yet: unknown on both axes.
    let state = client.server_state();
    assert_eq!(state.readiness, Readiness::Unknown);
    assert_eq!(state.health, Health::Unknown);

    haxe_log(&mut client, "Haxe Path: haxe");
    // A round trip through lsp-det and back as a synchronization wall: by the time this
    // response arrives, the log notification sent just before it has already been interpreted.
    client.upstream_methods_seen();
    assert_eq!(client.server_state().readiness, Readiness::Initializing);

    haxe_log(&mut client, "Haxe connected!");
    assert_eq!(client.await_state_changed().health, Health::Ok);

    haxe_progress(&mut client, 0, "begin", Some("Haxe: Building Cache..."));
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "a single begin should not move readiness while already initializing"
    );
    haxe_progress(&mut client, 1, "begin", Some("Haxe: Parsing Classpaths..."));
    haxe_progress(&mut client, 0, "end", None);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "claimed ready while \"Parsing Classpaths\" is still open"
    );
    haxe_progress(
        &mut client,
        2,
        "begin",
        Some("Haxe: Building Refactoring Cache\u{2026}..."),
    );
    haxe_progress(&mut client, 2, "end", None);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "claimed ready while \"Parsing Classpaths\" is still open"
    );
    haxe_progress(&mut client, 1, "end", None);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn haxe_ignores_request_processing_progress_and_reindexes_on_startup_titles() {
    let project = support::TempHaxeProject::with_cross_file_reference("request-progress");
    let (mut client, _) = haxe_client(&project.root);
    haxe_log(&mut client, "Haxe Path: haxe");
    haxe_progress(&mut client, 0, "begin", Some("Haxe: Building Cache..."));
    haxe_progress(&mut client, 0, "end", None);
    // Unlike the identity log alone (silent: it only places the starting state), this end
    // is a real, notifiable readiness change, so waiting for the notification (rather than
    // polling through a synchronization wall) is both sufficient and race-free.
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);

    haxe_progress(
        &mut client,
        3,
        "begin",
        Some("Haxe: Collecting Diagnostics..."),
    );
    haxe_progress(&mut client, 3, "end", None);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "moved because of request-processing progress"
    );

    haxe_progress(&mut client, 4, "begin", Some("Haxe: Building Cache..."));
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    haxe_progress(&mut client, 4, "end", None);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn haxe_maps_show_message_errors_to_error_health() {
    let project = support::TempHaxeProject::with_cross_file_reference("health");
    let (mut client, _) = haxe_client(&project.root);
    haxe_log(&mut client, "Haxe Path: haxe");
    client.upstream_methods_seen();
    assert_eq!(client.server_state().readiness, Readiness::Initializing);

    client.make_upstream_emit_notification(
        "window/showMessage",
        json!({"type": 1, "message": "Haxe version check failed: \"/bin/sh: haxe: command not found\""}),
    );
    let state = client.await_state_changed();
    assert_eq!(state.health, Health::Error);
    assert_eq!(
        state.message.as_deref(),
        Some("Haxe version check failed: \"/bin/sh: haxe: command not found\"")
    );

    haxe_log(&mut client, "Haxe connected!");
    assert_eq!(client.await_state_changed().health, Health::Ok);

    client.make_upstream_emit_notification("haxe/haxeKeepsCrashing", json!(null));
    let state = client.await_state_changed();
    assert_eq!(state.health, Health::Error);
    assert_eq!(state.message.as_deref(), Some("Haxe keeps crashing"));
    client.shutdown();
}

#[test]
fn haxe_spec_8_2_5_declares_no_guarantees() {
    let project = support::TempHaxeProject::with_cross_file_reference("guarantees");
    let (mut client, result) = haxe_client(&project.root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee haxe-language-server's vocabulary cannot keep: {result}"
    );
    client.shutdown();
}

// --- real haxe-language-server (local only) -------------------------------

/// haxe-language-server's `server.js` (M20's `haxe-language-server` derivation in `flake.nix`,
/// built from vshaxe's vsix). Needs `haxe` on `PATH`.
fn real_haxe(project: &support::TempHaxeProject) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec![
            "--".to_string(),
            "haxe-language-server".to_string(),
            "--stdio".to_string(),
        ],
        root: project.root.clone(),
    }
}

/// Via the real server. `initializationOptions.displayArguments` is required (Serena sends the
/// same) so the server finds `build.hxml`.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn haxe_spec_7_1_through_lsp_det_with_real_haxe() {
    let project = support::TempHaxeProject::with_cross_file_reference("readiness");
    let a = project.file("src/A.hx");
    let mut client = ConformanceClient::start(&real_haxe(&project));
    let result = client.initialize_with_root_and_initialization_options(
        true,
        &project.root,
        json!({"displayArguments": ["build.hxml"]}),
    );
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee for real haxe-language-server: {result}"
    );
    haxe_configure(&mut client);
    client.did_open(&a, "haxe");
    // Identification (the "Haxe Path: " log) is silent and only follows
    // `workspace/didChangeConfiguration` after the real compiler actually starts, unlike the
    // fake upstream tests where a synchronization wall stands in for that delay.
    // `wait_until_ready` requires readiness to already be observable, so poll past that first.
    poll_state_until(&mut client, |s| s.readiness != Readiness::Unknown);
    client.wait_until_ready();
    let state = poll_state_until(&mut client, |s| s.health != Health::Unknown);
    assert_eq!(state.health, Health::Ok, "{state:?}");
    client.shutdown();
}

/// Measures 7.2 coverage against the real server: 300 calls to `A.target()` across `C01` …
/// `C50` plus `B.hx`'s own call.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn haxe_spec_7_2_coverage_through_lsp_det_with_real_haxe() {
    let project = support::TempHaxeProject::with_many_calls("coverage", 50);
    let a = project.file("src/A.hx");
    let mut client = ConformanceClient::start(&real_haxe(&project));
    client.initialize_with_root_and_initialization_options(
        true,
        &project.root,
        json!({"displayArguments": ["build.hxml"]}),
    );
    haxe_configure(&mut client);
    client.did_open(&a, "haxe");
    poll_state_until(&mut client, |s| s.readiness != Readiness::Unknown);
    client.wait_until_ready();
    let (line, character) = support::HX_TARGET_DECLARATION;
    let found = client.references(&a, line, character);
    assert_eq!(
        found.len(),
        51,
        "missed references while declaring ready (coverage violation): {found:#?}"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// Dart analysis server (M21, ADR 0020 decision C row for Dart). The mapping in
// research/dart-readiness-measurement.md: identified by `serverInfo.name` "Dart SDK LSP
// Analysis Server" (the version is `serverInfo.version`). `$/progress` (fixed token
// "ANALYZING", title "Analyzing…") begin -> `indexing`, end -> `ready`, repeated on every
// analysis round (didChange, an on-disk change, or even nothing to analyze right after
// `initialized`), the same shape as rust-analyzer's `quiescent`. The server itself holds a
// request until the analysis it depends on completes, so there is nothing for the observer to
// predict from `didChange` or `workspace/didChangeWatchedFiles` (`observe_client` is not
// implemented). No health signal. `coverage: {scope: "workspace", incomplete: {}}` and
// `freshness: {fileChanges: ["Created", "Changed", "Deleted"]}` are declared for the tested
// version 3.13.0 (spec 8.2 item 5; the real-server tests below are the basis).
// ---------------------------------------------------------------------------

const DART_ANALYZING_TOKEN: &str = "ANALYZING";

fn dart_client() -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_dart();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    (client, result)
}

/// Emits the fixed-token analysis progress a real Dart analysis server sends
/// (research/dart-readiness-measurement.md).
fn dart_progress(client: &mut ConformanceClient, kind: &str) {
    let mut value = json!({"kind": kind});
    if kind == "begin" {
        value["title"] = json!("Analyzing\u{2026}");
        value["cancellable"] = json!(false);
    }
    client.make_upstream_emit_progress(json!({"token": DART_ANALYZING_TOKEN, "value": value}));
}

#[test]
fn dart_is_selected_by_server_info_and_declares_a_guarantee_for_the_tested_version() {
    let (mut client, result) = dart_client();
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({
            "coverage": {"scope": "workspace", "incomplete": {}},
            "freshness": {"fileChanges": ["Created", "Changed", "Deleted"]}
        }),
        "declared a different guarantee for the tested version 3.13.0: {result}"
    );
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    client.shutdown();
}

#[test]
fn dart_declares_no_guarantee_for_an_untested_version() {
    let server = ServerUnderTest::lsp_det_with_upstream_flags(
        "Dart SDK LSP Analysis Server",
        &["--server-version", "3.12.0"],
    );
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee for a version the conformance suite has not passed on: {result}"
    );
    client.shutdown();
}

#[test]
fn dart_becomes_ready_at_the_end_of_the_first_analyzing_round() {
    let (mut client, _) = dart_client();
    dart_progress(&mut client, "begin");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    dart_progress(&mut client, "end");
    let state = client.await_state_changed();
    assert_eq!(state.readiness, Readiness::Ready);
    assert_eq!(state.health, Health::Unknown, "Dart has no health signal");
    client.shutdown();
}

#[test]
fn dart_reindexes_on_every_later_round() {
    let (mut client, _) = dart_client();
    dart_progress(&mut client, "begin");
    client.await_state_changed();
    dart_progress(&mut client, "end");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    // A later round: a didChange, an on-disk change the server watches itself, or even nothing
    // to analyze (research: run 6, a begin/end pair still happens).
    dart_progress(&mut client, "begin");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    dart_progress(&mut client, "end");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn dart_ignores_progress_of_other_tokens() {
    let (mut client, _) = dart_client();
    client.make_upstream_emit_progress(json!({
        "token": "some-other-token",
        "value": {"kind": "begin", "title": "Something else"}
    }));
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "moved state on an unrelated progress token"
    );
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    client.shutdown();
}

#[test]
fn dart_does_not_predict_from_document_or_watched_file_changes() {
    let (mut client, _) = dart_client();
    dart_progress(&mut client, "begin");
    client.await_state_changed();
    dart_progress(&mut client, "end");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.did_change(
        &std::path::PathBuf::from("/fake/lib/a.dart"),
        2,
        "void target() {}\n",
    );
    client.did_change_watched_files(&[(&std::path::PathBuf::from("/fake/lib/b.dart"), 2)]);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "predicted a state change from a didChange or a watched-file change; the server itself \
         holds requests until analysis completes, so there is nothing to predict"
    );
    client.shutdown();
}

/// Gate (spec chapter 9) holds this on the client's behalf because it does not declare the
/// protocol itself; the readiness that drives the hold comes from the Dart mapping's
/// `$/progress` tracking. `--references-depend-on-readiness` makes the fake upstream's own
/// answer depend on ITS OWN state (default "ready"), so a non-empty answer arriving only after
/// `end` shows the request really was held, not merely delayed.
#[test]
fn dart_holds_references_until_ready() {
    let server = ServerUnderTest::lsp_det_with_upstream_flags(
        "Dart SDK LSP Analysis Server",
        &[
            "--server-version",
            "3.13.0",
            "--references-depend-on-readiness",
        ],
    );
    let mut client = ConformanceClient::start(&server);
    client.initialize(false);
    let id = client.send_references();
    assert!(
        client.response_within(id, NEGATIVE_WINDOW).is_none(),
        "forwarded references before the first analyzing round even started"
    );
    dart_progress(&mut client, "begin");
    assert!(
        client.response_within(id, NEGATIVE_WINDOW).is_none(),
        "forwarded references while indexing"
    );
    dart_progress(&mut client, "end");
    let response = client.await_response_to(id);
    assert!(
        !response["result"]
            .as_array()
            .expect("references answers an array")
            .is_empty(),
        "did not release the hold once ready: {response}"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// Real Dart analysis server integration (local only. Not part of CI — v0.1-design.md
// chapter 6). Requires the Dart SDK on PATH (`nix develop .#servers`; `dart language-server`).
// ---------------------------------------------------------------------------

/// Caller files under `lib/` (the method section of research/dart-readiness-measurement.md: a 401-file fixture,
/// large enough that analysis takes observable time).
const DART_FIXTURE_CALLERS: usize = 400;

fn real_dart(project: &support::TempDartProject) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec![
            "--".to_string(),
            "dart".to_string(),
            "language-server".to_string(),
        ],
        root: project.root.clone(),
    }
}

/// Returns only the references to `target` (declared in `a.dart`) that point at `file`.
fn dart_references_in(
    client: &mut ConformanceClient,
    a: &std::path::Path,
    file: &std::path::Path,
) -> Vec<Value> {
    let wanted = support::file_uri(file);
    let (line, character) = support::DART_TARGET_DECLARATION;
    client
        .references(a, line, character)
        .into_iter()
        .filter(|location| location["uri"] == Value::String(wanted.clone()))
        .collect()
}

/// Identity and the guarantee declared for the tested version (3.13.0).
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn dart_is_selected_by_its_real_server_info() {
    let project = support::TempDartProject::with_many_callers("select", 1);
    let mut client = ConformanceClient::start(&real_dart(&project));
    let result = client.initialize_with_root(true, &project.root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({
            "coverage": {"scope": "workspace", "incomplete": {}},
            "freshness": {"fileChanges": ["Created", "Changed", "Deleted"]}
        }),
        "no guarantee is declared for the tested version of real Dart: {result}"
    );
    client.shutdown();
}

/// 7.1: the first `references` is complete (research doc, run 1: the server holds the request
/// until the analysis it depends on finishes, so there is no empty or partial answer to
/// observe even though the query is sent right after `didOpen`, before the mapping itself has
/// observed the first analyzing round end). The answer is measured to arrive shortly BEFORE the
/// `ANALYZING` end (end is a whole-server idle notification; the answer only waits on the
/// target's own analysis), so this does not assert that readiness has already reached `ready`
/// by the time the answer arrives -- only that the round that started does complete afterward.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn dart_spec_7_1_first_references_is_complete_through_lsp_det_with_real_dart() {
    let project = support::TempDartProject::with_many_callers("readiness", DART_FIXTURE_CALLERS);
    let a = project.file("lib/a.dart");
    let mut client = ConformanceClient::start(&real_dart(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&a, "dart");
    assert_ne!(client.server_state().readiness, Readiness::Ready);
    let indexing = client.await_state_changed();
    assert_eq!(
        indexing.readiness,
        Readiness::Indexing,
        "did not observe the first analyzing round begin: {indexing:?}"
    );

    let (line, character) = support::DART_TARGET_DECLARATION;
    let found = client.references(&a, line, character);
    assert_eq!(
        found.len(),
        DART_FIXTURE_CALLERS,
        "the first references was not complete: {} of {}",
        found.len(),
        DART_FIXTURE_CALLERS
    );
    // The round that was open when the answer arrived does end (its own completion, not a
    // fixed timeout).
    client.wait_until_ready();
    client.shutdown();
}

/// 7.2: the result once `ready` matches the precomputed complete set (every caller file calls
/// `target` exactly once).
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn dart_spec_7_2_coverage_through_lsp_det_with_real_dart() {
    let project = support::TempDartProject::with_many_callers("coverage", DART_FIXTURE_CALLERS);
    let a = project.file("lib/a.dart");
    let mut client = ConformanceClient::start(&real_dart(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&a, "dart");
    client.wait_until_ready();

    let (line, character) = support::DART_TARGET_DECLARATION;
    let found = client.references(&a, line, character);
    assert_eq!(
        found.len(),
        DART_FIXTURE_CALLERS,
        "missed some callers while declaring ready (completeness violation): {} of {}",
        found.len(),
        DART_FIXTURE_CALLERS
    );
    client.shutdown();
}

/// 7.3 item 1: a `didChange` on an open file (`f0.dart`) that adds one more call to `target` is
/// incorporated. The query is sent right after the notification, as the spec recommends; if it
/// still races a request already in flight at the moment of the change, Dart answers it with
/// -32801 ContentModified (research doc, runs 2 and 5), which `ConformanceClient::references` already
/// retries once (tests/support/mod.rs) -- no extra retry logic is needed here.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn dart_spec_7_3_1_did_change_on_an_open_file_through_lsp_det_with_real_dart() {
    let project =
        support::TempDartProject::with_many_callers("freshness-didchange", DART_FIXTURE_CALLERS);
    let a = project.file("lib/a.dart");
    let f0 = project.file("lib/f0.dart");
    let mut client = ConformanceClient::start(&real_dart(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&a, "dart");
    client.did_open(&f0, "dart");
    client.wait_until_ready();

    let (line, character) = support::DART_TARGET_DECLARATION;
    let before = client.references(&a, line, character).len();
    client.did_change(&f0, 2, &support::dart_caller_file_with_calls(0, 2));
    let after = client.references(&a, line, character).len();
    assert_eq!(
        after,
        before + 1,
        "an added call in an open file was not incorporated: before={before} after={after}"
    );
    client.shutdown();
}

/// 7.3 items 2-4: watched-file Created / Changed / Deleted of a file that is not opened, each
/// reflected in a query from a different file (`a.dart`, the file that defines `target`). Dart
/// does its own file watching (research: the `workspace/didChangeWatchedFiles` lsp-det sends or
/// stands in for is answered with an "Unknown method" `window/showMessage` and otherwise
/// ignored; `ConformanceClient::stash` keeps that notification without failing the test).
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn dart_spec_7_3_2_watched_file_changes_through_lsp_det_with_real_dart() {
    let project = support::TempDartProject::with_many_callers("watched", DART_FIXTURE_CALLERS);
    let a = project.file("lib/a.dart");
    let caller = project.file("lib/f1.dart");
    let mut client = ConformanceClient::start(&real_dart(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&a, "dart");
    client.wait_until_ready();

    watched_file_changes_are_reflected(
        &mut client,
        &a,
        &caller,
        &support::dart_caller_file_with_calls(1, 2),
        &project.file("lib/g.dart"),
        support::DART_G,
        None,
        dart_references_in,
        true,
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// jdtls / Eclipse JDT Language Server (M23, ADR 0020 decision C row for jdtls). The mapping in
// research/jdtls-readiness-measurement.md: identified by `serverInfo.name`
// "JDT Language Server (Standard)" (the version is `serverInfo.version`, "1.60.0-SNAPSHOT" for
// nixpkgs 1.60.0). `language/status` (`{type, message}`) `type: "ServiceReady"` -> `ready`
// (starting from `initializing`); `$/progress` is not mapped to readiness ("Building" is
// compilation for diagnostics, not the index; JDT search waits for the index itself with
// WAIT_UNTIL_READY_TO_SEARCH, so mapping progress would only delay complete results). No
// prediction (`observe_client` is not implemented; the server itself holds requests). health:
// `language/status` `type: "ProjectStatus"` message "OK" -> `ok`, "WARNING" -> `warning`;
// `type: "Error"` -> `error`. Additionally, `textDocument/publishDiagnostics` on a URI that does
// not end with ".java" (the project resource itself or a build file) with a severity-1
// diagnostic -> `warning`, reverting to whatever the last `ProjectStatus` / `Error` reported once
// that URI's diagnostics are empty again. `coverage: {scope: "workspace", incomplete: {}}` and
// `freshness: {fileChanges: ["Created", "Changed", "Deleted"]}` are declared for the tested
// version 1.60.0-SNAPSHOT (spec 8.2 item 5; the real-server tests below are the basis).
// ---------------------------------------------------------------------------

fn jdtls_client() -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_jdtls();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    (client, result)
}

/// Emits `language/status` (`{type, message}`), as the real jdtls does
/// (research/jdtls-readiness-measurement.md).
fn jdtls_status(client: &mut ConformanceClient, status_type: &str, message: &str) {
    client.make_upstream_emit_notification(
        "language/status",
        json!({"type": status_type, "message": message}),
    );
}

/// `textDocument/publishDiagnostics` with a single severity-1 diagnostic (or none).
fn jdtls_diagnostics(
    client: &mut ConformanceClient,
    uri: &str,
    severity_one_message: Option<&str>,
) {
    let diagnostics = match severity_one_message {
        Some(message) => vec![json!({
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
            "severity": 1,
            "message": message
        })],
        None => vec![],
    };
    client.make_upstream_emit_notification(
        "textDocument/publishDiagnostics",
        json!({"uri": uri, "diagnostics": diagnostics}),
    );
}

#[test]
fn jdtls_is_selected_by_server_info_and_declares_a_guarantee_for_the_tested_version() {
    let (mut client, result) = jdtls_client();
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({
            "coverage": {"scope": "workspace", "incomplete": {}},
            "freshness": {"fileChanges": ["Created", "Changed", "Deleted"]}
        }),
        "declared a different guarantee for the tested version 1.60.0-SNAPSHOT: {result}"
    );
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    client.shutdown();
}

#[test]
fn jdtls_declares_no_guarantee_for_an_untested_version() {
    let server = ServerUnderTest::lsp_det_with_upstream_flags(
        "JDT Language Server (Standard)",
        &["--server-version", "1.59.0-SNAPSHOT"],
    );
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee for a version the conformance suite has not passed on: {result}"
    );
    client.shutdown();
}

#[test]
fn jdtls_becomes_ready_on_service_ready() {
    let (mut client, _) = jdtls_client();
    jdtls_status(&mut client, "ServiceReady", "ServiceReady");
    let state = client.await_state_changed();
    assert_eq!(state.readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn jdtls_other_status_types_do_not_move_readiness() {
    let (mut client, _) = jdtls_client();
    for (status_type, message) in [
        ("Starting", "Init..."),
        ("Started", "Ready"),
        ("Message", "some message"),
    ] {
        jdtls_status(&mut client, status_type, message);
    }
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "moved state on a status type other than ServiceReady/ProjectStatus/Error"
    );
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    client.shutdown();
}

#[test]
fn jdtls_ignores_progress() {
    let (mut client, _) = jdtls_client();
    client.make_upstream_emit_progress(json!({
        "token": "some-uuid",
        "value": {"kind": "begin", "title": "Building"}
    }));
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "$/progress must not move readiness (research: the index is not what it tracks)"
    );
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    client.shutdown();
}

#[test]
fn jdtls_project_status_ok_and_warning_move_health() {
    let (mut client, _) = jdtls_client();
    jdtls_status(&mut client, "ProjectStatus", "OK");
    assert_eq!(client.await_state_changed().health, Health::Ok);
    jdtls_status(&mut client, "ProjectStatus", "WARNING");
    assert_eq!(client.await_state_changed().health, Health::Warning);
    client.shutdown();
}

#[test]
fn jdtls_error_status_moves_health_to_error() {
    let (mut client, _) = jdtls_client();
    jdtls_status(&mut client, "ProjectStatus", "OK");
    client.await_state_changed();
    jdtls_status(&mut client, "Error", "something went wrong");
    let state = client.await_state_changed();
    assert_eq!(state.health, Health::Error);
    client.shutdown();
}

#[test]
fn jdtls_non_java_diagnostics_with_severity_one_are_warning_and_revert() {
    let (mut client, _) = jdtls_client();
    jdtls_status(&mut client, "ProjectStatus", "OK");
    assert_eq!(client.await_state_changed().health, Health::Ok);
    jdtls_diagnostics(
        &mut client,
        "file:///fixture",
        Some("Project 'fixture' is missing required library: 'missing.jar'"),
    );
    let state = client.await_state_changed();
    assert_eq!(state.health, Health::Warning);
    // Clearing that URI's diagnostics reverts to the last ProjectStatus (spec: `warning`
    // "partly functional", not a permanent state).
    jdtls_diagnostics(&mut client, "file:///fixture", None);
    let state = client.await_state_changed();
    assert_eq!(state.health, Health::Ok);
    client.shutdown();
}

#[test]
fn jdtls_java_diagnostics_are_ignored() {
    let (mut client, _) = jdtls_client();
    jdtls_status(&mut client, "ProjectStatus", "OK");
    client.await_state_changed();
    jdtls_diagnostics(
        &mut client,
        "file:///fixture/src/app/F0.java",
        Some("cannot find symbol"),
    );
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "a .java file's own diagnostics are not the project-URI health signal"
    );
    assert_eq!(client.server_state().health, Health::Ok);
    client.shutdown();
}

#[test]
fn jdtls_health_and_readiness_changes_preserve_each_other() {
    let (mut client, _) = jdtls_client();
    jdtls_status(&mut client, "ServiceReady", "ServiceReady");
    let state = client.await_state_changed();
    assert_eq!(state.readiness, Readiness::Ready);
    assert_eq!(state.health, Health::Unknown);
    // A health change afterward must not move readiness back.
    jdtls_status(&mut client, "ProjectStatus", "WARNING");
    let state = client.await_state_changed();
    assert_eq!(state.health, Health::Warning);
    assert_eq!(
        state.readiness,
        Readiness::Ready,
        "health change moved readiness"
    );
    client.shutdown();
}

#[test]
fn jdtls_does_not_predict_from_document_or_watched_file_changes() {
    let (mut client, _) = jdtls_client();
    jdtls_status(&mut client, "ServiceReady", "ServiceReady");
    client.await_state_changed();
    client.did_change(
        &std::path::PathBuf::from("/fake/src/app/F0.java"),
        2,
        "package app;\n",
    );
    client.did_change_watched_files(&[(&std::path::PathBuf::from("/fake/src/app/G.java"), 2)]);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "predicted a state change from a didChange or a watched-file change; the server itself \
         holds requests until the index is ready, so there is nothing to predict"
    );
    client.shutdown();
}

/// Gate (spec chapter 9) holds this on the client's behalf because it does not declare the
/// protocol itself; the readiness that drives the hold comes from the jdtls mapping's
/// `language/status` tracking. `--references-depend-on-readiness` makes the fake upstream's own
/// answer depend on ITS OWN state (default "ready"), so a non-empty answer arriving only after
/// `ServiceReady` shows the request really was held, not merely delayed.
#[test]
fn jdtls_holds_references_until_service_ready() {
    let server = ServerUnderTest::lsp_det_with_upstream_flags(
        "JDT Language Server (Standard)",
        &[
            "--server-version",
            "1.60.0-SNAPSHOT",
            "--references-depend-on-readiness",
        ],
    );
    let mut client = ConformanceClient::start(&server);
    client.initialize(false);
    let id = client.send_references();
    assert!(
        client.response_within(id, NEGATIVE_WINDOW).is_none(),
        "forwarded references before ServiceReady"
    );
    jdtls_status(&mut client, "ServiceReady", "ServiceReady");
    let response = client.await_response_to(id);
    assert!(
        !response["result"]
            .as_array()
            .expect("references answers an array")
            .is_empty(),
        "did not release the hold once ready: {response}"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// Real jdtls integration (local only. Not part of CI — v0.1-design.md chapter 6). Requires
// jdtls and a JDK on PATH (`nix develop .#servers`; `jdt-language-server`, `jdk21`).
// ---------------------------------------------------------------------------

/// Caller files under `src/app/` (the method section of research/jdtls-readiness-measurement.md: a 201-file
/// fixture, large enough that the index takes observable time).
const JDTLS_FIXTURE_CALLERS: usize = 200;

fn real_jdtls(project: &support::TempJdtlsProject) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec![
            "--".to_string(),
            "jdtls".to_string(),
            "-data".to_string(),
            project.data_dir.to_string_lossy().into_owned(),
        ],
        root: project.root.clone(),
    }
}

/// Returns only the references to `target` (declared in `Lib.java`) that point at `file`.
fn jdtls_references_in(
    client: &mut ConformanceClient,
    lib: &std::path::Path,
    file: &std::path::Path,
) -> Vec<Value> {
    let wanted = support::file_uri(file);
    let (line, character) = support::JDTLS_TARGET_DECLARATION;
    client
        .references(lib, line, character)
        .into_iter()
        .filter(|location| location["uri"] == Value::String(wanted.clone()))
        .collect()
}

/// Identity and the guarantee declared for the tested version (1.60.0-SNAPSHOT).
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn jdtls_is_selected_by_its_real_server_info() {
    let project = support::TempJdtlsProject::with_many_callers("select", 1);
    let mut client = ConformanceClient::start(&real_jdtls(&project));
    let result = client.initialize_with_root(true, &project.root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({
            "coverage": {"scope": "workspace", "incomplete": {}},
            "freshness": {"fileChanges": ["Created", "Changed", "Deleted"]}
        }),
        "no guarantee is declared for the tested version of real jdtls: {result}"
    );
    client.shutdown();
}

/// 7.1: the first `references` is complete. The server holds the request until the index it
/// depends on is ready (JDT's search runs with `WAIT_UNTIL_READY_TO_SEARCH`; research: a
/// `references` sent at 1.04s, before `ServiceReady` at 1.12s, answers only once the search can
/// run, and the answer is already complete). This mapping has no intermediate `indexing`
/// readiness (only `ServiceReady` moves it), so unlike the Dart mapping there is no "begin" to
/// observe first; the query is simply sent right after `didOpen`.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn jdtls_spec_7_1_first_references_is_complete_through_lsp_det_with_real_jdtls() {
    let project = support::TempJdtlsProject::with_many_callers("readiness", JDTLS_FIXTURE_CALLERS);
    let lib = project.file("src/app/Lib.java");
    let mut client = ConformanceClient::start(&real_jdtls(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&lib, "java");
    assert_ne!(client.server_state().readiness, Readiness::Ready);

    let (line, character) = support::JDTLS_TARGET_DECLARATION;
    let found = client.references(&lib, line, character);
    assert_eq!(
        found.len(),
        JDTLS_FIXTURE_CALLERS,
        "the first references was not complete: {} of {}",
        found.len(),
        JDTLS_FIXTURE_CALLERS
    );
    client.wait_until_ready();
    client.shutdown();
}

/// 7.2: the result once `ready` matches the precomputed complete set (every caller file calls
/// `target` exactly once).
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn jdtls_spec_7_2_coverage_through_lsp_det_with_real_jdtls() {
    let project = support::TempJdtlsProject::with_many_callers("coverage", JDTLS_FIXTURE_CALLERS);
    let lib = project.file("src/app/Lib.java");
    let mut client = ConformanceClient::start(&real_jdtls(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&lib, "java");
    client.wait_until_ready();

    let (line, character) = support::JDTLS_TARGET_DECLARATION;
    let found = client.references(&lib, line, character);
    assert_eq!(
        found.len(),
        JDTLS_FIXTURE_CALLERS,
        "missed some callers while declaring ready (completeness violation): {} of {}",
        found.len(),
        JDTLS_FIXTURE_CALLERS
    );
    client.shutdown();
}

/// 7.3 item 1: a `didChange` on an open file (`F0.java`) that adds one more call to
/// `Lib.target()` is incorporated.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn jdtls_spec_7_3_1_did_change_on_an_open_file_through_lsp_det_with_real_jdtls() {
    let project =
        support::TempJdtlsProject::with_many_callers("freshness-didchange", JDTLS_FIXTURE_CALLERS);
    let lib = project.file("src/app/Lib.java");
    let f0 = project.file("src/app/F0.java");
    let mut client = ConformanceClient::start(&real_jdtls(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&lib, "java");
    client.did_open(&f0, "java");
    client.wait_until_ready();

    let (line, character) = support::JDTLS_TARGET_DECLARATION;
    let before = client.references(&lib, line, character).len();
    client.did_change(&f0, 2, &support::jdtls_caller_file_with_calls(0, 2));
    let after = client.references(&lib, line, character).len();
    assert_eq!(
        after,
        before + 1,
        "an added call in an open file was not incorporated: before={before} after={after}"
    );
    client.shutdown();
}

/// 7.3 items 2-4: watched-file Created / Changed / Deleted of a file that is not opened, each
/// reflected in a query from a different file (`Lib.java`, the file that defines `target`).
/// jdtls registers `workspace/didChangeWatchedFiles` for `**/*.java` (research); the test client
/// sends the notification itself (`client_notifies: true`).
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn jdtls_spec_7_3_2_watched_file_changes_through_lsp_det_with_real_jdtls() {
    let project = support::TempJdtlsProject::with_many_callers("watched", JDTLS_FIXTURE_CALLERS);
    let lib = project.file("src/app/Lib.java");
    let caller = project.file("src/app/F1.java");
    let mut client = ConformanceClient::start(&real_jdtls(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&lib, "java");
    client.wait_until_ready();

    watched_file_changes_are_reflected(
        &mut client,
        &lib,
        &caller,
        &support::jdtls_caller_file_with_calls(1, 2),
        &project.file("src/app/G.java"),
        support::JDTLS_G,
        None,
        jdtls_references_in,
        true,
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// Sorbet (M22, ADR 0020 decision C row for Sorbet). The mapping in
// research/sorbet-readiness-measurement.md: identified by the `sorbet/showOperation`
// notification's method itself (no `serverInfo`, and the version never appears anywhere in the
// protocol). Operations not tied to a request (`Indexing`, `SlowPathBlocking`,
// `SlowPathNonBlocking`, `FastPath`) are counted -- they nest (measured: `Indexing` inside
// `SlowPathBlocking`): a `start` opens one and, once the state has reached `ready` before (a
// later round), moves to `indexing`; an `end` closes one, and readiness moves to `ready` once
// none is left open (the first round stays `initializing` until then, the same pattern as
// Gleam's first begin). Operations tied to a request (`References`, `SymbolSearch`, `Rename`,
// `MoveMethod`) are the request's own processing and are not readiness (ADR 0019 decision G).
// The server holds a request until the operation it depends on ends by itself, so there is
// nothing to predict from `didChange` / `workspace/didChangeWatchedFiles` (`observe_client` is
// not implemented). No health signal. No guarantee is declared for any version (ADR 0020
// decision E): the version never appears in the protocol.
//
// lsp-det injects `initializationOptions.supportsOperationNotifications: true` only when it is
// the one that launched the `sorbet` (or `srb`) command (ADR 0020 decision D); the fake
// upstream in this section is launched under its own build name (as every other fake-based
// test is), so it never receives the injected option and its `sorbet/showOperation` is emitted
// purely on the test's own command (`make_upstream_emit_notification`). The injection itself
// is exercised separately below, against a copy of the fake binary named `sorbet`.
// ---------------------------------------------------------------------------

fn sorbet_client() -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_sorbet();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    sorbet_identify(&mut client);
    (client, result)
}

/// Emits `sorbet/showOperation` the way a real Sorbet does
/// (research/sorbet-readiness-measurement.md).
fn sorbet_operation(client: &mut ConformanceClient, name: &str, status: &str) {
    client.make_upstream_emit_notification(
        "sorbet/showOperation",
        json!({"operationName": name, "description": format!("{name}..."), "status": status}),
    );
}

/// Establishes Sorbet's identity with a request-tied operation. The tracker lets the mapping
/// read the notification that established identity too (`Tracker::observe_upstream`), and a
/// request-tied operation is ignored by the mapping, so every operation asserted on afterward
/// starts counting from a known, empty count of open operations.
fn sorbet_identify(client: &mut ConformanceClient) {
    sorbet_operation(client, "References", "start");
    let _ = client.upstream_methods_seen();
}

#[test]
fn sorbet_is_selected_by_its_notification_and_becomes_ready_after_the_first_round() {
    let server = ServerUnderTest::lsp_det_with_fake_sorbet();
    let mut client = ConformanceClient::start(&server);
    client.initialize(true);
    let state = client.server_state();
    assert_eq!(state.readiness, Readiness::Unknown);
    assert_eq!(state.health, Health::Unknown);

    // The real startup sequence (research/sorbet-readiness-measurement.md): SlowPathBlocking
    // wraps Indexing. The very first notification establishes identity AND is read by the
    // mapping as the outer start (`Tracker::observe_upstream`), so all four are counted and
    // `ready` comes only at the outer end.
    sorbet_operation(&mut client, "SlowPathBlocking", "start");
    let _ = client.upstream_methods_seen();
    assert_eq!(
        client.server_state().readiness,
        Readiness::Initializing,
        "the notification establishing identity moves only to the mapping's starting state"
    );
    sorbet_operation(&mut client, "Indexing", "start");
    let _ = client.upstream_methods_seen();
    assert_eq!(
        client.server_state().readiness,
        Readiness::Initializing,
        "still the first round: a nested start does not move past initializing"
    );
    sorbet_operation(&mut client, "Indexing", "end");
    let _ = client.upstream_methods_seen();
    assert_eq!(
        client.server_state().readiness,
        Readiness::Initializing,
        "the outer SlowPathBlocking that established identity is still open"
    );
    sorbet_operation(&mut client, "SlowPathBlocking", "end");
    let state = client.await_state_changed();
    assert_eq!(state.readiness, Readiness::Ready);
    assert_eq!(state.health, Health::Unknown);
    client.shutdown();
}

#[test]
fn sorbet_becomes_ready_only_once_nested_operations_all_end() {
    let (mut client, _) = sorbet_client();
    sorbet_operation(&mut client, "SlowPathBlocking", "start");
    let _ = client.upstream_methods_seen();
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    sorbet_operation(&mut client, "Indexing", "start");
    let _ = client.upstream_methods_seen();
    assert_eq!(
        client.server_state().readiness,
        Readiness::Initializing,
        "still the first round: a nested start does not move past initializing"
    );
    sorbet_operation(&mut client, "Indexing", "end");
    let _ = client.upstream_methods_seen();
    assert_eq!(
        client.server_state().readiness,
        Readiness::Initializing,
        "the outer SlowPathBlocking is still open"
    );
    sorbet_operation(&mut client, "SlowPathBlocking", "end");
    let state = client.await_state_changed();
    assert_eq!(state.readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn sorbet_reindexes_on_a_later_round() {
    let (mut client, _) = sorbet_client();
    sorbet_operation(&mut client, "Indexing", "start");
    let _ = client.upstream_methods_seen();
    sorbet_operation(&mut client, "Indexing", "end");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);

    sorbet_operation(&mut client, "SlowPathNonBlocking", "start");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    sorbet_operation(&mut client, "SlowPathNonBlocking", "end");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn sorbet_ignores_request_tied_operations() {
    let (mut client, _) = sorbet_client();
    sorbet_operation(&mut client, "Indexing", "start");
    let _ = client.upstream_methods_seen();
    sorbet_operation(&mut client, "Indexing", "end");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);

    for name in ["References", "SymbolSearch", "Rename", "MoveMethod"] {
        sorbet_operation(&mut client, name, "start");
        assert!(
            client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
            "moved state on a request-tied operation's start: {name}"
        );
        sorbet_operation(&mut client, name, "end");
        assert!(
            client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
            "moved state on a request-tied operation's end: {name}"
        );
    }
    client.shutdown();
}

#[test]
fn sorbet_does_not_predict_from_document_or_watched_file_changes() {
    let (mut client, _) = sorbet_client();
    sorbet_operation(&mut client, "Indexing", "start");
    let _ = client.upstream_methods_seen();
    sorbet_operation(&mut client, "Indexing", "end");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);

    client.did_change(
        &std::path::PathBuf::from("/fake/lib/a.rb"),
        2,
        "# typed: true\nmodule Lib\nend\n",
    );
    client.did_change_watched_files(&[(&std::path::PathBuf::from("/fake/lib/b.rb"), 2)]);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "predicted a state change from a didChange or a watched-file change; the server itself \
         holds a request until the operation it depends on ends, so there is nothing to predict"
    );
    client.shutdown();
}

#[test]
fn sorbet_spec_8_2_5_declares_no_guarantees() {
    let server = ServerUnderTest::lsp_det_with_fake_sorbet();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee Sorbet's vocabulary cannot keep (the version never appears in the \
         protocol): {result}"
    );
    client.shutdown();
}

/// Gate (spec chapter 9) holds this on the client's behalf because it does not declare the
/// protocol itself; the readiness that drives the hold comes from the Sorbet mapping's
/// `sorbet/showOperation` tracking. `--references-depend-on-readiness` makes the fake
/// upstream's own answer depend on ITS OWN state (default "ready"), so a non-empty answer
/// arriving only after the matching end shows the request really was held, not merely delayed.
#[test]
fn sorbet_holds_references_until_ready() {
    let server =
        ServerUnderTest::lsp_det_with_upstream_flags("none", &["--references-depend-on-readiness"]);
    let mut client = ConformanceClient::start(&server);
    client.initialize(false);
    // Readiness is `unknown` (Gate forwards rather than holding, ADR 0008) until Sorbet is
    // identified; the hold only starts once readiness becomes a known non-ready value.
    sorbet_identify(&mut client);
    let id = client.send_references();
    assert!(
        client.response_within(id, NEGATIVE_WINDOW).is_none(),
        "forwarded references while initializing"
    );
    sorbet_operation(&mut client, "Indexing", "start");
    assert!(
        client.response_within(id, NEGATIVE_WINDOW).is_none(),
        "forwarded references while indexing"
    );
    sorbet_operation(&mut client, "Indexing", "end");
    let response = client.await_response_to(id);
    assert!(
        !response["result"]
            .as_array()
            .expect("references answers an array")
            .is_empty(),
        "did not release the hold once ready: {response}"
    );
    client.shutdown();
}

// --- initializationOptions injection (ADR 0020 decision D) --------------------------------

#[test]
fn sorbet_injects_the_operation_notifications_opt_in_when_launched_as_sorbet() {
    let server = ServerUnderTest::lsp_det_with_fake_upstream_named("sorbet");
    let mut client = ConformanceClient::start(&server);
    // The client does not pass initializationOptions itself: the injection is what makes the
    // option appear on the upstream side (ADR 0020 decision D).
    client.initialize(true);
    assert_eq!(
        client.upstream_initialization_options()["supportsOperationNotifications"],
        json!(true),
        "did not inject the opt-in for a command named \"sorbet\""
    );
    client.shutdown();
}

#[test]
fn sorbet_injects_the_operation_notifications_opt_in_when_launched_as_srb() {
    let server = ServerUnderTest::lsp_det_with_fake_upstream_named("srb");
    let mut client = ConformanceClient::start(&server);
    client.initialize(true);
    assert_eq!(
        client.upstream_initialization_options()["supportsOperationNotifications"],
        json!(true),
        "did not inject the opt-in for a command named \"srb\""
    );
    client.shutdown();
}

#[test]
fn sorbet_does_not_inject_for_an_upstream_command_with_a_different_name() {
    let server = ServerUnderTest::lsp_det_with_fake_upstream_named("not-sorbet");
    let mut client = ConformanceClient::start(&server);
    client.initialize(true);
    assert_eq!(
        client.upstream_initialization_options(),
        json!(null),
        "injected the opt-in for a command lsp-det did not launch as sorbet or srb"
    );
    client.shutdown();
}
// --- real Sorbet (local only) ---------------------------------------------

/// Caller files under `lib/` (the method section of research/sorbet-readiness-measurement.md).
const SORBET_FIXTURE_CALLERS: usize = 600;
/// The count `textDocument/references` on `Lib.target` answers with, even with
/// `includeDeclaration: false`: real Sorbet returns the `def self.target` site itself
/// regardless of the flag (measured), on top of the one call in each of the caller files.
const SORBET_EXPECTED_REFERENCES: usize = SORBET_FIXTURE_CALLERS + 1;

fn real_sorbet(project: &support::TempSorbetProject) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec![
            "--".to_string(),
            // No directory argument: `sorbet/config` supplies it, and passing one too makes
            // Sorbet exit with "requires a single input directory" (measured).
            "sorbet".to_string(),
            "--lsp".to_string(),
            "--disable-watchman".to_string(),
        ],
        root: project.root.clone(),
    }
}

/// Identity/selection and the guarantee declared (always none, ADR 0020 decision E: the
/// version never appears in the protocol). The test client does not pass
/// `initializationOptions` itself, so `sorbet/showOperation` -- and therefore identity and
/// every readiness notification -- only appears because lsp-det injects the opt-in for the
/// `sorbet` command it launched (ADR 0020 decision D). `sorbet` is the real binary's own
/// basename, so this is also the decision D injection exercised end to end.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn sorbet_is_selected_by_its_real_notifications() {
    let project = support::TempSorbetProject::with_many_callers("select", 1);
    let mut client = ConformanceClient::start(&real_sorbet(&project));
    let result = client.initialize_with_root(true, &project.root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "no guarantee can be declared for a server whose version never appears in the protocol: \
         {result}"
    );
    // Sorbet identifies itself only through `sorbet/showOperation` sent after `initialized`
    // (research/sorbet-readiness-measurement.md), an unavoidable round trip a bare
    // `experimental/serverState` request can race ahead of.
    poll_state_until(&mut client, |s| s.readiness != Readiness::Unknown);
    client.wait_until_ready();
    client.shutdown();
}

/// 7.1: the first `references` from `a.rb`, sent right after `ready`, is complete (research:
/// Sorbet holds a cross-file request until Idle, so there is no empty or partial answer to
/// observe even sent right after `ready` is first reached).
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn sorbet_spec_7_1_first_references_is_complete_through_lsp_det_with_real_sorbet() {
    let project =
        support::TempSorbetProject::with_many_callers("readiness", SORBET_FIXTURE_CALLERS);
    let a = project.file("lib/a.rb");
    let mut client = ConformanceClient::start(&real_sorbet(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&a, "ruby");
    // See sorbet_is_selected_by_its_real_notifications: identification is a round trip a bare
    // state request can race ahead of.
    poll_state_until(&mut client, |s| s.readiness != Readiness::Unknown);
    client.wait_until_ready();

    let (line, character) = support::SORBET_TARGET_DECLARATION;
    let found = client.references(&a, line, character);
    assert_eq!(
        found.len(),
        SORBET_EXPECTED_REFERENCES,
        "the first references was not complete: {} of {}",
        found.len(),
        SORBET_EXPECTED_REFERENCES
    );
    client.shutdown();
}

/// 7.2: the result once `ready` matches the precomputed complete set (every caller file calls
/// `Lib.target` exactly once).
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn sorbet_spec_7_2_coverage_through_lsp_det_with_real_sorbet() {
    let project = support::TempSorbetProject::with_many_callers("coverage", SORBET_FIXTURE_CALLERS);
    let a = project.file("lib/a.rb");
    let mut client = ConformanceClient::start(&real_sorbet(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&a, "ruby");
    // See sorbet_is_selected_by_its_real_notifications: identification is a round trip a bare
    // state request can race ahead of.
    poll_state_until(&mut client, |s| s.readiness != Readiness::Unknown);
    client.wait_until_ready();

    let (line, character) = support::SORBET_TARGET_DECLARATION;
    let found = client.references(&a, line, character);
    assert_eq!(
        found.len(),
        SORBET_EXPECTED_REFERENCES,
        "missed some callers while declaring ready (completeness violation): {} of {}",
        found.len(),
        SORBET_EXPECTED_REFERENCES
    );
    client.shutdown();
}

/// 7.3 item 1: a `didChange` on an open caller file that adds one more call to `Lib.target` is
/// incorporated. Items 2-4 (watched-file Created / Changed / Deleted) need watchman with a
/// pre-existing `watch-project` on this root: Sorbet's own `subscribe` never issues one
/// (research), and the test environment does not run a watchman daemon that has already
/// watched the temporary project root, so they are not exercised here.
#[test]
#[ignore = "Real server integration. Local only (v0.1-design.md chapter 6). Run with cargo test -- --ignored"]
fn sorbet_spec_7_3_1_did_change_on_an_open_file_through_lsp_det_with_real_sorbet() {
    let project = support::TempSorbetProject::with_many_callers(
        "freshness-didchange",
        SORBET_FIXTURE_CALLERS,
    );
    let a = project.file("lib/a.rb");
    let f0 = project.file("lib/f0.rb");
    let mut client = ConformanceClient::start(&real_sorbet(&project));
    client.initialize_with_root(true, &project.root);
    client.did_open(&a, "ruby");
    client.did_open(&f0, "ruby");
    // See sorbet_is_selected_by_its_real_notifications: identification is a round trip a bare
    // state request can race ahead of.
    poll_state_until(&mut client, |s| s.readiness != Readiness::Unknown);
    client.wait_until_ready();

    let (line, character) = support::SORBET_TARGET_DECLARATION;
    let before = client.references(&a, line, character).len();
    client.did_change(&f0, 2, &support::sorbet_caller_file_with_calls(0, 2));
    let after = client.references(&a, line, character).len();
    assert_eq!(
        after,
        before + 1,
        "an added call in an open file was not incorporated: before={before} after={after}"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// clangd (M24, ADR 0020 decision C row for clangd). The mapping in
// research/clangd-readiness-measurement.md: identified by `serverInfo.name` "clangd"
// (case-insensitive; the version is the whole `serverInfo.version` string). `$/progress`
// (fixed token "backgroundIndexProgress", title "indexing") begin -> `indexing`, `report`
// ignored, end -> `ready`, repeated whenever the background index queue goes from empty to
// non-empty again. Without a compilation database the token never arrives at all, so this
// mapping's starting `initializing` never moves (decision (a), ADR 0020 addendum). No health
// signal. `coverage: {scope: "workspace", incomplete: {}}` only -- no `freshness` -- is
// declared for the tested version (spec 8.2 item 5; the real-server tests below are the
// basis).
// ---------------------------------------------------------------------------

const CLANGD_BACKGROUND_INDEX_TOKEN: &str = "backgroundIndexProgress";

fn clangd_client() -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_clangd();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    (client, result)
}

/// Emits the fixed-token background-index progress a real clangd sends
/// (research/clangd-readiness-measurement.md).
fn clangd_progress(client: &mut ConformanceClient, kind: &str) {
    let mut value = json!({"kind": kind});
    if kind == "begin" {
        value["title"] = json!("indexing");
    }
    client.make_upstream_emit_progress(
        json!({"token": CLANGD_BACKGROUND_INDEX_TOKEN, "value": value}),
    );
}

#[test]
fn clangd_is_selected_by_server_info_and_declares_a_guarantee_for_the_tested_version() {
    let (mut client, result) = clangd_client();
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"coverage": {"scope": "workspace", "incomplete": {}}}),
        "declared a different guarantee for the tested version: {result}"
    );
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    client.shutdown();
}

#[test]
fn clangd_declares_no_guarantee_for_an_untested_version() {
    let server = ServerUnderTest::lsp_det_with_upstream_flags(
        "clangd",
        &[
            "--server-version",
            "clangd version 20.1.0 linux x86_64-unknown-linux-gnu",
        ],
    );
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({}),
        "declared a guarantee for a version the conformance suite has not passed on: {result}"
    );
    client.shutdown();
}

#[test]
fn clangd_becomes_ready_at_the_end_of_the_first_indexing_round() {
    let (mut client, _) = clangd_client();
    clangd_progress(&mut client, "begin");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    clangd_progress(&mut client, "end");
    let state = client.await_state_changed();
    assert_eq!(state.readiness, Readiness::Ready);
    assert_eq!(state.health, Health::Unknown, "clangd has no health signal");
    client.shutdown();
}

#[test]
fn clangd_report_is_ignored() {
    let (mut client, _) = clangd_client();
    clangd_progress(&mut client, "begin");
    client.await_state_changed();
    client.make_upstream_emit_progress(json!({
        "token": CLANGD_BACKGROUND_INDEX_TOKEN,
        "value": {"kind": "report", "message": "117/402"}
    }));
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "moved state on a report"
    );
    assert_eq!(client.server_state().readiness, Readiness::Indexing);
    client.shutdown();
}

#[test]
fn clangd_reindexes_on_every_later_round() {
    let (mut client, _) = clangd_client();
    clangd_progress(&mut client, "begin");
    client.await_state_changed();
    clangd_progress(&mut client, "end");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    // A later round: the background index queue went from empty to non-empty again.
    clangd_progress(&mut client, "begin");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    clangd_progress(&mut client, "end");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn clangd_ignores_progress_of_other_tokens() {
    let (mut client, _) = clangd_client();
    client.make_upstream_emit_progress(json!({
        "token": "some-other-token",
        "value": {"kind": "begin", "title": "Something else"}
    }));
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "moved state on an unrelated progress token"
    );
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    client.shutdown();
}

#[test]
fn clangd_does_not_predict_from_document_or_watched_file_changes() {
    let (mut client, _) = clangd_client();
    clangd_progress(&mut client, "begin");
    client.await_state_changed();
    clangd_progress(&mut client, "end");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.did_change(
        &std::path::PathBuf::from("/fake/f0.cpp"),
        2,
        "int use0() { return target(); }\n",
    );
    client.did_change_watched_files(&[(&std::path::PathBuf::from("/fake/f1.cpp"), 2)]);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "predicted a state change from a didChange or a watched-file change; neither a \
         didChange's completion nor an on-disk change has a signal (research doc, runs 2-6)"
    );
    client.shutdown();
}

/// Gate (spec chapter 9) holds this on the client's behalf because it does not declare the
/// protocol itself; the readiness that drives the hold comes from the clangd mapping's
/// `$/progress` tracking. `--references-depend-on-readiness` makes the fake upstream's own
/// answer depend on ITS OWN state (default "ready"), so a non-empty answer arriving only after
/// `end` shows the request really was held, not merely delayed.
#[test]
fn clangd_holds_references_until_ready() {
    let server = ServerUnderTest::lsp_det_with_upstream_flags(
        "clangd",
        &[
            "--server-version",
            support::CLANGD_TESTED_VERSION,
            "--references-depend-on-readiness",
        ],
    );
    let mut client = ConformanceClient::start(&server);
    client.initialize(false);
    let id = client.send_references();
    assert!(
        client.response_within(id, NEGATIVE_WINDOW).is_none(),
        "forwarded references before the first indexing round even started"
    );
    clangd_progress(&mut client, "begin");
    assert!(
        client.response_within(id, NEGATIVE_WINDOW).is_none(),
        "forwarded references while indexing"
    );
    clangd_progress(&mut client, "end");
    let response = client.await_response_to(id);
    assert!(
        !response["result"]
            .as_array()
            .expect("references answers an array")
            .is_empty(),
        "did not release the hold once ready: {response}"
    );
    client.shutdown();
}
