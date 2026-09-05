//! 台本ベースの偽 LSP サーバー（v0.1-design.md 6 章のテスト戦略）。
//!
//! 準拠テストスイートが決定的に動くための上流。時間ではなくクライアントの
//! 指示で状態を変えるのが要点で、`$/fake/emitServerStatus` を受けた瞬間に
//! `experimental/serverStatus` を送り返す。sleep を挟んだ台本にすると
//! テストがタイミング依存になり、CI で不安定になる。
//!
//! 制御メソッド:
//!
//! - `$/fake/emitServerStatus`（通知）: params をそのまま
//!   `experimental/serverStatus` の params として送出する
//! - `$/fake/emitProgress`（通知）: params をそのまま `$/progress` の params
//!   として送出する（gopls の "Setting up workspace" 等を再現する）
//! - `$/fake/report`（リクエスト）: これまでに受信した method の一覧と、
//!   `initialize` で受け取った params を返す。上流へ何が転送されたかを
//!   テスト側から検証するために使う
//!
//! handshake 前後の境界を再現するための起動フラグ:
//!
//! - `--exit-before-initialize-result`: `initialize` を受け取った瞬間に、
//!   応答せず終了する（起動時クラッシュ）
//! - `--declare-server-state-provider`: 上流自身が本プロトコルに準拠している
//!   ふりをする。`InitializeResult` に `serverStateProvider: {freshness: {fileChanges: ["Changed"]}}`
//!   を宣言し、`experimental/serverState` に自分の状態で答える。状態は
//!   `--initial-readiness <initializing|indexing|ready>`（既定 `ready`）から
//!   始まり、`$/fake/emitServerStateChanged`（通知。params は
//!   `{health, readiness}`）で変えると `experimental/serverStateChanged` を送る
//! - `--declare-server-state-provider-false`: `serverStateProvider: false` を
//!   宣言する（`hoverProvider: false` と同じ「提供しない」の書き方）
//! - `--server-name <name>`: `InitializeResult.serverInfo.name` で名乗る名前。
//!   既定は `fake-lsp-server`（既知の写像がない名前）。`rust-analyzer` と
//!   名乗れば lsp-det は rust-analyzer の写像を選ぶ。`none` を渡すと
//!   `serverInfo` そのものを省く（pyright はこれを返さない）
//! - `--startup-log <message>`: 起動直後（`initialize` を読む前）に
//!   `window/logMessage`（type 3）で `message` を送る。pyright 系の名乗り
//!   "Pyright language server 1.1.412 starting" を再現する
//! - `--server-version <version>`: `serverInfo.version` で名乗る版。既定は
//!   `1.98.0 (fake)`（rust-analyzer の写像が準拠テストを通した版の範囲内）。
//!   `none` を渡すと version を省く
//! - `--references-depend-on-readiness`: `textDocument/references` に、
//!   自分が `ready` なら 1 件、そうでなければ空配列を返す（インデックス未完了の
//!   空応答を再現する）。`ready` かどうかは準拠モードなら自分の状態、
//!   rust-analyzer を演じるときは最後に送った `quiescent` で決まる
//! - `--request-progress-create`: `initialized` を受けたら
//!   `window/workDoneProgress/create` リクエスト（id `"wdp-1"`）を送る。
//!   応答が返ったかは `$/fake/report` の `progressCreateAnswered` で分かる
//! - `--startup-typescript-version <version>`: `initialize` に応答した直後に
//!   typescript-language-server 固有の通知 `$/typescriptVersion`
//!   `{version, source: "fake"}` を送る（実サーバーと同じ順序）
//! - `$/fake/emitLogMessage`（通知。params は `{type, message}`）:
//!   `window/logMessage` をそのまま送る（pyright の "Starting service
//!   instance" / "Found N source files" 等を再現する）
//! - `--require-initialized-before-requests`: `initialized` より前に
//!   `initialize` 以外のリクエストが届いたら、rust-analyzer と同じく規約違反として
//!   エラーを stderr に出して終了する（LSP: サーバーは `initialized` まで他の
//!   リクエストを受けない）
//! - `--fail-first-initialize`: 最初の `initialize` にエラーで応答する。
//!   2 回目以降は通常どおり
//! - `--exit-after-initialize-error`: `--fail-first-initialize` と併用し、
//!   エラー応答を送った直後に終了する（応答済みの id に二重応答しないことを
//!   確かめる）

use std::io::{self, BufReader};

use lsp_det::framing::{self, RawMessage};
use serde_json::{Value, json};

fn main() {
    // プロセス寿命のテスト (tests/process_lifetime.rs) が、lsp-det の
    // stderr 中継越しに上流の pid を知るための名乗り。
    eprintln!("fake-lsp-server: pid {}", std::process::id());
    let flags: Vec<String> = std::env::args().skip(1).collect();
    let has = |name: &str| flags.iter().any(|flag| flag == name);
    let exit_before_initialize_result = has("--exit-before-initialize-result");
    let declare_server_state_provider = has("--declare-server-state-provider");
    let declare_server_state_provider_false = has("--declare-server-state-provider-false");
    let fail_first_initialize = has("--fail-first-initialize");
    let exit_after_initialize_error = has("--exit-after-initialize-error");
    let request_progress_create = has("--request-progress-create");
    let references_depend_on_readiness = has("--references-depend-on-readiness");
    // `workspace/didChangeWatchedFiles` を受けたら再インデックス (quiescent: false)
    // を始め、変更の数だけ references の結果を増やす。終わりは
    // `$/fake/emitServerStatus` で外から与える (仕様 7.3 の 2 の偽上流)。
    let reindex_on_watched_files = has("--reindex-on-watched-files");
    let mut watched_changes: usize = 0;
    let require_initialized_before_requests = has("--require-initialized-before-requests");
    let mut initialized_seen = false;
    let server_name = flags
        .iter()
        .position(|flag| flag == "--server-name")
        .and_then(|i| flags.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "fake-lsp-server".to_string());
    let server_version = flags
        .iter()
        .position(|flag| flag == "--server-version")
        .and_then(|i| flags.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "1.98.0 (fake)".to_string());
    let startup_log = flags
        .iter()
        .position(|flag| flag == "--startup-log")
        .and_then(|i| flags.get(i + 1))
        .cloned();
    let startup_typescript_version = flags
        .iter()
        .position(|flag| flag == "--startup-typescript-version")
        .and_then(|i| flags.get(i + 1))
        .cloned();
    let mut progress_create_answered = false;
    let mut fake_health = "ok".to_string();
    let mut fake_readiness = flags
        .iter()
        .position(|flag| flag == "--initial-readiness")
        .and_then(|i| flags.get(i + 1))
        .cloned()
        .unwrap_or_else(|| "ready".to_string());
    let mut initialize_failed_once = false;

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut stdout = io::stdout();

    let mut methods_seen: Vec<String> = Vec::new();
    // 通知の params を method ごとに記録する (代行の検証用)。
    let mut notifications_seen: std::collections::BTreeMap<String, Vec<Value>> =
        std::collections::BTreeMap::new();
    let mut initialize_params = Value::Null;

    if let Some(message) = &startup_log {
        // 実サーバーはコンストラクタで名乗る。initialize を読む前に送る。
        send(
            &mut stdout,
            json!({
                "jsonrpc": "2.0",
                "method": "window/logMessage",
                "params": {"type": 3, "message": message}
            }),
        );
    }

    loop {
        let msg = match framing::read_message(&mut reader) {
            Ok(Some(msg)) => msg,
            other => {
                eprintln!("fake-lsp-server: stdin ended: {other:?}");
                return;
            }
        };
        let Ok(value) = serde_json::from_slice::<Value>(&msg.body) else {
            continue;
        };
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        let id = value.get("id").cloned();
        let params = value.get("params").cloned().unwrap_or(Value::Null);

        if method.is_empty() {
            // 自分が出したリクエストへの応答。
            if id == Some(json!("wdp-1")) {
                progress_create_answered = true;
            }
            continue;
        }
        methods_seen.push(method.to_string());
        if id.is_none() {
            notifications_seen
                .entry(method.to_string())
                .or_default()
                .push(params.clone());
        }
        if method == "initialized" {
            initialized_seen = true;
        }
        if require_initialized_before_requests
            && !initialized_seen
            && id.is_some()
            && method != "initialize"
        {
            eprintln!("fake-lsp-server: expected initialized notification, got request {method}");
            return;
        }

        match method {
            "initialize" => {
                initialize_params = params;
                if fail_first_initialize && !initialize_failed_once {
                    initialize_failed_once = true;
                    send(
                        &mut stdout,
                        json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {"code": -32603, "message": "first attempt fails", "data": {"retry": true}}
                        }),
                    );
                    if exit_after_initialize_error {
                        return;
                    }
                    continue;
                }
                if exit_before_initialize_result {
                    return;
                }
                let mut experimental = json!({"fakeUpstreamMarker": true});
                if declare_server_state_provider {
                    experimental["serverStateProvider"] =
                        json!({"freshness": {"fileChanges": ["Changed"]}});
                }
                if declare_server_state_provider_false {
                    experimental["serverStateProvider"] = json!(false);
                }
                let mut result = json!({
                    "capabilities": {
                        "hoverProvider": true,
                        "referencesProvider": true,
                        "experimental": experimental
                    }
                });
                if server_name != "none" {
                    result["serverInfo"] = if server_version == "none" {
                        json!({"name": server_name})
                    } else {
                        json!({"name": server_name, "version": server_version})
                    };
                }
                respond(&mut stdout, id, result);
                if let Some(version) = &startup_typescript_version {
                    send(
                        &mut stdout,
                        json!({
                            "jsonrpc": "2.0",
                            "method": "$/typescriptVersion",
                            "params": {"version": version, "source": "fake"}
                        }),
                    );
                }
            }
            "experimental/serverState" if declare_server_state_provider => {
                respond(
                    &mut stdout,
                    id,
                    json!({"health": fake_health, "readiness": fake_readiness, "message": "answered by upstream"}),
                );
            }
            "$/fake/emitProgress" => {
                send(
                    &mut stdout,
                    json!({
                        "jsonrpc": "2.0",
                        "method": "$/progress",
                        "params": params
                    }),
                );
            }
            "$/fake/emitLogMessage" => {
                send(
                    &mut stdout,
                    json!({
                        "jsonrpc": "2.0",
                        "method": "window/logMessage",
                        "params": params
                    }),
                );
            }
            "$/fake/emitServerStateChanged" => {
                if let Some(health) = params.get("health").and_then(Value::as_str) {
                    fake_health = health.to_string();
                }
                if let Some(readiness) = params.get("readiness").and_then(Value::as_str) {
                    fake_readiness = readiness.to_string();
                }
                send(
                    &mut stdout,
                    json!({
                        "jsonrpc": "2.0",
                        "method": "experimental/serverStateChanged",
                        "params": {"health": fake_health, "readiness": fake_readiness}
                    }),
                );
            }
            "initialized" if request_progress_create => {
                send(
                    &mut stdout,
                    json!({
                        "jsonrpc": "2.0",
                        "id": "wdp-1",
                        "method": "window/workDoneProgress/create",
                        "params": {"token": "fake-progress"}
                    }),
                );
            }
            "textDocument/references" if references_depend_on_readiness => {
                let result = if fake_readiness == "ready" {
                    let mut locations = vec![
                        json!({"uri": "file:///fake/b.rs", "range": {"start": {"line": 3, "character": 4}, "end": {"line": 3, "character": 10}}}),
                    ];
                    // 再インデックスで取り込んだディスク上の変更のぶん。
                    for i in 0..watched_changes {
                        locations.push(json!({"uri": format!("file:///fake/c{i}.rs"), "range": {"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 10}}}));
                    }
                    Value::Array(locations)
                } else {
                    json!([])
                };
                respond(&mut stdout, id, result);
            }
            "workspace/didChangeWatchedFiles" if reindex_on_watched_files => {
                watched_changes += params
                    .get("changes")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                fake_readiness = "indexing".to_string();
                send(
                    &mut stdout,
                    json!({
                        "jsonrpc": "2.0",
                        "method": "experimental/serverStatus",
                        "params": {"health": "ok", "quiescent": false}
                    }),
                );
            }
            "$/fake/emitServerStatus" => {
                if let Some(quiescent) = params.get("quiescent").and_then(Value::as_bool) {
                    fake_readiness = if quiescent { "ready" } else { "indexing" }.to_string();
                }
                send(
                    &mut stdout,
                    json!({
                        "jsonrpc": "2.0",
                        "method": "experimental/serverStatus",
                        "params": params
                    }),
                );
            }
            "$/fake/report" => {
                respond(
                    &mut stdout,
                    id,
                    json!({
                        "methodsSeen": methods_seen,
                        "notifications": notifications_seen,
                        "initializeParams": initialize_params,
                        "progressCreateAnswered": progress_create_answered
                    }),
                );
            }
            "shutdown" => respond(&mut stdout, id, Value::Null),
            "exit" => return,
            _ => {
                // 未知のリクエストにも応答しないとクライアントが待ち続ける。
                // 通知は放置してよい。
                if id.is_some() {
                    respond(&mut stdout, id, Value::Null);
                }
            }
        }
    }
}

fn respond<W: io::Write>(writer: &mut W, id: Option<Value>, result: Value) {
    send(
        writer,
        json!({"jsonrpc": "2.0", "id": id.unwrap_or(Value::Null), "result": result}),
    );
}

fn send<W: io::Write>(writer: &mut W, value: Value) {
    let body = serde_json::to_vec(&value).expect("fake server payloads are serializable");
    let _ = framing::write_message(writer, &RawMessage { body });
}
