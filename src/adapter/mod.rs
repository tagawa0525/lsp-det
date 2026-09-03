//! 写像 (アダプタ、v0.1-design.md 5 章)。言語サーバー固有の語彙を
//! サーバー状態プロトコルに写す。
//!
//! 写像の役割は**上流メッセージの解釈**だけである。状態の保持と重複
//! 抑止は [`crate::tracker::Tracker`] が持つ。分けるのは、写像がなくても
//! 上流側は存在し、両軸 `unknown` を報告するからである (仕様 8.2 の 3)。
//!
//! 写像は上流が `InitializeResult.serverInfo.name` で名乗る名前で選ぶ
//! ([`select`])。言語サーバーの語彙の粗さを補う責任も写像が持ち、下流側は
//! 仕様の値だけを見る (ADR 0009 決定 D-6)。

pub mod gopls;
pub mod pyright;
pub mod rust_analyzer;

pub use gopls::{GoplsAdapter, TESTED_VERSIONS as GOPLS_TESTED_VERSIONS};
pub use pyright::{PyrightAdapter, TESTED_VERSIONS as PYRIGHT_TESTED_VERSIONS};
pub use rust_analyzer::{
    RustAnalyzerAdapter, SERVER_STATUS_METHOD, TESTED_VERSIONS as RUST_ANALYZER_TESTED_VERSIONS,
};

use crate::initialize::ServerInfo;
use crate::peek::MessageView;
use crate::state::{ServerState, ServerStateProvider};

/// 言語サーバー固有の語彙から `ServerState` を読む写像。
pub trait Mapping {
    /// 上流に接続した直後 (写像を選んだ時点) の状態。
    fn initial_state(&self) -> ServerState;
    /// `InitializeResult` に宣言する保証 (仕様 5 章)。準拠テストを通した版の
    /// 範囲でのみ宣言する (仕様 8.2 の 5)。
    fn guarantees(&self) -> ServerStateProvider;
    /// 上流→クライアント方向のメッセージから、上流が報告している状態を
    /// 読み取る。読むものがなければ `None` (状態を動かさない)。
    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState>;
}

/// 既知の写像すべてが必要とする client capability の和 (設計 4.2)。
///
/// 写像は `InitializeResult.serverInfo.name` で選ぶが、注入は上流へ
/// `initialize` を送る前に要る。だから上流が誰であっても全部を注入する
/// (ADR 0009 決定 D-3)。どちらも「通知を送ってよい」という許可にすぎない。
///
/// - `experimental.serverStatusNotification`: rust-analyzer。未宣言だと
///   `experimental/serverStatus` は一切送られない
/// - `window.workDoneProgress`: gopls (M4)。未宣言だと `$/progress` ではなく
///   `window/showMessage` にフォールバックする
pub const CLIENT_CAPABILITIES_FOR_ALL_MAPPINGS: &[&str] = &[
    "experimental.serverStatusNotification",
    "window.workDoneProgress",
];

/// 上流が名乗った名前に対応する写像。既知でなければ `None`
/// (上流側は両軸 `unknown` を報告する。仕様 8.2 の 3)。
pub fn select(server_name: &str, version: Option<&str>) -> Option<Box<dyn Mapping>> {
    match server_name {
        "rust-analyzer" => Some(Box::new(RustAnalyzerAdapter::for_version(version))),
        "gopls" => Some(Box::new(GoplsAdapter::for_version(version))),
        "pyright" | "basedpyright" => Some(Box::new(PyrightAdapter::for_version(version))),
        _ => None,
    }
}

/// `serverInfo` を返さない上流の名乗り (ADR 0011 決定 A-2)。
///
/// 上流→クライアント方向の通知から、上流が起動時に自ら送る名乗りを読む。
/// 今のところ pyright 系の `window/logMessage`
/// ("Pyright language server 1.1.412 starting") だけ。名乗りでなければ `None`。
/// 汎用の認識機構は作らない (必要になった写像が自分の認識を足す)。
pub fn identity_from_notification(view: &MessageView, body: &[u8]) -> Option<ServerInfo> {
    let _ = (view, body);
    todo!("M5 GREEN")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_gopls_by_its_server_info_name() {
        assert!(select("gopls", Some("v0.20.0")).is_some());
    }

    #[test]
    fn selects_pyright_and_basedpyright_by_their_server_info_names() {
        // basedpyright は serverInfo を返す。pyright は返さないが、名乗りの
        // 鍵は同じ "pyright" に揃える (identity_from_notification)。
        assert!(select("basedpyright", Some("1.39.8")).is_some());
        assert!(select("pyright", None).is_some());
        assert!(select("Pyright", None).is_none(), "鍵は小文字に揃える");
    }

    #[test]
    fn identifies_pyright_by_its_startup_log() {
        use crate::peek::peek;
        let body = br#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"Pyright language server 1.1.412 starting"}}"#;
        let view = peek(body).unwrap();
        let identity = identity_from_notification(&view, body).expect("起動ログは名乗り");
        assert_eq!(identity.name, "pyright");
        assert_eq!(identity.version.as_deref(), Some("1.1.412"));
        assert!(select(&identity.name, identity.version.as_deref()).is_some());
    }

    #[test]
    fn other_notifications_and_requests_are_not_identities() {
        use crate::peek::peek;
        for body in [
            &br#"{"jsonrpc":"2.0","method":"window/logMessage","params":{"type":3,"message":"Found 2 source files"}}"#[..],
            br#"{"jsonrpc":"2.0","method":"window/showMessage","params":{"type":3,"message":"Pyright language server 1.1.412 starting"}}"#,
            br#"{"jsonrpc":"2.0","id":7,"method":"window/workDoneProgress/create","params":{"token":"t"}}"#,
            br#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"ok","quiescent":true}}"#,
            br#"{"jsonrpc":"2.0","id":0,"result":{"capabilities":{}}}"#,
        ] {
            let view = peek(body).unwrap();
            assert!(
                identity_from_notification(&view, body).is_none(),
                "名乗りでないメッセージ: {}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn selects_rust_analyzer_by_its_server_info_name() {
        assert!(select("rust-analyzer", None).is_some());
        for unknown in ["fake-lsp-server", "", "Rust-Analyzer", "clangd"] {
            assert!(
                select(unknown, None).is_none(),
                "既知でない名前: {unknown:?}"
            );
        }
    }

    #[test]
    fn declares_guarantees_only_for_versions_the_conformance_suite_passed_on() {
        // 仕様 8.2 の 5 (ADR 0009 決定 D-5): 観測者が宣言できる保証は、
        // 準拠テスト 7.2 / 7.3 を当てて通った版の範囲に限る。lsp-det は
        // rust-analyzer の内部を保証できず、テストに通ったという観測しか持たない。
        let tested = select("rust-analyzer", Some("1.98.0 (88d9e12 2026-08-18)")).unwrap();
        assert_eq!(
            tested.guarantees(),
            ServerStateProvider::complete_and_fresh()
        );

        for untested in [
            Some("1.97.0 (abcdef1 2026-07-01)"),
            Some("0.3.2600-standalone"),
            Some("garbage"),
            None,
        ] {
            let adapter = select("rust-analyzer", untested).unwrap();
            assert_eq!(
                adapter.guarantees(),
                ServerStateProvider::Basic(true),
                "テストを当てていない版 {untested:?} に保証を宣言した"
            );
        }
    }
}
