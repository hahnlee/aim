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
        std::fs::write(&path, [0x0a, 0]).unwrap();
        assert_eq!(
            AppHibernation(host.clone())
                .unhibernate(0)
                .unwrap_err()
                .code,
            EX_UNSUPPORTED_OPERATION
        );
        let p = call(s, ah::IS_HIBERNATING_FOR_USER, 1000, 0, Some("p"));
        assert_eq!(error(&p), EX_UNSUPPORTED_OPERATION);
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
