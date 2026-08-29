//! ready 判定アダプタ (v0.1-design.md 5 章)。M2 では rust-analyzer のみ。
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
//! gopls アダプタは M3。共通の trait はそのとき 2 つ目の実装を見てから
//! 導入する (現在の要件に対する最小限の実装)。

use serde::Deserialize;

use crate::peek::MessageView;
use crate::state::{Health, Readiness, ServerState};

/// rust-analyzer が送る readiness 通知のメソッド名。
pub const SERVER_STATUS_METHOD: &str = "experimental/serverStatus";

/// `experimental/serverStatus` の params。
///
/// `health` を `state::Health` ではなく専用の enum で受けるのは、仕様 6.1 が
/// 「サーバーは `dead` を送出してはならない」と定めているため。上流が
/// `"dead"` を送ってきてもパースに失敗し、状態は変わらない。
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

pub struct RustAnalyzerAdapter {
    state: ServerState,
    /// パース不能な status を一度ログしたか (連投を避けるため)。
    warned_unparseable: bool,
}

impl RustAnalyzerAdapter {
    /// 上流への `initialize` に注入する client capability (v0.1-design.md 4.5)。
    /// 未宣言だと rust-analyzer は `experimental/serverStatus` を一切送らない。
    pub const REQUIRED_CLIENT_CAPABILITIES: &'static [&'static str] =
        &["experimental.serverStatusNotification"];

    pub fn new() -> Self {
        RustAnalyzerAdapter {
            state: ServerState::initializing(),
            warned_unparseable: false,
        }
    }

    pub fn state(&self) -> &ServerState {
        &self.state
    }

    /// 上流→クライアント方向のメッセージを観測して状態を更新する。
    /// 通知を要する変化 (仕様 4.2) があった場合のみ新しい状態を返す。
    pub fn observe_upstream(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if self.state.health == Health::Dead {
            return None;
        }
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

        self.apply(ServerState {
            health: params.health.into(),
            readiness: if params.quiescent {
                Readiness::Ready
            } else {
                Readiness::Indexing
            },
            message: params.message,
        })
    }

    /// 上流プロセスの消失を観測した。`dead` は中継層だけが出せる終端状態
    /// (仕様 6.1)。
    ///
    /// `readiness` は直前の値のまま残す。仕様 3 章が 2 軸を独立と定め、
    /// 「`health` が `error | dead` のとき `readiness` を判断材料に
    /// 使うべきではない」を推奨解釈としているため (ADR 0004 決定 1)。
    /// `dead` に対応する `readiness` の値は仕様に存在せず、`initializing`
    /// へ倒すのは別の嘘になる。
    ///
    /// **消費者への注意**: `{health: "dead", readiness: "ready"}` は正常に
    /// 出る組み合わせである。`readiness` を先に見る実装は死んだサーバーを
    /// 応答可能とみなす。ゲート (設計 4.2) は `health` を先に判定すること。
    pub fn mark_dead(&mut self) -> Option<ServerState> {
        self.apply(ServerState {
            health: Health::Dead,
            readiness: self.state.readiness,
            message: self.state.message.clone(),
        })
    }

    /// 新しい状態を取り込み、通知を要する変化なら新しい状態を返す。
    /// `message` だけの変化は通知しないが、状態としては更新する。
    fn apply(&mut self, next: ServerState) -> Option<ServerState> {
        let notifiable = next.notifiable_change_from(&self.state);
        self.state = next;
        notifiable.then(|| self.state.clone())
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

    /// 上流からの 1 メッセージを観測させる。
    fn observe(adapter: &mut RustAnalyzerAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.observe_upstream(&view, body.as_bytes())
    }

    fn status(health: &str, quiescent: bool) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{{"health":"{health}","quiescent":{quiescent},"message":null}}}}"#
        )
    }

    #[test]
    fn starts_in_initializing() {
        let adapter = RustAnalyzerAdapter::new();
        assert_eq!(adapter.state().readiness, Readiness::Initializing);
        assert_eq!(adapter.state().health, Health::Ok);
    }

    #[test]
    fn a_non_quiescent_status_means_indexing() {
        let mut adapter = RustAnalyzerAdapter::new();
        let changed = observe(&mut adapter, &status("ok", false)).expect("state should change");
        assert_eq!(changed.readiness, Readiness::Indexing);
        assert_eq!(adapter.state().readiness, Readiness::Indexing);
    }

    #[test]
    fn a_quiescent_status_means_ready() {
        let mut adapter = RustAnalyzerAdapter::new();
        let changed = observe(&mut adapter, &status("ok", true)).expect("state should change");
        assert_eq!(changed.readiness, Readiness::Ready);
    }

    #[test]
    fn repeating_the_same_status_does_not_notify_again() {
        let mut adapter = RustAnalyzerAdapter::new();
        assert!(observe(&mut adapter, &status("ok", true)).is_some());
        assert!(observe(&mut adapter, &status("ok", true)).is_none());
    }

    #[test]
    fn losing_quiescence_returns_to_indexing() {
        // 再インデックス (v0.1-design 4.3)。ワークスペース構成の変更で起きる。
        let mut adapter = RustAnalyzerAdapter::new();
        observe(&mut adapter, &status("ok", true));
        let changed = observe(&mut adapter, &status("ok", false)).expect("re-index should notify");
        assert_eq!(changed.readiness, Readiness::Indexing);
    }

    #[test]
    fn carries_health_through_unchanged() {
        for (upstream, expected) in [
            ("ok", Health::Ok),
            ("warning", Health::Warning),
            ("error", Health::Error),
        ] {
            let mut adapter = RustAnalyzerAdapter::new();
            observe(&mut adapter, &status(upstream, true));
            assert_eq!(adapter.state().health, expected);
        }
    }

    #[test]
    fn carries_the_human_message_through() {
        let mut adapter = RustAnalyzerAdapter::new();
        let body = r#"{"method":"experimental/serverStatus","params":{"health":"warning","quiescent":false,"message":"build scripts need rebuilding"}}"#;
        observe(&mut adapter, body);
        assert_eq!(
            adapter.state().message.as_deref(),
            Some("build scripts need rebuilding")
        );
    }

    #[test]
    fn a_message_only_change_updates_state_without_notifying() {
        // 仕様 4.2: 通知するのは 2 軸が変わったときだけ。ただし message は
        // 次の serverState 応答に載るよう更新しておく。
        let mut adapter = RustAnalyzerAdapter::new();
        observe(&mut adapter, &status("ok", false));

        let with_message = r#"{"method":"experimental/serverStatus","params":{"health":"ok","quiescent":false,"message":"loading crates"}}"#;
        assert!(observe(&mut adapter, with_message).is_none());
        assert_eq!(adapter.state().message.as_deref(), Some("loading crates"));
    }

    #[test]
    fn ignores_unrelated_notifications() {
        let mut adapter = RustAnalyzerAdapter::new();
        let progress = r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":"x","value":{"kind":"end"}}}"#;
        assert!(observe(&mut adapter, progress).is_none());
        assert_eq!(adapter.state().readiness, Readiness::Initializing);
    }

    #[test]
    fn ignores_a_request_that_happens_to_use_the_status_method_name() {
        // serverStatus は通知であってリクエストではない。
        let mut adapter = RustAnalyzerAdapter::new();
        let as_request = r#"{"jsonrpc":"2.0","id":1,"method":"experimental/serverStatus","params":{"health":"ok","quiescent":true}}"#;
        assert!(observe(&mut adapter, as_request).is_none());
        assert_eq!(adapter.state().readiness, Readiness::Initializing);
    }

    #[test]
    fn ignores_a_status_whose_params_do_not_parse() {
        let mut adapter = RustAnalyzerAdapter::new();
        let missing_quiescent =
            r#"{"method":"experimental/serverStatus","params":{"health":"ok"}}"#;
        assert!(observe(&mut adapter, missing_quiescent).is_none());
        assert_eq!(adapter.state().readiness, Readiness::Initializing);
    }

    #[test]
    fn refuses_a_dead_health_claimed_by_the_upstream() {
        // 仕様 6.1: サーバーは dead を送出してはならない。
        let mut adapter = RustAnalyzerAdapter::new();
        observe(&mut adapter, &status("ok", true));
        let claims_dead =
            r#"{"method":"experimental/serverStatus","params":{"health":"dead","quiescent":true}}"#;
        assert!(observe(&mut adapter, claims_dead).is_none());
        assert_eq!(adapter.state().health, Health::Ok);
    }

    #[test]
    fn marking_dead_notifies_once() {
        let mut adapter = RustAnalyzerAdapter::new();
        let changed = adapter.mark_dead().expect("death should notify");
        assert_eq!(changed.health, Health::Dead);
        assert!(adapter.mark_dead().is_none());
    }

    #[test]
    fn dead_is_terminal() {
        // 仕様 6.1: dead は終端状態。上流の残存メッセージで生き返らない。
        let mut adapter = RustAnalyzerAdapter::new();
        adapter.mark_dead();
        assert!(observe(&mut adapter, &status("ok", true)).is_none());
        assert_eq!(adapter.state().health, Health::Dead);
    }
}
