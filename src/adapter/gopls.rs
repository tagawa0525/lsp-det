//! gopls の写像 (v0.1-design.md 5.2)。
//!
//! gopls は readiness の語彙を持たない。`$/progress` から合成する
//! (gopls のソース `server/general.go`・`server/diagnostics.go`・
//! `progress/progress.go` で確認):
//!
//! - **readiness**: 初期化時にワークスペースフォルダごとに title
//!   "Setting up workspace" の progress が begin し、`AwaitInitialized` の後に
//!   "Finished loading packages." で end する (ロード失敗なら "Error loading
//!   packages: ..." で end)。トークンはランダムなので title で識別し、
//!   begin で覚えたトークンがすべて end したら `ready`。同じ title は
//!   `didChangeWorkspaceFolders` でフォルダが足されたときにも出るので、
//!   begin を見たら `indexing` に戻す (再武装)。go.mod の変更では出ない
//! - **health**: 最初のロードが "Finished loading packages." で終わったら
//!   `ok`、"Error loading packages:" で終わったら `error`。その後は title
//!   "Error loading workspace" (`WorkspaceLoadFailure`) の progress が
//!   begin したら `error` (message 付き)、report で message を更新、
//!   "Done." で end したら `ok` に戻る
//!
//! `coverage` / `freshness` は準拠テスト 7.2 / 7.3 を実 gopls に当てて
//! 通した版 ([`TESTED_VERSIONS`]) にだけ宣言する (設計 5.2、仕様 8.2 の 5)。

use serde::Deserialize;
use serde_json::Value;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{ALL_FILE_CHANGES, Health, Readiness, ServerState, ServerStateProvider};

const PROGRESS_METHOD: &str = "$/progress";
/// 初期ロード (`general.go` の `addFolders`)。
const WORKSPACE_SETUP_TITLE: &str = "Setting up workspace";
/// ワークスペースのロード失敗 (`diagnostics.go` の `WorkspaceLoadFailure`)。
const WORKSPACE_LOAD_FAILURE_TITLE: &str = "Error loading workspace";
/// フォルダのロード失敗時の end メッセージの先頭 (`general.go`)。
const FAILED_LOAD_PREFIX: &str = "Error loading packages";

/// 準拠テスト 7.2 / 7.3 を実 gopls に当てて通した版。[`gopls_version`] で
/// 正規化した名乗り (`v` を除いた `X.Y.Z`) と完全一致で突き合わせる。
///
/// 一覧にない版には保証を宣言しない。足すときは、その版で
/// `cargo test --test conformance -- --ignored gopls_` を通してから
/// (守れない保証の宣言は仕様 5.1 違反)。
///
/// 通した記録: v0.23.0 (nixpkgs、go1.26.7)、2026-09-03、5 回連続。
pub const TESTED_VERSIONS: &[&str] = &["0.23.0"];

/// gopls の `serverInfo.version` から版 (`X.Y.Z`) を取り出す。
///
/// gopls はビルド情報 (`debug.BuildInfo`) を JSON にした文字列を名乗る。
/// 版はその最上位の `"Version"` (`v0.23.0`)。`Main.Version` は nix ビルド
/// では `(devel)` なので使えない。JSON でなければ文字列そのものを版とみなす。
/// 前後の空白と先頭の `v` を落とし、`X.Y.Z` の形だけを受理する
/// (`(devel)` 等は `None`)。
pub fn gopls_version(version: &str) -> Option<String> {
    let raw = match serde_json::from_str::<Value>(version) {
        Ok(Value::Object(info)) => info.get("Version")?.as_str()?.to_string(),
        _ => version.to_string(),
    };
    let trimmed = raw.trim();
    let stripped = trimmed.strip_prefix('v').unwrap_or(trimmed);
    let mut parts = stripped.split('.');
    let is_semver = parts
        .by_ref()
        .take(3)
        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && stripped.matches('.').count() == 2;
    is_semver.then(|| stripped.to_string())
}

#[derive(Debug, Deserialize)]
struct ProgressParams {
    token: Value,
    value: ProgressValue,
}

#[derive(Debug, Deserialize)]
struct ProgressValue {
    kind: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

pub struct GoplsAdapter {
    /// 名乗った版が [`TESTED_VERSIONS`] に入っているか。保証を宣言する条件。
    version_is_tested: bool,
    state: ServerState,
    /// begin を見て end を待っている "Setting up workspace" のトークン。
    loading: Vec<Value>,
    /// 今回のロード (最後に `loading` が空から増えてから) で失敗した
    /// フォルダの end メッセージ。1 つでもあれば今回の結果は信頼できない。
    failed_in_round: Option<String>,
    /// begin 中の "Error loading workspace" のトークン。
    failure: Option<Value>,
}

impl Default for GoplsAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl GoplsAdapter {
    /// 版を名乗らない (または読めない) gopls 向け。保証は宣言しない。
    pub fn new() -> Self {
        Self::for_version(None)
    }

    /// `serverInfo.version` を見て、テスト済みの版なら保証を宣言する。
    pub fn for_version(version: Option<&str>) -> Self {
        let version_is_tested = version
            .and_then(gopls_version)
            .is_some_and(|v| TESTED_VERSIONS.contains(&v.as_str()));
        GoplsAdapter {
            version_is_tested,
            state: ServerState::initializing(),
            loading: Vec::new(),
            failed_in_round: None,
            failure: None,
        }
    }

    fn on_progress(&mut self, params: ProgressParams) -> Option<ServerState> {
        let ProgressParams { token, value } = params;
        match value.kind.as_str() {
            "begin" => match value.title.as_deref() {
                Some(WORKSPACE_SETUP_TITLE) => {
                    if self.loading.is_empty() {
                        // 新しいロードの回。前回の失敗は持ち越さない。
                        self.failed_in_round = None;
                    }
                    self.loading.push(token);
                    self.state.readiness = Readiness::Indexing;
                }
                Some(WORKSPACE_LOAD_FAILURE_TITLE) => {
                    self.failure = Some(token);
                    self.state.health = Health::Error;
                    self.state.message = value.message;
                }
                _ => return None,
            },
            "report" => {
                if self.failure.as_ref() != Some(&token) {
                    return None;
                }
                if value.message.is_some() {
                    self.state.message = value.message;
                }
            }
            "end" if self.failure.as_ref() == Some(&token) => {
                self.failure = None;
                self.state.health = Health::Ok;
                self.state.message = None;
            }
            "end" => {
                let index = self.loading.iter().position(|t| *t == token)?;
                self.loading.remove(index);
                let failed = value
                    .message
                    .as_deref()
                    .is_some_and(|m| m.starts_with(FAILED_LOAD_PREFIX));
                if failed {
                    // 試行は終わったが結果は信頼できない (仕様 6 章 5 項)。
                    self.failed_in_round = value.message;
                }
                if self.loading.is_empty() {
                    // 全フォルダのロードが終わって初めて ready。health も
                    // ここで初めて決まる (途中で ok は観測なしの主張)。
                    // 今回の回で 1 つでも失敗していれば error、全部成功なら
                    // 観測できた成功に基づいて ok (前回の失敗は持ち越さない)。
                    // ただし "Error loading workspace" が begin 中なら error のまま。
                    self.state.readiness = Readiness::Ready;
                    if let Some(message) = &self.failed_in_round {
                        self.state.health = Health::Error;
                        self.state.message = Some(message.clone());
                    } else if self.failure.is_none() {
                        self.state.health = Health::Ok;
                        self.state.message = None;
                    }
                }
            }
            _ => return None,
        }
        Some(self.state.clone())
    }
}

impl Mapping for GoplsAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// gopls はリクエストごとにスナップショットを取り、`didChange` の
    /// オーバーレイを織り込む。準拠テスト 7.2 (完全性) と 7.3 (クロスファイル
    /// 鮮度) を実 gopls に当てて確認済み (tests/conformance.rs の gopls_*
    /// ignored)。宣言できるのはテストを当てた版だけ。
    fn guarantees(&self) -> ServerStateProvider {
        if self.version_is_tested {
            ServerStateProvider::workspace(&[("workspace/symbol", 100)], &ALL_FILE_CHANGES)
        } else {
            ServerStateProvider::notifications_only()
        }
    }

    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() || view.method() != Some(PROGRESS_METHOD) {
            return None;
        }
        #[derive(Deserialize)]
        struct Envelope {
            params: ProgressParams,
        }
        let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
        self.on_progress(envelope.params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;

    fn status(health: &str, quiescent: bool) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{{"health":"{health}","quiescent":{quiescent},"message":null}}}}"#
        )
    }

    // --- gopls ---------------------------------------------------------------

    fn progress(token: &str, value: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"$/progress","params":{{"token":"{token}","value":{value}}}}}"#
        )
    }

    fn setup_begin(token: &str) -> String {
        progress(
            token,
            r#"{"kind":"begin","title":"Setting up workspace","message":"Loading packages...","cancellable":false}"#,
        )
    }

    fn setup_end(token: &str, message: &str) -> String {
        progress(token, &format!(r#"{{"kind":"end","message":"{message}"}}"#))
    }

    fn gopls_interpret(adapter: &mut GoplsAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    #[test]
    fn gopls_begin_of_workspace_setup_means_indexing() {
        let mut adapter = GoplsAdapter::new();
        let state = gopls_interpret(&mut adapter, &setup_begin("1")).expect("begin is a signal");
        assert_eq!(state.readiness, Readiness::Indexing);
        assert_eq!(state.health, Health::Unknown, "begin は health を語らない");
    }

    #[test]
    fn gopls_end_of_workspace_setup_means_ready_and_ok() {
        let mut adapter = GoplsAdapter::new();
        gopls_interpret(&mut adapter, &setup_begin("1"));
        let state = gopls_interpret(&mut adapter, &setup_end("1", "Finished loading packages."))
            .expect("end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(state.health, Health::Ok);
    }

    #[test]
    fn gopls_waits_for_all_tokens() {
        let mut adapter = GoplsAdapter::new();
        gopls_interpret(&mut adapter, &setup_begin("a"));
        gopls_interpret(&mut adapter, &setup_begin("b"));
        let after_a = gopls_interpret(&mut adapter, &setup_end("a", "Finished loading packages."));
        assert!(
            after_a
                .as_ref()
                .is_none_or(|s| s.readiness == Readiness::Indexing),
            "1 つ目の end で ready を名乗った: {after_a:?}"
        );
        let after_b = gopls_interpret(&mut adapter, &setup_end("b", "Finished loading packages."))
            .expect("last end is a signal");
        assert_eq!(after_b.readiness, Readiness::Ready);
    }

    #[test]
    fn gopls_successful_reload_after_a_failed_load_restores_ok() {
        // フォルダ追加などで再ロードが成功したら、観測できた成功に基づいて
        // health を ok に戻す (Copilot の指摘)。
        let mut adapter = GoplsAdapter::new();
        gopls_interpret(&mut adapter, &setup_begin("1"));
        let failed = gopls_interpret(&mut adapter, &setup_end("1", "Error loading packages: x"))
            .expect("end is a signal");
        assert_eq!(failed.health, Health::Error);

        gopls_interpret(&mut adapter, &setup_begin("2"));
        let state = gopls_interpret(&mut adapter, &setup_end("2", "Finished loading packages."))
            .expect("end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(state.health, Health::Ok, "再ロードの成功で ok に戻る");
        assert_eq!(state.message, None);
    }

    #[test]
    fn gopls_a_round_with_one_failed_folder_stays_error() {
        // 同じロードの中で 1 フォルダでも失敗していれば、後のフォルダが
        // 成功しても error のまま (結果は信頼できない)。
        let mut adapter = GoplsAdapter::new();
        gopls_interpret(&mut adapter, &setup_begin("a"));
        gopls_interpret(&mut adapter, &setup_begin("b"));
        gopls_interpret(&mut adapter, &setup_end("a", "Error loading packages: x"));
        let state = gopls_interpret(&mut adapter, &setup_end("b", "Finished loading packages."))
            .expect("last end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(state.health, Health::Error);
        assert_eq!(state.message.as_deref(), Some("Error loading packages: x"));
    }

    #[test]
    fn gopls_end_of_an_unknown_token_is_ignored() {
        // トークンは begin で覚えたものだけ。他の progress の end で ready にしない。
        let mut adapter = GoplsAdapter::new();
        assert!(
            gopls_interpret(
                &mut adapter,
                &setup_end("stray", "Finished loading packages.")
            )
            .is_none()
        );
    }

    #[test]
    fn gopls_failed_load_is_ready_but_error() {
        let mut adapter = GoplsAdapter::new();
        gopls_interpret(&mut adapter, &setup_begin("1"));
        let state = gopls_interpret(
            &mut adapter,
            &setup_end("1", "Error loading packages: no Go files"),
        )
        .expect("end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(state.health, Health::Error);
        assert_eq!(
            state.message.as_deref(),
            Some("Error loading packages: no Go files")
        );
    }

    #[test]
    fn gopls_workspace_load_failure_progress_drives_health() {
        let mut adapter = GoplsAdapter::new();
        gopls_interpret(&mut adapter, &setup_begin("1"));
        gopls_interpret(&mut adapter, &setup_end("1", "Finished loading packages."));

        let begin = progress(
            "e",
            r#"{"kind":"begin","title":"Error loading workspace","message":"err: go.mod file not found","cancellable":false}"#,
        );
        let state = gopls_interpret(&mut adapter, &begin).expect("failure begin is a signal");
        assert_eq!(state.health, Health::Error);
        assert_eq!(state.message.as_deref(), Some("err: go.mod file not found"));
        assert_eq!(state.readiness, Readiness::Ready, "readiness は変えない");

        let report = progress("e", r#"{"kind":"report","message":"err: still broken"}"#);
        let state = gopls_interpret(&mut adapter, &report).expect("report updates the message");
        assert_eq!(state.health, Health::Error);
        assert_eq!(state.message.as_deref(), Some("err: still broken"));

        let end = progress("e", r#"{"kind":"end","message":"Done."}"#);
        let state = gopls_interpret(&mut adapter, &end).expect("failure end is a signal");
        assert_eq!(state.health, Health::Ok);
    }

    #[test]
    fn gopls_ignores_other_progress_titles_and_other_methods() {
        let mut adapter = GoplsAdapter::new();
        let diag = progress(
            "d",
            r#"{"kind":"begin","title":"Calculating diagnostics","message":"..."}"#,
        );
        assert!(gopls_interpret(&mut adapter, &diag).is_none());
        assert!(
            gopls_interpret(
                &mut adapter,
                &progress("d", r#"{"kind":"end","message":"Done."}"#)
            )
            .is_none()
        );
        assert!(
            gopls_interpret(&mut adapter, &status("ok", true)).is_none(),
            "rust-analyzer の語彙は読まない"
        );
    }

    /// nix ビルドの gopls v0.23.0 が名乗った文字列 (2026-09-03 実測)。
    const GOPLS_VERSION_JSON: &str = r#"{"GoVersion":"go1.27.0","Path":"golang.org/x/tools/gopls","Main":{"Path":"golang.org/x/tools/gopls","Version":"(devel)"},"Deps":[{"Path":"golang.org/x/tools","Version":"v0.47.1-0.20260707181000-a299dadba899"}],"Settings":[{"Key":"GOOS","Value":"linux"}],"Version":"v0.23.0"}"#;

    #[test]
    fn gopls_reads_the_version_out_of_the_build_info_json() {
        // serverInfo.version はビルド情報の JSON。最上位の "Version" が版で、
        // Main.Version は nix ビルドでは "(devel)"。
        assert_eq!(gopls_version(GOPLS_VERSION_JSON).as_deref(), Some("0.23.0"));
        assert_eq!(gopls_version("v0.23.0").as_deref(), Some("0.23.0"));
        assert_eq!(gopls_version("0.23.0").as_deref(), Some("0.23.0"));
        assert_eq!(gopls_version(" v0.23.0 ").as_deref(), Some("0.23.0"));
        assert_eq!(gopls_version("(devel)"), None, "X.Y.Z 以外は受理しない");
        assert_eq!(gopls_version("0.23"), None);
        assert_eq!(gopls_version(""), None);
    }

    #[test]
    fn gopls_declares_guarantees_only_for_versions_the_conformance_suite_passed_on() {
        // 仕様 8.2 の 5。7.2 / 7.3 を実 gopls v0.23.0 に当てて通した
        // (tests/conformance.rs の gopls_* ignored)。それ以外には宣言しない。
        assert_eq!(
            GoplsAdapter::for_version(Some(GOPLS_VERSION_JSON)).guarantees(),
            ServerStateProvider::workspace(&[("workspace/symbol", 100)], &ALL_FILE_CHANGES)
        );
        for untested in [
            Some("v0.22.0"),
            Some("(devel)"),
            Some("1.98.0 (fake)"),
            None,
        ] {
            assert_eq!(
                GoplsAdapter::for_version(untested).guarantees(),
                ServerStateProvider::notifications_only(),
                "テストを当てていない版 {untested:?} に保証を宣言した"
            );
        }
    }
}
