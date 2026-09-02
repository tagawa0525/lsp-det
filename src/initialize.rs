//! `initialize` の capability 注入と `InitializeResult` の読み取り
//! (v0.1-design.md 4.2)。
//!
//! ready 信号はクライアントの capability 宣言に依存する。rust-analyzer は
//! `experimental.serverStatusNotification` が未宣言だと
//! `experimental/serverStatus` を一切送らないため、プロキシが上流への
//! `initialize` に自ら宣言を足す。どの写像を使うかは `InitializeResult` の
//! `serverInfo` で分かるが、注入はその前に要るので既知の写像ぶんを無条件に
//! 注入する (ADR 0009 決定 D-3)。
//!
//! ここはボディを完全パースして再シリアライズする数少ない例外である
//! (4.4 が `initialize` / `InitializeResult` と写像用の通知にだけ認めている)。
//! 書き換えが不要なら原文バイトをそのまま使うよう伝える。

use serde_json::{Map, Value};

use crate::state::ServerStateProvider;

/// クライアントが状態を自分で解釈すると宣言したか (仕様 5.2)。
///
/// この宣言は「通知が欲しい」と「保護が不要」の両方を意味する。宣言した
/// クライアントが状態を無視して不完全な結果を得た場合、それはその
/// クライアントの責任である。
pub fn client_declares_server_state(body: &[u8]) -> bool {
    let Ok(root) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    root.get("params")
        .and_then(|params| params.get("capabilities"))
        .and_then(|caps| caps.get("experimental"))
        .and_then(|experimental| experimental.get("serverState"))
        == Some(&Value::Bool(true))
}

/// クライアントが `window.workDoneProgress` を自分で宣言していたか。
///
/// 宣言していないクライアントは、注入した宣言に由来する
/// `window/workDoneProgress/create` を扱えない (Serena は `MethodNotFound` を
/// 返す)。その場合は上流側が自ら応答する (設計 4.2)。
pub fn client_declares_work_done_progress(body: &[u8]) -> bool {
    let Ok(root) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    root.get("params")
        .and_then(|params| params.get("capabilities"))
        .and_then(|caps| caps.get("window"))
        .and_then(|window| window.get("workDoneProgress"))
        == Some(&Value::Bool(true))
}

/// 上流が `InitializeResult.serverInfo` で名乗った名前と版 (LSP 3.15)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub name: String,
    pub version: Option<String>,
}

/// 成功した `initialize` 応答から `serverInfo` を読む。名乗りがなければ
/// `None` (写像は選べず、上流側は両軸 `unknown` を報告する)。
pub fn server_info(body: &[u8]) -> Option<ServerInfo> {
    let root = serde_json::from_slice::<Value>(body).ok()?;
    let info = root.get("result")?.get("serverInfo")?;
    let name = info.get("name")?.as_str()?.to_string();
    let version = info
        .get("version")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some(ServerInfo { name, version })
}

/// 上流の `initialize` 応答をどう扱うか。
#[derive(Debug, PartialEq, Eq)]
pub enum InitializeResultAction {
    /// 上流自身が `serverStateProvider` を宣言している。上流側は恒等写像に
    /// なる (仕様 8.2 の 6)。ボディは原文のまま流す。
    UpstreamDeclares,
    /// 中継層の宣言を足した新しいボディ。
    Declared(Vec<u8>),
    /// `result` はあるが `capabilities` / `experimental` がオブジェクトでなく
    /// 書き換えられない。原文のまま流す。
    Unrewritable,
    /// 成功応答ではない (エラー応答、または `result` がない)。handshake は
    /// 完了していないので、クライアントは `initialize` を再試行しうる。
    NotASuccess,
}

/// `InitializeResult` に `experimental.serverStateProvider` を足す。
///
/// 上流が返した capability は一切変えない。中継層は宣言を**足す**だけで、
/// 上流の宣言を置き換えると上流が本当に持つ機能を隠すことになる。
/// 上流が既に宣言していれば [`InitializeResultAction::UpstreamDeclares`]。
///
/// 宣言とみなすのは `true` かオブジェクトだけ (クライアント側の
/// [`client_declares_server_state`] と対称)。`false` は「提供しない」の
/// 意味 (`hoverProvider: false` と同じ書き方) なので上書きする。
pub fn declare_server_state_provider(
    body: &[u8],
    provider: &ServerStateProvider,
) -> InitializeResultAction {
    use InitializeResultAction::*;

    let Ok(mut root) = serde_json::from_slice::<Value>(body) else {
        return NotASuccess;
    };
    let Some(result) = root.get_mut("result").and_then(Value::as_object_mut) else {
        return NotASuccess;
    };
    let Some(capabilities) = result
        .entry("capabilities")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
    else {
        return Unrewritable;
    };
    let Some(experimental) = capabilities
        .entry("experimental")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
    else {
        return Unrewritable;
    };

    if experimental
        .get("serverStateProvider")
        .is_some_and(is_declaration)
    {
        return UpstreamDeclares;
    }
    let Ok(declared) = serde_json::to_value(provider) else {
        return Unrewritable;
    };
    experimental.insert("serverStateProvider".to_string(), declared);
    match serde_json::to_vec(&root) {
        Ok(bytes) => Declared(bytes),
        Err(_) => Unrewritable,
    }
}

fn is_declaration(value: &Value) -> bool {
    matches!(value, Value::Bool(true) | Value::Object(_))
}

/// `initialize` の `params.capabilities` に真偽フラグを足した新しいボディを返す。
/// 変更が不要・不可能なら `None` (呼び出し側は原文をそのまま転送する)。
///
/// `paths` は `capabilities` から見たドット区切りのパス
/// (例: `experimental.serverStatusNotification`)。
pub fn inject_client_capabilities(body: &[u8], paths: &[&str]) -> Option<Vec<u8>> {
    if paths.is_empty() {
        return None;
    }

    let mut root: Value = serde_json::from_slice(body).ok()?;
    let params = root.get_mut("params")?.as_object_mut()?;
    let capabilities = params
        .entry("capabilities")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()?;

    let mut changed = false;
    for path in paths {
        changed |= set_true(capabilities, path);
    }
    if !changed {
        return None;
    }

    serde_json::to_vec(&root).ok()
}

/// ドット区切りのパスの葉を `true` にする。値が変わったら `true` を返す。
/// 経路上に非オブジェクトがあれば何もしない (壊れた宣言を破壊しない)。
fn set_true(capabilities: &mut Map<String, Value>, path: &str) -> bool {
    let Some((parents, leaf)) = path.rsplit_once('.') else {
        return set_leaf_true(capabilities, path);
    };

    let mut current = capabilities;
    for segment in parents.split('.') {
        let entry = current
            .entry(segment)
            .or_insert_with(|| Value::Object(Map::new()));
        match entry.as_object_mut() {
            Some(object) => current = object,
            None => return false,
        }
    }
    set_leaf_true(current, leaf)
}

fn set_leaf_true(object: &mut Map<String, Value>, key: &str) -> bool {
    if object.get(key) == Some(&Value::Bool(true)) {
        return false;
    }
    object.insert(key.to_string(), Value::Bool(true));
    true
}

#[cfg(test)]
mod tests {
    use super::InitializeResultAction::*;
    use super::*;

    const RA: &[&str] = &["experimental.serverStatusNotification"];

    fn rewrite(body: &str, paths: &[&str]) -> Option<Value> {
        inject_client_capabilities(body.as_bytes(), paths)
            .map(|bytes| serde_json::from_slice(&bytes).expect("rewritten body must be JSON"))
    }

    /// `Declared` の中身を JSON として返す。それ以外なら panic。
    fn declared(body: &str, provider: &ServerStateProvider) -> Value {
        match declare_server_state_provider(body.as_bytes(), provider) {
            Declared(bytes) => serde_json::from_slice(&bytes).expect("declared body must be JSON"),
            other => panic!("宣言を足せるはず: {other:?}"),
        }
    }

    // --- クライアント → 上流 ------------------------------------------------

    #[test]
    fn adds_the_flag_when_the_experimental_section_is_absent() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#;
        let out = rewrite(body, RA).expect("body should be rewritten");
        assert_eq!(
            out["params"]["capabilities"]["experimental"]["serverStatusNotification"],
            Value::Bool(true)
        );
    }

    #[test]
    fn creates_the_capabilities_object_when_it_is_missing() {
        let body = r#"{"id":1,"method":"initialize","params":{"processId":42}}"#;
        let out = rewrite(body, RA).expect("body should be rewritten");
        assert_eq!(
            out["params"]["capabilities"]["experimental"]["serverStatusNotification"],
            Value::Bool(true)
        );
        assert_eq!(out["params"]["processId"], Value::from(42));
    }

    #[test]
    fn leaves_every_other_capability_untouched() {
        let body = r#"{"id":1,"method":"initialize","params":{"processId":7,"rootUri":"file:///w","capabilities":{"textDocument":{"hover":{"dynamicRegistration":true}},"experimental":{"somethingElse":{"nested":[1,2]}}},"initializationOptions":{"a":"b"}}}"#;
        let out = rewrite(body, RA).expect("body should be rewritten");

        assert_eq!(out["params"]["processId"], Value::from(7));
        assert_eq!(out["params"]["rootUri"], Value::from("file:///w"));
        assert_eq!(
            out["params"]["initializationOptions"]["a"],
            Value::from("b")
        );
        assert_eq!(
            out["params"]["capabilities"]["textDocument"]["hover"]["dynamicRegistration"],
            Value::Bool(true)
        );
        assert_eq!(
            out["params"]["capabilities"]["experimental"]["somethingElse"]["nested"],
            serde_json::json!([1, 2])
        );
        assert_eq!(
            out["params"]["capabilities"]["experimental"]["serverStatusNotification"],
            Value::Bool(true)
        );
    }

    #[test]
    fn does_not_rewrite_when_the_client_already_declared_the_flag() {
        // Serena は宣言済み。原文バイトをそのまま流すため None を返す。
        let body = r#"{"id":1,"method":"initialize","params":{"capabilities":{"experimental":{"serverStatusNotification":true}}}}"#;
        assert!(inject_client_capabilities(body.as_bytes(), RA).is_none());
    }

    #[test]
    fn overrides_an_explicit_false() {
        // 宣言が false でもプロキシは信号を必要とする。
        let body = r#"{"id":1,"method":"initialize","params":{"capabilities":{"experimental":{"serverStatusNotification":false}}}}"#;
        let out = rewrite(body, RA).expect("false should be overridden");
        assert_eq!(
            out["params"]["capabilities"]["experimental"]["serverStatusNotification"],
            Value::Bool(true)
        );
    }

    #[test]
    fn injects_several_paths_at_once() {
        let body = r#"{"id":1,"method":"initialize","params":{"capabilities":{}}}"#;
        let out = rewrite(
            body,
            &[
                "experimental.serverStatusNotification",
                "window.workDoneProgress",
            ],
        )
        .expect("body should be rewritten");
        assert_eq!(
            out["params"]["capabilities"]["experimental"]["serverStatusNotification"],
            Value::Bool(true)
        );
        assert_eq!(
            out["params"]["capabilities"]["window"]["workDoneProgress"],
            Value::Bool(true)
        );
    }

    #[test]
    fn refuses_to_clobber_a_non_object_on_the_path() {
        // 壊れたクライアントの宣言でも破壊しない。転送は原文のまま。
        let body = r#"{"id":1,"method":"initialize","params":{"capabilities":{"experimental":"nonsense"}}}"#;
        assert!(inject_client_capabilities(body.as_bytes(), RA).is_none());
    }

    #[test]
    fn leaves_a_params_less_initialize_alone() {
        let body = r#"{"id":1,"method":"initialize"}"#;
        assert!(inject_client_capabilities(body.as_bytes(), RA).is_none());
    }

    #[test]
    fn leaves_malformed_json_alone() {
        assert!(inject_client_capabilities(b"{not json", RA).is_none());
    }

    #[test]
    fn does_nothing_when_no_paths_are_requested() {
        // 注入する宣言がなければ原文をそのまま流す。
        let body = r#"{"id":1,"method":"initialize","params":{"capabilities":{}}}"#;
        assert!(inject_client_capabilities(body.as_bytes(), &[]).is_none());
    }

    #[test]
    fn detects_the_client_declaration() {
        let declared = r#"{"id":1,"method":"initialize","params":{"capabilities":{"experimental":{"serverState":true}}}}"#;
        assert!(client_declares_server_state(declared.as_bytes()));
    }

    #[test]
    fn treats_a_missing_or_false_declaration_as_absent() {
        for body in [
            r#"{"id":1,"method":"initialize","params":{"capabilities":{}}}"#,
            r#"{"id":1,"method":"initialize","params":{"capabilities":{"experimental":{}}}}"#,
            r#"{"id":1,"method":"initialize","params":{"capabilities":{"experimental":{"serverState":false}}}}"#,
            // truthy な別の値を宣言とみなさない。
            r#"{"id":1,"method":"initialize","params":{"capabilities":{"experimental":{"serverState":{}}}}}"#,
            r#"{"id":1,"method":"initialize"}"#,
            "{not json",
        ] {
            assert!(
                !client_declares_server_state(body.as_bytes()),
                "宣言とみなしてはならない: {body}"
            );
        }
    }

    #[test]
    fn detects_the_client_work_done_progress_declaration() {
        let declared = r#"{"id":1,"method":"initialize","params":{"capabilities":{"window":{"workDoneProgress":true}}}}"#;
        assert!(client_declares_work_done_progress(declared.as_bytes()));
        for body in [
            r#"{"id":1,"method":"initialize","params":{"capabilities":{}}}"#,
            r#"{"id":1,"method":"initialize","params":{"capabilities":{"window":{"workDoneProgress":false}}}}"#,
            "{not json",
        ] {
            assert!(!client_declares_work_done_progress(body.as_bytes()));
        }
    }

    #[test]
    fn reads_the_server_info_from_a_successful_result() {
        let body = r#"{"id":1,"result":{"capabilities":{},"serverInfo":{"name":"rust-analyzer","version":"1.98.0 (88d9e12 2026-08-18)"}}}"#;
        assert_eq!(
            server_info(body.as_bytes()),
            Some(ServerInfo {
                name: "rust-analyzer".to_string(),
                version: Some("1.98.0 (88d9e12 2026-08-18)".to_string()),
            })
        );
    }

    #[test]
    fn server_info_is_absent_when_the_upstream_does_not_name_itself() {
        for body in [
            r#"{"id":1,"result":{"capabilities":{}}}"#,
            r#"{"id":1,"result":{"capabilities":{},"serverInfo":{"version":"1"}}}"#,
            r#"{"id":1,"error":{"code":-32603,"message":"boom"}}"#,
            "{not json",
        ] {
            assert_eq!(server_info(body.as_bytes()), None, "名乗りがない: {body}");
        }
    }

    // --- 上流 → クライアント ------------------------------------------------

    #[test]
    fn adds_the_provider_to_an_initialize_result() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"hoverProvider":true}}}"#;
        let out = declared(body, &ServerStateProvider::complete());
        assert_eq!(
            out["result"]["capabilities"]["experimental"]["serverStateProvider"],
            serde_json::json!({"completeness": true})
        );
        // 上流の宣言はそのまま残る。
        assert_eq!(
            out["result"]["capabilities"]["hoverProvider"],
            Value::Bool(true)
        );
    }

    #[test]
    fn keeps_the_upstream_experimental_capabilities() {
        let body =
            r#"{"id":1,"result":{"capabilities":{"experimental":{"upstreamThing":{"a":1}}}}}"#;
        let out = declared(body, &ServerStateProvider::Basic(true));
        assert_eq!(
            out["result"]["capabilities"]["experimental"]["upstreamThing"]["a"],
            Value::from(1)
        );
        assert_eq!(
            out["result"]["capabilities"]["experimental"]["serverStateProvider"],
            Value::Bool(true)
        );
    }

    #[test]
    fn creates_the_capabilities_object_in_a_bare_result() {
        let out = declared(r#"{"id":1,"result":{}}"#, &ServerStateProvider::Basic(true));
        assert_eq!(
            out["result"]["capabilities"]["experimental"]["serverStateProvider"],
            Value::Bool(true)
        );
    }

    #[test]
    fn never_overwrites_an_upstream_declaration() {
        // 上流が本当に持つ保証 (freshness) を保証なしの宣言で隠してはならない。
        for body in [
            r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":{"freshness":true}}}}}"#,
            r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":{}}}}}"#,
            r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":true}}}}"#,
        ] {
            assert_eq!(
                declare_server_state_provider(body.as_bytes(), &ServerStateProvider::Basic(true)),
                UpstreamDeclares,
                "上流の宣言として透過すべき: {body}"
            );
        }
    }

    #[test]
    fn overwrites_a_false_or_null_upstream_declaration() {
        // `serverStateProvider: false` は「提供しない」の意味 (仕様 5 章の
        // boolean)。宣言ではないので上書きして自分の宣言を置く。
        for body in [
            r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":false}}}}"#,
            r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":null}}}}"#,
        ] {
            let out = declared(body, &ServerStateProvider::Basic(true));
            assert_eq!(
                out["result"]["capabilities"]["experimental"]["serverStateProvider"],
                Value::Bool(true),
                "false / null は宣言ではない: {body}"
            );
        }
    }

    #[test]
    fn an_error_response_is_not_a_success() {
        // handshake は完了していない。クライアントは initialize を再試行しうる。
        for body in [
            r#"{"id":1,"error":{"code":-32603,"message":"boom"}}"#,
            r#"{"id":1,"result":"not an object"}"#,
            "{not json",
        ] {
            assert_eq!(
                declare_server_state_provider(body.as_bytes(), &ServerStateProvider::Basic(true)),
                NotASuccess,
                "成功応答とみなしてはならない: {body}"
            );
        }
    }

    #[test]
    fn a_non_object_capabilities_is_unrewritable() {
        for body in [
            r#"{"id":1,"result":{"capabilities":"nonsense"}}"#,
            r#"{"id":1,"result":{"capabilities":{"experimental":null}}}"#,
        ] {
            assert_eq!(
                declare_server_state_provider(body.as_bytes(), &ServerStateProvider::Basic(true)),
                Unrewritable,
                "書き換え不能とみなすべき: {body}"
            );
        }
    }
}
