//! Launching the upstream process and managing its lifetime (v0.1-design.md 4.5, ADR 0012).
//!
//! - The upstream is a child of the proxy. It is reliably killed when the proxy exits
//! - The upstream must not be orphaned even when the proxy dies unexpectedly (`SIGKILL` etc.)
//! - The proxy itself follows the unexpected death of its parent (the client) and exits
//!   (`exit_with_parent`; called right after `main` starts)
//!
//! The first stage, EOF on stdin (if the client dies the proxy's stdin closes, and if the
//! proxy dies the upstream's stdin closes), works regardless of the OS. What is here is the
//! second stage for unexpected deaths where no EOF arrives, implemented with per-OS
//! mechanisms:
//!
//! | OS      | The proxy follows the parent's death          | The upstream follows the proxy's death    |
//! | ------- | --------------------------------------------- | ----------------------------------------- |
//! | Linux   | `PR_SET_PDEATHSIG`                            | `PR_SET_PDEATHSIG`                        |
//! | macOS   | wait on the parent via `kqueue` `EVFILT_PROC` | none; left to EOF on the upstream's stdin |
//! | Windows | wait on the parent process handle             | Job Object `KILL_ON_JOB_CLOSE`            |

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

/// Launches the upstream command. stdin/stdout/stderr are all connected by pipes.
/// The launched upstream follows an unexpected death of the proxy via the OS mechanism
/// (table above).
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

/// Makes this process exit too when the parent (client) process dies unexpectedly.
/// Called once at the start of `main`.
///
/// This is a setting on the process itself, "signal when the calling process's parent dies",
/// and is independent of launching the upstream (`spawn`).
pub fn exit_with_parent() {
    platform::exit_with_parent();
}

impl Upstream {
    /// Checks whether the upstream is still alive. Does not block.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Waits for the upstream to exit (blocks).
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait()
    }

    /// Kills the upstream and waits for it to exit. Not an error if it has already exited.
    pub fn kill_and_wait(&mut self) -> io::Result<()> {
        match self.child.kill() {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::InvalidInput => {
                // kill on a process that has already exited. May be ignored.
            }
            Err(err) => return Err(err),
        }
        self.child.wait()?;
        Ok(())
    }
}

// Following the parent's death and taking the upstream down with us can only be verified in a
// multi-process scenario. Because `cargo test` runs multithreaded, `fork()` inside the test
// binary risks deadlock and is avoided. These are verified by the integration test
// `tests/process_lifetime.rs` (killing through a pseudo client) in CI on the 3 OSes
// (ADR 0012 decision D).

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::thread;
    use std::time::Duration;

    /// A launch specification that runs a one-line script in the OS shell.
    fn shell(script: &str) -> (&'static str, Vec<String>) {
        if cfg!(windows) {
            ("cmd", vec!["/C".to_string(), script.to_string()])
        } else {
            ("sh", vec!["-c".to_string(), script.to_string()])
        }
    }

    /// A command that passes stdin through to stdout as is.
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

    /// A command that waits 30 seconds.
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
        drop(handles.stdin); // send EOF to make it exit

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
        // Already exited by the time kill_and_wait returns.
        assert!(handles.upstream.try_wait().expect("try_wait").is_some());
    }

    #[test]
    fn kill_and_wait_is_idempotent_after_natural_exit() {
        let (program, args) = shell("exit 0");
        let mut handles = spawn(program, &args).expect("spawn exit 0");
        // Wait for the natural exit.
        for _ in 0..50 {
            if handles.upstream.try_wait().unwrap().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        // kill on an already dead process is not an error.
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
