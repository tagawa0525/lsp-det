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

use std::io::{self, BufReader};

use lsp_det::framing::{self, RawMessage};
use serde_json::{Value, json};

fn main() {
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
                respond(
                    &mut stdout,
                    id,
                    json!({
                        "capabilities": {
                            "hoverProvider": true,
                            "referencesProvider": true,
                            "experimental": {"fakeUpstreamMarker": true}
                        },
                        "serverInfo": {"name": "fake-lsp-server", "version": "0"}
                    }),
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
