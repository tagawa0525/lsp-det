//! Linux: `PR_SET_PDEATHSIG`. When the parent dies, the child receives a signal.
//!
//! The same setting is applied to both the proxy itself and the upstream. The upstream's
//! setting is done in `pre_exec` (after fork, before exec), so it works whatever the upstream
//! command is.

use std::os::unix::process::CommandExt;
use std::process::{Child, Command};

pub fn prepare_upstream(cmd: &mut Command) {
    // SAFETY: only async-signal-safe operations are done inside pre_exec
    // (prctl is a single system call).
    unsafe {
        cmd.pre_exec(|| {
            set_pdeathsig_on_self();
            Ok(())
        });
    }
}

pub fn follow_upstream(_child: &Child) {
    // Already set in pre_exec.
}

pub fn exit_with_parent() {
    set_pdeathsig_on_self();
}

fn set_pdeathsig_on_self() {
    // SAFETY: the arguments are constants, and failure has no side effects.
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
    }
}
