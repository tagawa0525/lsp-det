//! macOS: `kqueue` の `EVFILT_PROC` / `NOTE_EXIT` で親プロセスの終了を待つ。
//!
//! 親の終了を観測したスレッドが、上流に `SIGTERM` を送ってから自分を終了
//! する (Linux の pdeathsig で起きることと同じ連鎖)。
//!
//! 上流がプロキシの不意の死 (`SIGKILL` 等) に追従する機構は macOS にない。
//! プロキシと共に上流の stdin の書き込み側が消えるので、上流は EOF で終了
//! する (4 つの言語サーバーで実測済み。research/language-server-exit-on-stdin-eof.md)。

use std::io;
use std::process::{Child, Command};
use std::ptr;
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread;

/// 親の終了を観測したスレッドが殺す相手。`spawn` が覚える。
static UPSTREAM_PID: AtomicI32 = AtomicI32::new(0);

pub fn prepare_upstream(_cmd: &mut Command) {}

pub fn follow_upstream(child: &Child) {
    UPSTREAM_PID.store(child.id() as i32, Ordering::SeqCst);
}

pub fn exit_with_parent() {
    // SAFETY: getppid は引数を取らず失敗しない。
    let parent = unsafe { libc::getppid() };
    // SAFETY: kqueue は引数を取らない。失敗は戻り値で分かる。
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        eprintln!(
            "lsp-det: kqueue failed: {}; will not follow the parent's death",
            io::Error::last_os_error()
        );
        return;
    }
    let change = libc::kevent {
        ident: parent as libc::uintptr_t,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: ptr::null_mut(),
    };
    // SAFETY: changelist は 1 要素の有効な配列。eventlist は使わない。
    let registered = unsafe { libc::kevent(kq, &change, 1, ptr::null_mut(), 0, ptr::null()) };
    if registered < 0 {
        eprintln!(
            "lsp-det: cannot watch parent process {parent}: {}; will not follow its death",
            io::Error::last_os_error()
        );
        return;
    }

    thread::spawn(move || {
        // SAFETY: kevent 構造体はどのビットパターンでも有効。
        let mut event: libc::kevent = unsafe { std::mem::zeroed() };
        loop {
            // SAFETY: eventlist は 1 要素の有効な配列。timeout なしでブロックする。
            let n = unsafe { libc::kevent(kq, ptr::null(), 0, &mut event, 1, ptr::null()) };
            if n < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            break;
        }
        eprintln!("lsp-det: parent process exited; terminating the upstream and exiting");
        let upstream = UPSTREAM_PID.load(Ordering::SeqCst);
        if upstream > 0 {
            // SAFETY: 自分が起動した子にだけ送る。
            unsafe {
                libc::kill(upstream, libc::SIGTERM);
            }
        }
        std::process::exit(1);
    });
}
