//! Linux: `PR_SET_PDEATHSIG`。親が死んだら子にシグナルが届く。
//!
//! プロキシ自身にも上流にも同じ設定をする。上流の設定は `pre_exec`
//! (fork の後、exec の前) で行うので、上流のコマンドが何であっても効く。

use std::os::unix::process::CommandExt;
use std::process::{Child, Command};

pub fn prepare_upstream(cmd: &mut Command) {
    // SAFETY: pre_exec の中では async-signal-safe な操作しかしない
    // (prctl はシステムコール 1 つ)。
    unsafe {
        cmd.pre_exec(|| {
            set_pdeathsig_on_self();
            Ok(())
        });
    }
}

pub fn follow_upstream(_child: &Child) {
    // pre_exec で設定済み。
}

pub fn exit_with_parent() {
    set_pdeathsig_on_self();
}

fn set_pdeathsig_on_self() {
    // SAFETY: 引数は定数で、失敗しても副作用はない。
    unsafe {
        libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
    }
}
