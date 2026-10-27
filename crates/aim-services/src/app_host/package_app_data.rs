//! `IPackageAppDataBridge` for the app host: the native installer's
//! app-data owner, which system_server's `NativePackageAppDataBridge`
//! serves (`android-16.0.0_r1`), to the system uid only. A user's storage
//! flags follow prepared CE/DE storage. Only directories created by this
//! invocation acquire rollback claims; existing data is never claimed.
//! Port of NativePackageAppDataBridge's installd receipt protocol.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::{
    collections::BTreeMap,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_ipackageappdatabridge as bridge;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;
/// `StorageManager.FLAG_STORAGE_DE`, `FLAG_STORAGE_CE`.
const FLAG_STORAGE_DE: i32 = 1;
const FLAG_STORAGE_CE: i32 = 2;
/// The one user the app host runs (`isUserRunning`).
const SYSTEM_USER: i32 = 0;

pub struct PackageAppData {
    host: Arc<AppHost>,
    claims: Mutex<BTreeMap<(String, i32, i64), Claim>>,
}

#[derive(Clone, Debug, PartialEq)]
struct Directory {
    device: u64,
    inode: i64,
    uid: u32,
}
// Internal directory arrays are DE, CE; the bridge wire receipt is CE, DE.
struct Claim {
    volume: Option<String>,
    paths: [PathBuf; 2],
    created: [Option<Directory>; 2],
}

fn failure(message: impl Into<String>) -> Exception {
    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message)
}
fn segment(value: &str) -> bool {
    !value.is_empty() && !matches!(value, "." | "..") && !value.contains(['/', '\\', '\0'])
}
fn directory(path: &Path) -> Result<Option<Directory>, Exception> {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(failure(error.to_string())),
    };
    if !metadata.is_dir() {
        return Err(Exception::security("App data is not an owned directory"));
    }
    let uid = aim_storage::guest_inode::read(path)
        .map_err(|error| failure(error.to_string()))?
        .and_then(|inode| inode.uid)
        .ok_or_else(|| failure("App data guest UID unavailable"))?;
    Ok(Some(Directory {
        device: metadata.dev(),
        inode: metadata.ino() as i64,
        uid,
    }))
}

/// Never destroy an existing directory or one that another owner replaced.
fn owned_mask(paths: &[PathBuf; 2], created: &[Option<Directory>; 2]) -> Result<i32, Exception> {
    let mut flags = 0;
    for (index, expected) in created.iter().enumerate() {
        if let Some(expected) = expected {
            if directory(&paths[index])?.as_ref() == Some(expected) {
                flags |= 1 << index;
            }
        }
    }
    Ok(flags)
}

fn create_claim(
    paths: [PathBuf; 2],
    volume: Option<String>,
    uid: u32,
    flags: i32,
    create: impl FnOnce() -> Result<[i64; 2], Exception>,
    destroy: impl Fn(i32, i64) -> Result<(), Exception>,
) -> Result<([i64; 2], Claim), Exception> {
    let requested = flags & 3;
    let mut before = [None, None];
    for index in 0..2 {
        if requested & (1 << index) != 0 {
            before[index] = directory(&paths[index])?;
        }
    }
    let read_created = || -> Result<[Option<Directory>; 2], Exception> {
        let mut created = [None, None];
        for index in 0..2 {
            if requested & (1 << index) != 0 && before[index].is_none() {
                created[index] = directory(&paths[index])?.filter(|current| current.uid == uid);
            }
        }
        Ok(created)
    };
    let result = create().and_then(|inodes| {
        for index in 0..2 {
            if requested & (1 << index) != 0 {
                let after = directory(&paths[index])?
                    .ok_or_else(|| failure("installd app-data directory absent"))?;
                if after.inode != inodes[index] || after.uid != uid {
                    return Err(failure(
                        "installd app-data inode or UID differs from directory owner",
                    ));
                }
            }
        }
        Ok(inodes)
    });
    let created = read_created()?;
    match result {
        Ok(inodes) => Ok((
            inodes,
            Claim {
                volume,
                paths,
                created,
            },
        )),
        Err(mut error) => {
            let mask = owned_mask(&paths, &created)?;
            if mask != 0 {
                let ce = created[1].as_ref().map_or(-1, |directory| directory.inode);
                if let Err(cleanup) = destroy(mask, ce) {
                    error
                        .message
                        .push_str(&format!("; app-data cleanup failed: {}", cleanup.message));
                }
            }
            Err(error)
        }
    }
}

impl PackageAppData {
    pub fn new(host: Arc<AppHost>) -> Self {
        PackageAppData {
            host,
            claims: Mutex::new(BTreeMap::new()),
        }
    }

    fn create(&self, args: bridge::CreateAppData) -> Result<Vec<u8>, Exception> {
        use super::{
            app_data::CreateAppDataArgs, package_internal_storage::PackageInternalStorage,
        };
        let name = args
            .package_name
            .ok_or_else(|| Exception::illegal_argument("Invalid app-data identity"))?;
        if !segment(&name)
            || args.user_id < 0
            || !(0..100_000).contains(&args.app_id)
            || args
                .volume_uuid
                .as_deref()
                .is_some_and(|volume| !segment(volume))
        {
            return Err(Exception::illegal_argument("Invalid app-data identity"));
        }
        let seinfo = args
            .se_info
            .ok_or_else(|| Exception::illegal_argument("Missing app-data seInfo"))?;
        let uid = args
            .user_id
            .checked_mul(100_000)
            .and_then(|id| id.checked_add(args.app_id))
            .ok_or_else(|| Exception::illegal_argument("App-data UID overflow"))?
            as u32;
        if args.flags & 3 & !self.flags(args.user_id)? != 0 {
            return Err(failure("Requested app-data storage is not prepared"));
        }
        let storage = PackageInternalStorage::new(self.host.clone());
        let paths = [
            storage
                .user_directory(args.volume_uuid.as_deref(), args.user_id, false)?
                .join(&name),
            storage
                .user_directory(args.volume_uuid.as_deref(), args.user_id, true)?
                .join(&name),
        ];
        let mut claims = self.claims.lock().unwrap();
        if claims
            .keys()
            .any(|(package, user, _)| package == &name && *user == args.user_id)
        {
            return Err(failure("App-data rollback claim already owned"));
        }
        let request = CreateAppDataArgs {
            uuid: args.volume_uuid.clone(),
            package_name: name.clone(),
            user_id: args.user_id,
            flags: args.flags,
            app_id: args.app_id,
            previous_app_id: 0,
            se_info: seinfo,
            target_sdk_version: args.target_sdk_version,
        };
        let (inodes, claim) = create_claim(
            paths,
            args.volume_uuid.clone(),
            uid,
            args.flags,
            || storage.create(&request).map(|[ce, de]| [de, ce]),
            |mask, ce| {
                storage
                    .destroy(
                        args.volume_uuid.clone(),
                        Some(name.clone()),
                        args.user_id,
                        mask,
                        ce,
                    )
                    .map(|_| ())
            },
        )?;
        let newly_created = claim.created.iter().any(Option::is_some);
        if newly_created {
            claims.insert((name, args.user_id, inodes[1]), claim);
        }
        let mut result = Parcel::new();
        result.write_i64(inodes[1]);
        result.write_i64(inodes[0]);
        result.write_bool(newly_created);
        Ok(result.data().to_vec())
    }
    fn finish(
        &self,
        name: Option<String>,
        user: i32,
        ce: i64,
        committed: bool,
    ) -> Result<(), Exception> {
        let name = name.ok_or_else(|| Exception::illegal_argument("Missing app-data identity"))?;
        let key = (name.clone(), user, ce);
        let mut claims = self.claims.lock().unwrap();
        if !committed {
            if let Some(claim) = claims.get(&key) {
                let mask = owned_mask(&claim.paths, &claim.created)?;
                if mask != 0 {
                    super::package_internal_storage::PackageInternalStorage::new(self.host.clone())
                        .destroy(claim.volume.clone(), Some(name), user, mask, ce)?;
                }
            }
        }
        claims.remove(&key);
        Ok(())
    }

    /// `appDataFlags`.
    fn flags(&self, user: i32) -> Result<i32, Exception> {
        if super::user_service::serial_number(&self.host, user)?.is_none() {
            return Err(Exception::illegal_argument(format!("Unknown user {user}")));
        }
        Ok(if self.host.ce_storage_prepared(user) {
            FLAG_STORAGE_DE | FLAG_STORAGE_CE
        } else if user == SYSTEM_USER {
            FLAG_STORAGE_DE
        } else {
            0
        })
    }
}

impl Service for PackageAppData {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // `owner`.
        let system = call.sender_euid == SYSTEM_UID;
        let caller = || {
            if system {
                Ok(())
            } else {
                Err(Exception::security("Native app-data owner required"))
            }
        };
        match call.code {
            bridge::APP_DATA_FLAGS => {
                let a = bridge::AppDataFlags::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().and_then(|()| self.flags(a.user_id)).map(|flags| {
                    let mut p = Parcel::new();
                    bridge::write_app_data_flags_reply(&mut p, flags);
                    p
                }))
            }
            bridge::CREATE_APP_DATA => {
                let args = bridge::CreateAppData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().and_then(|()| self.create(args)).map(|bytes| {
                    let mut p = Parcel::new();
                    bridge::write_create_app_data_reply(&mut p, &Some(bytes));
                    p
                }))
            }
            bridge::COMMIT_APP_DATA => {
                let args = bridge::CommitAppData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    caller()
                        .and_then(|()| {
                            self.finish(args.package_name, args.user_id, args.ce_data_inode, true)
                        })
                        .map(|()| {
                            let mut p = Parcel::new();
                            bridge::write_commit_app_data_reply(&mut p);
                            p
                        }),
                )
            }
            bridge::ROLLBACK_APP_DATA => {
                let args = bridge::RollbackAppData::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    caller()
                        .and_then(|()| {
                            self.finish(args.package_name, args.user_id, args.ce_data_inode, false)
                        })
                        .map(|()| {
                            let mut p = Parcel::new();
                            bridge::write_rollback_app_data_reply(&mut p);
                            p
                        }),
                )
            }
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::EX_SECURITY;

    struct Data(PathBuf);
    impl Data {
        fn new() -> Self {
            let path = crate::app_host::test_temp_dir("install-app-data");
            Self(path)
        }
        fn paths(&self) -> [PathBuf; 2] {
            [self.0.join("de"), self.0.join("ce")]
        }
    }
    impl Drop for Data {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn make(path: &Path, uid: u32) -> i64 {
        if !path.exists() {
            std::fs::create_dir(path).unwrap();
        }
        aim_storage::guest_inode::record(
            path,
            aim_storage::guest_inode::GuestInode {
                uid: Some(uid),
                gid: Some(uid),
                mode: Some(0o700),
            },
        )
        .unwrap();
        path.symlink_metadata().unwrap().ino() as i64
    }
    #[test]
    fn installd_receipts_claim_only_new_data_and_keep_existing_sentinels() {
        let data = Data::new();
        let paths = data.paths();
        let de = make(&paths[0], 10100);
        std::fs::write(paths[0].join("sentinel"), b"retained").unwrap();
        let (inodes, claim) = create_claim(
            paths.clone(),
            None,
            10100,
            3,
            || Ok([make(&paths[0], 10100), make(&paths[1], 10100)]),
            |_, _| panic!("successful creation cannot roll back"),
        )
        .unwrap();
        assert_eq!(inodes[0], de);
        assert!(claim.created[0].is_none());
        assert_eq!(
            owned_mask(&claim.paths, &claim.created).unwrap(),
            FLAG_STORAGE_CE
        );
        assert_eq!(
            std::fs::read(paths[0].join("sentinel")).unwrap(),
            b"retained"
        );
        let (_, retained) = create_claim(
            paths.clone(),
            None,
            10100,
            3,
            || Ok(inodes),
            |_, _| panic!("existing data cannot roll back"),
        )
        .unwrap();
        assert!(retained.created.iter().all(Option::is_none));
    }
    #[test]
    fn partial_installd_failure_cleans_only_its_new_owned_directory() {
        let data = Data::new();
        let paths = data.paths();
        make(&paths[0], 10100);
        std::fs::write(paths[0].join("sentinel"), b"keep").unwrap();
        let calls = Mutex::new(Vec::new());
        let result = create_claim(
            paths.clone(),
            None,
            10100,
            3,
            || {
                make(&paths[1], 10100);
                Err(failure("installd partial failure"))
            },
            |mask, ce| {
                calls.lock().unwrap().push((mask, ce));
                std::fs::remove_dir_all(&paths[1]).unwrap();
                Ok(())
            },
        );
        assert_eq!(result.err().unwrap().message, "installd partial failure");
        assert_eq!(calls.lock().unwrap()[0].0, FLAG_STORAGE_CE);
        assert!(!paths[1].exists());
        assert_eq!(std::fs::read(paths[0].join("sentinel")).unwrap(), b"keep");
    }
    #[test]
    fn rollback_does_not_take_a_replaced_inode_or_changed_guest_uid() {
        let data = Data::new();
        let paths = data.paths();
        let (_, claim) = create_claim(
            paths.clone(),
            None,
            10100,
            3,
            || Ok([make(&paths[0], 10100), make(&paths[1], 10100)]),
            |_, _| panic!("successful creation cannot roll back"),
        )
        .unwrap();
        std::fs::rename(&paths[0], data.0.join("old-de")).unwrap();
        make(&paths[0], 10100);
        make(&paths[1], 10101);
        assert_eq!(owned_mask(&claim.paths, &claim.created).unwrap(), 0);
    }
    #[test]
    fn bad_installd_receipt_fails_and_cleanup_failure_is_reported() {
        let data = Data::new();
        let paths = data.paths();
        let result = create_claim(
            paths.clone(),
            None,
            10100,
            3,
            || {
                make(&paths[0], 10100);
                make(&paths[1], 10100);
                Ok([-1, -1])
            },
            |mask, _| {
                assert_eq!(mask, 3);
                Err(failure("destroy failed"))
            },
        );
        let error = result.err().unwrap();
        assert!(error.message.contains("inode or UID differs"));
        assert!(error.message.contains("cleanup failed: destroy failed"));
    }
    #[test]
    fn app_data_path_segments_and_symlink_owners_are_rejected() {
        for name in ["", ".", "..", "a/b", "a\\b", "a\0b"] {
            assert!(!segment(name));
        }
        assert!(segment("dev.aim.installprobe"));
        let data = Data::new();
        let paths = data.paths();
        make(&paths[0], 10100);
        std::os::unix::fs::symlink(&paths[0], &paths[1]).unwrap();
        assert_eq!(
            directory(&paths[1]).unwrap_err().code,
            aim_binder_host::parcel::EX_SECURITY
        );
    }

    /// System-only idempotent release of a transaction without a creation claim.
    #[test]
    fn app_data_is_the_systems() {
        let node = |euid: u32| {
            let process = crate::test_process(euid);
            let host = AppHost::new(process.clone());
            let aim_binder_host::parcel::Binder::Local(ptr) =
                process.add_service(Arc::new(PackageAppData::new(host)))
            else {
                unreachable!()
            };
            process.local_service(ptr).unwrap()
        };
        let commit = |euid: u32| {
            let mut p = Parcel::new();
            bridge::CommitAppData {
                package_name: Some("a.b".into()),
                user_id: 0,
                ce_data_inode: 1,
            }
            .write(&mut p);
            let reply = node(euid)
                .transact(bridge::COMMIT_APP_DATA, &p, false)
                .unwrap();
            reply
                .reader()
                .read_exception()
                .unwrap()
                .map_err(|error| error.code)
        };
        assert_eq!(commit(10000), Err(EX_SECURITY));
        assert_eq!(commit(SYSTEM_UID), Ok(()));
    }
}
