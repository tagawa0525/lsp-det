//! JSON-RPC メッセージの覗き見 (v0.1-design.md 4.6)。
//!
//! ボディは原文バイトのまま転送するため、プロキシが解釈するのは
//! `method` と `id` の 2 フィールドだけである。完全パース +
//! 再シリアライズはしない (キー順序の変化・未知フィールドの欠落を招く)。
//!
//! 未知のフィールドは黙って読み飛ばす。ra-multiplex は
//! `deny_unknown_fields` で未知メッセージを落とす不具合を持つ
//! (docs/research/proxy-implementations.md)。

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum PeekError {
    #[error("message body is not valid JSON")]
    InvalidJson(#[from] serde_json::Error),
}

/// JSON-RPC のリクエスト ID。LSP は `integer | string` のみを許す。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

/// メッセージから覗き見た最小限の情報。ボディの寿命を借りる。
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct MessageView<'a> {
    #[serde(borrow)]
    pub method: Option<Cow<'a, str>>,
    pub id: Option<RequestId>,
}

impl MessageView<'_> {
    /// 応答を要するリクエスト (method と id の両方を持つ)。
    pub fn is_request(&self) -> bool {
        self.method.is_some() && self.id.is_some()
    }

    /// 応答を要さない通知 (method のみ)。
    pub fn is_notification(&self) -> bool {
        self.method.is_some() && self.id.is_none()
    }

    pub fn method(&self) -> Option<&str> {
        self.method.as_deref()
    }
}

/// メッセージボディから `method` と `id` を覗き見る。
pub fn peek(body: &[u8]) -> Result<MessageView<'_>, PeekError> {
    Ok(serde_json::from_slice(body)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peeks_method_and_numeric_id_of_a_request() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"textDocument/references","params":{}}"#;
        let view = peek(body).unwrap();
        assert_eq!(view.method(), Some("textDocument/references"));
        assert_eq!(view.id, Some(RequestId::Number(1)));
        assert!(view.is_request());
        assert!(!view.is_notification());
    }

    #[test]
    fn peeks_a_notification_as_having_no_id() {
        let body = br#"{"jsonrpc":"2.0","method":"textDocument/didChange","params":{}}"#;
        let view = peek(body).unwrap();
        assert_eq!(view.method(), Some("textDocument/didChange"));
        assert_eq!(view.id, None);
        assert!(view.is_notification());
        assert!(!view.is_request());
    }

    #[test]
    fn peeks_a_response_as_having_no_method() {
        let body = br#"{"jsonrpc":"2.0","id":7,"result":[]}"#;
        let view = peek(body).unwrap();
        assert_eq!(view.method(), None);
        assert_eq!(view.id, Some(RequestId::Number(7)));
        assert!(!view.is_request());
        assert!(!view.is_notification());
    }

    #[test]
    fn peeks_a_string_id() {
        let body = br#"{"jsonrpc":"2.0","id":"req-42","method":"workspace/symbol"}"#;
        let view = peek(body).unwrap();
        assert_eq!(view.id, Some(RequestId::String("req-42".to_string())));
    }

    #[test]
    fn treats_null_id_as_absent() {
        // JSON-RPC はパースエラー応答で `id: null` を使う。
        let body = br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"x"}}"#;
        let view = peek(body).unwrap();
        assert_eq!(view.id, None);
    }

    #[test]
    fn tolerates_unknown_fields() {
        // ra-multiplex の deny_unknown_fields 不具合を踏まない。
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"foo","$brandNew":{"a":[1,2]},"x":null}"#;
        let view = peek(body).unwrap();
        assert_eq!(view.method(), Some("foo"));
        assert_eq!(view.id, Some(RequestId::Number(1)));
    }

    #[test]
    fn is_independent_of_field_order() {
        let method_first = peek(br#"{"method":"foo","id":1}"#).unwrap();
        let id_first = peek(br#"{"id":1,"method":"foo"}"#).unwrap();
        assert_eq!(method_first, id_first);
    }

    #[test]
    fn decodes_escapes_in_the_method_name() {
        let body = br#"{"method":"a\/b","id":1}"#;
        let view = peek(body).unwrap();
        assert_eq!(view.method(), Some("a/b"));
    }

    #[test]
    fn errors_on_malformed_json() {
        assert!(peek(b"not json at all").is_err());
    }

    #[test]
    fn errors_on_a_json_value_that_is_not_an_object() {
        assert!(peek(b"[1,2,3]").is_err());
    }
}
