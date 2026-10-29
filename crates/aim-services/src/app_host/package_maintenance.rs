//! `IPackageMaintenanceBridge` for the app host: the native package
//! owner's permission checks, cache policy and wall time, answered as
//! system_server's `NativePackageMaintenanceBridge` answers them
//! (`android-16.0.0_r1`), to the system uid only. Its ART methods are ART
//! Service's (#2063), except dex-use recording in aim-system's original
//! DexUseManagerLocal; its storage and cache owners are not ported
//! (#2156).

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_NULL_POINTER, Exception, Parcel};
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
pub(super) struct DexUsePackages(pub crate::package::boot_captures::Captures);

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
            e.message.contains("deleteParserCache (#2156)"),
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
