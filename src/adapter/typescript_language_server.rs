//! typescript-language-server の写像 (ADR 0010 決定 B の M6、設計 5.3)。
//!
//! typescript-language-server は readiness の語彙を持たず、
//! `InitializeResult.serverInfo` も返さない。信号は次のとおり
//! (ソース `src/ts-client.ts`・`src/lsp-server.ts` と実測
//! research/typescript-language-server-readiness-measurement.md):
//!
//! - **名乗り**: `initialize` 応答の直後に独自通知 `$/typescriptVersion`
//!   `{version, source}` を送る。この通知は typescript-language-server 固有
//!   なので写像の選択に使う ([`identity_from_typescript_version`])。版は
//!   tsserver (TypeScript) の版で、typescript-language-server 自身の版は
//!   ワイヤに出ない
//! - **readiness**: tsserver の `projectLoadingStart` で title
//!   "Initializing JS/TS language features…" の `$/progress` が begin し、
//!   `projectLoadingFinish` 等で end する。プロジェクトはファイルを開いたとき
//!   に逐次ロードされ、新しい begin は前の progress を end してから始まる。
//!   begin で覚えたトークンがすべて end したら `ready`。tsconfig の変更でも
//!   再発行される
//! - **health**: 最初の end で `ok` (ロードの成功を観測した)。tsserver が
//!   落ちると `window/logMessage` (error) に "[tsserver] Exited. Code: N.
//!   Signal: S" が出る。言語サーバー自身は生き残って空配列を成功として返す
//!   ので、このログで `error` にする。再起動はないので戻らない
//!
//! `completeness` / `freshness` は準拠テスト 7.2 / 7.3 を実サーバーに当てて
//! 通した版 ([`TESTED_VERSIONS`]) にだけ宣言する (ADR 0009 決定 D-5)。

use serde::Deserialize;
use serde_json::Value;

use super::Mapping;
use crate::initialize::ServerInfo;
use crate::peek::MessageView;
use crate::state::{Health, Readiness, ServerState, ServerStateProvider};

const PROGRESS_METHOD: &str = "$/progress";
const LOG_MESSAGE_METHOD: &str = "window/logMessage";
/// プロジェクトのロード (`ts-client.ts` の `ServerInitializingIndicator`)。
const PROJECT_LOAD_TITLE: &str = "Initializing JS/TS language features…";
/// tsserver の終了 (`ts-client.ts` の `onExit`)。前に "[lspserver] [tsclient] "
/// のタグが付く。
const TSSERVER_EXITED: &str = "[tsserver] Exited. Code:";
/// 起動ログの定型句 (`lsp-server.ts` の `initialize`)。
const STARTUP_PREFIX: &str = "Using Typescript version (";
const STARTUP_INFIX: &str = ") ";
const STARTUP_SUFFIX_START: &str = " from path ";

/// `serverInfo` の代わりに名乗りとして使う名前。
pub const SERVER_NAME: &str = "typescript-language-server";

/// 準拠テスト 7.2 / 7.3 を通した **TypeScript (tsserver) の版**。
///
/// typescript-language-server 自身の版はワイヤに出ないので、名乗り
/// (`$/typescriptVersion` の `version`) で突き合わせられるのはこちらだけ。
/// 一覧にない版には保証を宣言しない。足すときは、その版で
/// `cargo test --test conformance -- --ignored typescript_language_server_`
/// を通してから (守れない保証の宣言は仕様 5.1 違反)。
pub const TESTED_VERSIONS: &[&str] = &[];

/// `initialize` 応答より先に届く `window/logMessage` (info) の名乗りを読む。
///
/// `lsp-server.ts` の `initialize` は応答を返す前に
/// `Using Typescript version (${source}) ${version} from path "${path}"` を
/// info に出す。`$/typescriptVersion` は応答の後に届くので、`InitializeResult`
/// に保証を宣言するにはこちらで先に選ぶ必要がある。文言は
/// typescript-language-server 固有。他の文言には `None`。
pub fn startup_identity(message: &str) -> Option<ServerInfo> {
    let rest = message.strip_prefix(STARTUP_PREFIX)?;
    let (_source, rest) = rest.split_once(STARTUP_INFIX)?;
    let (version, _path) = rest.split_once(STARTUP_SUFFIX_START)?;
    let version = version.trim();
    if version.is_empty() || version.contains(' ') {
        return None;
    }
    Some(ServerInfo {
        name: SERVER_NAME.to_string(),
        version: Some(version.to_string()),
    })
}

/// `$/typescriptVersion` の params から名乗りを読む。
pub fn identity_from_typescript_version(params: &Value) -> Option<ServerInfo> {
    let params = params.as_object()?;
    let version = params
        .get("version")
        .and_then(Value::as_str)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    Some(ServerInfo {
        name: SERVER_NAME.to_string(),
        version,
    })
}

#[derive(Deserialize)]
struct ProgressParams {
    token: Value,
    value: ProgressValue,
}

#[derive(Deserialize)]
struct ProgressValue {
    kind: String,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize)]
struct LogMessageParams {
    message: String,
}

/// typescript-language-server の写像。
pub struct TypescriptLanguageServerAdapter {
    /// 名乗った版が [`TESTED_VERSIONS`] に入っているか。保証を宣言する条件。
    version_is_tested: bool,
    state: ServerState,
    /// begin を見て end を待っているプロジェクトロードのトークン。
    loading: Vec<Value>,
}

impl Default for TypescriptLanguageServerAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl TypescriptLanguageServerAdapter {
    /// 版を名乗らない上流向け。保証は宣言しない。
    pub fn new() -> Self {
        Self::for_version(None)
    }

    /// 名乗った版 (TypeScript の版) を見て、テスト済みなら保証を宣言する。
    pub fn for_version(version: Option<&str>) -> Self {
        let version_is_tested = version.is_some_and(|v| TESTED_VERSIONS.contains(&v.trim()));
        TypescriptLanguageServerAdapter {
            version_is_tested,
            state: ServerState::initializing(),
            loading: Vec::new(),
        }
    }

    fn on_progress(&mut self, params: ProgressParams) -> Option<ServerState> {
        let ProgressParams { token, value } = params;
        match value.kind.as_str() {
            "begin" if value.title.as_deref() == Some(PROJECT_LOAD_TITLE) => {
                self.loading.push(token);
                self.state.readiness = Readiness::Indexing;
            }
            "end" => {
                let index = self.loading.iter().position(|t| *t == token)?;
                self.loading.remove(index);
                if !self.loading.is_empty() {
                    return None;
                }
                self.state.readiness = Readiness::Ready;
                // ロードの成功を観測した。ただし tsserver が落ちた後の end
                // (indicator の reset) は成功ではない。再起動はないので戻さない。
                if self.state.health != Health::Error {
                    self.state.health = Health::Ok;
                }
            }
            _ => return None,
        }
        Some(self.state.clone())
    }

    fn on_log(&mut self, message: &str) -> Option<ServerState> {
        if !message.contains(TSSERVER_EXITED) {
            return None;
        }
        self.state.health = Health::Error;
        self.state.message = Some(message.to_string());
        Some(self.state.clone())
    }
}

impl Mapping for TypescriptLanguageServerAdapter {
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
        if !view.is_notification() {
            return None;
        }
        match view.method() {
            Some(PROGRESS_METHOD) => {
                #[derive(Deserialize)]
                struct Envelope {
                    params: ProgressParams,
                }
                let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
                self.on_progress(envelope.params)
            }
            Some(LOG_MESSAGE_METHOD) => {
                #[derive(Deserialize)]
                struct Envelope {
                    params: LogMessageParams,
                }
                let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
                self.on_log(&envelope.params.message)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;
    use crate::state::{Health, Readiness};
    use serde_json::json;

    fn progress(token: &str, value: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"$/progress","params":{{"token":"{token}","value":{value}}}}}"#
        )
    }

    fn load_begin(token: &str) -> String {
        progress(
            token,
            r#"{"kind":"begin","title":"Initializing JS/TS language features…"}"#,
        )
    }

    fn load_end(token: &str) -> String {
        progress(token, r#"{"kind":"end"}"#)
    }

    fn log(kind: u8, message: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"window/logMessage","params":{{"type":{kind},"message":"{message}"}}}}"#
        )
    }

    /// 実測の文言 (SIGKILL)。
    const EXITED: &str = "[lspserver] [tsclient] [tsserver] Exited. Code: null. Signal: SIGKILL";

    fn interpret(adapter: &mut TypescriptLanguageServerAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    // --- 名乗り ----------------------------------------------------------------

    #[test]
    fn reads_the_startup_log_as_the_identity_before_initialize_completes() {
        // 実測の文言そのもの。initialize 応答より先に届く唯一の名乗り。
        let identity = startup_identity(
            r#"Using Typescript version (user-setting) 5.9.3 from path "/nix/store/x/lib/tsserver.js""#,
        )
        .expect("起動ログは名乗り");
        assert_eq!(identity.name, SERVER_NAME);
        assert_eq!(identity.version.as_deref(), Some("5.9.3"));

        let bundled = startup_identity(r#"Using Typescript version (bundled) 5.9.3 from path "x""#)
            .expect("source が違っても名乗り");
        assert_eq!(bundled.version.as_deref(), Some("5.9.3"));
    }

    #[test]
    fn other_log_lines_are_not_identities() {
        for other in [
            "Pyright language server 1.1.412 starting",
            "Using (workspace) TypeScript 5.9.3 instead - x",
            "[lspserver] Killing TS Server",
            "Using Typescript version",
            "",
        ] {
            assert!(
                startup_identity(other).is_none(),
                "名乗りでない行: {other:?}"
            );
        }
    }

    #[test]
    fn reads_the_typescript_version_as_the_identity() {
        // 実測の params そのもの。名前は写像の鍵、版は tsserver の版。
        let identity = identity_from_typescript_version(
            &json!({"version": "5.9.3", "source": "user-setting"}),
        )
        .expect("$/typescriptVersion は名乗り");
        assert_eq!(identity.name, SERVER_NAME);
        assert_eq!(identity.version.as_deref(), Some("5.9.3"));
    }

    #[test]
    fn a_typescript_version_without_a_version_string_still_names_the_server() {
        let identity = identity_from_typescript_version(&json!({"source": "bundled"}))
            .expect("版がなくても名乗り");
        assert_eq!(identity.name, SERVER_NAME);
        assert_eq!(identity.version, None);
        assert!(
            identity_from_typescript_version(&json!("5.9.3")).is_none(),
            "オブジェクト以外は読まない"
        );
    }

    // --- readiness -------------------------------------------------------------

    #[test]
    fn starts_initializing_with_unknown_health() {
        let state = TypescriptLanguageServerAdapter::new().initial_state();
        assert_eq!(state.readiness, Readiness::Initializing);
        assert_eq!(state.health, Health::Unknown);
    }

    #[test]
    fn begin_of_a_project_load_means_indexing() {
        let mut adapter = TypescriptLanguageServerAdapter::new();
        let state = interpret(&mut adapter, &load_begin("1")).expect("begin は信号");
        assert_eq!(state.readiness, Readiness::Indexing);
        assert_eq!(state.health, Health::Unknown, "begin は health を語らない");
    }

    #[test]
    fn end_of_the_project_load_means_ready_and_ok() {
        let mut adapter = TypescriptLanguageServerAdapter::new();
        interpret(&mut adapter, &load_begin("1"));
        let state = interpret(&mut adapter, &load_end("1")).expect("end は信号");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(state.health, Health::Ok, "ロードの成功を観測した");
    }

    #[test]
    fn sequential_project_loads_rearm() {
        // 2 つ目のプロジェクトを開くと、1 つ目の end → 2 つ目の begin → end。
        let mut adapter = TypescriptLanguageServerAdapter::new();
        interpret(&mut adapter, &load_begin("a"));
        interpret(&mut adapter, &load_end("a"));
        let state = interpret(&mut adapter, &load_begin("b")).expect("再ロードの begin は信号");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state = interpret(&mut adapter, &load_end("b")).expect("end は信号");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn waits_for_every_open_token() {
        // 逐次ロードなので同時に 2 つは開かないはずだが、開いたら全部待つ。
        let mut adapter = TypescriptLanguageServerAdapter::new();
        interpret(&mut adapter, &load_begin("a"));
        interpret(&mut adapter, &load_begin("b"));
        let after_a = interpret(&mut adapter, &load_end("a"));
        assert!(
            after_a
                .as_ref()
                .is_none_or(|s| s.readiness != Readiness::Ready),
            "1 つ目の end で ready を名乗った: {after_a:?}"
        );
        assert_eq!(
            interpret(&mut adapter, &load_end("b"))
                .expect("最後の end は信号")
                .readiness,
            Readiness::Ready
        );
    }

    #[test]
    fn end_of_an_unknown_token_is_ignored() {
        let mut adapter = TypescriptLanguageServerAdapter::new();
        assert!(interpret(&mut adapter, &load_end("stray")).is_none());
    }

    // --- health ----------------------------------------------------------------

    #[test]
    fn a_tsserver_exit_log_means_error_with_the_log_as_message() {
        let mut adapter = TypescriptLanguageServerAdapter::new();
        interpret(&mut adapter, &load_begin("1"));
        interpret(&mut adapter, &load_end("1"));
        let state = interpret(&mut adapter, &log(1, EXITED)).expect("クラッシュは信号");
        assert_eq!(state.health, Health::Error);
        assert_eq!(state.message.as_deref(), Some(EXITED));
        assert_eq!(state.readiness, Readiness::Ready, "readiness は変えない");
    }

    #[test]
    fn a_later_end_does_not_restore_ok_after_a_crash() {
        // tsserver は再起動されない。クラッシュ時に indicator が reset (end)
        // されても ok に戻さない。
        let mut adapter = TypescriptLanguageServerAdapter::new();
        interpret(&mut adapter, &load_begin("1"));
        interpret(&mut adapter, &log(1, EXITED));
        let state = interpret(&mut adapter, &load_end("1"));
        assert!(
            state.as_ref().is_none_or(|s| s.health == Health::Error),
            "クラッシュ後の end で ok に戻した: {state:?}"
        );
    }

    #[test]
    fn ignores_other_logs_progress_and_other_vocabularies() {
        let mut adapter = TypescriptLanguageServerAdapter::new();
        for other in [
            log(3, "Using Typescript version (user-setting) 5.9.3 from path x"),
            log(3, "[lspserver] Killing TS Server"),
            log(1, "some other error"),
            progress("d", r#"{"kind":"begin","title":"Finding references"}"#),
            progress("d", r#"{"kind":"end"}"#),
            r#"{"jsonrpc":"2.0","method":"$/typescriptVersion","params":{"version":"5.9.3","source":"user-setting"}}"#.to_string(),
            r#"{"jsonrpc":"2.0","method":"experimental/serverStatus","params":{"health":"ok","quiescent":true,"message":null}}"#.to_string(),
        ] {
            assert!(
                interpret(&mut adapter, &other).is_none(),
                "無関係なメッセージで状態が動いた: {other}"
            );
        }
    }

    // --- 保証 ------------------------------------------------------------------

    #[test]
    fn declares_no_guarantees_until_the_conformance_suite_passed_on_a_version() {
        for version in [Some("5.9.3"), Some("5.3.0"), None] {
            assert_eq!(
                TypescriptLanguageServerAdapter::for_version(version).guarantees(),
                ServerStateProvider::Basic(true),
                "測っていない版 {version:?} に保証を宣言した"
            );
        }
    }
}
