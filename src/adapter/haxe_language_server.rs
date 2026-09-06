//! The mapping for haxe-language-server (M20, ADR 0019 decision F;
//! research/haxe-language-server-readiness-measurement.md).
//!
//! haxe-language-server (measured with vshaxe 2.34.2, Haxe 4.3.7):
//!
//! - **identity**: no `serverInfo`. It names itself only after the client sends
//!   `workspace/didChangeConfiguration` (which is also what starts the underlying compiler): a
//!   `window/logMessage` beginning with "Haxe Path: ". A client that never configures it never
//!   sees this log, and both axes stay `unknown` (its `references` answers `-32601` instead,
//!   which is an honest error, not a silent lie)
//! - **readiness**: `initializing` from the moment it is identified. `$/progress` reuses one
//!   title format, `"Haxe: " + name + "..."`, for both the startup work and per-request work,
//!   and the names are fixed and disjoint: the startup titles are "Haxe: Building Cache...",
//!   "Haxe: Parsing Classpaths...", and "Haxe: Building Refactoring Cache…..." (they run
//!   concurrently); the per-request titles ("Haxe: Collecting Diagnostics...", "Haxe:
//!   Performing Refactor Operation…....." and "…Rename Operation…....") are ignored. A begin of
//!   a startup title moves an already-`ready` server to `indexing`; an end that leaves no
//!   startup token open moves to `ready`. A request answered while a startup token is open is
//!   not a lie: it is queued by the compiler (`haxe --wait`) and answered in full only once the
//!   queue drains, so there is no partial or empty answer to hide
//! - **health**: a `window/showMessage` (type 1) starting with "Haxe version check failed" or
//!   "Invalid compiler argument" is `error` (the message is the body); the `window/logMessage`
//!   "Haxe connected!" is `ok` (the compiler came up); `haxe/haxeKeepsCrashing` is `error`
//! - **guarantees**: none (`{}`). The version never appears on the protocol (spec 8.2 item 5
//!   requires a tested version), and an open document's `didChange` is not incorporated into
//!   another file's `references` (only a `didSave` after writing to disk is)
//!
//! What upstream is missing: `serverInfo`; starting the compiler without requiring
//! `workspace/didChangeConfiguration`; incorporating an open document's edits into other
//! files' `references`.

use serde::Deserialize;
use serde_json::Value;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{Health, Readiness, ServerState, ServerStateProvider};

/// The name this mapping is selected by (the server has no `serverInfo.name`).
pub const SERVER_NAME: &str = "haxe-language-server";

const PROGRESS_METHOD: &str = "$/progress";
const LOG_MESSAGE_METHOD: &str = "window/logMessage";
const SHOW_MESSAGE_METHOD: &str = "window/showMessage";
const KEEPS_CRASHING_METHOD: &str = "haxe/haxeKeepsCrashing";
const PATH_LOG_PREFIX: &str = "Haxe Path: ";
const CONNECTED_LOG: &str = "Haxe connected!";
/// LSP `MessageType.Error`.
const SHOW_MESSAGE_ERROR: u8 = 1;
const VERSION_CHECK_FAILED_PREFIX: &str = "Haxe version check failed";
const INVALID_ARGUMENT_PREFIX: &str = "Invalid compiler argument";
const KEEPS_CRASHING_MESSAGE: &str = "Haxe keeps crashing";

/// The `$/progress` titles that make up the startup work (`"Haxe: " + name + "..."`). The same
/// format is reused for per-request progress, but those names ("Collecting Diagnostics",
/// "Performing Refactor Operation…", "Performing Rename Operation…") are disjoint from this
/// list and are therefore ignored.
const STARTUP_TITLES: &[&str] = &[
    "Haxe: Building Cache...",
    "Haxe: Parsing Classpaths...",
    "Haxe: Building Refactoring Cache\u{2026}...",
];

/// Whether a `window/logMessage` names haxe-language-server. Sent only after
/// `workspace/didChangeConfiguration` starts the compiler.
pub fn identity_from_log(message: &str) -> bool {
    message.starts_with(PATH_LOG_PREFIX)
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

#[derive(Deserialize)]
struct ShowMessageParams {
    #[serde(rename = "type")]
    kind: u8,
    message: String,
}

pub struct HaxeLanguageServerAdapter {
    state: ServerState,
    /// Open tokens whose title is a startup title.
    open: Vec<Value>,
}

impl Default for HaxeLanguageServerAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl HaxeLanguageServerAdapter {
    pub fn new() -> Self {
        HaxeLanguageServerAdapter {
            state: ServerState::initializing(),
            open: Vec::new(),
        }
    }

    fn on_progress(&mut self, params: ProgressParams) -> Option<ServerState> {
        let ProgressParams { token, value } = params;
        match value.kind.as_str() {
            "begin" => {
                if !STARTUP_TITLES.contains(&value.title.as_deref()?) {
                    return None;
                }
                self.open.push(token);
                if self.state.readiness != Readiness::Ready {
                    return None;
                }
                self.state.readiness = Readiness::Indexing;
                Some(self.state.clone())
            }
            "end" => {
                let index = self.open.iter().position(|t| *t == token)?;
                self.open.remove(index);
                if !self.open.is_empty() || self.state.readiness == Readiness::Ready {
                    return None;
                }
                self.state.readiness = Readiness::Ready;
                Some(self.state.clone())
            }
            _ => None,
        }
    }

    fn set_health(&mut self, health: Health, message: Option<String>) -> Option<ServerState> {
        let next = ServerState {
            health,
            message,
            ..self.state.clone()
        };
        if next == self.state {
            return None;
        }
        self.state = next;
        Some(self.state.clone())
    }

    fn on_log_message(&mut self, params: LogMessageParams) -> Option<ServerState> {
        if params.message != CONNECTED_LOG {
            return None;
        }
        self.set_health(Health::Ok, None)
    }

    fn on_show_message(&mut self, params: ShowMessageParams) -> Option<ServerState> {
        if params.kind != SHOW_MESSAGE_ERROR {
            return None;
        }
        if params.message.starts_with(VERSION_CHECK_FAILED_PREFIX)
            || params.message.starts_with(INVALID_ARGUMENT_PREFIX)
        {
            self.set_health(Health::Error, Some(params.message))
        } else {
            None
        }
    }
}

impl Mapping for HaxeLanguageServerAdapter {
    /// `initializing` from the moment the mapping is chosen (the "Haxe Path: " log already
    /// happened by then).
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// Never a guarantee (see the module documentation): the version never appears on the
    /// protocol, and an open document's edits do not reach other files' `references`.
    fn guarantees(&self) -> ServerStateProvider {
        ServerStateProvider::notifications_only()
    }

    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() {
            return None;
        }
        match view.method()? {
            PROGRESS_METHOD => {
                #[derive(Deserialize)]
                struct Envelope {
                    params: ProgressParams,
                }
                let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
                self.on_progress(envelope.params)
            }
            LOG_MESSAGE_METHOD => {
                #[derive(Deserialize)]
                struct Envelope {
                    params: LogMessageParams,
                }
                let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
                self.on_log_message(envelope.params)
            }
            SHOW_MESSAGE_METHOD => {
                #[derive(Deserialize)]
                struct Envelope {
                    params: ShowMessageParams,
                }
                let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
                self.on_show_message(envelope.params)
            }
            KEEPS_CRASHING_METHOD => {
                self.set_health(Health::Error, Some(KEEPS_CRASHING_MESSAGE.to_string()))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;

    fn progress(token: &str, kind: &str, title: Option<&str>) -> String {
        let title = title
            .map(|t| format!(r#","title":"{t}""#))
            .unwrap_or_default();
        format!(
            r#"{{"jsonrpc":"2.0","method":"$/progress","params":{{"token":"{token}","value":{{"kind":"{kind}"{title}}}}}}}"#
        )
    }

    fn log_message(message: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"window/logMessage","params":{{"type":4,"message":"{message}"}}}}"#
        )
    }

    fn show_message(kind: u8, message: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"window/showMessage","params":{{"type":{kind},"message":"{message}"}}}}"#
        )
    }

    fn feed(adapter: &mut HaxeLanguageServerAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    #[test]
    fn recognizes_the_haxe_path_log_and_nothing_else() {
        assert!(identity_from_log("Haxe Path: haxe"));
        assert!(!identity_from_log("Using --server-connect 127.0.0.1:6000"));
        assert!(!identity_from_log("Done."));
    }

    #[test]
    fn starts_initializing_and_declares_no_guarantee() {
        let m = HaxeLanguageServerAdapter::new();
        assert_eq!(m.initial_state(), ServerState::initializing());
        assert_eq!(m.guarantees(), ServerStateProvider::notifications_only());
    }

    #[test]
    fn concurrent_startup_tokens_only_complete_when_all_have_ended() {
        let mut m = HaxeLanguageServerAdapter::new();
        assert!(
            feed(
                &mut m,
                &progress("0", "begin", Some("Haxe: Building Cache..."))
            )
            .is_none(),
            "a begin does not move an already-initializing server"
        );
        assert!(
            feed(
                &mut m,
                &progress("1", "begin", Some("Haxe: Parsing Classpaths..."))
            )
            .is_none()
        );
        assert!(
            feed(&mut m, &progress("0", "end", None)).is_none(),
            "\"Parsing Classpaths\" is still open"
        );
        assert!(
            feed(
                &mut m,
                &progress(
                    "2",
                    "begin",
                    Some("Haxe: Building Refactoring Cache\u{2026}...")
                )
            )
            .is_none()
        );
        assert!(
            feed(&mut m, &progress("2", "end", None)).is_none(),
            "\"Parsing Classpaths\" is still open"
        );
        let state = feed(&mut m, &progress("1", "end", None)).expect("the last token ended");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn a_rebuild_after_ready_goes_through_indexing() {
        let mut m = HaxeLanguageServerAdapter::new();
        feed(
            &mut m,
            &progress("0", "begin", Some("Haxe: Building Cache...")),
        );
        feed(&mut m, &progress("0", "end", None));
        let state = feed(
            &mut m,
            &progress("3", "begin", Some("Haxe: Building Cache...")),
        )
        .expect("a begin while ready moves to indexing");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state = feed(&mut m, &progress("3", "end", None)).expect("back to ready");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn per_request_progress_is_ignored() {
        let mut m = HaxeLanguageServerAdapter::new();
        assert!(
            feed(
                &mut m,
                &progress("d", "begin", Some("Haxe: Collecting Diagnostics..."))
            )
            .is_none()
        );
        assert!(feed(&mut m, &progress("d", "end", None)).is_none());
        assert!(
            feed(
                &mut m,
                &progress(
                    "r",
                    "begin",
                    Some("Haxe: Performing Refactor Operation\u{2026}.....")
                )
            )
            .is_none()
        );
        assert_eq!(m.state.readiness, Readiness::Initializing);
    }

    #[test]
    fn health_from_showmessage_and_the_connected_log() {
        let mut m = HaxeLanguageServerAdapter::new();
        let state = feed(
            &mut m,
            &show_message(
                1,
                "Haxe version check failed: \\\"/bin/sh: haxe: command not found\\\"",
            ),
        )
        .expect("an error showMessage moves health");
        assert_eq!(state.health, Health::Error);
        assert_eq!(
            state.message.as_deref(),
            Some("Haxe version check failed: \"/bin/sh: haxe: command not found\"")
        );
        let state = feed(&mut m, &log_message(CONNECTED_LOG)).expect("connected clears health");
        assert_eq!(state.health, Health::Ok);
        assert_eq!(state.message, None);
        // An info showMessage is not a failure.
        assert!(feed(&mut m, &show_message(3, "Checking memory...")).is_none());
    }

    #[test]
    fn keeps_crashing_is_error_health() {
        let mut m = HaxeLanguageServerAdapter::new();
        let body = r#"{"jsonrpc":"2.0","method":"haxe/haxeKeepsCrashing","params":null}"#;
        let state = feed(&mut m, body).expect("the crash notification moves health");
        assert_eq!(state.health, Health::Error);
        assert_eq!(state.message.as_deref(), Some(KEEPS_CRASHING_MESSAGE));
    }
}
