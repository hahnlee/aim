//! Transport contracts for retained PackageBootstrapBridge helpers (#2330).
use super::*;
use aim_binder_host::parcel::{Binder, EX_SECURITY, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_isystemprocess as sp;
use std::sync::atomic::{AtomicI32, Ordering};

fn call(service: &impl Service, uid: u32, code: u32, data: &Parcel) -> Reply {
    service.transact(&mut Call {
        code,
        flags: 0,
        sender_pid: 77,
        sender_euid: uid,
        data: data.reader(),
    })
}

struct Java {
    mode: Arc<AtomicI32>,
    calls: Arc<Mutex<Vec<u32>>>,
}
impl Service for Java {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.sender_euid, 1000);
        assert_eq!(call.flags & 1, 0);
        self.calls.lock().unwrap().push(call.code);
        match self.mode.load(Ordering::SeqCst) {
            1 => return reply(Err(Exception::security("Java owner denied"))),
            2 => return Err(UNKNOWN_TRANSACTION),
            _ => {}
        }
        let mut p = Parcel::new();
        match call.code {
            bridge::GET_PREFERRED_CROSS_PROFILE_ACCESS_CONTROL => {
                let a = bridge::GetPreferredCrossProfileAccessControl::read(&mut call.data)?;
                assert_eq!((a.source_user_id, a.target_user_id), (0, 10));
                bridge::write_get_preferred_cross_profile_access_control_reply(&mut p, 20);
            }
            bridge::RESET_PREFERRED_RUNTIME_PERMISSIONS => {
                let a = bridge::ResetPreferredRuntimePermissions::read(&mut call.data)?;
                assert_eq!(a.user_id, 0);
                bridge::write_reset_preferred_runtime_permissions_reply(&mut p);
            }
            bridge::ON_EXISTING_PACKAGE_INSTALLED => {
                let a = bridge::OnExistingPackageInstalled::read(&mut call.data)?;
                assert!(a.record.is_some());
                assert_eq!((a.user_id, a.install_flags), (0, 0x00400000));
                bridge::write_on_existing_package_installed_reply(&mut p, &Some(vec![3, -1, 99]));
            }
            bridge::SEND_PREFERRED_ACTIVITY_CHANGED => {
                let a = bridge::SendPreferredActivityChanged::read(&mut call.data)?;
                assert_eq!(a.user_id, -1);
                bridge::write_send_preferred_activity_changed_reply(&mut p);
            }
            bridge::WAIT_PACKAGE_BACKGROUND_HANDLER => {
                let a = bridge::WaitPackageBackgroundHandler::read(&mut call.data)?;
                bridge::write_wait_package_background_handler_reply(&mut p, a.timeout_millis > 0);
            }
            bridge::FORMAT_PACKAGE_TIMESTAMP => {
                let a = bridge::FormatPackageTimestamp::read(&mut call.data)?;
                assert_eq!(a.millis, -1);
                bridge::write_format_package_timestamp_reply(
                    &mut p,
                    &Some("original timestamp".into()),
                );
            }
            _ => unreachable!(),
        }
        assert_eq!(call.data.remaining(), 0);
        Ok(p)
    }
}

#[test]
fn retained_helpers_preserve_arguments_replies_and_owner_failures() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let leaf = PackageBootstrap::new(host.clone());
        let mode = Arc::new(AtomicI32::new(0));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let owner = export(Arc::new(Java {
            mode: mode.clone(),
            calls: calls.clone(),
        }));
        let mut broadcast = Parcel::new();
        bridge::SendPreferredActivityChanged { user_id: -1 }.write(&mut broadcast);
        let mut wait = Parcel::new();
        bridge::WaitPackageBackgroundHandler { timeout_millis: -1 }.write(&mut wait);
        let mut timestamp = Parcel::new();
        bridge::FormatPackageTimestamp { millis: -1 }.write(&mut timestamp);
        let mut installed = Parcel::new();
        bridge::OnExistingPackageInstalled {
            record: Some(Binder::Handle(owner.handle)),
            user_id: 0,
            install_flags: 0x00400000,
        }
        .write(&mut installed);
        let mut reset = Parcel::new();
        bridge::ResetPreferredRuntimePermissions { user_id: 0 }.write(&mut reset);
        let mut cross_profile = Parcel::new();
        bridge::GetPreferredCrossProfileAccessControl {
            source_user_id: 0,
            target_user_id: 10,
        }
        .write(&mut cross_profile);
        let requests = [
            (
                bridge::GET_PREFERRED_CROSS_PROFILE_ACCESS_CONTROL,
                cross_profile,
            ),
            (bridge::RESET_PREFERRED_RUNTIME_PERMISSIONS, reset),
            (bridge::ON_EXISTING_PACKAGE_INSTALLED, installed),
            (bridge::SEND_PREFERRED_ACTIVITY_CHANGED, broadcast),
            (bridge::WAIT_PACKAGE_BACKGROUND_HANDLER, wait),
            (bridge::FORMAT_PACKAGE_TIMESTAMP, timestamp),
        ];
        for (code, data) in &requests {
            let p = call(&leaf, 1000, *code, data).unwrap();
            assert_eq!(
                p.reader().read_exception().unwrap().unwrap_err().code,
                EX_ILLEGAL_STATE
            );
        }
        let system = super::super::system_process::SystemProcess::new(host.clone());
        let mut attachment = Parcel::new();
        sp::AttachPackageBootstrapJava {
            bridge: Some(Binder::Handle(owner.handle)),
        }
        .write(&mut attachment);
        for uid in [0, 2000, 10100] {
            let p = call(&system, uid, sp::ATTACH_PACKAGE_BOOTSTRAP_JAVA, &attachment).unwrap();
            assert_eq!(
                p.reader().read_exception().unwrap().unwrap_err().code,
                EX_SECURITY
            );
            assert!(host.package_bootstrap_java.lock().unwrap().is_none());
        }
        let p = call(
            &system,
            1000,
            sp::ATTACH_PACKAGE_BOOTSTRAP_JAVA,
            &attachment,
        )
        .unwrap();
        sp::read_attach_package_bootstrap_java_reply(&mut p.reader())
            .unwrap()
            .unwrap();
        for invalid in [None, Some(Binder::Local(1))] {
            assert!(host.attach_package_bootstrap_java(invalid).is_err());
        }
        for (code, data) in &requests {
            for uid in [0, 2000, 10100] {
                let p = call(&leaf, uid, *code, data).unwrap();
                assert_eq!(
                    p.reader().read_exception().unwrap().unwrap_err().code,
                    EX_SECURITY
                );
            }
            assert_eq!(
                call(&leaf, 1000, *code, &Parcel::new()).unwrap_err(),
                BAD_VALUE
            );
            let p = call(&leaf, 1000, *code, data).unwrap();
            let mut r = p.reader();
            match *code {
                bridge::GET_PREFERRED_CROSS_PROFILE_ACCESS_CONTROL => {
                    assert_eq!(
                        bridge::read_get_preferred_cross_profile_access_control_reply(&mut r)
                            .unwrap()
                            .unwrap(),
                        20
                    );
                }
                bridge::RESET_PREFERRED_RUNTIME_PERMISSIONS => {
                    bridge::read_reset_preferred_runtime_permissions_reply(&mut r)
                        .unwrap()
                        .unwrap()
                }
                bridge::SEND_PREFERRED_ACTIVITY_CHANGED => {
                    bridge::read_send_preferred_activity_changed_reply(&mut r)
                        .unwrap()
                        .unwrap()
                }
                bridge::WAIT_PACKAGE_BACKGROUND_HANDLER => assert!(
                    !bridge::read_wait_package_background_handler_reply(&mut r)
                        .unwrap()
                        .unwrap()
                ),
                bridge::ON_EXISTING_PACKAGE_INSTALLED => assert_eq!(
                    bridge::read_on_existing_package_installed_reply(&mut r)
                        .unwrap()
                        .unwrap(),
                    Some(vec![3, -1, 99])
                ),
                _ => assert_eq!(
                    bridge::read_format_package_timestamp_reply(&mut r)
                        .unwrap()
                        .unwrap()
                        .as_deref(),
                    Some("original timestamp")
                ),
            }
            assert_eq!(r.remaining(), 0);
            mode.store(1, Ordering::SeqCst);
            let p = call(&leaf, 1000, *code, data).unwrap();
            assert_eq!(
                p.reader().read_exception().unwrap(),
                Err(Exception::security("Java owner denied"))
            );
            mode.store(2, Ordering::SeqCst);
            assert_eq!(
                call(&leaf, 1000, *code, data).unwrap_err(),
                UNKNOWN_TRANSACTION
            );
            mode.store(0, Ordering::SeqCst);
        }
        assert_eq!(calls.lock().unwrap().len(), 18);
        let mut wait = Parcel::new();
        bridge::WaitPackageBackgroundHandler {
            timeout_millis: i64::MAX,
        }
        .write(&mut wait);
        let p = call(&leaf, 1000, bridge::WAIT_PACKAGE_BACKGROUND_HANDLER, &wait).unwrap();
        assert!(
            bridge::read_wait_package_background_handler_reply(&mut p.reader())
                .unwrap()
                .unwrap()
        );
        // A new system process replaces the retained owner; its predecessor's
        // failures must no longer be observed.
        mode.store(2, Ordering::SeqCst);
        let replacement = export(Arc::new(Java {
            mode: Arc::new(AtomicI32::new(0)),
            calls: Arc::new(Mutex::new(Vec::new())),
        }));
        host.attach_package_bootstrap_java(Some(Binder::Handle(replacement.handle)))
            .unwrap();
        let p = call(&leaf, 1000, bridge::WAIT_PACKAGE_BACKGROUND_HANDLER, &wait).unwrap();
        assert!(
            bridge::read_wait_package_background_handler_reply(&mut p.reader())
                .unwrap()
                .unwrap()
        );
    });
}

#[test]
fn domain_settings_v2_uses_the_existing_compat_owner() {
    crate::system::with_discovery_session(|host, _, _| {
        let config = host.root().unwrap().join("system/etc/compatconfig");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("domain.xml"),
            br#"<config><compat-change id="178111421" enableSinceTargetSdk="31"/></config>"#,
        )
        .unwrap();
        let leaf = PackageBootstrap::new(host.clone());
        for (name, sdk) in [
            (Some("a.b"), 29),
            (Some("a.b"), 36),
            (None, 36),
            (Some(""), 36),
            (Some("a.b"), -1),
        ] {
            let mut p = Parcel::new();
            bridge::IsDomainVerificationSettingsV2Enabled {
                package_name: name.map(str::to_owned),
                target_sdk: sdk,
            }
            .write(&mut p);
            let denied = call(
                &leaf,
                10100,
                bridge::IS_DOMAIN_VERIFICATION_SETTINGS_V2ENABLED,
                &p,
            )
            .unwrap();
            assert_eq!(
                denied.reader().read_exception().unwrap().unwrap_err().code,
                EX_SECURITY
            );
            let p = call(
                &leaf,
                1000,
                bridge::IS_DOMAIN_VERIFICATION_SETTINGS_V2ENABLED,
                &p,
            )
            .unwrap();
            let result =
                bridge::read_is_domain_verification_settings_v2enabled_reply(&mut p.reader())
                    .unwrap();
            if name.is_none_or(str::is_empty) || sdk < 0 {
                assert_eq!(
                    result.unwrap_err(),
                    Exception::illegal_argument("invalid domain compatibility identity")
                );
            } else {
                assert_eq!(result.unwrap(), sdk >= 31);
            }
        }
    });
}
