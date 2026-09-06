//! The fake client for the conformance test suite (v0.1-design.md chapter 6).
//!
//! The subject can be any "command that speaks LSP over stdio". lsp-det is only the first
//! subject; being able to apply the same suite to real servers is a requirement of this
//! deliverable (design chapter 6). The subject is therefore passed as a command description,
//! [`ServerUnderTest`].

#![allow(dead_code)] // which helpers are used differs per subject

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

/// Default limit for waiting on a response or notification. Exceeding it fails the test (never
/// passes silently).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// How to launch the server that is the subject.
pub struct ServerUnderTest {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// The directory to use as `rootUri`.
    pub root: PathBuf,
}

impl ServerUnderTest {
    /// lsp-det + a fake upstream that calls itself rust-analyzer. The default subject, which runs
    /// deterministically in CI. lsp-det selects the rust-analyzer mapping by `serverInfo.name`
    /// (design 4.2).
    pub fn lsp_det_with_fake_upstream() -> Self {
        Self::lsp_det_with_fake_upstream_flags(&[])
    }

    /// A fake upstream that calls itself by a name with no known mapping (`fake-lsp-server`) +
    /// lsp-det. A subject that reports `unknown` on both axes (spec 8.2 item 3, 8.4 item 1).
    pub fn lsp_det_without_adapter() -> Self {
        Self::lsp_det_without_adapter_flags(&[])
    }

    pub fn lsp_det_without_adapter_flags(upstream_flags: &[&str]) -> Self {
        Self::lsp_det_with_upstream("fake-lsp-server", upstream_flags)
    }

    /// The variant that passes launch flags to the fake upstream (reproduces the boundary around
    /// the handshake).
    pub fn lsp_det_with_fake_upstream_flags(upstream_flags: &[&str]) -> Self {
        Self::lsp_det_with_upstream("rust-analyzer", upstream_flags)
    }

    /// A fake upstream that calls itself gopls + lsp-det. lsp-det selects the gopls mapping.
    pub fn lsp_det_with_fake_gopls() -> Self {
        Self::lsp_det_with_upstream("gopls", &[])
    }

    /// A fake upstream that calls itself Metals + lsp-det. lsp-det selects the Metals mapping
    /// (M9, ADR 0019 decision F).
    pub fn lsp_det_with_fake_metals() -> Self {
        Self::lsp_det_with_upstream("Metals", &[])
    }

    /// A fake upstream that calls itself Expert + lsp-det. lsp-det selects the Expert mapping
    /// (M10, ADR 0019 decision F).
    pub fn lsp_det_with_fake_expert() -> Self {
        Self::lsp_det_with_upstream("Expert", &[])
    }

    /// A fake upstream that returns no `serverInfo` and calls itself haskell-language-server only
    /// through its pid-prefixed `executeCommandProvider.commands` (as the real one does) +
    /// lsp-det. lsp-det selects the HLS mapping (M15).
    pub fn lsp_det_with_fake_haskell_language_server() -> Self {
        Self::lsp_det_with_upstream(
            "none",
            &[
                "--execute-commands",
                "4242:ghcide-type-lenses:typesignature.add,4242:eval:evalCommand",
            ],
        )
    }

    /// A fake upstream that returns no `serverInfo` and calls itself Nextflow's language server
    /// only through `executeCommandProvider.commands` (as the real one does) + lsp-det. lsp-det
    /// selects the Nextflow mapping (M12).
    pub fn lsp_det_with_fake_nextflow() -> Self {
        Self::lsp_det_with_upstream(
            "none",
            &[
                "--execute-commands",
                "nextflow.server.previewDag,nextflow.server.previewWorkspace",
            ],
        )
    }

    /// A fake upstream that plays pyright + lsp-det. pyright returns no `serverInfo`, so what the
    /// server calls itself comes only from the startup log (ADR 0011 decision A-2). lsp-det
    /// selects the pyright mapping.
    pub fn lsp_det_with_fake_pyright() -> Self {
        Self::lsp_det_with_upstream(
            "none",
            &["--startup-log", "Pyright language server 1.1.412 starting"],
        )
    }

    /// A fake upstream that plays typescript-language-server + lsp-det. It returns no
    /// `serverInfo` and sends `$/typescriptVersion` right after the `initialize` response.
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

    /// A fake upstream conformant to this protocol + lsp-det. The upstream side becomes the
    /// identity mapping, and the downstream side reads the upstream's state across the boundary
    /// (design 4.1).
    pub fn lsp_det_with_conformant_upstream_flags(upstream_flags: &[&str]) -> Self {
        let mut flags = vec!["--declare-server-state-provider"];
        flags.extend_from_slice(upstream_flags);
        Self::lsp_det_with_upstream("fake-lsp-server", &flags)
    }

    /// The variant that specifies the name the server calls itself and the fake upstream's flags.
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

/// `target/<profile>/examples/fake_lsp_server`.
/// Test binaries are placed in `target/<profile>/deps/`, so walk from there.
pub fn fake_upstream_binary() -> PathBuf {
    let mut path = std::env::current_exe().expect("test binary path");
    path.pop(); // deps/
    path.pop(); // <profile>/
    path.push("examples");
    path.push(format!("fake_lsp_server{}", std::env::consts::EXE_SUFFIX));
    assert!(
        path.exists(),
        "fake upstream {} is missing. It is being run in a way that does not build the examples \
         (such as `cargo test --test conformance` alone). \
         Run `cargo test` or `cargo build --examples` first",
        path.display()
    );
    path
}

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// `target/<profile>/examples/pseudo_client` (the pseudo client used only by the process
/// lifetime tests). Found the same way as `fake_upstream_binary`.
pub fn pseudo_client_binary() -> PathBuf {
    let mut path = fake_upstream_binary();
    path.set_file_name(format!("pseudo_client{}", std::env::consts::EXE_SUFFIX));
    assert!(
        path.exists(),
        "pseudo client {} is missing. Run `cargo test` or `cargo build --examples` first",
        path.display()
    );
    path
}

/// True if the process `pid` disappears within `window`. Checks every 10ms.
/// For confirming the exit of a process we have no `Child` for (a child of a killed parent).
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

/// Whether the process `pid` still exists. Signal 0 only checks that the recipient exists.
#[cfg(unix)]
pub fn process_is_alive(pid: u32) -> bool {
    // SAFETY: signal 0 sends nothing; it only checks that the target exists and that we have
    // permission.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Same as above (Windows). An exited process either cannot have its handle opened, or its exit
/// code is no longer `STILL_ACTIVE`.
#[cfg(windows)]
pub fn process_is_alive(pid: u32) -> bool {
    win::process_is_alive(pid)
}

/// Sends SIGKILL (TerminateProcess on Windows) to `pid`. True if it could be sent.
#[cfg(unix)]
fn force_kill(pid: u32) -> bool {
    // SAFETY: sent only to descendants of a subject we launched ourselves.
    unsafe { libc::kill(pid as i32, libc::SIGKILL) == 0 }
}

#[cfg(windows)]
fn force_kill(pid: u32) -> bool {
    win::force_kill(pid)
}

/// The Windows API used by the test support. Like the main code (`src/process/windows.rs`),
/// declares only the functions it needs directly.
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
        // SAFETY: the arguments are constants and a pid. The handle is always closed.
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
        // SAFETY: sent only to descendants of a subject we launched ourselves. The handle is
        // always closed.
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

/// The fake client that checks conformance to this protocol.
pub struct ConformanceClient {
    child: Child,
    stdin: ChildStdin,
    /// The subject's stderr. Used only to diagnose failures (read after the subject has exited).
    stderr: Option<ChildStderr>,
    incoming: Receiver<Incoming>,
    /// Messages received but not yet taken out (notifications, responses, and server-initiated
    /// requests).
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
            .unwrap_or_else(|err| panic!("cannot launch subject {:?}: {err}", server.program));

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

    /// Completes `initialize` → `initialized` and returns the `InitializeResult`.
    ///
    /// `declare_server_state` is whether to send the client declaration of spec 5.2
    /// (`experimental.serverState: true`).
    pub fn initialize(&mut self, declare_server_state: bool) -> Value {
        let result = self.initialize_raw(declare_server_state);
        self.notify("initialized", json!({}));
        result
    }

    /// Receives only the `initialize` response, without sending `initialized`.
    /// Used to check the case where the handshake does not complete.
    pub fn initialize_raw(&mut self, declare_server_state: bool) -> Value {
        let mut capabilities = json!({"textDocument": {"hover": {}}});
        if declare_server_state {
            capabilities["experimental"] = json!({"serverState": true});
        }
        self.initialize_raw_with_capabilities(capabilities)
    }

    /// Completes `initialize` → `initialized` with `rootUri` and `workspaceFolders` specified.
    /// gopls emits progress per workspace folder, so without a folder "Setting up workspace"
    /// does not appear.
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

    /// Waits for the given notification only for `window`, and returns its params if it arrives.
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

    /// Completes `initialize` → `initialized` with arbitrary `ClientCapabilities`.
    /// `initialize` → `initialized` with `initializationOptions`.
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

    /// `initialize` → `initialized` specifying `rootUri` and capabilities.
    pub fn initialize_with_root_and_capabilities(
        &mut self,
        root: &std::path::Path,
        capabilities: Value,
    ) -> Value {
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

    pub fn did_close(&mut self, path: &std::path::Path) {
        self.notify(
            "textDocument/didClose",
            json!({"textDocument": {"uri": file_uri(path)}}),
        );
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

    /// Sends a request without waiting for the response and returns its id. Used to check
    /// holding.
    pub fn send_request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params
        }));
        id
    }

    /// Waits for the response to a request sent with `send_request`.
    pub fn await_response_to(&mut self, id: i64) -> Value {
        self.await_response(id)
    }

    /// Sends `$/cancelRequest`.
    pub fn cancel(&mut self, id: i64) {
        self.notify("$/cancelRequest", json!({"id": id}));
    }

    /// Checks that the response to `id` does not arrive within `window`.
    /// If it arrives, returns `Some(response)` (detects that it passed through without being
    /// held).
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
                    panic!("the subject went silent while waiting for the response to id={id}")
                }
                Err(RecvTimeoutError::Timeout) => return None,
            }
        }
    }

    /// Only sends `textDocument/references` (does not wait for the response).
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

    /// Only sends `textDocument/hover` (does not wait for the response).
    pub fn send_hover(&mut self) -> i64 {
        self.send_request(
            "textDocument/hover",
            json!({
                "textDocument": {"uri": "file:///fake/a.rs"},
                "position": {"line": 0, "character": 0},
            }),
        )
    }

    /// Makes the conformant fake upstream send `experimental/serverStateChanged`
    /// (a control specific to the fake upstream).
    pub fn make_upstream_emit_server_state_changed(&mut self, health: &str, readiness: &str) {
        self.notify(
            "$/fake/emitServerStateChanged",
            json!({"health": health, "readiness": readiness}),
        );
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// Makes the fake upstream send `experimental/serverStatus` (a control specific to the fake
    /// upstream).
    pub fn make_upstream_emit_status(&mut self, health: &str, quiescent: bool) {
        self.notify(
            "$/fake/emitServerStatus",
            json!({"health": health, "quiescent": quiescent}),
        );
    }

    /// Makes the fake upstream send `$/progress` (a control specific to the fake upstream). Passes
    /// gopls's `{"token", "value": {"kind", "title", "message"}}` as is.
    pub fn make_upstream_emit_progress(&mut self, params: Value) {
        self.notify("$/fake/emitProgress", params);
    }

    /// Makes the fake upstream send any notification (`method` with `params`).
    pub fn make_upstream_emit_notification(&mut self, method: &str, params: Value) {
        self.notify(
            "$/fake/emitNotification",
            json!({"method": method, "params": params}),
        );
    }

    /// Makes the fake upstream send `window/logMessage`.
    pub fn make_upstream_emit_log_message(&mut self, kind: u8, message: &str) {
        self.notify(
            "$/fake/emitLogMessage",
            json!({"type": kind, "message": message}),
        );
    }

    /// pyright-style "Starting service instance" (once per folder, info).
    pub fn make_upstream_start_service_instance(&mut self, folder: &str) {
        self.make_upstream_emit_log_message(3, &format!("Starting service instance \"{folder}\""));
    }

    /// pyright-style completion of file enumeration (info).
    pub fn make_upstream_finish_enumeration(&mut self, message: &str) {
        self.make_upstream_emit_log_message(3, message);
    }

    /// typescript-language-server-style begin of a project load.
    pub fn make_upstream_begin_project_load(&mut self, token: &str) {
        self.make_upstream_emit_progress(json!({
            "token": token,
            "value": {"kind": "begin", "title": "Initializing JS/TS language features…"}
        }));
    }

    /// Likewise, end.
    pub fn make_upstream_end_project_load(&mut self, token: &str) {
        self.make_upstream_emit_progress(json!({
            "token": token,
            "value": {"kind": "end"}
        }));
    }

    /// The pid of the subject (lsp-det). Used to find descendant processes.
    pub fn server_pid(&self) -> u32 {
        self.child.id()
    }

    /// gopls-style begin of "Setting up workspace".
    pub fn make_upstream_begin_workspace_load(&mut self, token: &str) {
        self.make_upstream_emit_progress(json!({
            "token": token,
            "value": {"kind": "begin", "title": "Setting up workspace", "message": "Loading packages...", "cancellable": false}
        }));
    }

    /// gopls-style end of "Setting up workspace".
    pub fn make_upstream_end_workspace_load(&mut self, token: &str, message: &str) {
        self.make_upstream_emit_progress(json!({
            "token": token,
            "value": {"kind": "end", "message": message}
        }));
    }

    /// Makes the upstream send `experimental/serverStatus` with a `message`.
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

    /// Requests the state of this protocol (spec 4.1).
    pub fn server_state(&mut self) -> ServerState {
        let response = self.request("experimental/serverState", json!(null));
        let result = response.get("result").unwrap_or_else(|| {
            panic!("the response to experimental/serverState has no result: {response}")
        });
        serde_json::from_value(result.clone())
            .unwrap_or_else(|err| panic!("cannot read as ServerState ({err}): {result}"))
    }

    /// Waits for the next `experimental/serverStateChanged` (spec 4.2).
    pub fn await_state_changed(&mut self) -> ServerState {
        let Some(params) = self.await_notification("experimental/serverStateChanged") else {
            self.fail_with_stderr("experimental/serverStateChanged did not arrive");
        };
        serde_json::from_value(params.clone())
            .unwrap_or_else(|err| panic!("cannot read as ServerState ({err}): {params}"))
    }

    /// Ends the test with the subject's stderr attached (lsp-det's state transitions and
    /// holds, and the upstream's own output), so that a wait that never ends can be read.
    /// Kills the subject first: reading stderr to EOF against a live subject would hang.
    fn fail_with_stderr(&mut self, what: &str) -> ! {
        let status = self.child.try_wait();
        let _ = self.child.kill();
        let _ = self.child.wait();
        let mut log = String::new();
        if let Some(mut stderr) = self.stderr.take() {
            let _ = stderr.read_to_string(&mut log);
        }
        panic!("{what} (the subject's status: {status:?})\nthe subject's stderr:\n{log}");
    }

    /// Waits for the given notification and returns its params. `None` if it does not arrive in
    /// time.
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

    /// Checks that the given notification does not arrive within `window`.
    /// This checks "that it does not arrive", so the wait is short and fixed.
    ///
    /// Panics if the subject dies in the middle of the observation window. Counting that as
    /// success without distinguishing silence from a crash would let a crashed subject pass this
    /// check.
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
                    panic!("the subject went silent while checking that {method} does not arrive")
                }
                Err(RecvTimeoutError::Timeout) => return true,
            }
        }
    }

    /// Waits for `serverStateChanged` until `readiness` becomes `ready`.
    /// Real servers become ready at their own pace, so wait on the state rather than on time.
    ///
    /// Stops waiting and fails if `health` becomes `error` (spec chapter 6 item 5, chapter 9
    /// item 2. Waiting on would be exactly the endless wait that ADR 0008 warns about).
    /// Cannot be used with a subject whose `readiness` is `unknown` (it never arrives).
    pub fn wait_until_ready(&mut self) {
        let mut state = self.server_state();
        loop {
            // Look at health first (the recommended interpretation of spec chapter 3).
            assert!(
                state.health != Health::Error,
                "the subject broke while waiting for ready: {state:?}"
            );
            if state.readiness == Readiness::Ready {
                return;
            }
            assert_ne!(
                state.readiness,
                Readiness::Unknown,
                "waiting for ready on a subject that does not observe readiness"
            );
            state = self.await_state_changed();
        }
    }

    /// Reads until the subject closes the connection, and checks that `method` did not arrive in
    /// the meantime. Used to "check silence" of a subject that is dying
    /// (`expect_no_notification` cannot be used because it panics on close).
    ///
    /// After the close, also checks that the exit code is 0, so that a subject whose stdout merely
    /// closed by a panic or an abnormal exit is not mistaken for one that "went silent on
    /// purpose".
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
                    panic!("the subject did not close while checking the silence of {method}")
                }
            }
        }
        let status = self
            .child
            .wait()
            .expect("cannot wait for the subject to exit");
        assert!(
            status.success(),
            "the subject exited abnormally ({status}). That is a crash, not silence"
        );
        true
    }

    /// Reads until the subject closes the connection, and checks that no **response** (a message
    /// with an id and no method) arrived in the meantime. Used to check that an already answered
    /// id is not answered twice.
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
                    panic!("the subject did not close while checking that no response arrives")
                }
            }
        }
    }

    pub fn did_open(&mut self, path: &std::path::Path, language_id: &str) {
        let text = std::fs::read_to_string(path).expect("cannot read the file to open");
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": file_uri(path), "languageId": language_id,
                "version": 1, "text": text,
            }}),
        );
    }

    /// Full-text replacement `didChange`. The notification covered by the freshness guarantee of
    /// spec 6.2.
    /// `workspace/didChangeWatchedFiles`. `kind` is the LSP FileChangeType
    /// (1 = Created, 2 = Changed, 3 = Deleted).
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

    /// `textDocument/references`. Excludes the declaration (counts only the uses).
    pub fn references(&mut self, path: &std::path::Path, line: u32, character: u32) -> Vec<Value> {
        let params = json!({
            "textDocument": {"uri": file_uri(path)},
            "position": {"line": line, "character": character},
            "context": {"includeDeclaration": false},
        });
        let mut response = self.request("textDocument/references", params.clone());
        if response["error"]["code"] == json!(-32801) {
            // ContentModified: the server discarded a computation during a change. LSP asks the
            // client to resend (rust-analyzer rejects requests right after didChangeWatchedFiles
            // with this). It is not a response, so resend exactly once.
            response = self.request("textDocument/references", params);
        }
        response["result"].as_array().cloned().unwrap_or_default()
    }

    /// The list of methods the fake upstream received. Used to check whether forwarding
    /// happened.
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

    /// The `ClientCapabilities` the fake upstream received in `initialize`.
    /// The params of the `method` notifications that reached the upstream (in arrival order).
    pub fn upstream_notifications(&mut self, method: &str) -> Vec<Value> {
        self.upstream_report()["notifications"][method]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    pub fn upstream_client_capabilities(&mut self) -> Value {
        self.upstream_report()["initializeParams"]["capabilities"].clone()
    }

    /// Whether the `window/workDoneProgress/create` the fake upstream sent was answered.
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

    /// The subject's stderr, read to EOF. Call after `shutdown()`: EOF arrives when the subject
    /// exits, so reading a live subject would hang. The read comes before `wait()`, otherwise a
    /// subject blocked on a full stderr pipe could never exit. The upstream's stderr is relayed
    /// through lsp-det, so the text contains both.
    pub fn stderr_after_exit(&mut self) -> String {
        let mut log = String::new();
        if let Some(mut stderr) = self.stderr.take() {
            let _ = stderr.read_to_string(&mut log);
        }
        let _ = self.child.wait();
        log
    }

    fn send(&mut self, value: Value) {
        let body = serde_json::to_vec(&value).expect("client payloads are serializable");
        if let Err(err) = framing::write_message(&mut self.stdin, &RawMessage { body }) {
            // Read stderr to EOF for diagnosis. Reading it against a live subject would hang, so
            // make sure it has exited first.
            let status = self.child.try_wait();
            let _ = self.child.kill();
            let _ = self.child.wait();
            let mut log = String::new();
            if let Some(mut stderr) = self.stderr.take() {
                let _ = stderr.read_to_string(&mut log);
            }
            panic!(
                "cannot write to the subject's stdin: {err} (the subject's status at the time of \
                 the write: {status:?})\nthe subject's stderr:\n{log}"
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
            let message = self.recv().unwrap_or_else(|| {
                panic!("the subject went silent while waiting for the response to id={id}")
            });
            if message.get("id").and_then(Value::as_i64) == Some(id)
                && message.get("method").is_none()
            {
                return message;
            }
            self.stash(message);
        }
    }

    fn stash(&mut self, message: Value) {
        // A server-initiated request is answered right away, as a real client does. Expert
        // sends `client/registerCapability` before starting its engine and waits for the
        // response; a client that never answers never sees the engine start. The answer is
        // `null` (an empty `workspace/configuration`, an accepted registration, no action
        // picked for `window/showMessageRequest`). It is not kept: nothing inspects it.
        if message.get("method").is_some()
            && let Some(id) = message.get("id").cloned()
        {
            let result = if message["method"] == "workspace/configuration" {
                let n = message["params"]["items"].as_array().map_or(0, Vec::len);
                Value::Array(vec![Value::Null; n])
            } else {
                Value::Null
            };
            self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
            return;
        }
        // Keep notifications and responses to other ids alike. Checking holding requires
        // picking up a "response that arrives later".
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

/// A `file://` URI. This is for tests, so percent-encoding is not handled
/// (assumes temporary directory names are ASCII-only).
/// The `file:` URI of a path. The same conversion as the lsp-det main code (`file:///C:/...` and
/// percent-encoding on Windows). It is matched against the uri a real server returns, so the
/// form must match.
pub fn file_uri(path: &std::path::Path) -> String {
    lsp_det::uri::path_to_uri(path)
}

/// A temporary cargo project. Cross-file queries need a real project with a symbol that is
/// referenced from another file.
pub struct TempCargoProject {
    pub root: PathBuf,
}

impl TempCargoProject {
    /// Creates a 2-file layout in which `b::caller` calls `a::target`.
    pub fn with_cross_file_reference(tag: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("lsp-det-conformance-{tag}-{}", std::process::id()));
        let src = root.join("src");
        std::fs::create_dir_all(&src).expect("cannot create the temporary project");
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
    /// `n` top-level functions sharing the prefix `wsymprobe` (split across 3 files).
    /// The fixture for spec 7.2 item 2 (the cap on the count).
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

/// A temporary Go module. `Caller` in `b.go` calls `fixture.Target`.
pub struct TempGoProject {
    pub root: PathBuf,
}

impl TempGoProject {
    pub fn with_cross_file_reference(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lsp-det-conformance-go-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("cannot create the temporary module");
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

/// A temporary scala-cli project. `B.scala` refers to `A.target` (M9, Metals).
pub struct TempScalaProject {
    pub root: PathBuf,
}

impl TempScalaProject {
    pub fn with_cross_file_reference(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lsp-det-conformance-scala-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("cannot create the temporary project");
        std::fs::write(root.join("project.scala"), SCALA_PROJECT).unwrap();
        std::fs::write(root.join("A.scala"), SCALA_A).unwrap();
        std::fs::write(root.join("B.scala"), SCALA_B_WITH_CALL).unwrap();
        TempScalaProject { root }
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    pub fn with_many_symbols(tag: &str, n: usize) -> Self {
        let project = Self::with_cross_file_reference(tag);
        for file in 0..3 {
            let body: String = std::iter::once(format!("object S{file} {{\n"))
                .chain(
                    (0..n)
                        .filter(|i| i % 3 == file)
                        .map(|i| format!("  def wsymprobe{i:03}(): Int = {i}\n")),
                )
                .chain(std::iter::once("}\n".to_string()))
                .collect();
            std::fs::write(project.root.join(format!("S{file}.scala")), body).unwrap();
        }
        project
    }
}

impl Drop for TempScalaProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A temporary Mix project. `B.x` in `lib/b.ex` calls `A.target` (M10, Expert).
pub struct TempMixProject {
    pub root: PathBuf,
}

impl TempMixProject {
    pub fn with_cross_file_reference(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lsp-det-conformance-mix-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("lib")).expect("cannot create the temporary project");
        std::fs::write(root.join("mix.exs"), MIX_EXS).unwrap();
        std::fs::write(root.join("lib/a.ex"), EX_A).unwrap();
        std::fs::write(root.join("lib/b.ex"), EX_B_WITH_CALL).unwrap();
        TempMixProject { root }
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    pub fn with_many_symbols(tag: &str, n: usize) -> Self {
        let project = Self::with_cross_file_reference(tag);
        for file in 0..3 {
            let body: String = std::iter::once(format!("defmodule S{file} do\n"))
                .chain(
                    (0..n)
                        .filter(|i| i % 3 == file)
                        .map(|i| format!("  def wsymprobe{i:03}, do: {i}\n")),
                )
                .chain(std::iter::once("end\n".to_string()))
                .collect();
            std::fs::write(project.root.join(format!("lib/s{file}.ex")), body).unwrap();
        }
        project
    }
}

impl Drop for TempMixProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A temporary Nextflow pipeline. `main.nf` includes and calls `GREET` from `modules/greet.nf`
/// (M12, Nextflow's language server).
pub struct TempNextflowProject {
    pub root: PathBuf,
}

impl TempNextflowProject {
    /// Only `nextflow.config`: nothing for the server to scan.
    pub fn without_scripts(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lsp-det-conformance-nextflow-{tag}-{}",
            std::process::id()
        ));
        // A leftover of a run that died before its `Drop` would be counted by the scan.
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("modules")).expect("cannot create the temporary project");
        std::fs::write(root.join("nextflow.config"), NF_CONFIG).unwrap();
        TempNextflowProject { root }
    }

    pub fn with_cross_file_reference(tag: &str) -> Self {
        let project = Self::without_scripts(tag);
        std::fs::write(project.root.join("main.nf"), NF_MAIN).unwrap();
        std::fs::write(project.root.join("modules/greet.nf"), NF_GREET).unwrap();
        project
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// `n` more scripts, each including `GREET` and calling it once.
    pub fn with_many_calls(tag: &str, n: usize) -> Self {
        let project = Self::with_cross_file_reference(tag);
        for i in 0..n {
            std::fs::write(
                project.root.join(format!("w_{i:03}.nf")),
                format!("include {{ GREET }} from './modules/greet.nf'\n\nworkflow W{i} {{\n    GREET(channel.of('x'))\n}}\n"),
            )
            .unwrap();
        }
        project
    }
}

impl Drop for TempNextflowProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A temporary cabal library with a `cradle: cabal:` hie.yaml. `B.x` in `src/B.hs` uses
/// `A.target` (M15, haskell-language-server).
pub struct TempCabalProject {
    pub root: PathBuf,
}

impl TempCabalProject {
    pub fn with_cross_file_reference(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lsp-det-conformance-cabal-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).expect("cannot create the temporary project");
        std::fs::write(root.join("fixture.cabal"), HS_CABAL).unwrap();
        std::fs::write(root.join("hie.yaml"), HS_HIE_YAML).unwrap();
        std::fs::write(root.join("src/A.hs"), HS_A).unwrap();
        std::fs::write(root.join("src/B.hs"), HS_B).unwrap();
        TempCabalProject { root }
    }

    /// The cradle names a component that does not exist: HLS cannot load the project.
    pub fn with_broken_cradle(tag: &str) -> Self {
        let project = Self::with_cross_file_reference(tag);
        std::fs::write(project.root.join("hie.yaml"), HS_HIE_YAML_BROKEN).unwrap();
        project
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for TempCabalProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A temporary Python project for pyrefly. `pkg/b.py` calls `target` of `pkg/a.py` (M16).
pub struct TempPyreflyProject {
    pub root: PathBuf,
}

impl TempPyreflyProject {
    pub fn with_cross_file_reference(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lsp-det-conformance-pyrefly-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("pkg")).expect("cannot create the temporary project");
        std::fs::write(root.join("pyproject.toml"), PYREFLY_PYPROJECT).unwrap();
        std::fs::write(root.join("pkg/__init__.py"), "").unwrap();
        std::fs::write(root.join("pkg/a.py"), PYREFLY_A).unwrap();
        std::fs::write(root.join("pkg/b.py"), PYREFLY_B).unwrap();
        TempPyreflyProject { root }
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for TempPyreflyProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Sends SIGKILL (TerminateProcess on Windows) to the descendants of `pid` whose command line
/// contains `needle`. Used to bring down the tsserver (a grandchild process) of a real
/// typescript-language-server. Returns the pids that were killed.
pub fn kill_descendants_matching(pid: u32, needle: &str) -> Vec<u32> {
    let mut killed = Vec::new();
    let mut frontier = vec![pid];
    while let Some(parent) = frontier.pop() {
        for (child, cmdline) in children_of(parent) {
            frontier.push(child);
            // Count as "killed" only when it could be sent (it fails if the process is already
            // gone).
            if cmdline.contains(needle) && force_kill(child) {
                killed.push(child);
            }
        }
    }
    killed
}

/// The direct children of `parent` and their command lines. Empty if the parent is gone.
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

/// Enumerates the direct children with `pgrep -P`. Even if pgrep fails (even if that parent is
/// gone), returns empty and the rest of the search continues.
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

/// A temporary TypeScript project. `caller` in `b.ts` calls `target` in `a.ts`.
pub struct TempTsProject {
    pub root: PathBuf,
}

impl TempTsProject {
    pub fn with_cross_file_reference(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lsp-det-conformance-ts-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("cannot create the temporary project");
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
/// `target` is at the 17th character of line 1 (0-based: line 0, character 16).
pub const TS_A: &str = "export function target(): number {\n  return 1;\n}\n";
/// The call is on line 4 (0-based: line 3). The import on line 1 is also counted as a reference.
pub const TS_B_WITH_CALL: &str =
    "import { target } from './a';\n\nexport function caller(): number {\n  return target();\n}\n";
pub const TS_B_WITHOUT_CALL: &str = "export function caller(): number {\n  return 1;\n}\n";
pub const TS_B_WITH_TWO_CALLS: &str = "import { target } from './a';\n\nexport function caller(): number {\n  target();\n  return target();\n}\n";
pub const TS_C_WITH_CALL: &str =
    "import { target } from './a';\n\nexport function other(): number {\n  return target();\n}\n";

/// A temporary Python project. `caller` in `b.py` calls `target` in `a.py`.
/// pyright turns the `workspaceFolders` of `initialize` into a service instance per folder, and
/// enumerates everything under that folder.
pub struct TempPyProject {
    pub root: PathBuf,
}

impl TempPyProject {
    pub fn with_cross_file_reference(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lsp-det-conformance-py-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).expect("cannot create the temporary project");
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

/// `target` is at the 5th character of line 1 (0-based: line 0, character 4).
pub const PY_A: &str = "def target():\n    return 1\n";
/// The call is on line 5 (0-based: line 4). The import on line 1 is also counted as a reference.
pub const PY_B_WITH_CALL: &str = "from a import target\n\n\ndef caller():\n    return target()\n";
pub const PY_B_WITHOUT_CALL: &str = "def caller():\n    return 1\n";
pub const PY_B_WITH_TWO_CALLS: &str =
    "from a import target\n\n\ndef caller():\n    target()\n    return target()\n";
pub const PY_C_WITH_CALL: &str = "import a\n\n\ndef other():\n    return a.target()\n";

/// `Target` is at the 6th character of line 3 (0-based: line 2, character 5).
pub const GO_A: &str = "package fixture\n\nfunc Target() {}\n";
/// The call is on line 4 (0-based: line 3).
pub const GO_B_WITH_CALL: &str = "package fixture\n\nfunc Caller() {\n\tTarget()\n}\n";
pub const GO_B_WITHOUT_CALL: &str = "package fixture\n\nfunc Caller() {}\n";
pub const GO_B_WITH_TWO_CALLS: &str =
    "package fixture\n\nfunc Caller() {\n\tTarget()\n\tTarget()\n}\n";
pub const GO_C_WITH_CALL: &str = "package fixture\n\nfunc Other() {\n\tTarget()\n}\n";

pub const MIX_EXS: &str = "defmodule Fixture.MixProject do\n  use Mix.Project\n\n  def project do\n    [app: :fixture, version: \"0.1.0\", elixir: \"~> 1.18\", deps: []]\n  end\nend\n";
/// `target` is on line 2 (0-based: line 1, character 6).
pub const EX_A: &str = "defmodule A do\n  def target, do: 1\nend\n";
/// The call is on line 2 (0-based: line 1).
pub const EX_B_WITH_CALL: &str = "defmodule B do\n  def x, do: A.target()\nend\n";
pub const PYREFLY_PYPROJECT: &str = "[project]\nname = \"fixture\"\nversion = \"0.1.0\"\n";
/// `target` is declared on line 0 (character 4).
pub const PYREFLY_A: &str = "def target() -> int:\n    return 1\n";
pub const PYREFLY_B: &str =
    "from pkg.a import target\n\n\ndef x() -> int:\n    return target() + 1\n";
pub const PYREFLY_TARGET_DECLARATION: (u32, u32) = (0, 4);
pub const HS_CABAL: &str = "cabal-version:      2.4\nname:               fixture\nversion:            0.1.0.0\nbuild-type:         Simple\n\nlibrary\n    exposed-modules:  A, B\n    build-depends:    base\n    hs-source-dirs:   src\n    default-language: Haskell2010\n";
pub const HS_HIE_YAML: &str = "cradle:\n  cabal:\n";
pub const HS_HIE_YAML_BROKEN: &str = "cradle:\n  cabal:\n    component: \"lib:doesnotexist\"\n";
/// `target` is declared on line 3 (character 0).
pub const HS_A: &str = "module A (target) where\n\ntarget :: Int\ntarget = 1\n";
pub const HS_B: &str = "module B (x) where\n\nimport A (target)\n\nx :: Int\nx = target + 1\n";
pub const HS_TARGET_DECLARATION: (u32, u32) = (3, 0);
pub const NF_CONFIG: &str = "nextflow.enable.dsl = 2\n";
/// The position of `GREET` in its declaration in `modules/greet.nf` (line, character).
pub const NF_GREET_DECLARATION: (u32, u32) = (0, 8);
pub const NF_GREET: &str = "process GREET {\n    input:\n    val name\n    output:\n    stdout\n    script:\n    \"\"\"\n    echo hello $name\n    \"\"\"\n}\n";
/// `main.nf` includes `GREET` (line 0) and calls it (line 3).
pub const NF_MAIN: &str =
    "include { GREET } from './modules/greet.nf'\n\nworkflow {\n    GREET(channel.of('a'))\n}\n";
pub const NF_MAIN_WITHOUT_CALL: &str =
    "include { GREET } from './modules/greet.nf'\n\nworkflow {\n    channel.of('a')\n}\n";
/// The line of the call in [`NF_MAIN`].
pub const NF_MAIN_CALL_LINE: u64 = 3;
pub const EX_B_WITHOUT_CALL: &str = "defmodule B do\n  def x, do: 1\nend\n";

pub const SCALA_PROJECT: &str = "//> using scala 3.3.4\n";
/// `target` is on line 2 (0-based: line 1, character 6).
pub const SCALA_A: &str = "object A {\n  def target: Int = 1\n}\n";
/// The reference is on line 2 (0-based: line 1).
pub const SCALA_B_WITH_CALL: &str = "object B {\n  val x: Int = A.target\n}\n";
pub const SCALA_B_WITHOUT_CALL: &str = "object B {\n  val x: Int = 1\n}\n";
pub const SCALA_B_WITH_TWO_CALLS: &str =
    "object B {\n  val x: Int = A.target\n  val y: Int = A.target\n}\n";
pub const SCALA_C_WITH_CALL: &str = "object C {\n  val z: Int = A.target\n}\n";

/// `target` is at the 8th character of line 1 (0-based: line 0, character 7).
pub const A_RS: &str = "pub fn target() {}\n";
pub const B_WITH_CALL: &str = "use crate::a::target;\n\npub fn caller() {\n    target();\n}\n";
pub const B_WITHOUT_CALL: &str = "pub fn caller() {}\n";
/// A change on disk (spec 7.3 item 2): adds one call.
pub const B_WITH_TWO_CALLS: &str =
    "use crate::a::target;\n\npub fn caller() {\n    target();\n    target();\n}\n";
/// A new file (spec 7.3 item 2): also calls from another file. In Rust a file does not enter the
/// crate until it is named by `mod`, so lib.rs changes too.
pub const C_RS_WITH_CALL: &str = "pub fn other() {\n    crate::a::target();\n}\n";
pub const LIB_RS_WITH_C: &str = "pub mod a;\npub mod b;\npub mod c;\n";

/// For using `write!`.
pub fn flush<W: Write>(writer: &mut W) {
    let _ = writer.flush();
}

/// Puts the fixture under git (the downstream side's stand-in enumerates with git ls-files).
pub fn git_init(root: &std::path::Path) {
    let status = Command::new("git")
        .args(["init", "-q"])
        .current_dir(root)
        .status()
        .expect("cannot launch git");
    assert!(status.success(), "git init failed: {}", root.display());
}

/// A temporary workspace under git (for testing the downstream side's stand-in).
/// Runs `git init` and places `a.rs`. Does not track it (picked up by `--others`).
pub struct TempGitWorkspace {
    pub root: PathBuf,
}

impl TempGitWorkspace {
    pub fn new(tag: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("lsp-det-stand-in-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("cannot create the temporary workspace");
        git_init(&root);
        std::fs::write(root.join("a.rs"), "pub fn target() {}\n").unwrap();
        TempGitWorkspace { root }
    }

    /// A temporary directory outside git.
    pub fn without_git(tag: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "lsp-det-stand-in-nogit-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("cannot create the temporary directory");
        std::fs::write(root.join("a.rs"), "pub fn target() {}\n").unwrap();
        TempGitWorkspace { root }
    }

    pub fn file(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for TempGitWorkspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
