//! `INativeStagingBridge` for the app host: the native staging owner's
//! apexd input, which system_server's `NativeStagingBridge` reads as
//! `ApexManager.getStagedApexInfos` (`android-16.0.0_r1`), to the system
//! uid only. Cold inventory queries retain the original Java owner in aim-system.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_inativestagingbridge as bridge;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;

impl AppHost {
    pub(super) fn attach_native_staging_bridge(
        &self,
        binder: Option<Binder>,
    ) -> Result<(), Exception> {
        let Some(Binder::Handle(handle)) = binder else {
            return Err(Exception::illegal_argument("no native staging bridge"));
        };
        let owner = Arc::new(self.process.strong(handle));
        *self.native_staging_bridge.lock().unwrap() = Some(owner.clone());
        let weak_host = self.me.clone();
        let weak_owner = Arc::downgrade(&owner);
        self.process.link_to_death(
            &owner,
            Box::new(move || {
                if let Some(host) = weak_host.upgrade() {
                    let mut current = host.native_staging_bridge.lock().unwrap();
                    if current
                        .as_ref()
                        .is_some_and(|o| weak_owner.ptr_eq(&Arc::downgrade(o)))
                    {
                        current.take();
                    }
                }
            }),
        );
        Ok(())
    }
}

pub struct NativeStaging {
    host: Arc<AppHost>,
}

impl NativeStaging {
    pub fn new(host: Arc<AppHost>) -> Self {
        NativeStaging { host }
    }
}

impl Service for NativeStaging {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            bridge::GET_STAGED_APEX_INFOS => {
                let a = bridge::GetStagedApexInfos::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if call.sender_euid != SYSTEM_UID {
                    return reply(Err(Exception::security(
                        "native staging transport requires system UID",
                    )));
                }
                if a.session_id <= 0 || a.apex_child_session_ids.is_none() {
                    return reply(Err(Exception::illegal_argument("invalid staged session")));
                }
                let owner = self.host.native_staging_bridge.lock().unwrap().clone();
                let Some(owner) = owner else {
                    return reply(Err(self.host.unimplemented_named(
                        "native staging",
                        "original bridge not attached (#2158)",
                    )));
                };
                let mut data = Parcel::new();
                a.write(&mut data);
                owner
                    .transact(call.code, &data, false)
                    .map(|r| r.into_parcel())
            }
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_UNSUPPORTED_OPERATION, Parcel};

    /// An invalid session is refused; an absent original owner names the issue.
    #[test]
    fn staged_apex_infos_are_not_read() {
        let process = crate::test_process(SYSTEM_UID);
        let host = AppHost::new(process.clone());
        let aim_binder_host::parcel::Binder::Local(ptr) =
            process.add_service(Arc::new(NativeStaging::new(host)))
        else {
            unreachable!()
        };
        let node = process.local_service(ptr).unwrap();
        let infos = |session_id: i32| {
            let mut p = Parcel::new();
            bridge::GetStagedApexInfos {
                session_id,
                apex_child_session_ids: Some(vec![2]),
            }
            .write(&mut p);
            let reply = node
                .transact(bridge::GET_STAGED_APEX_INFOS, &p, false)
                .unwrap();
            reply.reader().read_exception().unwrap().unwrap_err().code
        };
        assert_eq!(infos(0), EX_ILLEGAL_ARGUMENT);
        assert_eq!(infos(1), EX_UNSUPPORTED_OPERATION);
    }

    /// The cold query keeps the session inputs, original parcelables and failures.
    #[test]
    fn staged_inventory_uses_the_attached_original_owner() {
        use crate::package::staging::ApexInfo;
        use aim_binder_host::parcel::{DEAD_OBJECT, EX_SECURITY};
        use aim_service_aidl::dev_aim_server_isystemprocess as sp;
        struct Original;
        impl Service for Original {
            fn descriptor(&self) -> &str {
                bridge::DESCRIPTOR
            }
            fn transact(&self, call: &mut Call<'_>) -> Reply {
                assert_eq!(call.sender_euid, SYSTEM_UID);
                assert_eq!(call.code, bridge::GET_STAGED_APEX_INFOS);
                let a = bridge::GetStagedApexInfos::read(&mut call.data)?;
                assert_eq!(a.apex_child_session_ids, Some(vec![8, 9]));
                let mut p = Parcel::new();
                match a.session_id {
                    1 => bridge::write_get_staged_apex_infos_reply(
                        &mut p,
                        Some(&[Some(ApexInfo {
                            module_name: Some("com.android.test".into()),
                            disk_image_path: Some("/data/app-staging/test.apex".into()),
                            version_code: 1 << 40,
                            version_name: Some("v1".into()),
                            has_class_path_jars: true,
                        })]),
                    ),
                    2 => bridge::write_get_staged_apex_infos_reply::<ApexInfo>(&mut p, Some(&[])),
                    3 => p.write_exception(&Exception::security("original rejection")),
                    _ => return Err(DEAD_OBJECT),
                }
                Ok(p)
            }
        }
        fn invoke(service: &dyn Service, uid: u32, code: u32, p: &Parcel) -> Reply {
            service.transact(&mut Call {
                code,
                flags: 0,
                sender_pid: 71,
                sender_euid: uid,
                data: p.reader(),
            })
        }
        crate::system::with_discovery_binders(|host, _, _, export| {
            let system = super::super::system_process::SystemProcess::new(host.clone());
            let owner = export(Arc::new(Original));
            let mut attach = Parcel::new();
            sp::AttachNativeStagingBridge {
                bridge: Some(Binder::Handle(owner.handle)),
            }
            .write(&mut attach);
            let denied = invoke(&system, 10123, sp::ATTACH_NATIVE_STAGING_BRIDGE, &attach).unwrap();
            assert_eq!(
                denied.reader().read_exception().unwrap().unwrap_err().code,
                EX_SECURITY
            );
            assert!(host.native_staging_bridge.lock().unwrap().is_none());
            for binder in [None, Some(Binder::Local(1))] {
                assert!(host.attach_native_staging_bridge(binder).is_err());
            }
            invoke(
                &system,
                SYSTEM_UID,
                sp::ATTACH_NATIVE_STAGING_BRIDGE,
                &attach,
            )
            .unwrap()
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
            let service = NativeStaging::new(host.clone());
            for id in 1..=4 {
                let mut p = Parcel::new();
                bridge::GetStagedApexInfos {
                    session_id: id,
                    apex_child_session_ids: Some(vec![8, 9]),
                }
                .write(&mut p);
                let denied = invoke(&service, 10123, bridge::GET_STAGED_APEX_INFOS, &p).unwrap();
                assert_eq!(
                    denied.reader().read_exception().unwrap().unwrap_err().code,
                    EX_SECURITY
                );
                let response = invoke(&service, SYSTEM_UID, bridge::GET_STAGED_APEX_INFOS, &p);
                if id == 4 {
                    assert_eq!(response.err(), Some(DEAD_OBJECT));
                    continue;
                }
                let response = response.unwrap();
                let rows =
                    bridge::read_get_staged_apex_infos_reply::<ApexInfo>(&mut response.reader())
                        .unwrap();
                if id == 3 {
                    assert_eq!(rows.unwrap_err(), Exception::security("original rejection"));
                } else {
                    let rows = rows.unwrap().unwrap();
                    if id == 2 {
                        assert!(rows.is_empty());
                        continue;
                    }
                    assert_eq!(
                        rows,
                        vec![Some(ApexInfo {
                            module_name: Some("com.android.test".into()),
                            disk_image_path: Some("/data/app-staging/test.apex".into()),
                            version_code: 1 << 40,
                            version_name: Some("v1".into()),
                            has_class_path_jars: true,
                        })]
                    );
                }
            }
        });
    }
}
