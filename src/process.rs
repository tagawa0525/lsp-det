//! 上流プロセスの起動と寿命管理 (v0.1-design.md 4.7)。
//!
//! - 上流はプロキシの子。プロキシ終了時に確実に殺す
//! - 上流には `PR_SET_PDEATHSIG` を設定し、プロキシが `SIGKILL` 等で
//!   不意に死んでも上流が孤児化しないようにする
//! - プロキシ自身の pdeathsig (親クライアントの死への追従) は
//!   `main` の起動直後に別途設定する (`set_self_pdeathsig`)

use std::io;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

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
pub fn spawn(command: &str, args: &[String]) -> io::Result<UpstreamHandles> {
    let _ = (command, args, Stdio::piped);
    todo!("GREEN で実装する")
}

impl Upstream {
    /// 上流がまだ生きているか確認する。ブロックしない。
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        todo!("GREEN で実装する")
    }

    /// 上流の終了を待つ (ブロックする)。
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        todo!("GREEN で実装する")
    }

    /// 上流を殺して終了を待つ。既に終了していてもエラーにしない。
    pub fn kill_and_wait(&mut self) -> io::Result<()> {
        todo!("GREEN で実装する")
    }
}

/// プロキシ自身に `PR_SET_PDEATHSIG` を設定する。
/// 親 (クライアント) プロセスが死んだら、プロキシに `SIGTERM` が届くようにする。
/// `main` の最初に一度だけ呼ぶ。
///
/// 注意: これは「呼び出し元プロセスの親が死んだら合図する」という
/// プロセス自身に対する設定であり、子プロセスの起動 (`spawn`) とは独立している。
#[cfg(unix)]
pub fn set_self_pdeathsig() {
    set_pdeathsig_on_self();
}

#[cfg(unix)]
fn set_pdeathsig_on_self() {
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
    }
}

#[cfg(not(unix))]
pub fn set_self_pdeathsig() {
    // Linux 以外では未対応 (v0.1-design.md 4.7 は Linux を想定)。
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn spawns_and_pipes_stdio_end_to_end() {
        // `cat` は stdin を stdout へそのまま echo する。
        let mut handles = spawn("cat", &[]).expect("spawn cat");
        handles
            .stdin
            .write_all(b"hello\n")
            .expect("write to child stdin");
        drop(handles.stdin); // EOF を送って cat を終了させる

        let mut reader = BufReader::new(handles.stdout);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read echoed line");
        assert_eq!(line, "hello\n");

        handles.upstream.wait().expect("wait for cat to exit");
    }

    #[test]
    fn kill_and_wait_terminates_a_running_process() {
        let mut handles = spawn("sleep", &["30".to_string()]).expect("spawn sleep");
        assert!(
            handles.upstream.try_wait().unwrap().is_none(),
            "sleep should still be running"
        );

        handles.upstream.kill_and_wait().expect("kill sleep");
        assert!(
            handles.upstream.try_wait().unwrap().is_some(),
            "sleep should be dead after kill_and_wait"
        );
    }

    #[test]
    fn kill_and_wait_is_idempotent_after_natural_exit() {
        let mut handles = spawn("true", &[]).expect("spawn true");
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
        let mut handles =
            spawn("sh", &["-c".to_string(), "exit 3".to_string()]).expect("spawn sh -c exit 3");
        let status = handles.upstream.wait().expect("wait for sh to exit");
        assert_eq!(status.code(), Some(3));
    }

    #[test]
    fn stderr_is_captured_and_readable() {
        let mut handles = spawn("sh", &["-c".to_string(), "echo err-msg >&2".to_string()])
            .expect("spawn sh writing to stderr");
        let mut reader = BufReader::new(handles.stderr);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read stderr line");
        assert_eq!(line, "err-msg\n");
        handles.upstream.wait().expect("wait for sh to exit");
    }
}
