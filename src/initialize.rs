//! Capability injection into `initialize` and reading of `InitializeResult`
//! (v0.1-design.md 4.2).
//!
//! The ready signal depends on the client's capability declaration. rust-analyzer sends no
//! `experimental/serverStatus` at all unless `experimental.serverStatusNotification` is
//! declared, so the proxy adds the declaration itself to the `initialize` going upstream.
//! Which mapping is used is known from `serverInfo` in `InitializeResult`, but the injection
//! is needed before that, so the declarations for the known mappings are injected
//! unconditionally (ADR 0009 decision D-3).
//!
//! This is one of the few exceptions where the body is fully parsed and re-serialized
//! (4.4 allows it only for `initialize` / `InitializeResult` and the notifications used by
//! mappings). When no rewrite is needed, the caller is told to use the original bytes as is.

use serde_json::{Map, Value};

use crate::state::ServerStateProvider;

/// Whether the client declared that it interprets the state itself (spec 5.2).
///
/// This declaration means both "I want the notifications" and "I need no protection". If a
/// client that declared it ignores the state and gets an incomplete result, that is the
/// client's responsibility.
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

/// Whether the client declared `window.workDoneProgress` on its own.
///
/// A client that did not declare it cannot handle the `window/workDoneProgress/create` that
/// results from the injected declaration (Serena returns `MethodNotFound`). In that case the
/// upstream side responds itself (design 4.2).
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

/// Whether the client declares `workspace.didChangeWatchedFiles`
/// (ADR 0015: if it does, the downstream side does not stand in).
pub fn client_declares_watched_files(body: &[u8]) -> bool {
    let Ok(root) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    root.get("params")
        .and_then(|params| params.get("capabilities"))
        .and_then(|caps| caps.get("workspace"))
        .and_then(|workspace| workspace.get("didChangeWatchedFiles"))
        .is_some_and(Value::is_object)
}

/// The workspace roots the client's `initialize` points to
/// (`workspaceFolders`, or `rootUri` if absent). Anything other than `file:` is excluded.
pub fn workspace_roots(body: &[u8]) -> Vec<std::path::PathBuf> {
    let Ok(root) = serde_json::from_slice::<Value>(body) else {
        return Vec::new();
    };
    let params = &root["params"];
    let mut roots: Vec<std::path::PathBuf> = params["workspaceFolders"]
        .as_array()
        .map(|folders| {
            folders
                .iter()
                .filter_map(|folder| folder["uri"].as_str())
                .filter_map(crate::uri::uri_to_path)
                .collect()
        })
        .unwrap_or_default();
    if roots.is_empty()
        && let Some(path) = params["rootUri"].as_str().and_then(crate::uri::uri_to_path)
    {
        roots.push(path);
    }
    roots
}

/// The name and version the upstream called itself in `InitializeResult.serverInfo`
/// (LSP 3.15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub name: String,
    pub version: Option<String>,
}

/// Reads `serverInfo` from a successful `initialize` response. `None` if the server does not
/// call itself anything (no mapping can be chosen, and the upstream side reports `unknown` on
/// both axes).
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

/// What to do with the upstream's `initialize` response.
#[derive(Debug, PartialEq, Eq)]
pub enum InitializeResultAction {
    /// The upstream itself declares `serverStateProvider`. The upstream side becomes the
    /// identity mapping (spec 8.2 item 6). The body is passed through as the original.
    UpstreamDeclares,
    /// A new body with the relay's declaration added.
    Declared(Vec<u8>),
    /// `result` exists but `capabilities` / `experimental` is not an object and cannot be
    /// rewritten. Passed through as the original.
    Unrewritable,
    /// Not a success response (an error response, or no `result`). The handshake is not
    /// complete, so the client may retry `initialize`.
    NotASuccess,
}

/// Adds `experimental.serverStateProvider` to `InitializeResult`.
///
/// The capabilities the upstream returned are not changed at all. The relay only **adds** a
/// declaration; replacing the upstream's declaration would hide features the upstream really
/// has. If the upstream already declares it, [`InitializeResultAction::UpstreamDeclares`].
///
/// Only `true` or an object counts as a declaration (symmetric with
/// [`client_declares_server_state`] on the client side). `false` means "not provided"
/// (the same notation as `hoverProvider: false`), so it is overwritten.
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
    // A declaration is always an object (ADR 0016). `true` / `false` are not declarations.
    matches!(value, Value::Object(_))
}

/// Returns a new body with boolean flags added to `params.capabilities` of `initialize`.
/// `None` if no change is needed or possible (the caller forwards the original as is).
///
/// `paths` are dot-separated paths relative to `capabilities`
/// (e.g. `experimental.serverStatusNotification`).
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

/// Sets the leaf of a dot-separated path to `true`. Returns `true` if the value changed.
/// Does nothing if there is a non-object on the path (does not destroy a broken declaration).
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

    /// Returns the content of `Declared` as JSON. Panics otherwise.
    fn declared(body: &str, provider: &ServerStateProvider) -> Value {
        match declare_server_state_provider(body.as_bytes(), provider) {
            Declared(bytes) => serde_json::from_slice(&bytes).expect("declared body must be JSON"),
            other => panic!("should be able to add the declaration: {other:?}"),
        }
    }

    // --- Client -> upstream -------------------------------------------------

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
        // Serena already declares it. Returns None so the original bytes pass through as is.
        let body = r#"{"id":1,"method":"initialize","params":{"capabilities":{"experimental":{"serverStatusNotification":true}}}}"#;
        assert!(inject_client_capabilities(body.as_bytes(), RA).is_none());
    }

    #[test]
    fn overrides_an_explicit_false() {
        // Even if the declaration is false, the proxy needs the signal.
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
        // Even a broken client declaration is not destroyed. Forwarded as the original.
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
        // With no declaration to inject, the original passes through as is.
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
            // Another truthy value is not counted as a declaration.
            r#"{"id":1,"method":"initialize","params":{"capabilities":{"experimental":{"serverState":{}}}}}"#,
            r#"{"id":1,"method":"initialize"}"#,
            "{not json",
        ] {
            assert!(
                !client_declares_server_state(body.as_bytes()),
                "must not be counted as a declaration: {body}"
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
            assert_eq!(
                server_info(body.as_bytes()),
                None,
                "the server does not call itself anything: {body}"
            );
        }
    }

    // --- Upstream -> client -------------------------------------------------

    #[test]
    fn adds_the_provider_to_an_initialize_result() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{"hoverProvider":true}}}"#;
        let out = declared(body, &ServerStateProvider::workspace(&[], &[]));
        assert_eq!(
            out["result"]["capabilities"]["experimental"]["serverStateProvider"],
            serde_json::json!({"coverage": {"scope": "workspace", "incomplete": {}}, "freshness": {"fileChanges": []}})
        );
        // The upstream's declarations remain as they are.
        assert_eq!(
            out["result"]["capabilities"]["hoverProvider"],
            Value::Bool(true)
        );
    }

    #[test]
    fn keeps_the_upstream_experimental_capabilities() {
        let body =
            r#"{"id":1,"result":{"capabilities":{"experimental":{"upstreamThing":{"a":1}}}}}"#;
        let out = declared(body, &ServerStateProvider::notifications_only());
        assert_eq!(
            out["result"]["capabilities"]["experimental"]["upstreamThing"]["a"],
            Value::from(1)
        );
        assert_eq!(
            out["result"]["capabilities"]["experimental"]["serverStateProvider"],
            serde_json::json!({})
        );
    }

    #[test]
    fn creates_the_capabilities_object_in_a_bare_result() {
        let out = declared(
            r#"{"id":1,"result":{}}"#,
            &ServerStateProvider::notifications_only(),
        );
        assert_eq!(
            out["result"]["capabilities"]["experimental"]["serverStateProvider"],
            serde_json::json!({})
        );
    }

    /// Expert returns `"experimental": null` (measured with 0.1.9). LSP treats an absent and a
    /// null optional field alike, so null is replaced by an object, not refused.
    #[test]
    fn treats_an_explicit_null_experimental_as_absent() {
        let out = declared(
            r#"{"id":1,"result":{"capabilities":{"hoverProvider":true,"experimental":null}}}"#,
            &ServerStateProvider::notifications_only(),
        );
        assert_eq!(
            out["result"]["capabilities"]["experimental"]["serverStateProvider"],
            serde_json::json!({})
        );
        assert_eq!(out["result"]["capabilities"]["hoverProvider"], true);
    }

    #[test]
    fn a_bare_true_is_not_a_declaration() {
        // A declaration is always an object (ADR 0016). `true` is not a declaration and is
        // overwritten.
        let body =
            r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":true}}}}"#;
        let out = declared(body, &ServerStateProvider::notifications_only());
        assert_eq!(
            out["result"]["capabilities"]["experimental"]["serverStateProvider"],
            serde_json::json!({})
        );
    }

    #[test]
    fn never_overwrites_an_upstream_declaration() {
        // A guarantee the upstream really has (freshness) must not be hidden by a declaration
        // without guarantees.
        for body in [
            r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":{"freshness":{"fileChanges":["Changed"]}}}}}}"#,
            r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":{}}}}}"#,
        ] {
            assert_eq!(
                declare_server_state_provider(
                    body.as_bytes(),
                    &ServerStateProvider::notifications_only()
                ),
                UpstreamDeclares,
                "should pass through as the upstream's declaration: {body}"
            );
        }
    }

    #[test]
    fn overwrites_a_false_or_null_upstream_declaration() {
        // `serverStateProvider: false` means "not provided" (the boolean in spec chapter 5).
        // It is not a declaration, so it is overwritten with our own declaration.
        for body in [
            r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":false}}}}"#,
            r#"{"id":1,"result":{"capabilities":{"experimental":{"serverStateProvider":null}}}}"#,
        ] {
            let out = declared(body, &ServerStateProvider::notifications_only());
            assert_eq!(
                out["result"]["capabilities"]["experimental"]["serverStateProvider"],
                serde_json::json!({}),
                "false / null is not a declaration: {body}"
            );
        }
    }

    #[test]
    fn an_error_response_is_not_a_success() {
        // The handshake is not complete. The client may retry initialize.
        for body in [
            r#"{"id":1,"error":{"code":-32603,"message":"boom"}}"#,
            r#"{"id":1,"result":"not an object"}"#,
            "{not json",
        ] {
            assert_eq!(
                declare_server_state_provider(
                    body.as_bytes(),
                    &ServerStateProvider::notifications_only()
                ),
                NotASuccess,
                "must not be counted as a success response: {body}"
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
                declare_server_state_provider(
                    body.as_bytes(),
                    &ServerStateProvider::notifications_only()
                ),
                Unrewritable,
                "should be counted as unrewritable: {body}"
            );
        }
    }
}
