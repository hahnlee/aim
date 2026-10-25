//! User storage preparation and installd primitives at `android-16.0.0_r1`.
//! The app host retains `UserDataPreparer`'s storage/serial-number work;
//! the native package session coordinates package data and inode receipts.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::android_os_iinstalld as installd;
use aim_service_aidl::{ReadParcelable, Returned, WriteParcelable};
use sha1::{Digest, Sha1};

use super::AppHost;
use super::storage::VOLD_DESCRIPTOR;
use crate::package::model::PackageState;

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
pub(super) const FLAG_STORAGE_SDK: i32 = 8;

/// `android.os.CreateAppDataArgs`.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct CreateAppDataArgs {
    pub(super) uuid: Option<String>,
    pub(super) package_name: String,
    pub(super) user_id: i32,
    pub(super) flags: i32,
    pub(super) app_id: i32,
    pub(super) previous_app_id: i32,
    pub(super) se_info: String,
    pub(super) target_sdk_version: i32,
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
pub(super) struct CreateAppDataResult {
    pub(super) ce_data_inode: i64,
    pub(super) de_data_inode: i64,
    pub(super) exception_code: i32,
    pub(super) exception_message: Option<String>,
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
pub(super) fn is_64_bit_abi(abi: &str) -> Result<bool, String> {
    match abi {
        "arm64-v8a" | "x86_64" | "riscv64" => Ok(true),
        "armeabi" | "armeabi-v7a" | "x86" => Ok(false),
        _ => Err(format!("Unsupported ABI: {abi}")),
    }
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
pub(super) fn delete_contents(dir: &Path, itself: bool) {
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
    pub(super) fn file_encrypted(&self) -> bool {
        self.property("ro.crypto.type").as_deref() == Some("file")
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
            return Err(format!("Unknown storage user {user}"));
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

    /// `create_data_user_ce_path` on the host: user 0's is `/data/data`
    /// where that is a directory (`/data/user/0` binds it in the guest).
    pub(super) fn ce_dir(&self, root: &Path, user: i32) -> PathBuf {
        let data = root.join("data");
        if user == 0 && data.symlink_metadata().is_ok_and(|m| m.is_dir()) {
            return data;
        }
        root.join(format!("user/{user}"))
    }

    /// `AppDataHelper.clearAppDataLeafLIF` of `ps` in `user`: installd's
    /// `clearAppData` of its storage `flags`, a failure logged as the
    /// original logs it. Its ART profiles are ART Service's
    /// (`destroyAppProfilesLIF`), which the app host does not start, so
    /// they stay, as the original leaves them before ART Service runs.
    pub(super) fn clear_app_data(
        &self,
        installd: &Strong,
        ps: &PackageState,
        user: i32,
        flags: i32,
    ) {
        let mut data = Parcel::new();
        installd::ClearAppData {
            uuid: ps.volume_uuid.clone(),
            package_name: Some(ps.name.clone()),
            user_id: user,
            flags,
            ce_data_inode: crate::package::info::user_state(ps, user).ce_data_inode,
        }
        .write(&mut data);
        if let Err(e) = call(
            installd,
            "clearAppData",
            installd::CLEAR_APP_DATA,
            &data,
            installd::read_clear_app_data_reply,
        ) {
            eprintln!("app host: app data: {e}");
        }
    }

    /// `prepareAppDataContentsLeafLIF`: in CE, the native library link
    /// of a package whose primary ABI is 32-bit.
    pub(super) fn prepare_app_data_contents(
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
