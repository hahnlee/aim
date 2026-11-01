//! Source-derived: AppHibernationService.isHibernatingForUser and
//! getHibernatingPackagesForUser, android-16.0.0_r1: fresh/unrecorded
//! packages, MANAGE_APP_HIBERNATION, full cross-user access and disabled reads.
use super::super::permission_manager;
use super::*;
use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_SECURITY, EX_UNSUPPORTED_OPERATION};

const READS: [u32; 2] = [
    ah::IS_HIBERNATING_FOR_USER,
    ah::GET_HIBERNATING_PACKAGES_FOR_USER,
];
fn error(p: &Parcel) -> i32 {
    p.reader().read_exception().unwrap().unwrap_err().code
}
fn success(p: &Parcel) {
    p.reader().read_exception().unwrap().unwrap();
}

fn call(s: &dyn Service, code: u32, uid: u32, user: i32, package: Option<&str>) -> Parcel {
    let mut p = Parcel::new();
    p.write_interface_token(ah::DESCRIPTOR);
    if code == ah::IS_HIBERNATING_FOR_USER {
        p.write_string16(package);
    }
    p.write_i32(user);
    s.transact(&mut Call {
        code,
        flags: 0,
        sender_pid: 71,
        sender_euid: uid,
        data: p.reader(),
    })
    .unwrap()
}

#[test]
fn published_manager_reads_fresh_state_and_enforces_original_permissions() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        let pm =
            permission_manager::PermissionManager::with_system(host.clone(), Default::default());
        assert!(host.permission_manager.set(Arc::new(pm)).is_ok());
        let system = host.package_system().unwrap();
        let base = host.package_capture().unwrap();
        let mut context = (**base.context()).clone();
        context.system.system_permissions = Some(
            [
                (2901, "android.permission.INTERACT_ACROSS_USERS"),
                (2902, "android.permission.INTERACT_ACROSS_USERS_FULL"),
            ]
            .into_iter()
            .map(|(uid, cross)| (uid, [MANAGE.into(), cross.into()].into()))
            .collect(),
        );
        context.scan_version = base.scan().version() + 1;
        system
            .publish_package_scan_with_queries(
                &system.package_bootstrap().unwrap(),
                Some(base.scan()),
                base.scan().owner().clone(),
                base.scan().usage().clone(),
                context,
            )
            .unwrap();
        let services = host.services();
        let s = services
            .iter()
            .find(|(name, _)| *name == "app_hibernation")
            .unwrap()
            .1
            .as_ref();
        AppHibernation(host.clone()).unhibernate(0).unwrap();
        for package in [Some("p"), Some("uninstalled.package"), None] {
            let p = call(s, ah::IS_HIBERNATING_FOR_USER, 2901, 0, package);
            let mut r = p.reader();
            r.read_exception().unwrap().unwrap();
            assert_eq!(r.read_i32().unwrap(), 0);
            assert_eq!(r.remaining(), 0);
        }
        for code in READS {
            for (uid, user, expected) in [
                (10100, 0, EX_SECURITY),
                (2901, 10, EX_SECURITY),
                (1000, -1, EX_ILLEGAL_ARGUMENT),
            ] {
                let p = call(s, code, uid, user, Some("p"));
                assert_eq!(error(&p), expected);
            }
            // Full cross-user permission reaches the original absent-user empty state.
            success(&call(s, code, 2902, 10, None));
        }
        for user in [0, -2, -3] {
            let p = call(s, ah::GET_HIBERNATING_PACKAGES_FOR_USER, 1000, user, None);
            let mut r = p.reader();
            r.read_exception().unwrap().unwrap();
            assert_eq!(r.read_i32().unwrap(), 0); // empty list, never null (-1)
            assert_eq!(r.remaining(), 0);
        }
        let path = host.data().unwrap().join("system_ce/0/hibernation/states");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, [0x0a, 5, 0x0a, 1, b'p', 0x10, 1]).unwrap();
        assert_eq!(
            AppHibernation(host.clone())
                .unhibernate(0)
                .unwrap_err()
                .code,
            EX_UNSUPPORTED_OPERATION
        );
        let p = call(s, ah::IS_HIBERNATING_FOR_USER, 1000, 0, Some("p"));
        assert!(
            ah::read_is_hibernating_for_user_reply(&mut p.reader())
                .unwrap()
                .unwrap()
        );
        host.platform()
            .unwrap()
            .settings
            .set_config(
                "app_hibernation/",
                &mut [(
                    "app_hibernation/app_hibernation_enabled".into(),
                    Some("false".into()),
                )],
                Some("android"),
                true,
            )
            .unwrap();
        // Disabled reads precede permission, user validation and persisted state.
        for code in READS {
            success(&call(s, code, 10100, -1, None));
        }
        let p = call(s, ah::SET_HIBERNATING_FOR_USER, 1000, 0, Some("p"));
        assert_eq!(error(&p), EX_UNSUPPORTED_OPERATION);
    });
}

fn request(s: &dyn Service, code: u32, uid: u32, args: impl FnOnce(&mut Parcel)) -> Parcel {
    let mut p = Parcel::new();
    p.write_interface_token(ah::DESCRIPTOR);
    args(&mut p);
    s.transact(&mut Call {
        code,
        flags: 0,
        sender_pid: 71,
        sender_euid: uid,
        data: p.reader(),
    })
    .unwrap()
}

// Pinned UserLevelHibernationProto persists no savedByte, whereas the global
// writer persists int64 field 3. Unknown fields are skipped by both readers.
#[test]
fn original_proto_fields_preserve_global_bytes_and_skip_unknown_fields() {
    let bytes = [
        0x0a, 11, 0x0a, 1, b'p', 0x10, 1, 0x18, 0x80, 0x80, 0x80, 0x80, 0x10,
    ];
    let global = decode(&bytes, true).unwrap();
    assert!(global["p"].hibernated);
    assert_eq!(global["p"].saved, 1_i64 << 32);
    assert_eq!(decode(&bytes, false).unwrap()["p"].saved, 0);
    assert!(decode(&[0x0a, 2, 0x10], true).is_err());
    assert!(decode(&[0x0a, 0], false).unwrap().is_empty());
}

#[test]
fn persisted_global_and_user_queries_reload_atomic_backup_and_keep_user_isolation() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        let pm =
            permission_manager::PermissionManager::with_system(host.clone(), Default::default());
        assert!(host.permission_manager.set(Arc::new(pm)).is_ok());
        let s = AppHibernation(host.clone());
        let data = host.data().unwrap();
        let user = data.join("system_ce/0/hibernation/states");
        let global = data.join("system/hibernation/states");
        for path in [&user, &global] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        }
        let awake = [0x0a, 3, 0x0a, 1, b'p'];
        let asleep = [0x0a, 5, 0x0a, 1, b'p', 0x10, 1];
        std::fs::write(&user, awake).unwrap();
        std::fs::write(user.with_extension("bak"), asleep).unwrap();
        std::fs::write(&global, [0x0a, 7, 0x0a, 1, b'p', 0x10, 1, 0x18, 99]).unwrap();
        let p = call(&s, ah::IS_HIBERNATING_FOR_USER, 1000, 0, Some("p"));
        assert!(
            ah::read_is_hibernating_for_user_reply(&mut p.reader())
                .unwrap()
                .unwrap()
        );
        assert!(!user.with_extension("bak").exists());
        assert_eq!(std::fs::read(&user).unwrap(), asleep);
        let p = request(&s, ah::IS_HIBERNATING_GLOBALLY, 1000, |p| {
            p.write_string16(Some("p"))
        });
        let mut r = p.reader();
        r.read_exception().unwrap().unwrap();
        assert!(r.read_bool().unwrap());
        let p = request(&s, ah::GET_HIBERNATION_STATS_FOR_USER, 1000, |p| {
            aim_service_aidl::write_string_list(
                p,
                Some(&[Some("p".into()), Some("missing".into())]),
            );
            p.write_i32(0);
        });
        let mut r = p.reader();
        r.read_exception().unwrap().unwrap();
        let entries = aim_service_aidl::read_value_map(&mut r).unwrap().unwrap();
        assert_eq!(entries.len(), 1);
        let aim_service_aidl::ParcelValue::Custom { kind: 4, bytes } = &entries[0].1 else {
            panic!("no stats")
        };
        let mut stats = aim_binder_host::parcel::Reader::new(bytes, &[]);
        assert_eq!(
            stats.read_string16().unwrap().as_deref(),
            Some("android.apphibernation.HibernationStats")
        );
        assert_eq!(stats.read_i64().unwrap(), 99);
        assert_eq!(stats.remaining(), 0);
        assert_eq!(r.remaining(), 0);
        assert!(s.states(false, 10).unwrap().is_empty());
        std::fs::write(&user, awake).unwrap();
        let reloaded = AppHibernation(host.clone());
        assert!(!reloaded.hibernating(1000, Some("p"), 0, false).unwrap());
        assert!(reloaded.hibernating(1000, Some("p"), 0, true).unwrap());
        assert_eq!(
            error(&request(&s, ah::IS_HIBERNATING_GLOBALLY, 10100, |p| p
                .write_string16(Some("p")))),
            EX_SECURITY
        );
        host.platform()
            .unwrap()
            .settings
            .set_config(
                "app_hibernation/",
                &mut [(
                    "app_hibernation/app_hibernation_enabled".into(),
                    Some("false".into()),
                )],
                Some("android"),
                true,
            )
            .unwrap();
        assert!(!s.hibernating(10100, Some("p"), -1, true).unwrap());
        // This resource query always checks MANAGE, even while DeviceConfig is off.
        assert_eq!(
            error(&request(
                &s,
                ah::IS_OAT_ARTIFACT_DELETION_ENABLED,
                10100,
                |_| {}
            )),
            EX_SECURITY
        );
        assert_eq!(
            std::fs::read(&global).unwrap(),
            [0x0a, 7, 0x0a, 1, b'p', 0x10, 1, 0x18, 99]
        );
    });
}
