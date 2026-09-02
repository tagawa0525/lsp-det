//! `ServerState` の保持と遷移 (v0.1-design.md 4.2、ADR 0008)。
//!
//! 写像 (上流メッセージの解釈) と状態の保持を分ける。写像は上流が
//! `InitializeResult.serverInfo` で名乗るまで選べないので、それまでは
//! 両軸 `unknown`。既知の名前なら写像に切り替え、そうでなければ
//! `unknown` のまま正直に報告する (仕様 8.2 の 3)。

use crate::adapter::{self, RustAnalyzerAdapter};
use crate::initialize::ServerInfo;
use crate::peek::MessageView;
use crate::state::{ServerState, ServerStateProvider};

pub struct Tracker {
    state: ServerState,
    adapter: Option<RustAnalyzerAdapter>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

impl Tracker {
    /// 上流が名乗るまでは写像がなく、両軸 `unknown`。
    pub fn new() -> Self {
        Tracker {
            state: ServerState::unobserved(),
            adapter: None,
        }
    }

    /// 上流が `InitializeResult` で名乗った。既知の名前なら写像を選び、
    /// その開始状態 (「initialize 直後」の `initializing`) に移る。
    /// 名乗りがない・既知でない場合は何もしない。
    ///
    /// 開始状態・保証は写像の値に聞く。「写像がある」ことを rust-analyzer と
    /// 同一視すると、M4 で gopls を足したときに match を書き直すことになる。
    pub fn select_mapping(&mut self, server_info: Option<&ServerInfo>) -> Option<ServerState> {
        let server_info = server_info?;
        let adapter = adapter::select(&server_info.name, server_info.version.as_deref())?;
        self.state = adapter.initial_state();
        self.adapter = Some(adapter);
        Some(self.state.clone())
    }

    pub fn state(&self) -> &ServerState {
        &self.state
    }

    /// 上流のメッセージから読み取るものがあるか (アダプタがあるか)。
    /// なければ透過経路は覗き見を省ける。
    pub fn observes_upstream(&self) -> bool {
        self.adapter.is_some()
    }

    /// `InitializeResult` に宣言する保証 (仕様 5 章)。
    /// 写像がなければ保証なしの宣言 (`true`)。
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

    /// 準拠テストを通した版 (adapter::TESTED_VERSIONS) を名乗る serverInfo。
    fn info(name: &str) -> ServerInfo {
        ServerInfo {
            name: name.to_string(),
            version: Some("1.98.0 (88d9e12 2026-08-18)".to_string()),
        }
    }

    fn with_adapter() -> Tracker {
        let mut tracker = Tracker::new();
        tracker.select_mapping(Some(&info("rust-analyzer")));
        tracker
    }

    fn without_adapter() -> Tracker {
        let mut tracker = Tracker::new();
        tracker.select_mapping(Some(&info("fake-lsp-server")));
        tracker
    }

    // --- 開始状態 -----------------------------------------------------------

    #[test]
    fn starts_unobserved_before_the_upstream_names_itself() {
        // 写像は serverInfo で選ぶ。それまでは何も観測していない。
        assert_eq!(Tracker::new().state(), &ServerState::unobserved());
    }

    #[test]
    fn selecting_a_known_mapping_moves_to_initializing() {
        // readiness は「initialize 直後」という既知の局面なので initializing。
        // health は最初の serverStatus が届くまで何も観測していないので
        // unknown (仕様 8.2 の 2)。ok を名乗るのは観測なしの主張になる。
        let mut tracker = Tracker::new();
        let state = tracker
            .select_mapping(Some(&info("rust-analyzer")))
            .expect("既知の名前なら開始状態に移る");
        assert_eq!(state.readiness, Readiness::Initializing);
        assert_eq!(state.health, Health::Unknown);
        assert!(tracker.observes_upstream());
    }

    #[test]
    fn the_first_status_replaces_the_unknown_health() {
        let mut tracker = with_adapter();
        let changed = observe(&mut tracker, &status("ok", false)).expect("state should change");
        assert_eq!(changed.health, Health::Ok);
    }

    #[test]
    fn stays_unobserved_when_the_name_is_unknown_or_absent() {
        // 仕様 8.2 の 3: 信号のないサーバーは両軸 unknown。initializing からも
        // ok からも始めない。
        let mut tracker = Tracker::new();
        assert!(tracker.select_mapping(None).is_none());
        assert!(tracker.select_mapping(Some(&info("gopls"))).is_none());
        assert_eq!(tracker.state(), &ServerState::unobserved());
        assert!(!tracker.observes_upstream());
    }

    #[test]
    fn declares_the_adapter_guarantees_or_no_guarantees() {
        assert_eq!(
            with_adapter().provider(),
            ServerStateProvider::complete_and_fresh()
        );
        assert_eq!(
            without_adapter().provider(),
            ServerStateProvider::Basic(true)
        );
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
