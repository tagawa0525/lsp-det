//! A pseudo client. Only for the process lifetime test (`tests/process_lifetime.rs`).
//!
//! Launches the command given in the arguments with our own stdin / stdout / stderr
//! inherited, prints the child's pid to stderr, and then does nothing until killed.
//!
//! Inheriting stdin is the point. The child (lsp-det) directly holds the pipe the test owns,
//! so killing this pseudo client does not close the child's stdin. This creates a situation
//! where EOF does not cause an exit, and verifies that only the OS mechanism
//! (ADR 0012 decision B) makes the child exit.

use std::process::{Command, Stdio};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((program, rest)) = args.split_first() else {
        eprintln!("usage: pseudo_client <command> [args...]");
        std::process::exit(2);
    };
    // Not waiting on the child is intentional (do nothing until killed).
    #[allow(clippy::zombie_processes)]
    let child = Command::new(program)
        .args(rest)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap_or_else(|err| {
            eprintln!("pseudo-client: cannot start {program:?}: {err}");
            std::process::exit(1);
        });
    eprintln!("pseudo-client: child pid {}", child.id());
    // Wait until killed. Since the child is not waited on, this does not return even if the
    // child exits first.
    loop {
        std::thread::park();
    }
}
