//! The set of open documents, and the rewriting of a duplicate `didOpen` (design 4.3,
//! ADR 0015 decision B).
//!
//! Claude Code re-sends `textDocument/didOpen` for an already open uri on every Write
//! (an LSP violation). typescript-language-server rejects this, and the stale buffer lives on.
//! The downstream side remembers the open uris and rewrites the second `didOpen` into a
//! full-text `textDocument/didChange` before sending it upstream. `text` is cut out of the
//! original bytes and not re-serialized (design 4.4).

use std::collections::HashSet;

use serde::Deserialize;
use serde_json::value::RawValue;

use crate::framing::RawMessage;

#[derive(Default)]
pub struct OpenDocuments {
    uris: HashSet<String>,
}

#[derive(Deserialize)]
struct DidOpen<'a> {
    #[serde(borrow)]
    params: DidOpenParams<'a>,
}

#[derive(Deserialize)]
struct DidOpenParams<'a> {
    #[serde(borrow, rename = "textDocument")]
    text_document: TextDocumentItem<'a>,
}

#[derive(Deserialize)]
struct TextDocumentItem<'a> {
    uri: String,
    #[serde(borrow)]
    version: &'a RawValue,
    #[serde(borrow)]
    text: &'a RawValue,
}

#[derive(Deserialize)]
struct DidClose {
    params: DidCloseParams,
}

#[derive(Deserialize)]
struct DidCloseParams {
    #[serde(rename = "textDocument")]
    text_document: TextDocumentIdentifier,
}

#[derive(Deserialize)]
struct TextDocumentIdentifier {
    uri: String,
}

impl OpenDocuments {
    pub fn new() -> Self {
        Self::default()
    }

    /// Observes `textDocument/didOpen`. If the uri is already open, returns the message
    /// rewritten into a full-text `didChange` (which is sent upstream). If it is the first
    /// time, `None` (the original is sent as is). An unreadable body is also `None`.
    pub fn on_did_open(&mut self, body: &[u8]) -> Option<RawMessage> {
        let opened: DidOpen = serde_json::from_slice(body).ok()?;
        let item = opened.params.text_document;
        if self.uris.insert(item.uri.clone()) {
            return None;
        }
        // The uri is re-encoded as a JSON string (RawValue would also do to keep the original
        // escapes, but the uri is short and its content does not change).
        let uri = serde_json::to_string(&item.uri).ok()?;
        let mut rewritten = Vec::with_capacity(body.len() + 64);
        rewritten.extend_from_slice(
            br#"{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"textDocument":{"uri":"#,
        );
        rewritten.extend_from_slice(uri.as_bytes());
        rewritten.extend_from_slice(br#","version":"#);
        rewritten.extend_from_slice(item.version.get().as_bytes());
        rewritten.extend_from_slice(br#"},"contentChanges":[{"text":"#);
        rewritten.extend_from_slice(item.text.get().as_bytes());
        rewritten.extend_from_slice(b"}]}}");
        Some(RawMessage { body: rewritten })
    }

    /// Observes `textDocument/didClose`.
    pub fn on_did_close(&mut self, body: &[u8]) {
        if let Ok(closed) = serde_json::from_slice::<DidClose>(body) {
            self.uris.remove(&closed.params.text_document.uri);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn did_open(uri: &str, version: u32, text: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {"textDocument": {"uri": uri, "languageId": "rust", "version": version, "text": text}},
        }))
        .unwrap()
    }

    #[test]
    fn the_first_open_passes_and_the_second_becomes_a_full_text_change() {
        let mut docs = OpenDocuments::new();
        assert!(
            docs.on_did_open(&did_open("file:///w/a.rs", 1, "fn a() {}\n"))
                .is_none()
        );
        let rewritten = docs
            .on_did_open(&did_open("file:///w/a.rs", 1, "fn a() {}\nfn b() {}\n"))
            .expect("the second time is rewritten");
        let value: serde_json::Value = serde_json::from_slice(&rewritten.body).unwrap();
        assert_eq!(value["method"], "textDocument/didChange");
        assert_eq!(value["params"]["textDocument"]["uri"], "file:///w/a.rs");
        assert_eq!(value["params"]["textDocument"]["version"], 1);
        assert_eq!(
            value["params"]["contentChanges"],
            serde_json::json!([{"text": "fn a() {}\nfn b() {}\n"}])
        );
    }

    #[test]
    fn reopening_after_close_is_a_real_open() {
        let mut docs = OpenDocuments::new();
        assert!(
            docs.on_did_open(&did_open("file:///w/a.rs", 1, ""))
                .is_none()
        );
        docs.on_did_close(
            br#"{"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":"file:///w/a.rs"}}}"#,
        );
        assert!(
            docs.on_did_open(&did_open("file:///w/a.rs", 2, ""))
                .is_none()
        );
    }

    #[test]
    fn keeps_escapes_in_the_text_verbatim() {
        let mut docs = OpenDocuments::new();
        docs.on_did_open(&did_open("file:///w/a.rs", 1, ""));
        let body = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///w/a.rs","languageId":"rust","version":2,"text":"aé\n\"q\""}}}"#;
        let rewritten = docs.on_did_open(body.as_bytes()).unwrap();
        let text = String::from_utf8(rewritten.body).unwrap();
        assert!(text.contains(r#""contentChanges":[{"text":"aé\n\"q\""}]"#));
    }
}
