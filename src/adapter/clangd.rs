//! The mapping for clangd (M24, ADR 0020 decision C row for clangd;
//! research/clangd-readiness-measurement.md; the compilation-database probe below is the ADR
//! 0020 addendum 2026-09-09).
//!
//! Identified by `serverInfo.name` "clangd" (case-insensitive); the version is the whole
//! `serverInfo.version` string ("clangd version 21.1.8 linux x86_64-unknown-linux-gnu" for the
//! tested build). The platform is baked into that string, so a build for a different platform
//! at the same clangd release is a different, untested version.
//!
//! - **readiness**: starts `initializing`. LLVM's source (`ClangdLSPServer.cpp`,
//!   `onBackgroundIndexProgress`, added by D73218) sends `window/workDoneProgress/create` +
//!   `$/progress` with the fixed token `"backgroundIndexProgress"` (title "indexing") only once
//!   a compilation database (`compile_commands.json`) is found at the first `didOpen`: begin ->
//!   `indexing`, `report` is ignored, end -> `ready`. The pair repeats whenever the background
//!   index queue goes from empty to non-empty again (a later begin while `ready` -> `indexing`,
//!   its end -> `ready`). Other tokens are ignored
//! - **the compilation-database probe** (`observe_client`, ADR 0020 addendum 2026-09-09):
//!   without a database the background-index token never arrives at all, and this mapping
//!   cannot tell "no database" from "not yet begun" by waiting (spec chapter 6 item 6 -- there
//!   is no time-based escape hatch). So, at the first `textDocument/didOpen` only, it looks
//!   where clangd itself looks (`GlobalCompilationDatabase.cpp`,
//!   `DirectoryBasedGlobalCompilationDatabase`): from the opened file's directory up to the
//!   filesystem root, checking `compile_commands.json`, `build/compile_commands.json`, and
//!   `compile_flags.txt` at each level -- or, when the upstream was launched with
//!   `--compile-commands-dir=<dir>` (learned through `Mapping::learn_upstream_arguments`), only
//!   `<dir>/compile_commands.json`. Found -> nothing changes (`readiness` stays
//!   `initializing`; the observer's own hold still covers the measured ~2 ms gap to the first
//!   begin). Not found -> `readiness` becomes `unknown` (spec 8.2 item 3) and nothing is held,
//!   so a cross-file request from a client that does not declare this protocol is forwarded
//!   right away instead of waiting forever. The judgment is made once: a second and later
//!   `didOpen` is not looked at. A begin at any later time still moves `readiness` to
//!   `indexing` as usual, even after this probe settled on `unknown`. A database named only in
//!   `.clangd`'s `CompileFlags.CompilationDatabase` is not reproduced (that would need parsing
//!   YAML -- no new dependency, ADR 0005 -- and a wrong guess here only costs a hold, never a
//!   wrong answer), so such a workspace starts `unknown` too
//! - **no other prediction**: during begin..end, `references` answers an empty array and then
//!   a growing partial set as the index fills in (measured on a 402-file fixture: 0 -> 17 ->
//!   117 -> 217 -> 316 -> 400) -- the silent lie this protocol exists to remove, and exactly
//!   what the begin/end hold fixes. But a `didChange` on an open document has a measured 40-80
//!   ms stale window with no signal marking its end, and neither
//!   `workspace/didChangeWatchedFiles` (clangd never registers it) nor any on-disk change is
//!   ever incorporated, so there is nothing further to predict
//! - **health**: no signal. `unknown` (spec 8.2 item 3)
//!
//! `coverage: {scope: "workspace", incomplete: {}}` is declared only for versions
//! ([`TESTED_VERSIONS`]) for which conformance tests 7.1 / 7.2 were run against a real clangd
//! and passed (spec 8.2 item 5). No `freshness` is ever declared: the stale window after
//! `didChange` has no completion signal, and on-disk changes are never incorporated (7.3 is not
//! applicable to this mapping and is not written as a conformance test).

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::Mapping;
use crate::peek::MessageView;
use crate::state::{Readiness, ServerState, ServerStateProvider};
use crate::uri::uri_to_path;

/// The name clangd calls itself in `InitializeResult.serverInfo.name`, already lowercased for
/// the case-insensitive comparison [`super::select`] does.
pub const SERVER_NAME: &str = "clangd";

const PROGRESS_METHOD: &str = "$/progress";
const DID_OPEN_METHOD: &str = "textDocument/didOpen";
/// The fixed token of the background-index progress (`ClangdLSPServer.cpp`,
/// `onBackgroundIndexProgress`).
const BACKGROUND_INDEX_TOKEN: &str = "backgroundIndexProgress";
/// clangd's own flag for scoping its database search to one directory (`ClangdMain.cpp`).
const COMPILE_COMMANDS_DIR_FLAG: &str = "--compile-commands-dir";
const COMPILE_COMMANDS_JSON: &str = "compile_commands.json";
const COMPILE_FLAGS_TXT: &str = "compile_flags.txt";

/// `--compile-commands-dir=<dir>` or `--compile-commands-dir <dir>` from the upstream's own
/// launch arguments (ADR 0020 addendum 2026-09-09). The first occurrence wins. `None` if the
/// flag is not given.
fn compile_commands_dir_argument(args: &[String]) -> Option<PathBuf> {
    for (i, arg) in args.iter().enumerate() {
        if let Some(value) = arg.strip_prefix("--compile-commands-dir=") {
            return Some(PathBuf::from(value));
        }
        if arg == COMPILE_COMMANDS_DIR_FLAG {
            return args.get(i + 1).map(PathBuf::from);
        }
    }
    None
}

/// Whether `dir`, or one of its ancestors walking up to the filesystem root, has a compilation
/// database -- clangd's own search order and names
/// (`DirectoryBasedGlobalCompilationDatabase::getCompileCommand` in
/// `GlobalCompilationDatabase.cpp`): `compile_commands.json`, `build/compile_commands.json`,
/// `compile_flags.txt`, checked at each level before moving up.
fn database_reachable_from(dir: &Path) -> bool {
    let mut current = Some(dir);
    while let Some(d) = current {
        if d.join(COMPILE_COMMANDS_JSON).is_file()
            || d.join("build").join(COMPILE_COMMANDS_JSON).is_file()
            || d.join(COMPILE_FLAGS_TXT).is_file()
        {
            return true;
        }
        current = d.parent();
    }
    false
}

/// Versions for which conformance tests 7.1 / 7.2 were run against a real clangd and passed.
/// Matched by exact equality against `serverInfo.version`, which includes the platform
/// (`"clangd version 21.1.8 linux x86_64-unknown-linux-gnu"`); a build for another platform is
/// a different, untested version even at the same clangd release. No guarantee is declared for
/// a version not in the list. When adding one, run
/// `cargo test --test conformance -- --ignored clangd` against that version first (declaring a
/// guarantee that cannot be kept violates spec 5.1).
///
/// Record of versions passed: "clangd version 21.1.8 linux x86_64-unknown-linux-gnu" (nixpkgs
/// `clang-tools` 21.1.8, flake.nix `servers`), 2026-09-07.
pub const TESTED_VERSIONS: &[&str] = &["clangd version 21.1.8 linux x86_64-unknown-linux-gnu"];

#[derive(Deserialize)]
struct ProgressParams {
    token: String,
    value: ProgressValue,
}

#[derive(Deserialize)]
struct ProgressValue {
    kind: String,
}

pub struct ClangdAdapter {
    /// Whether the announced version is in [`TESTED_VERSIONS`]. The condition for declaring a
    /// guarantee.
    version_is_tested: bool,
    state: ServerState,
    /// Whether the first `didOpen`'s compilation-database probe has already run (ADR 0020
    /// addendum 2026-09-09). The judgment is made once; a later `didOpen` is not read.
    database_probed: bool,
    /// `--compile-commands-dir=<dir>` learned from the upstream's own launch arguments
    /// (`Mapping::learn_upstream_arguments`), if given. Scopes the probe to that directory's
    /// `compile_commands.json` only, mirroring clangd's own
    /// `DirectoryBasedGlobalCompilationDatabase`.
    compile_commands_dir: Option<PathBuf>,
}

impl Default for ClangdAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl ClangdAdapter {
    /// For a clangd that does not announce a version. Declares no guarantee.
    pub fn new() -> Self {
        Self::for_version(None)
    }

    /// Looks at `serverInfo.version` and declares a guarantee if it is a tested version.
    pub fn for_version(version: Option<&str>) -> Self {
        let version_is_tested = version.is_some_and(|v| TESTED_VERSIONS.contains(&v));
        ClangdAdapter {
            version_is_tested,
            state: ServerState::initializing(),
            database_probed: false,
            compile_commands_dir: None,
        }
    }
}

impl Mapping for ClangdAdapter {
    fn initial_state(&self) -> ServerState {
        ServerState::initializing()
    }

    /// The guarantee to declare (spec chapter 5). Declared only for [`TESTED_VERSIONS`] (spec
    /// 8.2 item 5): the observer's begin/end hold on `$/progress` keeps every `references`
    /// answer complete once `ready` (verified by conformance tests 7.1 / 7.2 against a real
    /// server). No `freshness`: neither a `didChange`'s completion nor any on-disk change has a
    /// signal.
    fn guarantees(&self) -> ServerStateProvider {
        if self.version_is_tested {
            ServerStateProvider::coverage_only(&[])
        } else {
            ServerStateProvider::notifications_only()
        }
    }

    fn interpret(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if !view.is_notification() || view.method() != Some(PROGRESS_METHOD) {
            return None;
        }
        #[derive(Deserialize)]
        struct Envelope {
            params: ProgressParams,
        }
        let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
        if envelope.params.token != BACKGROUND_INDEX_TOKEN {
            return None;
        }
        match envelope.params.value.kind.as_str() {
            "begin" => self.state.readiness = Readiness::Indexing,
            "end" => self.state.readiness = Readiness::Ready,
            // report: nothing this mapping reads from (spec chapter 8's rule against
            // per-request progress does not apply here -- this is a workspace-wide index, not a
            // per-request one -- but the percentage itself carries no boundary this mapping can
            // use).
            _ => return None,
        }
        Some(self.state.clone())
    }

    /// `--compile-commands-dir=<dir>` (or `--compile-commands-dir <dir>`) from the arguments
    /// lsp-det itself launched the upstream with (ADR 0020 addendum 2026-09-09).
    fn learn_upstream_arguments(&mut self, args: &[String]) {
        self.compile_commands_dir = compile_commands_dir_argument(args);
    }

    /// The compilation-database probe (ADR 0020 addendum 2026-09-09), read only from the first
    /// `textDocument/didOpen`. See the module documentation for the search rule.
    fn observe_client(&mut self, view: &MessageView, body: &[u8]) -> Option<ServerState> {
        if self.database_probed {
            return None;
        }
        if !view.is_notification() || view.method() != Some(DID_OPEN_METHOD) {
            return None;
        }
        #[derive(Deserialize)]
        struct Document {
            uri: String,
        }
        #[derive(Deserialize)]
        struct Params {
            #[serde(rename = "textDocument")]
            text_document: Document,
        }
        #[derive(Deserialize)]
        struct Envelope {
            params: Params,
        }
        let envelope = serde_json::from_slice::<Envelope>(body).ok()?;
        let path = uri_to_path(&envelope.params.text_document.uri)?;
        let dir = path.parent()?;
        // Only a didOpen that names a real path is the probe; one that does not (an untitled
        // buffer, a non-file scheme) teaches nothing and leaves the probe for the next didOpen.
        self.database_probed = true;
        let found = match &self.compile_commands_dir {
            Some(cdb_dir) => cdb_dir.join(COMPILE_COMMANDS_JSON).is_file(),
            None => database_reachable_from(dir),
        };
        if found {
            return None;
        }
        self.state.readiness = Readiness::Unknown;
        Some(self.state.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peek::peek;
    use crate::state::Health;
    use crate::uri::path_to_uri;

    fn progress(token: &str, kind: &str) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"$/progress","params":{{"token":"{token}","value":{{"kind":"{kind}"}}}}}}"#
        )
    }

    fn feed(adapter: &mut ClangdAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.interpret(&view, body.as_bytes())
    }

    #[test]
    fn starts_initializing() {
        let m = ClangdAdapter::new();
        assert_eq!(m.state.readiness, Readiness::Initializing);
        assert_eq!(m.state.health, Health::Unknown);
    }

    #[test]
    fn a_begin_of_the_background_index_token_is_indexing_and_its_end_is_ready() {
        let mut m = ClangdAdapter::new();
        let state = feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "begin"))
            .expect("a begin of the background index token is a signal");
        assert_eq!(state.readiness, Readiness::Indexing);
        assert_eq!(state.health, Health::Unknown);
        let state = feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "end"))
            .expect("the matching end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
        assert_eq!(m.state.readiness, Readiness::Ready);
    }

    #[test]
    fn report_is_ignored() {
        let mut m = ClangdAdapter::new();
        feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "begin"));
        assert!(
            feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "report")).is_none(),
            "report must not be read"
        );
        assert_eq!(
            m.state.readiness,
            Readiness::Indexing,
            "a report must not move readiness away from indexing"
        );
    }

    #[test]
    fn a_later_begin_after_ready_reindexes_and_its_end_is_ready_again() {
        let mut m = ClangdAdapter::new();
        feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "begin"));
        feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "end"));
        assert_eq!(m.state.readiness, Readiness::Ready);
        let state = feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "begin"))
            .expect("a begin while ready (the queue non-empty again) is a signal");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state =
            feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "end")).expect("its end is a signal");
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn ignores_other_tokens() {
        let mut m = ClangdAdapter::new();
        assert!(feed(&mut m, &progress("some-other-token", "begin")).is_none());
        assert!(feed(&mut m, &progress("some-other-token", "end")).is_none());
        assert_eq!(m.state.readiness, Readiness::Initializing);
    }

    #[test]
    fn ignores_a_progress_that_happens_to_use_the_token_as_a_request() {
        let mut m = ClangdAdapter::new();
        let as_request = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"$/progress","params":{{"token":"{BACKGROUND_INDEX_TOKEN}","value":{{"kind":"begin"}}}}}}"#
        );
        let view = peek(as_request.as_bytes()).unwrap();
        assert!(m.interpret(&view, as_request.as_bytes()).is_none());
        assert_eq!(m.state.readiness, Readiness::Initializing);
    }

    #[test]
    fn health_stays_unknown_regardless_of_readiness() {
        let mut m = ClangdAdapter::new();
        assert_eq!(
            feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "begin"))
                .unwrap()
                .health,
            Health::Unknown
        );
        assert_eq!(
            feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "end"))
                .unwrap()
                .health,
            Health::Unknown
        );
    }

    #[test]
    fn without_any_signal_readiness_stays_initializing() {
        // Before the compilation-database probe runs (no didOpen observed yet), readiness is
        // still the starting `initializing` -- the probe only ever moves it to `unknown`, never
        // the reverse.
        let m = ClangdAdapter::new();
        assert_eq!(m.state.readiness, Readiness::Initializing);
    }

    // --- Compilation-database probe (ADR 0020 addendum 2026-09-09) -------------------------

    fn observe(adapter: &mut ClangdAdapter, body: &str) -> Option<ServerState> {
        let view = peek(body.as_bytes()).expect("test bodies are valid JSON");
        adapter.observe_client(&view, body.as_bytes())
    }

    fn did_open(path: &std::path::Path) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{{"textDocument":{{"uri":"{}","languageId":"cpp","version":1,"text":""}}}}}}"#,
            path_to_uri(path)
        )
    }

    /// A unique temporary directory tree for one test, removed on drop (no dependency added;
    /// `std::env::temp_dir()` + a name unique to the test and the process, as `NextflowAdapter`'s
    /// tests do).
    struct Fixture {
        root: std::path::PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "lsp-det-clangd-adapter-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            Fixture { root }
        }

        /// Creates (if needed) and returns a subdirectory.
        fn dir(&self, rel: &str) -> std::path::PathBuf {
            let d = self.root.join(rel);
            std::fs::create_dir_all(&d).unwrap();
            d
        }

        /// Writes `content` at `rel` (parent directories created as needed).
        fn write(&self, rel: &str, content: &str) {
            let f = self.root.join(rel);
            if let Some(parent) = f.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&f, content).unwrap();
        }

        /// An empty file at `rel` (parent directories created as needed), for use as the
        /// `didOpen`ed document.
        fn file(&self, rel: &str) -> std::path::PathBuf {
            self.write(rel, "");
            self.root.join(rel)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn a_did_open_whose_uri_is_not_a_file_does_not_use_up_the_probe() {
        // A didOpen that cannot be turned into a path (an untitled buffer, a non-file scheme)
        // teaches nothing about the workspace; the next didOpen with a real path must still be
        // probed, or a workspace with no database would stay initializing forever.
        let fixture = Fixture::new("non-file-uri-first");
        let opened = fixture.file("main.cpp");
        let mut m = ClangdAdapter::new();
        let untitled = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"untitled:Untitled-1","languageId":"cpp","version":1,"text":""}}}"#;
        assert!(observe(&mut m, untitled).is_none());
        let state = observe(&mut m, &did_open(&opened))
            .expect("the first didOpen with a real path is the one that probes");
        assert_eq!(state.readiness, Readiness::Unknown);
    }

    #[test]
    fn a_compile_commands_json_beside_the_opened_file_leaves_readiness_untouched() {
        let fixture = Fixture::new("beside");
        fixture.write("compile_commands.json", "[]");
        let opened = fixture.file("main.cpp");
        let mut m = ClangdAdapter::new();
        assert!(
            observe(&mut m, &did_open(&opened)).is_none(),
            "a database was found next to the opened file, so nothing should change"
        );
        assert_eq!(m.state.readiness, Readiness::Initializing);
    }

    #[test]
    fn a_build_compile_commands_json_in_a_parent_directory_leaves_readiness_untouched() {
        let fixture = Fixture::new("build-parent");
        fixture.write("build/compile_commands.json", "[]");
        let opened = fixture.file("src/main.cpp");
        let mut m = ClangdAdapter::new();
        assert!(
            observe(&mut m, &did_open(&opened)).is_none(),
            "walking up from src/ must reach the parent's build/compile_commands.json"
        );
        assert_eq!(m.state.readiness, Readiness::Initializing);
    }

    #[test]
    fn a_compile_flags_txt_beside_the_opened_file_leaves_readiness_untouched() {
        let fixture = Fixture::new("flags");
        fixture.write("compile_flags.txt", "-std=c++20");
        let opened = fixture.file("main.cpp");
        let mut m = ClangdAdapter::new();
        assert!(observe(&mut m, &did_open(&opened)).is_none());
        assert_eq!(m.state.readiness, Readiness::Initializing);
    }

    #[test]
    fn no_database_anywhere_up_to_the_root_becomes_unknown() {
        let fixture = Fixture::new("none");
        let opened = fixture.file("main.cpp");
        let mut m = ClangdAdapter::new();
        let state = observe(&mut m, &did_open(&opened))
            .expect("no database found anywhere is a signal (spec 8.2 item 3)");
        assert_eq!(state.readiness, Readiness::Unknown);
        assert_eq!(m.state.readiness, Readiness::Unknown);
    }

    #[test]
    fn compile_commands_dir_scopes_the_probe_to_that_one_directory() {
        let fixture = Fixture::new("cdb-dir");
        // A database sits right beside the opened file...
        fixture.write("compile_commands.json", "[]");
        let opened = fixture.file("main.cpp");
        // ...but the upstream was launched with --compile-commands-dir pointing elsewhere,
        // which has no database of its own. clangd itself would look only there, ignoring the
        // one beside the opened file.
        let elsewhere = fixture.dir("elsewhere");
        let mut m = ClangdAdapter::new();
        m.learn_upstream_arguments(&[format!("--compile-commands-dir={}", elsewhere.display())]);
        let state = observe(&mut m, &did_open(&opened)).expect(
            "--compile-commands-dir scopes the probe to its own directory, which has no database",
        );
        assert_eq!(state.readiness, Readiness::Unknown);
    }

    #[test]
    fn a_begin_after_unknown_still_moves_to_indexing_and_its_end_to_ready() {
        let fixture = Fixture::new("begin-after-unknown");
        let opened = fixture.file("main.cpp");
        let mut m = ClangdAdapter::new();
        observe(&mut m, &did_open(&opened));
        assert_eq!(m.state.readiness, Readiness::Unknown);
        let state = feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "begin"))
            .expect("a begin still moves readiness even after the probe settled on unknown");
        assert_eq!(state.readiness, Readiness::Indexing);
        let state = feed(&mut m, &progress(BACKGROUND_INDEX_TOKEN, "end")).unwrap();
        assert_eq!(state.readiness, Readiness::Ready);
    }

    #[test]
    fn a_second_did_open_is_not_read() {
        let fixture = Fixture::new("second-open");
        let opened = fixture.file("main.cpp"); // no database anywhere: settles on unknown
        let mut m = ClangdAdapter::new();
        observe(&mut m, &did_open(&opened));
        assert_eq!(m.state.readiness, Readiness::Unknown);

        // A second didOpen, this time of a file right next to a database, must not be read.
        let with_db = fixture.dir("withdb");
        std::fs::write(with_db.join("compile_commands.json"), "[]").unwrap();
        let second = with_db.join("second.cpp");
        std::fs::write(&second, "").unwrap();
        assert!(
            observe(&mut m, &did_open(&second)).is_none(),
            "a second didOpen must not be read (the judgment is made once)"
        );
        assert_eq!(
            m.state.readiness,
            Readiness::Unknown,
            "the first judgment must stick"
        );
    }

    #[test]
    fn declares_a_guarantee_only_for_the_tested_version() {
        let tested = ClangdAdapter::for_version(Some(
            "clangd version 21.1.8 linux x86_64-unknown-linux-gnu",
        ));
        assert_eq!(tested.guarantees(), ServerStateProvider::coverage_only(&[]));
        let untested = ClangdAdapter::for_version(Some(
            "clangd version 20.1.0 linux x86_64-unknown-linux-gnu",
        ));
        assert_eq!(
            untested.guarantees(),
            ServerStateProvider::notifications_only()
        );
        let different_platform =
            ClangdAdapter::for_version(Some("clangd version 21.1.8 darwin arm64-apple-darwin"));
        assert_eq!(
            different_platform.guarantees(),
            ServerStateProvider::notifications_only(),
            "a different platform build is a different, untested version"
        );
        let unversioned = ClangdAdapter::new();
        assert_eq!(
            unversioned.guarantees(),
            ServerStateProvider::notifications_only()
        );
    }

    #[test]
    fn a_tested_guarantee_declares_no_freshness() {
        let tested = ClangdAdapter::for_version(Some(
            "clangd version 21.1.8 linux x86_64-unknown-linux-gnu",
        ));
        let json = serde_json::to_string(&tested.guarantees()).unwrap();
        assert!(
            !json.contains("freshness"),
            "clangd must not declare freshness: {json}"
        );
        assert!(
            json.contains("coverage"),
            "clangd must declare coverage: {json}"
        );
    }
}
