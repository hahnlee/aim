//! The guest a command runs, stopped when the command is: SIGINT, SIGTERM
//! and SIGHUP wake a thread of their own, which stops what is running as
//! the command's own cleanup would and exits as the signal would have.
//! One stop runs at a time, so a signal during the command's own cleanup
//! waits for it rather than ending the process in its middle. A second
//! signal ends the command at once; a data image it leaves attached is
//! detached by the next attach of that data directory.

use std::io::Read;
use std::os::fd::IntoRawFd;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// The children running: aim-display's and guest-init's pids, and
/// aim-apps's with its binary and the shims' directory it cleans; and the
/// data directory whose image guest-init hands to its parent to detach.
#[derive(Clone, Default)]
pub(crate) struct Running {
    pub(crate) display: Option<u32>,
    pub(crate) init: Option<u32>,
    pub(crate) apps: Option<(u32, PathBuf, PathBuf)>,
    pub(crate) data: Option<PathBuf>,
}

static RUNNING: Mutex<Running> = Mutex::new(Running {
    display: None,
    init: None,
    apps: None,
    data: None,
});

/// Held by the stop under way.
static STOPPING: Mutex<()> = Mutex::new(());

/// Records what a signal stops.
pub(crate) fn running(now: Running) {
    *RUNNING.lock().unwrap() = now;
}

/// The command stops its children itself while the guard lives; a signal
/// meanwhile waits for it, then finds only the data image to detach.
pub(crate) fn stopping() -> MutexGuard<'static, ()> {
    let guard = STOPPING.lock().unwrap_or_else(|e| e.into_inner());
    let mut now = RUNNING.lock().unwrap();
    *now = Running {
        data: now.data.take(),
        ..Default::default()
    };
    guard
}

/// The write end of the pipe the signal handler wakes the stop thread by.
static WAKE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn caught(signal: libc::c_int) {
    let byte = signal as u8;
    // SAFETY: async-signal-safe calls: one byte into our non-blocking pipe
    // (a full pipe already holds a wake-up), with the interrupted code's
    // errno kept.
    unsafe {
        let errno = *libc::__error();
        libc::write(WAKE.load(Ordering::Relaxed), (&byte as *const u8).cast(), 1);
        *libc::__error() = errno;
    }
}

/// Handles the signals by waking a thread of their own. They are caught,
/// not blocked: a blocked mask is inherited by every child, which would
/// then never see the SIGTERM that stops it (#2704).
pub(crate) fn on_signals(command: &'static str) {
    let (mut reader, writer) = std::io::pipe().expect("signal pipe");
    let writer = writer.into_raw_fd();
    // SAFETY: flags of our own descriptor.
    unsafe {
        let flags = libc::fcntl(writer, libc::F_GETFL);
        libc::fcntl(writer, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    WAKE.store(writer, Ordering::Relaxed);
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
        eprintln!("{command}: signal {signal}: stopping the guest");
        std::thread::spawn(move || {
            if reader.read_exact(&mut byte).is_ok() {
                eprintln!("{command}: signal {}: not waiting for the stop", byte[0]);
                std::process::exit(128 + i32::from(byte[0]));
            }
        });
        let _stopping = STOPPING.lock().unwrap_or_else(|e| e.into_inner());
        let now = std::mem::take(&mut *RUNNING.lock().unwrap());
        if let Err(error) = stop_now(now) {
            eprintln!("{command}: {error}");
        }
        std::process::exit(128 + signal);
    });
}

/// Stops what is running, as [`stop_now`], once no other stop is under
/// way; a signal from then on finds nothing left to stop.
pub(crate) fn stop(now: Running) -> Result<(), String> {
    let _stopping = STOPPING.lock().unwrap_or_else(|e| e.into_inner());
    *RUNNING.lock().unwrap() = Running::default();
    stop_now(now)
}

/// Stops guest-init (which stops its services on SIGTERM), killing it
/// after a minute, then aim-display, then aim-apps, whose shims go out of
/// Launch Services. Then, with none of them left to hold it, detaches the
/// data image guest-init handed off, as aimctl does after guest-init
/// exits.
fn stop_now(now: Running) -> Result<(), String> {
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
        end_group(init);
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
    match now.data {
        Some(data) => aim_storage::data::detach_existing(&data)
            .map_err(|e| format!("detaching {}: {e}", data.display())),
        None => Ok(()),
    }
}

/// Kills what is left of the process group of guest-init, reaped (it
/// leads a group of its own), so that nothing of it keeps the data volume
/// busy. Its services lead groups of their own, beyond this: one that
/// outlives a killed guest-init keeps the volume busy, and the detach
/// fails rather than forces it.
pub(crate) fn end_group(init: u32) {
    // SAFETY: the group stays allocated while any member lives; an empty
    // one is ESRCH.
    unsafe { libc::killpg(init as i32, libc::SIGKILL) };
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
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;

    /// A child started once the signals are handled can still be stopped
    /// by SIGTERM: the handling leaves it no blocked mask (#2704). The
    /// handling is process-wide, so it runs in a test process of its own.
    #[test]
    fn handling_the_signals_leaves_children_stoppable() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "stop::tests::handled_signals_child",
                "--ignored",
                "--quiet",
            ])
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    #[ignore = "runs in the process of handling_the_signals_leaves_children_stoppable"]
    fn handled_signals_child() {
        on_signals("test");
        let mut child = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        // SAFETY: signals the child this test started, not yet reaped.
        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break Some(status);
            }
            if Instant::now() > deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                break None;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.and_then(|s| s.signal()), Some(libc::SIGTERM));
    }

    /// The stop detaches the data image guest-init handed off, and says
    /// when it cannot: here a live owner still holds the image (#2766).
    #[test]
    fn stopping_detaches_the_data_image_or_says_why_not() {
        let dir = std::env::temp_dir().join(format!("stop-data-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        let data = dir.join("data");
        std::fs::write(aim_storage::data::image_of(&data), b"not attached").unwrap();
        let owner = std::fs::File::create(dir.join("data.lock")).unwrap();
        // SAFETY: flock on an open descriptor.
        assert_eq!(
            unsafe { libc::flock(owner.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        let stopped = stop(Running {
            data: Some(data.clone()),
            ..Default::default()
        });
        assert!(stopped.unwrap_err().contains("in use"));
        drop(owner);
        stop(Running {
            data: Some(data),
            ..Default::default()
        })
        .unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// guest-init's exit leaves its data image attached for its parent;
    /// the stop detaches it (#2766).
    #[test]
    #[ignore = "creates and attaches one owned disposable APFS data image"]
    fn stopping_detaches_an_image_guest_init_handed_off() {
        let dir = std::env::temp_dir().join(format!("stop-handoff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        let data = dir.join("data");
        /// Detaches and removes the image however the test ends.
        struct Removed(PathBuf);
        impl Drop for Removed {
            fn drop(&mut self) {
                if let Err(error) = aim_storage::data::remove(&self.0.join("data")) {
                    eprintln!("{error}");
                }
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _removed = Removed(dir.clone());
        let mut image = aim_storage::data::DataImage::attach(&data, None).unwrap();
        let backing = image.image().to_path_buf();
        image.handoff_detach().unwrap();
        drop(image);
        assert!(
            !aim_storage::disk::attachments_of(&backing)
                .unwrap()
                .is_empty()
        );
        stop(Running {
            data: Some(data.clone()),
            ..Default::default()
        })
        .unwrap();
        assert!(
            aim_storage::disk::attachments_of(&backing)
                .unwrap()
                .is_empty()
        );
        assert!(!aim_storage::disk::is_mount_point(&data));
        aim_storage::data::remove(&data).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
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
            apps: Some((apps.id(), binary, dir.clone())),
            ..Default::default()
        })
        .unwrap();
        assert!(dir.join("cleaned").exists(), "the shims were not cleaned");
        // SAFETY: probing a pid this test started and stop reaped.
        assert_eq!(unsafe { libc::kill(apps.id() as i32, 0) }, -1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
