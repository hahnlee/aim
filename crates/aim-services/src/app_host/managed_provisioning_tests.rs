//! ManagedProvisioning OTA calls at android-16.0.0_r1; no guest process.
use super::*;
use aim_binder_host::parcel::EX_SECURITY;
use aim_service_aidl::{
    android_app_admin_idevicepolicymanager as dp, android_content_pm_icrossprofileapps as cpa,
    android_content_pm_ipackagemanager as pm, android_os_iusermanager as um,
    com_android_internal_app_iappopsservice as ops,
};
use appops_service::{MODE_ALLOWED, MODE_DEFAULT, MODE_IGNORED, OP_INTERACT_ACROSS_PROFILES};

const USER_ACCESS: &str = "misc_de/0/apexdata/com.android.permission/access.abx";
const SYSTEM_ACCESS: &str = "misc/apexdata/com.android.permission/access.abx";
const OP: &str = "android:interact_across_profiles";

fn call(service: &dyn Service, uid: u32, code: u32, args: impl FnOnce(&mut Parcel)) -> Parcel {
    let mut data = Parcel::new();
    args(&mut data);
    service
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 77,
            sender_euid: uid,
            data: data.reader(),
        })
        .unwrap()
}

fn ok(p: &Parcel) {
    p.reader().read_exception().unwrap().unwrap();
}

fn write_access(host: &AppHost, path: &str, xml: &[u8]) {
    let path = host.data().unwrap().join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        aim_android_xml::abx::write(&aim_android_xml::read(xml).unwrap()).unwrap(),
    )
    .unwrap();
}

fn with_session(test: impl FnOnce(&Arc<AppHost>)) {
    crate::system::with_discovery_session_flags(
        &[("android.permission.flags.appop_mode_caching_enabled", true)],
        test,
    );
}

fn setup(host: &Arc<AppHost>) -> Arc<permission_manager::PermissionManager> {
    write_access(host, SYSTEM_ACCESS, b"<access><permissions/></access>");
    write_access(
        host,
        USER_ACCESS,
        b"<access><app-id-permissions/><extension value='retained'/></access>",
    );
    let owner = Arc::new(permission_manager::PermissionManager::new(host.clone()));
    host.permission_manager.set(owner.clone()).ok().unwrap();
    host.package_system()
        .unwrap()
        .set_native_permissions(&owner);
    user_service::ensure_system_user(host).unwrap();
    owner
}

fn disk(host: &AppHost) -> crate::package::permissions::AccessUser {
    let xml =
        aim_android_xml::read(&std::fs::read(host.data().unwrap().join(USER_ACCESS)).unwrap())
            .unwrap();
    assert!(xml.children().any(|e| e.name == "extension"));
    crate::package::permissions::AccessUser::parse(&xml).unwrap()
}

fn set_package(service: &dyn Service, caller: u32, package: &str, mode: i32) -> Parcel {
    call(service, caller, ops::SET_MODE, |p| {
        ops::SetMode {
            code: OP_INTERACT_ACROSS_PROFILES,
            uid: 10100,
            package_name: Some(package.into()),
            mode,
        }
        .write(p)
    })
}

fn set_uid(service: &dyn Service, caller: u32, mode: i32) -> Parcel {
    call(service, caller, ops::SET_UID_MODE, |p| {
        ops::SetUidMode {
            code: OP_INTERACT_ACROSS_PROFILES,
            uid: 10100,
            mode,
        }
        .write(p)
    })
}

/// AppOpsService.setMode: enforce first; package identity, persistence and
/// removal at the default. PackageAppOpPolicy must preserve other state.
#[test]
fn set_mode_checks_permission_and_persists_package_modes() {
    with_session(|host| {
        let owner = setup(host);
        let service = appops_service::AppOps::new(host.clone(), owner.clone());
        assert_eq!(
            set_package(&service, 10100, "p", MODE_ALLOWED)
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        assert!(disk(host).package_app_ops.is_empty());
        ok(&set_package(&service, 1000, "p", MODE_ALLOWED));
        assert_eq!(
            disk(host).package_app_ops,
            vec![("p".into(), vec![(OP.into(), MODE_ALLOWED)])]
        );
        ok(&set_package(&service, 1000, "q", MODE_IGNORED)); // different UID
        assert_eq!(owner.package_mode("q", OP).unwrap(), None);
        let reloaded = permission_manager::PermissionManager::new(host.clone());
        assert_eq!(reloaded.package_mode("p", OP).unwrap(), Some(MODE_ALLOWED));
        ok(&set_package(&service, 1000, "p", MODE_DEFAULT));
        assert!(disk(host).package_app_ops.is_empty());
    });
}

/// AppOpsService.setUidMode / AppIdAppOpPolicy: UID mode takes precedence,
/// MODE_DEFAULT removes it and exposes the package mode again.
#[test]
fn set_uid_mode_checks_permission_and_persists_uid_modes() {
    with_session(|host| {
        let owner = setup(host);
        let service = appops_service::AppOps::new(host.clone(), owner.clone());
        assert_eq!(
            set_uid(&service, 10100, MODE_IGNORED)
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        ok(&set_package(&service, 1000, "p", MODE_ALLOWED));
        ok(&set_uid(&service, 1000, MODE_IGNORED));
        assert_eq!(
            disk(host).app_id_app_ops,
            vec![(10100, vec![(OP.into(), MODE_IGNORED)])]
        );
        assert_eq!(
            appops_service::mode(host, OP_INTERACT_ACROSS_PROFILES, 10100, Some("p")).unwrap(),
            MODE_IGNORED
        );
        let reloaded = permission_manager::PermissionManager::new(host.clone());
        assert_eq!(reloaded.uid_mode(10100, OP).unwrap(), Some(MODE_IGNORED));
        ok(&set_uid(&service, 1000, MODE_DEFAULT));
        assert_eq!(
            appops_service::mode(host, OP_INTERACT_ACROSS_PROFILES, 10100, Some("p")).unwrap(),
            MODE_ALLOWED
        );
        assert!(disk(host).app_id_app_ops.is_empty());
    });
}

/// CrossProfileAppsServiceImpl.canUserAttemptToConfigure... exits before
/// package/allowlist access with < 2 profiles, including unknown users.
#[test]
fn can_configure_returns_false_before_package_or_allowlist_queries() {
    with_session(|host| {
        setup(host);
        let service = cross_profile_apps::CrossProfileApps::new(host.clone());
        for user in [0, 10] {
            for package in [Some("p"), Some("absent"), None] {
                let r = call(
                    &service,
                    10100,
                    cpa::CAN_CONFIGURE_INTERACT_ACROSS_PROFILES,
                    |p| {
                        cpa::CanConfigureInteractAcrossProfiles {
                            user_id: user,
                            package_name: package.map(str::to_owned),
                        }
                        .write(p)
                    },
                );
                assert_eq!(
                    cpa::read_can_configure_interact_across_profiles_reply(&mut r.reader())
                        .unwrap(),
                    Ok(false)
                );
            }
        }
    });
}

/// An empty list has no authorization checks. Each removed package checks
/// both permission groups even when absent; an installed one's UID is reset.
#[test]
fn reset_interact_across_profiles_app_ops_authorizes_and_changes_real_state() {
    with_session(|host| {
        let owner = setup(host);
        let ops = appops_service::AppOps::new(host.clone(), owner.clone());
        let service = cross_profile_apps::CrossProfileApps::new(host.clone());
        let reset = |uid, packages: &[&str]| {
            call(
                &service,
                uid,
                cpa::RESET_INTERACT_ACROSS_PROFILES_APP_OPS,
                |p| {
                    cpa::ResetInteractAcrossProfilesAppOps {
                        user_id: 0,
                        package_names: Some(packages.iter().map(|s| Some((*s).into())).collect()),
                    }
                    .write(p)
                },
            )
        };
        ok(&reset(10100, &[]));
        assert_eq!(
            reset(10100, &["absent"])
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        ok(&set_uid(&ops, 1000, MODE_ALLOWED));
        ok(&reset(1000, &["absent", "p"]));
        assert_eq!(owner.uid_mode(10100, OP).unwrap(), None);
        assert!(disk(host).app_id_app_ops.is_empty());
    });
}

#[test]
fn reset_requires_both_permission_groups_and_accepts_the_original_alternatives() {
    const CROSS: &str = "android.permission.INTERACT_ACROSS_USERS";
    const FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
    const CONFIGURE: &str = "android.permission.CONFIGURE_INTERACT_ACROSS_PROFILES";
    const MANAGE: &str = "android.permission.MANAGE_APP_OPS_MODES";
    for granted in [
        vec![],
        vec![CROSS],
        vec![CONFIGURE],
        vec![MANAGE],
        vec![FULL],
        vec![CROSS, CONFIGURE],
        vec![FULL, CONFIGURE],
        vec![CROSS, MANAGE],
        vec![FULL, MANAGE],
    ] {
        with_session(|host| {
            setup(host);
            let definitions = [CROSS, FULL, CONFIGURE, MANAGE].iter().map(|name|
                format!("<permission name='{name}' packageName='android' protectionLevel='2' type='0'/>"))
                .collect::<String>();
            write_access(
                host,
                SYSTEM_ACCESS,
                format!("<access><permissions>{definitions}</permissions></access>").as_bytes(),
            );
            let permissions = granted
                .iter()
                .map(|name| format!("<permission name='{name}' flags='4'/>"))
                .collect::<String>();
            write_access(host, USER_ACCESS, format!("<access><app-id-permissions><app-id id='10100'>{permissions}</app-id></app-id-permissions></access>").as_bytes());
            let service = cross_profile_apps::CrossProfileApps::new(host.clone());
            let r = call(
                &service,
                10100,
                cpa::RESET_INTERACT_ACROSS_PROFILES_APP_OPS,
                |p| {
                    cpa::ResetInteractAcrossProfilesAppOps {
                        user_id: 0,
                        package_names: Some(vec![Some("absent".into())]),
                    }
                    .write(p)
                },
            );
            let result = r.reader().read_exception().unwrap();
            if granted.len() == 2 {
                result.unwrap();
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.code, EX_SECURITY);
                let expected_group = if granted.contains(&CROSS) || granted.contains(&FULL) {
                    "MANAGE_APP_OPS_MODES"
                } else {
                    "INTERACT_ACROSS_USERS"
                };
                assert!(error.message.starts_with(expected_group));
            }
        });
    }
}

/// The resource query works independently of malformed package/policy files.
#[test]
fn get_default_cross_profile_packages_reads_the_configured_framework() {
    let root = aim_paths::original_image_with("system/framework/framework-res.apk")
        .expect("pinned framework resources");
    let data = test_temp_dir("default-cross-profile");
    std::fs::create_dir_all(data.join("system")).unwrap();
    for file in ["packages.xml", "device_policies.xml", "device_owner_2.xml"] {
        std::fs::write(data.join("system").join(file), b"invalid XML").unwrap();
    }
    let host = AppHost::new(crate::test_process(10100));
    configure_test_host(
        &host,
        &root,
        &data,
        &data,
        Box::new(|name| {
            Some(
                match name {
                    "ro.build.version.sdk" => "36",
                    "ro.build.version.codename" => "REL",
                    "persist.sys.locale" => "en-US",
                    "ro.sf.lcd_density" => "320",
                    _ => return None,
                }
                .into(),
            )
        }),
    );
    let service = device_policy::DevicePolicy::new(host.clone());
    let r = call(
        &service,
        10100,
        dp::GET_DEFAULT_CROSS_PROFILE_PACKAGES,
        |p| dp::GetDefaultCrossProfilePackages {}.write(p),
    );
    let actual = dp::read_get_default_cross_profile_packages_reply(&mut r.reader())
        .unwrap()
        .unwrap()
        .unwrap();
    let framework = host.framework().unwrap();
    let expected: HashSet<_> = ["cross_profile_apps", "vendor_cross_profile_apps"]
        .into_iter()
        .flat_map(|name| framework.framework_string_array(name).unwrap())
        .collect();
    assert_eq!(actual.len(), expected.len());
    assert_eq!(actual.into_iter().collect::<HashSet<_>>(), expected);
    std::fs::remove_dir_all(data).unwrap();
}

struct User(i32, i32);
impl aim_service_aidl::ReadParcelable for User {
    fn read_from(
        r: &mut aim_binder_host::parcel::Reader<'_>,
    ) -> aim_binder_host::parcel::Result<Self> {
        let id = r.read_i32()?;
        r.read_string8()?;
        r.read_string8()?;
        let flags = r.read_i32()?;
        r.read_string8()?;
        r.read_i32()?;
        r.read_i64()?;
        r.read_i64()?;
        r.read_string8()?;
        r.read_bool()?;
        r.read_bool()?;
        r.read_i32()?;
        r.read_bool()?;
        r.read_i32()?;
        r.read_i32()?;
        Ok(User(id, flags))
    }
}

/// OtaController.run -> MigrateSystemAppsSnapshotTask -> user/owner guards
/// -> UpdateInteractAcrossProfilesAppOpTask. Exercise fresh and changed
/// snapshots and the non-default package-mode migration, not just dispatch.
/// The package context uses fixture p; Android uses its framework package.
#[test]
fn ota_controller_single_user_call_list() {
    let root = aim_paths::original_image_with("system/framework/framework-res.apk")
        .expect("pinned framework resources");
    with_session(|host| {
        host.framework
            .set(resources::Framework::load(&root, 160, "en-US").unwrap())
            .ok()
            .unwrap();
        let owner = setup(host);
        let policy = device_policy::DevicePolicy::new(host.clone());
        let users = user_service::UserManager::new(host.clone());
        let profiles = cross_profile_apps::CrossProfileApps::new(host.clone());
        let packages = package_service::PackageService::new(host.clone());
        let appops = appops_service::AppOps::new(host.clone(), owner);

        // MetricsWriterFactory selects its writer before OtaController.run.
        use aim_service_aidl::android_content_icontentprovider as cp;
        let settings = settings_provider::SettingsProvider::new(host.clone());
        let r = call(&settings, 1000, cp::CALL_TRANSACTION, |p| {
            p.write_interface_token(cp::DESCRIPTOR);
            p.write_i32(4); // empty size-delimited AttributionSourceState
            p.write_string16(Some("settings"));
            p.write_string16(Some("GET_secure"));
            p.write_string16(Some("user_setup_complete"));
            p.write_i32(-1); // no extras
        });
        let mut r = r.reader();
        r.read_exception().unwrap().unwrap();
        let value = crate::bundle::read(&mut r).unwrap().unwrap();
        assert!(
            matches!(value.get("value"), Some(crate::bundle::Value::String(Some(value))) if value == "1")
        );

        // Legacy snapshot migration and each CrossProfileAppsSnapshot file.
        let r = call(&users, 1000, um::GET_USER_SERIAL_NUMBER, |p| {
            um::GetUserSerialNumber { user_id: 0 }.write(p)
        });
        assert_eq!(
            um::read_get_user_serial_number_reply(&mut r.reader()).unwrap(),
            Ok(0)
        );
        let r = call(&policy, 1000, dp::GET_DEVICE_OWNER_USER_ID, |p| {
            dp::GetDeviceOwnerUserId {}.write(p)
        });
        assert_eq!(
            dp::read_get_device_owner_user_id_reply(&mut r.reader()).unwrap(),
            Ok(-10000)
        );
        let r = call(&users, 1000, um::GET_USERS, |p| {
            um::GetUsers {
                exclude_partial: true,
                exclude_dying: false,
                exclude_pre_created: true,
            }
            .write(p)
        });
        let listed = um::read_get_users_reply::<User>(&mut r.reader())
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(listed.len(), 1);
        let User(id, flags) = listed.into_iter().next().unwrap().unwrap();
        assert_eq!(id, 0);
        assert_eq!(flags & (0x20 | 0x1000), 0); // managed profile / profile
        let r = call(&policy, 1000, dp::GET_PROFILE_OWNER_AS_USER, |p| {
            dp::GetProfileOwnerAsUser { user_handle: id }.write(p)
        });
        assert!(
            dp::read_get_profile_owner_as_user_reply::<package::intent::ComponentName>(
                &mut r.reader()
            )
            .unwrap()
            .unwrap()
            .is_none()
        );
        ok(&call(
            &policy,
            1000,
            dp::RESET_DEFAULT_CROSS_PROFILE_INTENT_FILTERS,
            |p| dp::ResetDefaultCrossProfileIntentFilters { user_id: id }.write(p),
        ));

        let r = call(&policy, 1000, dp::GET_DEFAULT_CROSS_PROFILE_PACKAGES, |p| {
            dp::GetDefaultCrossProfilePackages {}.write(p)
        });
        let defaults = dp::read_get_default_cross_profile_packages_reply(&mut r.reader())
            .unwrap()
            .unwrap()
            .unwrap();
        for package in defaults {
            let r = call(
                &profiles,
                1000,
                cpa::CAN_CONFIGURE_INTERACT_ACROSS_PROFILES,
                |p| {
                    cpa::CanConfigureInteractAcrossProfiles {
                        user_id: id,
                        package_name: package,
                    }
                    .write(p)
                },
            );
            assert_eq!(
                cpa::read_can_configure_interact_across_profiles_reply(&mut r.reader()).unwrap(),
                Ok(false)
            );
        }
        // A changed OEM list: Java filters previous minus current before this call.
        ok(&set_uid(&appops, 1000, MODE_ALLOWED));
        ok(&call(
            &profiles,
            1000,
            cpa::RESET_INTERACT_ACROSS_PROFILES_APP_OPS,
            |p| {
                cpa::ResetInteractAcrossProfilesAppOps {
                    user_id: id,
                    package_names: Some(vec![Some("p".into())]),
                }
                .write(p)
            },
        ));

        let r = call(&users, 1000, um::GET_PROFILE_IDS, |p| {
            um::GetProfileIds {
                user_id: id,
                enabled_only: false,
            }
            .write(p)
        });
        let group = um::read_get_profile_ids_reply(&mut r.reader())
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(group, vec![id]);
        let r = call(&packages, 1000, pm::GET_APPLICATION_INFO, |p| {
            pm::GetApplicationInfo {
                package_name: Some("p".into()),
                flags: 0,
                user_id: id,
            }
            .write(p)
        });
        ok(&r);
        assert_eq!(
            {
                let mut r = r.reader();
                r.read_exception().unwrap().unwrap();
                r.read_i32().unwrap()
            },
            1
        );
        let r = call(&packages, 1000, pm::GET_INSTALLED_APPLICATIONS, |p| {
            pm::GetInstalledApplications {
                flags: 0,
                user_id: id,
            }
            .write(p)
        });
        let mut applications = r.reader();
        applications.read_exception().unwrap().unwrap();
        assert_eq!(applications.read_i32().unwrap(), 1); // typed slice
        assert!(applications.read_i32().unwrap() >= 2); // fixture p and q

        // ReapplyCrossProfileAppsPermission visits all installed packages.
        // Seed the Android 11 package-mode bug, then walk its UID migration.
        ok(&set_package(&appops, 1000, "p", MODE_ALLOWED));
        let check = |code| {
            call(&appops, 1000, code, |p| {
                ops::CheckOperationForDevice {
                    code: OP_INTERACT_ACROSS_PROFILES,
                    uid: 10100,
                    package_name: Some("p".into()),
                    attribution_tag: None,
                    virtual_device_id: 0,
                }
                .write(p);
            })
        };
        let r = check(ops::CHECK_OPERATION_FOR_DEVICE);
        assert_eq!(
            ops::read_check_operation_for_device_reply(&mut r.reader()).unwrap(),
            Ok(MODE_ALLOWED)
        );
        let r = call(&packages, 1000, pm::GET_PACKAGES_FOR_UID, |p| {
            pm::GetPackagesForUid { uid: 10100 }.write(p)
        });
        let shared = pm::read_get_packages_for_uid_reply(&mut r.reader())
            .unwrap()
            .unwrap()
            .unwrap();
        for package in shared.into_iter().flatten() {
            let r = call(
                &profiles,
                1000,
                cpa::CAN_CONFIGURE_INTERACT_ACROSS_PROFILES,
                |p| {
                    cpa::CanConfigureInteractAcrossProfiles {
                        user_id: id,
                        package_name: Some(package.clone()),
                    }
                    .write(p)
                },
            );
            assert_eq!(
                cpa::read_can_configure_interact_across_profiles_reply(&mut r.reader()).unwrap(),
                Ok(false)
            );
            ok(&set_package(&appops, 1000, &package, MODE_DEFAULT));
            ok(&set_uid(&appops, 1000, MODE_DEFAULT));
        }
        let r = check(ops::CHECK_OPERATION_FOR_DEVICE);
        assert_eq!(
            ops::read_check_operation_for_device_reply(&mut r.reader()).unwrap(),
            Ok(MODE_DEFAULT)
        );
        let r = call(&appops, 1000, ops::CHECK_OPERATION_RAW_FOR_DEVICE, |p| {
            ops::CheckOperationRawForDevice {
                code: OP_INTERACT_ACROSS_PROFILES,
                uid: 10100,
                package_name: Some("p".into()),
                attribution_tag: None,
                virtual_device_id: 0,
            }
            .write(p)
        });
        assert_eq!(
            ops::read_check_operation_raw_for_device_reply(&mut r.reader()).unwrap(),
            Ok(MODE_DEFAULT)
        );
        assert!(disk(host).app_id_app_ops.is_empty());
        assert!(disk(host).package_app_ops.is_empty());
    });
}
