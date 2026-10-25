//! ExistingInstallEffects, android-16.0.0_r1.
use super::*;
use aim_binder_host::parcel::EX_SECURITY;
use aim_service_aidl::android_content_iintentsender as sender;
use std::sync::mpsc;

fn call(leaf: &PackageBootstrap, uid: u32, code: u32, data: &Parcel) -> Parcel {
    leaf.transact(&mut Call {
        code,
        flags: 0,
        sender_pid: 77,
        sender_euid: uid,
        data: data.reader(),
    })
    .unwrap()
}

#[test]
fn existing_policy_uses_the_user_restriction_and_device_owners() {
    crate::system::with_discovery_session(|host, _, _| {
        super::super::user_service::ensure_system_user(host).unwrap();
        let data = host.data().unwrap();
        std::fs::write(data.join("system/device_owner_2.xml"),
            br#"<root><device-owner component="p/.Admin"/><device-owner-context userId="0"/></root>"#).unwrap();
        std::fs::write(
            data.join("system/device_policies.xml"),
            br#"<policies><admin name="p/.Admin"/></policies>"#,
        )
        .unwrap();
        host.user_restrictions
            .set(["no_install_apps".into(), "no_debugging_features".into()].into())
            .unwrap();
        let leaf = PackageBootstrap::new(host.clone());
        for user in [-1, 0, 10] {
            let mut p = Parcel::new();
            bridge::GetExistingPackageInstallUserPolicy { user_id: user }.write(&mut p);
            let denied = call(
                &leaf,
                10100,
                bridge::GET_EXISTING_PACKAGE_INSTALL_USER_POLICY,
                &p,
            );
            assert_eq!(
                denied.reader().read_exception().unwrap().unwrap_err().code,
                EX_SECURITY
            );
            let reply = call(
                &leaf,
                1000,
                bridge::GET_EXISTING_PACKAGE_INSTALL_USER_POLICY,
                &p,
            );
            let result =
                bridge::read_get_existing_package_install_user_policy_reply(&mut reply.reader())
                    .unwrap();
            if user < 0 {
                assert_eq!(result.unwrap_err().code, EX_ILLEGAL_ARGUMENT);
            } else {
                let record = result.unwrap().unwrap();
                let mut r = aim_binder_host::parcel::Reader::new(&record, &[]);
                assert_eq!(r.read_i32().unwrap(), user);
                for _ in 0..4 {
                    assert_eq!(r.read_bool().unwrap(), user == 0);
                }
                assert_eq!(r.remaining(), 0);
            }
        }
    });
}

struct StatusReceiver(mpsc::Sender<sender::Send<RawIntent, RawBundle>>);
impl Service for StatusReceiver {
    fn descriptor(&self) -> &str {
        sender::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.code, sender::SEND);
        self.0.send(sender::Send::read(&mut call.data)?).unwrap();
        Ok(Parcel::new())
    }
}

#[test]
fn existing_completion_sends_only_public_status_and_denies_background_launch() {
    crate::system::with_discovery_session(|host, _, _| {
        let leaf = PackageBootstrap::new(host.clone());
        let (tx, rx) = mpsc::channel();
        let target = host.process.add_service(Arc::new(StatusReceiver(tx)));
        for (status, public) in [(1, 0), (-2, 4), (-7, 5), (-111, 7), (-999, 1)] {
            let mut p = Parcel::new();
            bridge::CompleteExistingPackageInstall {
                package_name: Some("p".into()),
                user_id: 0,
                target: Some(IntentSender(Some(target))),
                status,
                restore_permissions: false,
            }
            .write(&mut p);
            let denied = call(&leaf, 10100, bridge::COMPLETE_EXISTING_PACKAGE_INSTALL, &p);
            assert_eq!(
                denied.reader().read_exception().unwrap().unwrap_err().code,
                EX_SECURITY
            );
            let reply = call(&leaf, 1000, bridge::COMPLETE_EXISTING_PACKAGE_INSTALL, &p);
            assert_eq!(
                bridge::read_complete_existing_package_install_reply(&mut reply.reader())
                    .unwrap()
                    .unwrap(),
                None
            );
            let sent = rx.recv().unwrap();
            assert_eq!(sent.code, 0);
            assert!(
                sent.resolved_type.is_none()
                    && sent.whitelist_token.is_none()
                    && sent.finished_receiver.is_none()
                    && sent.required_permission.is_none()
            );
            let intent = sent.intent.unwrap();
            assert_eq!(intent.intent.action, None);
            assert_eq!(
                intent.test_extras(),
                [("android.content.pm.extra.STATUS".into(), Value::Int(public))].into()
            );
            let mut expected = Parcel::new();
            use aim_service_aidl::WriteParcelable;
            RawBundle::empty()
                .with_ints(&[("android.pendingIntent.backgroundActivityAllowed", 2)])
                .unwrap()
                .write_to(&mut expected);
            let mut actual = Parcel::new();
            sent.options.unwrap().write_to(&mut actual);
            assert_eq!(actual.data(), expected.data());
        }
        // A consumed one-shot PendingIntent is the Java cancellation diagnostic.
        let target = host
            .get_intent_sender(
                1000,
                super::super::pending_intents::Request {
                    kind: super::super::pending_intents::INTENT_SENDER_BROADCAST,
                    package: Some("android".into()),
                    feature: None,
                    token: None,
                    result_who: None,
                    request_code: 0,
                    intents: Some(vec![Some(RawIntent::new("test.existing.status", 0, &[]))]),
                    resolved_types: None,
                    flags: 1 << 30,
                    options: None,
                    user: 0,
                },
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            leaf.complete_existing_install(None, 0, Some(IntentSender(Some(target))), 1, false)
                .unwrap(),
            None
        );
        assert_eq!(
            leaf.complete_existing_install(None, 0, Some(IntentSender(Some(target))), 1, false)
                .unwrap()
                .as_deref(),
            Some("android.content.IntentSender$SendIntentException")
        );
        assert!(rx.try_recv().is_err());
    });
}

#[test]
fn existing_restore_has_no_backup_owner_and_completion_requires_permissions_only_when_reached() {
    let process = aim_binder_host::local::LocalProcess::open(
        &aim_binder_driver::Driver::new(),
        aim_binder_driver::Device::Binder,
        aim_binder_driver::Credentials {
            pid: std::process::id() as i32,
            euid: 1000,
            security_context: None,
        },
    );
    let host = AppHost::new(process);
    let leaf = PackageBootstrap::new(host.clone());
    let mut p = Parcel::new();
    bridge::RestoreExistingPackageInstall {
        package_name: Some("p".into()),
        user_id: 0,
        token: 7,
    }
    .write(&mut p);
    let reply = call(&leaf, 1000, bridge::RESTORE_EXISTING_PACKAGE_INSTALL, &p);
    assert!(
        !bridge::read_restore_existing_package_install_reply(&mut reply.reader())
            .unwrap()
            .unwrap()
    );
    let reply = call(&leaf, 0, bridge::RESTORE_EXISTING_PACKAGE_INSTALL, &p);
    assert_eq!(
        reply.reader().read_exception().unwrap().unwrap_err().code,
        EX_SECURITY
    );
    for (name, status, restore) in [
        (None, 1, true),
        (Some("p"), -2, true),
        (Some("p"), 1, false),
    ] {
        assert_eq!(
            leaf.complete_existing_install(name, -1, None, status, restore)
                .unwrap(),
            None
        );
    }
    assert_eq!(
        leaf.complete_existing_install(Some("p"), 0, None, 1, true)
            .unwrap_err()
            .message,
        "permission owner is unavailable"
    );
    host.permission_manager
        .set(Arc::new(
            super::super::permission_manager::PermissionManager::new(host.clone()),
        ))
        .ok()
        .unwrap();
    // The permission owner validates the user and actually tries the module:
    // no configured package owner cannot be mistaken for "no backup".
    assert_eq!(
        leaf.complete_existing_install(Some("p"), -1, None, 1, true)
            .unwrap_err()
            .code,
        EX_ILLEGAL_ARGUMENT
    );
    assert!(
        leaf.complete_existing_install(Some("p"), 0, None, 1, true)
            .is_err()
    );
}
