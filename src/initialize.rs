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

/// `initialize` の `params.capabilities` に真偽フラグを足した新しいボディを返す。
/// 変更が不要・不可能なら `None` (呼び出し側は原文をそのまま転送する)。
///
/// `paths` は `capabilities` から見たドット区切りのパス
/// (例: `experimental.serverStatusNotification`)。
pub fn inject_client_capabilities(body: &[u8], paths: &[&str]) -> Option<Vec<u8>> {
    todo!("M2: capabilities にフラグを注入する")
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
}
