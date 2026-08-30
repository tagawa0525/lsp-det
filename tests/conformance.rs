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

#[test]
fn spec_7_1_4_reports_an_index_failure_as_health_error() {
    // 失敗は readiness ではなく health で表す (仕様 6 章 5 項)。rust-analyzer は
    // ワークスペースのロード失敗を {health: error, quiescent: true} で送る。
    let (mut client, _) = client(true);
    client.make_upstream_emit_status("error", true);
    let state = client.await_state_changed();
    assert_eq!(state.health, Health::Error);
    client.shutdown();
}

// ---------------------------------------------------------------------------
// アダプタなし (v0.1-design.md 4.1、ADR 0008)
//
// readiness を観測する手段がないので両軸 unknown。それでもプロセスの消失は
// 観測できるため dead は出す。これが中継層の固有価値をアダプタのない
// サーバーに届ける経路。
// ---------------------------------------------------------------------------

fn client_without_adapter(declare_extension_s: bool) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_without_adapter();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(declare_extension_s);
    (client, result)
}

#[test]
fn spec_5_declares_the_basic_grade_without_an_adapter() {
    let (mut client, result) = client_without_adapter(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!(true),
        "アダプタなしは基本グレード (true) を宣言する: {result}"
    );
    client.shutdown();
}

#[test]
fn spec_7_1_1_reports_unknown_on_both_axes_without_an_adapter() {
    let (mut client, _) = client_without_adapter(true);
    let state = client.server_state();
    assert_eq!(state.readiness, Readiness::Unknown);
    assert_eq!(state.health, Health::Unknown);
    client.shutdown();
}

#[test]
fn does_not_interpret_the_upstream_status_without_an_adapter() {
    // 上流が rust-analyzer 風の serverStatus を送っても、アダプタなしでは
    // 読まない。他のサーバーの同名通知を誤読しないため。
    let (mut client, _) = client_without_adapter(true);
    client.make_upstream_emit_status("ok", true);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "アダプタなしで readiness が動いてはならない"
    );
    assert_eq!(client.server_state().readiness, Readiness::Unknown);
    client.shutdown();
}

#[test]
fn spec_7_1_2_dead_stays_silent_without_an_adapter_when_the_client_did_not_declare() {
    // 仕様 5.2: 宣言していないクライアントには dead も通知しない。
    let (mut client, _) = client_without_adapter(false);
    client.notify("exit", json!(null));
    assert!(
        client.expect_silence_until_closed("experimental/serverStateChanged"),
        "宣言していないクライアントへ dead を通知した"
    );
}

#[test]
fn an_upstream_that_dies_before_answering_initialize_does_not_hang_the_client_without_an_adapter() {
    // アダプタなしでも handshake 中の死は隠さない。
    let server =
        ServerUnderTest::lsp_det_without_adapter_flags(&["--exit-before-initialize-result"]);
    let mut client = ConformanceClient::start(&server);
    let response = client.initialize_raw(true);
    assert!(
        response.get("error").is_some(),
        "上流が initialize に答えず消えたのに、エラーも返らなかった: {response}"
    );
}

#[test]
fn spec_6_1_reports_dead_without_an_adapter() {
    let (mut client, _) = client_without_adapter(true);
    client.notify("exit", json!(null));
    let state = client.await_state_changed();
    assert_eq!(state.health, Health::Dead);
    assert_eq!(
        state.readiness,
        Readiness::Unknown,
        "dead になっても readiness は観測していないまま"
    );
}

// ---------------------------------------------------------------------------
// handshake 前後の境界
//
// LSP は `InitializeResult` より前のサーバー発通知を許さない。しかし
// 「送れないから捨てる」と、その遷移は永久に失われる。沈黙は本拡張が
// 消そうとしているものそのものなので、境界の扱いを明示的に縛る。
// ---------------------------------------------------------------------------

#[test]
fn a_state_change_before_the_handshake_is_delivered_afterwards() {
    // 偽上流は InitializeResult より前に quiescent:true を送る。
    let server =
        ServerUnderTest::lsp_det_with_fake_upstream_flags(&["--status-before-initialize-result"]);
    let mut client = ConformanceClient::start(&server);
    client.initialize(true);

    // handshake 前に起きた遷移も、宣言したクライアントには届かねばならない。
    let state = client.await_state_changed();
    assert_eq!(state.readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn an_upstream_that_dies_before_answering_initialize_does_not_hang_the_client() {
    // 起動時クラッシュ。仕様 6.1 の dead が最も効くはずの場面。
    let server =
        ServerUnderTest::lsp_det_with_fake_upstream_flags(&["--exit-before-initialize-result"]);
    let mut client = ConformanceClient::start(&server);

    // handshake 前なので通知は送れない。ならば宙に浮いた initialize を
    // エラーで閉じるしかない。沈黙して EOF だけ返すのは無言の嘘である。
    let response = client.initialize_raw(true);
    assert!(
        response.get("error").is_some(),
        "上流が initialize に答えず消えたのに、エラーも返らなかった: {response}"
    );
}

// ---------------------------------------------------------------------------
// 実サーバー結合（ローカル専用。CI に入れない — v0.1-design.md 6 章）
// ---------------------------------------------------------------------------

/// 負の対照。生の rust-analyzer は拡張 S を実装していないので、スイートは
/// これを非準拠と判定しなければならない。全部通るスイートは何も測っていない
/// のと同じなので、「落ちるべきものが落ちる」ことを確かめておく。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
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
        "生の rust-analyzer が拡張 S を宣言している。上流が拡張を実装したか、\
         被験者の取り違えである: {result}"
    );
}

/// 7.2 完全性（completeness）。実サーバーに対してのみ意味を持つ要件。
///
/// `ready` を名乗った時点で、横断問い合わせが不完全（クロスファイルの
/// 利用箇所を取りこぼす）であってはならない。インデックス未完了の空応答こそ
/// 本プロジェクトが消そうとしている「無言の嘘」そのもの。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
fn spec_7_2_completeness_through_lsp_det_with_real_rust_analyzer() {
    let project = support::TempCargoProject::with_cross_file_reference("completeness");
    let a = project.file("a.rs");
    let b = project.file("b.rs");

    let mut client = ConformanceClient::start(&real_rust_analyzer(&project));
    client.initialize(true);
    client.wait_until_ready();
    client.did_open(&a, "rust");
    client.did_open(&b, "rust");

    // 事前に分かっている完全な結果: b.rs の 4 行目（0 起点で 3）の呼び出し。
    let found = references_in(&mut client, &a, &b);
    assert!(
        found
            .iter()
            .any(|location| location["range"]["start"]["line"] == 3),
        "ready を名乗りながら b.rs の呼び出しを取りこぼした（完全性違反）: {found:#?}"
    );

    client.shutdown();
}

/// 7.3 鮮度（freshness）。実サーバーに対してのみ意味を持つ要件。
///
/// 仕様 7.3 が要求するとおり**クロスファイル**で測る。ファイル B を変更し、
/// 別ファイル A のシンボルを起点にした横断問い合わせが B の変更を反映して
/// いるかを見る。同一ファイル内で測ると LSP の処理順序保証だけで通ってしまい、
/// 鮮度を何も検証しない（仕様 6 章 2 項）。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
fn spec_7_3_cross_file_freshness_through_lsp_det_with_real_rust_analyzer() {
    let project = support::TempCargoProject::with_cross_file_reference("freshness");
    let a = project.file("a.rs");
    let b = project.file("b.rs");

    let mut client = ConformanceClient::start(&real_rust_analyzer(&project));
    client.initialize(true);
    client.wait_until_ready();

    client.did_open(&a, "rust");
    client.did_open(&b, "rust");

    // 前提: b.rs から a.rs の target への参照が見えている。
    // 件数は数え方（`use` を参照に含めるか）に依存するので、
    // 「b.rs を指す参照があるか」だけを見る。
    let before = references_in(&mut client, &a, &b);
    assert!(
        !before.is_empty(),
        "前提が崩れている。b.rs からの参照が見えるはず"
    );

    // B から呼び出しを消す。これが仕様 6.2 が対象とする didChange。
    client.did_change(&b, 2, support::B_WITHOUT_CALL);

    // ready のまま問い合わせる。ready なら織り込み済みでなければならない。
    let state = client.server_state();
    assert_eq!(
        state.readiness,
        Readiness::Ready,
        "この時点で ready でなくなるなら freshness ではなく readiness の問題"
    );

    let after = references_in(&mut client, &a, &b);
    assert!(
        after.is_empty(),
        "ready を名乗りながら、消したはずの参照を返した（鮮度違反）: {after:#?}"
    );

    client.shutdown();
}

/// lsp-det 経由で実 rust-analyzer を起動する被験者。
fn real_rust_analyzer(project: &support::TempCargoProject) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec![
            "--adapter".to_string(),
            "rust-analyzer".to_string(),
            "--".to_string(),
            "rust-analyzer".to_string(),
        ],
        root: project.root.clone(),
    }
}

/// `a` 内のシンボル `target` への参照のうち、`file` を指すものだけを返す。
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
