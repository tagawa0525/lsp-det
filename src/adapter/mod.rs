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
pub mod rust_analyzer;

pub use gopls::GoplsAdapter;
pub use rust_analyzer::{RustAnalyzerAdapter, SERVER_STATUS_METHOD, TESTED_VERSIONS};

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
        "gopls" => Some(Box::new(GoplsAdapter::new())),
        _ => None,
    }
}

/// `X.Y.Z` の 3 つ組。rust-analyzer の版文字列の先頭部分。
pub type Version = (u32, u32, u32);

/// rust-analyzer の版文字列 (`1.98.0 (88d9e12 2026-08-18)` 等) の先頭の
/// `X.Y.Z` を読む。ハッシュや日付、`-standalone` 等の後置は捨てる。
pub fn parse_version(version: &str) -> Option<Version> {
    let leading = version.split([' ', '-']).next().unwrap_or("");
    let mut parts = leading.split('.').map(|part| part.parse::<u32>().ok());
    let (major, minor, patch) = (parts.next()??, parts.next()??, parts.next()??);
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_gopls_by_its_server_info_name() {
        assert!(select("gopls", Some("v0.20.0")).is_some());
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

    #[test]
    fn parses_the_leading_semver_of_a_rust_analyzer_version_string() {
        assert_eq!(
            parse_version("1.98.0 (88d9e12 2026-08-18)"),
            Some((1, 98, 0))
        );
        assert_eq!(parse_version("1.98.0"), Some((1, 98, 0)));
        assert_eq!(parse_version("0.3.2600-standalone"), Some((0, 3, 2600)));
        assert_eq!(parse_version("nightly"), None);
        assert_eq!(parse_version(""), None);
    }
}
