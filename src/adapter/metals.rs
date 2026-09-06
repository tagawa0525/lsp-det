//! The Metals mapping (M9, ADR 0019 decision F. research/metals-readiness-measurement.md).
//!
//! Metals has no readiness vocabulary of its own. It is synthesized from the `$/progress`
//! titles of the build import (measured with Metals 1.6.8 and scala-cli):
//!
//! - **readiness**: a begin of "… bspConfig", "Importing build", "Indexing", or "Compiling …"
//!   means `indexing`. `ready` needs no token of those titles open, plus a condition on the
//!   token that ended last: the initial import is complete only once an "Indexing" has ended
//!   (before that, the gaps between tokens are not ready: 10 s between "bspConfig" and
//!   "Importing build" on a cold cache), and an "Importing build" or "bspConfig" end is never
//!   ready by itself ("Indexing" follows it within milliseconds). After the initial import, a
//!   "Compiling …" end is ready: a changed source only recompiles (measured: no import round
//!   follows). Serena fills the gaps with a 3-second quiet period; this mapping needs no clock.
//!   "Loading presentation compiler" is request processing (1 ms) and is not looked at
//! - **prediction** (ADR 0014 addendum decision D): a `workspace/didChangeWatchedFiles` from
//!   the client predicts `indexing`, because the first progress begin follows the notification
//!   by 0.15-0.33 s. A changed source is reverted by the next "Compiling" (or "Indexing") end;
//!   a created or deleted source, or any change of a build file, changes the build and is
//!   reverted only by the next "Indexing" end (measured: Compiling, then Importing build and
//!   Indexing, with the fresh answer after the latter)
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

/// Versions for which conformance tests 7.2 (with the `workspace/symbol` count) and 7.3 item 1
/// were run against a real Metals and passed. Matched by exact equality against
/// `serverInfo.version`. When adding one, run `cargo test --test conformance -- --ignored metals`
/// against that version first (declaring a guarantee that cannot be kept violates spec 5.1).
///
/// Record of versions passed: 1.6.8 (nixpkgs, scala-cli 1.16.0, OpenJDK 21.0.12), 2026-09-06,
/// 3 consecutive runs.
pub const TESTED_VERSIONS: &[&str] = &["1.6.8"];

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

/// The kinds of token the import consists of.
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

/// The last component of a file URI (a Windows file URI can arrive `\`-separated).
fn file_name(uri: &str) -> &str {
    uri.rsplit(['/', '\\']).next().unwrap_or(uri)
}

/// A build definition. Changing one makes Metals re-import the build (scala-cli's
/// `project.scala`, sbt's `build.sbt` and `*.sbt`, Mill's `build.mill` / `build.sc`).
fn is_build_file(uri: &str) -> bool {
    let name = file_name(uri);
    name.ends_with(".sbt") || matches!(name, "project.scala" | "build.mill" | "build.sc")
}

/// A Scala source. Changing one makes Metals recompile.
fn is_source_file(uri: &str) -> bool {
    let name = file_name(uri);
    (name.ends_with(".scala") || name.ends_with(".sc")) && !is_build_file(uri)
}

pub struct MetalsAdapter {
    version_is_tested: bool,
    state: ServerState,
    /// Open tokens of the import phases.
    open: Vec<(Value, Phase)>,
    /// An "Indexing" has ended at least once (the initial import is complete).
    imported: bool,
    /// A build change is expected to be under way (predicted from a created / deleted source
    /// or a build file). Only an "Indexing" end clears it.
    pending_import: bool,
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
            imported: false,
            pending_import: false,
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
                if phase == Phase::Indexing {
                    self.imported = true;
                    self.pending_import = false;
                }
                let completes = match phase {
                    Phase::Indexing => true,
                    Phase::Compiling => self.imported && !self.pending_import,
                    Phase::Importing | Phase::BspConfig => false,
                };
                if self.open.is_empty() && completes {
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

    /// `coverage` over the whole workspace with no cap (7.2: 300 of 300 `workspace/symbol`
    /// results). `freshness` with no watched-file kind: `didChange` of an open document is
    /// incorporated by the presentation compiler (7.3 item 1), but the semanticdb index that
    /// answers for a file changed on disk is rebuilt after the "Compiling" end with no signal,
    /// and references for that file are empty in the meantime (measured window 0.1 s).
    fn guarantees(&self) -> ServerStateProvider {
        if self.version_is_tested {
            ServerStateProvider::workspace(&[], &[])
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

    /// A watched-file change predicts `indexing` (the measured gap before the first progress
    /// begin). A changed source is reverted by the next "Compiling" or "Indexing" end; a created
    /// or deleted source, or any change of a build file, only by the next "Indexing" end.
    fn observe_client(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() || view.method() != Some(WATCHED_FILES_METHOD) {
            return None;
        }
        #[derive(Deserialize)]
        struct Change {
            uri: String,
            #[serde(rename = "type")]
            kind: u8,
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
        let mut source_changed = false;
        let mut build_changed = false;
        for change in &envelope.params.changes {
            if is_build_file(&change.uri) {
                build_changed = true;
            } else if is_source_file(&change.uri) {
                // 1 Created, 2 Changed, 3 Deleted (LSP FileChangeType).
                if change.kind == 2 {
                    source_changed = true;
                } else {
                    build_changed = true;
                }
            }
        }
        if !source_changed && !build_changed {
            return None;
        }
        if build_changed {
            self.pending_import = true;
        }
        if self.state.readiness != Readiness::Ready {
            return None;
        }
        self.state.readiness = Readiness::Indexing;
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

    fn readiness_after(adapter: &mut MetalsAdapter, body: &str) -> Readiness {
        feed(adapter, body)
            .expect("a phase token moves the state")
            .readiness
    }

    #[test]
    fn the_initial_import_completes_only_with_an_indexing_end() {
        let mut m = MetalsAdapter::new();
        assert_eq!(
            readiness_after(&mut m, &progress("b", "begin", Some("scala-cli bspConfig"))),
            Readiness::Indexing
        );
        // The gap: no token open, but Indexing has never ended.
        assert_eq!(
            readiness_after(&mut m, &progress("b", "end", None)),
            Readiness::Indexing
        );
        feed(&mut m, &progress("i", "begin", Some("Importing build")));
        assert_eq!(
            readiness_after(&mut m, &progress("i", "end", None)),
            Readiness::Indexing,
            "Importing build alone does not complete the import"
        );
        feed(&mut m, &progress("x", "begin", Some("Indexing")));
        feed(&mut m, &progress("c", "begin", Some("Compiling fixture_1")));
        assert_eq!(
            readiness_after(&mut m, &progress("x", "end", None)),
            Readiness::Indexing,
            "Compiling is still open"
        );
        assert_eq!(
            readiness_after(&mut m, &progress("c", "end", None)),
            Readiness::Ready,
            "the import is complete and the compile ended"
        );
        // After the import, a compile on its own (a changed source) completes.
        feed(
            &mut m,
            &progress("c2", "begin", Some("Compiling fixture_1")),
        );
        assert_eq!(
            readiness_after(&mut m, &progress("c2", "end", None)),
            Readiness::Ready
        );
    }

    #[test]
    fn a_compile_before_the_first_indexing_end_is_not_ready() {
        let mut m = MetalsAdapter::new();
        feed(&mut m, &progress("c", "begin", Some("Compiling fixture_1")));
        assert_eq!(
            readiness_after(&mut m, &progress("c", "end", None)),
            Readiness::Indexing
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
    fn predicts_indexing_from_watched_files_only_when_ready() {
        let mut m = MetalsAdapter::new();
        let change = |name: &str, kind: u8| {
            format!(
                r#"{{"jsonrpc":"2.0","method":"workspace/didChangeWatchedFiles","params":{{"changes":[{{"uri":"file:///w/{name}","type":{kind}}}]}}}}"#
            )
        };
        let observe = |m: &mut MetalsAdapter, body: &str| {
            let view = peek(body.as_bytes()).unwrap();
            m.observe_client(&view, body.as_bytes())
        };
        assert!(
            observe(&mut m, &change("C.scala", 2)).is_none(),
            "not ready yet"
        );
        feed(&mut m, &progress("x", "begin", Some("Indexing")));
        feed(&mut m, &progress("x", "end", None));
        assert!(observe(&mut m, &change("README.md", 2)).is_none());
        // A changed source: reverted by the next compile.
        assert_eq!(
            observe(&mut m, &change("B.scala", 2)).unwrap().readiness,
            Readiness::Indexing
        );
        assert!(
            observe(&mut m, &change("C.scala", 2)).is_none(),
            "already indexing"
        );
        feed(&mut m, &progress("c", "begin", Some("Compiling fixture_1")));
        assert_eq!(
            readiness_after(&mut m, &progress("c", "end", None)),
            Readiness::Ready
        );
        // A created source or a build file: only the next Indexing end reverts it.
        for (name, kind) in [("C.scala", 1), ("project.scala", 2), ("build.sbt", 3)] {
            assert_eq!(
                observe(&mut m, &change(name, kind)).unwrap().readiness,
                Readiness::Indexing,
                "{name}"
            );
            feed(&mut m, &progress("c", "begin", Some("Compiling fixture_1")));
            assert_eq!(
                readiness_after(&mut m, &progress("c", "end", None)),
                Readiness::Indexing,
                "{name}: a compile does not complete a build change"
            );
            feed(&mut m, &progress("i", "begin", Some("Importing build")));
            feed(&mut m, &progress("i", "end", None));
            feed(&mut m, &progress("x", "begin", Some("Indexing")));
            assert_eq!(
                readiness_after(&mut m, &progress("x", "end", None)),
                Readiness::Ready,
                "{name}"
            );
        }
        assert!(is_build_file("file:///w/build.sbt"));
        assert!(is_build_file("file:///w/project.scala"));
        assert!(is_source_file("file://C:\\w\\A.scala"));
        assert!(!is_source_file("file:///w/project.scala"));
        assert!(!is_source_file("file:///w/notes.txt"));
    }

    #[test]
    fn declares_guarantees_only_for_tested_versions() {
        assert_eq!(
            MetalsAdapter::for_version(Some("1.6.8")).guarantees(),
            ServerStateProvider::workspace(&[], &[])
        );
        assert_eq!(
            MetalsAdapter::for_version(Some("1.5.0")).guarantees(),
            ServerStateProvider::notifications_only()
        );
        assert_eq!(
            MetalsAdapter::new().guarantees(),
            ServerStateProvider::notifications_only()
        );
    }
}
