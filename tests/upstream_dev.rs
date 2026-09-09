//! Acceptance conditions for the changes to be sent upstream (local only. All `#[ignore]`).
//!
//! Build the clones in `reference/` with `scripts/upstream/build-*.sh`, and run with
//! `target/upstream/bin` at the front of PATH:
//!
//! ```text
//! PATH="$PWD/target/upstream/bin:$PATH" cargo test --test upstream_dev -- --ignored
//! ```
//!
//! Each test is written in the form "passes once the upstream behaves this way".
//! Against a distributed build or an unmodified source build, **failing is correct** (the change
//! is not in yet). Apply the change to the clone, rebuild, and once it passes, send it upstream.
//! Not run in CI (v0.1-design.md chapter 6).
//!
//! Targets:
//! - pyright / typescript-language-server: return `InitializeResult.serverInfo` (a standard
//!   field of LSP 3.15. ADR 0011 decision C). Once returned, lsp-det selects the mapping by
//!   `serverInfo` rather than by the startup log
//! - rust-analyzer / gopls: speak the server state protocol themselves (spec chapters 3 to 7.
//!   Path 2 of vision.md chapter 5). Once they do, the upstream side of lsp-det becomes the
//!   identity mapping (spec 8.2 item 6, 8.4 item 2), and the upstream's declaration and state
//!   flow through as they are
//! - typescript-language-server: exit when tsserver is killed by a signal, as it already does
//!   for a non-zero exit code (upstream #302 / #305). Once it does, a dead tsserver reaches
//!   lsp-det as the exit of the upstream (spec chapter 8) instead of empty answers
//! - rust-analyzer (the alternative to speaking the protocol): report `readiness` as a field of
//!   `experimental/serverStatus`. Once it does, the mapping reads the field instead of deriving
//!   readiness from `quiescent`, and `initializing` before the first load is the server's own word

mod support;

use serde_json::{Value, json};
use support::{ConformanceClient, ServerUnderTest};

/// A subject that launches the upstream directly, without lsp-det in between.
fn direct(command: &str, args: &[&str], root: std::path::PathBuf) -> ServerUnderTest {
    ServerUnderTest {
        program: which(command),
        args: args.iter().map(|a| a.to_string()).collect(),
        root,
    }
}

/// A subject launched via lsp-det.
fn via_lsp_det(command: &str, args: &[&str], root: std::path::PathBuf) -> ServerUnderTest {
    let mut all = vec!["--".to_string(), command.to_string()];
    all.extend(args.iter().map(|a| a.to_string()));
    ServerUnderTest {
        program: support::lsp_det_binary(),
        args: all,
        root,
    }
}

/// Finds a command in PATH. `ServerUnderTest.program` may be either an absolute path or a name,
/// but it is resolved so that a failure shows which build was used.
fn which(command: &str) -> std::path::PathBuf {
    let path = std::env::var_os("PATH").expect("PATH is missing");
    let found = std::env::split_paths(&path)
        .flat_map(|dir| candidates(&dir, command))
        .find(|candidate| is_executable(candidate))
        .unwrap_or_else(|| panic!("{command} is not in PATH"));
    eprintln!("upstream_dev: {command} -> {}", found.display());
    found
}

/// The file names that could run as `command` inside `dir`. On Windows, searches with extensions
/// added (npm's launchers are `.cmd`).
fn candidates(dir: &std::path::Path, command: &str) -> Vec<std::path::PathBuf> {
    let mut found = vec![dir.join(command)];
    if cfg!(windows) {
        found.extend(
            ["exe", "cmd", "bat"]
                .iter()
                .map(|ext| dir.join(format!("{command}.{ext}"))),
        );
    }
    found
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(windows)]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

fn server_info_of(server: &ServerUnderTest) -> Value {
    let mut client = ConformanceClient::start(server);
    let result = client.initialize_with_root(false, &server.root);
    client.shutdown();
    result["result"]["serverInfo"].clone()
}

// ---------------------------------------------------------------------------
// Changes to servers that do not return serverInfo (ADR 0011 decision C)
// ---------------------------------------------------------------------------

/// pyright: returns `InitializeResult.serverInfo` as `{name: "pyright", version}`.
/// basedpyright already returns it (`{name: "basedpyright", version: "1.39.8"}`).
#[test]
#[ignore = "acceptance condition for an upstream change. Local only. Put target/upstream/bin in PATH and run cargo test --test upstream_dev -- --ignored"]
fn pyright_names_itself_in_server_info() {
    let project = support::TempPyProject::with_cross_file_reference("upstream-dev");
    let info = server_info_of(&direct(
        "pyright-langserver",
        &["--stdio"],
        project.root.clone(),
    ));
    // The upstream calls itself by the productName ("Pyright"). lsp-det is case-insensitive.
    assert!(
        info["name"]
            .as_str()
            .is_some_and(|n| n.eq_ignore_ascii_case("pyright")),
        "pyright does not name itself in serverInfo: {info}"
    );
    assert!(
        info["version"].as_str().is_some_and(|v| !v.is_empty()),
        "does not name its version: {info}"
    );

    // Once it names itself, lsp-det selects the mapping by serverInfo rather than by the startup
    // log. If the version is not in pyright::TESTED_VERSIONS, no guarantee is declared (true).
    let mut client = ConformanceClient::start(&via_lsp_det(
        "pyright-langserver",
        &["--stdio"],
        project.root.clone(),
    ));
    let result = client.initialize_with_root(true, &project.root);
    assert!(
        !result["result"]["capabilities"]["experimental"]["serverStateProvider"].is_null(),
        "lsp-det could not select the mapping: {result}"
    );
    client.shutdown();
}

/// typescript-language-server: returns `InitializeResult.serverInfo` as
/// `{name: "typescript-language-server", version}`.
#[test]
#[ignore = "acceptance condition for an upstream change. Local only. Put target/upstream/bin in PATH and run cargo test --test upstream_dev -- --ignored"]
fn typescript_language_server_names_itself_in_server_info() {
    let project = support::TempTsProject::with_cross_file_reference("upstream-dev");
    let info = server_info_of(&direct(
        "typescript-language-server",
        &["--stdio"],
        project.root.clone(),
    ));
    assert_eq!(
        info["name"],
        json!("typescript-language-server"),
        "typescript-language-server does not name itself in serverInfo: {info}"
    );
    assert!(
        info["version"].as_str().is_some_and(|v| !v.is_empty()),
        "does not name its version: {info}"
    );
}

// ---------------------------------------------------------------------------
// Changes to speak the server state protocol themselves (path 2 of vision.md chapter 5)
// ---------------------------------------------------------------------------

/// The upstream declares `serverStateProvider` on its own and answers `experimental/serverState`.
/// With lsp-det in between, the upstream side becomes the identity mapping, and the upstream's
/// declaration and state flow through as they are (spec 8.4 item 2).
fn assert_upstream_speaks_the_protocol(command: &str, args: &[&str], root: std::path::PathBuf) {
    // Direct: the declaration and the response are there.
    let mut upstream = ConformanceClient::start(&direct(command, args, root.clone()));
    let direct_result = upstream.initialize_with_root(true, &root);
    let declared =
        direct_result["result"]["capabilities"]["experimental"]["serverStateProvider"].clone();
    assert!(
        !declared.is_null() && declared != json!(false),
        "{command} does not declare serverStateProvider: {direct_result}"
    );
    let direct_state = upstream.request("experimental/serverState", json!({}));
    assert!(
        direct_state["error"].is_null() && !direct_state["result"]["readiness"].is_null(),
        "{command} does not answer experimental/serverState: {direct_state}"
    );
    upstream.shutdown();

    // Via lsp-det: the declaration matches the upstream's, and the state too is the upstream's
    // answer returned as is.
    let mut client = ConformanceClient::start(&via_lsp_det(command, args, root.clone()));
    let result = client.initialize_with_root(true, &root);
    assert_eq!(
        result["result"]["capabilities"]["experimental"]["serverStateProvider"], declared,
        "lsp-det rewrote the upstream's declaration (spec 8.4 item 2): {result}"
    );
    let state = client.request("experimental/serverState", json!({}));
    assert!(
        state["error"].is_null() && !state["result"]["readiness"].is_null(),
        "experimental/serverState is not answered via lsp-det: {state}"
    );
    client.shutdown();
}

/// rust-analyzer: speaks this protocol as the successor of `experimental/serverStatus`
/// (spec chapter 10. quiescent → readiness, health as is, declares the guarantees).
#[test]
#[ignore = "acceptance condition for an upstream change. Local only. Put target/upstream/bin in PATH and run cargo test --test upstream_dev -- --ignored"]
fn rust_analyzer_speaks_the_server_state_protocol() {
    let project = support::TempCargoProject::with_cross_file_reference("upstream-dev");
    assert_upstream_speaks_the_protocol("rust-analyzer", &[], project.root.clone());
}

/// rust-analyzer, the field-addition alternative (docs/upstream-submissions.md, preparation 4):
/// `experimental/serverStatus` carries `readiness` next to `quiescent`. `quiescent` means "no
/// background work is pending", which is trivially `true` before the first load (nothing is in
/// flight yet), so a client reading it as "ready" is wrong exactly then; the field spells out
/// `initializing` / `indexing` / `ready`. Passes once every status notification carries the field consistently with
/// `quiescent` and the first load ends in `ready`.
#[test]
#[ignore = "acceptance condition for an upstream change. Local only. Put target/upstream/bin in PATH and run cargo test --test upstream_dev -- --ignored"]
fn rust_analyzer_reports_readiness_in_server_status() {
    let project = support::TempCargoProject::with_cross_file_reference("upstream-dev");
    let mut upstream =
        ConformanceClient::start(&direct("rust-analyzer", &[], project.root.clone()));
    upstream.initialize_with_root_and_capabilities(
        &project.root,
        json!({"experimental": {"serverStatusNotification": true}}),
    );

    // The first load of a small workspace ends within seconds; the cap only bounds the premise.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut seen = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let status = upstream
            .await_notification_within("experimental/serverStatus", remaining)
            .unwrap_or_else(|| {
                panic!("the first load did not end in readiness ready within 60 seconds: {seen:?}")
            });
        let readiness = status["readiness"]
            .as_str()
            .unwrap_or_else(|| panic!("the status carries no readiness field: {status}"))
            .to_string();
        let quiescent = status["quiescent"]
            .as_bool()
            .unwrap_or_else(|| panic!("the status carries no quiescent field: {status}"));
        assert!(
            ["initializing", "indexing", "ready"].contains(&readiness.as_str()),
            "readiness is not a value of the protocol: {status}"
        );
        // ready is quiescent, and non-quiescent is never ready (quiescent while initializing is
        // the trivial quiescence before the first load, which is the point of the field).
        assert!(
            (readiness == "ready") == quiescent || readiness == "initializing",
            "readiness and quiescent disagree: {status}"
        );
        seen.push(readiness.clone());
        if readiness == "ready" {
            break;
        }
    }
    upstream.shutdown();
}

/// gopls: speaks this protocol in addition to the "Setting up workspace" progress
/// (the reload after a go.mod change can then also be conveyed as `indexing`).
#[test]
#[ignore = "acceptance condition for an upstream change. Local only. Put target/upstream/bin in PATH and run cargo test --test upstream_dev -- --ignored"]
fn gopls_speaks_the_server_state_protocol() {
    let project = support::TempGoProject::with_cross_file_reference("upstream-dev");
    assert_upstream_speaks_the_protocol("gopls", &[], project.root.clone());
}

// ---------------------------------------------------------------------------
// Bug fix: the language server exits when tsserver is killed by a signal
// ---------------------------------------------------------------------------

/// typescript-language-server: when tsserver dies, the language server exits so that the client
/// restarts it (upstream #302, the design of #305). The exit is honoured only for a non-zero
/// exit code, so a tsserver killed by a signal (`exitCode: null`: SIGKILL from the OOM killer,
/// SIGABRT from Node's own out-of-memory abort) leaves the language server alive, answering
/// `references` with `[]` (docs/research/typescript-language-server-readiness-measurement.md).
/// Passes once the language server exits on that too.
#[test]
#[ignore = "acceptance condition for an upstream change. Local only. Put target/upstream/bin in PATH and run cargo test --test upstream_dev -- --ignored"]
fn typescript_language_server_exits_when_tsserver_is_killed() {
    let project = support::TempTsProject::with_cross_file_reference("upstream-dev-crash");
    let a = project.file("a.ts");
    let mut client = ConformanceClient::start(&direct(
        "typescript-language-server",
        &["--stdio"],
        project.root.clone(),
    ));
    client.initialize_with_root(false, &project.root);
    client.did_open(&a, "typescript");

    // Premise: tsserver is up and answers. Right after didOpen the project may still be loading,
    // so poll until the cross-file reference appears (the cap only bounds the premise).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if !client.references(&a, 0, 16).is_empty() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the premise is broken: no references within 20 seconds"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    let killed = support::kill_descendants_matching(client.server_pid(), "tsserver");
    assert!(
        !killed.is_empty(),
        "the tsserver grandchild process was not found"
    );

    // The language server notices the exit through the `exit` event of the child process and
    // stops itself with a non-zero exit code, so that the client restarts it. The wait only
    // bounds how long the test looks.
    let status = client
        .exit_status_within(std::time::Duration::from_secs(10))
        .expect(
            "the language server survived the death of tsserver (a request would now be \
             answered with an empty success)",
        );
    assert!(
        !status.success(),
        "the language server exited as if nothing happened: {status}"
    );
    let log = client.stderr_after_exit();
    assert!(
        log.contains("tsserver process has exited"),
        "the reason for stopping is not on stderr: {log}"
    );
}
