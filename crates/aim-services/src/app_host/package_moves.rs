//! The native package move's storage leaves for the app host, to the
//! system uid only: `IPackageRelocationBridge`, as system_server's
//! `PackageRelocationBridge` serves it, answers whether storage is
//! file-encrypted, a user's CE storage unlocked and the users; a move
//! between volumes needs vold's disks (#1839), as does
//! `IPackageMoveBridge`'s primary storage move.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_ipackagemovebridge as moves;
use aim_service_aidl::dev_aim_server_ipackagerelocationbridge as relocation;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;

pub struct PackageRelocation {
    host: Arc<AppHost>,
}

impl PackageRelocation {
    pub fn new(host: Arc<AppHost>) -> Self {
        PackageRelocation { host }
    }
}

impl Service for PackageRelocation {
    fn descriptor(&self) -> &str {
        relocation::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let Some(name) = aim_service_aidl::method_name(relocation::DESCRIPTOR, call.code) else {
            return Err(UNKNOWN_TRANSACTION);
        };
        // `enforce`.
        if call.sender_euid != SYSTEM_UID {
            return reply(Err(Exception::security("Native relocation owner required")));
        }
        let mut p = Parcel::new();
        match call.code {
            // `StorageManager.isFileEncrypted`.
            relocation::IS_FILE_ENCRYPTED => {
                relocation::IsFileEncrypted::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                relocation::write_is_file_encrypted_reply(&mut p, self.host.file_encrypted());
                Ok(p)
            }
            // `StorageManager.isCeStorageUnlocked`.
            relocation::IS_USER_UNLOCKED => {
                let a = relocation::IsUserUnlocked::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.host
                        .user_unlocking_or_unlocked(a.user)
                        .map(|unlocked| {
                            relocation::write_is_user_unlocked_reply(&mut p, unlocked);
                            p
                        }),
                )
            }
            // `UserManagerInternal.getUserIds`.
            relocation::GET_USERS => {
                relocation::GetUsers::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    super::user_service::user_ids(&self.host, false).map(|users| {
                        relocation::write_get_users_reply(&mut p, &Some(users));
                        p
                    }),
                )
            }
            _ => reply(Err(self.host.unimplemented_named(
                "package relocation",
                &format!("{name} (#1839)"),
            ))),
        }
    }
}

pub struct PackageMove {
    host: Arc<AppHost>,
}

impl PackageMove {
    pub fn new(host: Arc<AppHost>) -> Self {
        PackageMove { host }
    }
}

impl Service for PackageMove {
    fn descriptor(&self) -> &str {
        moves::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != moves::MOVE_PRIMARY_STORAGE {
            return Err(UNKNOWN_TRANSACTION);
        }
        reply(Err(if call.sender_euid != SYSTEM_UID {
            Exception::security("native storage move owner required")
        } else {
            self.host
                .unimplemented_named("package move", "movePrimaryStorage (#1839)")
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{EX_SECURITY, EX_UNSUPPORTED_OPERATION};

    /// Only the system uid is served; a volume move names vold's disks.
    #[test]
    fn moves_are_the_systems() {
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
                process.add_service(Arc::new(PackageRelocation::new(host)))
            else {
                unreachable!()
            };
            process.local_service(ptr).unwrap()
        };
        let code = |euid: u32, code: u32, p: &Parcel| {
            let reply = node(euid).transact(code, p, false).unwrap();
            reply
                .reader()
                .read_exception()
                .unwrap()
                .err()
                .map(|e| e.code)
        };
        let mut unlocked = Parcel::new();
        relocation::IsUserUnlocked { user: 0 }.write(&mut unlocked);
        assert_eq!(
            code(10000, relocation::IS_USER_UNLOCKED, &unlocked),
            Some(EX_SECURITY)
        );
        assert_eq!(
            code(SYSTEM_UID, relocation::IS_USER_UNLOCKED, &unlocked),
            None
        );
        let mut usable = Parcel::new();
        relocation::UsableBytes {
            path: Some("/data".into()),
        }
        .write(&mut usable);
        assert_eq!(
            code(SYSTEM_UID, relocation::USABLE_BYTES, &usable),
            Some(EX_UNSUPPORTED_OPERATION)
        );
    }
}
