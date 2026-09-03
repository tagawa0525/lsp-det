//! 下流側: プロトコルを話さないクライアントに代わって、仕様 9 章の推奨挙動を
//! 実行する (v0.1-design.md 4.3)。
//!
//! 境界の上の `ServerState` を見て、ワークスペース横断リクエスト (仕様 7.0)
//! を保留・転送・拒否する。保留に時間の上限はない (設計 2 章の非目標)。
//! 応答を返さないリクエストを作らないため、キャンセル・`shutdown`・上流の
//! 消失では保留分すべてに応答する (仕様 9 章 6 項)。
//!
//! 判定表 (設計 4.3):
//!
//! | `health` \ `readiness`       | `initializing` / `indexing` | `ready` | `unknown` |
//! | ---------------------------- | --------------------------- | ------- | --------- |
//! | `ok` / `warning` / `unknown` | 保留                        | 転送    | 転送      |
//! | `error`                      | 即座にエラー                | 同左    | 同左      |

use crate::framing::RawMessage;
use crate::peek::RequestId;
use crate::state::{Health, Readiness, ServerState};

/// 仕様 7.0 のワークスペース横断メソッドの一覧。下流側が `ready` を待つ
/// 対象であり、`workspace/symbol` を除いて `coverage` の保証対象でもある
/// (ADR 0013)。
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

/// `$/cancelRequest` の `params.id`。読めなければ `None`。
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

/// JSON-RPC / LSP のエラーコード。
pub mod error_code {
    /// JSON-RPC `InternalError`。上流が消えて答えられない。
    pub const INTERNAL_ERROR: i64 = -32603;
    /// LSP `RequestCancelled`。クライアントが `$/cancelRequest` した。
    pub const REQUEST_CANCELLED: i64 = -32800;
    /// LSP `RequestFailed` (3.17)。構文的には正しいが、`health` が `error` で
    /// 結果を信頼できない、または `shutdown` で待てなくなった。
    pub const REQUEST_FAILED: i64 = -32803;
}

/// クライアントのリクエストをどう扱うか。
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// 上流へ流す。
    Forward(RawMessage),
    /// `ready` まで保留した。
    Held,
    /// クライアントにエラー応答を返す。上流へは流さない。
    Reject(RawMessage),
}

/// 状態変化で保留分をどうするか。
#[derive(Debug, PartialEq, Eq)]
pub enum Release {
    /// 上流へ流す。
    Forward(RawMessage),
    /// クライアントにエラー応答を返す。
    Reject(RawMessage),
}

/// 保留分を捨てる理由。エラーコードと文言が変わる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainReason {
    /// クライアントが `shutdown` を要求した。
    Shutdown,
    /// 上流プロセスが消えた。
    UpstreamExited,
}

#[derive(Default)]
pub struct Gate {
    held: Vec<(RequestId, RawMessage)>,
    /// クライアントが仕様 5.2 の宣言をした。自分で判断するので代行しない。
    client_decides: bool,
}

impl Gate {
    pub fn new() -> Self {
        Self::default()
    }

    /// クライアントが `experimental.serverState` を宣言したら呼ぶ。以後は
    /// 何も保留しない (仕様 5.2)。
    pub fn set_client_decides(&mut self, decides: bool) {
        self.client_decides = decides;
    }

    /// クライアントのリクエストを判定する。`method` は覗き見済みのもの。
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
                self.held.push((id, msg));
                Decision::Held
            }
            Verdict::Reject => Decision::Reject(request_failed(&id, state)),
        }
    }

    /// 境界の上の状態が変わった。保留分を再評価する。
    pub fn on_state(&mut self, state: &ServerState) -> Vec<Release> {
        match verdict(state) {
            Verdict::Hold => Vec::new(),
            Verdict::Forward => self
                .held
                .drain(..)
                .map(|(_, msg)| Release::Forward(msg))
                .collect(),
            Verdict::Reject => self
                .held
                .drain(..)
                .map(|(id, _)| Release::Reject(request_failed(&id, state)))
                .collect(),
        }
    }

    /// `$/cancelRequest`。保留中なら除去して `RequestCancelled` を返す。
    /// 保留していなければ `None` (上流へ素通しする)。
    pub fn on_cancel(&mut self, id: &RequestId) -> Option<RawMessage> {
        let index = self.held.iter().position(|(held, _)| held == id)?;
        self.held.remove(index);
        Some(error_response(
            id,
            error_code::REQUEST_CANCELLED,
            "lsp-det: the request was cancelled while waiting for the server to become ready",
        ))
    }

    /// 保留分すべてにエラー応答を作り、空にする。
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
        self.held
            .drain(..)
            .map(|(id, _)| error_response(&id, code, message))
            .collect()
    }

    pub fn held_count(&self) -> usize {
        self.held.len()
    }
}

/// 判定表の行。`health` を先に見る (仕様 3 章の推奨解釈)。
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

/// `health` が `error` のときの応答。`ServerState` の `message` を添える
/// (設計 4.3)。壊れたサーバーを隠さない。
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
        .expect("固定の構造なので常にシリアライズできる"),
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

    // --- 判定表 -------------------------------------------------------------

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
        // 仕様 9 章 3 項: 待つべき信号がない。
        let mut gate = Gate::new();
        let s = state(Health::Unknown, Readiness::Unknown);
        assert!(matches!(
            decide(&mut gate, 1, "textDocument/references", &s),
            Decision::Forward(_)
        ));
    }

    #[test]
    fn treats_warning_like_ok() {
        // 仕様 9 章 5 項: 待っても改善しない。
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
        // 仕様 9 章 2 項。readiness によらない。
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
                        "message を添える: {v}"
                    );
                }
                other => panic!("error なら即座に拒否するはず: {other:?}"),
            }
            assert_eq!(gate.held_count(), 0);
        }
    }

    #[test]
    fn forwards_non_cross_workspace_requests_regardless_of_state() {
        // 仕様 9 章 4 項。
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
                "{method} は待たない"
            );
        }
    }

    #[test]
    fn forwards_everything_when_the_client_decides() {
        // 仕様 5.2。
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

    // --- 状態変化 -----------------------------------------------------------

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
                Release::Reject(_) => panic!("ready なら転送する"),
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
            other => panic!("error なら拒否するはず: {other:?}"),
        }
        assert_eq!(gate.held_count(), 0);
    }

    #[test]
    fn releases_when_readiness_becomes_unknown() {
        // 信号がなくなったなら待つ理由もない。
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
            .expect("保留中なら応答する");
        let v = json(&response);
        assert_eq!(v["id"], 1);
        assert_eq!(v["error"]["code"], error_code::REQUEST_CANCELLED);
        assert_eq!(gate.held_count(), 0);
        // ready になっても流さない。
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
