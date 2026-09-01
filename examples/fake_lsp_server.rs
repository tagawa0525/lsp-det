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
//! - `$/fake/report`（リクエスト）: これまでに受信した method の一覧と、
//!   `initialize` で受け取った params を返す。上流へ何が転送されたかを
//!   テスト側から検証するために使う
//!
//! handshake 前後の境界を再現するための起動フラグ:
//!
//! - `--exit-before-initialize-result`: `initialize` を受け取った瞬間に、
//!   応答せず終了する（起動時クラッシュ）
//! - `--status-before-initialize-result`: `InitializeResult` より**前**に
//!   `experimental/serverStatus` を送る
//! - `--declare-server-state-provider`: 上流自身が拡張 S に準拠している
//!   ふりをする。`InitializeResult` に `serverStateProvider: {freshness: true}`
//!   を宣言し、`experimental/serverState` に自分で答える
//! - `--declare-server-state-provider-false`: `serverStateProvider: false` を
//!   宣言する（`hoverProvider: false` と同じ「提供しない」の書き方）
//! - `--fail-first-initialize`: 最初の `initialize` にエラーで応答する。
//!   2 回目以降は通常どおり

use std::io::{self, BufReader};

use lsp_det::framing::{self, RawMessage};
use serde_json::{Value, json};

fn main() {
    let flags: Vec<String> = std::env::args().skip(1).collect();
    let has = |name: &str| flags.iter().any(|flag| flag == name);
    let exit_before_initialize_result = has("--exit-before-initialize-result");
    let status_before_initialize_result = has("--status-before-initialize-result");
    let declare_server_state_provider = has("--declare-server-state-provider");
    let declare_server_state_provider_false = has("--declare-server-state-provider-false");
    let fail_first_initialize = has("--fail-first-initialize");
    let mut initialize_failed_once = false;

    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut stdout = io::stdout();

    let mut methods_seen: Vec<String> = Vec::new();
    let mut initialize_params = Value::Null;

    while let Ok(Some(msg)) = framing::read_message(&mut reader) {
        let Ok(value) = serde_json::from_slice::<Value>(&msg.body) else {
            continue;
        };
        let method = value.get("method").and_then(Value::as_str).unwrap_or("");
        let id = value.get("id").cloned();
        let params = value.get("params").cloned().unwrap_or(Value::Null);

        if !method.is_empty() {
            methods_seen.push(method.to_string());
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
                    continue;
                }
                if exit_before_initialize_result {
                    return;
                }
                if status_before_initialize_result {
                    send(
                        &mut stdout,
                        json!({
                            "jsonrpc": "2.0",
                            "method": "experimental/serverStatus",
                            "params": {"health": "ok", "quiescent": true}
                        }),
                    );
                }
                let mut experimental = json!({"fakeUpstreamMarker": true});
                if declare_server_state_provider {
                    experimental["serverStateProvider"] = json!({"freshness": true});
                }
                if declare_server_state_provider_false {
                    experimental["serverStateProvider"] = json!(false);
                }
                respond(
                    &mut stdout,
                    id,
                    json!({
                        "capabilities": {
                            "hoverProvider": true,
                            "referencesProvider": true,
                            "experimental": experimental
                        },
                        "serverInfo": {"name": "fake-lsp-server", "version": "0"}
                    }),
                );
            }
            "experimental/serverState" if declare_server_state_provider => {
                respond(
                    &mut stdout,
                    id,
                    json!({"health": "ok", "readiness": "ready", "message": "answered by upstream"}),
                );
            }
            "$/fake/emitServerStatus" => {
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
                        "initializeParams": initialize_params
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
