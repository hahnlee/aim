//! The thread-layer fork hooks: after `fork_prepare`, a Darwin fork and
//! `fork_child`, the child's only thread is its main thread (tid = pid) and
//! the layer's locks are usable in both processes.

use darwin_linux_abi::{context, diag, sys};

#[test]
fn fork_child_makes_the_forking_thread_the_main_thread() {
    context::init_thread();
    diag::install_signal_handlers();
    sys::fork_prepare();
    // SAFETY: plain fork; the child only uses async-signal-safe paths of
    // the layer and exits with _exit.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0);
    if pid == 0 {
        sys::fork_child();
        // SAFETY: trivial.
        let ok = sys::host_tid() == unsafe { libc::getpid() } as i64;
        // The table and futex locks work again: a second prepare/parent
        // round trip takes them all.
        sys::fork_prepare();
        sys::fork_parent();
        // SAFETY: leaving the child without running test harness code.
        unsafe { libc::_exit(if ok { 0 } else { 1 }) };
    }
    sys::fork_parent();
    let mut status = 0;
    // SAFETY: waiting for our child.
    unsafe { libc::waitpid(pid, &mut status, 0) };
    assert!(
        libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
        "child status {status:#x}"
    );
}
