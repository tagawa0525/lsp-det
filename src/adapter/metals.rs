//! The Metals mapping (M9, ADR 0019 decision F. research/metals-readiness-measurement.md).
//!
//! Metals has no readiness vocabulary of its own. It is synthesized from the `$/progress`
//! titles of the build import (measured with Metals 1.6.8 and scala-cli):
//!
//! - **readiness**: a begin of "… bspConfig", "Importing build", "Indexing", or "Compiling …"
//!   means `indexing`. `ready` needs two things: no token of those titles is open, AND the
//!   last token that ended was "Indexing". The second condition is what carries the mapping
//!   across the measured gaps in which no token is open while the import is still under way
//!   (10 s between "bspConfig" and "Importing build" on a cold cache; 0.3 s between the end of
//!   an "Indexing" and the "Importing build" of a re-import). Serena fills those gaps with a
//!   3-second quiet period; this mapping needs no clock. "Loading presentation compiler" is
//!   request processing (1 ms) and is not looked at
//! - **prediction** (ADR 0014 addendum decision D): a `workspace/didChangeWatchedFiles` from
//!   the client for a Scala source or a build file predicts `indexing`, because the first
//!   progress begin follows the notification by 0.15-0.33 s. The next "Indexing" end reverts it
//! - **health**: the `level` of `metals/status` with `statusType: "module"` (sent regardless of
//!   `initializationOptions`): `error` → error with the `tooltip` (or `text`) as the message,
//!   `warn` → warning, `info` → ok. `unknown` until the first one. The `statusType: "metals"`
//!   status bar text (" Indexing complete!") is not looked at: it is also sent right before a
//!   re-import
//!
//! `coverage` / `freshness` are declared only for versions ([`TESTED_VERSIONS`]) for which
//! conformance tests 7.2 / 7.3 were run against a real Metals and passed (spec 8.2 item 5).

use serde::Deserialize;
use serde_json::Value;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{Health, Readiness, ServerState, ServerStateProvider};

const PROGRESS_METHOD: &str = "$/progress";
const STATUS_METHOD: &str = "metals/status";
const WATCHED_FILES_METHOD: &str = "workspace/didChangeWatchedFiles";
const INDEXING_TITLE: &str = "Indexing";
const IMPORTING_TITLE: &str = "Importing build";
const COMPILING_PREFIX: &str = "Compiling ";
const BSP_CONFIG_SUFFIX: &str = " bspConfig";

/// Versions for which conformance tests 7.2 / 7.3 were run against a real Metals and passed.
/// Matched by exact equality against `serverInfo.version`. Empty until the tests have passed
/// (declaring a guarantee that cannot be kept violates spec 5.1).
pub const TESTED_VERSIONS: &[&str] = &[];

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
}

#[derive(Debug, Deserialize)]
struct StatusParams {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    level: Option<String>,
    #[serde(default)]
    tooltip: Option<String>,
    #[serde(default, rename = "statusType")]
    status_type: Option<String>,
}

/// The kinds of token the import consists of. Only "Indexing" closes a round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    BspConfig,
    Importing,
    Indexing,
    Compiling,
}

fn phase_of(title: &str) -> Option<Phase> {
    if title == INDEXING_TITLE {
        Some(Phase::Indexing)
    } else if title == IMPORTING_TITLE {
        Some(Phase::Importing)
    } else if title.starts_with(COMPILING_PREFIX) {
        Some(Phase::Compiling)
    } else if title.ends_with(BSP_CONFIG_SUFFIX) {
        Some(Phase::BspConfig)
    } else {
        None
    }
}

/// A Scala source or a build file. Judged by the last component of the URI (a Windows file URI
/// can arrive `\`-separated).
fn is_watched_file(uri: &str) -> bool {
    let name = uri.rsplit(['/', '\\']).next().unwrap_or(uri);
    name.ends_with(".scala")
        || name.ends_with(".sc")
        || name.ends_with(".sbt")
        || matches!(name, "build.mill" | "build.sc")
}

pub struct MetalsAdapter {
    version_is_tested: bool,
    state: ServerState,
    /// Open tokens of the import phases.
    open: Vec<(Value, Phase)>,
    /// Whether the last token that ended was "Indexing". False until the first one ends, and
    /// false again after any other phase ends (a re-import is under way).
    last_ended_indexing: bool,
}

impl Default for MetalsAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl MetalsAdapter {
    pub fn new() -> Self {
        Self::for_version(None)
    }

    pub fn for_version(version: Option<&str>) -> Self {
        let version_is_tested = version.is_some_and(|v| TESTED_VERSIONS.contains(&v.trim()));
        MetalsAdapter {
            version_is_tested,
            state: ServerState::initializing(),
            open: Vec::new(),
            last_ended_indexing: false,
        }
    }

    fn on_progress(&mut self, params: ProgressParams) -> Option<ServerState> {
        let ProgressParams { token, value } = params;
        match value.kind.as_str() {
            "begin" => {
                let phase = phase_of(value.title.as_deref()?)?;
                self.open.push((token, phase));
                self.state.readiness = Readiness::Indexing;
            }
            "end" => {
                let index = self.open.iter().position(|(t, _)| *t == token)?;
                let (_, phase) = self.open.remove(index);
                self.last_ended_indexing = phase == Phase::Indexing;
                if self.open.is_empty() && self.last_ended_indexing {
                    self.state.readiness = Readiness::Ready;
                }
            }
            _ => return None,
        }
        Some(self.state.clone())
    }

    fn on_status(&mut self, params: StatusParams) -> Option<ServerState> {
        if params.status_type.as_deref() != Some("module") {
            return None;
        }
        let health = match params.level.as_deref().unwrap_or("info") {
            "error" => Health::Error,
            "warn" => Health::Warning,
            _ => Health::Ok,
        };
        self.state.health = health;
        self.state.message = if health == Health::Ok {
            None
        } else {
            params
                .tooltip
                .filter(|t| !t.trim().is_empty())
                .or(params.text)
                .map(|m| m.trim().to_string())
        };
        Some(self.state.clone())
    }
}

impl Mapping for MetalsAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    fn guarantees(&self) -> ServerStateProvider {
        if self.version_is_tested {
            ServerStateProvider::workspace(&[], &crate::state::ALL_FILE_CHANGES)
        } else {
            ServerStateProvider::notifications_only()
        }
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
            STATUS_METHOD => {
                #[derive(Deserialize)]
                struct Envelope {
                    params: StatusParams,
                }
                let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
                self.on_status(envelope.params)
            }
            _ => None,
        }
    }

    /// A watched-file change on a Scala source or a build file predicts `indexing` until the
    /// next "Indexing" end (the measured gap before the first progress begin).
    fn observe_client(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() || view.method() != Some(WATCHED_FILES_METHOD) {
            return None;
        }
        #[derive(Deserialize)]
        struct Change {
            uri: String,
        }
        #[derive(Deserialize)]
        struct Params {
            changes: Vec<Change>,
        }
        #[derive(Deserialize)]
        struct Envelope {
            params: Params,
        }
        let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
        if !envelope
            .params
            .changes
            .iter()
            .any(|c| is_watched_file(&c.uri))
        {
            return None;
        }
        if self.state.readiness != Readiness::Ready {
            return None;
        }
        self.state.readiness = Readiness::Indexing;
        self.last_ended_indexing = false;
        Some(self.state.clone())
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

    fn feed(adapter: &mut MetalsAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    #[test]
    fn ready_needs_no_open_token_and_an_indexing_end() {
        let mut m = MetalsAdapter::new();
        assert_eq!(
            feed(&mut m, &progress("b", "begin", Some("scala-cli bspConfig")))
                .unwrap()
                .readiness,
            Readiness::Indexing
        );
        // The gap: no token open, but Indexing has never ended.
        assert_eq!(
            feed(&mut m, &progress("b", "end", None)).unwrap().readiness,
            Readiness::Indexing
        );
        feed(&mut m, &progress("i", "begin", Some("Importing build")));
        feed(&mut m, &progress("i", "end", None));
        feed(&mut m, &progress("x", "begin", Some("Indexing")));
        feed(&mut m, &progress("c", "begin", Some("Compiling fixture_1")));
        assert_eq!(
            feed(&mut m, &progress("x", "end", None)).unwrap().readiness,
            Readiness::Indexing,
            "Compiling is still open"
        );
        assert_eq!(
            feed(&mut m, &progress("c", "end", None)).unwrap().readiness,
            Readiness::Indexing,
            "the last ended token is Compiling"
        );
        feed(&mut m, &progress("x2", "begin", Some("Indexing")));
        assert_eq!(
            feed(&mut m, &progress("x2", "end", None))
                .unwrap()
                .readiness,
            Readiness::Ready
        );
    }

    #[test]
    fn presentation_compiler_and_unknown_titles_are_ignored() {
        let mut m = MetalsAdapter::new();
        assert!(
            feed(
                &mut m,
                &progress("p", "begin", Some("Loading presentation compiler"))
            )
            .is_none()
        );
        assert!(feed(&mut m, &progress("p", "end", None)).is_none());
        assert!(feed(&mut m, &progress("d", "begin", Some("Running doctor"))).is_none());
    }

    #[test]
    fn health_from_the_module_status_level() {
        let mut m = MetalsAdapter::new();
        let status = |level: &str, text: &str, tooltip: &str, kind: &str| {
            format!(
                r#"{{"jsonrpc":"2.0","method":"metals/status","params":{{"text":"{text}","level":"{level}","show":true,"tooltip":"{tooltip}","statusType":"{kind}"}}}}"#
            )
        };
        assert!(
            feed(&mut m, &status("info", " Indexing complete!", "", "metals")).is_none(),
            "the status bar text is not health"
        );
        let s = feed(&mut m, &status("info", "importing...", "", "module")).unwrap();
        assert_eq!(s.health, Health::Ok);
        let s = feed(
            &mut m,
            &status(
                "error",
                "no target",
                "No build target for file found.",
                "module",
            ),
        )
        .unwrap();
        assert_eq!(s.health, Health::Error);
        assert_eq!(
            s.message.as_deref(),
            Some("No build target for file found.")
        );
        let s = feed(&mut m, &status("warn", "fixture", "", "module")).unwrap();
        assert_eq!(s.health, Health::Warning);
        assert_eq!(s.message.as_deref(), Some("fixture"));
    }

    #[test]
    fn predicts_indexing_from_watched_scala_and_build_files_only_when_ready() {
        let mut m = MetalsAdapter::new();
        let changed = |name: &str| {
            format!(
                r#"{{"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{{"changes":[{{"uri":"file:///w/{name}","type":2}}]}}}}"#
            )
        };
        let observe = |m: &mut MetalsAdapter, body: &str| {
            let view = peek(body.as_bytes()).unwrap();
            m.observe_client(&view, body.as_bytes())
        };
        assert!(
            observe(&mut m, &changed("C.scala")).is_none(),
            "not ready yet"
        );
        feed(&mut m, &progress("x", "begin", Some("Indexing")));
        feed(&mut m, &progress("x", "end", None));
        assert!(observe(&mut m, &changed("README.md")).is_none());
        assert_eq!(
            observe(&mut m, &changed("project.scala"))
                .unwrap()
                .readiness,
            Readiness::Indexing
        );
        assert!(
            observe(&mut m, &changed("C.scala")).is_none(),
            "already indexing"
        );
        feed(&mut m, &progress("c", "begin", Some("Compiling fixture_1")));
        assert_eq!(
            feed(&mut m, &progress("c", "end", None)).unwrap().readiness,
            Readiness::Indexing
        );
        feed(&mut m, &progress("x2", "begin", Some("Indexing")));
        assert_eq!(
            feed(&mut m, &progress("x2", "end", None))
                .unwrap()
                .readiness,
            Readiness::Ready
        );
        assert!(is_watched_file("file:///w/build.sbt"));
        assert!(is_watched_file("file://C:\\w\\A.scala"));
        assert!(!is_watched_file("file:///w/notes.txt"));
    }

    #[test]
    fn declares_guarantees_only_for_tested_versions() {
        assert_eq!(
            MetalsAdapter::for_version(Some("1.6.8")).guarantees(),
            ServerStateProvider::notifications_only(),
            "no version has passed 7.2 / 7.3 yet"
        );
    }
}
