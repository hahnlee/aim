//! `vold` of the derived image (ADR 0012 appendix, "Replaced native
//! daemons"): IVold for a /data that is a host directory.
//!
//! What vold does with the kernel (block devices, loop devices, dm-crypt,
//! fscrypt keys, FUSE and bind mounts) has no counterpart here, so:
//! - user storage is the directories vold would prepare, with no keys
//!   (every user's credential-encrypted storage counts as unlocked once
//!   asked to be);
//! - the emulated volume of each started user is `/data/media/<user>`,
//!   made visible by symlinks where vold would mount it
//!   (`storage::link_emulated`); MediaProvider's FUSE daemon is not
//!   started, since vold never offers it a FUSE fd;
//! - there are no disks, partitions, OBBs, AppFuse, incfs, checkpoints or
//!   device statistics: those calls fail with UNSUPPORTED_OPERATION or
//!   report nothing, as vold does on a device without the feature.

mod storage;

use std::sync::Mutex;

use android_os_vold::aidl::android::os::{
    IVold::{self, BnVold},
    IVoldListener::IVoldListener,
    IVoldMountCallback::IVoldMountCallback,
    IVoldTaskListener::IVoldTaskListener,
    incremental::IncrementalFileSystemControlParcel::IncrementalFileSystemControlParcel,
};
use binder::{
    BinderFeatures, ExceptionCode, Interface, ParcelFileDescriptor, PersistableBundle, Status,
    Strong,
};

const SERVICE: &str = "vold";

#[derive(Default)]
struct State {
    listener: Option<Strong<dyn IVoldListener>>,
    /// Users whose emulated volume exists (started users).
    started: Vec<i32>,
    unlocked: Vec<i32>,
}

#[derive(Default)]
struct Vold {
    state: Mutex<State>,
}

fn unsupported<T>(what: &str) -> binder::Result<T> {
    log::warn!("{what}: not supported on this device");
    Err(Status::new_exception_str(
        ExceptionCode::UNSUPPORTED_OPERATION,
        Some(format!("{what}: not supported on this device")),
    ))
}

fn io_error(what: &str, e: std::io::Error) -> Status {
    log::error!("{what}: {e}");
    Status::new_service_specific_error_str(
        -e.raw_os_error().unwrap_or(libc::EIO),
        Some(format!("{what}: {e}")),
    )
}

fn emulated_id(user: i32) -> String {
    format!("emulated;{user}")
}

impl Vold {
    fn listener(&self) -> Option<Strong<dyn IVoldListener>> {
        self.state.lock().unwrap().listener.clone()
    }

    /// VolumeManager::onUserStarted: create the user's emulated volume.
    fn create_emulated(&self, user: i32) {
        {
            let mut s = self.state.lock().unwrap();
            if s.started.contains(&user) {
                return;
            }
            s.started.push(user);
        }
        if let Some(l) = self.listener() {
            let id = emulated_id(user);
            let _ = l.onVolumeCreated(&id, IVold::VOLUME_TYPE_EMULATED, "", "", user);
            let _ = l.onVolumeStateChanged(&id, IVold::VOLUME_STATE_UNMOUNTED, user);
        }
    }

    fn destroy_emulated(&self, user: i32) {
        self.state.lock().unwrap().started.retain(|&u| u != user);
        if let Some(l) = self.listener() {
            let id = emulated_id(user);
            let _ = l.onVolumeStateChanged(&id, IVold::VOLUME_STATE_REMOVED, user);
            let _ = l.onVolumeDestroyed(&id);
        }
    }
}

/// Report a finished maintenance task with no work done.
fn finish_task(listener: &Strong<dyn IVoldTaskListener>) -> binder::Result<()> {
    listener.onFinished(0, &PersistableBundle::new())
}

impl Interface for Vold {}

#[allow(non_snake_case)]
impl IVold::IVold for Vold {
    fn setListener(&self, listener: &Strong<dyn IVoldListener>) -> binder::Result<()> {
        self.state.lock().unwrap().listener = Some(listener.clone());
        Ok(())
    }
    fn abortFuse(&self) -> binder::Result<()> {
        Ok(())
    }
    fn monitor(&self) -> binder::Result<()> {
        Ok(())
    }
    fn reset(&self) -> binder::Result<()> {
        // VolumeManager::reset: volumes are recreated as users start.
        let started = std::mem::take(&mut self.state.lock().unwrap().started);
        for user in started {
            self.destroy_emulated(user);
        }
        Ok(())
    }
    fn shutdown(&self) -> binder::Result<()> {
        self.reset()
    }
    fn onUserAdded(&self, _: i32, _: i32, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn onUserRemoved(&self, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn onUserStarted(&self, user: i32) -> binder::Result<()> {
        self.create_emulated(user);
        Ok(())
    }
    fn onUserStopped(&self, user: i32) -> binder::Result<()> {
        self.destroy_emulated(user);
        Ok(())
    }
    fn addAppIds(&self, _: &[String], _: &[i32]) -> binder::Result<()> {
        Ok(())
    }
    fn addSandboxIds(&self, _: &[i32], _: &[String]) -> binder::Result<()> {
        Ok(())
    }
    fn onSecureKeyguardStateChanged(&self, _: bool) -> binder::Result<()> {
        Ok(())
    }
    fn partition(&self, _: &str, _: i32, _: i32) -> binder::Result<()> {
        unsupported("partition")
    }
    fn forgetPartition(&self, _: &str, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn mount(
        &self,
        vol_id: &str,
        _: i32,
        user: i32,
        _: Option<&Strong<dyn IVoldMountCallback>>,
    ) -> binder::Result<()> {
        let Some(owner) = vol_id
            .strip_prefix("emulated;")
            .and_then(|u| u.parse::<i32>().ok())
        else {
            return unsupported(&format!("mount {vol_id}"));
        };
        let listener = self.listener();
        if let Some(l) = &listener {
            let _ = l.onVolumeStateChanged(vol_id, IVold::VOLUME_STATE_CHECKING, user);
        }
        if let Err(e) = storage::link_emulated(owner as u32) {
            if let Some(l) = &listener {
                let _ = l.onVolumeStateChanged(vol_id, IVold::VOLUME_STATE_UNMOUNTABLE, user);
            }
            return Err(io_error(&format!("mount {vol_id}"), e));
        }
        if let Some(l) = &listener {
            let _ = l.onVolumeInternalPathChanged(vol_id, "/data/media");
            let _ = l.onVolumePathChanged(vol_id, "/storage/emulated");
            let _ = l.onVolumeStateChanged(vol_id, IVold::VOLUME_STATE_MOUNTED, user);
        }
        log::info!("{vol_id} mounted: /storage/emulated -> /data/media");
        Ok(())
    }
    fn unmount(&self, vol_id: &str) -> binder::Result<()> {
        if let (Some(l), Some(user)) = (
            self.listener(),
            vol_id
                .strip_prefix("emulated;")
                .and_then(|u| u.parse().ok()),
        ) {
            let _ = l.onVolumeStateChanged(vol_id, IVold::VOLUME_STATE_EJECTING, user);
            let _ = l.onVolumeStateChanged(vol_id, IVold::VOLUME_STATE_UNMOUNTED, user);
        }
        Ok(())
    }
    fn format(&self, _: &str, _: &str) -> binder::Result<()> {
        unsupported("format")
    }
    fn benchmark(&self, _: &str, _: &Strong<dyn IVoldTaskListener>) -> binder::Result<()> {
        unsupported("benchmark")
    }
    fn moveStorage(
        &self,
        _: &str,
        _: &str,
        _: &Strong<dyn IVoldTaskListener>,
    ) -> binder::Result<()> {
        unsupported("moveStorage")
    }
    fn remountUid(&self, _: i32, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn remountAppStorageDirs(&self, _: i32, _: i32, _: &[String]) -> binder::Result<()> {
        Ok(())
    }
    fn unmountAppStorageDirs(&self, _: i32, _: i32, _: &[String]) -> binder::Result<()> {
        Ok(())
    }
    fn setupAppDir(&self, path: &str, app_uid: i32) -> binder::Result<()> {
        self.ensureAppDirsCreated(&[path.to_string()], app_uid)
    }
    fn fixupAppDir(&self, path: &str, app_uid: i32) -> binder::Result<()> {
        self.ensureAppDirsCreated(&[path.to_string()], app_uid)
    }
    fn ensureAppDirsCreated(&self, paths: &[String], app_uid: i32) -> binder::Result<()> {
        // PrepareAppDirFromRoot: the app's directory and the parents it
        // lacks, owned by the app (and ext_data_rw / ext_obb_rw), in the
        // volume's lower path.
        for path in paths {
            let Some(lower) = storage::lower_path(path) else {
                return Err(Status::new_exception_str(
                    ExceptionCode::ILLEGAL_ARGUMENT,
                    Some(format!("not an app directory: {path}")),
                ));
            };
            std::fs::create_dir_all(&lower).map_err(|e| io_error(path, e))?;
            let gid = if path.contains("/Android/obb/") {
                1079
            } else {
                1078
            };
            storage::prepare_dir(&lower, 0o2770, app_uid as u32, gid)
                .map_err(|e| io_error(path, e))?;
        }
        Ok(())
    }
    fn createObb(&self, _: &str, _: i32) -> binder::Result<String> {
        unsupported("createObb")
    }
    fn destroyObb(&self, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn fstrim(&self, _: i32, listener: &Strong<dyn IVoldTaskListener>) -> binder::Result<()> {
        finish_task(listener)
    }
    fn runIdleMaint(
        &self,
        _: bool,
        listener: &Strong<dyn IVoldTaskListener>,
    ) -> binder::Result<()> {
        finish_task(listener)
    }
    fn abortIdleMaint(&self, listener: &Strong<dyn IVoldTaskListener>) -> binder::Result<()> {
        finish_task(listener)
    }
    fn getStorageLifeTime(&self) -> binder::Result<i32> {
        Ok(-1)
    }
    fn setGCUrgentPace(
        &self,
        _: i32,
        _: i32,
        _: f32,
        _: f32,
        _: i32,
        _: i32,
        _: i32,
    ) -> binder::Result<()> {
        Ok(())
    }
    fn refreshLatestWrite(&self) -> binder::Result<()> {
        Ok(())
    }
    fn getWriteAmount(&self) -> binder::Result<i32> {
        Ok(-1)
    }
    fn mountAppFuse(&self, _: i32, _: i32) -> binder::Result<ParcelFileDescriptor> {
        unsupported("mountAppFuse")
    }
    fn unmountAppFuse(&self, _: i32, _: i32) -> binder::Result<()> {
        unsupported("unmountAppFuse")
    }
    fn fbeEnable(&self) -> binder::Result<()> {
        Ok(())
    }
    fn initUser0(&self) -> binder::Result<()> {
        let dirs = storage::init_user0().map_err(|e| io_error("initUser0", e))?;
        log::info!("initialized user 0: {} directories", dirs.len());
        Ok(())
    }
    fn mountFstab(&self, _: &str, _: &str, _: bool, _: &[String]) -> binder::Result<()> {
        unsupported("mountFstab")
    }
    fn encryptFstab(
        &self,
        _: &str,
        _: &str,
        _: bool,
        _: &str,
        _: bool,
        _: &[String],
        _: &[bool],
        _: i64,
    ) -> binder::Result<()> {
        unsupported("encryptFstab")
    }
    fn setStorageBindingSeed(&self, _: &[u8]) -> binder::Result<()> {
        Ok(())
    }
    fn createUserStorageKeys(&self, _: i32, _: bool) -> binder::Result<()> {
        Ok(())
    }
    fn destroyUserStorageKeys(&self, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn setCeStorageProtection(&self, _: i32, _: &[u8]) -> binder::Result<()> {
        Ok(())
    }
    fn getUnlockedUsers(&self) -> binder::Result<Vec<i32>> {
        Ok(self.state.lock().unwrap().unlocked.clone())
    }
    fn unlockCeStorage(&self, user: i32, _: &[u8]) -> binder::Result<()> {
        let mut s = self.state.lock().unwrap();
        if !s.unlocked.contains(&user) {
            s.unlocked.push(user);
        }
        Ok(())
    }
    fn lockCeStorage(&self, user: i32) -> binder::Result<()> {
        self.state.lock().unwrap().unlocked.retain(|&u| u != user);
        Ok(())
    }
    fn prepareUserStorage(&self, uuid: Option<&str>, user: i32, flags: i32) -> binder::Result<()> {
        let dirs = storage::prepare_user_storage(uuid, user as u32, flags)
            .map_err(|e| io_error(&format!("prepareUserStorage {user}"), e))?;
        log::info!(
            "prepared storage of user {user} (flags {flags}): {} directories",
            dirs.len()
        );
        Ok(())
    }
    fn destroyUserStorage(&self, _: Option<&str>, _: i32, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn prepareSandboxForApp(&self, _: &str, _: i32, _: &str, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn destroySandboxForApp(&self, _: &str, _: &str, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn startCheckpoint(&self, _: i32) -> binder::Result<()> {
        unsupported("startCheckpoint")
    }
    fn needsCheckpoint(&self) -> binder::Result<bool> {
        Ok(false)
    }
    fn needsRollback(&self) -> binder::Result<bool> {
        Ok(false)
    }
    fn isCheckpointing(&self) -> binder::Result<bool> {
        Ok(false)
    }
    fn abortChanges(&self, _: &str, _: bool) -> binder::Result<()> {
        Ok(())
    }
    fn commitChanges(&self) -> binder::Result<()> {
        Ok(())
    }
    fn prepareCheckpoint(&self) -> binder::Result<()> {
        Ok(())
    }
    fn restoreCheckpoint(&self, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn restoreCheckpointPart(&self, _: &str, _: i32) -> binder::Result<()> {
        Ok(())
    }
    fn markBootAttempt(&self) -> binder::Result<()> {
        Ok(())
    }
    fn supportsCheckpoint(&self) -> binder::Result<bool> {
        Ok(false)
    }
    fn supportsBlockCheckpoint(&self) -> binder::Result<bool> {
        Ok(false)
    }
    fn supportsFileCheckpoint(&self) -> binder::Result<bool> {
        Ok(false)
    }
    fn resetCheckpoint(&self) -> binder::Result<()> {
        Ok(())
    }
    fn earlyBootEnded(&self) -> binder::Result<()> {
        Ok(())
    }
    fn createStubVolume(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
        _: &str,
        _: i32,
    ) -> binder::Result<String> {
        unsupported("createStubVolume")
    }
    fn destroyStubVolume(&self, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn openAppFuseFile(
        &self,
        _: i32,
        _: i32,
        _: i32,
        _: i32,
    ) -> binder::Result<ParcelFileDescriptor> {
        unsupported("openAppFuseFile")
    }
    fn incFsEnabled(&self) -> binder::Result<bool> {
        Ok(false)
    }
    fn mountIncFs(
        &self,
        _: &str,
        _: &str,
        _: i32,
        _: &str,
    ) -> binder::Result<IncrementalFileSystemControlParcel> {
        unsupported("mountIncFs")
    }
    fn unmountIncFs(&self, _: &str) -> binder::Result<()> {
        unsupported("unmountIncFs")
    }
    fn setIncFsMountOptions(
        &self,
        _: &IncrementalFileSystemControlParcel,
        _: bool,
        _: bool,
        _: &str,
    ) -> binder::Result<()> {
        unsupported("setIncFsMountOptions")
    }
    fn bindMount(&self, _: &str, _: &str) -> binder::Result<()> {
        unsupported("bindMount")
    }
    fn destroyDsuMetadataKey(&self, _: &str) -> binder::Result<()> {
        Ok(())
    }
    fn getStorageSize(&self) -> binder::Result<i64> {
        // The size of the file system that holds /data.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: statvfs into a local buffer.
        if unsafe { libc::statvfs(c"/data".as_ptr(), &mut st) } != 0 {
            return Err(io_error("statvfs /data", std::io::Error::last_os_error()));
        }
        Ok(st.f_blocks as i64 * st.f_frsize as i64)
    }
    fn getStorageRemainingLifetime(&self) -> binder::Result<i32> {
        Ok(-1)
    }
    fn getWriteBoosterBufferSize(&self) -> binder::Result<i32> {
        Ok(-1)
    }
    fn getWriteBoosterBufferAvailablePercent(&self) -> binder::Result<i32> {
        Ok(-1)
    }
    fn setWriteBoosterBufferFlush(&self, _: bool) -> binder::Result<bool> {
        Ok(false)
    }
    fn setWriteBoosterBufferOn(&self, _: bool) -> binder::Result<bool> {
        Ok(false)
    }
    fn getWriteBoosterLifeTimeEstimate(&self) -> binder::Result<i32> {
        Ok(-1)
    }
}

fn main() {
    daemon_log::init("vold");
    // The original's arguments (SELinux contexts for blkid and fsck) have
    // nothing to act on here.
    let vold = BnVold::new_binder(Vold::default(), BinderFeatures::default());
    if let Err(e) = binder::add_service(SERVICE, vold.as_binder()) {
        log::error!("cannot register {SERVICE}: {e:?}");
        std::process::exit(1);
    }
    log::info!("registered {SERVICE}");
    binder::ProcessState::start_thread_pool();
    binder::ProcessState::join_thread_pool();
}
