//! A user's emulated storage (docs/app-host.md): the app host, in
//! StorageManagerService's place, asks the agent to mount it; the agent
//! asks vold as StorageManagerService does (`IVold.mount` of the primary
//! emulated volume, visible for writing), and the FUSE connection vold
//! hands its callback is served as MediaProvider's FuseDaemon serves it
//! (`aim_emulated_storage`), by a process of the agent's program as
//! MediaProvider: its uid, and the groups `ProcessList` gives the FUSE
//! daemon (`MOUNT_EXTERNAL_PASS_THROUGH`), so the lower files are owned as
//! the original's.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::sync::Arc;

use aim_emulated_storage::{BUFFER_SIZE, Daemon, op};
use android_os_vold::aidl::android::os::IVold::IVold;
use android_os_vold::aidl::android::os::IVoldMountCallback::{BnVoldMountCallback, IVoldMountCallback};
use binder::{BinderFeatures, Interface};

/// `IVold.MOUNT_FLAG_PRIMARY`, `MOUNT_FLAG_VISIBLE_FOR_WRITE`.
const MOUNT_FLAG_PRIMARY: i32 = 1;
const MOUNT_FLAG_VISIBLE_FOR_WRITE: i32 = 4;
/// The daemon's threads: requests are answered concurrently, as the
/// kernel's FUSE allows.
const THREADS: usize = 4;

/// The serving process's first argument, and the descriptor it finds the
/// connection at.
pub const SERVE: &str = "--serve-storage";
const SERVED_FD: i32 = 3;
/// `Process.SDCARD_RW_GID`, `EXT_DATA_RW_GID`, `EXT_OBB_RW_GID`,
/// `MEDIA_RW_GID`; `UserHandle.PER_USER_RANGE`.
const SDCARD_RW: u32 = 1015;
const EXT_DATA_RW: u32 = 1078;
const EXT_OBB_RW: u32 = 1079;
const MEDIA_RW: u32 = 1023;
const PER_USER_RANGE: u32 = 100_000;

/// Mounts `user`'s emulated volume through vold, served as `daemon_uid`
/// (MediaProvider's).
pub fn mount(user: i32, daemon_uid: u32) -> Result<(), String> {
    let vold: binder::Strong<dyn IVold> = binder::wait_for_interface("vold").map_err(|e| format!("vold: {e}"))?;
    let callback = BnVoldMountCallback::new_binder(Callback { user: user as u32, uid: daemon_uid }, BinderFeatures::default());
    vold.mount(&format!("emulated;{user}"), MOUNT_FLAG_PRIMARY | MOUNT_FLAG_VISIBLE_FOR_WRITE, user, Some(&callback))
        .map_err(|e| format!("mount emulated;{user}: {e}"))
}

struct Callback {
    user: u32,
    uid: u32,
}

impl Interface for Callback {}

impl IVoldMountCallback for Callback {
    /// The volume's FUSE connection: served from `internal_path`, its
    /// lower path, by a process of its own as the daemon's uid.
    fn onVolumeChecking(&self, fd: &android_os_vold::RawFileDescriptor, path: &str, internal_path: &str) -> binder::Result<bool> {
        let fd = fd.as_raw_fd();
        let (uid, sdcard_rw) = (self.uid, SDCARD_RW + self.user * PER_USER_RANGE);
        // The agent's own program, as init started it.
        let program = std::env::args_os().next().ok_or(binder::StatusCode::UNKNOWN_ERROR)?;
        let mut command = std::process::Command::new(program);
        command.arg(SERVE).arg(internal_path);
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            command.pre_exec(move || {
                let groups = [sdcard_rw, EXT_DATA_RW, EXT_OBB_RW, MEDIA_RW];
                if libc::dup2(fd, SERVED_FD) < 0
                    || libc::setgroups(groups.len(), groups.as_ptr()) != 0
                    || libc::setresgid(uid, uid, uid) != 0
                    || libc::setresuid(uid, uid, uid) != 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        match command.spawn() {
            Ok(child) => {
                log::info!("serving {path} from {internal_path} as {uid} (pid {})", child.id());
                // init reaps it with the agent's other orphans once the
                // agent goes; until then it serves.
                std::mem::forget(child);
                Ok(true)
            }
            Err(error) => {
                log::error!("emulated storage daemon: {error}");
                Ok(false)
            }
        }
    }
}

/// The serving process's main: the connection at its descriptor, the
/// lower path its argument.
pub fn serve_main(lower: &str) -> ! {
    // SAFETY: the descriptor the agent left at it, owned by nothing else.
    let fd = Arc::new(unsafe { OwnedFd::from_raw_fd(SERVED_FD) });
    let daemon = Arc::new(Daemon::new(lower));
    let threads: Vec<_> = (0..THREADS)
        .map(|_| {
            let (fd, daemon) = (fd.clone(), daemon.clone());
            std::thread::spawn(move || serve(&fd, &daemon))
        })
        .collect();
    for thread in threads {
        let _ = thread.join();
    }
    std::process::exit(0)
}

/// Reads requests and writes their replies until the connection ends.
fn serve(fd: &OwnedFd, daemon: &Daemon) {
    let mut buffer = vec![0u8; BUFFER_SIZE];
    loop {
        // SAFETY: a read into a buffer of its length.
        let n = unsafe { libc::read(fd.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if n < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            // ENODEV: unmounted.
            return log::info!("emulated storage connection: {error}");
        }
        let request = &buffer[..n as usize];
        let destroy = aim_emulated_storage::header(request).is_some_and(|h| h.opcode == op::DESTROY);
        if let Some(reply) = daemon.handle(request) {
            // SAFETY: a write of a whole reply.
            if unsafe { libc::write(fd.as_raw_fd(), reply.as_ptr().cast(), reply.len()) } < 0 {
                let error = std::io::Error::last_os_error();
                // ENOENT: the request was interrupted and is gone.
                if error.raw_os_error() != Some(libc::ENOENT) {
                    return log::error!("emulated storage reply: {error}");
                }
            }
        }
        if destroy {
            return;
        }
    }
}
