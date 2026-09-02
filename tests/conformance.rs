//! サーバー状態プロトコルの上流側の準拠テストスイート
//! （docs/spec/server-state.md 7 章と 8.4）。
//!
//! 仕様 7 章（サーバーの義務）と 8.4（観測者の準拠要件）を実行可能にした
//! もので、被験者は「stdio で LSP を話すコマンド」であればなんでもよい。
//! lsp-det は最初の被験者に過ぎない（v0.1-design.md 6 章）。
//!
//! 各テスト名は仕様の条番号に対応させてある。仕様が変わったらここが落ちる。
//!
//! 7.2（completeness）と 7.3（freshness）は、被験者が保証を宣言している
//! ときだけ意味を持つ。lsp-det + 偽上流で回すのは下流側（M3）の後。
//! 下流側の準拠要件（仕様 9.1）は別のスイートで扱う。

mod support;

use std::time::Duration;

use lsp_det::state::{Health, Readiness};
use serde_json::{Value, json};
use support::{ConformanceClient, ServerUnderTest};

/// 「届かないこと」を確かめるときの観測窓。
const NEGATIVE_WINDOW: Duration = Duration::from_millis(750);

fn client(declare_server_state: bool) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_upstream();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(declare_server_state);
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
// 7.1 保証なしの宣言
// ---------------------------------------------------------------------------

#[test]
fn spec_7_1_1_answers_server_state_right_after_initialize() {
    // 偽上流はまだ信号を出していないので、上流側は「initialize 直後」に
    // 対応する initializing を報告する (仕様 8.2 の 2)。ready でないことは
    // 仕様の要件ではなく (7 章の前提条件)、この被験者の事実である。
    let (mut client, _) = client(true);
    let state = client.server_state();
    assert_eq!(state.readiness, Readiness::Initializing);
    assert_eq!(state.health, Health::Unknown);
    client.shutdown();
}

#[test]
fn spec_7_1_1_answers_server_state_even_without_the_client_declaration() {
    // 仕様 5.2: リクエストは宣言の有無によらず応答する。
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
    // 上流側が自ら答えるメソッドであり、上流は本プロトコルを知らない。
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
fn spec_8_2_7_closes_the_connection_without_a_notification_when_the_upstream_disappears() {
    // 仕様 8.2 の 7: プロセスの消失は本プロトコルの値ではない。中継層は
    // 未応答のリクエストにエラーを応答したうえで接続を閉じ、下流には
    // EOF が伝わる。「死んだ」を表す通知は送らない。
    let (mut client, _) = client(true);
    client.make_upstream_emit_status("ok", true);
    client.await_state_changed();

    client.notify("exit", json!(null));
    assert!(
        client.expect_silence_until_closed("experimental/serverStateChanged"),
        "上流の消失を serverStateChanged で通知した (仕様 8.2 の 7 違反)"
    );
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
// 写像の選択と capability の注入 (設計 4.2、ADR 0009 決定 D-2・D-3)
//
// 写像は上流が InitializeResult.serverInfo.name で名乗る名前で選ぶ。
// 名乗る前に initialize を上流へ送る必要があるので、既知の写像ぶんの
// capability は無条件に注入する。
// ---------------------------------------------------------------------------

#[test]
fn selects_the_mapping_from_the_server_info_name() {
    // 既定の被験者は偽上流が rust-analyzer と名乗る。--adapter は存在しない。
    let (mut client, result) = client(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"completeness": true, "freshness": true}),
        "rust-analyzer と名乗った上流に rust-analyzer の写像が選ばれていない: {result}"
    );
    client.make_upstream_emit_status("ok", true);
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn injects_the_capabilities_of_every_known_mapping_unconditionally() {
    // serverInfo は initialize の応答で分かる。注入はその前に要るので、
    // 上流が誰であっても既知の写像ぶんを注入する。
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
            "rust-analyzer 用の宣言が注入されていない: {capabilities}"
        );
        assert_eq!(
            capabilities["window"]["workDoneProgress"],
            json!(true),
            "gopls 用の宣言が注入されていない: {capabilities}"
        );
        // 注入は対象の 2 キーを true にするだけで、クライアントの他の宣言
        // (hover 等) は残る。
        assert_eq!(capabilities["textDocument"]["hover"], json!({}));
        client.shutdown();
    }
}

#[test]
fn answers_work_done_progress_create_itself_when_the_client_did_not_declare_it() {
    // 注入した window.workDoneProgress に由来するリクエストは、クライアントに
    // 転送せず lsp-det が自ら成功応答する (設計 4.2)。宣言していない
    // クライアントは MethodNotFound を返す (Serena で確認済み)。
    let server = ServerUnderTest::lsp_det_with_fake_upstream_flags(&["--request-progress-create"]);
    let mut client = ConformanceClient::start(&server);
    client.initialize(true);
    assert!(
        client.expect_no_notification("window/workDoneProgress/create", NEGATIVE_WINDOW),
        "宣言していないクライアントに window/workDoneProgress/create を転送した"
    );
    assert!(
        client.upstream_progress_create_answered(),
        "上流の window/workDoneProgress/create に応答していない"
    );
    client.shutdown();
}

#[test]
fn forwards_work_done_progress_create_when_the_client_declared_it() {
    // クライアントが元々宣言していた capability に基づくリクエストは素通し。
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
        "宣言したクライアントへ window/workDoneProgress/create が届かない"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// 写像なし (仕様 8.2 の 3、8.4 の 1)
//
// readiness を観測する手段がないので両軸 unknown を正直に報告する。
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
        json!(true),
        "写像なしは保証のない宣言 (true) をする: {result}"
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
    // 既知の写像がない名前を名乗る上流が rust-analyzer 風の serverStatus を
    // 送っても読まない。他のサーバーの同名通知を誤読しないため。
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
fn spec_8_2_7_closes_the_connection_without_a_notification_without_an_adapter() {
    // 写像がなくても同じ。dead を出す代わりに、接続を閉じて EOF で伝える。
    let (mut client, _) = client_without_adapter(true);
    client.notify("exit", json!(null));
    assert!(
        client.expect_silence_until_closed("experimental/serverStateChanged"),
        "上流の消失を serverStateChanged で通知した (仕様 8.2 の 7 違反)"
    );
}

// ---------------------------------------------------------------------------
// 上流自身が宣言している場合 (仕様 8.2 の 6、8.4 の 2)
//
// 上流側は恒等写像になる。宣言を足さず、リクエストを転送し、自前の通知を
// 出さない。同一接続に送信者の異なる 2 系統が流れるのを避ける。
// ---------------------------------------------------------------------------

#[test]
fn spec_8_4_2_defers_to_a_conformant_upstream_without_an_adapter() {
    let server =
        ServerUnderTest::lsp_det_without_adapter_flags(&["--declare-server-state-provider"]);
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);

    // 上流の宣言をそのまま通す (保証なしの宣言で上書きしない)。
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"freshness": true}),
        "上流の宣言を書き換えた: {result}"
    );

    // リクエストは上流へ届き、上流の答えが返る。
    let state = client.server_state();
    assert_eq!(state.message.as_deref(), Some("answered by upstream"));
    assert!(
        client
            .upstream_methods_seen()
            .iter()
            .any(|m| m == "experimental/serverState"),
        "experimental/serverState を上流へ転送していない"
    );

    // 上流側は自前の通知を出さない (上流が送信者)。
    client.notify("exit", json!(null));
    assert!(
        client.expect_silence_until_closed("experimental/serverStateChanged"),
        "恒等写像のはずの上流側が通知を出した"
    );
}

#[test]
fn a_false_upstream_declaration_is_not_a_declaration() {
    // `serverStateProvider: false` は「提供しない」。恒等写像に切り替えては
    // ならず、上流側は自分の宣言を置いて自分で答える。
    let server =
        ServerUnderTest::lsp_det_without_adapter_flags(&["--declare-server-state-provider-false"]);
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!(true),
        "false を宣言とみなして恒等写像に切り替えた: {result}"
    );
    assert_eq!(client.server_state().readiness, Readiness::Unknown);
    client.shutdown();
}

#[test]
fn does_not_emit_its_own_notifications_under_deferral_with_an_adapter() {
    // 恒等写像中に自前の serverStateChanged を出すと、上流の通知と 2 系統に
    // なる (仕様 8.2 の 6)。写像が生きた遷移を読んでも出さない。
    let server =
        ServerUnderTest::lsp_det_with_fake_upstream_flags(&["--declare-server-state-provider"]);
    let mut client = ConformanceClient::start(&server);
    client.initialize(true);
    client.make_upstream_emit_status("ok", true);
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "恒等写像中に上流側が自前の通知を出した"
    );
    client.shutdown();
}

#[test]
fn spec_8_4_2_defers_to_a_conformant_upstream_even_with_an_adapter() {
    // 写像は上流の語彙を補うためのもの。上流が本プロトコルを話すなら不要で、
    // 上流側の宣言で上流の宣言を隠してはならない。
    let server =
        ServerUnderTest::lsp_det_with_fake_upstream_flags(&["--declare-server-state-provider"]);
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"freshness": true})
    );
    assert_eq!(
        client.server_state().message.as_deref(),
        Some("answered by upstream")
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// handshake 前後の境界
//
// 写像は InitializeResult の serverInfo で選ぶので、それより前の上流の
// 状態通知は解釈できない。LSP はサーバー発の通知を InitializeResult の後に
// 限っている (許されるのは showMessage / logMessage / telemetry だけ) ので、
// これは仕様の範囲内である。ここで縛るのは initialize の失敗と再試行の扱い。
// ---------------------------------------------------------------------------

#[test]
fn an_initialize_error_does_not_end_the_handshake() {
    // initialize へのエラー応答は handshake の完了ではない。クライアントは
    // 再試行でき、2 回目の InitializeResult が本当の handshake になる。
    let server = ServerUnderTest::lsp_det_with_fake_upstream_flags(&["--fail-first-initialize"]);
    let mut client = ConformanceClient::start(&server);

    let first = client.initialize_raw(true);
    assert!(
        first.get("error").is_some(),
        "偽上流は 1 回目を失敗させるはず"
    );

    let second = client.initialize(true);
    assert!(
        !second["result"]["capabilities"]["experimental"]["serverStateProvider"].is_null(),
        "再試行した initialize に serverStateProvider がない: {second}"
    );
    client.shutdown();
}

#[test]
fn death_during_a_retried_initialize_is_still_closed_with_an_error() {
    // 1 回目はエラー、2 回目に答えず消える。2 回目も宙に浮かせない
    // (仕様 8.2 の 7)。
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
        "再試行中に上流が消えたのに、エラーも返らなかった: {second}"
    );
}

#[test]
fn an_answered_initialize_is_not_answered_again_when_the_upstream_dies() {
    // initialize にエラー応答が返った時点で、その id は宙に浮いていない。
    // その後に上流が消えても、同じ id に二重に応答してはならない
    // (JSON-RPC は 1 リクエスト 1 応答)。
    let server = ServerUnderTest::lsp_det_with_fake_upstream_flags(&[
        "--fail-first-initialize",
        "--exit-after-initialize-error",
    ]);
    let mut client = ConformanceClient::start(&server);
    let first = client.initialize_raw(true);
    assert!(
        first.get("error").is_some(),
        "偽上流は 1 回目を失敗させるはず"
    );
    assert!(
        client.expect_no_response_until_closed(),
        "応答済みの initialize に、上流消失時にもう一度応答した"
    );
}

#[test]
fn an_upstream_that_dies_before_answering_initialize_does_not_hang_the_client() {
    // 起動時クラッシュ。仕様 8.2 の 7: 未応答のリクエストにエラーを応答して
    // から接続を閉じる。
    let server =
        ServerUnderTest::lsp_det_with_fake_upstream_flags(&["--exit-before-initialize-result"]);
    let mut client = ConformanceClient::start(&server);

    // 宙に浮いた initialize をエラーで閉じる。沈黙して EOF だけ返すと
    // クライアントは応答を永久に待つ。
    let response = client.initialize_raw(true);
    assert!(
        response.get("error").is_some(),
        "上流が initialize に答えず消えたのに、エラーも返らなかった: {response}"
    );
}

// ---------------------------------------------------------------------------
// 実サーバー結合（ローカル専用。CI に入れない — v0.1-design.md 6 章）
// ---------------------------------------------------------------------------

/// 負の対照。生の rust-analyzer は本プロトコルを実装していないので、スイートは
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
        "生の rust-analyzer が serverStateProvider を宣言している。上流が実装したか、\
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
        args: vec!["--".to_string(), "rust-analyzer".to_string()],
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
        args: vec!["--".to_string(), "rust-analyzer".to_string()],
        root: support::repo_root(),
    };
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert!(!result["result"]["capabilities"]["experimental"]["serverStateProvider"].is_null());

    // 7.1 の 1: initialize 直後は ready ではない。
    assert_ne!(client.server_state().readiness, Readiness::Ready);

    // 実サーバーは自分のペースで ready になる。health が壊れたら抜ける。
    client.wait_until_ready();
    client.shutdown();
}
