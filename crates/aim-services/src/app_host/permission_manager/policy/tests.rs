use super::*;
use crate::app_host::app_op_infos::DefaultMode;
use crate::package::model::PackageUserState;
use crate::package::pkg::{AndroidPackage, Component};

const CAMERA: &str = "android.permission.CAMERA";
const COARSE: &str = "android.permission.ACCESS_COARSE_LOCATION";
const FINE: &str = "android.permission.ACCESS_FINE_LOCATION";
const BACKGROUND: &str = "android.permission.ACCESS_BACKGROUND_LOCATION";
const APP: i32 = 10100;

fn package(name: &str, id: i32, requested: &[&str]) -> PackageState {
    PackageState {
        name: name.into(),
        app_id: id,
        users: [(
            0,
            PackageUserState {
                installed: true,
                ..Default::default()
            },
        )]
        .into(),
        pkg: Some(Arc::new(AndroidPackage {
            package_name: name.into(),
            target_sdk_version: 36,
            requested_permissions: requested.iter().map(|name| (*name).into()).collect(),
            ..Default::default()
        })),
        ..Default::default()
    }
}

struct Fixture {
    state: State,
    defined: Vec<Permission>,
    user: AccessUser,
}
impl Fixture {
    fn new(requested: &[&str]) -> Self {
        let names = [
            CAMERA,
            COARSE,
            FINE,
            BACKGROUND,
            READ_EXTERNAL_STORAGE,
            WRITE_EXTERNAL_STORAGE,
        ];
        let mut android = package("android", 1000, &[]);
        Arc::make_mut(android.pkg.as_mut().unwrap()).permissions = names
            .iter()
            .map(|name| crate::package::pkg::Permission {
                component: Component {
                    name: (*name).into(),
                    package_name: "android".into(),
                    ..Default::default()
                },
                background_permission: matches!(*name, COARSE | FINE).then(|| BACKGROUND.into()),
                protection_level: 1,
                ..Default::default()
            })
            .collect();
        Self {
            state: State {
                packages: [
                    ("p".into(), package("p", APP, requested)),
                    ("android".into(), android),
                ]
                .into(),
                ..Default::default()
            },
            defined: names
                .iter()
                .map(|name| Permission {
                    name: (*name).into(),
                    package: "android".into(),
                    protection_level: 1,
                    dynamic: None,
                })
                .collect(),
            user: AccessUser::default(),
        }
    }
    fn run(&mut self, reset: Option<&str>, sync: bool) -> Result<Vec<i32>, Exception> {
        let filter = AppsFilter::default();
        let compat = CompatConfig::default();
        let access = Access {
            state: &self.state,
            filter: &filter,
            defined: &self.defined,
            groups: &[],
            user0: &self.user,
            compat: &compat,
        };
        let mut after = self.user.clone();
        let (codes, _) = synchronize(
            &access,
            &mut after,
            APP,
            reset,
            sync,
            &|name| {
                APP_OP_INFOS
                    .iter()
                    .find(|op| op.permission.is_some_and(|(p, _)| p == name))
            },
            &|op| match op.default_mode {
                DefaultMode::Mode(mode) => Ok(mode),
                _ => panic!("unexpected op"),
            },
            "",
        )?;
        self.user = after;
        Ok(codes)
    }
    fn mode(&self, code: usize) -> i32 {
        let op = &APP_OP_INFOS[code];
        let DefaultMode::Mode(default) = op.default_mode else {
            panic!("unexpected op")
        };
        raw(&self.user, APP, "p", op, default)
    }
}

/// PermissionPolicyService.PermissionToOpSynchroniser and CTS's runtime
/// grant/revoke contract: permission changes decide the effective switch mode.
#[test]
fn grant_revoke_and_permission_flags_flip_the_op() {
    let mut f = Fixture::new(&[CAMERA]);
    for (flags, expected) in [
        (0, MODE_IGNORED),
        (RUNTIME_GRANTED, MODE_ALLOWED),
        (RUNTIME_GRANTED | APP_OP_REVOKED, MODE_IGNORED),
        (RUNTIME_GRANTED, MODE_ALLOWED),
        (0, MODE_IGNORED),
    ] {
        f.user.set_permission_flags(APP, CAMERA, flags);
        assert_eq!(f.run(None, true).unwrap(), [26]);
        assert_eq!(f.mode(26), expected);
        assert!(f.run(None, true).unwrap().is_empty());
    }
    f.user
        .set_permission_flags(APP, CAMERA, LEGACY_GRANTED | IMPLICIT);
    assert!(f.run(None, true).unwrap().is_empty());
    assert_eq!(f.mode(26), MODE_IGNORED);
}

#[test]
fn background_and_shared_switches_choose_the_most_permissive_mode() {
    let mut f = Fixture::new(&[FINE]);
    f.state
        .packages
        .insert("q".into(), package("q", APP, &[COARSE]));
    f.user.set_permission_flags(APP, COARSE, RUNTIME_GRANTED);
    f.run(None, true).unwrap();
    assert_eq!(f.mode(0), MODE_FOREGROUND);
    f.user
        .set_permission_flags(APP, BACKGROUND, RUNTIME_GRANTED);
    f.run(None, true).unwrap();
    assert_eq!(f.mode(0), MODE_ALLOWED);
    f.user.set_permission_flags(APP, COARSE, 0);
    f.run(None, true).unwrap();
    assert_eq!(f.mode(0), MODE_IGNORED);
    // Removing the fine-location request cannot reset a switch still used
    // by coarse location in another package of the UID.
    Arc::make_mut(f.state.packages.get_mut("p").unwrap().pkg.as_mut().unwrap())
        .requested_permissions
        .clear();
    f.run(Some("p"), true).unwrap();
    assert_eq!(f.mode(0), MODE_IGNORED);
}

#[test]
fn removed_package_resets_modes_and_preserves_other_uids() {
    let mut f = Fixture::new(&[CAMERA]);
    f.run(None, true).unwrap();
    f.user.package_app_ops = vec![
        ("p".into(), vec![("android:camera".into(), MODE_IGNORED)]),
        (
            "other".into(),
            vec![("android:camera".into(), MODE_IGNORED)],
        ),
    ];
    f.user
        .app_id_app_ops
        .push((APP + 1, vec![("android:camera".into(), MODE_IGNORED)]));
    f.state.packages.remove("p");
    f.run(Some("p"), false).unwrap();
    assert_eq!(f.mode(26), MODE_ALLOWED);
    assert_eq!(f.user.app_id_app_ops.len(), 1);
    assert_eq!(f.user.package_app_ops.len(), 1);
    assert_eq!(f.user.package_app_ops[0].0, "other");
}

#[test]
fn changed_package_no_longer_requesting_a_permission_resets_the_op() {
    let mut f = Fixture::new(&[CAMERA]);
    f.run(None, true).unwrap();
    f.user.package_app_ops = vec![("p".into(), vec![("android:camera".into(), MODE_IGNORED)])];
    Arc::make_mut(f.state.packages.get_mut("p").unwrap().pkg.as_mut().unwrap())
        .requested_permissions
        .clear();
    assert_eq!(f.run(Some("p"), true).unwrap(), [26]);
    assert_eq!(f.mode(26), MODE_ALLOWED);
    assert!(f.user.app_id_app_ops.is_empty());
    assert!(f.user.package_app_ops.is_empty());
}

#[test]
fn app_op_permission_reset_keeps_the_pinned_exclusions() {
    let mut f = Fixture::new(&[]);
    for name in [
        "android.permission.MANAGE_EXTERNAL_STORAGE",
        "android.permission.REQUEST_INSTALL_PACKAGES",
    ] {
        f.defined.push(Permission {
            name: name.into(),
            package: "android".into(),
            protection_level: 2 | PROTECTION_FLAG_APPOP,
            dynamic: None,
        });
    }
    let modes = vec![
        ("android:manage_external_storage".into(), MODE_ALLOWED),
        ("android:request_install_packages".into(), MODE_ALLOWED),
    ];
    f.user.app_id_app_ops = vec![(APP, modes.clone())];
    f.user.package_app_ops = vec![("p".into(), modes)];
    f.run(Some("p"), true).unwrap();
    assert_eq!(f.mode(92), appops_service::MODE_DEFAULT);
    assert_eq!(f.mode(66), MODE_ALLOWED);
    assert_eq!(f.user.app_id_app_ops[0].1.len(), 1);
    assert_eq!(f.user.package_app_ops[0].1.len(), 1);
}

#[test]
fn incorrect_package_override_is_reset_when_the_uid_mode_is_default() {
    let mut f = Fixture::new(&[CAMERA]);
    let mut shared = package("q", -1, &[CAMERA]);
    shared.shared_user_app_id = Some(APP);
    f.state.packages.insert("q".into(), shared);
    f.user.set_permission_flags(APP, CAMERA, RUNTIME_GRANTED);
    f.user.package_app_ops = ["p", "q"]
        .iter()
        .map(|name| {
            (
                (*name).into(),
                vec![("android:camera".into(), MODE_IGNORED)],
            )
        })
        .collect();
    f.run(None, true).unwrap();
    assert_eq!(f.mode(26), MODE_ALLOWED);
    assert!(f.user.package_app_ops.is_empty());
}

#[test]
fn grant_origin_and_fixed_flags_do_not_override_the_pinned_permission_rule() {
    let mut f = Fixture::new(&[CAMERA]);
    // PermissionToOpSynchroniser reads the committed grant regardless of
    // its origin. Role-controller and device-policy mode writes remain
    // their owners' unsupported branches (#1657, #1521).
    for flags in [ROLE, POLICY_FIXED, SYSTEM_FIXED, PREGRANT] {
        f.user.set_permission_flags(APP, CAMERA, flags);
        f.run(None, true).unwrap();
        assert_eq!(f.mode(26), MODE_IGNORED);
        f.user
            .set_permission_flags(APP, CAMERA, flags | RUNTIME_GRANTED);
        f.run(None, true).unwrap();
        assert_eq!(f.mode(26), MODE_ALLOWED);
    }
}

#[test]
fn hard_and_soft_restriction_flags_and_extra_op() {
    let mut f = Fixture::new(&[CAMERA]);
    Arc::make_mut(
        f.state
            .packages
            .get_mut("android")
            .unwrap()
            .pkg
            .as_mut()
            .unwrap(),
    )
    .permissions[0]
        .component
        .flags = FLAG_HARD_RESTRICTED;
    f.user
        .set_permission_flags(APP, CAMERA, LEGACY_GRANTED | RESTRICTION_REVOKED);
    f.run(None, true).unwrap();
    assert_eq!(f.mode(26), MODE_IGNORED);
    f.user
        .set_permission_flags(APP, CAMERA, LEGACY_GRANTED | SYSTEM_EXEMPT);
    f.run(None, true).unwrap();
    assert_eq!(f.mode(26), MODE_ALLOWED);
    let mut f = Fixture::new(&[READ_EXTERNAL_STORAGE]);
    Arc::make_mut(
        f.state
            .packages
            .get_mut("android")
            .unwrap()
            .pkg
            .as_mut()
            .unwrap(),
    )
    .permissions[4]
        .component
        .flags = FLAG_SOFT_RESTRICTED;
    f.user.set_permission_flags(
        APP,
        READ_EXTERNAL_STORAGE,
        RUNTIME_GRANTED | SOFT_RESTRICTED,
    );
    f.run(None, true).unwrap();
    assert_eq!(f.mode(59), MODE_ALLOWED);
    assert_eq!(f.mode(87), MODE_IGNORED);
}

#[test]
fn registry_callbacks_persist_and_reload_the_live_access_owner() {
    use crate::app_host::{package_bootstrap::PackageBootstrap, package_observers::Events};
    use aim_service_aidl::dev_aim_server_ipackageobservereventsbridge as events;
    crate::system::with_discovery_session(|host, publish, _| {
        super::super::tests::write_access(host, SYSTEM_ACCESS, b"<access><permissions><permission name=\"android.permission.CAMERA\" packageName=\"p\" protectionLevel=\"1\" type=\"0\"/></permissions></access>");
        super::super::tests::write_access(
            host,
            USER0_ACCESS,
            b"<access><app-id-permissions/></access>",
        );
        let manager = Arc::new(PermissionManager::new(host.clone()));
        assert!(host.permission_manager.set(manager.clone()).is_ok());
        publish(&mut |pkg| {
            if pkg.package_name == "p" {
                pkg.target_sdk_version = 36;
                pkg.requested_permissions = vec![CAMERA.into()];
                pkg.permissions = vec![crate::package::pkg::Permission {
                    component: Component {
                        name: CAMERA.into(),
                        package_name: "p".into(),
                        ..Default::default()
                    },
                    protection_level: 1,
                    ..Default::default()
                }];
            }
        });
        let _bootstrap = PackageBootstrap::new(host.clone());
        let registry = host.package_observers.lock().unwrap().upgrade().unwrap();
        let events = Events(registry);
        let event = |code| {
            let mut p = Parcel::new();
            p.write_interface_token(events::DESCRIPTOR);
            p.write_string16(Some("p"));
            // Even a publication for a stopped user synchronizes the UID
            // in the host's started user 0.
            p.write_i32(10 * PER_USER_RANGE + APP);
            events
                .transact(&mut Call {
                    code,
                    flags: 0,
                    sender_pid: 77,
                    sender_euid: 1000,
                    data: p.reader(),
                })
                .unwrap()
                .reader()
                .read_exception()
                .unwrap()
        };
        event(events::PACKAGE_ADDED).unwrap();
        assert_eq!(
            manager.uid_mode(APP, "android:camera").unwrap(),
            Some(MODE_IGNORED)
        );
        let before = manager.retained_access().unwrap();
        assert_eq!(
            PermissionManager::new(host.clone())
                .uid_mode(APP, "android:camera")
                .unwrap(),
            Some(MODE_IGNORED)
        );
        publish(&mut |pkg| {
            if pkg.package_name == "p" {
                pkg.requested_permissions.clear()
            }
        });
        let path = host.data().unwrap().join(USER0_ACCESS);
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let error = event(events::PACKAGE_CHANGED).unwrap_err();
        assert_eq!(error.code, EX_ILLEGAL_STATE);
        assert!(before.same_base(&manager.retained_access().unwrap()));
        std::fs::write(&path, bytes).unwrap();
        event(events::PACKAGE_CHANGED).unwrap();
        assert_eq!(manager.uid_mode(APP, "android:camera").unwrap(), None);
        assert_eq!(before.user0.app_id_app_ops[0].1[0].1, MODE_IGNORED);
        assert_eq!(
            PermissionManager::new(host.clone())
                .uid_mode(APP, "android:camera")
                .unwrap(),
            None
        );
        assert!(
            manager
                .package_mode("q", "android:camera")
                .unwrap()
                .is_none()
        );
    });
}

#[test]
fn permission_manager_grant_revoke_updates_persisted_modes() {
    crate::system::with_discovery_session(|host, publish, _| {
        super::super::tests::write_access(host, SYSTEM_ACCESS, b"<access><permissions><permission name=\"android.permission.CAMERA\" packageName=\"p\" protectionLevel=\"1\" type=\"0\"/></permissions></access>");
        super::super::tests::write_access(
            host,
            USER0_ACCESS,
            b"<access><app-id-permissions/></access>",
        );
        let manager = Arc::new(PermissionManager::new(host.clone()));
        assert!(host.permission_manager.set(manager.clone()).is_ok());
        publish(&mut |pkg| {
            if pkg.package_name == "p" {
                pkg.target_sdk_version = 36;
                pkg.requested_permissions = vec![CAMERA.into()];
                pkg.permissions = vec![crate::package::pkg::Permission {
                    component: Component {
                        name: CAMERA.into(),
                        package_name: "p".into(),
                        ..Default::default()
                    },
                    protection_level: 1,
                    ..Default::default()
                }];
            }
        });
        manager
            .synchronize_permission_app_ops(APP, None, true)
            .unwrap();
        assert_eq!(
            manager.uid_mode(APP, "android:camera").unwrap(),
            Some(MODE_IGNORED)
        );
        for granted in [true, false, true] {
            manager
                .mutate(None, |access, after| {
                    let mut state = access.state.clone();
                    state.system.permission_controller_package = Some(None);
                    Access {
                        state: &state,
                        ..*access
                    }
                    .set_runtime_permission_granted(
                        after,
                        &BTreeSet::new(),
                        1000,
                        "p",
                        CAMERA,
                        0,
                        granted,
                    )
                })
                .unwrap();
            let expected = if granted { None } else { Some(MODE_IGNORED) };
            assert_eq!(manager.uid_mode(APP, "android:camera").unwrap(), expected);
            assert_eq!(
                PermissionManager::new(host.clone())
                    .uid_mode(APP, "android:camera")
                    .unwrap(),
                expected
            );
            assert_eq!(
                manager.user0().unwrap().permission_flags(APP, CAMERA) & RUNTIME_GRANTED != 0,
                granted
            );
        }
    });
}
