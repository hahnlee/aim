//! `mount`: StorageManager for the app host, as StorageManagerService
//! keeps external storage at `android-16.0.0_r1`. It connects vold
//! (`setListener`, `reset`, user 0's addition and start), has the agent
//! mount user 0's emulated volume (`aim-app-host-agent`, which serves its
//! FUSE connection in MediaProvider's place), follows the volume's state
//! from vold's listener calls, and answers apps' `getVolumeList` and
//! `mkdirs`.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_os_storage_istoragemanager as sm;

use super::{AppHost, reply};

/// vold's service and `IVold`'s calls the app host makes, in the
/// interface's order.
const VOLD: &str = "vold";
const VOLD_DESCRIPTOR: &str = "android.os.IVold";
const SET_LISTENER: u32 = 1;
const RESET: u32 = 4;
const ON_USER_ADDED: u32 = 6;
const ON_USER_STARTED: u32 = 8;
const SETUP_APP_DIR: u32 = 23;
/// `IVoldListener`'s volume calls.
const LISTENER_DESCRIPTOR: &str = "android.os.IVoldListener";
const ON_VOLUME_CREATED: u32 = 5;
const ON_VOLUME_STATE_CHANGED: u32 = 6;
const ON_VOLUME_DESTROYED: u32 = 10;
/// How long vold may take to register.
const VOLD_PATIENCE: Duration = Duration::from_secs(30);
/// The agent's `mountStorage(int userId)`.
pub(super) const MOUNT_STORAGE: u32 = 5;
/// `VolumeInfo.STATE_*` (`IVold.VOLUME_STATE_*`).
const STATE_UNMOUNTED: i32 = 0;
const STATE_MOUNTED: i32 = 2;
/// User 0's emulated volume, its paths and its serial number
/// (`UserInfo.serialNumber` of the system user).
const VOLUME: &str = "emulated;0";
const USER: i32 = 0;
const SERIAL: i32 = 0;
/// `PackageManager.PROPERTY_NO_APP_DATA_STORAGE`.
const NO_APP_DATA_STORAGE: &str = "android.internal.PROPERTY_NO_APP_DATA_STORAGE";

/// The emulated volume's state, as vold reports it; `None` until vold
/// created it.
#[derive(Default)]
pub struct Volumes {
    state: Mutex<Option<i32>>,
    vold: Mutex<Option<Strong>>,
}

impl AppHost {
    /// StorageManagerService's start: vold connected and reset, user 0
    /// added and started, its emulated volume mounted by the agent.
    pub(super) fn start_storage(&self) -> Result<(), String> {
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
        let listener = self.process.add_service(Arc::new(Listener(self.me.clone())));
        let call = |code: u32, name: &str, write: &dyn Fn(&mut Parcel)| -> Result<(), String> {
            let mut data = Parcel::new();
            data.write_interface_token(VOLD_DESCRIPTOR);
            write(&mut data);
            let reply = vold.transact(code, &data, false).map_err(|s| format!("vold {name}: {s}"))?;
            match reply.reader().read_exception() {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(format!("vold {name}: {e:?}")),
                Err(s) => Err(format!("vold {name}: {s}")),
            }
        };
        call(SET_LISTENER, "setListener", &|p| p.write_binder(Some(listener)))?;
        call(RESET, "reset", &|_| {})?;
        call(ON_USER_ADDED, "onUserAdded", &|p| {
            p.write_i32(USER);
            p.write_i32(SERIAL);
            p.write_i32(-1); // shares storage with no user
        })?;
        call(ON_USER_STARTED, "onUserStarted", &|p| p.write_i32(USER))?;
        *self.storage.vold.lock().unwrap() = Some(vold);
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

    /// Whether user 0's emulated volume is mounted
    /// (`StorageManagerInternal.isFuseMounted`).
    pub(super) fn storage_mounted(&self) -> bool {
        *self.storage.state.lock().unwrap() == Some(STATE_MOUNTED)
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
        let ps = state.packages.get(&package).ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, format!("no {package}")))?;
        Ok(USER * 100_000 + ps.app_id as i32)
    }

    /// `getVolumeList`: user 0's primary emulated volume, or the stub
    /// StorageManagerService makes before vold created it.
    fn volume_list(&self, call: &Call<'_>, user: i32, package: Option<&str>) -> Result<Parcel, Exception> {
        let uid = call.sender_euid as i32;
        self.check_package(uid, package)?;
        if uid / 100_000 != user {
            return Err(Exception::security("Need INTERACT_ACROSS_USERS to get volumes for another user"));
        }
        let state = *self.storage.state.lock().unwrap();
        let inputs = self.inputs()?;
        let unknown = inputs.packages.framework_string("unknownName").ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no unknownName"))?;
        let volume = match state.filter(|_| user == USER) {
            // Mounted visible for writing by user 0, so listed for writing
            // or reading alike (`isVisibleForWrite`, `isVisibleForUser`).
            Some(state) => {
                let description = inputs
                    .packages
                    .framework_string("storage_internal")
                    .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no storage_internal"))?;
                StorageVolume {
                    id: VOLUME.into(),
                    path: format!("/storage/emulated/{user}"),
                    internal_path: format!("/data/media/{user}"),
                    description,
                    emulated: true,
                    user,
                    // `StorageManager.UUID_DEFAULT`, written as its
                    // string, null for internal storage.
                    uuid: Some(None),
                    state: environment(state).into(),
                }
            }
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
    fn mkdirs(&self, call: &Call<'_>, package: Option<&str>, path: Option<&str>) -> Result<Parcel, Exception> {
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
                props.iter().any(|(name, p)| name == NO_APP_DATA_STORAGE && p.value == crate::package::pkg::PropertyValue::Bool(true))
            });
        if no_storage {
            return Err(Exception::security(format!("{package} should not have {path}")));
        }
        let mut app_path = normalize(path);
        if !app_path.ends_with('/') {
            app_path.push('/');
        }
        let Some((user_part, owner)) = known_app_dir(&app_path) else {
            return Err(Exception::security(format!("Invalid mkdirs path: {path} is not a known app path.")));
        };
        if owner != package {
            return Err(Exception::security(format!("Invalid mkdirs path: {path} does not contain calling package {package}")));
        }
        if user_part.as_ref().is_some_and(|u| *u != user.to_string()) || (user_part.is_none() && user != USER) {
            return Err(Exception::security(format!("Invalid mkdirs path: {path} does not match calling user id {user}")));
        }
        let vold = self.storage.vold.lock().unwrap();
        let vold = vold.as_ref().ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, format!("Failed to prepare {path}")))?;
        let mut data = Parcel::new();
        data.write_interface_token(VOLD_DESCRIPTOR);
        data.write_string16(Some(&app_path));
        data.write_i32(uid);
        let reply = vold.transact(SETUP_APP_DIR, &data, false).map_err(|s| Exception::new(EX_ILLEGAL_STATE, format!("Failed to prepare {path}: {s}")))?;
        if let Ok(Err(e)) = reply.reader().read_exception() {
            return Err(Exception::new(EX_ILLEGAL_STATE, format!("Failed to prepare {path}: {e:?}")));
        }
        let mut p = Parcel::new();
        sm::write_mkdirs_reply(&mut p);
        Ok(p)
    }

    /// `isUidOwnerOfPackageOrSystem`, `AppOpsManager.checkPackage`.
    fn check_package(&self, uid: i32, package: Option<&str>) -> Result<(), Exception> {
        if uid % 100_000 == 1000 {
            return Ok(());
        }
        let (state, _) = self.packages()?;
        let owns = package.and_then(|p| state.packages.get(p)).is_some_and(|ps| ps.app_id as i32 == uid % 100_000);
        if owns { Ok(()) } else { Err(Exception::security("callingPackage does not match UID")) }
    }
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
        Some((user, rest)) if !user.is_empty() && user.bytes().all(|b| b.is_ascii_digit()) && rest.to_ascii_lowercase().starts_with("android/") => {
            (Some(user.to_string()), rest)
        }
        _ => (None, rest),
    };
    let (android, rest) = rest.split_once('/')?;
    let (kind, rest) = rest.split_once('/')?;
    if !android.eq_ignore_ascii_case("android") || !["data", "media", "obb", "sandbox"].iter().any(|k| kind.eq_ignore_ascii_case(k)) {
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

/// vold's listener: the emulated volume's creation and states.
struct Listener(std::sync::Weak<AppHost>);

impl Service for Listener {
    fn descriptor(&self) -> &str {
        LISTENER_DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(host) = self.0.upgrade() else { return Ok(Parcel::new()) };
        call.data.enforce_interface(LISTENER_DESCRIPTOR).map_err(|_| BAD_VALUE)?;
        let id = call.data.read_string16().map_err(|_| BAD_VALUE)?;
        if id.as_deref() != Some(VOLUME) {
            return Ok(Parcel::new());
        }
        let mut state = host.storage.state.lock().unwrap();
        match call.code {
            ON_VOLUME_CREATED => *state = Some(STATE_UNMOUNTED),
            ON_VOLUME_STATE_CHANGED => *state = Some(call.data.read_i32().map_err(|_| BAD_VALUE)?),
            ON_VOLUME_DESTROYED => *state = None,
            _ => {}
        }
        Ok(Parcel::new())
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
                reply(self.0.volume_list(call, args.user_id, args.calling_package.as_deref()))
            }
            sm::MKDIRS => {
                let args = sm::Mkdirs::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.mkdirs(call, args.calling_pkg.as_deref(), args.path.as_deref()))
            }
            code => reply(Err(self.0.unimplemented("mount", code))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_app_dirs_name_their_package_and_user() {
        assert_eq!(
            known_app_dir("/storage/emulated/0/Android/data/com.example/files/"),
            Some((Some("0".into()), "com.example".into()))
        );
        assert_eq!(known_app_dir("/storage/1234-ABCD/android/OBB/com.example/"), Some((None, "com.example".into())));
        assert_eq!(known_app_dir("/storage/emulated/0/Download/"), None);
        assert_eq!(normalize("/storage/emulated/0/Android/data/x/../com.example/./files"), "/storage/emulated/0/Android/data/com.example/files");
    }
}
