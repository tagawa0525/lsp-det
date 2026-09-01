//! `initialize` の capability 書き換え (v0.1-design.md 4.5)。
//!
//! ready 信号はクライアントの capability 宣言に依存する。rust-analyzer は
//! `experimental.serverStatusNotification` が未宣言だと
//! `experimental/serverStatus` を一切送らないため、プロキシが上流への
//! `initialize` に自ら宣言を足す。
//!
//! ここはボディを完全パースして再シリアライズする数少ない例外である
//! (4.6 が `initialize` と readiness 追跡用の通知にだけ認めている)。
//! 書き換えが不要なら原文バイトをそのまま使うよう `None` を返す。

use serde_json::{Map, Value};

use crate::state::ServerStateProvider;

/// クライアントが拡張 S を自分で扱うと宣言したか (仕様 5.2)。
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

/// 上流の `InitializeResult` が既に `experimental.serverStateProvider` を
/// 宣言しているか。宣言していれば上流自身が拡張 S に準拠しており、中継層は
/// 拡張 S について透過する (ADR 0008 追補 D)。
pub fn upstream_declares_server_state_provider(body: &[u8]) -> bool {
    let Ok(root) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    root.get("result")
        .and_then(|result| result.get("capabilities"))
        .and_then(|caps| caps.get("experimental"))
        .and_then(|experimental| experimental.get("serverStateProvider"))
        .is_some()
}

/// `InitializeResult` に `experimental.serverStateProvider` を足した
/// 新しいボディを返す。書き換えられなければ `None`。
///
/// 上流が返した capability は一切変えない。中継層は宣言を**足す**だけで、
/// 上流の宣言を置き換えると上流が本当に持つ機能を隠すことになる。
/// 上流が既に `serverStateProvider` を宣言していれば何もしない (`None`)。
pub fn declare_server_state_provider(
    body: &[u8],
    provider: &ServerStateProvider,
) -> Option<Vec<u8>> {
    let mut root: Value = serde_json::from_slice(body).ok()?;
    let result = root.get_mut("result")?.as_object_mut()?;
    let capabilities = result
        .entry("capabilities")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()?;
    let experimental = capabilities
        .entry("experimental")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()?;

    if experimental.contains_key("serverStateProvider") {
        // 上流自身の宣言を基本グレードで隠してはならない。
        return None;
    }
    experimental.insert(
        "serverStateProvider".to_string(),
        serde_json::to_value(provider).ok()?,
    );
    serde_json::to_vec(&root).ok()
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
    use super::*;

    const RA: &[&str] = &["experimental.serverStatusNotification"];

    fn rewrite(body: &str, paths: &[&str]) -> Option<Value> {
        inject_client_capabilities(body.as_bytes(), paths)
            .map(|bytes| serde_json::from_slice(&bytes).expect("rewritten body must be JSON"))
    }

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
        // アダプタなしのとき。原文をそのまま流す。
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
    fn adds_the_provider_to_an_initialize_result() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"hoverProvider":true}}}"#;
        let out: Value = serde_json::from_slice(
            &declare_server_state_provider(body.as_bytes(), &ServerStateProvider::complete())
                .expect("result should be rewritten"),
        )
        .unwrap();

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
        let out: Value = serde_json::from_slice(
            &declare_server_state_provider(body.as_bytes(), &ServerStateProvider::Basic(true))
                .expect("result should be rewritten"),
        )
        .unwrap();

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
        let body = r#"{"id":1,"result":{}}"#;
        let out: Value = serde_json::from_slice(
            &declare_server_state_provider(body.as_bytes(), &ServerStateProvider::Basic(true))
                .expect("result should be rewritten"),
        )
        .unwrap();
        assert_eq!(
            out["result"]["capabilities"]["experimental"]["serverStateProvider"],
            Value::Bool(true)
        );
    }

    #[test]
    fn never_overwrites_an_upstream_declaration() {
        // 上流が本当に持つ保証 (freshness) を基本グレードで隠してはならない。
        let body = r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":{"freshness":true}}}}}"#;
        assert!(
            declare_server_state_provider(body.as_bytes(), &ServerStateProvider::Basic(true))
                .is_none()
        );
    }

    #[test]
    fn overwrites_a_false_upstream_declaration() {
        // `serverStateProvider: false` は「提供しない」の意味 (仕様 5 章の
        // boolean)。上書きして自分の宣言を置く。
        let body =
            r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":false}}}}"#;
        let out: Value = serde_json::from_slice(
            &declare_server_state_provider(body.as_bytes(), &ServerStateProvider::Basic(true))
                .expect("false は宣言ではないので書き換える"),
        )
        .unwrap();
        assert_eq!(
            out["result"]["capabilities"]["experimental"]["serverStateProvider"],
            Value::Bool(true)
        );
    }

    #[test]
    fn detects_an_upstream_declaration() {
        for declared in [
            r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":true}}}}"#,
            r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":{"freshness":true}}}}}"#,
            r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":{}}}}}"#,
        ] {
            assert!(
                upstream_declares_server_state_provider(declared.as_bytes()),
                "宣言とみなすべき: {declared}"
            );
        }

        for body in [
            // クライアント側の判定 (client_declares_server_state) と対称に、
            // true かオブジェクトだけを宣言とみなす。
            r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":false}}}}"#,
            r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":null}}}}"#,
            r#"{"id":1,"result":{"capabilities":{"experimental":{}}}}"#,
            r#"{"id":1,"result":{"capabilities":{}}}"#,
            r#"{"id":1,"result":{}}"#,
            r#"{"id":1,"error":{"code":-32603,"message":"boom"}}"#,
            "{not json",
        ] {
            assert!(
                !upstream_declares_server_state_provider(body.as_bytes()),
                "宣言とみなしてはならない: {body}"
            );
        }
    }

    #[test]
    fn leaves_an_error_response_alone() {
        // 上流が initialize に失敗した応答へ capability を足しても意味がない。
        let body = r#"{"id":1,"error":{"code":-32603,"message":"boom"}}"#;
        assert!(
            declare_server_state_provider(body.as_bytes(), &ServerStateProvider::Basic(true))
                .is_none()
        );
    }
}
