//! The downstream side: performs the recommended behavior of spec chapter 9 on behalf of a
//! client that does not speak the protocol (v0.1-design.md 4.3).
//!
//! It looks at the `ServerState` on the boundary and holds, forwards, or rejects
//! cross-workspace requests (spec 7.0). There is no time limit on holding (a non-goal in
//! design chapter 2). To never create a request that gets no response, every held request is
//! answered on cancellation, on `shutdown`, and on the disappearance of the upstream
//! (spec chapter 9 item 6).
//!
//! The decision table (design 4.3):
//!
//! | `health` \ `readiness`       | `initializing` / `indexing` | `ready` | `unknown` |
//! | ---------------------------- | --------------------------- | ------- | --------- |
//! | `ok` / `warning` / `unknown` | hold                        | forward | forward   |
//! | `error`                      | error immediately           | same    | same      |
//!
//! Every hold is observable on stderr: one line when a request is held (with the state it
//! waited on) and one line when it leaves the queue (with how long it waited and why). Without
//! a time limit, a mapping that missed a signal would otherwise show up only as the client's
//! timeout (ADR 0018 decision A-1). The lines are written on events; no clock decides anything.

use std::time::Instant;

use crate::framing::RawMessage;
use crate::peek::RequestId;
use crate::state::{Health, Readiness, ServerState};

/// The list of spec 7.0 item 1. What the downstream side waits for `ready` on (holds), and
/// that is the only role of this constant. What `coverage` guarantees (spec 7.0 item 2) is
/// this list minus `workspace/symbol`, and it does not appear in the code (the guarantee is
/// the meaning of the declaration and is not used for the decision. ADR 0013).
pub const CROSS_WORKSPACE_METHODS: &[&str] = &[
    "textDocument/references",
    "textDocument/definition",
    "textDocument/typeDefinition",
    "textDocument/declaration",
    "textDocument/implementation",
    "workspace/symbol",
    "textDocument/prepareCallHierarchy",
    "callHierarchy/incomingCalls",
    "callHierarchy/outgoingCalls",
    "textDocument/rename",
    "textDocument/prepareRename",
];

pub fn is_cross_workspace(method: &str) -> bool {
    CROSS_WORKSPACE_METHODS.contains(&method)
}

/// The `params.id` of `$/cancelRequest`. `None` if it cannot be read.
pub fn cancel_target(body: &[u8]) -> Option<RequestId> {
    #[derive(serde::Deserialize)]
    struct Params {
        id: RequestId,
    }
    #[derive(serde::Deserialize)]
    struct Envelope {
        params: Params,
    }
    serde_json::from_slice::<Envelope>(body)
        .ok()
        .map(|e| e.params.id)
}

/// JSON-RPC / LSP error codes.
pub mod error_code {
    /// JSON-RPC `InternalError`. The upstream is gone and cannot answer.
    pub const INTERNAL_ERROR: i64 = -32603;
    /// LSP `RequestCancelled`. The client sent `$/cancelRequest`.
    pub const REQUEST_CANCELLED: i64 = -32800;
    /// LSP `RequestFailed` (3.17). Syntactically correct, but `health` is `error` and the
    /// result cannot be trusted, or `shutdown` made it impossible to keep waiting.
    pub const REQUEST_FAILED: i64 = -32803;
}

/// How to handle the client's request.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Forward to the upstream.
    Forward(RawMessage),
    /// Held until `ready`.
    Held,
    /// Return an error response to the client. Not forwarded to the upstream.
    Reject(RawMessage),
}

/// What to do with the held requests on a state change.
#[derive(Debug, PartialEq, Eq)]
pub enum Release {
    /// Forward to the upstream.
    Forward(RawMessage),
    /// Return an error response to the client.
    Reject(RawMessage),
}

/// The reason for discarding the held requests. The error code and the wording change with
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainReason {
    /// The client requested `shutdown`.
    Shutdown,
    /// The upstream process is gone.
    UpstreamExited,
}

/// A request waiting for `ready`.
struct Held {
    id: RequestId,
    method: String,
    msg: RawMessage,
    since: Instant,
}

pub struct Gate {
    held: Vec<Held>,
    /// The client made the declaration of spec 5.2. It decides for itself, so do not stand in.
    client_decides: bool,
    /// When the gate was created. Only for the timestamps in the log lines.
    started: Instant,
}

impl Default for Gate {
    fn default() -> Self {
        Self {
            held: Vec::new(),
            client_decides: false,
            started: Instant::now(),
        }
    }
}

impl Gate {
    pub fn new() -> Self {
        Self::default()
    }

    fn elapsed(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Called when the client declared `experimental.serverState`. From then on nothing is
    /// held (spec 5.2).
    pub fn set_client_decides(&mut self, decides: bool) {
        self.client_decides = decides;
    }

    /// Decide on the client's request. `method` is the one already peeked.
    pub fn on_request(
        &mut self,
        msg: RawMessage,
        id: RequestId,
        method: &str,
        state: &ServerState,
    ) -> Decision {
        if self.client_decides || !is_cross_workspace(method) {
            return Decision::Forward(msg);
        }
        match verdict(state) {
            Verdict::Forward => Decision::Forward(msg),
            Verdict::Hold => {
                self.held.push(Held {
                    id: id.clone(),
                    method: method.to_string(),
                    msg,
                    since: Instant::now(),
                });
                eprintln!(
                    "lsp-det: [{:.3}s] holding {method} (id {id}) while {}; {} held",
                    self.elapsed(),
                    render(state),
                    self.held.len()
                );
                Decision::Held
            }
            Verdict::Reject => {
                eprintln!(
                    "lsp-det: [{:.3}s] rejected {method} (id {id}): health is error ({})",
                    self.elapsed(),
                    render(state)
                );
                Decision::Reject(request_failed(&id, state))
            }
        }
    }

    /// The state on the boundary changed. Re-evaluate the held requests.
    pub fn on_state(&mut self, state: &ServerState) -> Vec<Release> {
        let started = self.started;
        match verdict(state) {
            Verdict::Hold => Vec::new(),
            Verdict::Forward => {
                let reason = if state.readiness == Readiness::Ready {
                    "ready"
                } else {
                    "readiness is unknown"
                };
                self.held
                    .drain(..)
                    .map(|held| {
                        log_end(started, "released", &held, reason);
                        Release::Forward(held.msg)
                    })
                    .collect()
            }
            Verdict::Reject => self
                .held
                .drain(..)
                .map(|held| {
                    log_end(started, "rejected", &held, "health is error");
                    Release::Reject(request_failed(&held.id, state))
                })
                .collect(),
        }
    }

    /// `$/cancelRequest`. If held, remove it and return `RequestCancelled`.
    /// If not held, `None` (passed through to the upstream).
    pub fn on_cancel(&mut self, id: &RequestId) -> Option<RawMessage> {
        let index = self.held.iter().position(|held| held.id == *id)?;
        let held = self.held.remove(index);
        log_end(
            self.started,
            "cancelled",
            &held,
            "the client sent $/cancelRequest",
        );
        Some(error_response(
            id,
            error_code::REQUEST_CANCELLED,
            "lsp-det: the request was cancelled while waiting for the server to become ready",
        ))
    }

    /// Build an error response for every held request, and empty the queue.
    pub fn drain(&mut self, reason: DrainReason) -> Vec<RawMessage> {
        let (code, message) = match reason {
            DrainReason::Shutdown => (
                error_code::REQUEST_FAILED,
                "lsp-det: shutdown was requested while waiting for the server to become ready",
            ),
            DrainReason::UpstreamExited => (
                error_code::INTERNAL_ERROR,
                "lsp-det: the upstream language server exited while the request was waiting",
            ),
        };
        let started = self.started;
        let why = match reason {
            DrainReason::Shutdown => "shutdown was requested",
            DrainReason::UpstreamExited => "the upstream exited",
        };
        self.held
            .drain(..)
            .map(|held| {
                log_end(started, "answered with an error", &held, why);
                error_response(&held.id, code, message)
            })
            .collect()
    }

    pub fn held_count(&self) -> usize {
        self.held.len()
    }
}

/// One line for a request leaving the queue: how long it waited and why it left.
fn log_end(started: Instant, what: &str, held: &Held, reason: &str) {
    eprintln!(
        "lsp-det: [{:.3}s] {what} {} (id {}) after {:.3}s: {reason}",
        started.elapsed().as_secs_f64(),
        held.method,
        held.id,
        held.since.elapsed().as_secs_f64()
    );
}

/// The state as it appears in the log lines (the same JSON as the state transition log).
fn render(state: &ServerState) -> String {
    serde_json::to_string(state).expect("ServerState can always be serialized")
}

/// A row of the decision table. `health` is looked at first (the recommended interpretation
/// of spec chapter 3).
enum Verdict {
    Forward,
    Hold,
    Reject,
}

fn verdict(state: &ServerState) -> Verdict {
    if state.health == Health::Error {
        return Verdict::Reject;
    }
    match state.readiness {
        Readiness::Ready | Readiness::Unknown => Verdict::Forward,
        Readiness::Initializing | Readiness::Indexing => Verdict::Hold,
    }
}

/// The response when `health` is `error`. Attaches the `message` of `ServerState`
/// (design 4.3). Do not hide a broken server.
fn request_failed(id: &RequestId, state: &ServerState) -> RawMessage {
    let message = match &state.message {
        Some(detail) => format!("lsp-det: the language server reports health: error ({detail})"),
        None => "lsp-det: the language server reports health: error".to_string(),
    };
    error_response(id, error_code::REQUEST_FAILED, &message)
}

fn error_response(id: &RequestId, code: i64, message: &str) -> RawMessage {
    RawMessage {
        body: serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message},
        }))
        .expect("a fixed structure, so it can always be serialized"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn state(health: Health, readiness: Readiness) -> ServerState {
        ServerState {
            health,
            readiness,
            message: None,
        }
    }

    fn request(id: i64, method: &str) -> (RawMessage, RequestId, String) {
        let body = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{{}}}}"#);
        (
            RawMessage {
                body: body.into_bytes(),
            },
            RequestId::Number(id),
            method.to_string(),
        )
    }

    fn decide(gate: &mut Gate, id: i64, method: &str, s: &ServerState) -> Decision {
        let (msg, rid, m) = request(id, method);
        gate.on_request(msg, rid, &m, s)
    }

    fn json(msg: &RawMessage) -> Value {
        serde_json::from_slice(&msg.body).unwrap()
    }

    fn indexing() -> ServerState {
        state(Health::Ok, Readiness::Indexing)
    }

    // --- Decision table ------------------------------------------------------------------

    #[test]
    fn holds_a_cross_workspace_request_while_indexing() {
        let mut gate = Gate::new();
        assert_eq!(
            decide(&mut gate, 1, "textDocument/references", &indexing()),
            Decision::Held
        );
        assert_eq!(gate.held_count(), 1);
    }

    #[test]
    fn holds_while_initializing_too() {
        let mut gate = Gate::new();
        let s = state(Health::Unknown, Readiness::Initializing);
        assert_eq!(decide(&mut gate, 1, "workspace/symbol", &s), Decision::Held);
    }

    #[test]
    fn forwards_when_ready() {
        let mut gate = Gate::new();
        let s = state(Health::Ok, Readiness::Ready);
        let (msg, id, m) = request(1, "textDocument/references");
        assert_eq!(
            gate.on_request(msg.clone(), id, &m, &s),
            Decision::Forward(msg)
        );
    }

    #[test]
    fn forwards_when_readiness_is_unknown() {
        // Spec chapter 9 item 3: there is no signal to wait for.
        let mut gate = Gate::new();
        let s = state(Health::Unknown, Readiness::Unknown);
        assert!(matches!(
            decide(&mut gate, 1, "textDocument/references", &s),
            Decision::Forward(_)
        ));
    }

    #[test]
    fn treats_warning_like_ok() {
        // Spec chapter 9 item 5: waiting does not improve it.
        let mut gate = Gate::new();
        assert_eq!(
            decide(
                &mut gate,
                1,
                "textDocument/references",
                &state(Health::Warning, Readiness::Indexing)
            ),
            Decision::Held
        );
        assert!(matches!(
            decide(
                &mut gate,
                2,
                "textDocument/references",
                &state(Health::Warning, Readiness::Ready)
            ),
            Decision::Forward(_)
        ));
    }

    #[test]
    fn rejects_immediately_when_health_is_error() {
        // Spec chapter 9 item 2. Regardless of readiness.
        for readiness in [Readiness::Indexing, Readiness::Ready, Readiness::Unknown] {
            let mut gate = Gate::new();
            let s = ServerState {
                health: Health::Error,
                readiness,
                message: Some("Failed to load workspaces.".to_string()),
            };
            match decide(&mut gate, 7, "textDocument/references", &s) {
                Decision::Reject(response) => {
                    let v = json(&response);
                    assert_eq!(v["id"], 7);
                    assert_eq!(v["error"]["code"], error_code::REQUEST_FAILED);
                    assert!(
                        v["error"]["message"]
                            .as_str()
                            .unwrap()
                            .contains("Failed to load workspaces."),
                        "the message is attached: {v}"
                    );
                }
                other => panic!("error should reject immediately: {other:?}"),
            }
            assert_eq!(gate.held_count(), 0);
        }
    }

    #[test]
    fn forwards_non_cross_workspace_requests_regardless_of_state() {
        // Spec chapter 9 item 4.
        let mut gate = Gate::new();
        for (method, s) in [
            ("textDocument/hover", indexing()),
            (
                "textDocument/completion",
                state(Health::Unknown, Readiness::Initializing),
            ),
            (
                "textDocument/documentSymbol",
                state(Health::Error, Readiness::Ready),
            ),
            ("initialize", indexing()),
            ("shutdown", indexing()),
        ] {
            assert!(
                matches!(decide(&mut gate, 1, method, &s), Decision::Forward(_)),
                "{method} is not made to wait"
            );
        }
    }

    #[test]
    fn forwards_everything_when_the_client_decides() {
        // Spec 5.2.
        let mut gate = Gate::new();
        gate.set_client_decides(true);
        assert!(matches!(
            decide(&mut gate, 1, "textDocument/references", &indexing()),
            Decision::Forward(_)
        ));
        assert!(matches!(
            decide(
                &mut gate,
                2,
                "textDocument/references",
                &state(Health::Error, Readiness::Ready)
            ),
            Decision::Forward(_)
        ));
    }

    #[test]
    fn lists_exactly_the_methods_of_spec_7_0() {
        assert_eq!(CROSS_WORKSPACE_METHODS.len(), 11);
        assert!(is_cross_workspace("textDocument/prepareRename"));
        assert!(!is_cross_workspace("textDocument/hover"));
    }

    // --- State changes -------------------------------------------------------------------

    #[test]
    fn releases_held_requests_in_order_when_ready() {
        let mut gate = Gate::new();
        decide(&mut gate, 1, "textDocument/references", &indexing());
        decide(&mut gate, 2, "workspace/symbol", &indexing());
        let released = gate.on_state(&state(Health::Ok, Readiness::Ready));
        let ids: Vec<Value> = released
            .iter()
            .map(|r| match r {
                Release::Forward(msg) => json(msg)["id"].clone(),
                Release::Reject(_) => panic!("ready should forward"),
            })
            .collect();
        assert_eq!(ids, vec![Value::from(1), Value::from(2)]);
        assert_eq!(gate.held_count(), 0);
    }

    #[test]
    fn keeps_holding_while_still_indexing() {
        let mut gate = Gate::new();
        decide(&mut gate, 1, "textDocument/references", &indexing());
        assert!(gate.on_state(&indexing()).is_empty());
        assert!(
            gate.on_state(&state(Health::Warning, Readiness::Initializing))
                .is_empty()
        );
        assert_eq!(gate.held_count(), 1);
    }

    #[test]
    fn rejects_held_requests_when_health_turns_error() {
        let mut gate = Gate::new();
        decide(&mut gate, 1, "textDocument/references", &indexing());
        let released = gate.on_state(&state(Health::Error, Readiness::Indexing));
        assert_eq!(released.len(), 1);
        match &released[0] {
            Release::Reject(response) => {
                assert_eq!(json(response)["error"]["code"], error_code::REQUEST_FAILED)
            }
            other => panic!("error should reject: {other:?}"),
        }
        assert_eq!(gate.held_count(), 0);
    }

    #[test]
    fn releases_when_readiness_becomes_unknown() {
        // If the signal is gone, there is no reason to wait either.
        let mut gate = Gate::new();
        decide(&mut gate, 1, "textDocument/references", &indexing());
        let released = gate.on_state(&state(Health::Unknown, Readiness::Unknown));
        assert!(matches!(released.as_slice(), [Release::Forward(_)]));
    }

    // --- cancel / drain -----------------------------------------------------

    #[test]
    fn cancel_removes_a_held_request_and_answers_request_cancelled() {
        let mut gate = Gate::new();
        decide(&mut gate, 1, "textDocument/references", &indexing());
        let response = gate
            .on_cancel(&RequestId::Number(1))
            .expect("answers if held");
        let v = json(&response);
        assert_eq!(v["id"], 1);
        assert_eq!(v["error"]["code"], error_code::REQUEST_CANCELLED);
        assert_eq!(gate.held_count(), 0);
        // Not forwarded even when it becomes ready.
        assert!(
            gate.on_state(&state(Health::Ok, Readiness::Ready))
                .is_empty()
        );
    }

    #[test]
    fn cancel_of_an_unheld_request_is_passed_through() {
        let mut gate = Gate::new();
        assert!(gate.on_cancel(&RequestId::Number(99)).is_none());
    }

    #[test]
    fn drain_answers_every_held_request() {
        for (reason, code) in [
            (DrainReason::Shutdown, error_code::REQUEST_FAILED),
            (DrainReason::UpstreamExited, error_code::INTERNAL_ERROR),
        ] {
            let mut gate = Gate::new();
            decide(&mut gate, 1, "textDocument/references", &indexing());
            decide(&mut gate, 2, "textDocument/rename", &indexing());
            let responses = gate.drain(reason);
            assert_eq!(responses.len(), 2, "{reason:?}");
            for response in &responses {
                assert_eq!(json(response)["error"]["code"], code, "{reason:?}");
            }
            assert_eq!(gate.held_count(), 0);
            assert!(gate.drain(reason).is_empty());
        }
    }
}
