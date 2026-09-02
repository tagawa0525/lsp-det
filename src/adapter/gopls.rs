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
//! `completeness` / `freshness` は準拠テスト 7.2 / 7.3 を実 gopls に当てて
//! 通すまで宣言しない (設計 5.2)。

use serde::Deserialize;
use serde_json::Value;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{Health, Readiness, ServerState, ServerStateProvider};

const PROGRESS_METHOD: &str = "$/progress";
/// 初期ロード (`general.go` の `addFolders`)。
const WORKSPACE_SETUP_TITLE: &str = "Setting up workspace";
/// ワークスペースのロード失敗 (`diagnostics.go` の `WorkspaceLoadFailure`)。
const WORKSPACE_LOAD_FAILURE_TITLE: &str = "Error loading workspace";
/// フォルダのロード失敗時の end メッセージの先頭 (`general.go`)。
const FAILED_LOAD_PREFIX: &str = "Error loading packages";

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
    state: ServerState,
    /// begin を見て end を待っている "Setting up workspace" のトークン。
    loading: Vec<Value>,
    /// begin 中の "Error loading workspace" のトークン。
    failure: Option<Value>,
}

impl Default for GoplsAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl GoplsAdapter {
    pub fn new() -> Self {
        GoplsAdapter {
            state: ServerState::initializing(),
            loading: Vec::new(),
            failure: None,
        }
    }

    fn on_progress(&mut self, params: ProgressParams) -> Option<ServerState> {
        let ProgressParams { token, value } = params;
        match value.kind.as_str() {
            "begin" => match value.title.as_deref() {
                Some(WORKSPACE_SETUP_TITLE) => {
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
                    self.state.health = Health::Error;
                    self.state.message = value.message;
                }
                if self.loading.is_empty() {
                    // 全フォルダのロードが終わって初めて ready。health も
                    // ここで初めて ok を名乗れる (途中で ok は観測なしの主張)。
                    self.state.readiness = Readiness::Ready;
                    if !failed && self.failure.is_none() && self.state.health != Health::Error {
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

    fn guarantees(&self) -> ServerStateProvider {
        ServerStateProvider::Basic(true)
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

    #[test]
    fn gopls_declares_no_guarantees_until_measured() {
        // 設計 5.2: 7.2 / 7.3 を実 gopls に当てるまで宣言しない。
        assert_eq!(
            GoplsAdapter::new().guarantees(),
            ServerStateProvider::Basic(true)
        );
    }
}
