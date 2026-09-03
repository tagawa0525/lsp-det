//! pyright / basedpyright の写像 (ADR 0011、設計 5.3)。
//!
//! pyright は readiness の語彙を持たず、`InitializeResult.serverInfo` も
//! 返さない (basedpyright は返す)。どちらも `window/logMessage` から読む
//! (pyright のソース `languageServerBase.ts`・`sourceEnumerator.ts`・
//! `referencesProvider.ts` と実測 research/pyright-readiness-measurement.md):
//!
//! - **名乗り**: 起動直後にコンストラクタが info で
//!   "Pyright language server 1.1.412 starting" (basedpyright は
//!   "basedpyright language server 1.39.8 starting") を送る。設定の読み込み前
//!   なので抑制されず、`initialize` 応答より先に届く。[`startup_identity`]
//! - **readiness**: 横断リクエストは追跡中のファイル一覧を走査し、その一覧は
//!   タイマーで少しずつ列挙される。列挙の完了は info の "Found N source
//!   files" または "No source files found."。ワークスペースフォルダごとに
//!   "Starting service instance \"name\"" が出て列挙もフォルダごとなので、
//!   その数だけ完了を数えてから `ready`。再列挙の開始 "Searching for source
//!   files" (log レベル。既定では届かない) が来たら `indexing` に戻す
//! - **health**: 信号がない。`unknown` のまま (仕様 8.2 の 2)。クラッシュは
//!   接続の終了で伝わる
//! - `$/progress` は開いたファイルの解析の進行であり、横断リクエストの完全性
//!   とは別の事柄なので読まない (ADR 0011 決定 B-4)
//!
//! `completeness` / `freshness` は準拠テスト 7.2 / 7.3 を実 pyright に当てて
//! 通した版 ([`TESTED_VERSIONS`]) にだけ宣言する (ADR 0009 決定 D-5)。

use serde::Deserialize;

use super::Mapping;
use crate::initialize::ServerInfo;
use crate::peek::MessageView;
use crate::state::{Readiness, ServerState, ServerStateProvider};

const LOG_MESSAGE_METHOD: &str = "window/logMessage";
/// 起動ログの productName と版の間の定型句 (`languageServerBase.ts` のコンストラクタ)。
const STARTUP_INFIX: &str = " language server ";
const STARTUP_SUFFIX: &str = " starting";
/// ワークスペースフォルダごとの `AnalyzerService` の開始 (`languageServerBase.ts`)。
const SERVICE_STARTED_PREFIX: &str = "Starting service instance ";
/// ファイル列挙の完了 (`sourceEnumerator.ts` の `_finish()`)。
const ENUMERATION_FOUND_PREFIX: &str = "Found ";
const ENUMERATION_FOUND_SUFFIX_ONE: &str = " source file";
const ENUMERATION_FOUND_SUFFIX_MANY: &str = " source files";
const ENUMERATION_EMPTY: &str = "No source files found.";
/// 再列挙の開始 (`sourceEnumerator.ts` のコンストラクタ、log レベル)。
const ENUMERATION_STARTED: &str = "Searching for source files";

/// 準拠テスト 7.2 / 7.3 を実 pyright / basedpyright に当てて通した版。
/// 名乗り (`serverInfo.version` または起動ログの版) と完全一致で突き合わせる。
///
/// 一覧にない版には保証を宣言しない。足すときは、その版で
/// `cargo test --test conformance -- --ignored pyright_` を通してから
/// (守れない保証の宣言は仕様 5.1 違反)。
pub const TESTED_VERSIONS: &[&str] = &[];

/// 起動ログの名乗りを読む。
///
/// pyright 系は `${productName} language server ${version} starting` を
/// 送る。productName は "Pyright" または "basedpyright"。写像の鍵は
/// `serverInfo.name` と同じ小文字の "pyright" / "basedpyright" に揃える
/// (basedpyright は `serverInfo.name` に "basedpyright" を名乗る)。
/// 版は省かれることがあり、そのときは `None`。他の文言には `None`。
pub fn startup_identity(message: &str) -> Option<ServerInfo> {
    let (product, rest) = message.split_once(STARTUP_INFIX)?;
    let name = match product {
        "Pyright" | "pyright" => "pyright",
        "basedpyright" => "basedpyright",
        _ => return None,
    };
    // 版は `serverOptions.version && serverOptions.version + ' '` なので
    // 省かれうる。そのとき rest は "starting" だけ。
    let version = match rest {
        r if r == STARTUP_SUFFIX.trim_start() => None,
        r => {
            let v = r.strip_suffix(STARTUP_SUFFIX)?;
            if v.is_empty() || v.contains(' ') {
                return None;
            }
            Some(v.to_string())
        }
    };
    Some(ServerInfo {
        name: name.to_string(),
        version,
    })
}

/// pyright / basedpyright の写像。
pub struct PyrightAdapter {
    /// 名乗った版が [`TESTED_VERSIONS`] に入っているか。保証を宣言する条件。
    version_is_tested: bool,
    state: ServerState,
    /// "Starting service instance" を見た数 (= 列挙を待つフォルダの数)。
    instances: usize,
    /// 列挙の完了ログを見た数。`instances` に追いついたら `ready`。
    completed: usize,
}

impl Default for PyrightAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl PyrightAdapter {
    /// 版を名乗らない pyright 向け。保証は宣言しない。
    pub fn new() -> Self {
        Self::for_version(None)
    }

    /// 名乗った版を見て、テスト済みの版なら保証を宣言する。
    pub fn for_version(version: Option<&str>) -> Self {
        let version_is_tested = version.is_some_and(|v| TESTED_VERSIONS.contains(&v.trim()));
        PyrightAdapter {
            version_is_tested,
            state: ServerState::initializing(),
            instances: 0,
            completed: 0,
        }
    }

    fn on_log(&mut self, message: &str) -> Option<ServerState> {
        if message.starts_with(SERVICE_STARTED_PREFIX) {
            // 新しいフォルダの列挙が始まる。ready だったなら indexing に戻す
            // (didChangeWorkspaceFolders)。initializing のときはそのまま。
            self.instances += 1;
            if self.state.readiness == Readiness::Ready {
                self.state.readiness = Readiness::Indexing;
            }
        } else if message == ENUMERATION_STARTED {
            // 再列挙 (log レベル。既定の logLevel では届かない)。
            if self.instances == 0 {
                return None;
            }
            self.completed = self.completed.saturating_sub(1);
            self.state.readiness = Readiness::Indexing;
        } else if is_enumeration_complete(message) {
            if self.completed >= self.instances {
                // 数える相手がいない完了。ready を名乗る根拠にしない。
                return None;
            }
            self.completed += 1;
            if self.completed == self.instances {
                self.state.readiness = Readiness::Ready;
            }
        } else {
            return None;
        }
        Some(self.state.clone())
    }
}

/// "Found N source file(s)" または "No source files found."。
fn is_enumeration_complete(message: &str) -> bool {
    if message == ENUMERATION_EMPTY {
        return true;
    }
    let Some(rest) = message.strip_prefix(ENUMERATION_FOUND_PREFIX) else {
        return false;
    };
    let count = rest
        .strip_suffix(ENUMERATION_FOUND_SUFFIX_MANY)
        .or_else(|| rest.strip_suffix(ENUMERATION_FOUND_SUFFIX_ONE));
    count.is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

#[derive(Deserialize)]
struct LogMessageParams {
    message: String,
}

impl Mapping for PyrightAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    fn guarantees(&self) -> ServerStateProvider {
        if self.version_is_tested {
            ServerStateProvider::complete_and_fresh()
        } else {
            ServerStateProvider::Basic(true)
        }
    }

    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() || view.method() != Some(LOG_MESSAGE_METHOD) {
            return None;
        }
        #[derive(Deserialize)]
        struct Envelope {
            params: LogMessageParams,
        }
        let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
        self.on_log(&envelope.params.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;
    use crate::state::{Health, Readiness};

    fn log(kind: u8, message: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"window/logMessage","params":{{"type":{kind},"message":"{message}"}}}}"#
        )
    }

    fn info(message: &str) -> String {
        log(3, message)
    }

    fn interpret(adapter: &mut PyrightAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    fn started(adapter: &mut PyrightAdapter, folder: &str) -> Option<ServerState> {
        interpret(
            adapter,
            &info(&format!("Starting service instance \\\"{folder}\\\"")),
        )
    }

    // --- 名乗り ----------------------------------------------------------------

    #[test]
    fn reads_the_name_and_version_out_of_the_startup_log() {
        // 実測 (research/pyright-readiness-measurement.md) の文言そのもの。
        let pyright = startup_identity("Pyright language server 1.1.412 starting")
            .expect("pyright の起動ログは名乗り");
        assert_eq!(pyright.name, "pyright");
        assert_eq!(pyright.version.as_deref(), Some("1.1.412"));

        let based = startup_identity("basedpyright language server 1.39.8 starting")
            .expect("basedpyright の起動ログは名乗り");
        assert_eq!(based.name, "basedpyright");
        assert_eq!(based.version.as_deref(), Some("1.39.8"));
    }

    #[test]
    fn the_startup_log_may_omit_the_version() {
        // `serverOptions.version && serverOptions.version + ' '` なので版は省かれうる。
        let identity =
            startup_identity("Pyright language server starting").expect("版なしでも名乗り");
        assert_eq!(identity.name, "pyright");
        assert_eq!(identity.version, None);
    }

    #[test]
    fn other_log_lines_are_not_identities() {
        for other in [
            "Server root directory: file:///nix/store/x/dist",
            "Starting service instance \"pyfix\"",
            "Found 2 source files",
            "rust-analyzer 1.98.0 starting",
            "language server starting",
            "",
        ] {
            assert!(
                startup_identity(other).is_none(),
                "名乗りでない行: {other:?}"
            );
        }
    }

    // --- readiness -------------------------------------------------------------

    #[test]
    fn starts_initializing_with_unknown_health() {
        let adapter = PyrightAdapter::new();
        let state = adapter.initial_state();
        assert_eq!(state.readiness, Readiness::Initializing);
        assert_eq!(state.health, Health::Unknown);
    }

    #[test]
    fn enumeration_of_the_only_folder_means_ready() {
        let mut adapter = PyrightAdapter::new();
        started(&mut adapter, "pyfix");
        let state = interpret(&mut adapter, &info("Found 2 source files")).expect("完了は信号");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(
            state.health,
            Health::Unknown,
            "列挙の完了は health の観測ではない"
        );
    }

    #[test]
    fn no_source_files_is_also_a_completion() {
        let mut adapter = PyrightAdapter::new();
        started(&mut adapter, "empty");
        let state = interpret(&mut adapter, &info("No source files found.")).expect("完了は信号");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn waits_for_every_workspace_folder() {
        // フォルダごとに "Starting service instance" と完了ログが 1 回ずつ出る。
        let mut adapter = PyrightAdapter::new();
        started(&mut adapter, "one");
        started(&mut adapter, "two");
        let after_one = interpret(&mut adapter, &info("Found 400 source files"));
        assert!(
            after_one
                .as_ref()
                .is_none_or(|s| s.readiness != Readiness::Ready),
            "1 フォルダの完了で ready を名乗った: {after_one:?}"
        );
        let after_two =
            interpret(&mut adapter, &info("Found 1200 source files")).expect("最後の完了は信号");
        assert_eq!(after_two.readiness, Readiness::Ready);
    }

    #[test]
    fn a_folder_added_after_ready_rearms() {
        // didChangeWorkspaceFolders で新しい service instance が始まる。
        let mut adapter = PyrightAdapter::new();
        started(&mut adapter, "one");
        interpret(&mut adapter, &info("Found 1 source file"));
        let state = started(&mut adapter, "two").expect("新しいフォルダの開始は信号");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state = interpret(&mut adapter, &info("Found 3 source files")).expect("完了は信号");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn reenumeration_start_rearms_when_it_is_visible() {
        // "Searching for source files" は log レベルで既定では届かないが、
        // 届いたときは再列挙の開始として indexing に戻す。
        let mut adapter = PyrightAdapter::new();
        started(&mut adapter, "one");
        interpret(&mut adapter, &info("Found 1 source file"));
        let state = interpret(&mut adapter, &log(4, "Searching for source files"))
            .expect("再列挙の開始は信号");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state = interpret(&mut adapter, &info("Found 2 source files")).expect("完了は信号");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn a_completion_without_a_started_instance_is_ignored() {
        // 数える相手がいない完了ログで ready にしない。
        let mut adapter = PyrightAdapter::new();
        assert!(interpret(&mut adapter, &info("Found 2 source files")).is_none());
    }

    #[test]
    fn ignores_other_logs_progress_and_other_vocabularies() {
        let mut adapter = PyrightAdapter::new();
        started(&mut adapter, "one");
        for other in [
            info("Pyright language server 1.1.412 starting"),
            info("Server root directory: file:///x"),
            info("Assuming Python version 3.14.7.final.0"),
            info("Auto-excluding **/node_modules"),
            log(1, "some error"),
            r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":"t","value":{"kind":"begin","title":"Finding references"}}}"#.to_string(),
            r#"{"jsonrpc":"2.0","method":"$/progress","params":{"token":"t","value":{"kind":"end"}}}"#.to_string(),
            r#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"ok","quiescent":true,"message":null}}"#.to_string(),
            r#"{"jsonrpc":"2.0","method":"window/showMessage","params":{"type":3,"message":"Found 2 source files"}}"#.to_string(),
        ] {
            assert!(
                interpret(&mut adapter, &other).is_none(),
                "無関係なメッセージで状態が動いた: {other}"
            );
        }
    }

    #[test]
    fn health_never_leaves_unknown() {
        let mut adapter = PyrightAdapter::new();
        started(&mut adapter, "one");
        let state = interpret(&mut adapter, &info("Found 1 source file")).unwrap();
        assert_eq!(state.health, Health::Unknown);
        assert_eq!(state.message, None);
    }

    // --- 保証 ------------------------------------------------------------------

    #[test]
    fn declares_no_guarantees_until_the_conformance_suite_passed_on_a_version() {
        // 仕様 8.2 の 5。まだどの版にも 7.2 / 7.3 を当てていない。
        for version in [Some("1.1.412"), Some("1.39.8"), None] {
            assert_eq!(
                PyrightAdapter::for_version(version).guarantees(),
                ServerStateProvider::Basic(true),
                "測っていない版 {version:?} に保証を宣言した"
            );
        }
    }
}
