//! プロセス寿命の 2 経路（設計 4.5、ADR 0012 決定 B）の多プロセス結合テスト。
//!
//! 1 段目の stdin の EOF は OS に依らず働くので、ここでは EOF が届かない状況を
//! 作り、2 段目（OS の機構）だけで終了することを確かめる。3 つの OS の CI で
//! 同じテストを回す（ADR 0012 決定 D）。
//!
//! 末尾の `#[ignore]` は実サーバーが stdin の EOF で終了するかの実測で、
//! macOS の上流の追従が EOF に委ねられている根拠になる。

mod support;

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use serde_json::json;

/// 殺してから消えるまでを待つ上限。超えたら失敗（黙って通さない）。
const EXIT_WINDOW: Duration = Duration::from_secs(10);

/// stderr の行を別スレッドで読み、`needle` を含む最初の行から pid を取り出す。
fn pid_from_stderr(lines: &Receiver<String>, needle: &str) -> u32 {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let line = lines
            .recv_timeout(remaining)
            .unwrap_or_else(|_| panic!("stderr に {needle:?} の行が来ない"));
        if let Some(rest) = line.split(needle).nth(1) {
            return rest
                .trim()
                .parse()
                .unwrap_or_else(|err| panic!("{line:?} から pid を読めない: {err}"));
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

/// 経路 1: クライアントが不意に死んだら lsp-det も上流も終了する。
///
/// 擬似クライアント（`examples/pseudo_client.rs`）が lsp-det を起動し、stdin は
/// 本テストが持つパイプを継承させる。擬似クライアントを殺しても lsp-det の
/// stdin は閉じないので、EOF ではなく OS の機構だけが lsp-det を終了させる。
/// 上流は lsp-det の終了に追従する。
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
        .expect("擬似クライアントを起動できない");
    // 書き込み側を持ち続ける。lsp-det の stdin はこのパイプの読み取り側。
    let held_stdin = pseudo_client.stdin.take().expect("stdin is piped");
    let lines = spawn_stderr_lines(pseudo_client.stderr.take().expect("stderr is piped"));

    let lsp_det_pid = pid_from_stderr(&lines, "pseudo-client: child pid");
    let upstream_pid = pid_from_stderr(&lines, "fake-lsp-server: pid");
    assert!(support::process_is_alive(lsp_det_pid));
    assert!(support::process_is_alive(upstream_pid));

    // クライアントの不意の死（SIGKILL / TerminateProcess）。片付けの機会はない。
    pseudo_client.kill().expect("擬似クライアントを殺せない");
    pseudo_client
        .wait()
        .expect("擬似クライアントを回収できない");

    assert!(
        support::wait_until_exited(lsp_det_pid, EXIT_WINDOW),
        "クライアントが死んでも lsp-det (pid {lsp_det_pid}) が残っている"
    );
    assert!(
        support::wait_until_exited(upstream_pid, EXIT_WINDOW),
        "クライアントが死んでも上流 (pid {upstream_pid}) が残っている"
    );
    drop(held_stdin);
}

/// 経路 2: lsp-det が不意に死んだら上流も終了する。
///
/// lsp-det の stdin は本テストが持ち続ける（EOF を送らない）。lsp-det を殺すと
/// 上流の stdin の書き込み側は lsp-det と共に消えるので、上流は EOF でも
/// OS の機構でも終了しうる。どちらでもよく、残らないことが要件である。
#[test]
fn upstream_exits_when_lsp_det_dies_abruptly() {
    let mut lsp_det = Command::new(support::lsp_det_binary())
        .arg("--")
        .arg(support::fake_upstream_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lsp-det を起動できない");
    let held_stdin = lsp_det.stdin.take().expect("stdin is piped");
    let lines = spawn_stderr_lines(lsp_det.stderr.take().expect("stderr is piped"));

    let upstream_pid = pid_from_stderr(&lines, "fake-lsp-server: pid");
    assert!(support::process_is_alive(upstream_pid));

    lsp_det.kill().expect("lsp-det を殺せない");
    lsp_det.wait().expect("lsp-det を回収できない");

    assert!(
        support::wait_until_exited(upstream_pid, EXIT_WINDOW),
        "lsp-det が死んでも上流 (pid {upstream_pid}) が残っている"
    );
    drop(held_stdin);
}

// ---------------------------------------------------------------------------
// 実サーバーが stdin の EOF で終了するか（ローカル専用）
// ---------------------------------------------------------------------------

/// `initialize` → `initialized` の後に stdin を閉じ、`window` 以内に終了するか。
/// stdout は捨て続ける（パイプが詰まって終了できない状態を作らない）。
fn exits_on_stdin_eof(mut child: Child, root: &std::path::Path, window: Duration) -> bool {
    let mut stdin = child.stdin.take().expect("stdin is piped");
    let stdout = child.stdout.take().expect("stdout is piped");
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut BufReader::new(stdout), &mut std::io::sink());
    });
    for message in [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "processId": std::process::id(),
            "rootUri": support::file_uri(root),
            "capabilities": {},
        }}),
        json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
    ] {
        let body = serde_json::to_vec(&message).unwrap();
        write!(stdin, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
        stdin.write_all(&body).unwrap();
        stdin.flush().unwrap();
    }
    drop(stdin);

    let deadline = std::time::Instant::now() + window;
    loop {
        if child.try_wait().expect("try_wait").is_some() {
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

fn spawn_direct(program: &str, args: &[&str], root: &std::path::Path) -> Child {
    Command::new(program)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|err| panic!("{program} を起動できない: {err}"))
}

#[test]
#[ignore = "実 rust-analyzer が要る。ローカル専用"]
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
#[ignore = "実 gopls が要る。ローカル専用"]
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
#[ignore = "実 pyright が要る。ローカル専用"]
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
#[ignore = "実 typescript-language-server が要る。ローカル専用"]
fn real_typescript_language_server_exits_on_stdin_eof() {
    let project = support::TempTsProject::with_cross_file_reference("eof");
    let child = spawn_direct("typescript-language-server", &["--stdio"], &project.root);
    assert!(exits_on_stdin_eof(
        child,
        &project.root,
        Duration::from_secs(30)
    ));
}
