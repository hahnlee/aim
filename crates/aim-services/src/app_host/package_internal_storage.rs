//! `IPackageInternalStorageBridge` for the app host: the native package
//! boot's internal storage, which system_server's
//! `NativePackageInternalStorageBridge` serves, to the system uid only.
//! The writable private volumes, a user's CE unlock, file encryption, the
//! device-protected default and a user's data directory names are
//! answered; installd's and ART's operations on app data are not
//! implemented (#2166).

use std::path::PathBuf;
use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_ipackageinternalstoragebridge as bridge;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;
/// `PackageManager.APPLY_DEFAULT_TO_DEVICE_PROTECTED_STORAGE`.
const APPLY_DEFAULT_TO_DEVICE_PROTECTED_STORAGE: bool = true;

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
                reply(
                    self.host
                        .user_unlocking_or_unlocked(a.user)
                        .map(|unlocked| {
                            bridge::write_ce_unlocked_reply(&mut p, unlocked);
                            p
                        }),
                )
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
                reply(
                    self.user_directory(a.volume.as_deref(), a.user, a.ce)
                        .map(|dir| {
                            let names = std::fs::read_dir(dir)
                                .into_iter()
                                .flatten()
                                .flatten()
                                .map(|e| Some(e.file_name().to_string_lossy().into_owned()))
                                .collect::<Vec<_>>();
                            bridge::write_data_directory_names_reply(&mut p, &Some(names));
                            p
                        }),
                )
            }
            _ => reply(Err(self.host.unimplemented_named(
                "package internal storage",
                &format!("{name} (#2166)"),
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{EX_SECURITY, EX_UNSUPPORTED_OPERATION};

    /// Only the system uid is served; the device-protected default is the
    /// release's, and installd's operations name their issue.
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
        let mut obb = Parcel::new();
        bridge::MigrateObb {}.write(&mut obb);
        let reply = node(SYSTEM_UID)
            .transact(bridge::MIGRATE_OBB, &obb, false)
            .unwrap();
        assert_eq!(
            reply.reader().read_exception().unwrap().unwrap_err().code,
            EX_UNSUPPORTED_OPERATION
        );
    }
}
