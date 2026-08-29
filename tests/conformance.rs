//! 拡張 S の準拠テストスイート（docs/spec/extension-s-server-state.md 7 章）。
//!
//! M2 の中心成果物。仕様 7 節の準拠要件を実行可能にしたもので、被験者は
//! 「stdio で LSP を話すコマンド」であればなんでもよい。lsp-det は最初の
//! 被験者に過ぎない（v0.1-design.md 6 章）。
//!
//! 各テスト名は仕様の条番号に対応させてある。仕様が変わったらここが落ちる。
//!
//! 7.2（completeness）と 7.3（freshness）は、被験者が保証を宣言している
//! ときだけ意味を持つ。lsp-det については、ゲート（設計 4.2）が入る M2 の
//! 次の PR で追加する。

mod support;

use std::time::Duration;

use lsp_det::state::{Health, Readiness};
use serde_json::{Value, json};
use support::{ConformanceClient, ServerUnderTest};

/// 「届かないこと」を確かめるときの観測窓。
const NEGATIVE_WINDOW: Duration = Duration::from_millis(750);

fn client(declare_extension_s: bool) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_upstream();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(declare_extension_s);
    (client, result)
}

// ---------------------------------------------------------------------------
// 5 章: capability
// ---------------------------------------------------------------------------

#[test]
fn spec_5_declares_the_server_state_provider_capability() {
    let (mut client, result) = client(true);
    let provider = &result["result"]["capabilities"]["experimental"]["serverStateProvider"];
    assert!(
        !provider.is_null(),
        "InitializeResult に experimental.serverStateProvider がない: {result}"
    );
    client.shutdown();
}

#[test]
fn spec_5_keeps_the_upstream_capabilities_intact() {
    // 宣言を足すのであって、上流の宣言を置き換えてはならない。
    let (mut client, result) = client(true);
    let capabilities = &result["result"]["capabilities"];
    assert_eq!(capabilities["hoverProvider"], json!(true));
    assert_eq!(capabilities["referencesProvider"], json!(true));
    assert_eq!(
        capabilities["experimental"]["fakeUpstreamMarker"],
        json!(true),
        "上流の experimental が失われた: {capabilities}"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// 7.1 基本グレード
// ---------------------------------------------------------------------------

#[test]
fn spec_7_1_1_answers_server_state_right_after_initialize_and_is_not_ready() {
    let (mut client, _) = client(true);
    let state = client.server_state();
    assert_ne!(
        state.readiness,
        Readiness::Ready,
        "initialize 直後に ready を名乗ってはならない"
    );
    client.shutdown();
}

#[test]
fn spec_7_1_1_answers_server_state_even_without_the_client_declaration() {
    // 仕様 5.2: リクエストは宣言の有無によらず応答する。
    let (mut client, _) = client(false);
    let state = client.server_state();
    assert_ne!(state.readiness, Readiness::Ready);
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
        "宣言していないクライアントへ serverStateChanged を送ってはならない"
    );
    // 状態そのものは追跡されているので、リクエストには新しい値が返る。
    assert_eq!(client.server_state().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn spec_7_1_3_observes_ready_then_indexing_then_ready() {
    let (mut client, _) = client(true);

    client.make_upstream_emit_status("ok", true);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);

    // 依存変更に相当する再インデックス。
    client.make_upstream_emit_status("ok", false);
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);

    client.make_upstream_emit_status("ok", true);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);

    client.shutdown();
}

// ---------------------------------------------------------------------------
// 4 章: メソッドの意味
// ---------------------------------------------------------------------------

#[test]
fn spec_4_2_does_not_repeat_a_notification_for_an_unchanged_state() {
    let (mut client, _) = client(true);
    client.make_upstream_emit_status("ok", true);
    client.await_state_changed();

    client.make_upstream_emit_status("ok", true);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "2 軸が変わっていないのに通知してはならない"
    );
    client.shutdown();
}

#[test]
fn spec_4_1_does_not_forward_the_state_request_upstream() {
    // 中継層が自ら答えるメソッドであり、上流は拡張 S を知らない。
    let (mut client, _) = client(true);
    client.server_state();
    let seen = client.upstream_methods_seen();
    assert!(
        !seen.iter().any(|m| m == "experimental/serverState"),
        "experimental/serverState を上流へ転送した: {seen:?}"
    );
    client.shutdown();
}

#[test]
fn spec_6_1_reports_dead_when_the_upstream_disappears() {
    // 中継層だけが出せる値。プロセス消失の観測に基づく。
    let (mut client, _) = client(true);
    client.make_upstream_emit_status("ok", true);
    client.await_state_changed();

    // 偽上流は exit 通知で終了する。lsp-det はその消失を観測するはず。
    client.notify("exit", json!(null));
    let state = client.await_state_changed();
    assert_eq!(state.health, Health::Dead);
}

// ---------------------------------------------------------------------------
// 実サーバー結合（ローカル専用。CI に入れない — v0.1-design.md 6 章）
// ---------------------------------------------------------------------------

/// 本スイートが実サーバーにも当たることの確認。被験者を差し替えるだけで
/// 同じ準拠要件を検証できることが、この成果物の要件（設計 6 章）。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
fn spec_7_1_against_real_rust_analyzer() {
    let server = ServerUnderTest {
        program: "rust-analyzer".into(),
        args: vec![],
        root: support::repo_root(),
    };
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);

    assert!(
        !result["result"]["capabilities"]["experimental"]["serverStateProvider"].is_null(),
        "lsp-det を通していない生の rust-analyzer には拡張 S の宣言がない"
    );
}

/// lsp-det 経由の実 rust-analyzer。initializing から ready への遷移を見る。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
fn spec_7_1_through_lsp_det_with_real_rust_analyzer() {
    let server = ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec![
            "--adapter".to_string(),
            "rust-analyzer".to_string(),
            "--".to_string(),
            "rust-analyzer".to_string(),
        ],
        root: support::repo_root(),
    };
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert!(!result["result"]["capabilities"]["experimental"]["serverStateProvider"].is_null());

    // 7.1 の 1: initialize 直後は ready ではない。
    assert_ne!(client.server_state().readiness, Readiness::Ready);

    // 実サーバーは自分のペースで ready になる。
    loop {
        let state = client.await_state_changed();
        if state.readiness == Readiness::Ready {
            break;
        }
    }
    client.shutdown();
}
