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

/// gopls: speaks this protocol in addition to the "Setting up workspace" progress
/// (the reload after a go.mod change can then also be conveyed as `indexing`).
#[test]
#[ignore = "acceptance condition for an upstream change. Local only. Put target/upstream/bin in PATH and run cargo test --test upstream_dev -- --ignored"]
fn gopls_speaks_the_server_state_protocol() {
    let project = support::TempGoProject::with_cross_file_reference("upstream-dev");
    assert_upstream_speaks_the_protocol("gopls", &[], project.root.clone());
}

// ---------------------------------------------------------------------------
// Bug fix: a request without a tsserver is an error, not an empty success
// ---------------------------------------------------------------------------

/// typescript-language-server: after tsserver has exited, a request is answered with an error
/// (`RequestFailed`, -32803, the reason in the message) instead of an empty array reported as
/// success (research/typescript-language-server-readiness-measurement.md: the language server
/// survives a SIGKILL of tsserver and answers `references` with `[]`).
#[test]
#[ignore = "acceptance condition for an upstream change. Local only. Put target/upstream/bin in PATH and run cargo test --test upstream_dev -- --ignored"]
fn typescript_language_server_fails_requests_after_tsserver_exit() {
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
    // The language server notices the exit and logs it ("[tsserver] Exited. Code: ..."). Send
    // the request only after that, so the test does not depend on the order of the kill and the
    // request.
    loop {
        let log = client
            .await_notification_within("window/logMessage", std::time::Duration::from_secs(10))
            .expect("the exit of tsserver is not logged within 10 seconds");
        if log["message"]
            .as_str()
            .is_some_and(|m| m.contains("Exited. Code:"))
        {
            break;
        }
    }

    let id = client.send_request(
        "textDocument/references",
        json!({
            "textDocument": {"uri": support::file_uri(&a)},
            "position": {"line": 0, "character": 16},
            "context": {"includeDeclaration": false},
        }),
    );
    let response = client.await_response_to(id);
    assert_eq!(
        response["error"]["code"],
        json!(-32803),
        "a request without a tsserver is not RequestFailed: {response}"
    );
    assert!(
        response["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("tsserver exited")),
        "the reason is not in the message: {response}"
    );
    client.shutdown();
}
