//! The guest a command runs, stopped when the command is: SIGINT, SIGTERM
//! and SIGHUP wake a thread of their own, which stops what is running as
//! the command's own cleanup would and exits as the signal would have.

use std::io::Read;
use std::os::fd::IntoRawFd;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

/// The children running: aim-display's and guest-init's pids, and
/// aim-apps's with its binary and the shims' directory it cleans.
#[derive(Clone, Default)]
pub(crate) struct Running {
    pub(crate) display: Option<u32>,
    pub(crate) init: Option<u32>,
    pub(crate) apps: Option<(u32, PathBuf, PathBuf)>,
}

static RUNNING: Mutex<Running> = Mutex::new(Running {
    display: None,
    init: None,
    apps: None,
});

/// Records what a signal stops.
pub(crate) fn running(now: Running) {
    *RUNNING.lock().unwrap() = now;
}

/// Nothing is running any more: the command has stopped it itself.
pub(crate) fn forget() {
    running(Running::default());
}

/// The write end of the pipe the signal handler wakes the stop thread by.
static WAKE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn caught(signal: libc::c_int) {
    let byte = signal as u8;
    // SAFETY: write(2) is async-signal-safe; one byte into our pipe.
    unsafe { libc::write(WAKE.load(Ordering::Relaxed), (&byte as *const u8).cast(), 1) };
}

/// Handles the signals by waking a thread of their own. They are caught,
/// not blocked: a blocked mask is inherited by every child, which would
/// then never see the SIGTERM that stops it (#2704).
pub(crate) fn on_signals(command: &'static str) {
    let (mut reader, writer) = std::io::pipe().expect("signal pipe");
    WAKE.store(writer.into_raw_fd(), Ordering::Relaxed);
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        // SAFETY: installing a handler that only writes to the pipe.
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = caught as extern "C" fn(libc::c_int) as libc::sighandler_t;
            action.sa_flags = libc::SA_RESTART;
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
    std::thread::spawn(move || {
        let mut byte = [0u8];
        if reader.read_exact(&mut byte).is_err() {
            return;
        }
        let signal = i32::from(byte[0]);
        let now = RUNNING.lock().unwrap().clone();
        eprintln!("{command}: signal {signal}: stopping the guest");
        stop(now);
        std::process::exit(128 + signal);
    });
}

/// Stops guest-init (which stops its services on SIGTERM), killing it
/// after a minute, then aim-display, then aim-apps, whose shims go out of
/// Launch Services.
pub(crate) fn stop(now: Running) {
    let reaped = |pid: u32| {
        let mut status = 0;
        // SAFETY: polling a child we started.
        unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) != 0 }
    };
    if let Some(init) = now.init {
        // SAFETY: signals a child we started, not yet reaped.
        unsafe { libc::kill(init as i32, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(60);
        while !reaped(init) {
            if Instant::now() > deadline {
                // SAFETY: as above.
                unsafe { libc::kill(init as i32, libc::SIGKILL) };
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if let Some(display) = now.display {
        // SAFETY: as above.
        unsafe { libc::kill(display as i32, libc::SIGTERM) };
        while !reaped(display) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    if let Some((apps, binary, into)) = now.apps {
        // SAFETY: as above.
        unsafe { libc::kill(apps as i32, libc::SIGKILL) };
        while !reaped(apps) {
            std::thread::sleep(Duration::from_millis(100));
        }
        clean_shims(&binary, &into);
    }
}

/// `aim-apps clean --into DIR`: the shims go with the guest, and out of
/// Launch Services.
pub(crate) fn clean_shims(binary: &std::path::Path, into: &std::path::Path) {
    let cleaned = Command::new(binary)
        .args(["clean", "--into"])
        .arg(into)
        .status();
    if !cleaned.is_ok_and(|s| s.success()) {
        eprintln!("aim-apps clean {}: failed", into.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A child started once the signals are handled can still be stopped
    /// by SIGTERM: the handling leaves it no blocked mask (#2704).
    #[test]
    fn handling_the_signals_leaves_children_stoppable() {
        on_signals("test");
        let mut child = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        // SAFETY: signals the child this test started, not yet reaped.
        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        let status = child.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGTERM));
    }

    /// A stopped boot stops aim-apps too and cleans its shims (#796).
    #[test]
    fn stopping_stops_the_shims_and_cleans_them() {
        let dir = std::env::temp_dir().join(format!("stop-apps-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        // A stand-in aim-apps whose `clean --into DIR` leaves a marker.
        let binary = dir.join("aim-apps");
        std::fs::write(
            &binary,
            "#!/bin/sh\n[ \"$1 $2\" = 'clean --into' ] && touch \"$3/cleaned\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let apps = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        stop(Running {
            display: None,
            init: None,
            apps: Some((apps.id(), binary, dir.clone())),
        });
        assert!(dir.join("cleaned").exists(), "the shims were not cleaned");
        // SAFETY: probing a pid this test started and stop reaped.
        assert_eq!(unsafe { libc::kill(apps.id() as i32, 0) }, -1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
