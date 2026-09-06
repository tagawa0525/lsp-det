//! A script-driven fake LSP server (the test strategy of v0.1-design.md chapter 6).
//!
//! The upstream that lets the conformance test suite run deterministically. The point is that
//! the state changes on the client's instruction, not on time: the moment it receives
//! `$/fake/emitServerStatus`, it sends back `experimental/serverStatus`. A script with sleeps
//! in it would make the tests timing-dependent and flaky in CI.
//!
//! Control methods:
//!
//! - `$/fake/emitServerStatus` (notification): emits the params as is as the params of
//!   `experimental/serverStatus`
//! - `$/fake/emitNotification` (notification): emits `params.params` as the params of the
//!   notification `params.method` (any server-specific vocabulary)
//! - `$/fake/emitProgress` (notification): emits the params as is as the params of
//!   `$/progress` (reproduces gopls's "Setting up workspace" etc.)
//! - `$/fake/report` (request): returns the list of methods received so far and the params
//!   received in `initialize`. Used by the test side to verify what was forwarded upstream
//!
//! Launch flags for reproducing the boundary around the handshake:
//!
//! - `--exit-before-initialize-result`: exits without responding the moment it receives
//!   `initialize` (a crash at startup)
//! - `--declare-server-state-provider`: pretends the upstream itself conforms to this
//!   protocol. Declares `serverStateProvider: {freshness: {fileChanges: ["Changed"]}}` in
//!   `InitializeResult` and answers `experimental/serverState` with its own state. The state
//!   starts from `--initial-readiness <initializing|indexing|ready>` (default `ready`), and
//!   when changed with `$/fake/emitServerStateChanged` (notification; params are
//!   `{health, readiness}`) it sends `experimental/serverStateChanged`
//! - `--declare-server-state-provider-false`: declares `serverStateProvider: false`
//!   (the same "not provided" notation as `hoverProvider: false`)
//! - `--server-name <name>`: the name it calls itself in `InitializeResult.serverInfo.name`.
//!   Default is `fake-lsp-server` (a name with no known mapping). If it calls itself
//!   `rust-analyzer`, lsp-det chooses the rust-analyzer mapping. Passing `none` omits
//!   `serverInfo` itself (pyright does not return it)
//! - `--startup-log <message>`: right after startup (before reading `initialize`), sends
//!   `message` via `window/logMessage` (type 3). Reproduces the pyright family's
//!   "Pyright language server 1.1.412 starting" self-identification
//! - `--execute-commands <a,b>`: the `executeCommandProvider.commands` it declares (Nextflow's
//!   language server calls itself only through them and returns no `serverInfo`)
//! - `--server-version <version>`: the version it calls itself in `serverInfo.version`.
//!   Default is `1.98.0 (fake)` (within the range of versions for which the rust-analyzer
//!   mapping passed the conformance tests). Passing `none` omits version
//! - `--references-depend-on-readiness`: answers `textDocument/references` with 1 item if it
//!   is `ready`, otherwise an empty array (reproduces the empty response of an unfinished
//!   index). Whether it is `ready` is decided by its own state in conforming mode, and by the
//!   last `quiescent` it sent when playing rust-analyzer
//! - `--request-progress-create`: on receiving `initialized`, sends a
//!   `window/workDoneProgress/create` request (id `"wdp-1"`). Whether a response came back is
//!   known from `progressCreateAnswered` in `$/fake/report`
//! - `--startup-typescript-version <version>`: right after responding to `initialize`, sends
//!   the typescript-language-server-specific notification `$/typescriptVersion`
//!   `{version, source: "fake"}` (the same order as the real server)
//! - `$/fake/emitLogMessage` (notification; params are `{type, message}`): sends
//!   `window/logMessage` as is (reproduces pyright's "Starting service instance" /
//!   "Found N source files" etc.)
//! - `--require-initialized-before-requests`: if a request other than `initialize` arrives
//!   before `initialized`, treats it as a protocol violation like rust-analyzer does, prints
//!   an error to stderr, and exits (LSP: a server accepts no other requests until
//!   `initialized`)
//! - `--fail-first-initialize`: responds to the first `initialize` with an error. From the
//!   second time on, as normal
//! - `--exit-after-initialize-error`: used together with `--fail-first-initialize`; exits
//!   right after sending the error response (verifies that an already answered id is not
//!   answered twice)

use std::io::{self, BufReader};

use lsp_det::framing::{self, RawMessage};
use serde_json::{Value, json};

fn main() {
    // Self-identification so that the process lifetime test (tests/process_lifetime.rs) can
    // learn the upstream's pid through lsp-det's stderr relay.
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
    // On receiving `workspace/didChangeWatchedFiles`, start reindexing (quiescent: false) and
    // grow the references result by the number of changes. The end is given from outside via
    // `$/fake/emitServerStatus` (the fake upstream for spec 7.3 item 2).
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
    let execute_commands = flags
        .iter()
        .position(|flag| flag == "--execute-commands")
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
    // Record the params of notifications per method (for verifying the stand-in).
    let mut notifications_seen: std::collections::BTreeMap<String, Vec<Value>> =
        std::collections::BTreeMap::new();
    let mut initialize_params = Value::Null;

    if let Some(message) = &startup_log {
        // A real server identifies itself in its constructor. Sent before reading initialize.
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
            // A response to a request we sent.
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
                if let Some(commands) = &execute_commands {
                    result["capabilities"]["executeCommandProvider"] =
                        json!({"commands": commands.split(',').collect::<Vec<_>>()});
                }
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
            "$/fake/emitNotification" => {
                // Any server-to-client notification: `{method, params}` (reproduces server-specific
                // vocabularies such as Metals's `metals/status`). A missing or non-string method
                // is a mistake on the test side; say so instead of emitting an invalid message.
                match params["method"].as_str() {
                    Some(method) => send(
                        &mut stdout,
                        json!({
                            "jsonrpc": "2.0",
                            "method": method,
                            "params": params.get("params").cloned().unwrap_or(Value::Null)
                        }),
                    ),
                    None => eprintln!(
                        "fake-lsp-server: $/fake/emitNotification needs a string `method`: {params}"
                    ),
                }
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
                    // The on-disk changes taken in by reindexing.
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
                // Unknown requests must be answered too, or the client keeps waiting.
                // Notifications may be left alone.
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
