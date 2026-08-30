//! `ServerState` の保持と遷移 (v0.1-design.md 4.1、ADR 0008)。
//!
//! アダプタ (上流メッセージの解釈) と状態の保持を分ける。分けるのは、
//! アダプタがなくてもプロセスの消失は観測できるからである。アダプタなしの
//! 中継層は両軸 `unknown` から始まり、消失で `health` だけが `dead` になる。
//! これが中継層の固有価値 (`dead` を出せること、ADR 0003) をアダプタのない
//! サーバーにも届ける経路になる。

use crate::adapter::RustAnalyzerAdapter;
use crate::peek::MessageView;
use crate::state::{ServerState, ServerStateProvider};

pub struct Tracker {
    state: ServerState,
    adapter: Option<RustAnalyzerAdapter>,
}

impl Tracker {
    /// アダプタがあれば `initializing`、なければ両軸 `unknown` から始める。
    pub fn new(adapter: Option<RustAnalyzerAdapter>) -> Self {
        let _ = adapter;
        todo!("ADR 0008: アダプタの有無で開始状態を変える")
    }

    pub fn state(&self) -> &ServerState {
        &self.state
    }

    /// 上流への `initialize` に注入する client capability (v0.1-design.md 4.5)。
    /// アダプタがなければ何も注入しない。
    pub fn required_client_capabilities(&self) -> &'static [&'static str] {
        todo!("ADR 0008: アダプタに委譲する")
    }

    /// `InitializeResult` に宣言する保証グレード (仕様 5 章)。
    /// アダプタがなければ基本グレード。
    pub fn provider(&self) -> ServerStateProvider {
        todo!("ADR 0008: アダプタなしは基本グレード")
    }

    /// 上流→クライアント方向のメッセージを観測して状態を更新する。
    /// 通知を要する変化 (仕様 4.2) があった場合のみ新しい状態を返す。
    pub fn observe_upstream(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        let _ = (view, body);
        todo!("ADR 0008: アダプタで解釈し、2 軸の変化だけを通知する")
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
    /// 出る組み合わせである。ゲート (設計 4.2 の表) は `health` の行を先に
    /// 見ること。
    pub fn mark_dead(&mut self) -> Option<ServerState> {
        todo!("ADR 0008: health だけを dead にする")
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
        let tracker = with_adapter();
        assert_eq!(tracker.state().readiness, Readiness::Initializing);
        assert_eq!(tracker.state().health, Health::Ok);
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

    // --- dead ---------------------------------------------------------------

    #[test]
    fn marking_dead_notifies_once() {
        let mut tracker = with_adapter();
        let changed = tracker.mark_dead().expect("death should notify");
        assert_eq!(changed.health, Health::Dead);
        assert!(tracker.mark_dead().is_none());
    }

    #[test]
    fn dead_keeps_the_previous_readiness() {
        let mut tracker = with_adapter();
        observe(&mut tracker, &status("ok", true));
        let dead = tracker.mark_dead().expect("death should notify");
        assert_eq!(dead.readiness, Readiness::Ready);
    }

    #[test]
    fn dead_is_terminal() {
        // 仕様 6.1: dead は終端状態。上流の残存メッセージで生き返らない。
        let mut tracker = with_adapter();
        tracker.mark_dead();
        assert!(observe(&mut tracker, &status("ok", true)).is_none());
        assert_eq!(tracker.state().health, Health::Dead);
    }

    #[test]
    fn dead_is_reported_even_without_an_adapter() {
        // 中継層の固有価値。アダプタがなくてもプロセス消失は観測できる。
        let mut tracker = without_adapter();
        let dead = tracker.mark_dead().expect("death should notify");
        assert_eq!(dead.health, Health::Dead);
        assert_eq!(dead.readiness, Readiness::Unknown);
    }
}
