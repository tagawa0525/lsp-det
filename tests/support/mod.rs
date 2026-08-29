//! 準拠テストスイートの偽クライアント（v0.1-design.md 6 章）。
//!
//! 被験者は「stdio で LSP を話すコマンド」であればなんでもよい。lsp-det は
//! 最初の被験者に過ぎず、実サーバーにも同じスイートを当てられることが
//! この成果物の要件である（設計 6 章）。そのため被験者は
//! [`ServerUnderTest`] というコマンド記述として渡す。

#![allow(dead_code)] // 被験者ごとに使うヘルパーが異なる

use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::Duration;

use lsp_det::framing::{self, RawMessage};
use lsp_det::state::ServerState;
use serde_json::{Value, json};

/// 応答・通知を待つ既定の上限。超えたらテストは失敗する（黙って通さない）。
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// 被験者となるサーバーの起動方法。
pub struct ServerUnderTest {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// `rootUri` に使うディレクトリ。
    pub root: PathBuf,
}

impl ServerUnderTest {
    /// lsp-det（rust-analyzer アダプタ）+ 偽上流。CI で決定的に動く既定の被験者。
    pub fn lsp_det_with_fake_upstream() -> Self {
        ServerUnderTest {
            program: lsp_det_binary(),
            args: vec![
                "--adapter".to_string(),
                "rust-analyzer".to_string(),
                "--".to_string(),
                fake_upstream_binary().to_string_lossy().into_owned(),
            ],
            root: repo_root(),
        }
    }
}

pub fn lsp_det_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_lsp-det"))
}

/// `target/<profile>/examples/fake_lsp_server`。
/// テストバイナリは `target/<profile>/deps/` に置かれるので、そこから辿る。
pub fn fake_upstream_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("test binary path");
    path.pop(); // deps/
    path.pop(); // <profile>/
    path.push("examples");
    path.push("fake_lsp_server");
    assert!(
        path.exists(),
        "偽上流 {} が無い。examples をビルドしない起動方法\
         (`cargo test --test conformance` 単体など) で走らせている。\
         `cargo test` か `cargo build --examples` を先に実行すること",
        path.display()
    );
    path
}

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

enum Incoming {
    Message(Value),
    Closed,
}

/// 拡張 S の準拠を確かめる偽クライアント。
pub struct ConformanceClient {
    child: Child,
    stdin: ChildStdin,
    incoming: Receiver<Incoming>,
    /// 受信済みだが取り出されていない通知。
    pending_notifications: Vec<Value>,
    next_id: i64,
}

impl ConformanceClient {
    pub fn start(server: &ServerUnderTest) -> Self {
        let mut child = Command::new(&server.program)
            .args(&server.args)
            .current_dir(&server.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|err| panic!("被験者 {:?} を起動できない: {err}", server.program));

        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (tx, rx) = channel();
        spawn_reader(stdout, tx);

        ConformanceClient {
            child,
            stdin,
            incoming: rx,
            pending_notifications: Vec::new(),
            next_id: 1,
        }
    }

    /// `initialize` → `initialized` を済ませ、`InitializeResult` を返す。
    ///
    /// `declare_extension_s` は仕様 5.2 のクライアント宣言
    /// （`experimental.serverState: true`）を送るかどうか。
    pub fn initialize(&mut self, declare_extension_s: bool) -> Value {
        let mut capabilities = json!({"textDocument": {"hover": {}}});
        if declare_extension_s {
            capabilities["experimental"] = json!({"serverState": true});
        }
        let result = self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": null,
                "capabilities": capabilities,
            }),
        );
        self.notify("initialized", json!({}));
        result
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }));
        self.await_response(id)
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// 偽上流に `experimental/serverStatus` を送らせる（偽上流専用の制御）。
    pub fn make_upstream_emit_status(&mut self, health: &str, quiescent: bool) {
        self.notify(
            "$/fake/emitServerStatus",
            json!({"health": health, "quiescent": quiescent}),
        );
    }

    /// 拡張 S の状態を問い合わせる（仕様 4.1）。
    pub fn server_state(&mut self) -> ServerState {
        let response = self.request("experimental/serverState", json!(null));
        let result = response.get("result").unwrap_or_else(|| {
            panic!("experimental/serverState への応答に result がない: {response}")
        });
        serde_json::from_value(result.clone())
            .unwrap_or_else(|err| panic!("ServerState として読めない ({err}): {result}"))
    }

    /// 次の `experimental/serverStateChanged` を待つ（仕様 4.2）。
    pub fn await_state_changed(&mut self) -> ServerState {
        let params = self
            .await_notification("experimental/serverStateChanged")
            .unwrap_or_else(|| panic!("experimental/serverStateChanged が届かなかった"));
        serde_json::from_value(params.clone())
            .unwrap_or_else(|err| panic!("ServerState として読めない ({err}): {params}"))
    }

    /// 指定した通知を待ち、その params を返す。時間内に来なければ `None`。
    pub fn await_notification(&mut self, method: &str) -> Option<Value> {
        if let Some(index) = self
            .pending_notifications
            .iter()
            .position(|n| n["method"] == method)
        {
            return Some(self.pending_notifications.remove(index)["params"].clone());
        }
        loop {
            let message = self.recv()?;
            if message["method"] == method {
                return Some(message["params"].clone());
            }
            self.stash(message);
        }
    }

    /// 指定した通知が `window` の間に届かないことを確かめる。
    /// 「届かないこと」の検証なので、待ち時間は短く固定する。
    pub fn expect_no_notification(&mut self, method: &str, window: Duration) -> bool {
        if self
            .pending_notifications
            .iter()
            .any(|n| n["method"] == method)
        {
            return false;
        }
        let deadline = std::time::Instant::now() + window;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return true;
            }
            match self.incoming.recv_timeout(remaining) {
                Ok(Incoming::Message(message)) => {
                    if message["method"] == method {
                        return false;
                    }
                    self.stash(message);
                }
                Ok(Incoming::Closed) | Err(RecvTimeoutError::Disconnected) => return true,
                Err(RecvTimeoutError::Timeout) => return true,
            }
        }
    }

    /// 偽上流が受信した method の一覧。転送の有無を確かめるのに使う。
    pub fn upstream_methods_seen(&mut self) -> Vec<String> {
        let response = self.request("$/fake/report", json!(null));
        response["result"]["methodsSeen"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn shutdown(&mut self) {
        let _ = self.request("shutdown", json!(null));
        self.notify("exit", json!(null));
    }

    fn send(&mut self, value: Value) {
        let body = serde_json::to_vec(&value).expect("client payloads are serializable");
        framing::write_message(&mut self.stdin, &RawMessage { body })
            .expect("被験者の stdin へ書けない");
    }

    fn await_response(&mut self, id: i64) -> Value {
        loop {
            let message = self
                .recv()
                .unwrap_or_else(|| panic!("id={id} への応答を待つ間に被験者が沈黙した"));
            if message.get("id").and_then(Value::as_i64) == Some(id)
                && message.get("method").is_none()
            {
                return message;
            }
            self.stash(message);
        }
    }

    fn stash(&mut self, message: Value) {
        if message.get("method").is_some() && message.get("id").is_none() {
            self.pending_notifications.push(message);
        }
        // サーバー→クライアントのリクエストと、他 id への応答は本スイートの
        // 関心外。捨てても被験者は待たないため放置する。
    }

    fn recv(&mut self) -> Option<Value> {
        match self.incoming.recv_timeout(DEFAULT_TIMEOUT) {
            Ok(Incoming::Message(message)) => Some(message),
            Ok(Incoming::Closed) | Err(RecvTimeoutError::Disconnected) => None,
            Err(RecvTimeoutError::Timeout) => None,
        }
    }
}

impl Drop for ConformanceClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn spawn_reader(stdout: ChildStdout, tx: Sender<Incoming>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match framing::read_message(&mut reader) {
                Ok(Some(msg)) => match serde_json::from_slice::<Value>(&msg.body) {
                    Ok(value) => {
                        if tx.send(Incoming::Message(value)).is_err() {
                            return;
                        }
                    }
                    Err(_) => continue,
                },
                Ok(None) | Err(_) => {
                    let _ = tx.send(Incoming::Closed);
                    return;
                }
            }
        }
    });
}

/// `write!` を使うため。
pub fn flush<W: Write>(writer: &mut W) {
    let _ = writer.flush();
}
