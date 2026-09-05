//! macOS: wait for the parent process to exit with `kqueue` `EVFILT_PROC` / `NOTE_EXIT`.
//!
//! The thread that observes the parent's exit sends `SIGTERM` to the upstream and then exits
//! itself (the same chain as what happens with pdeathsig on Linux).
//!
//! macOS has no mechanism for the upstream to follow an unexpected death of the proxy
//! (`SIGKILL` etc.). The write side of the upstream's stdin disappears together with the
//! proxy, so the upstream exits on EOF (measured on the 4 language servers.
//! research/language-server-exit-on-stdin-eof.md).

use std::io;
use std::process::{Child, Command};
use std::ptr;
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread;

/// The target the thread that observes the parent's exit kills. Remembered by `spawn`.
static UPSTREAM_PID: AtomicI32 = AtomicI32::new(0);

pub fn prepare_upstream(_cmd: &mut Command) {}

pub fn follow_upstream(child: &Child) {
    UPSTREAM_PID.store(child.id() as i32, Ordering::SeqCst);
}

pub fn exit_with_parent() {
    // SAFETY: getppid takes no arguments and does not fail.
    let parent = unsafe { libc::getppid() };
    // SAFETY: kqueue takes no arguments. Failure is known from the return value.
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
    // SAFETY: changelist is a valid one-element array. eventlist is unused.
    let registered = unsafe { libc::kevent(kq, &change, 1, ptr::null_mut(), 0, ptr::null()) };
    if registered < 0 {
        eprintln!(
            "lsp-det: cannot watch parent process {parent}: {}; will not follow its death",
            io::Error::last_os_error()
        );
        // SAFETY: close the fd we opened ourselves, while no one else holds it.
        unsafe {
            libc::close(kq);
        }
        return;
    }

    thread::spawn(move || {
        // SAFETY: the kevent struct is valid for any bit pattern.
        let mut event: libc::kevent = unsafe { std::mem::zeroed() };
        loop {
            // SAFETY: eventlist is a valid one-element array. Blocks with no timeout.
            let n = unsafe { libc::kevent(kq, ptr::null(), 0, &mut event, 1, ptr::null()) };
            if n < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            break;
        }
        eprintln!("lsp-det: parent process exited; terminating the upstream and exiting");
        let upstream = UPSTREAM_PID.load(Ordering::SeqCst);
        if upstream > 0 {
            // SAFETY: sent only to the child we launched ourselves.
            unsafe {
                libc::kill(upstream, libc::SIGTERM);
            }
        }
        std::process::exit(1);
    });
}
