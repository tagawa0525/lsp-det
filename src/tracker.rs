//! `ServerState` の保持と遷移 (v0.1-design.md 4.2、ADR 0008)。
//!
//! 写像 (上流メッセージの解釈) と状態の保持を分ける。写像がなくても
//! 上流側は存在し、両軸 `unknown` を正直に報告する (仕様 8.2 の 3)。

use crate::adapter::RustAnalyzerAdapter;
use crate::peek::MessageView;
use crate::state::{ServerState, ServerStateProvider};

pub struct Tracker {
    state: ServerState,
    adapter: Option<RustAnalyzerAdapter>,
}

impl Tracker {
    /// アダプタがあれば `initializing`、なければ両軸 `unknown` から始める。
    ///
    /// 開始状態・注入する capability・保証グレードはアダプタの値に聞く。
    /// 「アダプタがある」ことを rust-analyzer と同一視すると、M3 で gopls を
    /// 足したときに 3 箇所の match を書き直すことになる。
    pub fn new(adapter: Option<RustAnalyzerAdapter>) -> Self {
        let state = adapter
            .as_ref()
            .map_or_else(ServerState::unobserved, |adapter| adapter.initial_state());
        Tracker { state, adapter }
    }

    pub fn state(&self) -> &ServerState {
        &self.state
    }

    /// 上流のメッセージから読み取るものがあるか (アダプタがあるか)。
    /// なければ透過経路は覗き見を省ける。
    pub fn observes_upstream(&self) -> bool {
        self.adapter.is_some()
    }

    /// 上流への `initialize` に注入する client capability (v0.1-design.md 4.5)。
    /// アダプタがなければ何も注入しない。
    pub fn required_client_capabilities(&self) -> &'static [&'static str] {
        self.adapter
            .as_ref()
            .map_or(&[], |adapter| adapter.required_client_capabilities())
    }

    /// `InitializeResult` に宣言する保証グレード (仕様 5 章)。
    /// アダプタがなければ基本グレード。
    pub fn provider(&self) -> ServerStateProvider {
        self.adapter
            .as_ref()
            .map_or(ServerStateProvider::Basic(true), |adapter| {
                adapter.guarantees()
            })
    }

    /// 上流→クライアント方向のメッセージを観測して状態を更新する。
    /// 通知を要する変化 (仕様 4.2) があった場合のみ新しい状態を返す。
    ///
    /// アダプタがなければ何も読まない。rust-analyzer の語彙を知っているのは
    /// アダプタだけで、なしのときに勝手に読むと他のサーバーの同名通知を
    /// 誤読する。
    pub fn observe_upstream(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        let next = self.adapter.as_mut()?.interpret(view, body)?;
        self.apply(next)
    }

    /// 新しい状態を取り込み、通知を要する変化なら新しい状態を返す。
    /// `message` だけの変化は通知しないが、状態としては更新する。
    fn apply(&mut self, next: ServerState) -> Option<ServerState> {
        let notifiable = next.notifiable_change_from(&self.state);
        self.state = next;
        notifiable.then(|| self.state.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;
    use crate::state::{Health, Readiness};

    fn observe(tracker: &mut Tracker, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        tracker.observe_upstream(&view, body.as_bytes())
    }

    fn status(health: &str, quiescent: bool) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{{"health":"{health}","quiescent":{quiescent},"message":null}}}}"#
        )
    }

    fn with_adapter() -> Tracker {
        Tracker::new(Some(RustAnalyzerAdapter::new()))
    }

    fn without_adapter() -> Tracker {
        Tracker::new(None)
    }

    // --- 開始状態 -----------------------------------------------------------

    #[test]
    fn starts_in_initializing_with_an_adapter() {
        // readiness は「initialize 直後」という既知の局面なので initializing。
        // health は最初の serverStatus が届くまで何も観測していないので
        // unknown (ADR 0008 追補 E)。ok を名乗るのは観測なしの主張になる。
        let tracker = with_adapter();
        assert_eq!(tracker.state().readiness, Readiness::Initializing);
        assert_eq!(tracker.state().health, Health::Unknown);
    }

    #[test]
    fn the_first_status_replaces_the_unknown_health() {
        let mut tracker = with_adapter();
        let changed = observe(&mut tracker, &status("ok", false)).expect("state should change");
        assert_eq!(changed.health, Health::Ok);
    }

    #[test]
    fn starts_unobserved_without_an_adapter() {
        // v0.1-design 4.1: initializing からも ok からも始めない。
        let tracker = without_adapter();
        assert_eq!(tracker.state(), &ServerState::unobserved());
    }

    #[test]
    fn declares_the_adapter_guarantees_or_the_basic_grade() {
        assert_eq!(
            with_adapter().provider(),
            ServerStateProvider::complete_and_fresh()
        );
        assert_eq!(
            without_adapter().provider(),
            ServerStateProvider::Basic(true)
        );
    }

    #[test]
    fn injects_client_capabilities_only_with_an_adapter() {
        assert_eq!(
            with_adapter().required_client_capabilities(),
            RustAnalyzerAdapter::REQUIRED_CLIENT_CAPABILITIES
        );
        assert!(without_adapter().required_client_capabilities().is_empty());
    }

    // --- 遷移 (アダプタあり) -----------------------------------------------

    #[test]
    fn applies_what_the_adapter_reads() {
        let mut tracker = with_adapter();
        let changed = observe(&mut tracker, &status("ok", true)).expect("state should change");
        assert_eq!(changed.readiness, Readiness::Ready);
        assert_eq!(tracker.state().readiness, Readiness::Ready);
    }

    #[test]
    fn repeating_the_same_state_does_not_notify_again() {
        let mut tracker = with_adapter();
        assert!(observe(&mut tracker, &status("ok", true)).is_some());
        assert!(observe(&mut tracker, &status("ok", true)).is_none());
    }

    #[test]
    fn losing_quiescence_returns_to_indexing() {
        // 再インデックス (v0.1-design 4.3、仕様 6 章 3 項)。readiness の後退も
        // 2 軸の変化なので通知する。
        let mut tracker = with_adapter();
        observe(&mut tracker, &status("ok", true));
        let changed = observe(&mut tracker, &status("ok", false)).expect("re-index should notify");
        assert_eq!(changed.readiness, Readiness::Indexing);
    }

    #[test]
    fn a_message_only_change_updates_state_without_notifying() {
        // 仕様 4.2: 通知するのは 2 軸が変わったときだけ。ただし message は
        // 次の serverState 応答に載るよう更新しておく。
        let mut tracker = with_adapter();
        observe(&mut tracker, &status("ok", false));

        let with_message = r#"{"method":"experimental/serverStatus","params":{"health":"ok","quiescent":false,"message":"loading crates"}}"#;
        assert!(observe(&mut tracker, with_message).is_none());
        assert_eq!(tracker.state().message.as_deref(), Some("loading crates"));
    }

    // --- 遷移 (アダプタなし) -----------------------------------------------

    #[test]
    fn does_not_interpret_upstream_status_without_an_adapter() {
        // rust-analyzer の語彙を知っているのはアダプタだけ。なしのときに
        // 勝手に読むと、他のサーバーの同名通知を誤読する。
        let mut tracker = without_adapter();
        assert!(observe(&mut tracker, &status("ok", true)).is_none());
        assert_eq!(tracker.state(), &ServerState::unobserved());
    }
}
