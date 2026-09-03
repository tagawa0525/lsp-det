//! `ServerState` の保持と遷移 (v0.1-design.md 4.2、ADR 0008)。
//!
//! 写像 (上流メッセージの解釈) と状態の保持を分ける。写像は上流が
//! 名乗るまで選べないので、それまでは両軸 `unknown`。名乗りは
//! `InitializeResult.serverInfo` か、それを返さない上流では起動時の
//! `window/logMessage` (ADR 0011 決定 A)。既知の名前なら写像に切り替え、
//! そうでなければ `unknown` のまま正直に報告する (仕様 8.2 の 3)。

use crate::adapter::{self, Mapping};
use crate::initialize::ServerInfo;
use crate::peek::MessageView;
use crate::state::{ServerState, ServerStateProvider};

pub struct Tracker {
    state: ServerState,
    adapter: Option<Box<dyn Mapping>>,
    /// 写像を選ぶ根拠になった名乗り。
    identity: Option<ServerInfo>,
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
            identity: None,
        }
    }

    /// 写像を選ぶ根拠になった名乗り (`serverInfo` または起動ログ)。
    pub fn identity(&self) -> Option<&ServerInfo> {
        self.identity.as_ref()
    }

    /// 上流が `InitializeResult` で名乗った。既知の名前なら写像を選び、
    /// その開始状態 (「initialize 直後」の `initializing`) に移る。
    /// 名乗りがない・既知でない場合は何もしない。
    ///
    /// 開始状態・保証は写像の値に聞く。「写像がある」ことを rust-analyzer と
    /// 同一視すると、M4 で gopls を足したときに match を書き直すことになる。
    pub fn select_mapping(&mut self, server_info: Option<&ServerInfo>) -> Option<ServerState> {
        let server_info = server_info?;
        if let Some(current) = &self.identity
            && current.name == server_info.name
        {
            // 起動ログで既に同じ写像を選んでいる (basedpyright は両方で名乗る)。
            // 選び直すと起動ログの後に読んだ観測 ("Starting service instance"
            // の数) が消えるので、写像はそのまま名乗りだけ serverInfo に揃える。
            self.identity = Some(server_info.clone());
            return Some(self.state.clone());
        }
        self.adopt(server_info.clone())
    }

    fn adopt(&mut self, identity: ServerInfo) -> Option<ServerState> {
        let adapter = adapter::select(&identity.name, identity.version.as_deref())?;
        self.state = adapter.initial_state();
        self.adapter = Some(adapter);
        self.identity = Some(identity);
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
        // 保証は名乗り (名前と版) の関数 (仕様 8.2 の 5)。起動ログが版を省き、
        // 後から `serverInfo` で版が分かったときも、写像 (と観測) は保ったまま
        // 最新の名乗りで決める。
        self.identity
            .as_ref()
            .and_then(|identity| adapter::select(&identity.name, identity.version.as_deref()))
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
        let Some(adapter) = self.adapter.as_mut() else {
            // 写像がまだない。上流が起動時のログで名乗っていれば、それで選ぶ
            // (ADR 0011 決定 A-2)。`serverInfo` が後から来ればそれで選び直す。
            // 選択は開始状態を置くだけで、通知する変化ではない。`serverInfo` で
            // 選んだときと同じ。起動ログは `initialize` 応答より先に届き、LSP は
            // 応答前のサーバー→クライアント通知を (logMessage 等を除き) 禁じる。
            let identity = adapter::identity_from_notification(view, body)?;
            self.adopt(identity);
            return None;
        };
        let next = adapter.interpret(view, body)?;
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

    // --- 起動ログからの選択 (ADR 0011 決定 A) ----------------------------------

    const PYRIGHT_STARTUP: &str = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"Pyright language server 1.1.412 starting"}}"#;
    const PYRIGHT_STARTED: &str = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"Starting service instance \"pyfix\""}}"#;
    const PYRIGHT_FOUND: &str = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"Found 2 source files"}}"#;

    #[test]
    fn a_startup_log_selects_the_mapping_before_the_upstream_answers_initialize() {
        // pyright は serverInfo を返さない。起動ログの名乗りで写像を選び、
        // 開始状態 (initializing) に移る。選択は通知する変化ではない
        // (serverInfo で選んだときと同じ。起動ログは initialize 応答より先に
        // 届き、LSP は応答前のサーバー→クライアント通知を禁じる)。
        let mut tracker = Tracker::new();
        assert!(
            observe(&mut tracker, PYRIGHT_STARTUP).is_none(),
            "選択は通知しない"
        );
        let state = tracker.state();
        assert_eq!(state.readiness, Readiness::Initializing);
        assert_eq!(state.health, Health::Unknown);
        assert!(tracker.observes_upstream());
        assert_eq!(tracker.identity().map(|i| i.name.as_str()), Some("pyright"));

        // 写像はその後の通知を読む。
        observe(&mut tracker, PYRIGHT_STARTED);
        let ready = observe(&mut tracker, PYRIGHT_FOUND).expect("列挙の完了で ready");
        assert_eq!(ready.readiness, Readiness::Ready);
    }

    #[test]
    fn an_initialize_result_without_server_info_keeps_the_mapping_from_the_startup_log() {
        let mut tracker = Tracker::new();
        observe(&mut tracker, PYRIGHT_STARTUP);
        assert!(
            tracker.select_mapping(None).is_none(),
            "名乗りがなければ選び直さない"
        );
        assert!(tracker.observes_upstream());
        assert_eq!(tracker.state().readiness, Readiness::Initializing);
    }

    #[test]
    fn server_info_is_the_stronger_identity_and_reselects() {
        // basedpyright は起動ログと serverInfo の両方を出す。serverInfo が来たら
        // それで選び直す (同じ写像を指すので状態は開始状態のまま)。
        let mut tracker = Tracker::new();
        let based = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"basedpyright language server 1.39.8 starting"}}"#;
        observe(&mut tracker, based);
        let state = tracker
            .select_mapping(Some(&ServerInfo {
                name: "basedpyright".to_string(),
                version: Some("1.39.8".to_string()),
            }))
            .expect("serverInfo で選び直す");
        assert_eq!(state.readiness, Readiness::Initializing);
        assert!(tracker.observes_upstream());
    }

    #[test]
    fn reselecting_the_same_mapping_from_server_info_keeps_what_it_observed() {
        // basedpyright は起動ログで名乗り、"Starting service instance" を出して
        // から initialize に serverInfo 付きで応答する。serverInfo で選び直す
        // ときに数えたフォルダを捨てると、完了ログが数える相手を失い ready に
        // ならない (実 basedpyright 1.39.8 で観測)。
        let mut tracker = Tracker::new();
        let based = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"basedpyright language server 1.39.8 starting"}}"#;
        observe(&mut tracker, based);
        observe(&mut tracker, PYRIGHT_STARTED);
        tracker.select_mapping(Some(&ServerInfo {
            name: "basedpyright".to_string(),
            version: Some("1.39.8".to_string()),
        }));
        let ready = observe(&mut tracker, PYRIGHT_FOUND).expect("数えたフォルダの完了で ready");
        assert_eq!(ready.readiness, Readiness::Ready);
    }

    #[test]
    fn server_info_updates_the_declared_guarantees_even_when_the_mapping_is_kept() {
        // 起動ログが版を省いていても、serverInfo がテスト済みの版を名乗れば
        // 保証を宣言する (Copilot の指摘)。保証は名乗り (名前と版) の関数で、
        // 観測 (数えたフォルダ) は保つ。
        let mut tracker = Tracker::new();
        let unversioned = r#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"basedpyright language server starting"}}"#;
        observe(&mut tracker, unversioned);
        assert_eq!(tracker.provider(), ServerStateProvider::Basic(true));
        observe(&mut tracker, PYRIGHT_STARTED);

        tracker.select_mapping(Some(&ServerInfo {
            name: "basedpyright".to_string(),
            version: Some("1.39.8".to_string()),
        }));
        assert_eq!(
            tracker.provider(),
            ServerStateProvider::complete_and_fresh(),
            "serverInfo の版で保証を宣言し直していない"
        );
        let ready = observe(&mut tracker, PYRIGHT_FOUND).expect("観測は保たれている");
        assert_eq!(ready.readiness, Readiness::Ready);
    }

    #[test]
    fn a_different_name_in_server_info_replaces_the_mapping() {
        // 名前が違えば serverInfo が強い。起動ログの写像は捨てて選び直す。
        let mut tracker = Tracker::new();
        observe(&mut tracker, PYRIGHT_STARTUP);
        let state = tracker
            .select_mapping(Some(&info("rust-analyzer")))
            .expect("serverInfo で選び直す");
        assert_eq!(state.readiness, Readiness::Initializing);
        assert!(
            observe(&mut tracker, &status("ok", true)).is_some(),
            "rust-analyzer の写像"
        );
        assert_eq!(
            tracker.identity().map(|i| i.name.as_str()),
            Some("rust-analyzer")
        );
    }

    #[test]
    fn ordinary_logs_do_not_select_a_mapping() {
        let mut tracker = Tracker::new();
        assert!(observe(&mut tracker, PYRIGHT_FOUND).is_none());
        assert!(!tracker.observes_upstream());
        assert_eq!(tracker.state(), &ServerState::unobserved());
    }

    #[test]
    fn a_startup_log_does_not_replace_a_mapping_chosen_from_server_info() {
        // 既に写像があるなら、起動ログの名乗りで選び直さない (serverInfo が強い)。
        let mut tracker = with_adapter();
        assert!(observe(&mut tracker, PYRIGHT_STARTUP).is_none());
        assert!(
            observe(&mut tracker, &status("ok", true)).is_some(),
            "rust-analyzer の写像のまま"
        );
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
        assert!(tracker.select_mapping(Some(&info("clangd"))).is_none());
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
