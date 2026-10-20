//! User 0's storage and its apps' data directories, prepared as
//! PackageManagerService and UserManagerService prepare them at
//! `android-16.0.0_r1`: `AppDataHelper.fixAppsDataOnBoot` at the package
//! side's start, then at the user's start and unlock
//! `UserDataPreparer.prepareUserData` (vold's `prepareUserStorage`, the
//! directories' serial numbers, installd's `createUserData`) and
//! `AppDataHelper.reconcileAppsData`, whose installd `createAppData`
//! makes each installed package's directories and reports their inodes,
//! which the package restrictions keep.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::android_os_iinstalld as installd;
use aim_service_aidl::{ReadParcelable, Returned, WriteParcelable};
use sha1::{Digest, Sha1};

use super::AppHost;
use super::storage::VOLD_DESCRIPTOR;
use crate::package::model::{PackageState, State};
use crate::package::pkg::booleans::{CORE_APP, DEFAULT_TO_DEVICE_PROTECTED_STORAGE};

const INSTALLD: &str = "installd";
const INSTALLD_PATIENCE: Duration = Duration::from_secs(30);

/// `IVold.prepareUserStorage`, `destroyUserStorage`.
const PREPARE_USER_STORAGE: u32 = 48;
const DESTROY_USER_STORAGE: u32 = 49;
/// `IAppHostAgent.enforceSerialNumber(String path, int serialNumber)`.
const ENFORCE_SERIAL_NUMBER: u32 = 14;

/// `StorageManager.FLAG_STORAGE_DE`, `FLAG_STORAGE_CE`;
/// `Installer.FLAG_STORAGE_SDK`.
pub(super) const FLAG_STORAGE_DE: i32 = 1;
pub(super) const FLAG_STORAGE_CE: i32 = 2;
const FLAG_STORAGE_SDK: i32 = 8;
/// `Installer.Batch.CREATE_APP_DATA_BATCH_SIZE`.
const CREATE_APP_DATA_BATCH_SIZE: usize = 256;
/// `PackageManager.PROPERTY_NO_APP_DATA_STORAGE`.
const NO_APP_DATA_STORAGE: &str = "android.internal.PROPERTY_NO_APP_DATA_STORAGE";

/// `android.os.CreateAppDataArgs`.
#[derive(Clone, Debug, PartialEq)]
struct CreateAppDataArgs {
    uuid: Option<String>,
    package_name: String,
    user_id: i32,
    flags: i32,
    app_id: i32,
    previous_app_id: i32,
    se_info: String,
    target_sdk_version: i32,
}

impl WriteParcelable for CreateAppDataArgs {
    fn write_to(&self, p: &mut Parcel) {
        let start = p.position();
        p.write_i32(0);
        p.write_string16(self.uuid.as_deref());
        p.write_string16(Some(&self.package_name));
        p.write_i32(self.user_id);
        p.write_i32(self.flags);
        p.write_i32(self.app_id);
        p.write_i32(self.previous_app_id);
        p.write_string16(Some(&self.se_info));
        p.write_i32(self.target_sdk_version);
        p.set_i32_at(start, (p.position() - start) as i32);
    }
}

/// `android.os.CreateAppDataResult`, as its structured parcelable reads:
/// the fields its size covers, the rest skipped.
#[derive(Debug, Default, PartialEq)]
struct CreateAppDataResult {
    ce_data_inode: i64,
    de_data_inode: i64,
    exception_code: i32,
    exception_message: Option<String>,
}

impl ReadParcelable for CreateAppDataResult {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let start = r.position();
        let size = r.read_i32()?;
        if size < 4 {
            return Err(BAD_VALUE);
        }
        let end = start + size as usize;
        let mut result = CreateAppDataResult::default();
        if r.position() < end {
            result.ce_data_inode = r.read_i64()?;
        }
        if r.position() < end {
            result.de_data_inode = r.read_i64()?;
        }
        if r.position() < end {
            result.exception_code = r.read_i32()?;
        }
        if r.position() < end {
            result.exception_message = r.read_string16()?;
        }
        r.set_position(end);
        Ok(result)
    }
}

/// `PackagePartitions.FINGERPRINT`: `SystemProperties.digestOf` of the
/// build's and the package partitions' fingerprints.
pub(super) fn fingerprint(property: impl Fn(&str) -> Option<String>) -> String {
    let mut keys: Vec<String> = ["system", "vendor", "odm", "oem", "product", "system_ext"]
        .iter()
        .map(|p| format!("ro.{p}.build.fingerprint"))
        .collect();
    keys.push("ro.build.fingerprint".into());
    keys.sort();
    let mut digest = Sha1::new();
    for key in &keys {
        let value = property(key).unwrap_or_default();
        digest.update(format!("{key}={value}\n").as_bytes());
    }
    digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `VMRuntime.is64BitAbi`.
fn is_64_bit_abi(abi: &str) -> Result<bool, String> {
    match abi {
        "arm64-v8a" | "x86_64" | "riscv64" => Ok(true),
        "armeabi" | "armeabi-v7a" | "x86" => Ok(false),
        _ => Err(format!("Unsupported ABI: {abi}")),
    }
}

/// `AppDataHelper.shouldHaveAppStorage`.
fn should_have_app_storage(ps: &PackageState) -> bool {
    let Some(pkg) = &ps.pkg else {
        return true;
    };
    let no_storage = pkg.properties.as_ref().is_some_and(|props| {
        props.iter().any(|(name, p)| {
            name == NO_APP_DATA_STORAGE && p.value == crate::package::pkg::PropertyValue::Bool(true)
        })
    });
    !no_storage && ps.app_id >= 0
}

/// `assertPackageStorageValid` of the internal volume's directory
/// `package` of `user`.
fn assert_package_storage_valid(state: &State, package: &str, user: i32) -> Result<(), String> {
    let Some(ps) = state.packages.get(package) else {
        return Err(format!("Package {package} is unknown"));
    };
    if ps.volume_uuid.is_some() {
        return Err(format!(
            "Package {package} found on unknown volume null; expected volume {}",
            ps.volume_uuid.as_deref().unwrap_or_default()
        ));
    }
    let state = crate::package::info::user_state(ps, user);
    if !state.installed && !(state.ce_data_inode > 0 || state.de_data_inode > 0) {
        return Err(format!(
            "Package {package} not installed for user {user} or was deleted without DELETE_KEEP_DATA"
        ));
    }
    if !should_have_app_storage(ps) {
        return Err(format!("Package {package} shouldn't have storage"));
    }
    Ok(())
}

/// The reply of a call `name` to `service`, read by `read`.
fn call<T>(
    service: &Strong,
    name: &str,
    code: u32,
    data: &Parcel,
    read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<Returned<T>>,
) -> Result<T, String> {
    let reply = service
        .transact(code, data, false)
        .map_err(|s| format!("{name}: {s}"))?;
    match read(&mut reply.reader()) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err(format!("{name}: {}", e.message)),
        Err(s) => Err(format!("{name} reply: {s}")),
    }
}

/// Deletes what `dir` holds (`FileUtils.deleteContents`), and the
/// directory too with `itself`, logging what cannot go.
fn delete_contents(dir: &Path, itself: bool) {
    let result = if itself {
        std::fs::remove_dir_all(dir)
    } else {
        std::fs::read_dir(dir).and_then(|entries| {
            entries.into_iter().try_for_each(|entry| {
                let path = entry?.path();
                if path.symlink_metadata()?.is_dir() {
                    std::fs::remove_dir_all(path)
                } else {
                    std::fs::remove_file(path)
                }
            })
        })
    };
    match result {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            eprintln!(
                "app host: user data: failed to delete {}: {e}",
                dir.display()
            )
        }
        _ => {}
    }
}

/// A package's `createAppData` arguments and what is done with its
/// result.
struct Prepare<'a> {
    ps: &'a PackageState,
    args: CreateAppDataArgs,
    migrate: bool,
}

impl AppHost {
    /// installd, once it registers.
    pub(super) fn installd(&self) -> Result<Strong, String> {
        let deadline = Instant::now() + INSTALLD_PATIENCE;
        loop {
            if let Ok(installd) = super::display::check_service(&self.process, INSTALLD) {
                return Ok(installd);
            }
            if Instant::now() > deadline {
                return Err("installd did not register".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// `StorageManager.isFileEncrypted`: `ro.crypto.type` is `file`.
    fn file_encrypted(&self) -> bool {
        self.property("ro.crypto.type").as_deref() == Some("file")
    }

    /// `AppDataHelper.fixAppsDataOnBoot`: the system user's core apps'
    /// data (DE, and CE without file-based encryption), installd's
    /// `fixupAppData`, then the other apps'.
    pub(super) fn fix_apps_data_on_boot(&self, installd: &Strong) -> Result<(), String> {
        // system_server is zygote's child, so PackageManagerService starts
        // after init's `zygote-start`, once post-fs-data's `init_user0`
        // has made `/data/data` (`/data/user/0`) and user 0's DE storage;
        // installd, of class main, is up before that.
        let deadline = Instant::now() + INSTALLD_PATIENCE;
        while self.property("init.svc.zygote").is_none() {
            if Instant::now() > deadline {
                return Err("zygote did not start".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let flags = if self.file_encrypted() {
            FLAG_STORAGE_DE
        } else {
            FLAG_STORAGE_DE | FLAG_STORAGE_CE
        };
        let deferred = self.reconcile_apps_data_li(installd, 0, flags, true, true)?;
        let mut data = Parcel::new();
        installd::FixupAppData {
            uuid: None,
            flags: FLAG_STORAGE_DE | FLAG_STORAGE_CE,
        }
        .write(&mut data);
        if let Err(e) = call(
            installd,
            "fixupAppData",
            installd::FIXUP_APP_DATA,
            &data,
            installd::read_fixup_app_data_reply,
        ) {
            eprintln!("app host: app data: Trouble fixing GIDs: {e}");
        }
        let (state, _) = self.packages().map_err(|e| e.message)?;
        let packages: Vec<&PackageState> = deferred
            .iter()
            .filter_map(|name| state.packages.get(name))
            .filter(|ps| crate::package::info::user_state(ps, 0).installed)
            .collect();
        let count = packages.len();
        self.prepare_apps_data(installd, &packages, 0, flags, true)?;
        eprintln!("app host: app data: deferred reconcileAppsData finished {count} packages");
        Ok(())
    }

    /// `UserDataPreparer.prepareUserData` of the internal storage: the
    /// user's storage `flags` prepared by vold, its directories' serial
    /// numbers enforced, its data created by installd; a new user's
    /// destroyed and prepared once more on failure.
    pub(super) fn prepare_user_data(
        &self,
        vold: &Strong,
        installd: &Strong,
        user: i32,
        flags: i32,
    ) -> Result<(), String> {
        let Some((serial, last_logged_in, _)) =
            super::user_service::storage_user(self, user).map_err(|e| e.message)?
        else {
            return Ok(());
        };
        self.prepare_user_data_li(
            vold,
            installd,
            user,
            serial,
            last_logged_in == 0,
            flags,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_user_data_li(
        &self,
        vold: &Strong,
        installd: &Strong,
        user: i32,
        serial: i32,
        new_user: bool,
        flags: i32,
        allow_recover: bool,
    ) -> Result<(), String> {
        let prepared = (|| -> Result<(), String> {
            let mut data = Parcel::new();
            data.write_interface_token(VOLD_DESCRIPTOR);
            data.write_string16(None);
            data.write_i32(user);
            data.write_i32(flags);
            call(
                vold,
                "prepareUserStorage",
                PREPARE_USER_STORAGE,
                &data,
                |r| r.read_exception(),
            )?;
            let mut dirs = Vec::new();
            if flags & FLAG_STORAGE_DE != 0 {
                dirs.push(format!("/data/user_de/{user}"));
                dirs.push(format!("/data/system_de/{user}"));
            }
            if flags & FLAG_STORAGE_CE != 0 {
                dirs.push(format!("/data/user/{user}"));
                dirs.push(format!("/data/system_ce/{user}"));
            }
            for dir in dirs {
                self.call_agent(ENFORCE_SERIAL_NUMBER, "enforceSerialNumber", |p| {
                    p.write_string16(Some(&dir));
                    p.write_i32(serial);
                })
                .map_err(|e| e.message)?;
            }
            let mut data = Parcel::new();
            installd::CreateUserData {
                uuid: None,
                user_id: user,
                user_serial: serial,
                flags,
            }
            .write(&mut data);
            call(
                installd,
                "createUserData",
                installd::CREATE_USER_DATA,
                &data,
                installd::read_create_user_data_reply,
            )?;
            if flags & FLAG_STORAGE_CE != 0 && user == 0 {
                self.set_property(&format!("sys.user.{user}.ce_available"), "true");
            }
            Ok(())
        })();
        let Err(e) = prepared else {
            return Ok(());
        };
        if new_user {
            eprintln!(
                "app host: user data: Destroying user {user} on volume null because we failed to prepare: {e}"
            );
            self.destroy_user_data_li(vold, installd, user, flags);
        } else {
            eprintln!("app host: user data: Failed to prepare user {user} on volume null: {e}");
        }
        if allow_recover {
            return self.prepare_user_data_li(
                vold,
                installd,
                user,
                serial,
                new_user,
                flags | FLAG_STORAGE_DE,
                false,
            );
        }
        // The original reboots to recovery to wipe the new system user's
        // data, which the app host does not.
        Err(format!("prepareUserData failed for user {user}: {e}"))
    }

    /// `UserDataPreparer.destroyUserDataLI` of the internal storage, its
    /// failure logged.
    fn destroy_user_data_li(&self, vold: &Strong, installd: &Strong, user: i32, flags: i32) {
        let destroyed = (|| -> Result<(), String> {
            let mut data = Parcel::new();
            installd::DestroyUserData {
                uuid: None,
                user_id: user,
                flags,
            }
            .write(&mut data);
            call(
                installd,
                "destroyUserData",
                installd::DESTROY_USER_DATA,
                &data,
                installd::read_destroy_user_data_reply,
            )?;
            let root = self.data().map_err(|e| e.message)?;
            if flags & FLAG_STORAGE_DE != 0 {
                delete_contents(&root.join(format!("system/users/{user}")), true);
                delete_contents(&root.join(format!("system_de/{user}")), false);
            }
            if flags & FLAG_STORAGE_CE != 0 {
                delete_contents(&root.join(format!("system_ce/{user}")), false);
            }
            // `StorageManagerService.destroyUserStorage`, whose failure
            // is only logged.
            let mut data = Parcel::new();
            data.write_interface_token(VOLD_DESCRIPTOR);
            data.write_string16(None);
            data.write_i32(user);
            data.write_i32(flags);
            if let Err(e) = call(
                vold,
                "destroyUserStorage",
                DESTROY_USER_STORAGE,
                &data,
                |r| r.read_exception(),
            ) {
                eprintln!("app host: user data: {e}");
            }
            Ok(())
        })();
        if let Err(e) = destroyed {
            eprintln!("app host: user data: Failed to destroy user {user} on volume null: {e}");
        }
    }

    /// `AppDataHelper.reconcileAppsData` of the internal volume, the only
    /// writable private volume.
    pub(super) fn reconcile_apps_data(
        &self,
        installd: &Strong,
        user: i32,
        flags: i32,
        migrate: bool,
    ) -> Result<(), String> {
        self.reconcile_apps_data_li(installd, user, flags, migrate, false)
            .map(|_| ())
    }

    /// `reconcileAppsDataLI`: the user's directories of packages that
    /// should have none destroyed, then the installed packages' prepared
    /// (but for core apps' alone with `only_core`, which returns the
    /// others).
    fn reconcile_apps_data_li(
        &self,
        installd: &Strong,
        user: i32,
        flags: i32,
        migrate: bool,
        only_core: bool,
    ) -> Result<Vec<String>, String> {
        eprintln!(
            "app host: app data: reconcileAppsData for null u{user} 0x{flags:x} migrateAppData={migrate}"
        );
        let mut data = Parcel::new();
        installd::CleanupInvalidPackageDirs {
            uuid: None,
            user_id: user,
            flags,
        }
        .write(&mut data);
        if let Err(e) = call(
            installd,
            "cleanupInvalidPackageDirs",
            installd::CLEANUP_INVALID_PACKAGE_DIRS,
            &data,
            installd::read_cleanup_invalid_package_dirs_reply,
        ) {
            eprintln!("app host: app data: Failed to cleanup deleted dirs: {e}");
        }
        let root = self.data().map_err(|e| e.message)?;
        let (state, _) = self.packages().map_err(|e| e.message)?;
        if flags & FLAG_STORAGE_CE != 0 {
            self.destroy_invalid(
                installd,
                &state,
                &self.ce_dir(root, user),
                user,
                FLAG_STORAGE_CE,
            );
        }
        if flags & FLAG_STORAGE_DE != 0 {
            let de = root.join(format!("user_de/{user}"));
            self.destroy_invalid(installd, &state, &de, user, FLAG_STORAGE_DE);
        }
        let mut deferred = Vec::new();
        let mut packages = Vec::new();
        for ps in state
            .packages
            .values()
            .filter(|ps| ps.volume_uuid.is_none())
        {
            let Some(pkg) = &ps.pkg else {
                eprintln!(
                    "app host: app data: Odd, missing scanned package {}",
                    ps.name
                );
                continue;
            };
            if only_core && !pkg.is(CORE_APP) {
                deferred.push(ps.name.clone());
                continue;
            }
            if crate::package::info::user_state(ps, user).installed {
                packages.push(ps);
            }
        }
        let count = packages.len();
        self.prepare_apps_data(installd, &packages, user, flags, migrate)?;
        eprintln!("app host: app data: reconcileAppsData finished {count} packages");
        Ok(deferred)
    }

    /// `create_data_user_ce_path` on the host: user 0's is `/data/data`
    /// where that is a directory (`/data/user/0` binds it in the guest).
    fn ce_dir(&self, root: &Path, user: i32) -> PathBuf {
        let data = root.join("data");
        if user == 0 && data.symlink_metadata().is_ok_and(|m| m.is_dir()) {
            return data;
        }
        root.join(format!("user/{user}"))
    }

    /// The directories of `dir` whose package fails
    /// `assertPackageStorageValid`, destroyed by installd (`flags`).
    fn destroy_invalid(&self, installd: &Strong, state: &State, dir: &Path, user: i32, flags: i32) {
        let names: Vec<String> = match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        };
        for package in names {
            let Err(reason) = assert_package_storage_valid(state, &package, user) else {
                continue;
            };
            eprintln!(
                "app host: app data: Destroying {}/{package} due to: {reason}",
                dir.display()
            );
            let mut data = Parcel::new();
            installd::DestroyAppData {
                uuid: None,
                package_name: Some(package.clone()),
                user_id: user,
                flags,
                ce_data_inode: 0,
            }
            .write(&mut data);
            if let Err(e) = call(
                installd,
                "destroyAppData",
                installd::DESTROY_APP_DATA,
                &data,
                installd::read_destroy_app_data_reply,
            ) {
                eprintln!("app host: app data: Failed to destroy: {e}");
            }
        }
    }

    /// `prepareAppDataAndMigrate` of each of `packages` in one
    /// `Installer.Batch`, then the inodes installd reported recorded in
    /// the package restrictions.
    fn prepare_apps_data(
        &self,
        installd: &Strong,
        packages: &[&PackageState],
        user: i32,
        flags: i32,
        migrate: bool,
    ) -> Result<(), String> {
        let mut batch = Vec::new();
        for ps in packages {
            if !should_have_app_storage(ps) {
                eprintln!(
                    "app host: app data: Skipping preparing app data for {}",
                    ps.name
                );
                continue;
            }
            batch.push(Prepare {
                ps,
                args: create_app_data_args(ps, user, flags)?,
                migrate,
            });
        }
        let mut inodes = Vec::new();
        self.execute_batch(installd, &batch, user, flags, &mut inodes)?;
        self.record_inodes(user, inodes)
    }

    /// `Installer.Batch.execute` and each package's completion
    /// (`prepareAppData`'s, then the migration of `prepareAppDataAndMigrate`);
    /// a batch that fails is logged (`executeBatchLI`).
    fn execute_batch(
        &self,
        installd: &Strong,
        batch: &[Prepare<'_>],
        user: i32,
        flags: i32,
        inodes: &mut Vec<(String, i64, i64)>,
    ) -> Result<(), String> {
        for chunk in batch.chunks(CREATE_APP_DATA_BATCH_SIZE) {
            let mut data = Parcel::new();
            installd::CreateAppDataBatched {
                args: Some(chunk.iter().map(|p| Some(p.args.clone())).collect()),
            }
            .write(&mut data);
            let results = match call(
                installd,
                "createAppDataBatched",
                installd::CREATE_APP_DATA_BATCHED,
                &data,
                installd::read_create_app_data_batched_reply::<CreateAppDataResult>,
            ) {
                Ok(results) => results.unwrap_or_default(),
                Err(e) => {
                    eprintln!("app host: app data: Failed to execute pending operations: {e}");
                    return Ok(());
                }
            };
            for (prepare, result) in chunk.iter().zip(results) {
                let result = match result {
                    Some(r) if r.exception_code == 0 => Some(r),
                    failed => {
                        let message = failed.and_then(|r| r.exception_message);
                        self.recover(installd, prepare, user, flags, message.as_deref())
                    }
                };
                // A failed recovery leaves `createAppDataResult` null: the
                // completion throws, and nothing of it is recorded.
                let Some(result) = result else {
                    continue;
                };
                let ps = prepare.ps;
                record(inodes, ps, user, flags, &result);
                self.prepare_app_data_contents(installd, ps, user, flags)?;
                if prepare.migrate && self.maybe_migrate_app_data(installd, ps, user) {
                    let again = [Prepare {
                        ps,
                        args: prepare.args.clone(),
                        migrate: false,
                    }];
                    self.execute_batch(installd, &again, user, flags, inodes)?;
                }
            }
        }
        Ok(())
    }

    /// `prepareAppData`'s recovery of a package whose `createAppData`
    /// failed: its data destroyed (`destroyAppDataLeafLIF`) and created
    /// once more.
    fn recover(
        &self,
        installd: &Strong,
        prepare: &Prepare<'_>,
        user: i32,
        flags: i32,
        message: Option<&str>,
    ) -> Option<CreateAppDataResult> {
        let ps = prepare.ps;
        eprintln!(
            "app host: app data: Failed to create app data for {}, but trying to recover: {}",
            ps.name,
            message.unwrap_or_default()
        );
        let mut data = Parcel::new();
        installd::DestroyAppData {
            uuid: ps.volume_uuid.clone(),
            package_name: Some(ps.name.clone()),
            user_id: user,
            flags,
            ce_data_inode: crate::package::info::user_state(ps, user).ce_data_inode,
        }
        .write(&mut data);
        if let Err(e) = call(
            installd,
            "destroyAppData",
            installd::DESTROY_APP_DATA,
            &data,
            installd::read_destroy_app_data_reply,
        ) {
            eprintln!("app host: app data: {e}");
        }
        let mut data = Parcel::new();
        installd::CreateAppData {
            args: Some(prepare.args.clone()),
        }
        .write(&mut data);
        match call(
            installd,
            "createAppData",
            installd::CREATE_APP_DATA,
            &data,
            installd::read_create_app_data_reply::<CreateAppDataResult>,
        ) {
            Ok(Some(result)) => {
                eprintln!("app host: app data: Recovery succeeded!");
                Some(result)
            }
            Ok(None) | Err(_) => {
                eprintln!("app host: app data: Recovery failed!");
                None
            }
        }
    }

    /// `prepareAppDataContentsLeafLIF`: in CE, the native library link
    /// of a package whose primary ABI is 32-bit.
    fn prepare_app_data_contents(
        &self,
        installd: &Strong,
        ps: &PackageState,
        user: i32,
        flags: i32,
    ) -> Result<(), String> {
        let Some(pkg) = &ps.pkg else {
            return Ok(());
        };
        if flags & FLAG_STORAGE_CE == 0 {
            return Ok(());
        }
        let Some(abi) = &ps.primary_cpu_abi else {
            return Ok(());
        };
        if is_64_bit_abi(abi)? {
            return Ok(());
        }
        let Some(native) = &pkg.native_library_dir else {
            return Ok(());
        };
        let root = self.root().map_err(|e| e.message)?;
        let data = self.data().map_err(|e| e.message)?;
        if !super::packages::host_path(root, data, native)?.exists() {
            return Ok(());
        }
        let mut p = Parcel::new();
        installd::LinkNativeLibraryDirectory {
            uuid: ps.volume_uuid.clone(),
            package_name: Some(ps.name.clone()),
            native_lib_path32: Some(native.clone()),
            user_id: user,
        }
        .write(&mut p);
        if let Err(e) = call(
            installd,
            "linkNativeLibraryDirectory",
            installd::LINK_NATIVE_LIBRARY_DIRECTORY,
            &p,
            installd::read_link_native_library_directory_reply,
        ) {
            eprintln!(
                "app host: app data: Failed to link native for {}: {e}",
                ps.name
            );
        }
        Ok(())
    }

    /// `maybeMigrateAppDataLIF`: a system app's data moved to the storage
    /// its `defaultToDeviceProtectedStorage` asks for, without file-based
    /// encryption; whether it was.
    fn maybe_migrate_app_data(&self, installd: &Strong, ps: &PackageState, user: i32) -> bool {
        if !ps.is.system || self.file_encrypted() {
            return false;
        }
        let de = ps
            .pkg
            .as_ref()
            .is_some_and(|pkg| pkg.is(DEFAULT_TO_DEVICE_PROTECTED_STORAGE));
        let mut data = Parcel::new();
        installd::MigrateAppData {
            uuid: ps.volume_uuid.clone(),
            package_name: Some(ps.name.clone()),
            user_id: user,
            flags: if de { FLAG_STORAGE_DE } else { FLAG_STORAGE_CE },
        }
        .write(&mut data);
        if let Err(e) = call(
            installd,
            "migrateAppData",
            installd::MIGRATE_APP_DATA,
            &data,
            installd::read_migrate_app_data_reply,
        ) {
            eprintln!("app host: app data: Failed to migrate {}: {e}", ps.name);
        }
        true
    }

    /// `PackageSetting.setCeDataInode`, `setDeDataInode` of `user`, in
    /// the package model and its package restrictions; none written when
    /// none changed.
    fn record_inodes(&self, user: i32, inodes: Vec<(String, i64, i64)>) -> Result<(), String> {
        let mut store = self.package_store.lock().unwrap();
        let _writer = self.model_writer.lock().unwrap();
        let (before, _) = self.packages().map_err(|e| e.message)?;
        let changed: Vec<(String, i64, i64)> = inodes
            .into_iter()
            .filter(|(name, ce, de)| {
                let state = crate::package::info::user_state(&before.packages[name], user);
                (state.ce_data_inode, state.de_data_inode) != (*ce, *de)
            })
            .collect();
        if changed.is_empty() {
            return Ok(());
        }
        if store.is_none() {
            let data = self.data().map_err(|e| e.message)?;
            *store = Some(
                crate::package::owner::Store::open(data, &[user as u32])
                    .map_err(|e| format!("package restrictions: {e}"))?
                    .ok_or("package restrictions: no package state")?,
            );
        }
        let result = store
            .as_mut()
            .expect("opened")
            .commit_data_inodes(user as u32, &changed);
        if result.as_ref().is_ok() || result.as_ref().is_err_and(|e| e.committed) {
            let mut after = (*before).clone();
            for (name, ce, de) in &changed {
                if let Some(ps) = after.packages.get_mut(name) {
                    let state = ps.users.entry(user).or_default();
                    state.ce_data_inode = *ce;
                    state.de_data_inode = *de;
                }
            }
            self.user_states_changed(&before, Arc::new(after))
                .map_err(|e| e.message)?;
        }
        result.map_err(|e| format!("package restrictions: {}", e.message))
    }
}

/// `Installer.buildCreateAppDataArgs` of `prepareAppData`: the package's
/// seinfo with the user's (`SELinuxUtil.getSeinfoUser`), and the SDK
/// storage of a package that uses SDK libraries. `Installer` sends no
/// previous app id (b/221088088).
fn create_app_data_args(
    ps: &PackageState,
    user: i32,
    flags: i32,
) -> Result<CreateAppDataArgs, String> {
    let seinfo = ps
        .seinfo
        .as_ref()
        .ok_or_else(|| format!("{}: no seinfo", ps.name))?;
    let instant = crate::package::info::user_state(ps, user).instant_app;
    let seinfo_user = if instant {
        ":ephemeralapp:complete"
    } else {
        ":complete"
    };
    let sdk = if ps.uses_sdk_libraries.is_empty() {
        0
    } else {
        FLAG_STORAGE_SDK
    };
    Ok(CreateAppDataArgs {
        uuid: ps.volume_uuid.clone(),
        package_name: ps.name.clone(),
        user_id: user,
        flags: flags | sdk,
        app_id: ps.app_id,
        previous_app_id: 0,
        se_info: format!("{seinfo}{seinfo_user}"),
        target_sdk_version: ps.target_sdk_version,
    })
}

/// The inodes of `result` that `prepareAppData` keeps: CE's and DE's of
/// the storage prepared, unless -1; the package's others as they were.
fn record(
    inodes: &mut Vec<(String, i64, i64)>,
    ps: &PackageState,
    user: i32,
    flags: i32,
    result: &CreateAppDataResult,
) {
    let index = match inodes.iter().position(|(name, ..)| *name == ps.name) {
        Some(i) => i,
        None => {
            let state = crate::package::info::user_state(ps, user);
            inodes.push((ps.name.clone(), state.ce_data_inode, state.de_data_inode));
            inodes.len() - 1
        }
    };
    let entry = &mut inodes[index];
    if flags & FLAG_STORAGE_CE != 0 && result.ce_data_inode != -1 {
        entry.1 = result.ce_data_inode;
    }
    if flags & FLAG_STORAGE_DE != 0 && result.de_data_inode != -1 {
        entry.2 = result.de_data_inode;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_app_data_args_parcel_as_the_original() {
        let args = CreateAppDataArgs {
            uuid: None,
            package_name: "p".into(),
            user_id: 0,
            flags: 3,
            app_id: 10001,
            previous_app_id: 0,
            se_info: "default:complete".into(),
            target_sdk_version: 36,
        };
        let mut p = Parcel::new();
        args.write_to(&mut p);
        let mut r = p.reader();
        let size = r.read_i32().unwrap();
        assert_eq!(size as usize, p.position());
        assert_eq!(r.read_string16().unwrap(), None);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("p"));
        assert_eq!([(); 4].map(|_| r.read_i32().unwrap()), [0, 3, 10001, 0]);
        assert_eq!(
            r.read_string16().unwrap().as_deref(),
            Some("default:complete")
        );
        assert_eq!(r.read_i32().unwrap(), 36);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn a_result_reads_the_fields_its_size_covers() {
        let mut p = Parcel::new();
        p.write_i32(4 + 8 + 8 + 4 + 4);
        p.write_i64(7);
        p.write_i64(9);
        p.write_i32(0);
        p.write_string16(None);
        p.write_i32(42);
        let mut r = p.reader();
        let result = CreateAppDataResult::read_from(&mut r).unwrap();
        assert_eq!(
            result,
            CreateAppDataResult {
                ce_data_inode: 7,
                de_data_inode: 9,
                exception_code: 0,
                exception_message: None,
            }
        );
        assert_eq!(r.read_i32().unwrap(), 42);
        // An older writer's shorter parcelable leaves the rest default.
        let mut p = Parcel::new();
        p.write_i32(4 + 8);
        p.write_i64(5);
        let result = CreateAppDataResult::read_from(&mut p.reader()).unwrap();
        assert_eq!((result.ce_data_inode, result.de_data_inode), (5, 0));
    }

    #[test]
    fn the_fingerprint_digests_the_partitions_as_digest_of() {
        let props = |key: &str| (key == "ro.build.fingerprint").then(|| "f".to_string());
        let mut expected = Sha1::new();
        for key in [
            "ro.build.fingerprint",
            "ro.odm.build.fingerprint",
            "ro.oem.build.fingerprint",
            "ro.product.build.fingerprint",
            "ro.system.build.fingerprint",
            "ro.system_ext.build.fingerprint",
            "ro.vendor.build.fingerprint",
        ] {
            let value = if key == "ro.build.fingerprint" {
                "f"
            } else {
                ""
            };
            expected.update(format!("{key}={value}\n").as_bytes());
        }
        let expected: String = expected
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(fingerprint(props), expected);
        assert_eq!(expected.len(), 40);
    }

    #[test]
    fn abis_are_32_or_64_bit() {
        assert!(is_64_bit_abi("arm64-v8a").unwrap());
        assert!(!is_64_bit_abi("armeabi-v7a").unwrap());
        assert!(is_64_bit_abi("mips").is_err());
    }
}
