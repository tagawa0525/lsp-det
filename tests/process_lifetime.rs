//! Multi-process integration tests for the 2 paths of process lifetime (design 4.5, ADR 0012
//! decision B).
//!
//! The 1st stage, EOF on stdin, works regardless of the OS, so here we create a situation where
//! EOF does not arrive and check that the 2nd stage (the OS mechanism) alone terminates the
//! process. The same tests run in CI on the 3 OSes (ADR 0012 decision D).
//!
//! The `#[ignore]` tests at the end measure whether real servers exit on EOF of stdin, which is
//! the basis for leaving the upstream's lifetime tracking on macOS to EOF.

mod support;

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::json;

/// Limit for waiting from the kill until the process disappears. Exceeding it fails (never
/// passes silently).
const EXIT_WINDOW: Duration = Duration::from_secs(10);

/// Reads stderr lines on a separate thread, and extracts the pid from the first line containing
/// `needle`. On a timeout the lines seen so far are part of the failure, so that a process that
/// died or printed something else (a panic of lsp-det, an OS error) can be told from one that
/// never started.
fn pid_from_stderr(lines: &Receiver<String>, needle: &str) -> u32 {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut seen = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let line = lines.recv_timeout(remaining).unwrap_or_else(|err| {
            panic!(
                "no line containing {needle:?} arrives on stderr ({err:?}); lines so far: {seen:#?}"
            )
        });
        seen.push(line.clone());
        if let Some(rest) = line.split(needle).nth(1) {
            return rest
                .trim()
                .parse()
                .unwrap_or_else(|err| panic!("cannot read the pid from {line:?}: {err}"));
        }
    }
}

fn spawn_stderr_lines<R: Read + Send + 'static>(stderr: R) -> Receiver<String> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// Path 1: if the client dies unexpectedly, both lsp-det and the upstream exit.
///
/// The pseudo client (`examples/pseudo_client.rs`) launches lsp-det, and makes it inherit a pipe
/// held by this test as stdin. Killing the pseudo client does not close lsp-det's stdin, so only
/// the OS mechanism, not EOF, terminates lsp-det. The upstream follows lsp-det's exit.
#[test]
fn lsp_det_and_upstream_exit_when_the_client_dies_without_closing_stdin() {
    let mut pseudo_client = Command::new(support::pseudo_client_binary())
        .arg(support::lsp_det_binary())
        .arg("--")
        .arg(support::fake_upstream_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cannot launch the pseudo client");
    // Keep holding the write end. lsp-det's stdin is the read end of this pipe.
    let held_stdin = pseudo_client.stdin.take().expect("stdin is piped");
    let lines = spawn_stderr_lines(pseudo_client.stderr.take().expect("stderr is piped"));

    let lsp_det_pid = pid_from_stderr(&lines, "pseudo-client: child pid");
    let upstream_pid = pid_from_stderr(&lines, "fake-lsp-server: pid");
    assert!(support::process_is_alive(lsp_det_pid));
    assert!(support::process_is_alive(upstream_pid));

    // The client's unexpected death (SIGKILL / TerminateProcess). No chance to clean up.
    pseudo_client.kill().expect("cannot kill the pseudo client");
    pseudo_client.wait().expect("cannot reap the pseudo client");

    assert!(
        support::wait_until_exited(lsp_det_pid, EXIT_WINDOW),
        "lsp-det (pid {lsp_det_pid}) remains even though the client died"
    );
    assert!(
        support::wait_until_exited(upstream_pid, EXIT_WINDOW),
        "the upstream (pid {upstream_pid}) remains even though the client died"
    );
    drop(held_stdin);
}

/// Path 2: if lsp-det dies unexpectedly, the upstream exits too.
///
/// This test keeps holding lsp-det's stdin (never sends EOF). When lsp-det is killed, the write
/// end of the upstream's stdin disappears with lsp-det, so the upstream may exit by either EOF or
/// the OS mechanism. Either is fine; the requirement is that it does not remain.
#[test]
fn upstream_exits_when_lsp_det_dies_abruptly() {
    let mut lsp_det = Command::new(support::lsp_det_binary())
        .arg("--")
        .arg(support::fake_upstream_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("cannot launch lsp-det");
    let held_stdin = lsp_det.stdin.take().expect("stdin is piped");
    let lines = spawn_stderr_lines(lsp_det.stderr.take().expect("stderr is piped"));

    let upstream_pid = pid_from_stderr(&lines, "fake-lsp-server: pid");
    assert!(support::process_is_alive(upstream_pid));

    lsp_det.kill().expect("cannot kill lsp-det");
    lsp_det.wait().expect("cannot reap lsp-det");

    assert!(
        support::wait_until_exited(upstream_pid, EXIT_WINDOW),
        "the upstream (pid {upstream_pid}) remains even though lsp-det died"
    );
    drop(held_stdin);
}

// ---------------------------------------------------------------------------
// Whether real servers exit on EOF of stdin (local only)
// ---------------------------------------------------------------------------

/// Whether the server exits within `window` after receiving the `initialize` response, sending
/// `initialized`, and then closing stdin. The response is awaited so that a server that failed to
/// start and exited at once is not counted as "exited on EOF". The exit code does not matter
/// (some servers return 1, treating EOF as an abnormal exit). stdout keeps being discarded after
/// the response (so as not to create a state where a clogged pipe prevents exiting).
fn exits_on_stdin_eof(mut child: Child, root: &std::path::Path, window: Duration) -> bool {
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout is piped"));
    let initialize = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "processId": std::process::id(),
        "rootUri": support::file_uri(root),
        "capabilities": {},
    }});
    write_lsp(&mut stdin, &initialize);
    loop {
        let message = lsp_det::framing::read_message(&mut stdout)
            .expect("cannot read stdout")
            .expect("stdout closed before answering initialize");
        let value: serde_json::Value = serde_json::from_slice(&message.body).unwrap();
        if value["id"] == json!(1) {
            assert!(value["error"].is_null(), "initialize failed: {value}");
            break;
        }
    }
    write_lsp(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
    );
    drop(stdin);
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
    });

    let deadline = std::time::Instant::now() + window;
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            eprintln!("exited on stdin EOF with {status}");
            return true;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn write_lsp(stdin: &mut impl Write, message: &serde_json::Value) {
    let body = serde_json::to_vec(message).unwrap();
    write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    stdin.write_all(&body).unwrap();
    stdin.flush().unwrap();
}

fn spawn_direct(program: &str, args: &[&str], root: &std::path::Path) -> Child {
    Command::new(program)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|err| panic!("cannot launch {program}: {err}"))
}

#[test]
#[ignore = "requires a real rust-analyzer. Local only"]
fn real_rust_analyzer_exits_on_stdin_eof() {
    let project = support::TempCargoProject::with_cross_file_reference("eof");
    let child = spawn_direct("rust-analyzer", &[], &project.root);
    assert!(exits_on_stdin_eof(
        child,
        &project.root,
        Duration::from_secs(30)
    ));
}

#[test]
#[ignore = "requires a real gopls. Local only"]
fn real_gopls_exits_on_stdin_eof() {
    let project = support::TempGoProject::with_cross_file_reference("eof");
    let child = spawn_direct("gopls", &[], &project.root);
    assert!(exits_on_stdin_eof(
        child,
        &project.root,
        Duration::from_secs(30)
    ));
}

#[test]
#[ignore = "requires a real pyright. Local only"]
fn real_pyright_exits_on_stdin_eof() {
    let project = support::TempPyProject::with_cross_file_reference("eof");
    let child = spawn_direct("pyright-langserver", &["--stdio"], &project.root);
    assert!(exits_on_stdin_eof(
        child,
        &project.root,
        Duration::from_secs(30)
    ));
}

#[test]
#[ignore = "requires a real typescript-language-server. Local only"]
fn real_typescript_language_server_exits_on_stdin_eof() {
    let project = support::TempTsProject::with_cross_file_reference("eof");
    let child = spawn_direct("typescript-language-server", &["--stdio"], &project.root);
    assert!(exits_on_stdin_eof(
        child,
        &project.root,
        Duration::from_secs(30)
    ));
}
