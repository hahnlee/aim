//! `IPackageInternalStorageBridge` for the app host: the native package
//! boot's internal storage, which system_server's
//! `NativePackageInternalStorageBridge` serves, to the system uid only.
//! The writable private volumes, a user's CE unlock, file encryption, the
//! device-protected default and a user's data directory names are
//! answered, and installd's operations on app data made as the original's
//! `Installer` makes them, a failure its `InstallerException`; a package's
//! dexopt artifacts deleted need ART Service (#2063).

use std::path::PathBuf;
use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_SERVICE_SPECIFIC, Exception, Parcel,
    UNKNOWN_TRANSACTION,
};
use aim_service_aidl::android_os_iinstalld as installd;
use aim_service_aidl::dev_aim_server_ipackageinternalstoragebridge as bridge;

use super::app_data::{CreateAppDataArgs, CreateAppDataResult, FLAG_STORAGE_CE, FLAG_STORAGE_SDK};
use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;
/// `PackageManager.APPLY_DEFAULT_TO_DEVICE_PROTECTED_STORAGE`.
const APPLY_DEFAULT_TO_DEVICE_PROTECTED_STORAGE: bool = true;
/// `Installer.FLAG_FREE_CACHE_V2`, `FLAG_FREE_CACHE_DEFY_TARGET_FREE_BYTES`.
const FLAG_FREE_CACHE_V2: i32 = 0x100;
const FLAG_FREE_CACHE_DEFY_TARGET_FREE_BYTES: i32 = 0x800;

/// `new ParcelableException(InstallerException.from(e))`: the cause's
/// `toString`, the InstallerException's message.
fn installer_exception(e: &Exception) -> Exception {
    let cause = if e.code == EX_SERVICE_SPECIFIC {
        format!(
            "android.os.ServiceSpecificException: {} (code {})",
            e.message, e.service_specific
        )
    } else {
        e.message.clone()
    };
    let class = "com.android.server.pm.Installer$InstallerException";
    let mut p = Parcel::new();
    p.write_string16(Some("android.os.ParcelableException"));
    p.write_string16(Some(class));
    p.write_string16(Some(&cause));
    Exception::parcelable(Some(&format!("{class}: {cause}")), &p).expect("a message has no objects")
}

pub struct PackageInternalStorage {
    host: Arc<AppHost>,
}

impl PackageInternalStorage {
    pub fn new(host: Arc<AppHost>) -> Self {
        PackageInternalStorage { host }
    }

    /// `Environment.getDataUserCeDirectory` or `getDataUserDeDirectory` of
    /// `volume` on the host.
    fn user_directory(
        &self,
        volume: Option<&str>,
        user: i32,
        ce: bool,
    ) -> Result<PathBuf, Exception> {
        let data = self.host.data()?;
        let root = match volume {
            None => data.to_path_buf(),
            Some(uuid) => self.host.root()?.join(format!("mnt/expand/{uuid}")),
        };
        Ok(if ce {
            self.host.ce_dir(&root, user)
        } else {
            root.join(format!("user_de/{user}"))
        })
    }

    /// A call `code` to installd, its reply once its status is read; a
    /// failure the `Installer`'s.
    fn call(
        &self,
        code: u32,
        data: &Parcel,
    ) -> Result<aim_binder_host::local::Received, Exception> {
        let fail =
            |message: String| installer_exception(&Exception::new(EX_ILLEGAL_STATE, message));
        let service = self.host.installd().map_err(fail)?;
        let reply = service
            .transact(code, data, false)
            .map_err(|s| fail(format!("installd: {s}")))?;
        match reply.reader().read_exception() {
            Ok(Ok(())) => Ok(reply),
            Ok(Err(e)) => Err(installer_exception(&e)),
            Err(s) => Err(fail(format!("installd reply: {s}"))),
        }
    }

    /// A void call's empty reply.
    fn void(&self, code: u32, data: &Parcel) -> Result<Parcel, Exception> {
        self.call(code, data)?;
        let mut p = Parcel::new();
        p.write_no_exception();
        Ok(p)
    }

    fn destroy(
        &self,
        volume: Option<String>,
        name: Option<String>,
        user: i32,
        flags: i32,
        ce_inode: i64,
    ) -> Result<Parcel, Exception> {
        let mut data = Parcel::new();
        installd::DestroyAppData {
            uuid: volume,
            package_name: name,
            user_id: user,
            flags,
            ce_data_inode: ce_inode,
        }
        .write(&mut data);
        self.void(installd::DESTROY_APP_DATA, &data)
    }

    fn create(&self, args: &CreateAppDataArgs) -> Result<[i64; 2], Exception> {
        let mut data = Parcel::new();
        installd::CreateAppData {
            args: Some(args.clone()),
        }
        .write(&mut data);
        let reply = self.call(installd::CREATE_APP_DATA, &data)?;
        let mut r = reply.reader();
        r.read_exception()
            .map_err(|s| installer_exception(&Exception::new(EX_ILLEGAL_STATE, s.to_string())))?
            .map_err(|e| installer_exception(&e))?;
        let fail = |message| installer_exception(&Exception::new(EX_ILLEGAL_STATE, message));
        let result = aim_service_aidl::read_typed::<CreateAppDataResult>(&mut r)
            .map_err(|s| fail(format!("createAppData result: {s}")))?
            .ok_or_else(|| fail("createAppData returned no result".into()))?;
        if result.exception_code != 0 {
            let mut error = Exception::new(
                EX_SERVICE_SPECIFIC,
                result
                    .exception_message
                    .as_deref()
                    .unwrap_or("createAppData failed"),
            );
            error.service_specific = result.exception_code;
            return Err(installer_exception(&error));
        }
        if r.remaining() != 0 {
            return Err(fail("createAppData result has trailing data".into()));
        }
        Ok([result.ce_data_inode, result.de_data_inode])
    }

    /// `prepareData`.
    fn prepare(&self, a: bridge::PrepareData) -> Result<[i64; 2], Exception> {
        let name = a
            .package_name
            .clone()
            .ok_or_else(|| Exception::new(EX_NULL_POINTER, "packageName"))?;
        let args = CreateAppDataArgs {
            uuid: a.volume.clone(),
            package_name: name,
            user_id: a.user,
            flags: a.flags | if a.uses_sdk { FLAG_STORAGE_SDK } else { 0 },
            app_id: a.app_id,
            // `Installer.createAppData` disables data migration.
            previous_app_id: 0,
            se_info: a.seinfo.unwrap_or_default(),
            target_sdk_version: a.target_sdk,
        };
        self.create(&args).or_else(|error| {
            eprintln!(
                "native package: app data: createAppData {} failed; recovering: {}",
                args.package_name, error.message
            );
            self.destroy(a.volume, a.package_name, a.user, a.flags, 0)?;
            self.create(&args)
        })
    }

    /// `prepareContents`: a 32-bit package's native library directory
    /// linked into its CE data.
    fn prepare_contents(&self, a: bridge::PrepareContents) -> Result<(), Exception> {
        let Some(abi) = a.primary_abi.as_deref() else {
            return Ok(());
        };
        if a.flags & FLAG_STORAGE_CE == 0
            || super::app_data::is_64_bit_abi(abi).map_err(Exception::illegal_argument)?
        {
            return Ok(());
        }
        let library = a.native_library_dir.ok_or_else(|| {
            Exception::new(
                EX_ILLEGAL_STATE,
                "32-bit native library directory unavailable",
            )
        })?;
        let (root, data) = (self.host.root()?, self.host.data()?);
        if !super::resources::host_path(root, data, &library).is_ok_and(|h| h.exists()) {
            return Ok(());
        }
        let mut data = Parcel::new();
        installd::LinkNativeLibraryDirectory {
            uuid: a.volume,
            package_name: a.package_name,
            native_lib_path32: Some(library),
            user_id: a.user,
        }
        .write(&mut data);
        self.call(installd::LINK_NATIVE_LIBRARY_DIRECTORY, &data)
            .map(|_| ())
    }
}

impl Service for PackageInternalStorage {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(name) = aim_service_aidl::method_name(bridge::DESCRIPTOR, call.code) else {
            return Err(UNKNOWN_TRANSACTION);
        };
        // `enforce`.
        if call.sender_euid != SYSTEM_UID {
            return reply(Err(Exception::security("Native storage owner required")));
        }
        let mut p = Parcel::new();
        match call.code {
            // `StorageManager.getWritablePrivateVolumes`' file system UUIDs.
            bridge::WRITABLE_VOLUMES => {
                bridge::WritableVolumes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let volumes = self.host.writable_private_volume_uuids();
                bridge::write_writable_volumes_reply(&mut p, &Some(volumes));
                Ok(p)
            }
            // `StorageManager.isCeStorageUnlocked`.
            bridge::CE_UNLOCKED => {
                let a = bridge::CeUnlocked::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                bridge::write_ce_unlocked_reply(&mut p, self.host.ce_storage_prepared(a.user));
                Ok(p)
            }
            bridge::FILE_ENCRYPTED => {
                bridge::FileEncrypted::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                bridge::write_file_encrypted_reply(&mut p, self.host.file_encrypted());
                Ok(p)
            }
            bridge::APPLY_DEFAULT_DEVICE_STORAGE => {
                bridge::ApplyDefaultDeviceStorage::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                bridge::write_apply_default_device_storage_reply(
                    &mut p,
                    APPLY_DEFAULT_TO_DEVICE_PROTECTED_STORAGE,
                );
                Ok(p)
            }
            // `FileUtils.listFilesOrEmpty` of the user's directory.
            bridge::DATA_DIRECTORY_NAMES => {
                let a = bridge::DataDirectoryNames::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let dir = self.user_directory(a.volume.as_deref(), a.user, a.ce)?;
                    let entries = match std::fs::read_dir(dir) {
                        Ok(entries) => Some(entries),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                        Err(error) => return Err(super::storage::io_exception(&error.to_string())),
                    };
                    let names = entries
                        .into_iter()
                        .flatten()
                        .map(|entry| {
                            entry.map(|e| Some(e.file_name().to_string_lossy().into_owned()))
                        })
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| super::storage::io_exception(&error.to_string()))?;
                    bridge::write_data_directory_names_reply(&mut p, &Some(names));
                    Ok(p)
                })())
            }
            bridge::FREE_ALL_ABOVE_QUOTA => {
                let a = bridge::FreeAllAboveQuota::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut data = Parcel::new();
                installd::FreeCache {
                    uuid: a.volume,
                    target_free_bytes: i64::MAX,
                    flags: FLAG_FREE_CACHE_V2 | FLAG_FREE_CACHE_DEFY_TARGET_FREE_BYTES,
                }
                .write(&mut data);
                reply(self.void(installd::FREE_CACHE, &data))
            }
            bridge::MIGRATE_OBB => {
                bridge::MigrateObb::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut data = Parcel::new();
                installd::MigrateLegacyObbData {}.write(&mut data);
                reply(self.void(installd::MIGRATE_LEGACY_OBB_DATA, &data))
            }
            bridge::CLEANUP_INVALID => {
                let a = bridge::CleanupInvalid::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut data = Parcel::new();
                installd::CleanupInvalidPackageDirs {
                    uuid: a.volume,
                    user_id: a.user,
                    flags: a.flags,
                }
                .write(&mut data);
                reply(self.void(installd::CLEANUP_INVALID_PACKAGE_DIRS, &data))
            }
            bridge::DESTROY_DATA => {
                let a = bridge::DestroyData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.destroy(a.volume, a.package_name, a.user, a.flags, a.ce_inode))
            }
            // `Installer.buildCreateAppDataArgs`, created, else the data
            // destroyed and created once more.
            bridge::PREPARE_DATA => {
                let a = bridge::PrepareData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.prepare(a).map(|inodes| {
                    bridge::write_prepare_data_reply(&mut p, &Some(inodes.to_vec()));
                    p
                }))
            }
            bridge::MIGRATE_DATA => {
                let a = bridge::MigrateData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut data = Parcel::new();
                installd::MigrateAppData {
                    uuid: a.volume,
                    package_name: a.package_name,
                    user_id: a.user,
                    flags: a.target,
                }
                .write(&mut data);
                reply(self.void(installd::MIGRATE_APP_DATA, &data))
            }
            bridge::PREPARE_CONTENTS => {
                let a = bridge::PrepareContents::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.prepare_contents(a).map(|()| {
                    bridge::write_prepare_contents_reply(&mut p);
                    p
                }))
            }
            bridge::FIXUP_DATA => {
                let a = bridge::FixupData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut data = Parcel::new();
                installd::FixupAppData {
                    uuid: a.volume,
                    flags: a.flags,
                }
                .write(&mut data);
                reply(self.void(installd::FIXUP_APP_DATA, &data))
            }
            _ => reply(Err(self.host.unimplemented_named(
                "package internal storage",
                &format!("{name} (#2063)"),
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{EX_SECURITY, EX_UNSUPPORTED_OPERATION};

    /// Only the system uid is served; the device-protected default is the
    /// release's, and deleting dexopt artifacts names ART's issue.
    #[test]
    fn internal_storage_is_the_systems() {
        let node = |euid: u32| {
            let process = aim_binder_host::local::LocalProcess::open(
                &aim_binder_driver::Driver::new(),
                aim_binder_driver::Device::Binder,
                aim_binder_driver::Credentials {
                    pid: std::process::id() as i32,
                    euid,
                    security_context: None,
                },
            );
            let host = AppHost::new(process.clone());
            let aim_binder_host::parcel::Binder::Local(ptr) =
                process.add_service(Arc::new(PackageInternalStorage::new(host)))
            else {
                unreachable!()
            };
            process.local_service(ptr).unwrap()
        };
        let mut default = Parcel::new();
        bridge::ApplyDefaultDeviceStorage {}.write(&mut default);
        let applies = |euid: u32| {
            let reply = node(euid)
                .transact(bridge::APPLY_DEFAULT_DEVICE_STORAGE, &default, false)
                .unwrap();
            bridge::read_apply_default_device_storage_reply(&mut reply.reader()).unwrap()
        };
        assert!(applies(SYSTEM_UID).unwrap());
        assert_eq!(applies(10000).unwrap_err().code, EX_SECURITY);
        let mut oat = Parcel::new();
        bridge::DeleteOat {
            package_name: Some("a.b".into()),
            caller_uid: 1000,
        }
        .write(&mut oat);
        let reply = node(SYSTEM_UID)
            .transact(bridge::DELETE_OAT, &oat, false)
            .unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            EX_UNSUPPORTED_OPERATION
        );
    }
}
