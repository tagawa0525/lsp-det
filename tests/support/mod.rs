//! 準拠テストスイートの偽クライアント（v0.1-design.md 6 章）。
//!
//! 被験者は「stdio で LSP を話すコマンド」であればなんでもよい。lsp-det は
//! 最初の被験者に過ぎず、実サーバーにも同じスイートを当てられることが
//! この成果物の要件である（設計 6 章）。そのため被験者は
//! [`ServerUnderTest`] というコマンド記述として渡す。

#![allow(dead_code)] // 被験者ごとに使うヘルパーが異なる

use std::io::Read;
use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::thread;
use std::time::Duration;

use lsp_det::framing::{self, RawMessage};
use lsp_det::state::{Health, Readiness, ServerState};
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
    /// lsp-det + rust-analyzer と名乗る偽上流。CI で決定的に動く既定の被験者。
    /// lsp-det は `serverInfo.name` で rust-analyzer の写像を選ぶ (設計 4.2)。
    pub fn lsp_det_with_fake_upstream() -> Self {
        Self::lsp_det_with_fake_upstream_flags(&[])
    }

    /// 既知の写像がない名前 (`fake-lsp-server`) を名乗る偽上流 + lsp-det。
    /// 両軸 `unknown` を報告する被験者（仕様 8.2 の 3、8.4 の 1）。
    pub fn lsp_det_without_adapter() -> Self {
        Self::lsp_det_without_adapter_flags(&[])
    }

    pub fn lsp_det_without_adapter_flags(upstream_flags: &[&str]) -> Self {
        Self::lsp_det_with_upstream("fake-lsp-server", upstream_flags)
    }

    /// 偽上流に起動フラグを渡す版（handshake 前後の境界を再現する）。
    pub fn lsp_det_with_fake_upstream_flags(upstream_flags: &[&str]) -> Self {
        Self::lsp_det_with_upstream("rust-analyzer", upstream_flags)
    }

    /// gopls と名乗る偽上流 + lsp-det。lsp-det は gopls の写像を選ぶ。
    pub fn lsp_det_with_fake_gopls() -> Self {
        Self::lsp_det_with_upstream("gopls", &[])
    }

    /// pyright を演じる偽上流 + lsp-det。pyright は `serverInfo` を返さないので
    /// 名乗りは起動ログだけ (ADR 0011 決定 A-2)。lsp-det は pyright の写像を選ぶ。
    pub fn lsp_det_with_fake_pyright() -> Self {
        Self::lsp_det_with_upstream(
            "none",
            &["--startup-log", "Pyright language server 1.1.412 starting"],
        )
    }

    /// typescript-language-server を演じる偽上流 + lsp-det。`serverInfo` を
    /// 返さず、`initialize` 応答の直後に `$/typescriptVersion` を送る。
    pub fn lsp_det_with_fake_typescript_language_server() -> Self {
        Self::lsp_det_with_upstream(
            "none",
            &[
                "--startup-log",
                r#"Using Typescript version (fake) 5.9.3 from path "/fake/tsserver.js""#,
                "--startup-typescript-version",
                "5.9.3",
            ],
        )
    }

    /// 本プロトコルに準拠した偽上流 + lsp-det。上流側は恒等写像になり、
    /// 下流側は上流の状態を境界越しに読む（設計 4.1）。
    pub fn lsp_det_with_conformant_upstream_flags(upstream_flags: &[&str]) -> Self {
        let mut flags = vec!["--declare-server-state-provider"];
        flags.extend_from_slice(upstream_flags);
        Self::lsp_det_with_upstream("fake-lsp-server", &flags)
    }

    /// 名乗る名前と偽上流のフラグを指定する版。
    pub fn lsp_det_with_upstream_flags(server_name: &str, upstream_flags: &[&str]) -> Self {
        Self::lsp_det_with_upstream(server_name, upstream_flags)
    }

    fn lsp_det_with_upstream(server_name: &str, upstream_flags: &[&str]) -> Self {
        let mut args = vec![
            "--".to_string(),
            fake_upstream_binary().to_string_lossy().into_owned(),
            "--server-name".to_string(),
            server_name.to_string(),
        ];
        args.extend(upstream_flags.iter().map(|flag| flag.to_string()));
        ServerUnderTest {
            program: lsp_det_binary(),
            args,
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
    path.push(format!("fake_lsp_server{}", std::env::consts::EXE_SUFFIX));
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

/// `target/<profile>/examples/pseudo_client`（プロセス寿命のテスト専用の
/// 擬似クライアント）。探し方は `fake_upstream_binary` と同じ。
pub fn pseudo_client_binary() -> PathBuf {
    let mut path = fake_upstream_binary();
    path.set_file_name(format!("pseudo_client{}", std::env::consts::EXE_SUFFIX));
    assert!(
        path.exists(),
        "擬似クライアント {} が無い。`cargo test` か `cargo build --examples` を先に実行すること",
        path.display()
    );
    path
}

/// `pid` のプロセスが `window` 以内に消えたら true。10ms ごとに見る。
/// 手元に `Child` がないプロセス（殺した親の子）の終了を確かめるためのもの。
pub fn wait_until_exited(pid: u32, window: Duration) -> bool {
    let deadline = std::time::Instant::now() + window;
    while process_is_alive(pid) {
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

/// `pid` のプロセスがまだ存在するか。シグナル 0 は届け先の存在確認だけをする。
#[cfg(unix)]
pub fn process_is_alive(pid: u32) -> bool {
    // SAFETY: シグナル 0 は何も送らず、対象の存在と権限だけを確かめる。
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// 同上 (Windows)。終了したプロセスはハンドルが開けないか、終了コードが
/// `STILL_ACTIVE` でなくなる。
#[cfg(windows)]
pub fn process_is_alive(pid: u32) -> bool {
    win::process_is_alive(pid)
}

/// `pid` に SIGKILL (Windows は TerminateProcess) を送る。送れたら true。
#[cfg(unix)]
fn force_kill(pid: u32) -> bool {
    // SAFETY: 自分が起動した被験者の子孫にだけ送る。
    unsafe { libc::kill(pid as i32, libc::SIGKILL) == 0 }
}

#[cfg(windows)]
fn force_kill(pid: u32) -> bool {
    win::force_kill(pid)
}

/// テスト補助が使う Windows API。本体 (`src/process/windows.rs`) と同じく
/// 必要な関数だけを直接宣言する。
#[cfg(windows)]
mod win {
    use std::ffi::c_void;

    type Handle = *mut c_void;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const PROCESS_TERMINATE: u32 = 0x0001;
    const STILL_ACTIVE: u32 = 259;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(desired_access: u32, inherit_handle: i32, process_id: u32) -> Handle;
        fn GetExitCodeProcess(process: Handle, exit_code: *mut u32) -> i32;
        fn TerminateProcess(process: Handle, exit_code: u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }

    pub fn process_is_alive(pid: u32) -> bool {
        // SAFETY: 引数は定数と pid。ハンドルは必ず閉じる。
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut code = 0u32;
            let ok = GetExitCodeProcess(handle, &mut code);
            CloseHandle(handle);
            ok != 0 && code == STILL_ACTIVE
        }
    }

    pub fn force_kill(pid: u32) -> bool {
        // SAFETY: 自分が起動した被験者の子孫にだけ送る。ハンドルは必ず閉じる。
        unsafe {
            let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
            if handle.is_null() {
                return false;
            }
            let ok = TerminateProcess(handle, 9);
            CloseHandle(handle);
            ok != 0
        }
    }
}

enum Incoming {
    Message(Value),
    Closed,
}

/// 本プロトコルの準拠を確かめる偽クライアント。
pub struct ConformanceClient {
    child: Child,
    stdin: ChildStdin,
    /// 被験者の stderr。失敗の診断にだけ使う (読むのは被験者が終わった後)。
    stderr: Option<ChildStderr>,
    incoming: Receiver<Incoming>,
    /// 受信済みだが取り出されていないメッセージ（通知・応答・サーバー発
    /// リクエスト）。
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
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|err| panic!("被験者 {:?} を起動できない: {err}", server.program));

        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take();
        let (tx, rx) = channel();
        spawn_reader(stdout, tx);

        ConformanceClient {
            child,
            stdin,
            stderr,
            incoming: rx,
            pending_notifications: Vec::new(),
            next_id: 1,
        }
    }

    /// `initialize` → `initialized` を済ませ、`InitializeResult` を返す。
    ///
    /// `declare_server_state` は仕様 5.2 のクライアント宣言
    /// （`experimental.serverState: true`）を送るかどうか。
    pub fn initialize(&mut self, declare_server_state: bool) -> Value {
        let result = self.initialize_raw(declare_server_state);
        self.notify("initialized", json!({}));
        result
    }

    /// `initialized` を送らずに `initialize` の応答だけを受け取る。
    /// handshake が成立しない場合の検証に使う。
    pub fn initialize_raw(&mut self, declare_server_state: bool) -> Value {
        let mut capabilities = json!({"textDocument": {"hover": {}}});
        if declare_server_state {
            capabilities["experimental"] = json!({"serverState": true});
        }
        self.initialize_raw_with_capabilities(capabilities)
    }

    /// `rootUri` と `workspaceFolders` を指定して `initialize` → `initialized`
    /// を済ませる。gopls はワークスペースフォルダごとに progress を出すので、
    /// フォルダなしだと "Setting up workspace" が出ない。
    pub fn initialize_with_root(
        &mut self,
        declare_server_state: bool,
        root: &std::path::Path,
    ) -> Value {
        let mut capabilities = json!({"textDocument": {"hover": {}}});
        if declare_server_state {
            capabilities["experimental"] = json!({"serverState": true});
        }
        let uri = file_uri(root);
        let result = self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": uri,
                "workspaceFolders": [{"uri": uri, "name": "fixture"}],
                "capabilities": capabilities,
            }),
        );
        self.notify("initialized", json!({}));
        result
    }

    /// 指定した通知を `window` の間だけ待ち、届けば params を返す。
    pub fn await_notification_within(&mut self, method: &str, window: Duration) -> Option<Value> {
        if let Some(index) = self
            .pending_notifications
            .iter()
            .position(|n| n["method"] == method)
        {
            return Some(self.pending_notifications.remove(index)["params"].clone());
        }
        let deadline = std::time::Instant::now() + window;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match self.incoming.recv_timeout(remaining) {
                Ok(Incoming::Message(message)) => {
                    if message["method"] == method {
                        return Some(message["params"].clone());
                    }
                    self.stash(message);
                }
                Ok(Incoming::Closed) | Err(RecvTimeoutError::Disconnected) => return None,
                Err(RecvTimeoutError::Timeout) => return None,
            }
        }
    }

    /// 任意の `ClientCapabilities` で `initialize` → `initialized` を済ませる。
    /// `initializationOptions` 付きの `initialize` → `initialized`。
    pub fn initialize_with_initialization_options(
        &mut self,
        declare_server_state: bool,
        options: Value,
    ) -> Value {
        let mut capabilities = json!({"textDocument": {"hover": {}}});
        if declare_server_state {
            capabilities["experimental"] = json!({"serverState": true});
        }
        let result = self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": null,
                "capabilities": capabilities,
                "initializationOptions": options,
            }),
        );
        self.notify("initialized", json!({}));
        result
    }

    pub fn initialize_with_capabilities(&mut self, capabilities: Value) -> Value {
        let result = self.initialize_raw_with_capabilities(capabilities);
        self.notify("initialized", json!({}));
        result
    }

    pub fn initialize_raw_with_capabilities(&mut self, capabilities: Value) -> Value {
        self.request(
            "initialize",
            json!({
                "processId": std::process::id(),
                "rootUri": null,
                "capabilities": capabilities,
            }),
        )
    }

    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.send_request(method, params);
        self.await_response(id)
    }

    /// 応答を待たずにリクエストを送り、id を返す。保留の検証に使う。
    pub fn send_request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }));
        id
    }

    /// `send_request` で送ったリクエストへの応答を待つ。
    pub fn await_response_to(&mut self, id: i64) -> Value {
        self.await_response(id)
    }

    /// `$/cancelRequest` を送る。
    pub fn cancel(&mut self, id: i64) {
        self.notify("$/cancelRequest", json!({"id": id}));
    }

    /// `id` への応答が `window` の間に届かないことを確かめる。
    /// 届いたら `Some(応答)` を返す（保留されずに通ったことの検出）。
    pub fn response_within(&mut self, id: i64, window: Duration) -> Option<Value> {
        if let Some(index) = self.pending_notifications.iter().position(|m| {
            m.get("id").and_then(Value::as_i64) == Some(id) && m.get("method").is_none()
        }) {
            return Some(self.pending_notifications.remove(index));
        }
        let deadline = std::time::Instant::now() + window;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match self.incoming.recv_timeout(remaining) {
                Ok(Incoming::Message(message)) => {
                    if message.get("id").and_then(Value::as_i64) == Some(id)
                        && message.get("method").is_none()
                    {
                        return Some(message);
                    }
                    self.stash(message);
                }
                Ok(Incoming::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    panic!("id={id} への応答を待つ間に被験者が沈黙した")
                }
                Err(RecvTimeoutError::Timeout) => return None,
            }
        }
    }

    /// `textDocument/references` を送るだけ（応答は待たない）。
    pub fn send_references(&mut self) -> i64 {
        self.send_request(
            "textDocument/references",
            json!({
                "textDocument": {"uri": "file:///fake/a.rs"},
                "position": {"line": 0, "character": 0},
                "context": {"includeDeclaration": false},
            }),
        )
    }

    /// `textDocument/hover` を送るだけ（応答は待たない）。
    pub fn send_hover(&mut self) -> i64 {
        self.send_request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": "file:///fake/a.rs"},
                "position": {"line": 0, "character": 0},
            }),
        )
    }

    /// 準拠した偽上流に `experimental/serverStateChanged` を送らせる
    /// （偽上流専用の制御）。
    pub fn make_upstream_emit_server_state_changed(&mut self, health: &str, readiness: &str) {
        self.notify(
            "$/fake/emitServerStateChanged",
            json!({"health": health, "readiness": readiness}),
        );
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

    /// 偽上流に `$/progress` を送らせる（偽上流専用の制御）。gopls の
    /// `{"token", "value": {"kind", "title", "message"}}` をそのまま渡す。
    pub fn make_upstream_emit_progress(&mut self, params: Value) {
        self.notify("$/fake/emitProgress", params);
    }

    /// 偽上流に `window/logMessage` を送らせる。
    pub fn make_upstream_emit_log_message(&mut self, kind: u8, message: &str) {
        self.notify(
            "$/fake/emitLogMessage",
            json!({"type": kind, "message": message}),
        );
    }

    /// pyright 風の "Starting service instance" (フォルダごとに 1 回、info)。
    pub fn make_upstream_start_service_instance(&mut self, folder: &str) {
        self.make_upstream_emit_log_message(3, &format!("Starting service instance \"{folder}\""));
    }

    /// pyright 風のファイル列挙完了 (info)。
    pub fn make_upstream_finish_enumeration(&mut self, message: &str) {
        self.make_upstream_emit_log_message(3, message);
    }

    /// typescript-language-server 風のプロジェクトロードの begin。
    pub fn make_upstream_begin_project_load(&mut self, token: &str) {
        self.make_upstream_emit_progress(json!({
            "token": token,
            "value": {"kind": "begin", "title": "Initializing JS/TS language features…"}
        }));
    }

    /// 同じく end。
    pub fn make_upstream_end_project_load(&mut self, token: &str) {
        self.make_upstream_emit_progress(json!({
            "token": token,
            "value": {"kind": "end"}
        }));
    }

    /// 被験者 (lsp-det) の pid。子孫プロセスを探すのに使う。
    pub fn server_pid(&self) -> u32 {
        self.child.id()
    }

    /// gopls 風の "Setting up workspace" の begin。
    pub fn make_upstream_begin_workspace_load(&mut self, token: &str) {
        self.make_upstream_emit_progress(json!({
            "token": token,
            "value": {"kind": "begin", "title": "Setting up workspace", "message": "Loading packages...", "cancellable": false}
        }));
    }

    /// gopls 風の "Setting up workspace" の end。
    pub fn make_upstream_end_workspace_load(&mut self, token: &str, message: &str) {
        self.make_upstream_emit_progress(json!({
            "token": token,
            "value": {"kind": "end", "message": message}
        }));
    }

    /// `message` 付きで `experimental/serverStatus` を送らせる。
    pub fn make_upstream_emit_status_with_message(
        &mut self,
        health: &str,
        quiescent: bool,
        message: &str,
    ) {
        self.notify(
            "$/fake/emitServerStatus",
            json!({"health": health, "quiescent": quiescent, "message": message}),
        );
    }

    /// 本プロトコルの状態を問い合わせる（仕様 4.1）。
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
    ///
    /// 観測窓の途中で被験者が死んだ場合は panic する。沈黙していたのか
    /// 落ちたのかを区別せずに成功とすると、クラッシュした被験者が
    /// この検査を通ってしまう。
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
                Ok(Incoming::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    panic!("{method} が来ないことを確かめている途中で被験者が沈黙した")
                }
                Err(RecvTimeoutError::Timeout) => return true,
            }
        }
    }

    /// `readiness` が `ready` になるまで `serverStateChanged` を待つ。
    /// 実サーバーは自分のペースで ready になるため、時間ではなく状態で待つ。
    ///
    /// `health` が `error` になったら待つのをやめて失敗する（仕様 6 章 5 項、
    /// 9 章 2 項。待ち続けるのは ADR 0008 が警告する永久待ちそのもの）。
    /// `readiness` が `unknown` の被験者には使えない（永遠に来ない）。
    pub fn wait_until_ready(&mut self) {
        let mut state = self.server_state();
        loop {
            // health を先に見る (仕様 3 章の推奨解釈)。
            assert!(
                state.health != Health::Error,
                "ready を待つ間に被験者が壊れた: {state:?}"
            );
            if state.readiness == Readiness::Ready {
                return;
            }
            assert_ne!(
                state.readiness,
                Readiness::Unknown,
                "readiness を観測しない被験者に ready を待たせている"
            );
            state = self.await_state_changed();
        }
    }

    /// 被験者が接続を閉じるまで読み、その間に `method` が届かなかったことを
    /// 確かめる。死んでいく被験者に対する「沈黙の検証」に使う
    /// (`expect_no_notification` は閉じると panic するため使えない)。
    ///
    /// 閉じた後は終了コードが 0 であることも確かめる。panic や異常終了で
    /// stdout が閉じただけの被験者を「意図して沈黙した」と誤認しないため。
    pub fn expect_silence_until_closed(&mut self, method: &str) -> bool {
        if self
            .pending_notifications
            .iter()
            .any(|n| n["method"] == method)
        {
            return false;
        }
        let deadline = std::time::Instant::now() + DEFAULT_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match self.incoming.recv_timeout(remaining) {
                Ok(Incoming::Message(message)) => {
                    if message["method"] == method {
                        return false;
                    }
                    self.stash(message);
                }
                Ok(Incoming::Closed) | Err(RecvTimeoutError::Disconnected) => break,
                Err(RecvTimeoutError::Timeout) => {
                    panic!("{method} の沈黙を確かめている間、被験者が閉じなかった")
                }
            }
        }
        let status = self.child.wait().expect("被験者の終了を待てない");
        assert!(
            status.success(),
            "被験者が異常終了した ({status})。沈黙ではなく墜落である"
        );
        true
    }

    /// 被験者が接続を閉じるまで読み、その間に**応答**（id 付きで method の
    /// ないメッセージ）が届かなかったことを確かめる。応答済みの id に
    /// 二重応答しないことの検証に使う。
    pub fn expect_no_response_until_closed(&mut self) -> bool {
        let deadline = std::time::Instant::now() + DEFAULT_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match self.incoming.recv_timeout(remaining) {
                Ok(Incoming::Message(message)) => {
                    if message.get("id").is_some() && message.get("method").is_none() {
                        return false;
                    }
                    self.stash(message);
                }
                Ok(Incoming::Closed) | Err(RecvTimeoutError::Disconnected) => return true,
                Err(RecvTimeoutError::Timeout) => {
                    panic!("応答が来ないことを確かめている間、被験者が閉じなかった")
                }
            }
        }
    }

    pub fn did_open(&mut self, path: &std::path::Path, language_id: &str) {
        let text = std::fs::read_to_string(path).expect("開くファイルを読めない");
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": file_uri(path), "languageId": language_id,
                "version": 1, "text": text,
            }}),
        );
    }

    /// 全文置換の `didChange`。仕様 6.2 の鮮度保証が対象とする通知。
    /// `workspace/didChangeWatchedFiles`。`kind` は LSP の FileChangeType
    /// (1 = Created, 2 = Changed, 3 = Deleted)。
    pub fn did_change_watched_files(&mut self, changes: &[(&std::path::Path, u8)]) {
        let changes: Vec<Value> = changes
            .iter()
            .map(|(path, kind)| json!({"uri": file_uri(path), "type": kind}))
            .collect();
        self.notify(
            "workspace/didChangeWatchedFiles",
            json!({"changes": changes}),
        );
    }

    pub fn did_change(&mut self, path: &std::path::Path, version: i64, text: &str) {
        self.notify(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": file_uri(path), "version": version},
                "contentChanges": [{"text": text}],
            }),
        );
    }

    /// `textDocument/references`。宣言は含めない（利用箇所だけ数える）。
    pub fn references(&mut self, path: &std::path::Path, line: u32, character: u32) -> Vec<Value> {
        let response = self.request(
            "textDocument/references",
            json!({
                "textDocument": {"uri": file_uri(path)},
                "position": {"line": line, "character": character},
                "context": {"includeDeclaration": false},
            }),
        );
        response["result"].as_array().cloned().unwrap_or_default()
    }

    /// 偽上流が受信した method の一覧。転送の有無を確かめるのに使う。
    pub fn upstream_methods_seen(&mut self) -> Vec<String> {
        self.upstream_report()["methodsSeen"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 偽上流が `initialize` で受け取った `ClientCapabilities`。
    pub fn upstream_client_capabilities(&mut self) -> Value {
        self.upstream_report()["initializeParams"]["capabilities"].clone()
    }

    /// 偽上流が送った `window/workDoneProgress/create` に応答が返ったか。
    pub fn upstream_progress_create_answered(&mut self) -> bool {
        self.upstream_report()["progressCreateAnswered"] == json!(true)
    }

    fn upstream_report(&mut self) -> Value {
        self.request("$/fake/report", json!(null))["result"].clone()
    }

    pub fn shutdown(&mut self) {
        let _ = self.request("shutdown", json!(null));
        self.notify("exit", json!(null));
    }

    fn send(&mut self, value: Value) {
        let body = serde_json::to_vec(&value).expect("client payloads are serializable");
        if let Err(err) = framing::write_message(&mut self.stdin, &RawMessage { body }) {
            // 診断のために stderr を EOF まで読む。生きている被験者を相手に
            // 読むとハングするので、先に終了を確定させる。
            let status = self.child.try_wait();
            let _ = self.child.kill();
            let _ = self.child.wait();
            let mut log = String::new();
            if let Some(mut stderr) = self.stderr.take() {
                let _ = stderr.read_to_string(&mut log);
            }
            panic!(
                "被験者の stdin へ書けない: {err} (書き込み時点の被験者の状態: {status:?})\n被験者の stderr:\n{log}"
            );
        }
    }

    fn await_response(&mut self, id: i64) -> Value {
        if let Some(index) = self.pending_notifications.iter().position(|m| {
            m.get("id").and_then(Value::as_i64) == Some(id) && m.get("method").is_none()
        }) {
            return self.pending_notifications.remove(index);
        }
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
        // 通知も、他 id への応答も、サーバー発リクエストも取っておく。
        // 保留の検証では「後から届く応答」を拾う必要がある。
        self.pending_notifications.push(message);
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

/// `file://` URI。テスト用なのでパーセントエンコードは扱わない
/// (一時ディレクトリ名を ASCII に限る前提)。
pub fn file_uri(path: &std::path::Path) -> String {
    format!("file://{}", path.display())
}

/// 一時的な cargo プロジェクト。クロスファイルの問い合わせには、
/// 別ファイルから参照されるシンボルを持つ実プロジェクトが要る。
pub struct TempCargoProject {
    pub root: PathBuf,
}

impl TempCargoProject {
    /// `a::target` を `b::caller` から呼ぶ 2 ファイル構成を作る。
    pub fn with_cross_file_reference(tag: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("lsp-det-conformance-{tag}-{}", std::process::id()));
        let src = root.join("src");
        std::fs::create_dir_all(&src).expect("一時プロジェクトを作れない");
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"conformance-fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\n",
        )
        .unwrap();
        std::fs::write(src.join("lib.rs"), "pub mod a;\npub mod b;\n").unwrap();
        std::fs::write(src.join("a.rs"), A_RS).unwrap();
        std::fs::write(src.join("b.rs"), B_WITH_CALL).unwrap();
        TempCargoProject { root }
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.root.join("src").join(name)
    }
}

impl TempCargoProject {
    /// 接頭辞 `wsymprobe` を共有する `n` 個のトップレベル関数 (3 ファイルに分ける)。
    /// 仕様 7.2 の 2 (件数の上限) の fixture。
    pub fn with_many_symbols(tag: &str, n: usize) -> Self {
        let project = Self::with_cross_file_reference(tag);
        let src = project.root.join("src");
        let mut lib = String::from("pub mod a;\npub mod b;\n");
        for file in 0..3 {
            lib.push_str(&format!("pub mod s{file};\n"));
            let body: String = (0..n)
                .filter(|i| i % 3 == file)
                .map(|i| format!("pub fn wsymprobe_{i:03}() {{}}\n"))
                .collect();
            std::fs::write(src.join(format!("s{file}.rs")), body).unwrap();
        }
        std::fs::write(src.join("lib.rs"), lib).unwrap();
        project
    }
}

impl Drop for TempCargoProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// 一時的な Go モジュール。`fixture.Target` を `b.go` の `Caller` から呼ぶ。
pub struct TempGoProject {
    pub root: PathBuf,
}

impl TempGoProject {
    pub fn with_cross_file_reference(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lsp-det-conformance-go-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("一時モジュールを作れない");
        std::fs::write(root.join("go.mod"), "module fixture\n\ngo 1.21\n").unwrap();
        std::fs::write(root.join("a.go"), GO_A).unwrap();
        std::fs::write(root.join("b.go"), GO_B_WITH_CALL).unwrap();
        TempGoProject { root }
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl TempGoProject {
    pub fn with_many_symbols(tag: &str, n: usize) -> Self {
        let project = Self::with_cross_file_reference(tag);
        for file in 0..3 {
            let body: String = std::iter::once("package fixture\n\n".to_string())
                .chain(
                    (0..n)
                        .filter(|i| i % 3 == file)
                        .map(|i| format!("func Wsymprobe{i:03}() {{}}\n")),
                )
                .collect();
            std::fs::write(project.root.join(format!("s{file}.go")), body).unwrap();
        }
        project
    }
}

impl Drop for TempGoProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// `pid` の子孫のうち、コマンドラインに `needle` を含むものに SIGKILL
/// (Windows は TerminateProcess) を送る。実 typescript-language-server の
/// tsserver (孫プロセス) を落とすのに使う。殺した pid を返す。
pub fn kill_descendants_matching(pid: u32, needle: &str) -> Vec<u32> {
    let mut killed = Vec::new();
    let mut frontier = vec![pid];
    while let Some(parent) = frontier.pop() {
        for (child, cmdline) in children_of(parent) {
            frontier.push(child);
            // 送れたときだけ「殺した」と数える (既に消えていれば失敗する)。
            if cmdline.contains(needle) && force_kill(child) {
                killed.push(child);
            }
        }
    }
    killed
}

/// `parent` の直接の子と、そのコマンドライン。親が消えていれば空。
#[cfg(target_os = "linux")]
fn children_of(parent: u32) -> Vec<(u32, String)> {
    pgrep_children(parent)
        .into_iter()
        .map(|child| {
            let cmdline = std::fs::read(format!("/proc/{child}/cmdline")).unwrap_or_default();
            (child, String::from_utf8_lossy(&cmdline).into_owned())
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn children_of(parent: u32) -> Vec<(u32, String)> {
    pgrep_children(parent)
        .into_iter()
        .map(|child| {
            let out = std::process::Command::new("ps")
                .args(["-o", "command=", "-p", &child.to_string()])
                .output()
                .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
                .unwrap_or_default();
            (child, out)
        })
        .collect()
}

#[cfg(windows)]
fn children_of(parent: u32) -> Vec<(u32, String)> {
    let script = format!(
        "Get-CimInstance Win32_Process | Where-Object {{ $_.ParentProcessId -eq {parent} }} \
         | ForEach-Object {{ \"$($_.ProcessId)`t$($_.CommandLine)\" }}"
    );
    let Ok(out) = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &script])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let (pid, cmdline) = line.split_once('\t')?;
            Some((pid.trim().parse().ok()?, cmdline.to_string()))
        })
        .collect()
}

/// `pgrep -P` で直接の子を列挙する。pgrep が失敗しても (その親が消えて
/// いても) 空を返して残りの探索は続く。
#[cfg(unix)]
fn pgrep_children(parent: u32) -> Vec<u32> {
    let Ok(out) = std::process::Command::new("pgrep")
        .args(["-P", &parent.to_string()])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|s| s.parse::<u32>().ok())
        .collect()
}

/// 一時的な TypeScript プロジェクト。`a.ts` の `target` を `b.ts` の `caller` から呼ぶ。
pub struct TempTsProject {
    pub root: PathBuf,
}

impl TempTsProject {
    pub fn with_cross_file_reference(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lsp-det-conformance-ts-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("一時プロジェクトを作れない");
        std::fs::write(root.join("tsconfig.json"), TSCONFIG).unwrap();
        std::fs::write(root.join("a.ts"), TS_A).unwrap();
        std::fs::write(root.join("b.ts"), TS_B_WITH_CALL).unwrap();
        TempTsProject { root }
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl TempTsProject {
    pub fn with_many_symbols(tag: &str, n: usize) -> Self {
        let project = Self::with_cross_file_reference(tag);
        for file in 0..3 {
            let body: String = (0..n)
                .filter(|i| i % 3 == file)
                .map(|i| {
                    format!("export function wsymprobe_{i:03}(): number {{\n  return 1;\n}}\n")
                })
                .collect();
            std::fs::write(project.root.join(format!("s{file}.ts")), body).unwrap();
        }
        project
    }
}

impl Drop for TempTsProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub const TSCONFIG: &str = r#"{"compilerOptions":{"strict":true,"module":"esnext","target":"es2020","moduleResolution":"bundler"},"include":["**/*.ts"]}"#;
/// `target` は 1 行目の 17 文字目 (0 起点で line 0, character 16) にある。
pub const TS_A: &str = "export function target(): number {\n  return 1;\n}\n";
/// 呼び出しは 4 行目 (0 起点で line 3)。1 行目の import も参照として数えられる。
pub const TS_B_WITH_CALL: &str =
    "import { target } from './a';\n\nexport function caller(): number {\n  return target();\n}\n";
pub const TS_B_WITHOUT_CALL: &str = "export function caller(): number {\n  return 1;\n}\n";
pub const TS_B_WITH_TWO_CALLS: &str = "import { target } from './a';\n\nexport function caller(): number {\n  target();\n  return target();\n}\n";
pub const TS_C_WITH_CALL: &str =
    "import { target } from './a';\n\nexport function other(): number {\n  return target();\n}\n";

/// 一時的な Python プロジェクト。`a.py` の `target` を `b.py` の `caller` から呼ぶ。
/// pyright は `initialize` の `workspaceFolders` をフォルダごとの service
/// instance にし、そのフォルダ以下を列挙する。
pub struct TempPyProject {
    pub root: PathBuf,
}

impl TempPyProject {
    pub fn with_cross_file_reference(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lsp-det-conformance-py-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("一時プロジェクトを作れない");
        std::fs::write(root.join("a.py"), PY_A).unwrap();
        std::fs::write(root.join("b.py"), PY_B_WITH_CALL).unwrap();
        TempPyProject { root }
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl TempPyProject {
    pub fn with_many_symbols(tag: &str, n: usize) -> Self {
        let project = Self::with_cross_file_reference(tag);
        for file in 0..3 {
            let body: String = (0..n)
                .filter(|i| i % 3 == file)
                .map(|i| format!("def wsymprobe_{i:03}():\n    return 1\n\n\n"))
                .collect();
            std::fs::write(project.root.join(format!("s{file}.py")), body).unwrap();
        }
        project
    }
}

impl Drop for TempPyProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// `target` は 1 行目の 5 文字目 (0 起点で line 0, character 4) にある。
pub const PY_A: &str = "def target():\n    return 1\n";
/// 呼び出しは 5 行目 (0 起点で line 4)。1 行目の import も参照として数えられる。
pub const PY_B_WITH_CALL: &str = "from a import target\n\n\ndef caller():\n    return target()\n";
pub const PY_B_WITHOUT_CALL: &str = "def caller():\n    return 1\n";
pub const PY_B_WITH_TWO_CALLS: &str =
    "from a import target\n\n\ndef caller():\n    target()\n    return target()\n";
pub const PY_C_WITH_CALL: &str = "import a\n\n\ndef other():\n    return a.target()\n";

/// `Target` は 3 行目の 6 文字目 (0 起点で line 2, character 5) にある。
pub const GO_A: &str = "package fixture\n\nfunc Target() {}\n";
/// 呼び出しは 4 行目 (0 起点で line 3)。
pub const GO_B_WITH_CALL: &str = "package fixture\n\nfunc Caller() {\n\tTarget()\n}\n";
pub const GO_B_WITHOUT_CALL: &str = "package fixture\n\nfunc Caller() {}\n";
pub const GO_B_WITH_TWO_CALLS: &str =
    "package fixture\n\nfunc Caller() {\n\tTarget()\n\tTarget()\n}\n";
pub const GO_C_WITH_CALL: &str = "package fixture\n\nfunc Other() {\n\tTarget()\n}\n";

/// `target` は 1 行目の 8 文字目 (0 起点で line 0, character 7) にある。
pub const A_RS: &str = "pub fn target() {}\n";
pub const B_WITH_CALL: &str = "use crate::a::target;\n\npub fn caller() {\n    target();\n}\n";
pub const B_WITHOUT_CALL: &str = "pub fn caller() {}\n";
/// ディスク上の変更 (仕様 7.3 の 2): 呼び出しを 1 つ足す。
pub const B_WITH_TWO_CALLS: &str =
    "use crate::a::target;\n\npub fn caller() {\n    target();\n    target();\n}\n";
/// 新規ファイル (仕様 7.3 の 2): 別のファイルからも呼ぶ。Rust では `mod` で
/// 名指しされるまで crate に入らないので、lib.rs も変える。
pub const C_RS_WITH_CALL: &str = "pub fn other() {\n    crate::a::target();\n}\n";
pub const LIB_RS_WITH_C: &str = "pub mod a;\npub mod b;\npub mod c;\n";

/// `write!` を使うため。
pub fn flush<W: Write>(writer: &mut W) {
    let _ = writer.flush();
}
