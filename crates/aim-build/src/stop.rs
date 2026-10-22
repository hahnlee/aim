//! The guest a command runs, stopped when the command is: SIGINT, SIGTERM
//! and SIGHUP go to a thread of their own, which stops what is running as
//! the command's own cleanup would and exits as the signal would have.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
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

/// Blocks the signals in this thread before any other starts, so every
/// later thread inherits the mask (children get the default mask from
/// `Command`), and waits for them on a thread of their own.
pub(crate) fn on_signals(command: &'static str) {
    // SAFETY: a local signal set, blocked in this thread.
    let set = unsafe {
        let mut set: libc::sigset_t = 0;
        libc::sigemptyset(&mut set);
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            libc::sigaddset(&mut set, signal);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    };
    std::thread::spawn(move || {
        let mut signal = 0;
        // SAFETY: waiting for one of the signals blocked above.
        unsafe { libc::sigwait(&set, &mut signal) };
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
