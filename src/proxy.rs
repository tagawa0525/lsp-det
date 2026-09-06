//! The event loop that relays between the client and the upstream language server
//! (v0.1-design.md 4.6).
//!
//! All state is confined to the single loop in this module, and there are no locks. Reading is
//! done by std threads + `mpsc`, and decisions are made only inside the loop. The upstream side
//! ([`UpstreamSide`]) lives here. The downstream side (M3) rides on the same loop.

use std::io::{self, BufReader, Read, Write};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::adapter;
use crate::documents::OpenDocuments;
use crate::framing::{self, RawMessage};
use crate::gate::{self, Decision, DrainReason, Gate, Release};
use crate::initialize;
use crate::peek::{self, RequestId};
use crate::process;
use crate::state::{self, ServerState, ServerStateProvider};
use crate::tracker::Tracker;
use crate::watched_files::WatchedFiles;

/// The interval for polling whether the upstream is alive. To keep ownership of `Upstream`
/// (the right to kill it) in the one place that is the main loop, no separate thread calls
/// `wait()`; instead `try_wait()` runs on every `recv_timeout` (v0.1-design.md 4.8: timers are
/// `recv_timeout`).
const UPSTREAM_POLL_INTERVAL: Duration = Duration::from_millis(20);

enum Event {
    FromClient(RawMessage),
    ClientClosed,
    ClientReadError(io::Error),
    FromUpstream(RawMessage),
    /// The upstream's stdout closed. Polling for liveness (`try_wait`) runs only when
    /// `recv_timeout` times out, so detecting death is delayed while the client keeps talking
    /// without pause. The reader reports it explicitly.
    UpstreamClosed,
}

/// Relays between the client and the upstream, and returns the proxy's own exit code.
///
/// The mapping is selected by the name the upstream calls itself in
/// `InitializeResult.serverInfo` (v0.1-design.md 4.2). If it is not known, the upstream side
/// declares no guarantees and reports `unknown` on both axes (spec 8.2 item 3). The
/// disappearance of the upstream is conveyed by closing the connection, not by a notification
/// (spec 8.2 item 7). The downstream side ([`Gate`]) looks at the state on the boundary and
/// stands in for cross-workspace requests (design 4.3).
pub fn run<R, W>(client_in: R, client_out: W, command: &str, args: &[String]) -> io::Result<i32>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let mut upstream_side = UpstreamSide::new();
    let mut gate = Gate::new();
    let mut handles = process::spawn(command, args)?;
    let mut upstream_stdin = handles.stdin;
    let mut client_out = client_out;

    let (tx, rx) = mpsc::channel::<Event>();

    spawn_stderr_relay(handles.stderr);
    spawn_client_reader(client_in, tx.clone());
    spawn_upstream_reader(handles.stdout, tx);

    // Write failures are ignored and we continue. Even if the client has stopped reading, or
    // the upstream's stdin is closed, the next poll detects the exit.
    let mut deliver = |outs: Vec<Out>| {
        for out in outs {
            match out {
                Out::ToClient(msg) => {
                    let _ = framing::write_message(&mut client_out, &msg);
                }
                Out::ToUpstream(msg) => {
                    let _ = framing::write_message(&mut upstream_stdin, &msg);
                }
            }
        }
    };

    let exit_code = loop {
        match rx.recv_timeout(UPSTREAM_POLL_INTERVAL) {
            Ok(Event::FromClient(msg)) => deliver(upstream_side.on_client(msg, &mut gate)),
            Ok(Event::FromUpstream(msg)) => deliver(upstream_side.on_upstream(msg, &mut gate)),
            Ok(Event::UpstreamClosed) => {
                // An upstream that closed its stdout no longer answers. Close here even if
                // the client keeps talking.
                deliver(close_pending(&mut upstream_side, &mut gate));
                break reap(&mut handles.upstream);
            }
            Ok(Event::ClientClosed) => {
                eprintln!("lsp-det: client closed connection, terminating upstream");
                let _ = handles.upstream.kill_and_wait();
                break 0;
            }
            Ok(Event::ClientReadError(err)) => {
                eprintln!("lsp-det: error reading from client: {err}");
                let _ = handles.upstream.kill_and_wait();
                break 0;
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Ok(Some(status)) = handles.upstream.try_wait() {
                    let code = status.code().unwrap_or(1);
                    if code != 0 {
                        eprintln!("lsp-det: upstream exited with status {code}");
                    }
                    deliver(close_pending(&mut upstream_side, &mut gate));
                    break code;
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                // Both client_reader and upstream_reader have finished.
                // Check the upstream's final status and exit.
                let code = handles
                    .upstream
                    .wait()
                    .ok()
                    .and_then(|s| s.code())
                    .unwrap_or(1);
                deliver(close_pending(&mut upstream_side, &mut gate));
                break code;
            }
        }
    };

    let _ = client_out.flush();
    Ok(exit_code)
}

/// The output of the relay. Which side to write to.
enum Out {
    ToClient(RawMessage),
    ToUpstream(RawMessage),
}

/// The kind of message that came from the client.
///
/// To detach the borrow of the peek from the ownership of `RawMessage`, only the result of
/// the judgment is extracted as owned data.
enum ClientKind {
    ServerStateRequest(RequestId),
    InitializeRequest(Option<RequestId>),
    ShutdownRequest,
    CancelRequest(RequestId),
    Request {
        id: RequestId,
        method: String,
    },
    /// The client's `initialized` notification. Under the identity mapping, the query for the
    /// initial state is sent after this has been forwarded.
    Initialized,
    /// Any other notification. Carries the change the mapping predicted from the client's
    /// notification, if any (ADR 0014 addendum decision D).
    Notification(Option<ServerState>),
    /// `textDocument/didOpen`. If the uri is already open, carries the message rewritten into
    /// a full-text `didChange` (ADR 0015 decision B).
    /// `textDocument/didOpen`: the message rewritten into a `didChange` when the document is
    /// already open (ADR 0015), and the change the mapping predicted from it, if any.
    DidOpen(Option<RawMessage>, Option<ServerState>),
    Other,
}

/// A server-initiated request that stems from the injected `window.workDoneProgress`.
/// If the client did not declare it itself, the upstream side answers (design 4.2).
const WORK_DONE_PROGRESS_CREATE: &str = "window/workDoneProgress/create";

/// The subscription declaration for this protocol, injected into the `initialize` sent to the
/// upstream (design 4.2). Needed so that the downstream side can read the notifications when
/// the upstream speaks this protocol itself. If the client did not declare it, the
/// notifications that arrive are not forwarded to the client.
const SERVER_STATE_CLIENT_CAPABILITY: &str = "experimental.serverState";

/// The id of the request with which lsp-det itself queries the upstream's initial state under
/// the identity mapping. The upstream's notifications come only on a change, so the first
/// state can only be asked for. JSON-RPC allows string ids too, so there is no guarantee
/// against a collision, but using `lsp-det:` as a reserved prefix avoids one in practice.
const SELF_STATE_REQUEST_ID: &str = "lsp-det:serverState";

/// Takes the exit code, and takes an upstream that lingers down with it.
///
/// There is a slight gap between closing stdout and actually exiting, so wait briefly instead
/// of deciding at once. An upstream that still has not exited after the wait is killed
/// (if the proxy hangs, the client hangs too).
fn reap(upstream: &mut process::Upstream) -> i32 {
    const ATTEMPTS: u32 = 50; // 20ms x 50 = at most 1 second
    for _ in 0..ATTEMPTS {
        match upstream.try_wait() {
            Ok(Some(status)) => return status.code().unwrap_or(1),
            Ok(None) => thread::sleep(UPSTREAM_POLL_INTERVAL),
            Err(_) => break,
        }
    }
    eprintln!("lsp-det: upstream closed stdout but did not exit; killing it");
    let _ = upstream.kill_and_wait();
    1
}

/// The upstream is gone. Answer the unanswered requests with errors, then close the connection
/// (spec 8.2 item 7, design 4.2 "disappearance of the upstream"). If it disappeared without
/// answering `initialize`, that is closed too, as are all the cross-workspace requests the
/// downstream side was holding.
///
/// No notification representing death is sent. The disappearance of the process is not a
/// value of this protocol; the EOF that follows conveys it. Everything must be written out
/// before leaving the loop.
fn close_pending(upstream_side: &mut UpstreamSide, gate: &mut Gate) -> Vec<Out> {
    let mut outs = Vec::new();
    if let Some(response) = upstream_side.fail_pending_initialize() {
        outs.push(Out::ToClient(response));
    }
    outs.extend(
        gate.drain(DrainReason::UpstreamExited)
            .into_iter()
            .map(Out::ToClient),
    );
    outs
}

/// The upstream side: stands in for the language server and provides this protocol
/// (v0.1-design.md 4.2).
///
/// Exists regardless of whether there is an adapter. Without an adapter it reports `unknown`
/// on both axes (spec 8.2 item 3). It also has the role of handing the state on the boundary
/// ([`Self::boundary_state`]) to the downstream side.
struct UpstreamSide {
    tracker: StateTracker,
    /// Whether the client made the declaration of spec 5.2. The condition for sending
    /// notifications.
    client_declared: bool,
    /// Whether the client declared `window.workDoneProgress` itself.
    /// If not, the upstream side answers `window/workDoneProgress/create`.
    client_declared_progress: bool,
    initialize_id: Option<RequestId>,
    /// Whether `InitializeResult` has been forwarded. The mapping is also selected at that
    /// point, so the state never moves before this (LSP limits server-initiated notifications
    /// to after `InitializeResult`).
    handshake_done: bool,
    /// The upstream itself declares this protocol (`InitializeResult` has
    /// `serverStateProvider`). From then on the upstream side becomes the identity mapping: it
    /// adds no declaration, forwards the requests, and emits no notifications of its own. This
    /// is so that two streams from different senders do not flow on the same connection
    /// (spec 8.2 item 6, ADR 0009 decision D-1).
    identity: bool,
    /// The state on the boundary under the identity mapping. Updated from the upstream's
    /// `serverStateChanged` and from the response to lsp-det's own query. Until the first
    /// response arrives, it is the `initializing` of "right after initialize" (the downstream
    /// side waits).
    identity_state: ServerState,
    /// Became the identity mapping, but the query for the initial state has not been sent yet.
    /// It is sent after the client's `initialized` has been forwarded to the upstream (LSP
    /// allows a server to accept no other requests until `initialized`, and rust-analyzer
    /// exits, treating it as a protocol violation).
    identity_query_pending: bool,
    /// The stand-in for `workspace/didChangeWatchedFiles` (ADR 0015 decision A). `Some` only
    /// when the client does not declare the capability and there is a git-managed root.
    /// Reverts to `None` once the client sends the notification itself.
    watched_files: Option<WatchedFiles>,
    /// The open documents (rewriting of duplicate `didOpen`. ADR 0015 decision B).
    documents: OpenDocuments,
}

impl UpstreamSide {
    fn new() -> Self {
        UpstreamSide {
            tracker: StateTracker::new(),
            client_declared: false,
            client_declared_progress: false,
            initialize_id: None,
            handshake_done: false,
            identity: false,
            identity_state: ServerState::initializing(),
            identity_query_pending: false,
            watched_files: None,
            documents: OpenDocuments::new(),
        }
    }

    /// The state on the boundary. The downstream side looks only at this (design 4.1).
    fn boundary_state(&self) -> &ServerState {
        if self.identity {
            &self.identity_state
        } else {
            self.tracker.state()
        }
    }

    fn on_client(&mut self, msg: RawMessage, gate: &mut Gate) -> Vec<Out> {
        let kind = match peek::peek(&msg.body) {
            Ok(view) if view.is_request() => match (view.method(), view.id.clone()) {
                (Some(state::SERVER_STATE_METHOD), Some(id)) => ClientKind::ServerStateRequest(id),
                (Some("initialize"), id) => ClientKind::InitializeRequest(id),
                (Some("shutdown"), _) => ClientKind::ShutdownRequest,
                (Some(method), Some(id)) => ClientKind::Request {
                    id,
                    method: method.to_string(),
                },
                _ => ClientKind::Other,
            },
            Ok(view) if view.is_notification() && view.method() == Some("initialized") => {
                ClientKind::Initialized
            }
            Ok(view) if view.is_notification() && view.method() == Some("$/cancelRequest") => {
                match gate::cancel_target(&msg.body) {
                    Some(id) => ClientKind::CancelRequest(id),
                    None => ClientKind::Other,
                }
            }
            Ok(view) if view.is_notification() => match view.method() {
                Some("textDocument/didOpen") => {
                    let rewritten = self.documents.on_did_open(&msg.body);
                    let predicted = if self.identity {
                        None
                    } else {
                        self.tracker.observe_client(&view, &msg.body)
                    };
                    ClientKind::DidOpen(rewritten, predicted)
                }
                Some("textDocument/didClose") => {
                    self.documents.on_did_close(&msg.body);
                    if self.identity {
                        ClientKind::Other
                    } else {
                        ClientKind::Notification(self.tracker.observe_client(&view, &msg.body))
                    }
                }
                Some("workspace/didChangeWatchedFiles") => {
                    // The client sends it itself. No longer stand in from now on
                    // (ADR 0015 decision A).
                    self.watched_files = None;
                    let predicted = if self.identity {
                        None
                    } else {
                        self.tracker.observe_client(&view, &msg.body)
                    };
                    ClientKind::Notification(predicted)
                }
                _ if !self.identity => {
                    ClientKind::Notification(self.tracker.observe_client(&view, &msg.body))
                }
                _ => ClientKind::Other,
            },
            _ => ClientKind::Other,
        };

        match kind {
            ClientKind::ServerStateRequest(id) => {
                if self.identity {
                    // The upstream speaks this protocol. It is the upstream's job
                    // (spec 8.2 item 6).
                    vec![Out::ToUpstream(msg)]
                } else {
                    // Spec 5.2: this request is answered regardless of the declaration.
                    vec![Out::ToClient(self.state_response(&id))]
                }
            }
            ClientKind::InitializeRequest(id) => {
                self.initialize_id = id;
                self.tracker.remember_initialize(&msg.body);
                if !initialize::client_declares_watched_files(&msg.body) {
                    // Stand in only for a client that does not declare it
                    // (ADR 0015 decision A).
                    self.watched_files = WatchedFiles::new(&initialize::workspace_roots(&msg.body));
                }
                self.client_declared = initialize::client_declares_server_state(&msg.body);
                self.client_declared_progress =
                    initialize::client_declares_work_done_progress(&msg.body);
                gate.set_client_decides(self.client_declared);
                // Who the upstream is is not known yet. Inject all of them: the capabilities
                // for the known mappings, and the subscription declaration for this protocol.
                let mut paths: Vec<&str> = adapter::CLIENT_CAPABILITIES_FOR_ALL_MAPPINGS.to_vec();
                paths.push(SERVER_STATE_CLIENT_CAPABILITY);
                let injected = initialize::inject_client_capabilities(&msg.body, &paths);
                vec![Out::ToUpstream(match injected {
                    Some(body) => RawMessage { body },
                    None => msg,
                })]
            }
            ClientKind::ShutdownRequest => {
                // Answer every held request with an error, then forward the shutdown
                // (spec chapter 9 item 6). Never create a request that gets no response.
                let mut outs: Vec<Out> = gate
                    .drain(DrainReason::Shutdown)
                    .into_iter()
                    .map(Out::ToClient)
                    .collect();
                outs.push(Out::ToUpstream(msg));
                outs
            }
            ClientKind::CancelRequest(id) => match gate.on_cancel(&id) {
                // It was held. Remove it from the queue and answer; do not send it upstream.
                Some(response) => vec![Out::ToClient(response)],
                None => vec![Out::ToUpstream(msg)],
            },
            ClientKind::Request { id, method } => {
                // Before a cross-workspace request, tell the upstream about the changes on
                // disk (stand-in).
                let mut outs = Vec::new();
                if gate::is_cross_workspace(&method)
                    && let Some(watched) = self.watched_files.as_mut()
                    && let Some(notification) = watched.changes_since_last_scan()
                {
                    outs.push(Out::ToUpstream(notification));
                }
                match gate.on_request(msg, id, &method, self.boundary_state()) {
                    Decision::Forward(msg) => outs.push(Out::ToUpstream(msg)),
                    Decision::Held => {}
                    Decision::Reject(response) => outs.push(Out::ToClient(response)),
                }
                outs
            }
            ClientKind::Initialized => {
                let mut outs = vec![Out::ToUpstream(msg)];
                if self.identity_query_pending {
                    self.identity_query_pending = false;
                    outs.push(Out::ToUpstream(self_state_request()));
                }
                outs
            }
            ClientKind::Notification(predicted) => {
                // The notification is forwarded to the upstream first; the predicted change
                // is conveyed after it.
                let mut outs = vec![Out::ToUpstream(msg)];
                if let Some(state) = predicted {
                    outs.extend(releases(gate, &state));
                    if let Some(notification) = self.notify(&state) {
                        outs.push(Out::ToClient(notification));
                    }
                }
                outs
            }
            ClientKind::DidOpen(rewritten, predicted) => {
                let mut outs = vec![Out::ToUpstream(rewritten.unwrap_or(msg))];
                if let Some(state) = predicted {
                    outs.extend(releases(gate, &state));
                    if let Some(notification) = self.notify(&state) {
                        outs.push(Out::ToClient(notification));
                    }
                }
                outs
            }
            ClientKind::Other => vec![Out::ToUpstream(msg)],
        }
    }

    /// Observe a message from the upstream and return the sequence of outputs.
    fn on_upstream(&mut self, msg: RawMessage, gate: &mut Gate) -> Vec<Out> {
        // After the handshake, when this is not the identity mapping, no stand-in for
        // progress is needed either, and the upstream is observed under a known mapping, the
        // peek is needed for that mapping. It cannot be skipped while there is no mapping yet
        // either: what the server calls itself can arrive after the `initialize` response
        // (typescript-language-server's `$/typescriptVersion`. ADR 0011 decision A-3). It can
        // be skipped only after reading what the server calls itself and finding it unknown.
        if self.handshake_done
            && !self.identity
            && self.tracker.upstream_is_unmapped()
            && self.client_declared_progress
        {
            return vec![Out::ToClient(msg)];
        }

        // Peek only once. Upstream messages can be large (diagnostics etc.), so re-parsing
        // for each judgment would double the load on the transparent path.
        let Ok(view) = peek::peek(&msg.body) else {
            return vec![Out::ToClient(msg)];
        };

        if view.is_request()
            && view.method() == Some(WORK_DONE_PROGRESS_CREATE)
            && !self.client_declared_progress
        {
            // A request that stems from the injected declaration. The client cannot handle
            // it, so the upstream side answers with success. The id is returned as the
            // upstream's, unchanged.
            let id = view.id.clone().expect("is_request has an id");
            return vec![Out::ToUpstream(null_response(&id))];
        }

        let is_initialize_response = !self.handshake_done
            && view.method().is_none()
            && view.id.is_some()
            && view.id == self.initialize_id;

        if is_initialize_response {
            return self.on_initialize_result(msg);
        }

        if self.identity {
            let is_self_response = view.method().is_none()
                && matches!(&view.id, Some(RequestId::String(id)) if id == SELF_STATE_REQUEST_ID);
            let is_state_changed =
                view.is_notification() && view.method() == Some(state::SERVER_STATE_CHANGED_METHOD);
            return self.on_upstream_under_identity(msg, is_self_response, is_state_changed, gate);
        }

        let mut outs = Vec::new();
        let changed = self.tracker.observe(&view, &msg.body);
        outs.push(Out::ToClient(msg));
        if let Some(state) = changed {
            if let Some(notification) = self.notify(&state) {
                outs.push(Out::ToClient(notification));
            }
            outs.extend(releases(gate, &state));
        }
        outs
    }

    /// The upstream's `InitializeResult`. Select the mapping, and either add the declaration
    /// or switch to the identity mapping.
    fn on_initialize_result(&mut self, msg: RawMessage) -> Vec<Out> {
        use initialize::InitializeResultAction::*;
        // Select the mapping by the name the upstream calls itself, or, without a
        // `serverInfo`, by what the result declares. Ask that mapping for the guarantees to
        // declare.
        let identity = initialize::server_info(&msg.body)
            .or_else(|| adapter::identity_from_initialize_result(&msg.body));
        self.tracker.select_mapping(identity.as_ref());
        let provider = self.tracker.provider();
        match initialize::declare_server_state_provider(&msg.body, &provider) {
            NotASuccess => {
                // An error response. The handshake is not complete, and the client may retry
                // initialize. This id has been answered, so it is no longer a request left
                // hanging (no double response even if the upstream disappears).
                self.initialize_id = None;
                vec![Out::ToClient(msg)]
            }
            UpstreamDeclares => {
                self.handshake_done = true;
                self.identity = true;
                eprintln!(
                    "lsp-det: the upstream declares serverStateProvider itself; \
                     the upstream side becomes an identity mapping"
                );
                // The upstream's notifications come only on a change. Ask for the initial
                // state ourselves, but after forwarding the client's `initialized`.
                self.identity_query_pending = true;
                vec![Out::ToClient(msg)]
            }
            Unrewritable => {
                // capabilities / experimental is not an object. We would go on acting as the
                // upstream side without being able to declare, so leave the reason instead
                // of proceeding silently.
                self.handshake_done = true;
                eprintln!(
                    "lsp-det: cannot declare serverStateProvider; \
                     the upstream InitializeResult has an unexpected shape"
                );
                vec![Out::ToClient(msg)]
            }
            Declared(body) => {
                self.handshake_done = true;
                vec![Out::ToClient(RawMessage { body })]
            }
        }
    }

    /// Under the identity mapping. The upstream's state is read from the upstream's
    /// notifications and from our own query. Notifications are forwarded if the client
    /// declared, and otherwise only the downstream side reads them (spec 5.2). The response to
    /// our own query is not shown to the client.
    fn on_upstream_under_identity(
        &mut self,
        msg: RawMessage,
        is_self_response: bool,
        is_state_changed: bool,
        gate: &mut Gate,
    ) -> Vec<Out> {
        if is_self_response {
            match parse_state_response(&msg.body) {
                Some(state) => return self.adopt_identity_state(state, gate),
                None => {
                    // An upstream that claims conformance did not answer the initial state.
                    // There is no basis for waiting, so fall to the unobservable state.
                    eprintln!(
                        "lsp-det: the upstream did not answer {}; treating its state as unknown",
                        state::SERVER_STATE_METHOD
                    );
                    return self.adopt_identity_state(ServerState::unobserved(), gate);
                }
            }
        }

        if is_state_changed {
            // Read the state first, and hand over the original message only when it needs
            // to be forwarded (do not duplicate the body).
            let state = parse_state_notification(&msg.body);
            let mut outs = Vec::new();
            if self.client_declared {
                outs.push(Out::ToClient(msg));
            }
            if let Some(state) = state {
                outs.extend(self.adopt_identity_state(state, gate));
            }
            return outs;
        }

        vec![Out::ToClient(msg)]
    }

    /// Update the state on the boundary under the identity mapping and have the downstream
    /// side re-evaluate.
    fn adopt_identity_state(&mut self, state: ServerState, gate: &mut Gate) -> Vec<Out> {
        self.tracker.log_boundary(&state);
        self.identity_state = state.clone();
        releases(gate, &state)
    }

    /// The upstream disappeared before the handshake finished. Close the hanging `initialize`
    /// with an error. Without this the client waits for the response forever.
    /// After the handshake, return nothing (the EOF conveys it).
    fn fail_pending_initialize(&mut self) -> Option<RawMessage> {
        if self.handshake_done {
            return None;
        }
        self.initialize_id.take().map(|id| initialize_failed(&id))
    }

    /// Build the notification of spec 4.2. Not sent to a client that did not declare
    /// (spec 5.2). Not sent under the identity mapping, since the upstream is the sender.
    fn notify(&self, state: &ServerState) -> Option<RawMessage> {
        if !self.client_declared || self.identity {
            return None;
        }
        Some(changed_notification(state))
    }

    fn state_response(&self, id: &RequestId) -> RawMessage {
        // Answered regardless of the declaration (spec 5.2).
        RawMessage {
            body: serde_json::to_vec(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": self.tracker.state(),
            }))
            .expect("ServerState can always be serialized"),
        }
    }
}

/// Turn the held requests the downstream side released or rejected on a state change into
/// the sequence of outputs.
fn releases(gate: &mut Gate, state: &ServerState) -> Vec<Out> {
    gate.on_state(state)
        .into_iter()
        .map(|release| match release {
            Release::Forward(msg) => Out::ToUpstream(msg),
            Release::Reject(response) => Out::ToClient(response),
        })
        .collect()
}

/// The request that queries the upstream's initial state under the identity mapping.
fn self_state_request() -> RawMessage {
    RawMessage {
        body: serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": SELF_STATE_REQUEST_ID,
            "method": state::SERVER_STATE_METHOD,
        }))
        .expect("a fixed structure, so it can always be serialized"),
    }
}

fn parse_state_response(body: &[u8]) -> Option<ServerState> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        result: ServerState,
    }
    serde_json::from_slice::<Envelope>(body)
        .ok()
        .map(|e| e.result)
}

fn parse_state_notification(body: &[u8]) -> Option<ServerState> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        params: ServerState,
    }
    serde_json::from_slice::<Envelope>(body)
        .ok()
        .map(|e| e.params)
}

/// A success response (`result: null`) to an upstream-initiated request.
fn null_response(id: &RequestId) -> RawMessage {
    RawMessage {
        body: serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": null,
        }))
        .expect("a fixed structure, so it can always be serialized"),
    }
}

fn changed_notification(state: &ServerState) -> RawMessage {
    RawMessage {
        body: serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": state::SERVER_STATE_CHANGED_METHOD,
            "params": state,
        }))
        .expect("ServerState can always be serialized"),
    }
}

/// The response when the upstream disappeared without answering `initialize`.
///
/// Staying silent makes the client wait for the response forever. Never create a request that
/// gets no response (design 4.2 "disappearance of the upstream").
fn initialize_failed(id: &RequestId) -> RawMessage {
    RawMessage {
        body: serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": -32603, // JSON-RPC InternalError
                "message": "lsp-det: the upstream language server exited before answering initialize",
            },
        }))
        .expect("a fixed structure, so it can always be serialized"),
    }
}

/// Tracking of the upstream's state, and the record of its transitions.
///
/// Prints the time of each transition and how long the previous state lasted to stderr. Once
/// the gate is in place, "how long it stayed in which state" is the hold time itself, so this
/// is the only record for tracing the cause when a request was made to wait.
struct StateTracker {
    tracker: Tracker,
    started: Instant,
    entered_state: Instant,
}

impl StateTracker {
    fn new() -> Self {
        let now = Instant::now();
        let mut tracker = StateTracker {
            tracker: Tracker::new(),
            started: now,
            entered_state: now,
        };
        // Print the starting state as the first line. Without it the series of durations
        // loses its origin and cannot be used to measure flaps.
        let initial = tracker.tracker.state().clone();
        tracker.log(&initial);
        tracker
    }

    /// The upstream called itself a name. If a mapping could be selected, log the starting
    /// state and return it.
    fn select_mapping(&mut self, info: Option<&initialize::ServerInfo>) -> Option<ServerState> {
        match self.tracker.select_mapping(info) {
            Some(state) => {
                self.log_selected_mapping("is");
                self.log(&state);
                Some(state)
            }
            None if info.is_none() && self.tracker.observes_upstream() => {
                // An upstream that returns no serverInfo (pyright). Keep the mapping selected
                // from its startup log.
                eprintln!(
                    "lsp-det: the upstream InitializeResult has no serverInfo; \
                     keeping the mapping selected from its startup log"
                );
                None
            }
            None => {
                eprintln!(
                    "lsp-det: upstream is {:?}; no known mapping, reporting unknown",
                    info.map(|i| i.name.as_str()).unwrap_or("<unnamed>")
                );
                None
            }
        }
    }

    fn state(&self) -> &ServerState {
        self.tracker.state()
    }

    fn provider(&self) -> ServerStateProvider {
        self.tracker.provider()
    }

    fn upstream_is_unmapped(&self) -> bool {
        self.tracker.upstream_is_unmapped()
    }

    /// Hand the client's `initialize` to the mapping (`initializationOptions`).
    fn remember_initialize(&mut self, body: &[u8]) {
        self.tracker.remember_initialize(body);
    }

    /// Log and return the change the mapping predicted from the client's notification.
    fn observe_client(&mut self, view: &peek::MessageView, body: &[u8]) -> Option<ServerState> {
        let state = self.tracker.observe_client(view, body)?;
        self.log(&state);
        Some(state)
    }

    /// Under the identity mapping, log the state on the boundary read from the upstream.
    fn log_boundary(&mut self, state: &ServerState) {
        self.log(state);
    }

    /// If the state changed, log it and return the new state. When the mapping was selected
    /// by this notification (the upstream called itself a name in its startup log), record
    /// that too.
    fn observe(&mut self, view: &peek::MessageView, body: &[u8]) -> Option<ServerState> {
        let had_mapping = self.tracker.observes_upstream();
        let changed = self.tracker.observe_upstream(view, body);
        if !had_mapping && self.tracker.observes_upstream() {
            self.log_selected_mapping("introduced itself in its startup log as");
            let initial = self.tracker.state().clone();
            self.log(&initial);
        }
        let state = changed?;
        self.log(&state);
        Some(state)
    }

    fn log_selected_mapping(&self, how: &str) {
        let provider = serde_json::to_string(&self.tracker.provider())
            .unwrap_or_else(|_| "<unserializable>".to_string());
        let identity = self.tracker.identity();
        eprintln!(
            "lsp-det: upstream {how} {:?} version {:?}; using its mapping, declaring {provider}",
            identity.map(|i| i.name.as_str()).unwrap_or(""),
            identity
                .and_then(|i| i.version.as_deref())
                .unwrap_or("<none>")
        );
    }

    fn log(&mut self, state: &ServerState) {
        let now = Instant::now();
        let rendered =
            serde_json::to_string(state).unwrap_or_else(|_| "<unserializable>".to_string());
        eprintln!(
            "lsp-det: [{:.3}s] server state -> {rendered} (previous held {:.3}s)",
            now.duration_since(self.started).as_secs_f64(),
            now.duration_since(self.entered_state).as_secs_f64(),
        );
        self.entered_state = now;
    }
}

fn spawn_stderr_relay(stderr: std::process::ChildStderr) {
    thread::spawn(move || {
        let mut reader = stderr;
        let mut stderr_out = io::stderr();
        let _ = io::copy(&mut reader, &mut stderr_out);
    });
}

fn spawn_client_reader<R>(client_in: R, tx: Sender<Event>)
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut reader = BufReader::new(client_in);
        loop {
            match framing::read_message(&mut reader) {
                Ok(Some(msg)) => {
                    if tx.send(Event::FromClient(msg)).is_err() {
                        return;
                    }
                }
                Ok(None) => {
                    let _ = tx.send(Event::ClientClosed);
                    return;
                }
                Err(err) => {
                    let _ = tx.send(Event::ClientReadError(io::Error::other(err)));
                    return;
                }
            }
        }
    });
}

fn spawn_upstream_reader(stdout: std::process::ChildStdout, tx: Sender<Event>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match framing::read_message(&mut reader) {
                Ok(Some(msg)) => {
                    if tx.send(Event::FromUpstream(msg)).is_err() {
                        return;
                    }
                }
                Ok(None) | Err(_) => {
                    let _ = tx.send(Event::UpstreamClosed);
                    return;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::write_message;
    use std::time::Duration;

    /// With `cat` as the upstream, a message makes the round trip client -> proxy -> cat ->
    /// proxy -> client, so "the bytes the proxy wrote to the upstream" can be observed as they
    /// are.
    fn spawn_with_cat() -> (
        io::PipeWriter,
        BufReader<io::PipeReader>,
        thread::JoinHandle<i32>,
    ) {
        let (client_out_reader, client_out_writer) = io::pipe().unwrap();
        let (client_in_reader, client_in_writer) = io::pipe().unwrap();
        let handle =
            thread::spawn(move || run(client_in_reader, client_out_writer, "cat", &[]).unwrap());
        (client_in_writer, BufReader::new(client_out_reader), handle)
    }

    fn send(writer: &mut io::PipeWriter, body: &str) {
        write_message(
            writer,
            &RawMessage {
                body: body.as_bytes().to_vec(),
            },
        )
        .unwrap();
    }

    #[test]
    fn injects_the_capabilities_of_every_known_mapping_before_knowing_the_upstream() {
        // Design 4.2: serverInfo becomes known in the initialize response, so the injection
        // is done unconditionally, for the known mappings, before knowing who the upstream is.
        let (mut client_in, mut client_out, handle) = spawn_with_cat();

        send(
            &mut client_in,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#,
        );

        let forwarded = framing::read_message(&mut client_out).unwrap().unwrap();
        let value: serde_json::Value = serde_json::from_slice(&forwarded.body).unwrap();
        assert_eq!(
            value["params"]["capabilities"]["experimental"]["serverStatusNotification"],
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            value["params"]["capabilities"]["window"]["workDoneProgress"],
            serde_json::Value::Bool(true)
        );

        drop(client_in);
        handle.join().unwrap();
    }

    #[test]
    fn forwards_observed_upstream_messages_byte_for_byte() {
        // Even while the state is tracked, what reaches the client is the original text
        // (design 4.4).
        let (mut client_in, mut client_out, handle) = spawn_with_cat();

        let status = r#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"ok","quiescent":true,"message":null}}"#;
        send(&mut client_in, status);

        let forwarded = framing::read_message(&mut client_out).unwrap().unwrap();
        assert_eq!(forwarded.body, status.as_bytes());

        drop(client_in);
        handle.join().unwrap();
    }

    #[test]
    fn only_rewrites_the_initialize_request() {
        // Even with the same method, a notification is not rewritten.
        let (mut client_in, mut client_out, handle) = spawn_with_cat();

        let notification =
            r#"{"jsonrpc":"2.0","method":"initialize","params":{"capabilities":{}}}"#;
        send(&mut client_in, notification);

        let forwarded = framing::read_message(&mut client_out).unwrap().unwrap();
        assert_eq!(forwarded.body, notification.as_bytes());

        drop(client_in);
        handle.join().unwrap();
    }

    #[test]
    fn round_trips_a_message_through_a_real_upstream_process() {
        // Use `cat` as the upstream: the round trip client -> proxy -> cat(echo) -> proxy ->
        // client verifies that the bytes make the round trip unmodified.
        let (client_out_reader, client_out_writer) = io::pipe().unwrap();
        let (client_in_reader, mut client_in_writer) = io::pipe().unwrap();

        let handle =
            thread::spawn(move || run(client_in_reader, client_out_writer, "cat", &[]).unwrap());

        // initialize is rewritten by the capability injection (design 4.2), so measure with
        // a different request.
        let sent = RawMessage {
            body: br#"{"jsonrpc":"2.0","id":1,"method":"textDocument/hover","params":{}}"#.to_vec(),
        };
        write_message(&mut client_in_writer, &sent).unwrap();

        let mut reader = BufReader::new(client_out_reader);
        let received = framing::read_message(&mut reader).unwrap().unwrap();
        assert_eq!(received.body, sent.body);

        drop(client_in_writer); // client disconnects -> the proxy should exit
        let code = handle.join().unwrap();
        assert_eq!(code, 0);
    }

    #[test]
    fn propagates_upstream_exit_code_to_client() {
        let (client_out_reader, client_out_writer) = io::pipe().unwrap();
        let (client_in_reader, client_in_writer) = io::pipe().unwrap();

        let handle = thread::spawn(move || {
            run(
                client_in_reader,
                client_out_writer,
                "sh",
                &["-c".to_string(), "exit 7".to_string()],
            )
            .unwrap()
        });

        // Wait for the upstream to exit on its own while holding client_in_writer without
        // dropping it.
        let code = handle.join().unwrap();
        assert_eq!(code, 7);
        drop(client_in_writer);

        // client_out is closed when the proxy exits, and the reading side sees EOF.
        let mut buf = Vec::new();
        let mut reader = client_out_reader;
        reader.read_to_end(&mut buf).unwrap();
        assert!(buf.is_empty());
    }

    #[test]
    fn client_disconnect_kills_upstream_and_exits_cleanly() {
        let (client_out_reader, client_out_writer) = io::pipe().unwrap();
        let (client_in_reader, client_in_writer) = io::pipe().unwrap();
        drop(client_out_reader); // the client side does not read (not of interest)

        let handle = thread::spawn(move || {
            run(
                client_in_reader,
                client_out_writer,
                "sleep",
                &["30".to_string()],
            )
            .unwrap()
        });

        drop(client_in_writer); // the client cuts the connection

        // The proxy should kill the upstream (sleep 30) and exit promptly.
        // If we are made to wait 30 seconds, the kill did not take effect.
        let start = std::time::Instant::now();
        let code = handle.join().unwrap();
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "proxy should kill upstream promptly on client disconnect"
        );
        assert_eq!(code, 0);
    }
}
