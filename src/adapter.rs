//! ready 判定アダプタ (v0.1-design.md 5 章)。M2 では rust-analyzer のみ。
//!
//! アダプタの役割は**上流メッセージの解釈**だけである。状態の保持・重複
//! 抑止・`dead` は [`crate::tracker::Tracker`] が持つ。分けるのは、アダプタが
//! なくてもプロセスの消失は観測でき、`dead` を出せるからである (ADR 0008)。
//!
//! rust-analyzer は `experimental/serverStatus` 通知で
//! `{health, quiescent, message}` を送る (`lsp/ext.rs`)。`quiescent` の実体は
//! `is_fully_ready()` = ワークスペースロード完了かつキャッシュプライミング
//! 非実行である。
//!
//! `false` に戻るのはワークスペース構成が変わったとき (`Cargo.toml`、
//! ブランチ切り替え等) だけで、**通常のソース編集では戻らない**。実測と
//! その構造的な裏付けは ADR 0007 と
//! docs/research/rust-analyzer-quiescent-measurement.md にある。したがって
//! フラップ対策 (平滑化・デバウンス) は不要である。
//!
//! 失敗は `health` で来る。ワークスペースのロード失敗は
//! `{health: error, quiescent: true}` (`current_status()`)。仕様 6 章 5 項の
//! とおり `readiness` ではなく `health` に写す。
//!
//! gopls アダプタは M3。共通の trait はそのとき 2 つ目の実装を見てから
//! 導入する (現在の要件に対する最小限の実装)。

use serde::Deserialize;

use crate::peek::MessageView;
use crate::state::{Health, Readiness, ServerState, ServerStateProvider};

/// rust-analyzer が送る readiness 通知のメソッド名。
pub const SERVER_STATUS_METHOD: &str = "experimental/serverStatus";

/// `experimental/serverStatus` の params。
///
/// `health` を `state::Health` ではなく専用の enum で受けるのは、仕様 6.1 が
/// 「サーバーは `dead` / `unknown` を送出してはならない」と定めているため。
/// 上流がそれらを送ってきてもパースに失敗し、状態は変わらない。
#[derive(Debug, Deserialize)]
struct ServerStatusParams {
    health: UpstreamHealth,
    quiescent: bool,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum UpstreamHealth {
    Ok,
    Warning,
    Error,
}

impl From<UpstreamHealth> for Health {
    fn from(value: UpstreamHealth) -> Self {
        match value {
            UpstreamHealth::Ok => Health::Ok,
            UpstreamHealth::Warning => Health::Warning,
            UpstreamHealth::Error => Health::Error,
        }
    }
}

#[derive(Default)]
pub struct RustAnalyzerAdapter {
    /// パース不能な status を一度ログしたか (連投を避けるため)。
    warned_unparseable: bool,
}

impl RustAnalyzerAdapter {
    /// 上流への `initialize` に注入する client capability (v0.1-design.md 4.5)。
    /// 未宣言だと rust-analyzer は `experimental/serverStatus` を一切送らない。
    pub const REQUIRED_CLIENT_CAPABILITIES: &'static [&'static str] =
        &["experimental.serverStatusNotification"];

    pub fn new() -> Self {
        Self::default()
    }

    /// 上流に接続した直後の状態。rust-analyzer は `initialize` 応答後に
    /// 最初の `serverStatus` を送るまで何も報告しない。
    pub fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// 上流への `initialize` に注入する client capability (v0.1-design.md 4.5)。
    pub fn required_client_capabilities(&self) -> &'static [&'static str] {
        Self::REQUIRED_CLIENT_CAPABILITIES
    }

    /// `InitializeResult` に宣言する保証グレード (仕様 5 章)。
    ///
    /// rust-analyzer は両方の保証を満たす。準拠テストスイートの仕様 7.2
    /// (完全性) と 7.3 (クロスファイル鮮度) を実 rust-analyzer に当てて
    /// 確認済み (tests/conformance.rs の #[ignore] 付き 2 件)。守れない
    /// 保証を宣言することは仕様 5.1 違反なので、この宣言を変えるときは
    /// 対応するテストの結果を根拠にすること。
    pub fn guarantees(&self) -> ServerStateProvider {
        ServerStateProvider::complete_and_fresh()
    }

    /// 上流→クライアント方向のメッセージから、上流が報告している状態を
    /// 読み取る。`experimental/serverStatus` 以外、および読めない status は
    /// `None` (状態を動かさない)。
    pub fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() || view.method() != Some(SERVER_STATUS_METHOD) {
            return None;
        }

        let Some(params) = parse_status_params(body) else {
            // 未知の形の status は状態を動かさない。壊れた 1 通で
            // readiness を誤って進めるより、前の状態を保つ方が安全。
            //
            // ただし黙って捨ててはならない。上流が params の形を変えると
            // 全通が読めなくなり、状態が最後の値で凍りつく。ゲート実装後は
            // そのまま非常口タイムアウトまでの保留として現れるため、
            // 理由がログにないと診断できなくなる。連投を避けて 1 度だけ出す。
            if !self.warned_unparseable {
                self.warned_unparseable = true;
                eprintln!(
                    "lsp-det: cannot parse {SERVER_STATUS_METHOD} params; \
                     keeping the previous state (further occurrences are not logged)"
                );
            }
            return None;
        };

        Some(ServerState {
            health: params.health.into(),
            readiness: if params.quiescent {
                Readiness::Ready
            } else {
                Readiness::Indexing
            },
            message: params.message,
        })
    }
}

/// `params` を取り出して `ServerStatusParams` として読む。
/// `params` の欠落・型違い・未知の `health` 値はすべて `None`。
fn parse_status_params(body: &[u8]) -> Option<ServerStatusParams> {
    #[derive(Deserialize)]
    struct Envelope {
        params: ServerStatusParams,
    }

    serde_json::from_slice::<Envelope>(body)
        .ok()
        .map(|envelope| envelope.params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;

    fn interpret(adapter: &mut RustAnalyzerAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    fn status(health: &str, quiescent: bool) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{{"health":"{health}","quiescent":{quiescent},"message":null}}}}"#
        )
    }

    #[test]
    fn a_non_quiescent_status_means_indexing() {
        let mut adapter = RustAnalyzerAdapter::new();
        let state = interpret(&mut adapter, &status("ok", false)).expect("status is readable");
        assert_eq!(state.readiness, Readiness::Indexing);
    }

    #[test]
    fn a_quiescent_status_means_ready() {
        let mut adapter = RustAnalyzerAdapter::new();
        let state = interpret(&mut adapter, &status("ok", true)).expect("status is readable");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn carries_health_through_unchanged() {
        // 失敗は health で来る (仕様 6 章 5 項)。error でも quiescent は
        // 独立に読む。
        for (upstream, expected) in [
            ("ok", Health::Ok),
            ("warning", Health::Warning),
            ("error", Health::Error),
        ] {
            let mut adapter = RustAnalyzerAdapter::new();
            let state = interpret(&mut adapter, &status(upstream, true)).unwrap();
            assert_eq!(state.health, expected);
            assert_eq!(state.readiness, Readiness::Ready);
        }
    }

    #[test]
    fn carries_the_human_message_through() {
        let mut adapter = RustAnalyzerAdapter::new();
        let body = r#"{"method":"experimental/serverStatus","params":{"health":"warning","quiescent":false,"message":"build scripts need rebuilding"}}"#;
        let state = interpret(&mut adapter, body).unwrap();
        assert_eq!(
            state.message.as_deref(),
            Some("build scripts need rebuilding")
        );
    }

    #[test]
    fn ignores_unrelated_notifications() {
        let mut adapter = RustAnalyzerAdapter::new();
        let progress = r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":"x","value":{"kind":"end"}}}"#;
        assert!(interpret(&mut adapter, progress).is_none());
    }

    #[test]
    fn ignores_a_request_that_happens_to_use_the_status_method_name() {
        // serverStatus は通知であってリクエストではない。
        let mut adapter = RustAnalyzerAdapter::new();
        let as_request = r#"{"jsonrpc":"2.0","id":1,"method":"experimental/serverStatus","params":{"health":"ok","quiescent":true}}"#;
        assert!(interpret(&mut adapter, as_request).is_none());
    }

    #[test]
    fn ignores_a_status_whose_params_do_not_parse() {
        let mut adapter = RustAnalyzerAdapter::new();
        let missing_quiescent =
            r#"{"method":"experimental/serverStatus","params":{"health":"ok"}}"#;
        assert!(interpret(&mut adapter, missing_quiescent).is_none());
    }

    #[test]
    fn refuses_observer_only_health_values_claimed_by_the_upstream() {
        // 仕様 6.1: サーバーは dead / unknown を送出してはならない。
        for claimed in ["dead", "unknown"] {
            let mut adapter = RustAnalyzerAdapter::new();
            let body = format!(
                r#"{{"method":"experimental/serverStatus","params":{{"health":"{claimed}","quiescent":true}}}}"#
            );
            assert!(
                interpret(&mut adapter, &body).is_none(),
                "上流の {claimed} を受け入れてはならない"
            );
        }
    }
}
