//! 下流側の準拠テストスイート（docs/spec/server-state.md 9.1）。
//!
//! 仕様 9 章「クライアントの推奨挙動」を実行可能にしたもの。被験者は
//! 「本プロトコルに準拠したサーバーを相手にするクライアント」で、lsp-det の
//! 下流側が最初の被験者である。将来 Claude Code や Serena がネイティブに
//! 対応したとき、同じ要件で被験者を差し替える（v0.1-design.md 6 章）。
//!
//! lsp-det については、境界の上の状態の出所が 2 つある。上流が自ら宣言して
//! いれば上流の通知から（上流側は恒等写像）、そうでなければ写像から。
//! どちらでも下流側の挙動は同じでなければならないので、両方を被験者にする。

mod support;

use std::time::Duration;

use serde_json::{Value, json};
use support::{ConformanceClient, ServerUnderTest};

/// 「届かないこと」を確かめるときの観測窓。
const NEGATIVE_WINDOW: Duration = Duration::from_millis(750);

/// 境界の上の状態を動かす手段が違う 2 種類の被験者。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Subject {
    /// 準拠した偽上流 + lsp-det（上流側は恒等写像）。
    ConformantUpstream,
    /// rust-analyzer と名乗る偽上流 + lsp-det（写像が状態を作る）。
    MappedUpstream,
}

const SUBJECTS: [Subject; 2] = [Subject::ConformantUpstream, Subject::MappedUpstream];

/// 被験者を起動し、`initialize` 直後に `indexing` の状態にする。
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
        // 写像は最初の serverStatus まで initializing。indexing に進める。
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

/// 上流の状態変化が lsp-det に届くまで待つ同期点。偽上流は通知を送ってから
/// 次のリクエストに答えるので、上流への往復リクエストが返れば、その前に
/// 送らせた通知は lsp-det が処理済みである。
fn sync_with_upstream(client: &mut ConformanceClient) {
    let _ = client.upstream_methods_seen();
}

// ---------------------------------------------------------------------------
// 9.1 の 1: indexing の間は横断リクエストが上流に届かず、ready の後に届く
// ---------------------------------------------------------------------------

#[test]
fn spec_9_1_1_holds_cross_workspace_requests_until_ready() {
    for subject in SUBJECTS {
        let mut client = start_indexing(subject, false);
        let id = client.send_references();
        assert!(
            client.response_within(id, NEGATIVE_WINDOW).is_none(),
            "{subject:?}: indexing 中に references が応答された"
        );
        assert!(
            !saw_upstream(&mut client, "textDocument/references"),
            "{subject:?}: indexing 中に references が上流へ届いた"
        );

        make_ready(&mut client, subject);
        let response = client.await_response_to(id);
        assert!(
            response.get("result").is_some(),
            "{subject:?}: ready 後の references が成功応答でない: {response}"
        );
        assert!(
            saw_upstream(&mut client, "textDocument/references"),
            "{subject:?}: ready 後も references が上流へ届いていない"
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
            "{subject:?}: 2 件とも上流に届くはず: {seen:?}"
        );
        client.shutdown();
    }
}

// ---------------------------------------------------------------------------
// 9.1 の 2: health が error なら待たずに失敗する
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
            .unwrap_or_else(|| panic!("{subject:?}: error なのに references が待たされた"));
        assert!(
            response.get("error").is_some(),
            "{subject:?}: error のときの references が失敗応答でない: {response}"
        );
        assert!(
            !saw_upstream(&mut client, "textDocument/references"),
            "{subject:?}: error なのに references が上流へ届いた"
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
            "{subject:?}: error になったのに保留分が失敗応答でない: {response}"
        );
        client.shutdown();
    }
}

#[test]
fn returns_to_holding_after_the_error_recovers() {
    // error は回復しうる (設計 4.3)。回復後の横断リクエストは再び待つ。
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
        "回復後の indexing 中に references が待たされなかった"
    );
    make_ready(&mut client, subject);
    assert!(client.await_response_to(id).get("result").is_some());
    client.shutdown();
}

// ---------------------------------------------------------------------------
// 9.1 の 3: readiness が unknown なら待たない
// ---------------------------------------------------------------------------

#[test]
fn spec_9_1_3_does_not_hold_when_readiness_is_unknown() {
    // 既知の写像がない上流。両軸 unknown で、待つべき信号がない。
    let server = ServerUnderTest::lsp_det_without_adapter();
    let mut client = ConformanceClient::start(&server);
    client.initialize(false);
    let id = client.send_references();
    let response = client
        .response_within(id, Duration::from_secs(5))
        .expect("unknown なのに references が待たされた");
    assert!(response.get("result").is_some(), "{response}");
    client.shutdown();
}

// ---------------------------------------------------------------------------
// 9.1 の 4: 横断以外は indexing 中も通す
// ---------------------------------------------------------------------------

#[test]
fn spec_9_1_4_forwards_single_file_requests_while_indexing() {
    for subject in SUBJECTS {
        let mut client = start_indexing(subject, false);
        let id = client.send_hover();
        let response = client
            .response_within(id, Duration::from_secs(5))
            .unwrap_or_else(|| panic!("{subject:?}: indexing 中に hover が待たされた"));
        assert!(response.get("result").is_some(), "{subject:?}: {response}");
        client.shutdown();
    }
}

// ---------------------------------------------------------------------------
// 9.1 の 5: 代行中に cancel / shutdown を受けたら保留分すべてに応答する
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
            "{subject:?}: キャンセルした保留分が RequestCancelled でない: {response}"
        );
        // キャンセル済みの要求は ready になっても上流へ流さない。
        make_ready(&mut client, subject);
        assert!(
            !saw_upstream(&mut client, "textDocument/references"),
            "{subject:?}: キャンセル済みの references が上流へ届いた"
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
            "{subject:?}: shutdown 時の保留分が失敗応答でない: {response}"
        );
        let shutdown_response = client.await_response_to(shutdown);
        assert!(
            shutdown_response.get("error").is_none(),
            "{subject:?}: shutdown 自体が失敗した: {shutdown_response}"
        );
        client.notify("exit", json!(null));
    }
}

#[test]
fn spec_9_1_5_answers_held_requests_when_the_upstream_exits() {
    // 設計 4.2「上流の消失」: 保留分にエラーを応答してから接続を閉じる。
    for subject in SUBJECTS {
        let mut client = start_indexing(subject, false);
        let held = client.send_references();
        assert!(client.response_within(held, NEGATIVE_WINDOW).is_none());

        // 偽上流は exit 通知で終了する（shutdown なしでも）。
        client.notify("exit", json!(null));
        let response = client.await_response_to(held);
        assert!(
            response.get("error").is_some(),
            "{subject:?}: 上流消失時の保留分が失敗応答でない: {response}"
        );
    }
}

// ---------------------------------------------------------------------------
// 7.2 完全性を、下流側 + 偽上流で回す
//
// 偽上流はインデックス未完了の間、references に空配列を返す (無言の嘘)。
// 下流側が ready まで待たせるので、クライアントには完全な結果だけが届く。
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
            "{subject:?}: インデックス未完了の空応答がクライアントに届いた: {response}"
        );
        client.shutdown();
    }
}

// ---------------------------------------------------------------------------
// 仕様 5.2: 宣言したクライアントには代行しない
// ---------------------------------------------------------------------------

#[test]
fn does_not_hold_when_the_client_declared_server_state() {
    for subject in SUBJECTS {
        let mut client = start_indexing(subject, true);
        let id = client.send_references();
        let response = client
            .response_within(id, Duration::from_secs(5))
            .unwrap_or_else(|| {
                panic!("{subject:?}: 宣言したクライアントの references が待たされた")
            });
        assert!(response.get("result").is_some(), "{subject:?}: {response}");
        client.shutdown();
    }
}

// ---------------------------------------------------------------------------
// 恒等写像のときの境界の状態の読み方 (設計 4.1・4.2)
// ---------------------------------------------------------------------------

#[test]
fn upstream_notifications_are_not_forwarded_to_a_client_that_did_not_declare() {
    // 下流側が読むために上流に通知を出させるが、宣言していないクライアントには
    // 流さない (仕様 5.2)。
    let mut client = start_indexing(Subject::ConformantUpstream, false);
    client.make_upstream_emit_server_state_changed("ok", "ready");
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "宣言していないクライアントへ上流の serverStateChanged を流した"
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
    // 上流の通知は変化のときにしか来ない。初期状態は lsp-det が自分で
    // 問い合わせて得る。その問い合わせの応答はクライアントには見えない。
    let mut client = start_indexing(Subject::ConformantUpstream, false);
    assert!(
        saw_upstream(&mut client, "experimental/serverState"),
        "lsp-det が上流に初期状態を問い合わせていない"
    );
    let id = client.send_references();
    assert!(
        client.response_within(id, NEGATIVE_WINDOW).is_none(),
        "初期状態 (indexing) を読めておらず references が待たされなかった"
    );
    client.cancel(id);
    client.await_response_to(id);
    client.shutdown();
}

/// 未使用警告を避ける（被験者ごとに使うヘルパーが異なる）。
#[allow(dead_code)]
fn _unused(_: Value) {}

// ---------------------------------------------------------------------------
// 9.1 の 1（ADR 0014）: 通知で始まった再インデックスの間も保留する
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
        "通知で始まった再インデックスの間に references が応答された"
    );

    client.make_upstream_emit_status("ok", true);
    let response = client.await_response_to(id);
    let locations = response["result"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        locations.len(),
        2,
        "ready 後の応答が通知した変更を織り込んでいない: {response}"
    );
    client.shutdown();
}
