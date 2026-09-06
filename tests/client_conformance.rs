//! The conformance test suite for the downstream side (docs/spec/server-state.md 9.1).
//!
//! Spec chapter 9 "recommended behavior for clients" made executable. The subject is "a client
//! that faces a server conformant to this protocol", and the downstream side of lsp-det is the
//! first subject. When Claude Code or Serena support it natively in the future, the subject is
//! swapped under the same requirements (v0.1-design.md chapter 6).
//!
//! For lsp-det, the state above the boundary has 2 sources. If the upstream declares on its own,
//! it comes from the upstream's notifications (the upstream side is the identity mapping);
//! otherwise it comes from the mapping. The downstream side's behavior must be the same either
//! way, so both are made subjects.

mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::{ConformanceClient, ServerUnderTest};

/// The observation window when checking "that something does not arrive".
const NEGATIVE_WINDOW: Duration = Duration::from_millis(750);

/// The 2 kinds of subject, which differ in how the state above the boundary is moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Subject {
    /// A conformant fake upstream + lsp-det (the upstream side is the identity mapping).
    ConformantUpstream,
    /// A fake upstream that calls itself rust-analyzer + lsp-det (the mapping produces the
    /// state).
    MappedUpstream,
}

const SUBJECTS: [Subject; 2] = [Subject::ConformantUpstream, Subject::MappedUpstream];

/// Launches the subject and puts it in the `indexing` state right after `initialize`.
fn start_indexing(subject: Subject, client_declares: bool) -> ConformanceClient {
    let server = match subject {
        Subject::ConformantUpstream => ServerUnderTest::lsp_det_with_conformant_upstream_flags(&[
            "--initial-readiness",
            "indexing",
        ]),
        Subject::MappedUpstream => ServerUnderTest::lsp_det_with_fake_upstream(),
    };
    let mut client = ConformanceClient::start(&server);
    client.initialize(client_declares);
    if subject == Subject::MappedUpstream {
        // The mapping is initializing until the first serverStatus. Advance it to indexing.
        client.make_upstream_emit_status("ok", false);
    }
    client
}

fn make_ready(client: &mut ConformanceClient, subject: Subject) {
    match subject {
        Subject::ConformantUpstream => {
            client.make_upstream_emit_server_state_changed("ok", "ready")
        }
        Subject::MappedUpstream => client.make_upstream_emit_status("ok", true),
    }
}

fn make_error(client: &mut ConformanceClient, subject: Subject) {
    match subject {
        Subject::ConformantUpstream => {
            client.make_upstream_emit_server_state_changed("error", "indexing")
        }
        Subject::MappedUpstream => client.make_upstream_emit_status("error", true),
    }
}

fn saw_upstream(client: &mut ConformanceClient, method: &str) -> bool {
    client.upstream_methods_seen().iter().any(|m| m == method)
}

/// A synchronization point that waits until the upstream's state change reaches lsp-det. The
/// fake upstream sends the notification before answering the next request, so once a round-trip
/// request to the upstream returns, lsp-det has processed the notification it was made to send
/// before that.
fn sync_with_upstream(client: &mut ConformanceClient) {
    let _ = client.upstream_methods_seen();
}

// ---------------------------------------------------------------------------
// 9.1 item 1: cross-workspace requests do not reach the upstream during indexing, and reach it
// after ready
// ---------------------------------------------------------------------------

#[test]
fn spec_9_1_1_holds_cross_workspace_requests_until_ready() {
    for subject in SUBJECTS {
        let mut client = start_indexing(subject, false);
        let id = client.send_references();
        assert!(
            client.response_within(id, NEGATIVE_WINDOW).is_none(),
            "{subject:?}: references was answered during indexing"
        );
        assert!(
            !saw_upstream(&mut client, "textDocument/references"),
            "{subject:?}: references reached the upstream during indexing"
        );

        make_ready(&mut client, subject);
        let response = client.await_response_to(id);
        assert!(
            response.get("result").is_some(),
            "{subject:?}: references after ready is not a success response: {response}"
        );
        assert!(
            saw_upstream(&mut client, "textDocument/references"),
            "{subject:?}: references has not reached the upstream even after ready"
        );
        client.shutdown();
    }
}

#[test]
fn spec_9_1_1_releases_held_requests_in_order() {
    for subject in SUBJECTS {
        let mut client = start_indexing(subject, false);
        let first = client.send_references();
        let second = client.send_references();
        make_ready(&mut client, subject);
        client.await_response_to(first);
        client.await_response_to(second);
        let seen = client.upstream_methods_seen();
        let positions: Vec<usize> = seen
            .iter()
            .enumerate()
            .filter(|(_, m)| *m == "textDocument/references")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            positions.len(),
            2,
            "{subject:?}: both of the 2 should reach the upstream: {seen:?}"
        );
        client.shutdown();
    }
}

// ---------------------------------------------------------------------------
// 9.1 item 2: fails without waiting if health is error
// ---------------------------------------------------------------------------

#[test]
fn spec_9_1_2_fails_fast_when_health_is_error() {
    for subject in SUBJECTS {
        let mut client = start_indexing(subject, false);
        make_error(&mut client, subject);
        sync_with_upstream(&mut client);
        assert_eq!(client.server_state().health, lsp_det::state::Health::Error);

        let id = client.send_references();
        let response = client
            .response_within(id, Duration::from_secs(5))
            .unwrap_or_else(|| panic!("{subject:?}: references was made to wait despite error"));
        assert!(
            response.get("error").is_some(),
            "{subject:?}: references under error is not a failure response: {response}"
        );
        assert!(
            !saw_upstream(&mut client, "textDocument/references"),
            "{subject:?}: references reached the upstream despite error"
        );
        client.shutdown();
    }
}

#[test]
fn spec_9_1_2_fails_held_requests_when_health_turns_error() {
    for subject in SUBJECTS {
        let mut client = start_indexing(subject, false);
        let id = client.send_references();
        assert!(client.response_within(id, NEGATIVE_WINDOW).is_none());

        make_error(&mut client, subject);
        let response = client.await_response_to(id);
        assert!(
            response.get("error").is_some(),
            "{subject:?}: the held request is not a failure response even though health became \
             error: {response}"
        );
        client.shutdown();
    }
}

#[test]
fn returns_to_holding_after_the_error_recovers() {
    // error can recover (design 4.3). Cross-workspace requests after recovery wait again.
    let subject = Subject::MappedUpstream;
    let mut client = start_indexing(subject, false);
    make_error(&mut client, subject);
    sync_with_upstream(&mut client);
    assert_eq!(client.server_state().health, lsp_det::state::Health::Error);

    client.make_upstream_emit_status("ok", false);
    sync_with_upstream(&mut client);
    assert_eq!(client.server_state().health, lsp_det::state::Health::Ok);
    let id = client.send_references();
    assert!(
        client.response_within(id, NEGATIVE_WINDOW).is_none(),
        "references was not made to wait during indexing after recovery"
    );
    make_ready(&mut client, subject);
    assert!(client.await_response_to(id).get("result").is_some());
    client.shutdown();
}

// ---------------------------------------------------------------------------
// 9.1 item 3: does not wait if readiness is unknown
// ---------------------------------------------------------------------------

#[test]
fn spec_9_1_3_does_not_hold_when_readiness_is_unknown() {
    // An upstream with no known mapping. Both axes are unknown, and there is no signal to wait
    // for.
    let server = ServerUnderTest::lsp_det_without_adapter();
    let mut client = ConformanceClient::start(&server);
    client.initialize(false);
    let id = client.send_references();
    let response = client
        .response_within(id, Duration::from_secs(5))
        .expect("references was made to wait despite unknown");
    assert!(response.get("result").is_some(), "{response}");
    client.shutdown();
}

// ---------------------------------------------------------------------------
// 9.1 item 4: everything other than cross-workspace passes through even during indexing
// ---------------------------------------------------------------------------

#[test]
fn spec_9_1_4_forwards_single_file_requests_while_indexing() {
    for subject in SUBJECTS {
        let mut client = start_indexing(subject, false);
        let id = client.send_hover();
        let response = client
            .response_within(id, Duration::from_secs(5))
            .unwrap_or_else(|| panic!("{subject:?}: hover was made to wait during indexing"));
        assert!(response.get("result").is_some(), "{subject:?}: {response}");
        client.shutdown();
    }
}

// ---------------------------------------------------------------------------
// 9.1 item 5: on receiving cancel / shutdown while standing in, answers all held requests
// ---------------------------------------------------------------------------

#[test]
fn spec_9_1_5_answers_a_held_request_on_cancel() {
    for subject in SUBJECTS {
        let mut client = start_indexing(subject, false);
        let id = client.send_references();
        assert!(client.response_within(id, NEGATIVE_WINDOW).is_none());

        client.cancel(id);
        let response = client.await_response_to(id);
        assert_eq!(
            response["error"]["code"],
            json!(-32800),
            "{subject:?}: the canceled held request is not RequestCancelled: {response}"
        );
        // A canceled request is not forwarded to the upstream even after ready.
        make_ready(&mut client, subject);
        assert!(
            !saw_upstream(&mut client, "textDocument/references"),
            "{subject:?}: the canceled references reached the upstream"
        );
        client.shutdown();
    }
}

#[test]
fn spec_9_1_5_answers_held_requests_on_shutdown() {
    for subject in SUBJECTS {
        let mut client = start_indexing(subject, false);
        let held = client.send_references();
        assert!(client.response_within(held, NEGATIVE_WINDOW).is_none());

        let shutdown = client.send_request("shutdown", json!(null));
        let response = client.await_response_to(held);
        assert!(
            response.get("error").is_some(),
            "{subject:?}: the held request at shutdown is not a failure response: {response}"
        );
        let shutdown_response = client.await_response_to(shutdown);
        assert!(
            shutdown_response.get("error").is_none(),
            "{subject:?}: shutdown itself failed: {shutdown_response}"
        );
        client.notify("exit", json!(null));
    }
}

#[test]
fn spec_9_1_5_answers_held_requests_when_the_upstream_exits() {
    // Design 4.2 "loss of the upstream": answers the held requests with an error, then closes the
    // connection.
    for subject in SUBJECTS {
        let mut client = start_indexing(subject, false);
        let held = client.send_references();
        assert!(client.response_within(held, NEGATIVE_WINDOW).is_none());

        // The fake upstream exits on the exit notification (even without shutdown).
        client.notify("exit", json!(null));
        let response = client.await_response_to(held);
        assert!(
            response.get("error").is_some(),
            "{subject:?}: the held request on loss of the upstream is not a failure response: \
             {response}"
        );
    }
}

// ---------------------------------------------------------------------------
// Observability of holding (ADR 0018 decision A-1): the start and the end of every hold are on
// stderr, with the reason, so that a mapping that missed a signal shows up as "still holding"
// rather than as a client-side timeout
// ---------------------------------------------------------------------------

#[test]
fn logs_the_start_and_the_release_of_a_hold_to_stderr() {
    for subject in SUBJECTS {
        let mut client = start_indexing(subject, false);
        let id = client.send_references();
        make_ready(&mut client, subject);
        client.await_response_to(id);
        client.shutdown();
        let log = client.stderr_after_exit();
        assert!(
            log.contains(&format!("holding textDocument/references (id {id})")),
            "{subject:?}: the start of the hold is not on stderr:\n{log}"
        );
        let released = log
            .lines()
            .find(|l| l.contains(&format!("released textDocument/references (id {id})")));
        assert!(
            released.is_some_and(|l| l.contains("ready")),
            "{subject:?}: the release and its reason are not on stderr:\n{log}"
        );
    }
}

#[test]
fn logs_the_reason_when_a_held_request_does_not_reach_the_upstream() {
    // Rejected because health turned error.
    for subject in SUBJECTS {
        let mut client = start_indexing(subject, false);
        let id = client.send_references();
        make_error(&mut client, subject);
        client.await_response_to(id);
        client.shutdown();
        let log = client.stderr_after_exit();
        let rejected = log
            .lines()
            .find(|l| l.contains(&format!("rejected textDocument/references (id {id})")));
        assert!(
            rejected.is_some_and(|l| l.contains("error")),
            "{subject:?}: the rejection and its reason are not on stderr:\n{log}"
        );
    }
    // Cancelled by the client.
    let mut client = start_indexing(Subject::ConformantUpstream, false);
    let id = client.send_references();
    client.cancel(id);
    client.await_response_to(id);
    client.shutdown();
    let log = client.stderr_after_exit();
    assert!(
        log.lines()
            .any(|l| l.contains(&format!("cancelled textDocument/references (id {id})"))),
        "the cancellation is not on stderr:\n{log}"
    );
    // Answered with an error because of shutdown.
    let mut client = start_indexing(Subject::ConformantUpstream, false);
    let id = client.send_references();
    client.shutdown();
    let log = client.stderr_after_exit();
    assert!(
        log.lines().any(
            |l| l.contains(&format!("textDocument/references (id {id})")) && l.contains("shutdown")
        ),
        "the error answer on shutdown is not on stderr:\n{log}"
    );
}

// ---------------------------------------------------------------------------
// 7.2 coverage, run through the downstream side + the fake upstream
//
// The fake upstream returns an empty array for references while indexing is incomplete (a silent
// lie). The downstream side makes it wait until ready, so only complete results reach the client.
// ---------------------------------------------------------------------------

#[test]
fn spec_7_2_coverage_through_the_downstream_side_with_a_fake_upstream() {
    let subjects = [
        ServerUnderTest::lsp_det_with_conformant_upstream_flags(&[
            "--initial-readiness",
            "indexing",
            "--references-depend-on-readiness",
        ]),
        ServerUnderTest::lsp_det_with_fake_upstream_flags(&["--references-depend-on-readiness"]),
    ];
    for (i, server) in subjects.into_iter().enumerate() {
        let subject = if i == 0 {
            Subject::ConformantUpstream
        } else {
            Subject::MappedUpstream
        };
        let mut client = ConformanceClient::start(&server);
        client.initialize(false);
        if subject == Subject::MappedUpstream {
            client.make_upstream_emit_status("ok", false);
        }

        let id = client.send_references();
        assert!(client.response_within(id, NEGATIVE_WINDOW).is_none());
        make_ready(&mut client, subject);
        let response = client.await_response_to(id);
        let found = response["result"].as_array().cloned().unwrap_or_default();
        assert!(
            found.iter().any(|l| l["range"]["start"]["line"] == 3),
            "{subject:?}: an empty response from incomplete indexing reached the client: {response}"
        );
        client.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Spec 5.2: does not stand in for a client that declared
// ---------------------------------------------------------------------------

#[test]
fn does_not_hold_when_the_client_declared_server_state() {
    for subject in SUBJECTS {
        let mut client = start_indexing(subject, true);
        let id = client.send_references();
        let response = client
            .response_within(id, Duration::from_secs(5))
            .unwrap_or_else(|| {
                panic!("{subject:?}: references from a client that declared was made to wait")
            });
        assert!(response.get("result").is_some(), "{subject:?}: {response}");
        client.shutdown();
    }
}

// ---------------------------------------------------------------------------
// How the state at the boundary is read under the identity mapping (design 4.1, 4.2)
// ---------------------------------------------------------------------------

#[test]
fn upstream_notifications_are_not_forwarded_to_a_client_that_did_not_declare() {
    // The upstream is made to emit the notification so the downstream side can read it, but it is
    // not forwarded to a client that did not declare (spec 5.2).
    let mut client = start_indexing(Subject::ConformantUpstream, false);
    client.make_upstream_emit_server_state_changed("ok", "ready");
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "forwarded the upstream's serverStateChanged to a client that did not declare"
    );
    client.shutdown();
}

#[test]
fn upstream_notifications_are_forwarded_to_a_client_that_declared() {
    let mut client = start_indexing(Subject::ConformantUpstream, true);
    client.make_upstream_emit_server_state_changed("ok", "ready");
    let state = client.await_state_changed();
    assert_eq!(state.readiness, lsp_det::state::Readiness::Ready);
    client.shutdown();
}

#[test]
fn the_initial_state_of_a_conformant_upstream_is_read_by_asking_it() {
    // The upstream's notification comes only on a change. lsp-det obtains the initial state by
    // asking on its own. The response to that request is not visible to the client.
    let mut client = start_indexing(Subject::ConformantUpstream, false);
    assert!(
        saw_upstream(&mut client, "experimental/serverState"),
        "lsp-det did not ask the upstream for the initial state"
    );
    let id = client.send_references();
    assert!(
        client.response_within(id, NEGATIVE_WINDOW).is_none(),
        "the initial state (indexing) was not read, and references was not made to wait"
    );
    client.cancel(id);
    client.await_response_to(id);
    client.shutdown();
}

/// Avoids an unused warning (which helpers are used differs per subject).
#[allow(dead_code)]
fn _unused(_: Value) {}

// ---------------------------------------------------------------------------
// 9.1 item 1 (ADR 0014): also holds during reindexing that started from a notification
// ---------------------------------------------------------------------------

#[test]
fn spec_9_1_1_holds_while_reindexing_after_watched_file_changes() {
    let server = ServerUnderTest::lsp_det_with_fake_upstream_flags(&[
        "--references-depend-on-readiness",
        "--reindex-on-watched-files",
    ]);
    let mut client = ConformanceClient::start(&server);
    client.initialize(false);
    client.make_upstream_emit_status("ok", true);
    sync_with_upstream(&mut client);

    let root = support::repo_root();
    client.did_change_watched_files(&[(&root.join("src/c.rs"), 1)]);
    sync_with_upstream(&mut client);

    let id = client.send_references();
    assert!(
        client.response_within(id, NEGATIVE_WINDOW).is_none(),
        "references was answered during reindexing that started from a notification"
    );

    client.make_upstream_emit_status("ok", true);
    let response = client.await_response_to(id);
    let locations = response["result"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        locations.len(),
        2,
        "the response after ready does not incorporate the notified change: {response}"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// ADR 0015: the downstream side's stand-ins (outside the protocol)
//
// (A) On behalf of a client that neither declares the capability nor sends the notification,
//     compares the mtimes of the git ls-files listing before a 7.0 request and sends
//     workspace/didChangeWatchedFiles
// (B) Rewrites a didOpen for an already open uri into a full-text didChange
// ---------------------------------------------------------------------------

/// The subject for the stand-ins: a fake upstream that calls itself rust-analyzer, made ready.
fn ready_client_in(root: &std::path::Path, capabilities: Value) -> ConformanceClient {
    let mut client = ConformanceClient::start(&ServerUnderTest::lsp_det_with_fake_upstream());
    client.initialize_with_root_and_capabilities(root, capabilities);
    client.make_upstream_emit_status("ok", true);
    sync_with_upstream(&mut client);
    client
}

fn plain_capabilities() -> Value {
    json!({"textDocument": {"hover": {}}})
}

fn watching_capabilities() -> Value {
    json!({"textDocument": {"hover": {}}, "workspace": {"didChangeWatchedFiles": {"dynamicRegistration": true}}})
}

fn changes_of(notification: &Value) -> Vec<(String, u64)> {
    notification["changes"]
        .as_array()
        .map(|changes| {
            changes
                .iter()
                .map(|c| {
                    (
                        c["uri"].as_str().unwrap_or("").to_string(),
                        c["type"].as_u64().unwrap_or(0),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn stands_in_for_watched_files_before_a_cross_workspace_request() {
    let ws = support::TempGitWorkspace::new("watched");
    let mut client = ready_client_in(&ws.root, plain_capabilities());
    let a = ws.file("a.rs");
    let b = ws.file("b.rs");

    // Changes after launch: rewrite a.rs and create b.rs (open neither).
    std::fs::write(&a, "pub fn target() {}\npub fn more() {}\n").unwrap();
    std::fs::write(&b, "pub fn other() {}\n").unwrap();

    let id = client.send_references();
    client.await_response_to(id);

    let seen = client.upstream_methods_seen();
    let watched = seen
        .iter()
        .position(|m| m == "workspace/didChangeWatchedFiles");
    let references = seen.iter().position(|m| m == "textDocument/references");
    assert!(
        matches!((watched, references), (Some(w), Some(r)) if w < r),
        "the stand-in notification did not reach the upstream before references: {seen:?}"
    );
    let notifications = client.upstream_notifications("workspace/didChangeWatchedFiles");
    assert_eq!(
        notifications.len(),
        1,
        "the notifications are combined into 1: {notifications:#?}"
    );
    let mut changes = changes_of(&notifications[0]);
    changes.sort();
    assert_eq!(
        changes,
        vec![(support::file_uri(&a), 2), (support::file_uri(&b), 1)],
        "Changed and Created arrive with uri and type"
    );

    // No notification if nothing changed.
    let id = client.send_references();
    client.await_response_to(id);
    assert_eq!(
        client
            .upstream_notifications("workspace/didChangeWatchedFiles")
            .len(),
        1,
        "notified although nothing changed"
    );

    // Deleted.
    std::fs::remove_file(&b).unwrap();
    let id = client.send_references();
    client.await_response_to(id);
    let notifications = client.upstream_notifications("workspace/didChangeWatchedFiles");
    assert_eq!(
        changes_of(&notifications[1]),
        vec![(support::file_uri(&b), 3)]
    );
    client.shutdown();
}

#[test]
fn does_not_stand_in_when_the_client_declares_watched_files() {
    let ws = support::TempGitWorkspace::new("declared");
    let mut client = ready_client_in(&ws.root, watching_capabilities());
    std::fs::write(ws.file("b.rs"), "pub fn other() {}\n").unwrap();
    let id = client.send_references();
    client.await_response_to(id);
    assert!(
        client
            .upstream_notifications("workspace/didChangeWatchedFiles")
            .is_empty(),
        "stood in for a client that declared"
    );
    client.shutdown();
}

#[test]
fn stops_standing_in_once_the_client_sends_its_own_notification() {
    let ws = support::TempGitWorkspace::new("own");
    let mut client = ready_client_in(&ws.root, plain_capabilities());
    // The client sends it itself (Serena sends without declaring).
    client.did_change_watched_files(&[(&ws.file("a.rs"), 2)]);
    std::fs::write(ws.file("b.rs"), "pub fn other() {}\n").unwrap();
    let id = client.send_references();
    client.await_response_to(id);
    let notifications = client.upstream_notifications("workspace/didChangeWatchedFiles");
    assert_eq!(
        notifications.len(),
        1,
        "only the client's own notification arrives: {notifications:#?}"
    );
    assert_eq!(
        changes_of(&notifications[0]),
        vec![(support::file_uri(&ws.file("a.rs")), 2)]
    );
    client.shutdown();
}

#[test]
fn does_not_stand_in_outside_a_git_repository() {
    let ws = support::TempGitWorkspace::without_git("nogit");
    let mut client = ready_client_in(&ws.root, plain_capabilities());
    std::fs::write(ws.file("b.rs"), "pub fn other() {}\n").unwrap();
    let id = client.send_references();
    client.await_response_to(id);
    assert!(
        client
            .upstream_notifications("workspace/didChangeWatchedFiles")
            .is_empty(),
        "stood in outside a git repository"
    );
    client.shutdown();
}

#[test]
fn rewrites_a_duplicate_did_open_into_a_full_text_did_change() {
    let ws = support::TempGitWorkspace::new("didopen");
    let mut client = ready_client_in(&ws.root, plain_capabilities());
    let a = ws.file("a.rs");
    client.did_open(&a, "rust");
    // Claude Code resends didOpen to the same uri on every Write.
    client.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": support::file_uri(&a), "languageId": "rust", "version": 1, "text": "pub fn target() {}\npub fn more() {}\n"}}),
    );
    sync_with_upstream(&mut client);

    assert_eq!(
        client.upstream_notifications("textDocument/didOpen").len(),
        1,
        "forwarded the 2nd didOpen as is"
    );
    let changes = client.upstream_notifications("textDocument/didChange");
    assert_eq!(changes.len(), 1, "not rewritten into a full-text didChange");
    assert_eq!(
        changes[0]["textDocument"]["uri"],
        json!(support::file_uri(&a))
    );
    assert_eq!(changes[0]["textDocument"]["version"], json!(1));
    assert_eq!(
        changes[0]["contentChanges"],
        json!([{"text": "pub fn target() {}\npub fn more() {}\n"}])
    );

    // Closing and then reopening is a legitimate didOpen.
    client.did_close(&a);
    client.did_open(&a, "rust");
    sync_with_upstream(&mut client);
    assert_eq!(
        client.upstream_notifications("textDocument/didOpen").len(),
        2
    );
    assert_eq!(
        client
            .upstream_notifications("textDocument/didChange")
            .len(),
        1
    );
    client.shutdown();
}
