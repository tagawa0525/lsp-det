//! 上流プロセスの起動と寿命管理 (v0.1-design.md 4.5、ADR 0012)。
//!
//! - 上流はプロキシの子。プロキシ終了時に確実に殺す
//! - プロキシが `SIGKILL` 等で不意に死んでも上流が孤児化しないようにする
//! - プロキシ自身は親 (クライアント) の不意の死に追従して終了する
//!   (`exit_with_parent`。`main` の起動直後に呼ぶ)
//!
//! 1 段目の stdin の EOF (クライアントが死ねばプロキシの stdin が閉じ、
//! プロキシが死ねば上流の stdin が閉じる) は OS に依らず働く。ここにあるのは
//! EOF が届かない不意の死への 2 段目で、OS ごとの機構で実装する:
//!
//! | OS      | プロキシが親の死に追従する          | 上流がプロキシの死に追従する            |
//! | ------- | ----------------------------------- | --------------------------------------- |
//! | Linux   | `PR_SET_PDEATHSIG`                  | `PR_SET_PDEATHSIG`                      |
//! | macOS   | `kqueue` の `EVFILT_PROC` で親を待つ | 機構がない。上流の stdin の EOF に委ねる |
//! | Windows | 親プロセスのハンドルを待つ          | Job Object の `KILL_ON_JOB_CLOSE`       |

use std::io;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

pub struct Upstream {
    child: Child,
}

pub struct UpstreamHandles {
    pub upstream: Upstream,
    pub stdin: ChildStdin,
    pub stdout: ChildStdout,
    pub stderr: ChildStderr,
}

/// 上流コマンドを起動する。stdin/stdout/stderr はすべて pipe で接続する。
/// 起動した上流は、プロキシの不意の死に OS の機構で追従する (上表)。
pub fn spawn(command: &str, args: &[String]) -> io::Result<UpstreamHandles> {
    let mut cmd = Command::new(command);
    cmd.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    platform::prepare_upstream(&mut cmd);

    let mut child = cmd.spawn()?;
    platform::follow_upstream(&child);
    let stdin = child.stdin.take().expect("stdin is piped");
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");

    Ok(UpstreamHandles {
        upstream: Upstream { child },
        stdin,
        stdout,
        stderr,
    })
}

/// 親 (クライアント) プロセスが不意に死んだら、このプロセスも終了するように
/// する。`main` の最初に一度だけ呼ぶ。
///
/// これは「呼び出し元プロセスの親が死んだら合図する」というプロセス自身に
/// 対する設定であり、上流の起動 (`spawn`) とは独立している。
pub fn exit_with_parent() {
    platform::exit_with_parent();
}

impl Upstream {
    /// 上流がまだ生きているか確認する。ブロックしない。
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// 上流の終了を待つ (ブロックする)。
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait()
    }

    /// 上流を殺して終了を待つ。既に終了していてもエラーにしない。
    pub fn kill_and_wait(&mut self) -> io::Result<()> {
        match self.child.kill() {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::InvalidInput => {
                // 既に終了しているプロセスへの kill。無視してよい。
            }
            Err(err) => return Err(err),
        }
        self.child.wait()?;
        Ok(())
    }
}

// 親の死への追従と上流の道連れは多プロセスのシナリオでしか検証できない。
// `cargo test` はマルチスレッドで実行されるため、テストバイナリ内での
// `fork()` はデッドロックの危険があり避けている。これらは結合テスト
// `tests/process_lifetime.rs` (擬似クライアント越しに殺す) が、3 つの OS の
// CI で確かめる (ADR 0012 決定 D)。

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::thread;
    use std::time::Duration;

    /// OS のシェルで 1 行のスクリプトを走らせる起動指定。
    fn shell(script: &str) -> (&'static str, Vec<String>) {
        if cfg!(windows) {
            ("cmd", vec!["/C".to_string(), script.to_string()])
        } else {
            ("sh", vec!["-c".to_string(), script.to_string()])
        }
    }

    /// stdin を stdout へそのまま流すコマンド。
    fn echo_stdin() -> (&'static str, Vec<String>) {
        if cfg!(windows) {
            (
                "powershell",
                vec![
                    "-NoProfile".to_string(),
                    "-Command".to_string(),
                    "$input | ForEach-Object { $_ }".to_string(),
                ],
            )
        } else {
            ("cat", vec![])
        }
    }

    /// 30 秒待つコマンド。
    fn sleep_30() -> (&'static str, Vec<String>) {
        if cfg!(windows) {
            (
                "powershell",
                vec![
                    "-NoProfile".to_string(),
                    "-Command".to_string(),
                    "Start-Sleep 30".to_string(),
                ],
            )
        } else {
            ("sleep", vec!["30".to_string()])
        }
    }

    #[test]
    fn spawns_and_pipes_stdio_end_to_end() {
        let (program, args) = echo_stdin();
        let mut handles = spawn(program, &args).expect("spawn echo");
        handles
            .stdin
            .write_all(b"hello\n")
            .expect("write to child stdin");
        drop(handles.stdin); // EOF を送って終了させる

        let mut reader = BufReader::new(handles.stdout);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read echoed line");
        assert_eq!(line.trim_end(), "hello");

        handles.upstream.wait().expect("wait for echo to exit");
    }

    #[test]
    fn kill_and_wait_terminates_a_running_process() {
        let (program, args) = sleep_30();
        let mut handles = spawn(program, &args).expect("spawn sleep");
        assert!(
            handles.upstream.try_wait().expect("try_wait").is_none(),
            "sleep should still be running"
        );

        handles.upstream.kill_and_wait().expect("kill_and_wait");
        // kill_and_wait が返った時点で終了済み。
        assert!(handles.upstream.try_wait().expect("try_wait").is_some());
    }

    #[test]
    fn kill_and_wait_is_idempotent_after_natural_exit() {
        let (program, args) = shell("exit 0");
        let mut handles = spawn(program, &args).expect("spawn exit 0");
        // 自然終了を待つ。
        for _ in 0..50 {
            if handles.upstream.try_wait().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        // 既に死んでいるプロセスへの kill はエラーにならない。
        handles
            .upstream
            .kill_and_wait()
            .expect("kill_and_wait on already-exited process must not error");
    }

    #[test]
    fn try_wait_reports_exit_status_of_naturally_exiting_process() {
        let (program, args) = shell("exit 3");
        let mut handles = spawn(program, &args).expect("spawn exit 3");
        let status = handles.upstream.wait().expect("wait for exit");
        assert_eq!(status.code(), Some(3));
    }

    #[test]
    fn stderr_is_captured_and_readable() {
        let (program, args) = shell("echo err-msg>&2");
        let mut handles = spawn(program, &args).expect("spawn shell writing to stderr");
        let mut reader = BufReader::new(handles.stderr);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read stderr line");
        assert_eq!(line.trim_end(), "err-msg");
        handles.upstream.wait().expect("wait for exit");
    }
}
