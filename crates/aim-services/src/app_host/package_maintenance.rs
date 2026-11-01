//! `IPackageMaintenanceBridge` for the app host: the native package
//! owner's permission checks, cache policy and wall time, answered as
//! system_server's `NativePackageMaintenanceBridge` answers them
//! (`android-16.0.0_r1`), to the system uid only. Its ART methods are ART
//! Service's (#2063), except dex-use recording in aim-system's original
//! DexUseManagerLocal. Package caches stay owned by installd; profile
//! cleanup and the external storage session cache remain explicit gaps.
//! Storage policy follows AOSP android-16.0.0_r1 PMS/AppDataHelper (Apache-2.0).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::android_os_iinstalld as installd;
use aim_service_aidl::dev_aim_server_ipackagemaintenancebridge as bridge;
use aim_service_aidl::{
    dev_aim_server_idexusebridge as dex, dev_aim_server_idexusepackages as packages,
};

use super::{AppHost, reply, system_boolean};

const ROOT_UID: u32 = 0;
const SYSTEM_UID: u32 = 1000;
const SHELL_UID: u32 = 2000;
/// `PackageManager.PERMISSION_GRANTED`.
const GRANTED: i32 = 0;

pub struct PackageMaintenance {
    host: Arc<AppHost>,
}

impl PackageMaintenance {
    pub fn new(host: Arc<AppHost>) -> Self {
        PackageMaintenance { host }
    }

    /// `Context.checkPermission(permission, pid, uid)`: ActivityManager's
    /// check.
    fn check(&self, permission: Option<&str>, uid: i32) -> Result<bool, Exception> {
        let Some(permission) = permission else {
            return Err(Exception::illegal_argument("permission is null"));
        };
        Ok(self.host.check_permission(Some(permission), uid)? == GRANTED)
    }

    /// `Context.enforcePermission(permission, pid, uid, null)`.
    fn enforce(&self, permission: Option<&str>, uid: i32) -> Result<(), Exception> {
        if self.check(permission, uid)? {
            return Ok(());
        }
        Err(Exception::security(format!(
            "uid {uid} does not have {}.",
            permission.unwrap_or_default()
        )))
    }

    fn clear_cache_files(
        &self,
        name: Option<&str>,
        user: i32,
        instant: bool,
        caller: i32,
    ) -> Result<(), Exception> {
        let capture = self.host.package_capture()?;
        if cache_package(capture.state(), name, instant, caller).is_none() {
            return Ok(());
        }
        let system = self.host.package_system()?;
        let _install = system.package_install_guard();
        let capture = self.host.package_capture()?;
        let Some(package) = cache_package(capture.state(), name, instant, caller) else {
            return Ok(());
        };
        let owner = self
            .host
            .installd()
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        self.clear_cache_storage(&owner, package, user)?;
        // ART deletion forwarding is not connected yet (#2063). Do not
        // report a completed clear while its profile effect is unavailable.
        Err(self
            .host
            .unimplemented_named("package maintenance", "clearAppProfiles (#2063)"))
    }

    fn clear_cache_storage(
        &self,
        owner: &aim_binder_host::local::Strong,
        package: &crate::package::model::PackageState,
        user: i32,
    ) -> Result<(), Exception> {
        // Installd owns CE inode resolution (including locked CE), DE,
        // external cache and SDK sandbox caches. External has no code cache.
        for cache in [0x10, 0x20] {
            self.host
                .clear_app_data(owner, package, user, 1 | 2 | 4 | cache)
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        }
        Ok(())
    }
}

fn cache_package<'a>(
    state: &'a crate::package::model::State,
    name: Option<&str>,
    instant: bool,
    caller: i32,
) -> Option<&'a crate::package::model::PackageState> {
    let package = state.packages.get(name?)?;
    if package.pkg.is_none()
        || crate::package::info::user_state(package, caller / 100_000).instant_app && !instant
    {
        return None;
    }
    Some(package)
}

impl Service for PackageMaintenance {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code == bridge::HANDLE_ART_SHELL_COMMAND {
            return reply(Err(
                if call.sender_euid != ROOT_UID && call.sender_euid != SHELL_UID {
                    Exception::security("ART service shell commands need root or shell access")
                } else {
                    self.host
                        .unimplemented_named("package maintenance", "handleArtShellCommand (#2063)")
                },
            ));
        }
        if call.sender_euid != SYSTEM_UID {
            return reply(Err(Exception::security(
                "Native package maintenance owner required",
            )));
        }
        let host = &self.host;
        let mut p = Parcel::new();
        let result = match call.code {
            bridge::CLEAR_CACHE_FILES => {
                let a = bridge::ClearCacheFiles::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.clear_cache_files(
                    a.package_name.as_deref(),
                    a.user_id,
                    a.can_access_instant_apps,
                    a.calling_uid,
                )
                .map(|()| bridge::write_clear_cache_files_reply(&mut p))
            }
            bridge::STORAGE_SPACE => {
                let a = bridge::StorageSpace::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut record = Parcel::new();
                match AppHost::find_path_for_uuid(a.volume_uuid.as_deref())
                    .and_then(|()| host.data_space())
                {
                    Ok((_, usable)) => {
                        record.write_bool(true);
                        record.write_i64(usable);
                        record.write_string16(None);
                    }
                    Err(error) => {
                        record.write_bool(false);
                        record.write_i64(0);
                        record.write_string16(Some(&error.message));
                    }
                }
                bridge::write_storage_space_reply(&mut p, &Some(record.data().to_vec()));
                Ok(())
            }
            bridge::DELETE_PRELOADS_FILE_CACHE => {
                bridge::DeletePreloadsFileCache::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                host.data().map(|data| {
                    super::app_data::delete_contents(&data.join("preloads/file_cache"), false);
                    bridge::write_delete_preloads_file_cache_reply(&mut p);
                })
            }
            bridge::DELETE_PARSER_CACHE => {
                bridge::DeleteParserCache::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let cache = host.package_parser_cache.get().ok_or_else(|| {
                        Exception::new(EX_ILLEGAL_STATE, "package parser cache not initialized")
                    })?;
                    if let Some(cache) = cache {
                        super::app_data::delete_contents(
                            &host
                                .data()?
                                .join(cache.strip_prefix("/data/").ok_or_else(|| {
                                    Exception::new(
                                        EX_ILLEGAL_STATE,
                                        "package parser cache outside data",
                                    )
                                })?),
                            false,
                        );
                    }
                    bridge::write_delete_parser_cache_reply(&mut p);
                    Ok(())
                })()
            }
            bridge::NOTE_CACHE_CLEAR_CALLER => {
                let a =
                    bridge::NoteCacheClearCaller::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                use super::event_log::Value;
                host.write_event(
                    3132,
                    Value::List(&[
                        Value::Int(a.calling_pid),
                        Value::Int(a.calling_uid),
                        Value::String(a.package_name.as_deref().unwrap_or("NULL")),
                    ]),
                )
                .map(|()| bridge::write_note_cache_clear_caller_reply(&mut p))
            }
            bridge::SEND_FREE_STORAGE_RESULT => {
                let a = bridge::SendFreeStorageResult::<
                    crate::package::installer::existing::IntentSender,
                >::read(&mut call.data)
                .map_err(|_| BAD_VALUE)?;
                (|| {
                    if let Some(sender) = a.sender {
                        let options = super::kept::RawBundle::empty()
                            .with_ints(&[("android.pendingIntent.backgroundActivityAllowed", 2)])
                            .map_err(|e| {
                                Exception::new(
                                    EX_ILLEGAL_STATE,
                                    format!("free storage options: {e}"),
                                )
                            })?;
                        let result = host.send_intent_sender(
                            SYSTEM_UID as i32,
                            None,
                            sender.0,
                            i32::from(a.success),
                            None,
                            None,
                            None,
                            None,
                            Some(options),
                        )?;
                        if result < 0 {
                            eprintln!("app host: Free storage IntentSender failed");
                        }
                    }
                    bridge::write_send_free_storage_result_reply(&mut p);
                    Ok(())
                })()
            }
            bridge::FREE_INSTALLD_CACHE => {
                let a = bridge::FreeInstalldCache::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                (|| {
                    let system = host.package_system()?;
                    let _install = system.package_install_guard();
                    let mut request = Parcel::new();
                    installd::FreeCache {
                        uuid: a.volume_uuid,
                        target_free_bytes: a.target_bytes,
                        flags: if a.v2 { 0x100 } else { 0 } | if a.defy_quota { 0x200 } else { 0 },
                    }
                    .write(&mut request);
                    // InstallerException is deliberately ignored by the pinned
                    // free-storage policy; acquisition/transport errors are not.
                    let owner = host
                        .installd()
                        .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
                    let response = owner
                        .transact(installd::FREE_CACHE, &request, false)
                        .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("freeCache: {e}")))?;
                    let _installer_result = installd::read_free_cache_reply(&mut response.reader())
                        .map_err(|e| {
                            Exception::new(EX_ILLEGAL_STATE, format!("freeCache reply: {e}"))
                        })?;
                    bridge::write_free_installd_cache_reply(&mut p);
                    Ok(())
                })()
            }
            bridge::NOTIFY_DEX_LOAD => {
                let a = bridge::NotifyDexLoad::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let owner = host.dex_use_bridge.lock().unwrap().clone();
                (|| {
                    let owner = owner.ok_or_else(|| {
                        Exception::new(
                            aim_binder_host::parcel::EX_ILLEGAL_STATE,
                            "ART dex-use owner has not attached",
                        )
                    })?;
                    let mut request = Parcel::new();
                    dex::NotifyDexLoad {
                        package_name: a.loading_package_name,
                        paths: a.dex_paths,
                        contexts: a.class_loader_contexts,
                        loader_isa: a.loader_isa,
                        calling_uid: a.calling_uid,
                    }
                    .write(&mut request);
                    let response = owner
                        .transact_as_owner(dex::NOTIFY_DEX_LOAD, &request, false)
                        .map_err(|status| {
                            Exception::new(
                                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                                format!("ART dex-use transport: {status}"),
                            )
                        })?;
                    dex::read_notify_dex_load_reply(&mut response.reader()).map_err(
                        |status| {
                            Exception::new(
                                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                                format!("ART dex-use reply: {status}"),
                            )
                        },
                    )??;
                    bridge::write_notify_dex_load_reply(&mut p);
                    Ok(())
                })()
            }
            bridge::CHECK_PERMISSION => {
                let a = bridge::CheckPermission::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.check(a.permission.as_deref(), a.calling_uid)
                    .map(|granted| bridge::write_check_permission_reply(&mut p, granted))
            }
            bridge::ENFORCE_PERMISSION => {
                let a = bridge::EnforcePermission::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enforce(a.permission.as_deref(), a.calling_uid)
                    .map(|()| bridge::write_enforce_permission_reply(&mut p))
            }
            bridge::FREE_CACHE_V2 => {
                bridge::FreeCacheV2::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let v2 = system_boolean(host.property("fw.free_cache_v2").as_deref(), true);
                bridge::write_free_cache_v2_reply(&mut p, v2);
                Ok(())
            }
            bridge::PRELOADS_EXPIRED => {
                bridge::PreloadsExpired::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let expired = system_boolean(
                    host.property("persist.sys.preloads.file_cache_expired")
                        .as_deref(),
                    false,
                );
                bridge::write_preloads_expired_reply(&mut p, expired);
                Ok(())
            }
            bridge::CACHE_PERIOD => {
                let a = bridge::CachePeriod::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match a.setting {
                    None => Err(Exception::new(EX_NULL_POINTER, "")),
                    // `Settings.Global.getLong`: the default where unset or
                    // not a number.
                    Some(name) => {
                        let period = host.global_setting(&name).unwrap_or(a.default_value);
                        bridge::write_cache_period_reply(&mut p, period);
                        Ok(())
                    }
                }
            }
            bridge::WALL_TIME_MILLIS => {
                bridge::WallTimeMillis::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis() as i64);
                bridge::write_wall_time_millis_reply(&mut p, now);
                Ok(())
            }
            code => {
                let name = bridge::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                let issue = match code {
                    bridge::GET_ART_MANAGER_BINDER
                    | bridge::DEXOPT_PACKAGE
                    | bridge::CLEAR_APP_PROFILES => "#2063",
                    _ => "#2156",
                };
                Err(host.unimplemented_named("package maintenance", &format!("{name} ({issue})")))
            }
        };
        reply(result.map(|()| p))
    }
}

/// The same lease-bound native visibility owner used by the Java package facade.
pub(super) struct DexUsePackages(
    pub std::sync::Arc<crate::package::boot_captures::Captures>,
    pub std::sync::Weak<AppHost>,
);

impl Service for DexUsePackages {
    fn descriptor(&self) -> &str {
        packages::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID {
            return reply(Err(Exception::security(
                "Native dex-use packages require system UID",
            )));
        }
        let mut p = Parcel::new();
        let result = match call.code {
            packages::GET_ART_OWNER_GENERATION => {
                packages::GetArtOwnerGeneration::read(&mut call.data)?;
                self.0
                    .current()
                    .and_then(|_| {
                        self.1
                            .upgrade()
                            .ok_or_else(|| Exception::illegal_argument("ART host stopped"))?
                            .art_owner_generation(call.sender_pid)
                    })
                    .map(|generation| {
                        packages::write_get_art_owner_generation_reply(&mut p, generation)
                    })
            }
            packages::ATTACH_ART_OWNER => {
                let a = packages::AttachArtOwner::read(&mut call.data)?;
                self.0
                    .current()
                    .and_then(|_| {
                        self.1
                            .upgrade()
                            .ok_or_else(|| Exception::illegal_argument("ART host stopped"))?
                            .attach_art_owner(
                                call.sender_pid,
                                a.generation,
                                self.0.clone(),
                                a.owner,
                            )
                    })
                    .map(|()| packages::write_attach_art_owner_reply(&mut p))
            }
            packages::CHECK_ART_OWNER => {
                let a = packages::CheckArtOwner::read(&mut call.data)?;
                self.1
                    .upgrade()
                    .ok_or_else(|| Exception::illegal_argument("ART host stopped"))
                    .and_then(|host| host.check_art_owner(call.sender_pid, a.generation, &self.0))
                    .map(|()| packages::write_check_art_owner_reply(&mut p))
            }
            packages::GET_SNAPSHOT => {
                packages::GetSnapshot::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.0
                    .snapshot()
                    .map(|snapshot| packages::write_get_snapshot_reply(&mut p, Some(snapshot)))
            }
            packages::FILTER_PACKAGE_NAME => {
                let a = packages::FilterPackageName::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.0
                    .filter_package_name(
                        a.version,
                        a.name.as_deref().unwrap_or_default(),
                        a.uid,
                        a.user_id,
                    )
                    .map(|name| packages::write_filter_package_name_reply(&mut p, &name))
            }
            packages::SHOULD_FILTER => {
                let a = packages::ShouldFilter::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.0
                    .should_filter(
                        a.version,
                        a.name.as_deref().unwrap_or_default(),
                        a.uid,
                        a.user_id,
                    )
                    .map(|filtered| packages::write_should_filter_reply(&mut p, filtered))
            }
            packages::CROSS_USER_SUSPENSIONS => {
                packages::CrossUserSuspensions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.0.current().map(|capture| {
                    packages::write_cross_user_suspensions_reply(
                        &mut p,
                        capture.context().cross_user_suspensions,
                    )
                })
            }
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_deletion_uses_installd_under_the_shared_lock_even_with_ce_locked() {
        use aim_binder_host::parcel::{Binder, EX_SECURITY, EX_UNSUPPORTED_OPERATION};
        use aim_service_aidl::android_os_iservicemanager as sm;
        use std::sync::{
            Mutex, OnceLock,
            atomic::{AtomicBool, Ordering},
        };
        type Calls = Arc<Mutex<Vec<(Option<String>, Option<String>, i32, i32, i64)>>>;
        struct Installer {
            calls: Calls,
            gate: Arc<OnceLock<Arc<Mutex<()>>>>,
            fail: Arc<AtomicBool>,
        }
        impl Service for Installer {
            fn descriptor(&self) -> &str {
                installd::DESCRIPTOR
            }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                assert_eq!(call.sender_euid, SYSTEM_UID);
                assert!(self.gate.get().unwrap().try_lock().is_err());
                let mut p = Parcel::new();
                match call.code {
                    installd::CLEAR_APP_DATA => {
                        let a = installd::ClearAppData::read(&mut call.data)?;
                        self.calls.lock().unwrap().push((
                            a.uuid,
                            a.package_name,
                            a.user_id,
                            a.flags,
                            a.ce_data_inode,
                        ));
                        if self.fail.load(Ordering::SeqCst) {
                            return reply(Err(Exception::new(
                                EX_ILLEGAL_STATE,
                                "cache deletion failed",
                            )));
                        }
                        installd::write_clear_app_data_reply(&mut p);
                    }
                    installd::FREE_CACHE => {
                        let a = installd::FreeCache::read(&mut call.data)?;
                        assert_eq!(a.uuid.as_deref(), Some("volume"));
                        assert_eq!(a.target_free_bytes, 1i64 << 40);
                        assert_eq!(a.flags, 0x300);
                        return reply(Err(Exception::new(EX_ILLEGAL_STATE, "free cache failed")));
                    }
                    _ => panic!("unexpected installd method"),
                }
                assert_eq!(call.data.remaining(), 0);
                Ok(p)
            }
        }
        struct Manager {
            process: std::sync::Weak<aim_binder_host::local::LocalProcess>,
            fallback: Mutex<Option<aim_binder_host::local::Strong>>,
            installer: Binder,
        }
        impl Service for Manager {
            fn descriptor(&self) -> &str {
                sm::DESCRIPTOR
            }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                if call.code == 0x00ff_ff00 {
                    let Some(Binder::Handle(handle)) = call.data.read_binder()? else {
                        panic!("remote host");
                    };
                    *self.fallback.lock().unwrap() =
                        Some(self.process.upgrade().unwrap().strong(handle));
                    return Ok(Parcel::new());
                }
                let at = call.data.position();
                let lookup = call.data.enforce_interface(sm::DESCRIPTOR).is_ok();
                call.data.set_position(at);
                if !lookup {
                    call.data.skip(call.data.remaining())?;
                    let (bytes, objects) = call.data.since(at);
                    let mut p = Parcel::new();
                    p.write_raw(bytes, &objects);
                    return self
                        .fallback
                        .lock()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .transact(call.code, &p, call.is_oneway())
                        .map(|r| r.into_parcel());
                }
                assert_eq!(call.code, sm::CHECK_SERVICE);
                let a = sm::CheckService::read(&mut call.data)?;
                assert_eq!(a.name.as_deref(), Some("installd"));
                let mut p = Parcel::new();
                sm::write_check_service_reply(&mut p, Some(self.installer));
                Ok(p)
            }
        }
        let calls: Calls = Arc::default();
        let gate = Arc::new(OnceLock::new());
        let fail = Arc::new(AtomicBool::new(false));
        let (record, lock, failure) = (calls.clone(), gate.clone(), fail.clone());
        crate::system::with_discovery_context(
            &[],
            true,
            true,
            move |_, process, _| {
                let installer = process.add_service(Arc::new(Installer {
                    calls: record,
                    gate: lock,
                    fail: failure,
                }));
                Arc::new(Manager {
                    process: Arc::downgrade(process),
                    fallback: Mutex::new(None),
                    installer,
                })
            },
            |host, _, _, _, _| {
                gate.set(host.package_system().unwrap().package_install_gate())
                    .unwrap();
                assert!(!host.ce_storage_prepared(0));
                let permissions = Arc::new(
                    super::super::permission_manager::PermissionManager::with_system(
                        host.clone(),
                        crate::package::permissions::AccessSystem::default(),
                    ),
                );
                assert!(host.permission_manager.set(permissions).is_ok());
                let leaf = PackageMaintenance::new(host.clone());
                for (uid, granted) in [(1000, true), (99_001, false)] {
                    let mut p = Parcel::new();
                    bridge::CheckPermission {
                        permission: Some("android.permission.INTERNAL_DELETE_CACHE_FILES".into()),
                        calling_uid: uid,
                        calling_pid: 77,
                    }
                    .write(&mut p);
                    let response = leaf
                        .transact(&mut Call {
                            code: bridge::CHECK_PERMISSION,
                            flags: 0,
                            sender_pid: 77,
                            sender_euid: SYSTEM_UID,
                            data: p.reader(),
                        })
                        .unwrap();
                    assert_eq!(
                        bridge::read_check_permission_reply(&mut response.reader())
                            .unwrap()
                            .unwrap(),
                        granted
                    );
                    let mut p = Parcel::new();
                    bridge::EnforcePermission {
                        permission: Some("android.permission.INTERNAL_DELETE_CACHE_FILES".into()),
                        calling_uid: uid,
                        calling_pid: 77,
                    }
                    .write(&mut p);
                    let response = leaf
                        .transact(&mut Call {
                            code: bridge::ENFORCE_PERMISSION,
                            flags: 0,
                            sender_pid: 77,
                            sender_euid: SYSTEM_UID,
                            data: p.reader(),
                        })
                        .unwrap();
                    let result =
                        bridge::read_enforce_permission_reply(&mut response.reader()).unwrap();
                    assert_eq!(result.is_ok(), granted);
                    if let Err(error) = result {
                        assert_eq!(error.code, EX_SECURITY);
                    }
                }
                let clear = |uid, package| {
                    let mut p = Parcel::new();
                    bridge::ClearCacheFiles {
                        package_name: Some(package),
                        user_id: 0,
                        can_access_instant_apps: false,
                        calling_uid: 1000,
                    }
                    .write(&mut p);
                    let response = leaf
                        .transact(&mut Call {
                            code: bridge::CLEAR_CACHE_FILES,
                            flags: 0,
                            sender_pid: 77,
                            sender_euid: uid,
                            data: p.reader(),
                        })
                        .unwrap();
                    bridge::read_clear_cache_files_reply(&mut response.reader()).unwrap()
                };
                for uid in [0, 2000, 10100] {
                    assert_eq!(clear(uid, "p".into()).unwrap_err().code, EX_SECURITY);
                }
                assert!(calls.lock().unwrap().is_empty());
                clear(SYSTEM_UID, "absent".into()).unwrap();
                let error = clear(SYSTEM_UID, "p".into()).unwrap_err();
                assert_eq!(error.code, EX_UNSUPPORTED_OPERATION);
                assert!(error.message.contains("#2063"));
                assert_eq!(
                    *calls.lock().unwrap(),
                    vec![
                        (None, Some("p".into()), 0, 0x17, 991),
                        (None, Some("p".into()), 0, 0x27, 991),
                    ]
                );
                let capture = host.package_capture().unwrap();
                let mut state = (**capture.state()).clone();
                state
                    .packages
                    .get_mut("p")
                    .unwrap()
                    .users
                    .entry(10)
                    .or_default()
                    .instant_app = true;
                assert!(cache_package(&state, Some("p"), false, 1_001_000).is_none());
                assert!(cache_package(&state, Some("p"), true, 1_001_000).is_some());
                assert!(cache_package(&state, Some("p"), false, 1000).is_some());
                fail.store(true, Ordering::SeqCst);
                assert!(
                    clear(SYSTEM_UID, "p".into())
                        .unwrap_err()
                        .message
                        .contains("cache deletion failed")
                );
                let mut p = Parcel::new();
                bridge::FreeInstalldCache {
                    volume_uuid: Some("volume".into()),
                    target_bytes: 1i64 << 40,
                    v2: true,
                    defy_quota: true,
                }
                .write(&mut p);
                let response = leaf
                    .transact(&mut Call {
                        code: bridge::FREE_INSTALLD_CACHE,
                        flags: 0,
                        sender_pid: 77,
                        sender_euid: SYSTEM_UID,
                        data: p.reader(),
                    })
                    .unwrap();
                bridge::read_free_installd_cache_reply(&mut response.reader())
                    .unwrap()
                    .unwrap();
                assert!(gate.get().unwrap().try_lock().is_ok());
            },
        );
    }

    #[test]
    fn storage_records_and_cache_roots_keep_the_original_scope() {
        let data = super::super::test_temp_dir("maintenance-cache");
        let host = AppHost::new(crate::test_process(SYSTEM_UID));
        super::super::configure_test_host(&host, &data, &data, &data, Box::new(|_| None));
        host.package_parser_cache
            .set(Some("/data/system/package_cache/fingerprint".into()))
            .unwrap();
        let leaf = PackageMaintenance::new(host.clone());
        let request = |code, p: &Parcel| {
            leaf.transact(&mut Call {
                code,
                flags: 0,
                sender_pid: 77,
                sender_euid: SYSTEM_UID,
                data: p.reader(),
            })
            .unwrap()
        };
        for (uuid, success) in [(None, true), (Some("absent"), false)] {
            let mut p = Parcel::new();
            bridge::StorageSpace {
                volume_uuid: uuid.map(str::to_owned),
            }
            .write(&mut p);
            let response = request(bridge::STORAGE_SPACE, &p);
            let bytes = bridge::read_storage_space_reply(&mut response.reader())
                .unwrap()
                .unwrap()
                .unwrap();
            let mut r = aim_binder_host::parcel::Reader::new(&bytes, &[]);
            assert_eq!(r.read_bool().unwrap(), success);
            assert_eq!(r.read_i64().unwrap() > 0, success);
            let error = r.read_string16().unwrap();
            assert_eq!(error.is_none(), success);
            if let Some(error) = error {
                assert!(error.contains("java.io.FileNotFoundException"));
            }
            assert_eq!(r.remaining(), 0);
        }
        for path in [
            "preloads/file_cache/nested/remove",
            "preloads/keep",
            "system/package_cache/fingerprint/nested/remove",
            "system/package_cache/sibling/keep",
        ] {
            let path = data.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"fixture").unwrap();
        }
        let mut p = Parcel::new();
        bridge::DeletePreloadsFileCache {}.write(&mut p);
        let response = request(bridge::DELETE_PRELOADS_FILE_CACHE, &p);
        bridge::read_delete_preloads_file_cache_reply(&mut response.reader())
            .unwrap()
            .unwrap();
        let mut p = Parcel::new();
        bridge::DeleteParserCache {}.write(&mut p);
        let response = request(bridge::DELETE_PARSER_CACHE, &p);
        bridge::read_delete_parser_cache_reply(&mut response.reader())
            .unwrap()
            .unwrap();
        for root in ["preloads/file_cache", "system/package_cache/fingerprint"] {
            assert_eq!(std::fs::read_dir(data.join(root)).unwrap().count(), 0);
        }
        for sibling in ["preloads/keep", "system/package_cache/sibling/keep"] {
            assert_eq!(std::fs::read(data.join(sibling)).unwrap(), b"fixture");
        }
        host.process.shutdown().unwrap();
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn dex_use_requires_the_owner_and_preserves_the_report_and_original_failure() {
        use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, EX_SECURITY};
        use aim_service_aidl::dev_aim_server_isystemprocess as sp;
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct Dex(Arc<AtomicUsize>);
        impl Service for Dex {
            fn descriptor(&self) -> &str {
                dex::DESCRIPTOR
            }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                assert_eq!(call.sender_euid, SYSTEM_UID);
                assert_eq!(call.code, dex::NOTIFY_DEX_LOAD);
                let a = dex::NotifyDexLoad::read(&mut call.data)?;
                assert_eq!(a.calling_uid, 99_001);
                assert_eq!(a.package_name.as_deref(), Some("a.b"));
                assert_eq!(
                    a.paths,
                    Some(vec![Some("/data/user/0/a.b/code.dex".into())])
                );
                assert_eq!(a.contexts, Some(vec![Some("PCL[]".into())]));
                assert_eq!(a.loader_isa.as_deref(), Some("arm64"));
                self.0.fetch_add(1, Ordering::SeqCst);
                reply(Err(Exception::illegal_argument("original ART validation")))
            }
        }
        let process = crate::test_process(SYSTEM_UID);
        let host = AppHost::new(process.clone());
        let leaf = PackageMaintenance::new(host.clone());
        let call = |service: &dyn Service, uid, code, parcel: &Parcel| {
            service
                .transact(&mut Call {
                    code,
                    flags: 0,
                    sender_pid: 77,
                    sender_euid: uid,
                    data: parcel.reader(),
                })
                .unwrap()
        };
        let mut request = Parcel::new();
        bridge::NotifyDexLoad {
            loading_package_name: Some("a.b".into()),
            dex_paths: Some(vec![Some("/data/user/0/a.b/code.dex".into())]),
            class_loader_contexts: Some(vec![Some("PCL[]".into())]),
            loader_isa: Some("arm64".into()),
            calling_uid: 99_001,
            calling_pid: 42,
        }
        .write(&mut request);
        let result = call(&leaf, 10_100, bridge::NOTIFY_DEX_LOAD, &request);
        assert_eq!(
            bridge::read_notify_dex_load_reply(&mut result.reader())
                .unwrap()
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        let result = call(&leaf, SYSTEM_UID, bridge::NOTIFY_DEX_LOAD, &request);
        assert_eq!(
            bridge::read_notify_dex_load_reply(&mut result.reader())
                .unwrap()
                .unwrap_err()
                .code,
            EX_ILLEGAL_STATE
        );
        let calls = Arc::new(AtomicUsize::new(0));
        let binder = process.add_service(Arc::new(Dex(calls.clone())));
        assert!(matches!(binder, Binder::Local(_)));
        let system = super::super::system_process::SystemProcess::new(host.clone());
        let mut attach = Parcel::new();
        sp::AttachDexUseBridge {
            bridge: Some(binder),
        }
        .write(&mut attach);
        let result = call(&system, 10_100, sp::ATTACH_DEX_USE_BRIDGE, &attach);
        assert_eq!(
            sp::read_attach_dex_use_bridge_reply(&mut result.reader())
                .unwrap()
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        assert!(host.dex_use_bridge.lock().unwrap().is_none());
        let result = call(&system, SYSTEM_UID, sp::ATTACH_DEX_USE_BRIDGE, &attach);
        sp::read_attach_dex_use_bridge_reply(&mut result.reader())
            .unwrap()
            .unwrap();
        let result = call(&leaf, SYSTEM_UID, bridge::NOTIFY_DEX_LOAD, &request);
        let failure = bridge::read_notify_dex_load_reply(&mut result.reader())
            .unwrap()
            .unwrap_err();
        assert_eq!(failure.message, "original ART validation");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// The descriptor the installer's permission owner checks, the cache
    /// policy's defaults, a null permission refused, and the owners not
    /// ported naming their issues.
    #[test]
    fn the_maintenance_bridge_answers_as_the_original() {
        let process = crate::test_process(SYSTEM_UID);
        let host = AppHost::new(process.clone());
        let aim_binder_host::parcel::Binder::Local(ptr) =
            process.add_service(Arc::new(PackageMaintenance::new(host)))
        else {
            unreachable!()
        };
        let node = process.local_service(ptr).unwrap();
        let reply = node
            .transact(u32::from_be_bytes(*b"_NTF"), &Parcel::new(), false)
            .unwrap();
        assert_eq!(
            reply.reader().read_string16().unwrap().as_deref(),
            Some(bridge::DESCRIPTOR)
        );
        let call = |code: u32, write: &dyn Fn(&mut Parcel)| {
            let mut p = Parcel::new();
            write(&mut p);
            node.transact(code, &p, false).unwrap()
        };
        let reply = call(bridge::FREE_CACHE_V2, &|p| bridge::FreeCacheV2 {}.write(p));
        assert!(
            bridge::read_free_cache_v2_reply(&mut reply.reader())
                .unwrap()
                .unwrap()
        );
        let reply = call(bridge::PRELOADS_EXPIRED, &|p| {
            bridge::PreloadsExpired {}.write(p)
        });
        assert!(
            !bridge::read_preloads_expired_reply(&mut reply.reader())
                .unwrap()
                .unwrap()
        );
        let reply = call(bridge::CACHE_PERIOD, &|p| {
            bridge::CachePeriod {
                setting: Some("installer_cache_period".into()),
                default_value: 42,
            }
            .write(p)
        });
        assert_eq!(
            bridge::read_cache_period_reply(&mut reply.reader())
                .unwrap()
                .unwrap(),
            42
        );
        let reply = call(bridge::CHECK_PERMISSION, &|p| {
            bridge::CheckPermission {
                permission: None,
                calling_uid: 10_100,
                calling_pid: 1,
            }
            .write(p)
        });
        let e = bridge::read_check_permission_reply(&mut reply.reader())
            .unwrap()
            .unwrap_err();
        assert_eq!(e.message, "permission is null");
        let reply = call(bridge::DELETE_PARSER_CACHE, &|p| {
            bridge::DeleteParserCache {}.write(p)
        });
        let e = bridge::read_delete_parser_cache_reply(&mut reply.reader())
            .unwrap()
            .unwrap_err();
        assert!(
            e.message.contains("package parser cache not initialized"),
            "{}",
            e.message
        );
        let reply = call(bridge::CLEAR_APP_PROFILES, &|p| {
            bridge::ClearAppProfiles {
                package_name: Some("a.b".into()),
            }
            .write(p)
        });
        let e = bridge::read_clear_app_profiles_reply(&mut reply.reader())
            .unwrap()
            .unwrap_err();
        assert!(
            e.message.contains("clearAppProfiles (#2063)"),
            "{}",
            e.message
        );
    }
}
