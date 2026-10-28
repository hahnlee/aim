//! `mount`: StorageManager for the app host, as StorageManagerService
//! keeps external storage at `android-16.0.0_r1`. It connects vold
//! (`setListener`, `reset`, user 0's addition and start at unlock), has the agent
//! mount user 0's emulated volume (`aim-app-host-agent`, which serves its
//! FUSE connection in MediaProvider's place), follows the volume's state
//! from vold's listener calls, answers apps' `getVolumeList`,
//! `getVolumes`, `mkdirs`, `fixupAppDir` and allocations, and tells the listeners apps register of
//! the volume's state changes (`registerListener`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_SERVICE_SPECIFIC, Exception, Parcel,
};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_os_storage_istoragemanager as sm;

use super::zygote::MountExternal;
use super::{AppHost, reply, system_boolean};

/// vold's service and `IVold`'s calls the app host makes, in the
/// interface's order.
const VOLD: &str = "vold";
pub(super) const VOLD_DESCRIPTOR: &str = "android.os.IVold";
const SET_LISTENER: u32 = 1;
const RESET: u32 = 4;
const ON_USER_ADDED: u32 = 6;
const ON_USER_STARTED: u32 = 8;
const SETUP_APP_DIR: u32 = 23;
const FIXUP_APP_DIR: u32 = 24;
const FSTRIM: u32 = 28;
const NEEDS_CHECKPOINT: u32 = 53;
const SUPPORTS_CHECKPOINT: u32 = 62;
const SUPPORTS_BLOCK_CHECKPOINT: u32 = 63;
/// `IVoldTaskListener`'s descriptor and calls.
const TASK_LISTENER_DESCRIPTOR: &str = "android.os.IVoldTaskListener";
const ON_STATUS: u32 = 1;
const ON_FINISHED: u32 = 2;
/// `StorageManagerService.LAST_FSTRIM_FILE`, `TAG_STORAGE_TRIM`.
const LAST_FSTRIM_FILE: &str = "system/last-fstrim";
const TAG_STORAGE_TRIM: &str = "storage_trim";
/// `IVoldListener`'s volume calls.
const LISTENER_DESCRIPTOR: &str = "android.os.IVoldListener";
const ON_DISK_CREATED: u32 = 1;
const ON_VOLUME_CREATED: u32 = 5;
const ON_VOLUME_STATE_CHANGED: u32 = 6;
const ON_VOLUME_METADATA_CHANGED: u32 = 7;
const ON_VOLUME_PATH_CHANGED: u32 = 8;
const ON_VOLUME_INTERNAL_PATH_CHANGED: u32 = 9;
const ON_VOLUME_DESTROYED: u32 = 10;
/// `IStorageEventListener`'s descriptor and its state calls.
const EVENT_LISTENER_DESCRIPTOR: &str = "android.os.storage.IStorageEventListener";
const ON_STORAGE_STATE_CHANGED: u32 = 2;
const ON_VOLUME_STATE_CHANGED_EVENT: u32 = 3;
/// `VolumeInfo.MOUNT_FLAG_PRIMARY`, `MOUNT_FLAG_VISIBLE_FOR_WRITE`.
const MOUNT_FLAG_PRIMARY: i32 = 1 << 0;
const MOUNT_FLAG_VISIBLE_FOR_WRITE: i32 = 1 << 2;
/// How long vold may take to register.
const VOLD_PATIENCE: Duration = Duration::from_secs(30);
/// The agent's `mountStorage(int userId)`.
pub(super) const MOUNT_STORAGE: u32 = 5;
/// `VolumeInfo.TYPE_PRIVATE` (`IVold.VOLUME_TYPE_PRIVATE`).
const TYPE_PRIVATE: i32 = 1;
/// `VolumeInfo.STATE_*` (`IVold.VOLUME_STATE_*`).
const STATE_UNMOUNTED: i32 = 0;
const STATE_MOUNTED: i32 = 2;
const STATE_MOUNTED_READ_ONLY: i32 = 3;
/// User 0's emulated volume, its paths and its serial number
/// (`UserInfo.serialNumber` of the system user).
const VOLUME: &str = "emulated;0";
const USER: i32 = 0;
const SERIAL: i32 = 0;
/// `Intent.FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT`,
/// `FLAG_RECEIVER_INCLUDE_BACKGROUND`; `StorageVolume.EXTRA_STORAGE_VOLUME`.
const FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT: i32 = 0x0400_0000;
const FLAG_RECEIVER_INCLUDE_BACKGROUND: i32 = 0x0100_0000;
const EXTRA_STORAGE_VOLUME: &str = "android.os.storage.extra.STORAGE_VOLUME";
/// `Manifest.permission.WRITE_MEDIA_STORAGE`.
const WRITE_MEDIA_STORAGE: &str = "android.permission.WRITE_MEDIA_STORAGE";
/// `ApplicationInfo.PRIVATE_FLAG_SIGNED_WITH_PLATFORM_KEY`.
const PRIVATE_FLAG_SIGNED_WITH_PLATFORM_KEY: i32 = 1 << 20;
/// `PackageManager.PROPERTY_NO_APP_DATA_STORAGE`.
/// `StorageManager`'s storage threshold defaults.
const DEFAULT_THRESHOLD_PERCENTAGE: i64 = 5;
const DEFAULT_THRESHOLD_MAX_BYTES: i64 = 500 << 20;
const DEFAULT_FULL_THRESHOLD_BYTES: i64 = 1 << 20;
/// `StorageManager.FLAG_ALLOCATE_*`.
const FLAG_ALLOCATE_AGGRESSIVE: i32 = 1 << 0;
const FLAG_ALLOCATE_CACHE_ONLY: i32 = 1 << 4;
const ALLOCATE_AGGRESSIVE: &str = "android.permission.ALLOCATE_AGGRESSIVE";

const NO_APP_DATA_STORAGE: &str = "android.internal.PROPERTY_NO_APP_DATA_STORAGE";

/// The emulated volume, as vold reports it (`None` until vold created
/// it), and the listeners apps registered (`mCallbacks`).
#[derive(Default)]
pub struct Volumes {
    pub(super) boot_completed: std::sync::atomic::AtomicBool,
    volume: Mutex<Option<VolumeInfo>>,
    /// Storage successfully prepared by vold and installd, before the user
    /// enters unlocking. This is independent of the user's lifecycle state.
    prepared_users: Mutex<HashMap<i32, i32>>,
    vold: Mutex<Option<Strong>>,
    listeners: Mutex<HashMap<u32, (Strong, u64)>>,
    /// Each user's cloud media provider's authority, as MediaProvider set
    /// it (`mCloudMediaProviders`).
    cloud_media_providers: Mutex<HashMap<i32, Option<String>>>,
    /// `mInternalStorageSize`, once read.
    internal_storage_size: Mutex<i64>,
    /// The listeners of this process (`StorageEventListener`s registered
    /// with `StorageManager`), by the id they were registered with.
    native_listeners: Mutex<Vec<(u64, NativeListener)>>,
    /// `mLastMaintenance`: when idle maintenance last ran, from
    /// `last-fstrim`'s time; none until it is first read.
    last_maintenance: Mutex<Option<i64>>,
}

/// A listener of this process: told each volume's type, new state and
/// file system UUID (`onVolumeStateChanged`).
pub(super) type NativeListener = Arc<dyn Fn(i32, i32, Option<&str>) + Send + Sync>;

/// The ids of this process's storage listeners.
static NATIVE_LISTENER_IDS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// `VolumeInfo` of user 0's emulated volume, as StorageManagerService
/// keeps it from vold's calls, or of internal storage.
#[derive(Clone)]
struct VolumeInfo {
    id: &'static str,
    kind: i32,
    part_guid: Option<String>,
    mount_flags: i32,
    mount_user: i32,
    state: i32,
    fs_type: Option<String>,
    fs_uuid: Option<String>,
    fs_label: Option<String>,
    path: Option<String>,
    internal_path: Option<String>,
}

impl VolumeInfo {
    /// `onVolumeCreatedLocked` of the primary emulated volume on internal
    /// storage: primary and visible for writing.
    fn created(kind: i32, part_guid: Option<String>, mount_user: i32) -> Self {
        VolumeInfo {
            id: VOLUME,
            kind,
            part_guid,
            mount_flags: MOUNT_FLAG_PRIMARY | MOUNT_FLAG_VISIBLE_FOR_WRITE,
            mount_user,
            state: STATE_UNMOUNTED,
            fs_type: None,
            fs_uuid: None,
            fs_label: None,
            path: None,
            internal_path: None,
        }
    }

    /// `addInternalVolumeLocked`: internal storage, mounted at the data
    /// directory, present from the service's start and after each reset.
    fn internal() -> Self {
        VolumeInfo {
            id: "private",
            kind: TYPE_PRIVATE,
            part_guid: None,
            mount_flags: 0,
            mount_user: -10000, // `UserHandle.USER_NULL`
            state: STATE_MOUNTED,
            fs_type: None,
            fs_uuid: None,
            fs_label: None,
            path: Some("/data".into()),
            internal_path: None,
        }
    }
}

impl WriteParcelable for VolumeInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string8(Some(self.id));
        p.write_i32(self.kind);
        p.write_i32(0); // no disk
        p.write_string8(self.part_guid.as_deref());
        p.write_i32(self.mount_flags);
        p.write_i32(self.mount_user);
        p.write_i32(self.state);
        p.write_string8(self.fs_type.as_deref());
        p.write_string8(self.fs_uuid.as_deref());
        p.write_string8(self.fs_label.as_deref());
        p.write_string8(self.path.as_deref());
        p.write_string8(self.internal_path.as_deref());
    }
}

/// User 0 started and its DE storage prepared, its CE storage still to
/// unlock (`UserController.finishUserUnlocking`).
pub(super) struct StartedUser {
    vold: Strong,
    installd: Strong,
    /// The build's fingerprint, and whether it is not the one the user
    /// last logged in with (a system update: `PRE_BOOT_COMPLETED`).
    fingerprint: String,
    pub updated: bool,
}

impl AppHost {
    /// StorageManagerService's start: vold connected and reset, user 0
    /// added and its DE storage prepared; with
    /// PackageManagerService's and UserManagerService's preparation of the
    /// user's and its apps' data (`app_data`). Its CE storage is unlocked
    /// later (`unlock_user_storage`), as `finishUserBoot` unlocks it.
    pub(super) fn start_storage(&self) -> Result<StartedUser, String> {
        let installd = self.installd()?;
        self.package_system()
            .and_then(|system| system.wait_native_package_app_data())
            .map_err(|e| e.message)?;
        let deadline = Instant::now() + VOLD_PATIENCE;
        let vold = loop {
            if let Ok(vold) = super::display::check_service(&self.process, VOLD) {
                break vold;
            }
            if Instant::now() > deadline {
                return Err("vold did not register".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let listener = self
            .process
            .add_service(Arc::new(Listener(self.me.clone())));
        let call = |code: u32, name: &str, write: &dyn Fn(&mut Parcel)| -> Result<(), String> {
            let mut data = Parcel::new();
            data.write_interface_token(VOLD_DESCRIPTOR);
            write(&mut data);
            let reply = vold
                .transact(code, &data, false)
                .map_err(|s| format!("vold {name}: {s}"))?;
            match reply.reader().read_exception() {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(format!("vold {name}: {e:?}")),
                Err(s) => Err(format!("vold {name}: {s}")),
            }
        };
        call(SET_LISTENER, "setListener", &|p| {
            p.write_binder(Some(listener))
        })?;
        call(RESET, "reset", &|_| {})?;
        call(ON_USER_ADDED, "onUserAdded", &|p| {
            p.write_i32(USER);
            p.write_i32(SERIAL);
            p.write_i32(-1); // shares storage with no user
        })?;
        // UserManagerService's `onBeforeStartUser`: the user's DE
        // storage and apps' data (the CE storage at its unlock),
        // migrated when the build's fingerprint is not the one the
        // user last logged in with.
        let fingerprint = super::app_data::fingerprint(|key| self.property(key));
        let updated = super::user_service::storage_user(self, USER)
            .map_err(|e| e.message)?
            .is_some_and(|(_, _, last)| last.as_deref() != Some(&fingerprint));
        let started = StartedUser {
            vold: self.process.strong(vold.handle),
            installd,
            fingerprint,
            updated,
        };
        self.prepare_user_storage(&started, super::app_data::FLAG_STORAGE_DE)?;
        *self.storage.vold.lock().unwrap() = Some(self.process.strong(vold.handle));
        Ok(started)
    }

    /// StorageManagerService's `onUserUnlocking`: vold creates the
    /// emulated volume on `onUserStarted`, then `H_VOLUME_MOUNT` mounts it.
    /// The user's CE storage already exists (`onBeforeUnlockUser`).
    fn mount_user_storage(&self, vold: &Strong) -> Result<(), String> {
        let mut data = Parcel::new();
        data.write_interface_token(VOLD_DESCRIPTOR);
        data.write_i32(USER);
        let reply = vold
            .transact(ON_USER_STARTED, &data, false)
            .map_err(|e| format!("vold onUserStarted: {e}"))?;
        reply
            .reader()
            .read_exception()
            .map_err(|e| format!("vold onUserStarted: {e}"))?
            .map_err(|e| format!("vold onUserStarted: {e:?}"))?;
        let daemon = self.media_provider_uid().map_err(|e| format!("{e:?}"))?;
        self.call_agent(MOUNT_STORAGE, "mountStorage", |p| {
            p.write_i32(USER);
            p.write_i32(daemon);
        })
        .map(|_| ())
        .map_err(|e| format!("{e:?}"))?;
        eprintln!("app host: {VOLUME} mounted");
        Ok(())
    }

    /// The user's and its apps' data of the storage of `flags`
    /// (`prepareUserData`, `reconcileAppsData`).
    fn prepare_user_storage(&self, user: &StartedUser, flags: i32) -> Result<(), String> {
        let system = self.package_system().map_err(|e| e.message)?;
        system
            .wait_native_package_app_data()
            .map_err(|e| e.message)?;
        self.prepare_user_data(&user.vold, &user.installd, USER, flags)?;
        *self
            .storage
            .prepared_users
            .lock()
            .unwrap()
            .entry(USER)
            .or_default() |= flags;
        system
            .internal_reconcile_apps_data(USER, flags, user.updated, 1000, 0)
            .map_err(|e| e.message)
    }

    pub(super) fn ce_storage_prepared(&self, user: i32) -> bool {
        self.storage
            .prepared_users
            .lock()
            .unwrap()
            .get(&user)
            .is_some_and(|flags| flags & super::app_data::FLAG_STORAGE_CE != 0)
    }

    /// `finishUserUnlocking` of user 0: UserManagerService's
    /// `onBeforeUnlockUser`, its CE storage and apps' data, then the user
    /// unlocking, then StorageManagerService's `onUserUnlocking`.
    pub(super) fn unlock_user_storage(&self, user: &StartedUser) -> Result<(), String> {
        self.prepare_user_storage(user, super::app_data::FLAG_STORAGE_CE)?;
        self.finish_storage_unlock(&user.vold)
    }

    fn finish_storage_unlock(&self, vold: &Strong) -> Result<(), String> {
        self.user_unlocking(USER).map_err(|e| e.message)?;
        // StorageManagerService logs onUserStarted/mount exceptions and
        // lets UserController finish unlocking and send BOOT_COMPLETED.
        if let Err(e) = self.mount_user_storage(vold) {
            eprintln!("app host: external storage: {e}");
        }
        Ok(())
    }

    /// `finishUserUnlockedCompleted`: UserManagerService's
    /// `onUserLoggedIn` of user 0, with the build's fingerprint.
    pub(super) fn user_logged_in(&self, user: &StartedUser) -> Result<(), String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        super::user_service::user_logged_in(self, USER, now, &user.fingerprint)
            .map_err(|e| e.message)
    }

    /// `getVolumes`: `mVolumes`, internal storage and the emulated
    /// volume once vold created it, in their `ArrayMap` order.
    fn volume_infos(&self) -> Vec<Option<VolumeInfo>> {
        let mut volumes = vec![VolumeInfo::internal()];
        volumes.extend(self.storage.volume.lock().unwrap().clone());
        crate::package::info::array_order(volumes, |v| v.id)
            .into_iter()
            .map(Some)
            .collect()
    }

    /// The file system UUIDs of the private volumes mounted readable
    /// (`getVolumes`, `TYPE_PRIVATE`, `isMountedReadable`, a `fsUuid`).
    pub(super) fn mounted_private_volume_uuids(&self) -> Vec<String> {
        self.volume_infos()
            .into_iter()
            .flatten()
            .filter(|v| {
                v.kind == TYPE_PRIVATE && matches!(v.state, STATE_MOUNTED | STATE_MOUNTED_READ_ONLY)
            })
            .filter_map(|v| v.fs_uuid)
            .collect()
    }

    /// The file system UUIDs of the private volumes mounted writable
    /// (`getWritablePrivateVolumes`: `TYPE_PRIVATE`, `isMountedWritable`),
    /// none for internal storage.
    pub(super) fn writable_private_volume_uuids(&self) -> Vec<Option<String>> {
        self.volume_infos()
            .into_iter()
            .flatten()
            .filter(|v| v.kind == TYPE_PRIVATE && v.state == STATE_MOUNTED)
            .map(|v| v.fs_uuid)
            .collect()
    }

    /// The data volume's total and usable bytes (`File.getTotalSpace`,
    /// `getUsableSpace` of `/data`).
    pub(super) fn data_space(&self) -> Result<(i64, i64), Exception> {
        let inputs = self.inputs()?;
        let path = std::ffi::CString::new(inputs.data.as_os_str().as_encoded_bytes())
            .map_err(|_| Exception::new(EX_ILLEGAL_STATE, "data directory"))?;
        // SAFETY: statvfs into a local struct.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
            return Err(io_exception(&format!(
                "statvfs: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok((
            st.f_blocks as i64 * st.f_frsize as i64,
            st.f_bavail as i64 * st.f_frsize as i64,
        ))
    }

    pub(super) fn global_setting(&self, name: &str) -> Option<i64> {
        self.inputs()
            .ok()?
            .settings
            .get(super::settings::Table::Global, name)
            .and_then(|v| v.parse().ok())
    }

    /// `StorageManager.getStorageLowBytes` of a volume of `total` bytes.
    pub(super) fn storage_low_bytes(&self, total: i64) -> i64 {
        let percent = self
            .global_setting("sys_storage_threshold_percentage")
            .unwrap_or(DEFAULT_THRESHOLD_PERCENTAGE);
        let max = self
            .global_setting("sys_storage_threshold_max_bytes")
            .unwrap_or(DEFAULT_THRESHOLD_MAX_BYTES);
        (total * percent / 100).min(max)
    }

    /// `StorageManager.getStorageFullBytes`.
    fn storage_full_bytes(&self) -> i64 {
        self.global_setting("sys_storage_full_threshold_bytes")
            .unwrap_or(DEFAULT_FULL_THRESHOLD_BYTES)
    }

    /// The reserve an allocation leaves: the full one when aggressive,
    /// else the low one.
    fn allocation_reserve(&self, flags: i32, total: i64) -> i64 {
        if flags & FLAG_ALLOCATE_AGGRESSIVE != 0 {
            self.storage_full_bytes()
        } else {
            self.storage_low_bytes(total)
        }
    }

    /// `StorageManager.findPathForUuid`: internal storage only, the app
    /// host's volumes having no other file system UUID.
    pub(super) fn find_path_for_uuid(uuid: Option<&str>) -> Result<(), Exception> {
        match uuid {
            None => Ok(()),
            Some(uuid) => Err(parcelable_io(
                "java.io.FileNotFoundException",
                &format!("Failed to find a storage device for {uuid}"),
            )),
        }
    }

    /// `adjustAllocateFlags`: aggressive allocation needs its permission.
    /// The reserved cache space it lets a camera user defy only counts
    /// with quotas, which the data volume does not support.
    fn adjust_allocate_flags(&self, flags: i32, uid: i32) -> Result<i32, Exception> {
        if flags & FLAG_ALLOCATE_AGGRESSIVE != 0
            && self.check_permission(Some(ALLOCATE_AGGRESSIVE), uid)? != 0
        {
            return Err(Exception::security(format!(
                "StorageManagerService: Neither user {uid} nor current process has {ALLOCATE_AGGRESSIVE}."
            )));
        }
        Ok(flags)
    }

    /// `getAllocatableBytes`: the usable space above the reserve.
    /// installd's quotas are not supported on the data volume (the syscall
    /// layer has no `quotactl`), so no cached data counts as clearable
    /// (`isQuotaSupported`).
    fn allocatable_bytes(
        &self,
        uuid: Option<&str>,
        flags: i32,
        uid: i32,
    ) -> Result<i64, Exception> {
        let flags = self.adjust_allocate_flags(flags, uid)?;
        Self::find_path_for_uuid(uuid)?;
        if flags & FLAG_ALLOCATE_CACHE_ONLY != 0 {
            return Ok(0);
        }
        let (total, usable) = self.data_space()?;
        Ok((usable - self.allocation_reserve(flags, total)).max(0))
    }

    /// `allocateBytes`: `bytes` and the reserve made free
    /// (`PackageManagerService.freeStorage`), which needs no clearing when
    /// the volume has the space; clearing caches is not implemented
    /// (#1819).
    pub(super) fn allocate_bytes(
        &self,
        uuid: Option<&str>,
        bytes: i64,
        flags: i32,
        uid: i32,
    ) -> Result<(), Exception> {
        let allocatable = self.allocatable_bytes(uuid, flags, uid)?;
        if bytes > allocatable {
            return Err(io_exception(&format!(
                "Failed to allocate {bytes} because only {allocatable} allocatable"
            )));
        }
        let (total, usable) = self.data_space()?;
        if usable >= bytes + self.allocation_reserve(flags, total) {
            return Ok(());
        }
        Err(self.unimplemented_named("mount", "freeStorage's cache clearing (#1819)"))
    }

    /// `getInternalStorageBlockDeviceSize`: vold's `GetStorageSize`, read
    /// once. vold follows `/data`'s block device through device-mapper to
    /// its parent and reads the disk's size from `/sys/block`; here
    /// `/data` is an APFS volume of the data image, and the disk it is on
    /// is the outermost whole IOMedia under it: through the APFS container
    /// to its physical store, the attached image. A disk it cannot find is
    /// vold's EINVAL.
    pub(super) fn internal_storage_block_device_size(&self) -> Result<i64, Exception> {
        let mut size = self.storage.internal_storage_size.lock().unwrap();
        if *size == 0 {
            let data = &self.inputs()?.data;
            *size = aim_storage::disk::block_device_size(data).ok_or_else(|| Exception {
                code: EX_SERVICE_SPECIFIC,
                message: String::new(),
                service_specific: libc::EINVAL,
                parcelable: None,
            })? as i64;
        }
        Ok(*size)
    }

    /// `storage.xml`, as `readSettingsLocked` reads it; none when missing.
    fn storage_settings(&self) -> Result<Option<aim_android_xml::Element>, Exception> {
        let path = self.data()?.join("system/storage.xml");
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_exception(&format!("/data/system/storage.xml: {e}"))),
        };
        aim_android_xml::read(&bytes)
            .map(Some)
            .map_err(|e| io_exception(&format!("/data/system/storage.xml: {e}")))
    }

    /// `getVolumeRecords`: `mRecords`, read from `storage.xml`
    /// (`readSettingsLocked`); the records are of volumes on vold's disks,
    /// which the app host does not keep (#1839).
    fn volume_records(&self) -> Result<(), Exception> {
        if self
            .storage_settings()?
            .is_some_and(|root| root.children().any(|e| e.name == "volume"))
        {
            return Err(
                self.unimplemented_named("mount", "the volume records of vold's disks (#1839)")
            );
        }
        Ok(())
    }

    /// `getPrimaryStorageUuid`: `mPrimaryStorageUuid` as
    /// `readSettingsLocked` reads it from `storage.xml`'s `volumes`, when
    /// its version has a valid one, else the default
    /// (`getDefaultPrimaryStorageUuid`: primary physical storage with
    /// `ro.vold.primary_physical`, else internal storage's none); a file
    /// that cannot be read leaves the default.
    fn primary_storage_uuid(&self) -> Option<String> {
        const VERSION_INIT: i32 = 1;
        const VERSION_ADD_PRIMARY: i32 = 2;
        const VERSION_FIX_PRIMARY: i32 = 3;
        let primary_physical =
            system_boolean(self.property("ro.vold.primary_physical").as_deref(), false);
        let default = primary_physical.then(|| "primary_physical".to_owned());
        let root = match self.storage_settings() {
            Ok(Some(root)) => root,
            Ok(None) => return default,
            Err(e) => {
                eprintln!("app host: mount: Failed reading metadata: {}", e.message);
                return default;
            }
        };
        let mut pending = vec![&root];
        while let Some(e) = pending.pop() {
            if e.name == "volumes" {
                let version = e.int("version").ok().flatten().unwrap_or(VERSION_INIT);
                let valid = version >= VERSION_FIX_PRIMARY
                    || (version >= VERSION_ADD_PRIMARY && !primary_physical);
                if valid {
                    return e.string("primaryStorageUuid").map(|s| s.into_owned());
                }
            }
            pending.extend(e.children());
        }
        default
    }

    /// Whether user 0's emulated volume is mounted
    /// (`StorageManagerInternal.isFuseMounted`).
    pub(super) fn storage_mounted(&self) -> bool {
        self.storage
            .volume
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|v| v.state == STATE_MOUNTED)
    }

    /// `StorageManagerInternal.getExternalStorageMountMode`
    /// (`getMountModeInternal`) of `package`'s process of `uid`.
    pub(super) fn mount_mode(
        &self,
        capture: &crate::package::scan_snapshot::query_state::Capture,
        uid: i32,
        package: &str,
    ) -> Result<MountExternal, Exception> {
        /// `AppOpsManager.OP_REQUEST_INSTALL_PACKAGES`.
        const OP_REQUEST_INSTALL_PACKAGES: i32 = 66;
        let state = capture.state();
        let resolution = capture
            .resolution()
            .map_err(|e| io_exception(&format!("mount mode resolution: {e:?}")))?;
        let user = uid / 100_000;
        let app_id = uid % 100_000;
        let packages: Vec<&str> = state
            .packages
            .values()
            .filter(|ps| ps.app_id == app_id)
            .map(|ps| ps.name.as_str())
            .collect();
        if packages.is_empty() {
            return Ok(MountExternal::None);
        }
        if state
            .packages
            .get(package)
            .and_then(|ps| ps.users.get(&user))
            .is_some_and(|u| u.instant_app)
        {
            return Ok(MountExternal::None);
        }
        let authority_app_id = |authority: &str| {
            resolution
                .components
                .provider_by_authority(authority)
                .and_then(|p| state.packages.get(&p.package))
                .map(|ps| ps.app_id)
        };
        // `isExternalStorageService`: the media store's.
        if authority_app_id("media") == Some(app_id) {
            return Ok(MountExternal::PassThrough);
        }
        if authority_app_id("downloads") == Some(app_id)
            || authority_app_id("com.android.externalstorage.documents") == Some(app_id)
        {
            return Ok(MountExternal::AndroidWritable);
        }
        if self.check_permission(Some("android.permission.ACCESS_MTP"), uid)? == 0
            && self
                .application_info(package, 0, Some(&state))?
                .is_some_and(|ai| ai.private_flags & PRIVATE_FLAG_SIGNED_WITH_PLATFORM_KEY != 0)
        {
            return Ok(MountExternal::AndroidWritable);
        }
        let install = self.check_permission(Some("android.permission.INSTALL_PACKAGES"), uid)? == 0;
        let mut install_op = false;
        for name in packages {
            if super::appops_service::mode(self, OP_REQUEST_INSTALL_PACKAGES, uid, Some(name))?
                == super::appops_service::MODE_ALLOWED
            {
                install_op = true;
                break;
            }
        }
        Ok(if install || install_op {
            MountExternal::Installer
        } else {
            MountExternal::Default
        })
    }

    /// `StorageManagerInternal.hasExternalStorageAccess`: the
    /// MANAGE_EXTERNAL_STORAGE op allowed, or at its default the
    /// permission held.
    pub(super) fn has_external_storage_access(
        &self,
        uid: i32,
        package: &str,
    ) -> Result<bool, Exception> {
        /// `AppOpsManager.OP_MANAGE_EXTERNAL_STORAGE`.
        const OP_MANAGE_EXTERNAL_STORAGE: i32 = 92;
        Ok(
            match super::appops_service::mode(self, OP_MANAGE_EXTERNAL_STORAGE, uid, Some(package))?
            {
                super::appops_service::MODE_DEFAULT => {
                    self.check_permission(Some("android.permission.MANAGE_EXTERNAL_STORAGE"), uid)?
                        == 0
                }
                mode => mode == super::appops_service::MODE_ALLOWED,
            },
        )
    }

    /// The uid the FUSE daemon runs as: user 0's of the app holding the
    /// `media` authority (`MediaStore.AUTHORITY`), MediaProvider.
    fn media_provider_uid(&self) -> Result<i32, Exception> {
        let (state, resolution) = self.packages()?;
        let package = resolution
            .components
            .provider_by_authority("media")
            .map(|p| p.package.clone())
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no media provider"))?;
        let ps = state
            .packages
            .get(&package)
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, format!("no {package}")))?;
        Ok(USER * 100_000 + ps.app_id as i32)
    }

    /// `getVolumeList`: user 0's primary emulated volume, or the stub
    /// StorageManagerService makes before vold created it.
    fn volume_list(
        &self,
        call: &Call<'_>,
        user: i32,
        package: Option<&str>,
    ) -> Result<Parcel, Exception> {
        let uid = call.sender_euid as i32;
        self.check_package(uid, package)?;
        if uid / 100_000 != user {
            return Err(Exception::security(
                "Need INTERACT_ACROSS_USERS to get volumes for another user",
            ));
        }
        let state = self
            .storage
            .volume
            .lock()
            .unwrap()
            .as_ref()
            .map(|v| v.state);
        let unknown = self
            .framework()?
            .framework_string("unknownName")
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no unknownName"))?;
        let volume = match state.filter(|_| user == USER) {
            // Mounted visible for writing by user 0, so listed for writing
            // or reading alike (`isVisibleForWrite`, `isVisibleForUser`).
            Some(state) => self.storage_volume(user, state)?,
            None => {
                eprintln!("app host: No primary storage defined yet; hacking together a stub");
                StorageVolume {
                    id: "stub_primary".into(),
                    path: "/storage/sdcard0".into(),
                    internal_path: "/storage/sdcard0".into(),
                    description: unknown,
                    emulated: true,
                    user,
                    uuid: None,
                    state: "removed".into(),
                }
            }
        };
        let mut p = Parcel::new();
        sm::write_get_volume_list_reply(&mut p, Some(&[Some(volume)]));
        Ok(p)
    }

    /// `mkdirs`: an app's own directory of `Android/data`, `media`, `obb`
    /// or `sandbox`, made by vold (`setupAppDir`).
    fn mkdirs(
        &self,
        call: &Call<'_>,
        package: Option<&str>,
        path: Option<&str>,
    ) -> Result<Parcel, Exception> {
        let uid = call.sender_euid as i32;
        let user = uid / 100_000;
        let package = package.ok_or_else(|| Exception::illegal_argument("no package"))?;
        let path = path.ok_or_else(|| Exception::illegal_argument("no path"))?;
        self.check_package(uid, Some(package))?;
        let (state, _) = self.packages()?;
        let no_storage = state
            .packages
            .get(package)
            .and_then(|ps| ps.pkg.as_ref())
            .and_then(|pkg| pkg.properties.as_ref())
            .is_some_and(|props| {
                props.iter().any(|(name, p)| {
                    name == NO_APP_DATA_STORAGE
                        && p.value == crate::package::pkg::PropertyValue::Bool(true)
                })
            });
        if no_storage {
            return Err(Exception::security(format!(
                "{package} should not have {path}"
            )));
        }
        let mut app_path = normalize(path);
        if !app_path.ends_with('/') {
            app_path.push('/');
        }
        let Some((user_part, owner)) = known_app_dir(&app_path) else {
            return Err(Exception::security(format!(
                "Invalid mkdirs path: {path} is not a known app path."
            )));
        };
        if owner != package {
            return Err(Exception::security(format!(
                "Invalid mkdirs path: {path} does not contain calling package {package}"
            )));
        }
        if user_part.as_ref().is_some_and(|u| *u != user.to_string())
            || (user_part.is_none() && user != USER)
        {
            return Err(Exception::security(format!(
                "Invalid mkdirs path: {path} does not match calling user id {user}"
            )));
        }
        let vold = self.storage.vold.lock().unwrap();
        let vold = vold
            .as_ref()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, format!("Failed to prepare {path}")))?;
        let mut data = Parcel::new();
        data.write_interface_token(VOLD_DESCRIPTOR);
        data.write_string16(Some(&app_path));
        data.write_i32(uid);
        let reply = vold.transact(SETUP_APP_DIR, &data, false).map_err(|s| {
            Exception::new(EX_ILLEGAL_STATE, format!("Failed to prepare {path}: {s}"))
        })?;
        if let Ok(Err(e)) = reply.reader().read_exception() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                format!("Failed to prepare {path}: {e:?}"),
            ));
        }
        let mut p = Parcel::new();
        sm::write_mkdirs_reply(&mut p);
        Ok(p)
    }

    /// `fixupAppDir`: an app's directory of a known app path, with its
    /// user, fixed up by vold for the package's uid; what fails is
    /// logged.
    fn fixup_app_dir(&self, path: &str) -> Result<(), Exception> {
        let Some((user, package)) = known_app_dir(path) else {
            eprintln!("app host: mount: Path {path} is not a valid application-specific directory");
            return Ok(());
        };
        let Some(user) = user else {
            eprintln!("app host: mount: Asked to fixup an app dir without a userId: {path}");
            return Ok(());
        };
        let Ok(user) = user.parse::<i32>() else {
            eprintln!("app host: mount: Invalid userId in path: {path}");
            return Ok(());
        };
        let (state, resolution) = self.packages()?;
        let query = crate::package::query::Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: crate::package::apps_filter::SYSTEM_UID,
        };
        let uid = query
            .package_uid(&package, 0, user)
            .map_err(|e| self.unimplemented_named("mount", e.0))??;
        if uid < 0 {
            eprintln!("app host: mount: Couldn't find package to fixup app dir {path}");
            return Ok(());
        }
        let vold = self.storage.vold.lock().unwrap();
        let vold = vold
            .as_ref()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "vold is not connected"))?;
        let mut data = Parcel::new();
        data.write_interface_token(VOLD_DESCRIPTOR);
        data.write_string16(Some(&format!("{path}/")));
        data.write_i32(uid);
        let failed = match vold.transact(FIXUP_APP_DIR, &data, false) {
            Err(s) => Some(s.to_string()),
            Ok(reply) => match reply.reader().read_exception() {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.message),
                Err(s) => Some(s.to_string()),
            },
        };
        if let Some(e) = failed {
            eprintln!("app host: mount: Failed to fixup app dir for {package}: {e}");
        }
        Ok(())
    }

    /// `isUidOwnerOfPackageOrSystem`, `AppOpsManager.checkPackage`.
    fn check_package(&self, uid: i32, package: Option<&str>) -> Result<(), Exception> {
        if uid % 100_000 == 1000 {
            return Ok(());
        }
        let (state, _) = self.packages()?;
        let owns = package
            .and_then(|p| state.packages.get(p))
            .is_some_and(|ps| ps.app_id as i32 == uid % 100_000);
        if owns {
            Ok(())
        } else {
            Err(Exception::security("callingPackage does not match UID"))
        }
    }
}

impl AppHost {
    /// `registerListener` (`RemoteCallbackList.register`): once per
    /// binder, until it dies.
    fn register_storage_listener(&self, handle: u32) {
        let mut listeners = self.storage.listeners.lock().unwrap();
        if listeners.contains_key(&handle) {
            return;
        }
        let strong = self.process.strong(handle);
        let me = self.me.clone();
        let cookie = self.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(host) = me.upgrade() {
                    host.storage.listeners.lock().unwrap().remove(&handle);
                }
            }),
        );
        listeners.insert(handle, (strong, cookie));
    }

    /// `unregisterListener`.
    fn unregister_storage_listener(&self, handle: u32) {
        if let Some((strong, cookie)) = self.storage.listeners.lock().unwrap().remove(&handle) {
            self.process.clear_death(&strong, cookie);
        }
    }

    /// `VolumeInfo.buildStorageVolume` of the emulated volume for `user`
    /// in `state`.
    fn storage_volume(&self, user: i32, state: i32) -> Result<StorageVolume, Exception> {
        let description = self
            .framework()?
            .framework_string("storage_internal")
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no storage_internal"))?;
        Ok(StorageVolume {
            id: VOLUME.into(),
            path: format!("/storage/emulated/{user}"),
            internal_path: format!("/data/media/{user}"),
            description,
            emulated: true,
            user,
            // `StorageManager.UUID_DEFAULT`, written as its string, null
            // for internal storage.
            uuid: Some(None),
            state: environment(state).into(),
        })
    }

    /// `H_VOLUME_BROADCAST` of the emulated volume for `user`: the
    /// broadcast of its environment's state, if it has one
    /// (`VolumeInfo.getBroadcastForEnvironment`), with the volume.
    fn volume_broadcast(&self, user: i32, state: i32) -> Result<(), Exception> {
        let Some(action) = broadcast_for_environment(environment(state)) else {
            return Ok(());
        };
        let volume = self.storage_volume(user, state)?;
        eprintln!(
            "app host: Volume {VOLUME} broadcasting {} to UserHandle{{{user}}}",
            volume.state
        );
        let mut data = Parcel::new();
        volume.write_to(&mut data);
        let path = volume.path.clone();
        let intent = super::kept::RawIntent::with_data(
            Some(action),
            Some(&crate::package::uri::Uri::preferred_hierarchical(
                "file",
                Some(""),
                Some(&path),
            )),
            None,
            FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT | FLAG_RECEIVER_INCLUDE_BACKGROUND,
            &[(
                EXTRA_STORAGE_VOLUME,
                crate::bundle::Value::Parcelable {
                    class: "android.os.storage.StorageVolume".into(),
                    data: data.data().to_vec(),
                },
            )],
        );
        self.broadcasts.send_from_system_as_user(self, intent, user);
        Ok(())
    }

    /// `setCloudMediaProvider`, which only the external storage service
    /// (MediaProvider) may call. No system service of the app host listens
    /// for the change (`CloudProviderChangeListener`).
    fn set_cloud_media_provider(
        &self,
        call: &Call<'_>,
        authority: Option<String>,
    ) -> Result<(), Exception> {
        let uid = call.sender_euid as i32;
        if self.check_permission(Some(WRITE_MEDIA_STORAGE), uid)? != 0 {
            return Err(Exception::security(format!(
                "{WRITE_MEDIA_STORAGE}: Neither user {uid} nor current process has {WRITE_MEDIA_STORAGE}."
            )));
        }
        if uid % 100_000 != self.media_provider_uid()? % 100_000 {
            return Err(Exception::security(
                "Only the ExternalStorageService is permitted",
            ));
        }
        self.storage
            .cloud_media_providers
            .lock()
            .unwrap()
            .insert(uid / 100_000, authority);
        Ok(())
    }

    /// `getCloudMediaProvider`: the caller's user's, if the caller may see
    /// its provider.
    fn cloud_media_provider(&self, call: &Call<'_>) -> Result<Option<String>, Exception> {
        let uid = call.sender_euid as i32;
        let user = uid / 100_000;
        let Some(authority) = self
            .storage
            .cloud_media_providers
            .lock()
            .unwrap()
            .get(&user)
            .cloned()
            .flatten()
        else {
            return Ok(None);
        };
        let Some(package) = self.provider_package(&authority, user)? else {
            return Ok(None);
        };
        let (state, resolution) = self.packages()?;
        // `PackageManagerInternal.filterAppAccess`.
        let ps = state.packages.get(&package);
        let filtered = ps.is_none()
            || crate::package::apps_filter::should_filter_application(
                &state,
                &resolution.apps_filter,
                ps,
                uid,
                user,
                true,
                true,
            )
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("filterAppAccess: {}", e.0)))?;
        Ok((!filtered).then_some(authority))
    }

    /// `StorageManager.registerListener` of a listener of this process;
    /// the id to unregister it with.
    pub(super) fn register_native_storage_listener(&self, listener: NativeListener) -> u64 {
        let id = NATIVE_LISTENER_IDS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.storage
            .native_listeners
            .lock()
            .unwrap()
            .push((id, listener));
        id
    }

    /// `StorageManager.unregisterListener`.
    pub(super) fn unregister_native_storage_listener(&self, id: u64) {
        self.storage
            .native_listeners
            .lock()
            .unwrap()
            .retain(|(i, _)| *i != id);
    }

    /// `lastMaintenance`: `mLastMaintenance`, read at the service's start
    /// from `last-fstrim`'s time, which is made empty when absent (so the
    /// first maintenance after it runs at once).
    pub(super) fn last_maintenance(&self) -> Result<i64, Exception> {
        let mut last = self.storage.last_maintenance.lock().unwrap();
        if let Some(last) = *last {
            return Ok(last);
        }
        let file = self.data()?.join(LAST_FSTRIM_FILE);
        let read = match std::fs::metadata(&file).and_then(|m| m.modified()) {
            Ok(time) => time
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64),
            Err(_) => {
                if let Err(e) = std::fs::File::create(&file) {
                    eprintln!(
                        "app host: mount: Unable to create fstrim record {}: {e}",
                        file.display()
                    );
                }
                0
            }
        };
        *last = Some(read);
        Ok(read)
    }

    /// `runMaintenance` → `H_FSTRIM`: the time kept and `last-fstrim`
    /// touched, then `fstrim(0, null)`, on the service's thread.
    pub(super) fn run_maintenance(&self) -> Result<(), Exception> {
        let me = self
            .me
            .upgrade()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host gone"))?;
        let now = std::time::SystemTime::now();
        std::thread::spawn(move || {
            eprintln!("app host: mount: Running fstrim idle maintenance");
            let millis = now
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64);
            *me.storage.last_maintenance.lock().unwrap() = Some(millis);
            let touched = me.data().map_err(|e| e.message).and_then(|data| {
                std::fs::File::options()
                    .write(true)
                    .open(data.join(LAST_FSTRIM_FILE))
                    .and_then(|f| f.set_modified(now))
                    .map_err(|e| e.to_string())
            });
            if touched.is_err() {
                eprintln!("app host: mount: Unable to record last fstrim!");
            }
            if let Err(e) = me.fstrim() {
                eprintln!("app host: mount: fstrim: {}", e.message);
            }
        });
        Ok(())
    }

    /// `StorageManagerService.supportsCheckpoint`: vold's answer.
    pub(super) fn supports_checkpoint(&self) -> Result<bool, Exception> {
        let vold = self.storage.vold.lock().unwrap();
        let vold = vold
            .as_ref()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "vold not connected"))?;
        let fail = |s| Exception::new(EX_ILLEGAL_STATE, format!("vold supportsCheckpoint: {s}"));
        let mut data = Parcel::new();
        data.write_interface_token(VOLD_DESCRIPTOR);
        let reply = vold
            .transact(SUPPORTS_CHECKPOINT, &data, false)
            .map_err(fail)?;
        let mut r = reply.reader();
        r.read_exception().map_err(fail)??;
        r.read_bool().map_err(fail)
    }

    /// `fstrim(0, null)`: vold's trim unless a block checkpoint is in
    /// progress, each trimmed path recorded in the dropbox.
    fn fstrim(&self) -> Result<(), Exception> {
        let vold = self.storage.vold.lock().unwrap();
        let vold = vold
            .as_ref()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "vold not connected"))?;
        let call = |code: u32, name: &str, write: &dyn Fn(&mut Parcel)| {
            let mut data = Parcel::new();
            data.write_interface_token(VOLD_DESCRIPTOR);
            write(&mut data);
            vold.transact(code, &data, false)
                .map_err(|s| Exception::new(EX_ILLEGAL_STATE, format!("vold {name}: {s}")))
        };
        let boolean = |code: u32, name: &str| -> Result<bool, Exception> {
            let reply = call(code, name, &|_| {})?;
            let mut r = reply.reader();
            r.read_exception()
                .map_err(|s| Exception::new(EX_ILLEGAL_STATE, format!("vold {name}: {s}")))??;
            r.read_bool()
                .map_err(|s| Exception::new(EX_ILLEGAL_STATE, format!("vold {name}: {s}")))
        };
        if boolean(NEEDS_CHECKPOINT, "needsCheckpoint")?
            && boolean(SUPPORTS_BLOCK_CHECKPOINT, "supportsBlockCheckpoint")?
        {
            eprintln!("app host: mount: Skipping fstrim - block based checkpoint in progress");
            return Ok(());
        }
        let listener = self
            .process
            .add_service(Arc::new(TrimListener(self.me.clone())));
        let reply = call(FSTRIM, "fstrim", &|p| {
            p.write_i32(0);
            p.write_binder(Some(listener));
        })?;
        reply
            .reader()
            .read_exception()
            .map_err(|s| Exception::new(EX_ILLEGAL_STATE, format!("vold fstrim: {s}")))?
    }

    /// `scrubPath`: internal storage's paths, else an adopted volume's age.
    fn scrub_path(path: &str) -> &'static str {
        // `Environment.getDataDirectory`.
        if path.starts_with("/data") {
            return "internal";
        }
        // `findRecordForPath`: the app host keeps no volume records
        // (#1839), so no adopted volume is known.
        "unknown"
    }

    /// `onVolumeStateChangedAsync`'s calls of the listeners: the volume's
    /// new state, then, when its environment changed, the storage state of
    /// each user it is visible to (user 0's, unlocked).
    fn volume_state_changed(&self, info: &VolumeInfo, old: i32) {
        let natives: Vec<NativeListener> = self
            .storage
            .native_listeners
            .lock()
            .unwrap()
            .iter()
            .map(|(_, l)| l.clone())
            .collect();
        for listener in natives {
            listener(info.kind, info.state, info.fs_uuid.as_deref());
        }
        let listeners: Vec<u32> = self
            .storage
            .listeners
            .lock()
            .unwrap()
            .keys()
            .copied()
            .collect();
        // `RemoteCallbackList.broadcast`: a listener that failed is left
        // to its death notice.
        let send = |handle: u32, code: u32, data: &Parcel| {
            if let Err(e) = self.process.strong(handle).transact(code, data, true) {
                eprintln!("app host: a storage listener: {e:?}");
            }
        };
        for &listener in &listeners {
            let mut data = Parcel::new();
            data.write_interface_token(EVENT_LISTENER_DESCRIPTOR);
            data.write_i32(1);
            info.write_to(&mut data);
            data.write_i32(old);
            data.write_i32(info.state);
            send(listener, ON_VOLUME_STATE_CHANGED_EVENT, &data);
        }
        if let Some(intent) = volume_state_intent(
            info,
            self.storage
                .boot_completed
                .load(std::sync::atomic::Ordering::Acquire),
        ) {
            self.broadcasts.send_from_system_with(
                self,
                intent,
                -1,
                &[WRITE_MEDIA_STORAGE.to_string()],
                None,
                None,
            );
        }
        let (old_env, new_env) = (environment(old), environment(info.state));
        let visible =
            info.mount_user == USER && info.mount_flags & MOUNT_FLAG_VISIBLE_FOR_WRITE != 0;
        if old_env == new_env || !visible {
            return;
        }
        // `buildStorageVolume(user).getPath()`: the emulated volume's path
        // for the user.
        let Some(path) = info.path.as_deref().map(|p| format!("{p}/{USER}")) else {
            return;
        };
        // User 0 is unlocked while the app host runs (`mSystemUnlockedUsers`).
        if let Err(e) = self.volume_broadcast(USER, info.state) {
            eprintln!("app host: a volume broadcast: {e:?}");
        }
        for &listener in &listeners {
            let mut data = Parcel::new();
            data.write_interface_token(EVENT_LISTENER_DESCRIPTOR);
            data.write_string16(Some(&path));
            data.write_string16(Some(old_env));
            data.write_string16(Some(new_env));
            send(listener, ON_STORAGE_STATE_CHANGED, &data);
        }
    }
}

/// `StorageManagerService.isBroadcastWorthy` and its internal broadcast.
fn volume_state_intent(info: &VolumeInfo, boot_completed: bool) -> Option<super::kept::RawIntent> {
    use crate::bundle::Value;
    if !boot_completed
        || !matches!(info.kind, 0 | 1 | 2 | 5)
        || !matches!(info.state, 0 | 2 | 3 | 5 | 6 | 8)
    {
        return None;
    }
    Some(super::kept::RawIntent::new(
        "android.os.storage.action.VOLUME_STATE_CHANGED",
        FLAG_RECEIVER_REGISTERED_ONLY_BEFORE_BOOT | FLAG_RECEIVER_INCLUDE_BACKGROUND,
        &[
            (
                "android.os.storage.extra.VOLUME_ID",
                Value::String(Some(info.id.into())),
            ),
            (
                "android.os.storage.extra.VOLUME_STATE",
                Value::Int(info.state),
            ),
            (
                "android.os.storage.extra.FS_UUID",
                Value::String(info.fs_uuid.clone()),
            ),
        ],
    ))
}

/// `VolumeInfo.getBroadcastForEnvironment`.
fn broadcast_for_environment(environment: &str) -> Option<&'static str> {
    Some(match environment {
        "unmounted" => "android.intent.action.MEDIA_UNMOUNTED",
        "checking" => "android.intent.action.MEDIA_CHECKING",
        "mounted" | "mounted_ro" => "android.intent.action.MEDIA_MOUNTED",
        "ejecting" => "android.intent.action.MEDIA_EJECT",
        "unmountable" => "android.intent.action.MEDIA_UNMOUNTABLE",
        "removed" => "android.intent.action.MEDIA_REMOVED",
        "bad_removal" => "android.intent.action.MEDIA_BAD_REMOVAL",
        _ => return None,
    })
}

/// `VolumeInfo.getEnvironmentForState`.
fn environment(state: i32) -> &'static str {
    match state {
        0 | 4 => "unmounted",
        1 => "checking",
        2 => "mounted",
        3 => "mounted_ro",
        5 => "ejecting",
        6 => "unmountable",
        7 => "removed",
        8 => "bad_removal",
        _ => "unknown",
    }
}

/// `File.getCanonicalFile` of a path without links: `.` and `..` resolved.
fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    format!("/{}", parts.join("/"))
}

/// `KNOWN_APP_DIR_PATHS`: `/storage/<volume>/[<user>/]Android/(data|media|obb|sandbox)/<package>/...`
/// (case-insensitively): the user, if any, and the package.
fn known_app_dir(path: &str) -> Option<(Option<String>, String)> {
    let rest = path.strip_prefix("/storage/")?;
    let (_, rest) = rest.split_once('/')?;
    let (user, rest) = match rest.split_once('/') {
        Some((user, rest))
            if !user.is_empty()
                && user.bytes().all(|b| b.is_ascii_digit())
                && rest.to_ascii_lowercase().starts_with("android/") =>
        {
            (Some(user.to_string()), rest)
        }
        _ => (None, rest),
    };
    let (android, rest) = rest.split_once('/')?;
    let (kind, rest) = rest.split_once('/')?;
    if !android.eq_ignore_ascii_case("android")
        || !["data", "media", "obb", "sandbox"]
            .iter()
            .any(|k| kind.eq_ignore_ascii_case(k))
    {
        return None;
    }
    let package = rest.split('/').next().filter(|p| !p.is_empty())?;
    Some((user, package.to_string()))
}

/// `StorageVolume`, as `VolumeInfo.buildStorageVolume` builds it.
struct StorageVolume {
    id: String,
    path: String,
    internal_path: String,
    description: String,
    emulated: bool,
    user: i32,
    /// Present (its string, null for the default), or none.
    uuid: Option<Option<String>>,
    state: String,
}

impl WriteParcelable for StorageVolume {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string8(Some(&self.id));
        p.write_string8(Some(&self.path));
        p.write_string8(Some(&self.internal_path));
        p.write_string8(Some(&self.description));
        p.write_i32(1); // primary
        p.write_i32(0); // not removable
        p.write_i32(i32::from(self.emulated));
        p.write_i32(0); // not externally managed
        p.write_i32(0); // no mass storage
        p.write_i64(0); // no maximum file size
        // `writeParcelable(UserHandle)`: its creator's class, its id.
        p.write_string16(Some("android.os.UserHandle"));
        p.write_i32(self.user);
        match &self.uuid {
            Some(uuid) => {
                p.write_i32(1);
                p.write_string8(uuid.as_deref());
            }
            None => p.write_i32(0),
        }
        p.write_string8(None); // no file system UUID
        p.write_string8(Some(&self.state));
    }
}

/// A `ParcelableException` of `class` (`ParcelableException.writeToParcel`).
pub(super) fn parcelable_io(class: &str, message: &str) -> Exception {
    let mut payload = Parcel::new();
    payload.write_string16(Some("android.os.ParcelableException"));
    payload.write_string16(Some(class));
    payload.write_string16(Some(message));
    Exception::parcelable(Some(&format!("{class}: {message}")), &payload)
        .expect("an exception of strings")
}

pub(super) fn io_exception(message: &str) -> Exception {
    parcelable_io("java.io.IOException", message)
}

/// vold's listener: the emulated volume's creation and states.
struct Listener(std::sync::Weak<AppHost>);

impl Service for Listener {
    fn descriptor(&self) -> &str {
        LISTENER_DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(host) = self.0.upgrade() else {
            return Ok(Parcel::new());
        };
        call.data
            .enforce_interface(LISTENER_DESCRIPTOR)
            .map_err(|_| BAD_VALUE)?;
        let id = call.data.read_string16().map_err(|_| BAD_VALUE)?;
        // `mDisks`: the app host keeps none, vold reporting none for its
        // guest.
        if call.code == ON_DISK_CREATED {
            eprintln!(
                "app host: mount: vold's disk {}: portable and adoptable storage (#1839) is not implemented",
                id.as_deref().unwrap_or_default()
            );
        }
        if id.as_deref() != Some(VOLUME) {
            return Ok(Parcel::new());
        }
        let r = &mut call.data;
        let changed = {
            let mut volume = host.storage.volume.lock().unwrap();
            match (call.code, volume.as_mut()) {
                (ON_VOLUME_CREATED, _) => {
                    let kind = r.read_i32().map_err(|_| BAD_VALUE)?;
                    r.read_string16().map_err(|_| BAD_VALUE)?; // its disk: none
                    let part_guid = r.read_string16().map_err(|_| BAD_VALUE)?;
                    let user = r.read_i32().map_err(|_| BAD_VALUE)?;
                    *volume = Some(VolumeInfo::created(kind, part_guid, user));
                    None
                }
                (ON_VOLUME_STATE_CHANGED, Some(v)) => {
                    let state = r.read_i32().map_err(|_| BAD_VALUE)?;
                    let user = r.read_i32().map_err(|_| BAD_VALUE)?;
                    let old = std::mem::replace(&mut v.state, state);
                    let mut info = v.clone();
                    info.mount_user = user;
                    Some((info, old))
                }
                (ON_VOLUME_METADATA_CHANGED, Some(v)) => {
                    v.fs_type = r.read_string16().map_err(|_| BAD_VALUE)?;
                    v.fs_uuid = r.read_string16().map_err(|_| BAD_VALUE)?;
                    v.fs_label = r.read_string16().map_err(|_| BAD_VALUE)?;
                    None
                }
                (ON_VOLUME_PATH_CHANGED, Some(v)) => {
                    v.path = r.read_string16().map_err(|_| BAD_VALUE)?;
                    None
                }
                (ON_VOLUME_INTERNAL_PATH_CHANGED, Some(v)) => {
                    v.internal_path = r.read_string16().map_err(|_| BAD_VALUE)?;
                    None
                }
                (ON_VOLUME_DESTROYED, _) => {
                    *volume = None;
                    None
                }
                _ => None,
            }
        };
        if let Some((info, old)) = changed {
            host.volume_state_changed(&info, old);
        }
        Ok(Parcel::new())
    }
}

/// A `DiskInfo` or `VolumeRecord`, of which the app host has none.
enum NoDisk {}

impl WriteParcelable for NoDisk {
    fn write_to(&self, _: &mut Parcel) {
        match *self {}
    }
}

/// `mount`: the `IStorageManager` calls of apps.
pub struct StorageManager(pub Arc<AppHost>);

impl Service for StorageManager {
    fn descriptor(&self) -> &str {
        sm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            sm::GET_VOLUME_LIST => {
                let args = sm::GetVolumeList::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .volume_list(call, args.user_id, args.calling_package.as_deref()),
                )
            }
            // `mCeUnlockedUsers`: storage availability precedes the user's
            // lifecycle transition to unlocking.
            sm::IS_CE_STORAGE_UNLOCKED => {
                let args = sm::IsCeStorageUnlocked::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let unlocked = self.0.ce_storage_prepared(args.user_id);
                let mut p = Parcel::new();
                sm::write_is_ce_storage_unlocked_reply(&mut p, unlocked);
                Ok(p)
            }
            // `mDisks`, of which there are none (#1839).
            sm::GET_DISKS => {
                sm::GetDisks::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                sm::write_get_disks_reply::<NoDisk>(&mut p, Some(&[]));
                Ok(p)
            }
            sm::GET_VOLUME_RECORDS => {
                sm::GetVolumeRecords::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.volume_records().map(|()| {
                    let mut p = Parcel::new();
                    sm::write_get_volume_records_reply::<NoDisk>(&mut p, Some(&[]));
                    p
                }))
            }
            sm::GET_PRIMARY_STORAGE_UUID => {
                sm::GetPrimaryStorageUuid::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                sm::write_get_primary_storage_uuid_reply(&mut p, &self.0.primary_storage_uuid());
                Ok(p)
            }
            sm::GET_VOLUMES => {
                sm::GetVolumes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                sm::write_get_volumes_reply(&mut p, Some(&self.0.volume_infos()));
                Ok(p)
            }
            sm::MKDIRS => {
                let args = sm::Mkdirs::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .mkdirs(call, args.calling_pkg.as_deref(), args.path.as_deref()),
                )
            }
            sm::FIXUP_APP_DIR => {
                let args = sm::FixupAppDir::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let path = args.path.ok_or(BAD_VALUE)?;
                reply(self.0.fixup_app_dir(&path).map(|()| {
                    let mut p = Parcel::new();
                    sm::write_fixup_app_dir_reply(&mut p);
                    p
                }))
            }
            sm::REGISTER_LISTENER => {
                let args = sm::RegisterListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(Binder::Handle(listener)) = args.listener {
                    self.0.register_storage_listener(listener);
                }
                let mut p = Parcel::new();
                sm::write_register_listener_reply(&mut p);
                Ok(p)
            }
            sm::UNREGISTER_LISTENER => {
                let args = sm::UnregisterListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(Binder::Handle(listener)) = args.listener {
                    self.0.unregister_storage_listener(listener);
                }
                let mut p = Parcel::new();
                sm::write_unregister_listener_reply(&mut p);
                Ok(p)
            }
            sm::SET_CLOUD_MEDIA_PROVIDER => {
                let args =
                    sm::SetCloudMediaProvider::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .set_cloud_media_provider(call, args.authority)
                        .map(|()| {
                            let mut p = Parcel::new();
                            sm::write_set_cloud_media_provider_reply(&mut p);
                            p
                        }),
                )
            }
            sm::GET_CLOUD_MEDIA_PROVIDER => {
                sm::GetCloudMediaProvider::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.cloud_media_provider(call).map(|authority| {
                    let mut p = Parcel::new();
                    sm::write_get_cloud_media_provider_reply(&mut p, &authority);
                    p
                }))
            }
            sm::GET_ALLOCATABLE_BYTES => {
                let a = sm::GetAllocatableBytes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .allocatable_bytes(
                            a.volume_uuid.as_deref(),
                            a.flags,
                            call.sender_euid as i32,
                        )
                        .map(|n| {
                            let mut p = Parcel::new();
                            sm::write_get_allocatable_bytes_reply(&mut p, n);
                            p
                        }),
                )
            }
            sm::ALLOCATE_BYTES => {
                let a = sm::AllocateBytes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let uid = call.sender_euid as i32;
                reply(
                    self.0
                        .allocate_bytes(a.volume_uuid.as_deref(), a.bytes, a.flags, uid)
                        .map(|()| {
                            let mut p = Parcel::new();
                            sm::write_allocate_bytes_reply(&mut p);
                            p
                        }),
                )
            }
            sm::GET_INTERNAL_STORAGE_BLOCK_DEVICE_SIZE => {
                sm::GetInternalStorageBlockDeviceSize::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                reply(self.0.internal_storage_block_device_size().map(|size| {
                    let mut p = Parcel::new();
                    sm::write_get_internal_storage_block_device_size_reply(&mut p, size);
                    p
                }))
            }
            code => reply(Err(self.0.unimplemented("mount", self.descriptor(), code))),
        }
    }
}

/// fstrim's `IVoldTaskListener`: a trimmed path's bytes and time recorded
/// in the dropbox (`TAG_STORAGE_TRIM`), failures ignored.
struct TrimListener(std::sync::Weak<AppHost>);

impl Service for TrimListener {
    fn descriptor(&self) -> &str {
        TASK_LISTENER_DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            ON_STATUS => {
                let status = call.data.read_i32()?;
                let extras = crate::bundle::read(&mut call.data)?.unwrap_or_default();
                if status != 0 {
                    return Ok(Parcel::new());
                }
                let Some(host) = self.0.upgrade() else {
                    return Ok(Parcel::new());
                };
                // `getString("path")`, whose absence the original's
                // `scrubPath` throws on; `getLong`'s default 0.
                let Some(crate::bundle::Value::String(Some(path))) = extras.get("path") else {
                    eprintln!("app host: mount: fstrim status without a path");
                    return Ok(Parcel::new());
                };
                let long = |key: &str| match extras.get(key) {
                    Some(crate::bundle::Value::Long(v)) => *v,
                    _ => 0,
                };
                let line = format!(
                    "{} {} {}",
                    AppHost::scrub_path(path),
                    long("bytes"),
                    long("time")
                );
                if let Some(dropbox) = host.dropbox.get()
                    && let Err(e) = dropbox.add_text(TAG_STORAGE_TRIM, &line)
                {
                    eprintln!("app host: mount: dropbox: {}", e.message);
                }
                Ok(Parcel::new())
            }
            ON_FINISHED => Ok(Parcel::new()),
            _ => Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_read_errors_name_guest_files_in_the_exception_and_cause() {
        let data = super::super::test_temp_dir("storage-read-error");
        std::fs::create_dir_all(data.join("system/storage.xml")).unwrap();
        let host = AppHost::new(crate::test_process(1000));
        super::super::configure_test_host(&host, &data, &data, &data, Box::new(|_| None));
        let error = host.storage_settings().unwrap_err();
        assert!(
            error
                .message
                .starts_with("java.io.IOException: /data/system/storage.xml:"),
            "{error:?}"
        );
        assert!(!error.message.contains(data.to_str().unwrap()), "{error:?}");
        let mut payload = Parcel::new();
        payload.write_raw(error.parcelable.unwrap().bytes(), &[]);
        let mut reader = payload.reader();
        assert_eq!(
            reader.read_string16().unwrap().as_deref(),
            Some("android.os.ParcelableException")
        );
        assert_eq!(
            reader.read_string16().unwrap().as_deref(),
            Some("java.io.IOException")
        );
        let cause = reader.read_string16().unwrap().unwrap();
        assert!(cause.starts_with("/data/system/storage.xml:"), "{cause}");
        assert!(!cause.contains(data.to_str().unwrap()), "{cause}");
        assert_eq!(reader.remaining(), 0);
        host.process.shutdown().unwrap();
        std::fs::remove_dir_all(data).unwrap();
    }

    struct UnlockStorage {
        host: std::sync::Weak<AppHost>,
        agent: bool,
        fail: bool,
        calls: Arc<Mutex<Vec<u32>>>,
    }

    impl Service for UnlockStorage {
        fn descriptor(&self) -> &str {
            if self.agent {
                super::super::AGENT_DESCRIPTOR
            } else {
                VOLD_DESCRIPTOR
            }
        }

        fn transact(&self, call: &mut Call<'_>) -> Reply {
            call.data.enforce_interface(self.descriptor())?;
            assert_eq!(
                call.code,
                if self.agent {
                    MOUNT_STORAGE
                } else {
                    ON_USER_STARTED
                }
            );
            assert_eq!(call.data.read_i32()?, USER);
            let host = self.host.upgrade().unwrap();
            assert!(host.ce_storage_prepared(USER));
            assert!(host.user_unlocking_or_unlocked(USER).unwrap());
            assert!(host.data().unwrap().join("media/0").is_dir());
            if self.agent {
                assert_eq!(call.data.read_i32()?, 10100);
            }
            assert_eq!(call.data.remaining(), 0);
            self.calls.lock().unwrap().push(call.code);
            let mut reply = Parcel::new();
            if self.fail {
                reply.write_exception(&Exception::new(EX_SERVICE_SPECIFIC, "storage unavailable"));
            } else {
                if self.agent {
                    std::fs::create_dir(host.data().unwrap().join("media/0/Android")).unwrap();
                }
                reply.write_no_exception();
            }
            Ok(reply)
        }
    }

    /// UserController.finishUserUnlocking (CE, then unlocking) precedes
    /// StorageManagerService.onUserUnlocking (onUserStarted, then mount).
    /// SMS logs either external-storage failure and user boot continues.
    #[test]
    fn emulated_storage_starts_at_unlock_and_failures_do_not_abort_it() {
        for failure in [None, Some(ON_USER_STARTED), Some(MOUNT_STORAGE)] {
            crate::system::with_discovery_binders(|host, publish, _, export| {
                publish(&mut |package| {
                    if package.package_name == "p" {
                        let mut provider = crate::package::pkg::Provider {
                            main: package.activities[0].main.clone(),
                            authority: Some("media".into()),
                            ..Default::default()
                        };
                        provider.main.component.name = "p.Media".into();
                        package.providers.push(provider);
                    }
                });
                let calls = Arc::new(Mutex::new(Vec::new()));
                let service = |agent| {
                    export(Arc::new(UnlockStorage {
                        host: Arc::downgrade(host),
                        agent,
                        fail: failure
                            == Some(if agent {
                                MOUNT_STORAGE
                            } else {
                                ON_USER_STARTED
                            }),
                        calls: calls.clone(),
                    }))
                };
                let vold = service(false);
                *host.agent.lock().unwrap() = Some(service(true));
                assert!(!host.data().unwrap().join("media/0").exists());
                assert!(!host.ce_storage_prepared(USER));
                assert!(!host.user_unlocking_or_unlocked(USER).unwrap());

                // A failed CE preparation must leave vold's user unstarted.
                let user = StartedUser {
                    vold: host.process.strong(vold.handle),
                    installd: host.process.strong(vold.handle),
                    fingerprint: String::new(),
                    updated: false,
                };
                assert!(host.unlock_user_storage(&user).is_err());
                assert!(calls.lock().unwrap().is_empty());
                assert!(!host.user_unlocking_or_unlocked(USER).unwrap());

                // Vold's prepareUserStorage(CE) owns this directory, before
                // UserController dispatches the services' unlocking callbacks.
                std::fs::create_dir_all(host.data().unwrap().join("media/0")).unwrap();
                host.storage.prepared_users.lock().unwrap().insert(USER, 3);
                host.finish_storage_unlock(&vold).unwrap();
                assert!(host.user_unlocking_or_unlocked(USER).unwrap());
                assert_eq!(
                    *calls.lock().unwrap(),
                    if failure == Some(ON_USER_STARTED) {
                        vec![ON_USER_STARTED]
                    } else {
                        vec![ON_USER_STARTED, MOUNT_STORAGE]
                    }
                );
                assert_eq!(
                    host.data().unwrap().join("media/0/Android").exists(),
                    failure.is_none()
                );
                assert!(host.storage.volume.lock().unwrap().is_none());
            });
        }
    }

    /// StorageManager's CE fact precedes UserController's unlocking state.
    /// Both the internal bridge and the public storage API must agree.
    #[test]
    fn ce_storage_availability_is_independent_of_user_unlocking() {
        use aim_service_aidl::dev_aim_server_ipackageinternalstoragebridge as internal;
        let process = crate::test_process(1000);
        let host = AppHost::new(process.clone());
        let Binder::Local(ptr) = process.add_service(Arc::new(
            super::super::package_internal_storage::PackageInternalStorage::new(host.clone()),
        )) else {
            unreachable!()
        };
        let bridge = process.local_service(ptr).unwrap();
        let Binder::Local(ptr) = process.add_service(Arc::new(StorageManager(host.clone()))) else {
            unreachable!()
        };
        let mount = process.local_service(ptr).unwrap();
        let available = || {
            let mut request = Parcel::new();
            internal::CeUnlocked { user: 0 }.write(&mut request);
            let response = bridge
                .transact(internal::CE_UNLOCKED, &request, false)
                .unwrap();
            let native = internal::read_ce_unlocked_reply(&mut response.reader())
                .unwrap()
                .unwrap();
            let mut request = Parcel::new();
            sm::IsCeStorageUnlocked { user_id: 0 }.write(&mut request);
            let response = mount
                .transact(sm::IS_CE_STORAGE_UNLOCKED, &request, false)
                .unwrap();
            assert_eq!(
                sm::read_is_ce_storage_unlocked_reply(&mut response.reader())
                    .unwrap()
                    .unwrap(),
                native
            );
            native
        };
        assert!(!available());
        host.unlocked_users.lock().unwrap().insert(0, 0);
        assert!(
            !available(),
            "a lifecycle flag alone cannot permit CE reconciliation"
        );
        host.unlocked_users.lock().unwrap().clear();
        host.storage.prepared_users.lock().unwrap().insert(0, 1);
        assert!(!available());
        host.storage.prepared_users.lock().unwrap().insert(0, 3);
        assert!(available());
        assert!(!host.user_unlocking_or_unlocked(0).unwrap());
        assert!(!host.ce_storage_prepared(10));
    }

    /// StorageManagerService.isBroadcastWorthy's complete type/state matrix,
    /// including the boot gate and the nullable filesystem UUID on the wire.
    #[test]
    fn internal_volume_broadcast_follows_the_boot_gate_and_volume_state() {
        use crate::bundle::Value;
        let mut info = VolumeInfo::created(TYPE_PRIVATE, None, -1);
        for kind in -1..=6 {
            info.kind = kind;
            for state in -1..=9 {
                info.state = state;
                assert!(volume_state_intent(&info, false).is_none());
                let intent = volume_state_intent(&info, true);
                let expected = [0, 1, 2, 5].contains(&kind) && [0, 2, 3, 5, 6, 8].contains(&state);
                assert_eq!(intent.is_some(), expected, "type {kind}, state {state}");
                if let Some(intent) = intent {
                    assert_eq!(
                        intent.intent.action.as_deref(),
                        Some("android.os.storage.action.VOLUME_STATE_CHANGED")
                    );
                    assert_eq!(intent.intent.flags, 0x0500_0000);
                    let extras = intent.test_extras();
                    assert_eq!(extras.len(), 3);
                    assert_eq!(
                        extras["android.os.storage.extra.VOLUME_STATE"],
                        Value::Int(state)
                    );
                    assert_eq!(
                        extras["android.os.storage.extra.VOLUME_ID"],
                        Value::String(Some(VOLUME.into()))
                    );
                    assert_eq!(
                        extras["android.os.storage.extra.FS_UUID"],
                        Value::String(None)
                    );
                }
            }
        }
        info.kind = TYPE_PRIVATE;
        info.state = STATE_MOUNTED;
        info.fs_uuid = Some("1234-5678".into());
        assert_eq!(
            volume_state_intent(&info, true).unwrap().test_extras()["android.os.storage.extra.FS_UUID"],
            Value::String(info.fs_uuid)
        );
    }

    /// A listener of this process is told each volume's change, until it
    /// is unregistered.
    #[test]
    fn native_listeners_are_told_volume_changes() {
        let process = crate::test_process(1000);
        let host = AppHost::new(process);
        let told = Arc::new(Mutex::new(Vec::new()));
        let seen = told.clone();
        let id = host.register_native_storage_listener(Arc::new(move |kind, state, uuid| {
            seen.lock()
                .unwrap()
                .push((kind, state, uuid.map(String::from)));
        }));
        let mut info = VolumeInfo::created(TYPE_PRIVATE, None, -1);
        info.state = STATE_MOUNTED;
        info.fs_uuid = Some("1234-5678".into());
        info.mount_flags = 0;
        host.volume_state_changed(&info, STATE_UNMOUNTED);
        host.unregister_native_storage_listener(id);
        host.volume_state_changed(&info, STATE_UNMOUNTED);
        assert_eq!(
            *told.lock().unwrap(),
            [(TYPE_PRIVATE, STATE_MOUNTED, Some("1234-5678".into()))]
        );
    }

    /// A volume of another UUID is not found, as `findPathForUuid`
    /// throws, in the envelope `ParcelableException` writes.
    #[test]
    fn other_volumes_are_not_found() {
        assert!(AppHost::find_path_for_uuid(None).is_ok());
        let e = AppHost::find_path_for_uuid(Some("1234-5678")).unwrap_err();
        assert_eq!(
            e.message,
            "java.io.FileNotFoundException: Failed to find a storage device for 1234-5678"
        );
        let mut p = Parcel::new();
        p.write_exception(&e);
        assert!(p.reader().read_exception().unwrap().is_err());
    }

    #[test]
    fn known_app_dirs_name_their_package_and_user() {
        assert_eq!(
            known_app_dir("/storage/emulated/0/Android/data/com.example/files/"),
            Some((Some("0".into()), "com.example".into()))
        );
        assert_eq!(
            known_app_dir("/storage/1234-ABCD/android/OBB/com.example/"),
            Some((None, "com.example".into()))
        );
        assert_eq!(known_app_dir("/storage/emulated/0/Download/"), None);
        assert_eq!(
            normalize("/storage/emulated/0/Android/data/x/../com.example/./files"),
            "/storage/emulated/0/Android/data/com.example/files"
        );
    }

    #[test]
    fn a_volume_info_parcels_as_the_original() {
        let mut info = VolumeInfo::created(2, None, 0);
        info.state = STATE_MOUNTED;
        info.path = Some("/storage/emulated".into());
        let mut p = Parcel::new();
        info.write_to(&mut p);
        let mut r = p.reader();
        assert_eq!(r.read_string8().unwrap().as_deref(), Some(VOLUME));
        assert_eq!(r.read_i32().unwrap(), 2); // `TYPE_EMULATED`
        assert_eq!(r.read_i32().unwrap(), 0); // no disk
        assert_eq!(r.read_string8().unwrap(), None);
        assert_eq!(
            r.read_i32().unwrap(),
            MOUNT_FLAG_PRIMARY | MOUNT_FLAG_VISIBLE_FOR_WRITE
        );
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.read_i32().unwrap(), STATE_MOUNTED);
        for _ in 0..3 {
            assert_eq!(r.read_string8().unwrap(), None);
        }
        assert_eq!(
            r.read_string8().unwrap().as_deref(),
            Some("/storage/emulated")
        );
        assert_eq!(r.read_string8().unwrap(), None);
    }

    /// `mVolumes` hashes its ids: `"private"` hashes below `"emulated;0"`
    /// and sorts first.
    #[test]
    fn volumes_keep_their_array_map_order() {
        let ids = crate::package::info::array_order(
            vec![VolumeInfo::created(2, None, 0), VolumeInfo::internal()],
            |v| v.id,
        );
        assert_eq!(
            ids.iter().map(|v| v.id).collect::<Vec<_>>(),
            ["private", VOLUME]
        );
    }
}
