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
fn spec_8_2_5_declares_no_guarantees_for_an_untested_version() {
    // 仕様 8.2 の 5: 保証は準拠テストを通した版の範囲でのみ宣言する。
    // 範囲外の版や版の名乗りがない上流には、状態の通知だけを約束する。
    for version in ["1.97.0 (old)", "none"] {
        let server =
            ServerUnderTest::lsp_det_with_fake_upstream_flags(&["--server-version", version]);
        let mut client = ConformanceClient::start(&server);
        let result = client.initialize(true);
        assert_eq!(
            result["result"]["capabilities"]["experimental"]["serverStateProvider"],
            json!(true),
            "テストを当てていない版 {version:?} に保証を宣言した: {result}"
        );
        // 写像そのものは働く (状態は追跡する)。
        client.make_upstream_emit_status("ok", true);
        assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
        client.shutdown();
    }
}

#[test]
fn maps_the_missing_workspace_warning_of_rust_analyzer_to_error() {
    // 設計 5.1: 写像は言語サーバーの語彙の粗さを補う。プロジェクト未発見の
    // warning は横断問い合わせにとって機能不全なので error に写す。
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
// gopls の写像 (設計 5.2)
//
// gopls は readiness の語彙を持たない。上流側は `$/progress` の
// "Setting up workspace" の begin / end から readiness を、
// "Error loading workspace" の begin / end から health を合成する。
// ---------------------------------------------------------------------------

fn gopls_client(declare_server_state: bool) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_gopls();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(declare_server_state);
    (client, result)
}

#[test]
fn gopls_spec_8_2_5_declares_no_guarantees_for_an_untested_version() {
    // 偽 gopls の既定の版 (1.98.0 (fake)) は読めるが gopls::TESTED_VERSIONS の
    // 範囲外なので、保証は宣言しない。
    let (mut client, result) = gopls_client(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!(true),
        "gopls に測っていない保証を宣言した: {result}"
    );
    client.shutdown();
}

#[test]
fn gopls_spec_5_declares_the_measured_guarantees_for_a_tested_version() {
    // 7.2 / 7.3 を実 gopls v0.23.0 に当てて通した (gopls_* ignored)。
    let server =
        ServerUnderTest::lsp_det_with_upstream_flags("gopls", &["--server-version", "v0.23.0"]);
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"completeness": true, "freshness": true}),
        "測った版に保証を宣言していない: {result}"
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
    assert_eq!(state.health, Health::Ok, "ロード成功で health は ok");
    client.shutdown();
}

#[test]
fn gopls_waits_for_every_workspace_folder() {
    // フォルダごとに progress が 1 つ出る。全部終わるまで ready ではない。
    let (mut client, _) = gopls_client(true);
    client.make_upstream_begin_workspace_load("a");
    client.make_upstream_begin_workspace_load("b");
    client.await_state_changed();
    client.make_upstream_end_workspace_load("a", "Finished loading packages.");
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "1 フォルダの終了で ready を名乗った"
    );
    client.make_upstream_end_workspace_load("b", "Finished loading packages.");
    assert_eq!(client.await_state_changed().readiness, Readiness::Ready);
    client.shutdown();
}

#[test]
fn gopls_spec_7_1_3_rearms_when_a_folder_is_added() {
    // didChangeWorkspaceFolders で "Setting up workspace" が再発行される。
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
        "失敗の message を添える"
    );

    // 回復: "Done." で終わる。
    client.make_upstream_emit_progress(json!({
        "token": "err",
        "value": {"kind": "end", "message": "Done."}
    }));
    assert_eq!(client.await_state_changed().health, Health::Ok);
    client.shutdown();
}

#[test]
fn gopls_reports_a_failed_load_as_health_error() {
    // フォルダのロード失敗は "Error loading packages: ..." で end する。
    // 試行は終わったので ready、結果は信頼できないので error (仕様 6 章 5 項)。
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
    // 診断や govulncheck 等、他の title の progress は readiness に触れない。
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
        "無関係な progress で状態が動いた"
    );
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    client.shutdown();
}

// ---------------------------------------------------------------------------
// pyright の写像 (ADR 0011、設計 5.3)
//
// pyright は readiness の語彙を持たず `serverInfo` も返さない。上流側は
// 起動ログの名乗りで写像を選び、`window/logMessage` のファイル列挙完了
// ("Found N source files" / "No source files found.") から readiness を
// 合成する。health の信号はなく unknown のまま。
// ---------------------------------------------------------------------------

fn pyright_client(declare_server_state: bool) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_pyright();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(declare_server_state);
    (client, result)
}

#[test]
fn pyright_is_identified_by_its_startup_log_when_server_info_is_absent() {
    // serverInfo がないと写像なし (両軸 unknown) になるところ、起動ログの
    // 名乗りで pyright の写像が選ばれ、開始状態 (initializing) にいる。
    let (mut client, result) = pyright_client(true);
    assert!(
        !result["result"]["capabilities"]["experimental"]["serverStateProvider"].is_null(),
        "上流側の宣言がない: {result}"
    );
    assert!(
        result["result"]["serverInfo"].is_null(),
        "前提が崩れている。偽 pyright は serverInfo を返さないはず: {result}"
    );
    let state = client.server_state();
    assert_eq!(
        state.readiness,
        Readiness::Initializing,
        "写像が選ばれていない"
    );
    assert_eq!(state.health, Health::Unknown);
    client.shutdown();
}

#[test]
fn basedpyright_is_identified_by_its_server_info() {
    // basedpyright は serverInfo を返す。起動ログがなくても同じ写像を選ぶ。
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
    // 起動ログの版が pyright::TESTED_VERSIONS になければ保証は宣言しない。
    let server = ServerUnderTest::lsp_det_with_upstream_flags(
        "none",
        &["--startup-log", "Pyright language server 1.1.400 starting"],
    );
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!(true),
        "pyright に測っていない保証を宣言した: {result}"
    );
    client.shutdown();
}

#[test]
fn pyright_spec_5_declares_the_measured_guarantees_for_a_tested_version() {
    // 7.2 / 7.3 を実 pyright 1.1.412 に当てて通した (pyright_* ignored)。
    // 版は起動ログから読む (serverInfo がない)。
    let (mut client, result) = pyright_client(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"completeness": true, "freshness": true}),
        "測った版に保証を宣言していない: {result}"
    );
    client.shutdown();
}

#[test]
fn basedpyright_spec_5_declares_the_measured_guarantees_for_a_tested_version() {
    // 7.2 / 7.3 を実 basedpyright 1.39.8 に当てて通した。版は serverInfo から読む。
    let server = ServerUnderTest::lsp_det_with_upstream_flags(
        "basedpyright",
        &["--server-version", "1.39.8"],
    );
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"completeness": true, "freshness": true}),
        "測った版に保証を宣言していない: {result}"
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
        "列挙の完了は health の観測ではない"
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
    // フォルダごとに "Starting service instance" と完了ログが 1 回ずつ出る。
    let (mut client, _) = pyright_client(true);
    client.make_upstream_start_service_instance("one");
    client.make_upstream_start_service_instance("two");
    client.make_upstream_finish_enumeration("Found 400 source files");
    assert!(
        client.expect_no_notification("experimental/serverStateChanged", NEGATIVE_WINDOW),
        "1 フォルダの完了で ready を名乗った"
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
    // "Searching for source files" は log レベル (type 4)。既定では届かないが、
    // 届いたときは再列挙の開始。
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
    // 開いたファイルの解析の $/progress と他のログは readiness に触れない。
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
        "無関係なメッセージで状態が動いた"
    );
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    client.shutdown();
}

#[test]
fn pyright_logs_are_forwarded_to_the_client_unchanged() {
    // 写像が読むだけで、ログはクライアントにもそのまま届く (原文転送)。
    let (mut client, _) = pyright_client(false);
    client.make_upstream_start_service_instance("one");
    client.make_upstream_finish_enumeration("Found 2 source files");
    let found = client
        .await_notification("window/logMessage")
        .expect("ログが届かない");
    assert!(
        found["message"].as_str().is_some(),
        "ログの形が変わった: {found}"
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
fn spec_8_4_2_asks_the_conformant_upstream_only_after_initialized() {
    // 恒等写像のとき、上流側は初期状態を自分で問い合わせる（設計 4.2）。その
    // 問い合わせはクライアントの `initialized` を流した後でなければならない。
    // LSP はサーバーが `initialized` まで他のリクエストを受けないことを許し、
    // rust-analyzer は実際に規約違反として終了する（上流のパッチで観測）。
    let server = ServerUnderTest::lsp_det_without_adapter_flags(&[
        "--declare-server-state-provider",
        "--require-initialized-before-requests",
    ]);
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"freshness": true}),
        "上流の宣言を書き換えた: {result}"
    );
    let state = client.server_state();
    assert_eq!(
        state.message.as_deref(),
        Some("answered by upstream"),
        "initialized より前に問い合わせて上流を落とした"
    );
    client.shutdown();
}

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

// ---------------------------------------------------------------------------
// typescript-language-server の写像 (ADR 0010 決定 B の M6、設計 5.3)
//
// serverInfo を返さず readiness の語彙も持たない。上流側は固有の通知
// `$/typescriptVersion` で写像を選び、"Initializing JS/TS language features…"
// の $/progress から readiness を、"[tsserver] Exited. Code:" のログから
// health を合成する。
// ---------------------------------------------------------------------------

fn tsls_client(declare_server_state: bool) -> (ConformanceClient, Value) {
    let server = ServerUnderTest::lsp_det_with_fake_typescript_language_server();
    let mut client = ConformanceClient::start(&server);
    let result = client.initialize(declare_server_state);
    (client, result)
}

#[test]
fn typescript_language_server_is_identified_by_its_typescript_version_notification() {
    // 名乗りは initialize 応答の後に届く。写像はその時点で選ばれ、開始状態にいる。
    let (mut client, result) = tsls_client(true);
    assert!(
        result["result"]["serverInfo"].is_null(),
        "前提が崩れている。偽 typescript-language-server は serverInfo を返さないはず: {result}"
    );
    // 名乗りは応答の後なので、届くまで待つ (最初の状態問い合わせの前に届く)。
    client.make_upstream_begin_project_load("1");
    assert_eq!(client.await_state_changed().readiness, Readiness::Indexing);
    client.shutdown();
}

#[test]
fn typescript_language_server_is_identified_by_its_startup_log_before_initialize_completes() {
    // "Using Typescript version …" は initialize 応答より先に届く。応答の時点で
    // 写像が選ばれ、直後の状態問い合わせが initializing を返す。
    let (mut client, result) = tsls_client(true);
    assert!(
        !result["result"]["capabilities"]["experimental"]["serverStateProvider"].is_null(),
        "上流側の宣言がない: {result}"
    );
    assert_eq!(
        client.server_state().readiness,
        Readiness::Initializing,
        "応答の時点で写像が選ばれていない"
    );
    client.shutdown();
}

#[test]
fn typescript_language_server_is_identified_by_the_version_notification_alone() {
    // 起動ログが (設定で) 出なくても、$/typescriptVersion だけで選べる。
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
    // クライアントが window.workDoneProgress を宣言していると、上流側は
    // handshake 後の覗き見を省く経路に入る。写像が未選択の間は省いてはならない。
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
        json!(true),
        "測っていない保証を宣言した: {result}"
    );
    client.shutdown();
}

#[test]
fn typescript_language_server_spec_5_declares_the_measured_guarantees_for_a_tested_version() {
    // 7.2 / 7.3 を実サーバー (TypeScript 5.9.3) に当てて通した。版は起動ログから
    // 読むので initialize 応答に間に合う。
    let (mut client, result) = tsls_client(true);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"completeness": true, "freshness": true}),
        "測った版に保証を宣言していない: {result}"
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
    assert_eq!(state.health, Health::Ok, "ロードの成功で health は ok");
    client.shutdown();
}

#[test]
fn typescript_language_server_spec_7_1_3_rearms_on_the_next_project_load() {
    // 2 つ目のプロジェクト (または tsconfig の変更) で再発行される。
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
        "失敗の message を添える: {state:?}"
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
        "無関係なメッセージで状態が動いた"
    );
    assert_eq!(client.server_state().readiness, Readiness::Initializing);
    client.shutdown();
}

// ---------------------------------------------------------------------------
// 実 pyright / basedpyright 結合（ローカル専用。CI に入れない — v0.1-design.md 6 章）
//
// pyright-langserver と basedpyright-langserver が PATH にあること (flake.nix)。
// ---------------------------------------------------------------------------

/// lsp-det 経由で実 pyright を起動する被験者。
fn real_pyright(project: &support::TempPyProject, command: &str) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec!["--".to_string(), command.to_string(), "--stdio".to_string()],
        root: project.root.clone(),
    }
}

/// `a.py` の `target` への参照のうち、`file` を指すものだけを返す。
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

/// pyright 経由。起動ログで写像が選ばれ、列挙完了で ready になる (ADR 0011)。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
fn pyright_spec_7_1_through_lsp_det_with_real_pyright() {
    let project = support::TempPyProject::with_cross_file_reference("readiness");
    let mut client = ConformanceClient::start(&real_pyright(&project, "pyright-langserver"));
    let result = client.initialize_with_root(true, &project.root);
    assert!(
        result["result"]["serverInfo"].is_null(),
        "前提が崩れている。pyright が serverInfo を返すようになった: {result}"
    );
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"completeness": true, "freshness": true}),
        "測った版の実 pyright に保証が宣言されていない: {result}"
    );
    let state = client.server_state();
    assert_ne!(
        state.readiness,
        Readiness::Unknown,
        "起動ログで写像が選ばれていない"
    );
    client.wait_until_ready();
    assert_eq!(
        client.server_state().health,
        Health::Unknown,
        "pyright に health の信号はない"
    );
    client.shutdown();
}

/// basedpyright 経由。serverInfo で同じ写像が選ばれる。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
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
        json!({"completeness": true, "freshness": true}),
        "測った版の実 basedpyright に保証が宣言されていない: {result}"
    );
    assert_ne!(client.server_state().readiness, Readiness::Unknown);
    client.wait_until_ready();
    client.shutdown();
}

/// 7.2 完全性を実 pyright で測る。宣言の根拠。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
fn pyright_spec_7_2_completeness_through_lsp_det_with_real_pyright() {
    py_completeness_with("pyright-langserver", "completeness");
}

/// 7.2 完全性を実 basedpyright で測る。宣言の根拠。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
fn pyright_spec_7_2_completeness_through_lsp_det_with_real_basedpyright() {
    py_completeness_with("basedpyright-langserver", "based-completeness");
}

fn py_completeness_with(command: &str, tag: &str) {
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
        "ready を名乗りながら b.py の呼び出しを取りこぼした (完全性違反): {found:#?}"
    );
    client.shutdown();
}

/// 7.3 鮮度を実 pyright で測る (クロスファイル)。宣言の根拠。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
fn pyright_spec_7_3_cross_file_freshness_through_lsp_det_with_real_pyright() {
    py_freshness_with("pyright-langserver", "freshness");
}

/// 7.3 鮮度を実 basedpyright で測る。宣言の根拠。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
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
        "前提が崩れている。b.py からの参照が見えるはず"
    );

    client.did_change(&b, 2, support::PY_B_WITHOUT_CALL);
    assert_eq!(client.server_state().readiness, Readiness::Ready);

    let after = py_references_in(&mut client, &a, &b);
    assert!(
        after.is_empty(),
        "ready を名乗りながら、消したはずの参照を返した (鮮度違反): {after:#?}"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// 実 typescript-language-server 結合（ローカル専用。CI に入れない — v0.1-design.md 6 章）
//
// typescript-language-server と tsserver (typescript) が PATH にあること (flake.nix)。
// ---------------------------------------------------------------------------

/// lsp-det 経由で実 typescript-language-server を起動する被験者。
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

/// `a.ts` の `target` への参照のうち、`file` を指すものだけを返す。
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

/// ファイルを開くとプロジェクトがロードされ、initializing → indexing → ready と進む。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
fn typescript_language_server_spec_7_1_through_lsp_det_with_real_server() {
    let project = support::TempTsProject::with_cross_file_reference("readiness");
    let mut client = ConformanceClient::start(&real_tsls(&project));
    let result = client.initialize_with_root(true, &project.root);
    assert!(
        result["result"]["serverInfo"].is_null(),
        "前提が崩れている。typescript-language-server が serverInfo を返すようになった: {result}"
    );
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"completeness": true, "freshness": true}),
        "測った版の実サーバーに保証が宣言されていない: {result}"
    );
    client.did_open(&project.file("a.ts"), "typescript");
    client.wait_until_ready();
    assert_eq!(client.server_state().health, Health::Ok);
    client.shutdown();
}

/// 7.2 完全性。宣言の根拠。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
fn typescript_language_server_spec_7_2_completeness_through_lsp_det_with_real_server() {
    let project = support::TempTsProject::with_cross_file_reference("completeness");
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
        "ready を名乗りながら b.ts の呼び出しを取りこぼした (完全性違反): {found:#?}"
    );
    client.shutdown();
}

/// 7.3 鮮度 (クロスファイル)。宣言の根拠。仕様 10 章の見込みは「freshness 不可」。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
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
        "前提が崩れている。b.ts からの参照が見えるはず"
    );

    client.did_change(&b, 2, support::TS_B_WITHOUT_CALL);
    assert_eq!(client.server_state().readiness, Readiness::Ready);

    let after = ts_references_in(&mut client, &a, &b);
    assert!(
        after.is_empty(),
        "ready を名乗りながら、消したはずの参照を返した (鮮度違反): {after:#?}"
    );
    client.shutdown();
}

/// tsconfig の変更でロードが再発行され、indexing を経て ready に戻る。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
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
        .expect("tsconfig の変更で readiness が動かない (ソースの読みと違う)");
    assert_eq!(observed["readiness"], json!("indexing"));
    client.wait_until_ready();
    client.shutdown();
}

/// `experimental/serverState` を問い合わせ続けて条件を満たす状態を返す。
/// 通知を受け取らないクライアント (宣言なし) 用。上限は実サーバーの起動と
/// ロードを十分に覆う値で、被験者の判定には使わない。
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
            "20 秒待っても条件を満たさない: {state:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// tsserver を落とすと、言語サーバーは生き残って空応答を返す。上流側は
/// "[tsserver] Exited. Code:" のログで error にし、下流側は references を拒否する。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
fn typescript_language_server_tsserver_crash_becomes_health_error_with_real_server() {
    // 下流側が代行するのは、本プロトコルを宣言しないクライアントに対して
    // (ADR 0002 決定 3)。宣言しないので通知は来ず、状態は問い合わせで追う。
    let project = support::TempTsProject::with_cross_file_reference("crash");
    let a = project.file("a.ts");
    let mut client = ConformanceClient::start(&real_tsls(&project));
    client.initialize_with_root(false, &project.root);
    client.did_open(&a, "typescript");
    let state = poll_state_until(&mut client, |s| s.readiness == Readiness::Ready);
    assert_eq!(state.health, Health::Ok, "前提が崩れている: {state:?}");

    let killed = support::kill_descendants_matching(client.server_pid(), "tsserver");
    assert!(!killed.is_empty(), "tsserver の孫プロセスが見つからない");

    let state = poll_state_until(&mut client, |s| s.health == Health::Error);
    assert!(
        state
            .message
            .as_deref()
            .is_some_and(|m| m.contains("Exited. Code:")),
        "クラッシュの理由を添えていない: {state:?}"
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
        "壊れたサーバーの成功風応答をそのまま流した: {response}"
    );
    client.shutdown();
}

// ---------------------------------------------------------------------------
// 実 gopls 結合（ローカル専用。CI に入れない — v0.1-design.md 6 章）
//
// gopls と go ツールチェーンが PATH にあること。
// ---------------------------------------------------------------------------

/// lsp-det 経由で実 gopls を起動する被験者。
fn real_gopls(project: &support::TempGoProject) -> ServerUnderTest {
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: vec!["--".to_string(), "gopls".to_string()],
        root: project.root.clone(),
    }
}

/// gopls 経由。initializing から ready への遷移を見る (設計 5.2 の写像)。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
fn gopls_spec_7_1_through_lsp_det_with_real_gopls() {
    let project = support::TempGoProject::with_cross_file_reference("readiness");
    let mut client = ConformanceClient::start(&real_gopls(&project));
    let result = client.initialize_with_root(true, &project.root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"],
        json!({"completeness": true, "freshness": true}),
        "測った版の実 gopls に保証が宣言されていない: {result}"
    );
    assert_ne!(client.server_state().readiness, Readiness::Ready);
    client.wait_until_ready();
    assert_eq!(client.server_state().health, Health::Ok);
    client.shutdown();
}

/// 7.2 完全性を実 gopls で測る。宣言の根拠 (設計 5.2)。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
fn gopls_spec_7_2_completeness_through_lsp_det_with_real_gopls() {
    let project = support::TempGoProject::with_cross_file_reference("completeness");
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
        "ready を名乗りながら b.go の呼び出しを取りこぼした (完全性違反): {found:#?}"
    );
    client.shutdown();
}

/// 7.3 鮮度を実 gopls で測る (クロスファイル)。宣言の根拠 (設計 5.2)。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
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
        "前提が崩れている。b.go からの参照が見えるはず"
    );

    client.did_change(&b, 2, support::GO_B_WITHOUT_CALL);
    assert_eq!(client.server_state().readiness, Readiness::Ready);

    let after = go_references_in(&mut client, &a, &b);
    assert!(
        after.is_empty(),
        "ready を名乗りながら、消したはずの参照を返した (鮮度違反): {after:#?}"
    );
    client.shutdown();
}

/// go.mod の変更で "Setting up workspace" が再発行されるか (設計 5.2 の実測)。
///
/// gopls のソースでは再発行は didChangeWorkspaceFolders のときだけで、go.mod の
/// 変更では出ない。ここではその読みを実サーバーで確かめる。
#[test]
#[ignore = "実サーバー結合。ローカル専用 (v0.1-design.md 6 章)。cargo test -- --ignored で実行"]
fn gopls_does_not_reemit_workspace_setup_on_go_mod_change() {
    let project = support::TempGoProject::with_cross_file_reference("gomod");
    let mut client = ConformanceClient::start(&real_gopls(&project));
    client.initialize_with_root(true, &project.root);
    client.wait_until_ready();

    // go.mod をディスク上で変える (Claude Code の編集はディスク書き込み)。
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
        "go.mod の変更で readiness が動いた (ソースの読みと違う): {observed:?}"
    );
    assert_eq!(client.server_state().readiness, Readiness::Ready);
    client.shutdown();
}

/// `a.go` の `Target` への参照のうち、`file` を指すものだけを返す。
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
