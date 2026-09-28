use std::io;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicBool, Ordering};

static TERMINATION_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn request_termination(_: libc::c_int) {
    TERMINATION_REQUESTED.store(true, Ordering::Relaxed);
}

pub(crate) fn install() -> io::Result<()> {
    TERMINATION_REQUESTED.store(false, Ordering::Relaxed);
    let mut action = MaybeUninit::<libc::sigaction>::zeroed();
    // SAFETY: sigaction is initialized before publication. The handler only
    // stores to a lock-free process-global atomic and calls no runtime code.
    unsafe {
        let action = action.assume_init_mut();
        action.sa_sigaction = request_termination as libc::sighandler_t;
        action.sa_flags = 0;
        libc::sigemptyset(&mut action.sa_mask);
        if libc::sigaction(libc::SIGTERM, action, std::ptr::null_mut()) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

pub(crate) fn termination_requested() -> bool {
    TERMINATION_REQUESTED.load(Ordering::Relaxed)
}
