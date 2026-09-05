//! rust-analyzer の写像 (v0.1-design.md 5.1)。
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

use serde::Deserialize;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{ALL_FILE_CHANGES, Health, Readiness, ServerState, ServerStateProvider};

/// rust-analyzer が送る readiness 通知のメソッド名。
pub const SERVER_STATUS_METHOD: &str = "experimental/serverStatus";

/// `experimental/serverStatus` の params。
///
/// `health` を `state::Health` ではなく専用の enum で受けるのは、仕様 8.1 が
/// 「サーバーは `unknown` を送出してはならない」と定めているため。上流が
/// それを送ってきてもパースに失敗し、状態は変わらない。
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

/// 準拠テスト 7.2 / 7.3 を実 rust-analyzer に当てて通した版。`serverInfo.version`
/// の先頭トークン (空白の前) と完全一致で突き合わせる。
///
/// rustup の配布物は `1.98.0 (88d9e12 2026-08-18)`、nixpkgs のビルドは
/// `2026-08-03` と名乗る。形式が違うので semver として解釈せず、名乗りを
/// そのまま一覧に持つ。lsp-det は rust-analyzer の内部を保証できず、テストに
/// 通ったという観測しか持たない (仕様 8.2 の 5)。一覧にない版には保証を
/// 宣言しない。足すときは、その版で
/// `cargo test --test conformance -- --ignored` を通してから (守れない保証の
/// 宣言は仕様 5.1 違反)。
///
/// 通した記録:
/// - `1.98.0 (88d9e12 2026-08-18)` (rustup stable)、2026-08-29 と 2026-09-03
/// - `2026-08-03` (nixpkgs、flake.nix の開発環境)、2026-09-03
pub const TESTED_VERSIONS: &[&str] = &["1.98.0", "2026-08-03"];

/// `serverInfo.version` の先頭トークン。ハッシュや日付の後置を捨てる。
fn leading_token(version: &str) -> &str {
    version.split_whitespace().next().unwrap_or("")
}

/// プロジェクトが 1 つも見つからないときに rust-analyzer が `warning` に
/// 添えるメッセージ (`reload.rs` の `current_status()`)。判別材料はこれしか
/// ないので文字列で見る。脆いが、[`TESTED_VERSIONS`] の範囲で守る。
const MISSING_WORKSPACE_MESSAGE: &str = "Failed to discover workspace.";

pub struct RustAnalyzerAdapter {
    /// 名乗った版が [`TESTED_VERSIONS`] に入っているか。保証を宣言する条件。
    version_is_tested: bool,
    /// パース不能な status を一度ログしたか (連投を避けるため)。
    warned_unparseable: bool,
    /// `workspace/symbol` の上限。既定 128。クライアントの
    /// `initializationOptions.workspace.symbol.search.limit` で変わる。
    workspace_symbol_limit: u64,
    /// 最後に読んだ health。通知からの先読みで readiness だけを動かすときに使う。
    last_health: Health,
}

/// rust-analyzer が既定で持つ `workspace/symbol` の上限 (`config.rs` の
/// `workspace_symbol_search_limit`)。
const DEFAULT_WORKSPACE_SYMBOL_LIMIT: u64 = 128;

/// rust-analyzer が `client/registerCapability` で監視を登録するファイル
/// (`**/*.rs`、`**/Cargo.{toml,lock}`、`**/rust-analyzer.toml`) か。これらの
/// Created / Deleted には必ず `quiescent: false → true` が続く
/// (research/disk-edit-propagation-measurement.md の追記)。
fn is_watched_file(uri: &str) -> bool {
    // URI の最後の要素で見る。Windows の file URI は `\\` 区切りで来ることがある。
    let name = uri.rsplit(['/', '\\']).next().unwrap_or(uri);
    name.ends_with(".rs") || matches!(name, "Cargo.toml" | "Cargo.lock" | "rust-analyzer.toml")
}

impl Default for RustAnalyzerAdapter {
    fn default() -> Self {
        Self::for_version(None)
    }
}

impl RustAnalyzerAdapter {
    /// 版を名乗らない (または読めない) rust-analyzer 向け。保証は宣言しない。
    pub fn new() -> Self {
        Self::default()
    }

    /// `serverInfo.version` を見て、テスト済みの版なら保証を宣言する。
    pub fn for_version(version: Option<&str>) -> Self {
        let version_is_tested =
            version.is_some_and(|v| TESTED_VERSIONS.contains(&leading_token(v)));
        RustAnalyzerAdapter {
            version_is_tested,
            warned_unparseable: false,
            workspace_symbol_limit: DEFAULT_WORKSPACE_SYMBOL_LIMIT,
            last_health: Health::Unknown,
        }
    }

    /// 名乗った版が準拠テストを通した範囲に入っているか。
    pub fn version_is_tested(&self) -> bool {
        self.version_is_tested
    }
}

impl Mapping for RustAnalyzerAdapter {
    /// 上流に接続した直後の状態。rust-analyzer は `initialize` 応答後に
    /// 最初の `serverStatus` を送るまで何も報告しない。
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// `InitializeResult` に宣言する保証 (仕様 5 章)。
    ///
    /// rust-analyzer は両方の保証を満たす。準拠テストスイートの仕様 7.2
    /// (完全性) と 7.3 (クロスファイル鮮度) を実 rust-analyzer に当てて
    /// 確認済み (tests/conformance.rs の #[ignore] 付き 2 件)。ただし宣言
    /// できるのはテストを当てた版 ([`TESTED_VERSIONS`]) に限る (仕様 8.2 の 5)。
    /// 範囲外の版には状態の通知だけを約束する。
    fn guarantees(&self) -> ServerStateProvider {
        if self.version_is_tested {
            ServerStateProvider::workspace(
                &[("workspace/symbol", self.workspace_symbol_limit)],
                &ALL_FILE_CHANGES,
            )
        } else {
            ServerStateProvider::notifications_only()
        }
    }

    fn learn_initialization_options(&mut self, options: &serde_json::Value) {
        if let Some(limit) = options["workspace"]["symbol"]["search"]["limit"].as_u64() {
            self.workspace_symbol_limit = limit;
        }
    }

    /// Created / Deleted の通知で `indexing` を先読みする (ADR 0014 追補
    /// 決定 D)。監視の対象のファイルにだけ効く。Changed には信号が続かない
    /// (送信中の要求が -32801 で拒まれるだけ) ので先読みしない。
    fn observe_client(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() || view.method() != Some("workspace/didChangeWatchedFiles") {
            return None;
        }
        let changes = parse_watched_file_changes(body)?;
        // FileChangeType: 1 = Created, 2 = Changed, 3 = Deleted。
        let reindexes = changes
            .iter()
            .any(|change| matches!(change.kind, 1 | 3) && is_watched_file(&change.uri));
        reindexes.then_some(ServerState {
            health: self.last_health,
            readiness: Readiness::Indexing,
            message: None,
        })
    }

    /// 上流→クライアント方向のメッセージから、上流が報告している状態を
    /// 読み取る。`experimental/serverStatus` 以外、および読めない status は
    /// `None` (状態を動かさない)。
    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
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

        // 語彙の粗さを補う (設計 5.1)。プロジェクト未発見は横断問い合わせに
        // とって機能不全なので、rust-analyzer の warning を error に写す。
        let mut health: Health = params.health.into();
        if health == Health::Warning
            && params
                .message
                .as_deref()
                .is_some_and(|m| m.contains(MISSING_WORKSPACE_MESSAGE))
        {
            health = Health::Error;
        }

        self.last_health = health;
        Some(ServerState {
            health,
            readiness: if params.quiescent {
                Readiness::Ready
            } else {
                Readiness::Indexing
            },
            message: params.message,
        })
    }
}

struct WatchedFileChange {
    uri: String,
    /// LSP の `FileChangeType` の値 (1..=3)。範囲外の値の変更は捨てる。
    kind: u64,
}

/// `workspace/didChangeWatchedFiles` の `changes` (uri と FileChangeType)。
fn parse_watched_file_changes(body: &[u8]) -> Option<Vec<WatchedFileChange>> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let changes = value["params"]["changes"].as_array()?;
    Some(
        changes
            .iter()
            .filter_map(|change| {
                Some(WatchedFileChange {
                    uri: change["uri"].as_str()?.to_string(),
                    kind: change["type"]
                        .as_u64()
                        .filter(|kind| (1..=3).contains(kind))?,
                })
            })
            .collect(),
    )
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
    #[test]
    fn watched_files_are_recognised_with_either_path_separator() {
        assert!(super::is_watched_file("file:///w/src/c.rs"));
        assert!(super::is_watched_file("file:///C:/w/Cargo.toml"));
        assert!(super::is_watched_file("file:///C:\\w\\Cargo.lock"));
        assert!(!super::is_watched_file("file:///w/notes.txt"));
        assert!(!super::is_watched_file(
            "file:///w/src/rust-analyzer.toml.bak"
        ));
    }

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
    fn declares_guarantees_for_the_nixpkgs_build_the_suite_passed_on() {
        // nixpkgs のビルドは日付で名乗る (`2026-08-03`)。この版にも 7.2 / 7.3 を
        // 当てて通した (flake.nix の開発環境、2026-09-03)。版の識別は semver に
        // 限らず、名乗りの先頭トークンをテスト済みの一覧と突き合わせる。
        let tested = crate::adapter::select("rust-analyzer", Some("2026-08-03")).unwrap();
        assert_eq!(
            tested.guarantees(),
            ServerStateProvider::workspace(&[("workspace/symbol", 128)], &ALL_FILE_CHANGES)
        );
        let untested = crate::adapter::select("rust-analyzer", Some("2026-08-04")).unwrap();
        assert_eq!(
            untested.guarantees(),
            ServerStateProvider::notifications_only()
        );
    }

    #[test]
    fn maps_a_missing_workspace_warning_to_error() {
        // 設計 5.1: プロジェクトが 1 つも見つからないとき rust-analyzer は
        // warning と "Failed to discover workspace." を出す (reload.rs の
        // current_status())。横断問い合わせは機能しないので error に写す。
        // 判別材料は message 文字列しかない。
        let mut adapter = RustAnalyzerAdapter::new();
        let body = r#"{"method":"experimental/serverStatus","params":{"health":"warning","quiescent":true,"message":"Failed to discover workspace.\nConsider adding the `Cargo.toml` of the workspace to the [`linkedProjects`](https://rust-analyzer.github.io/book/configuration.html#linkedProjects) setting.\n\n"}}"#;
        let state = interpret(&mut adapter, body).unwrap();
        assert_eq!(state.health, Health::Error);
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn maps_the_missing_workspace_warning_to_error_even_after_other_warnings() {
        // current_status() は警告文を連結する。先頭でなくても見つける。
        let mut adapter = RustAnalyzerAdapter::new();
        let body = r#"{"method":"experimental/serverStatus","params":{"health":"warning","quiescent":true,"message":"Auto-reloading is disabled and the workspace has changed, a manual workspace reload is required.\n\nFailed to discover workspace.\n"}}"#;
        assert_eq!(interpret(&mut adapter, body).unwrap().health, Health::Error);
    }

    #[test]
    fn keeps_other_warnings_as_warning() {
        let mut adapter = RustAnalyzerAdapter::new();
        let body = r#"{"method":"experimental/serverStatus","params":{"health":"warning","quiescent":true,"message":"Failed to run build scripts of some packages.\n\n"}}"#;
        assert_eq!(
            interpret(&mut adapter, body).unwrap().health,
            Health::Warning
        );
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
        // 仕様 8.1: サーバーは unknown を送出してはならない。dead は本プロトコルの
        // 値ではない (仕様 3 章)。
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
