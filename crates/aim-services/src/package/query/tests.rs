use std::collections::BTreeMap;
use std::sync::Arc;

use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_SECURITY, Parcel, Reader};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;
use aim_service_aidl::android_content_pm_ipackagemanagernative as native;

use super::*;
use crate::package::model::{PackageUserState, SharedUser, System};
use crate::package::pkg::{Activity, Component, MetaData, SigningDetails, Value as Meta};

const APP: &str = "org.example.app";
const OTHER: &str = "org.example.other";

fn package(
    name: &str,
    app_id: i32,
    edit: impl FnOnce(&mut PackageState, &mut AndroidPackage),
) -> PackageState {
    let mut ps = PackageState {
        name: name.into(),
        app_id,
        target_sdk_version: 35,
        last_update_time: 0x18f0,
        ..PackageState::default()
    };
    let mut pkg = AndroidPackage {
        package_name: name.into(),
        uid: app_id,
        target_sdk_version: 35,
        version_code: 7,
        install_location: -1,
        booleans: booleans::ENABLED | booleans::HAS_CODE | booleans::ALLOW_BACKUP,
        base_apk_path: Some(format!("/data/app/{name}/base.apk")),
        path: Some(format!("/data/app/{name}")),
        signing_details: Some(SigningDetails {
            signatures: Some(vec![vec![7, 7]]),
            scheme_version: 3,
            past_signing_certificates: None,
        }),
        ..AndroidPackage::default()
    };
    ps.users.insert(
        0,
        PackageUserState {
            first_install_time: 0x1234,
            ..PackageUserState::default()
        },
    );
    edit(&mut ps, &mut pkg);
    ps.pkg = Some(Arc::new(pkg));
    ps
}

fn activity(name: &str) -> Activity {
    let mut a = Activity::default();
    a.main.exported = true;
    a.main.enabled = true;
    a.main.component = Component {
        name: name.into(),
        package_name: APP.into(),
        meta_data: Some(MetaData(vec![("k".into(), Meta::Int(3))])),
        ..Component::default()
    };
    a
}

fn state() -> State {
    let packages = [
        package("android", 1000, |ps, p| {
            ps.is.system = true;
            ps.shared_user = Some("android.uid.system".into());
            p.booleans |= booleans::SYSTEM;
            p.shared_user_id = Some("android.uid.system".into());
            p.signing_details.as_mut().unwrap().signatures = Some(vec![vec![1]]);
            let u = ps.users.get_mut(&0).unwrap();
            u.granted_permissions = vec![INTERACT_ACROSS_USERS_FULL.into()];
        }),
        package(APP, 10100, |ps, p| {
            p.activities = vec![
                activity("org.example.app.Main"),
                activity("org.example.app.B"),
            ];
            p.meta_data = Some(MetaData(vec![("m".into(), Meta::String(Some("v".into())))]));
            let u = ps.users.get_mut(&0).unwrap();
            u.disabled_components = vec!["org.example.app.B".into()];
        }),
        package(OTHER, 10101, |_, _| {}),
    ];
    let mut users = BTreeMap::new();
    for id in [0, 10] {
        users.insert(
            id,
            User {
                id,
                unlocking_or_unlocked: true,
                ..User::default()
            },
        );
    }
    State {
        packages: packages.into_iter().map(|p| (p.name.clone(), p)).collect(),
        shared_users: [(
            "android.uid.system".to_string(),
            SharedUser {
                name: "android.uid.system".into(),
                app_id: 1000,
                packages: vec!["android".into()],
                ..SharedUser::default()
            },
        )]
        .into(),
        users,
        system: System {
            features: vec![
                ("android.hardware.camera".into(), 0),
                ("android.hardware.vulkan.version".into(), 4206592),
            ],
            gl_es_version: 0x30000,
            ..System::default()
        },
        ..State::default()
    }
}

/// Answers a call of `uid` to `package` or `package_native`.
fn call(
    state: &State,
    uid: i32,
    native_call: bool,
    code: u32,
    write: impl FnOnce(&mut Parcel),
) -> Parcel {
    let filter = AppsFilter::new(state, &Config::default());
    let q = Query {
        state,
        filter: &filter,
        calling_uid: uid,
    };
    let mut p = Parcel::new();
    write(&mut p);
    let mut r = Reader::new(p.data(), p.objects());
    if native_call {
        q.native(code, &mut r).unwrap()
    } else {
        q.package(code, &mut r).unwrap()
    }
}

/// A reply decoded as the comparison decodes it, read to its end.
fn decoded(code: u32, reply: &Parcel) -> Value {
    let mut r = Reader::new(reply.data(), reply.objects());
    let v = reply::decode(pm::DESCRIPTOR, code, &mut r)
        .unwrap()
        .unwrap();
    assert_eq!(r.remaining(), 0);
    v
}

fn field<'v>(v: &'v Value, name: &str) -> &'v Value {
    let Value::Fields(fields) = v else {
        panic!("{v:?} has no fields");
    };
    &fields.iter().find(|(n, _)| n == name).unwrap().1
}

fn get_application_info(state: &State, uid: i32, name: &str, flags: i64) -> Value {
    let reply = call(state, uid, false, pm::GET_APPLICATION_INFO, |p| {
        pm::GetApplicationInfo {
            package_name: Some(name.into()),
            flags,
            user_id: 0,
        }
        .write(p)
    });
    decoded(pm::GET_APPLICATION_INFO, &reply)
}

#[test]
fn application_info_for_the_user() {
    let state = state();
    let ai = get_application_info(&state, 1000, APP, 0);
    assert_eq!(field(&ai, "uid"), &Value::Int(10100));
    assert_eq!(
        field(&ai, "dataDir"),
        &Value::Str("/data/user/0/org.example.app".into())
    );
    assert_eq!(
        field(&ai, "deviceProtectedDataDir"),
        &Value::Str("/data/user_de/0/org.example.app".into())
    );
    assert_eq!(field(&ai, "seInfoUser"), &Value::Str(":complete".into()));
    assert_eq!(field(&ai, "metaData"), &Value::Null);
    let Value::Int(flags) = field(&ai, "flags") else {
        panic!()
    };
    // Installed, with code, backup allowed and every screen supported.
    assert_eq!(
        *flags & ((1 << 23) | (1 << 2) | (1 << 15)),
        (1 << 23) | (1 << 2) | (1 << 15)
    );
    assert_eq!(field(&ai, "processName"), &Value::Str(APP.into()));
    assert_eq!(field(&ai, "enabled"), &Value::Bool(true));

    let ai = get_application_info(&state, 1000, APP, GET_META_DATA);
    assert_eq!(
        field(&ai, "metaData"),
        &Value::Fields(vec![("m".into(), Value::Str("v".into()))])
    );
    // The system's data directory.
    let ai = get_application_info(&state, 1000, "android", 0);
    assert_eq!(field(&ai, "dataDir"), &Value::Str("/data/system".into()));
    assert_eq!(field(&ai, "credentialProtectedDataDir"), &Value::Null);
}

#[test]
fn package_info_squashes_its_application_info() {
    let state = state();
    let reply = call(&state, 1000, false, pm::GET_PACKAGE_INFO, |p| {
        pm::GetPackageInfo {
            package_name: Some(APP.into()),
            flags: GET_ACTIVITIES | GET_META_DATA | GET_SIGNATURES,
            user_id: 0,
        }
        .write(p)
    });
    let pi = decoded(pm::GET_PACKAGE_INFO, &reply);
    let app = field(&pi, "applicationInfo");
    let Value::List(activities) = field(&pi, "activities") else {
        panic!()
    };
    // The disabled activity is left out; the other shares the application's info.
    assert_eq!(activities.len(), 1);
    assert_eq!(field(&activities[0], "applicationInfo"), app);
    assert_eq!(
        field(&activities[0], "name"),
        &Value::Str("org.example.app.Main".into())
    );
    assert_eq!(field(&pi, "firstInstallTime"), &Value::Long(0x1234));
    assert_eq!(
        field(&pi, "signatures"),
        &Value::List(vec![Value::Bytes(vec![7, 7])])
    );
}

#[test]
fn packages_are_filtered_by_visibility() {
    let state = state();
    let as_other = get_application_info(&state, 10101, APP, 0);
    assert_eq!(as_other, Value::Null);
    let as_self = get_application_info(&state, 10100, APP, 0);
    assert_ne!(as_self, Value::Null);
    let mut queries = state.clone();
    let other = queries.packages.get_mut(OTHER).unwrap();
    let mut pkg = (**other.pkg.as_ref().unwrap()).clone();
    pkg.queries_packages = vec![APP.into()];
    other.pkg = Some(Arc::new(pkg));
    assert_ne!(get_application_info(&queries, 10101, APP, 0), Value::Null);
}

#[test]
fn enabled_settings_and_unknown_components() {
    let state = state();
    let get = |class: &str, package: &str| {
        let reply = call(
            &state,
            10100,
            false,
            pm::GET_COMPONENT_ENABLED_SETTING,
            |p| {
                pm::GetComponentEnabledSetting {
                    component_name: Some(Named(package.into(), class.into())),
                    user_id: 0,
                }
                .write(p)
            },
        );
        let mut r = Reader::new(reply.data(), reply.objects());
        pm::read_get_component_enabled_setting_reply(&mut r).unwrap()
    };
    assert_eq!(
        get("org.example.app.B", APP),
        Ok(COMPONENT_ENABLED_STATE_DISABLED)
    );
    assert_eq!(
        get("org.example.app.Main", APP),
        Ok(COMPONENT_ENABLED_STATE_DEFAULT)
    );
    let e = get("X", "org.example.none").unwrap_err();
    assert_eq!(e.code, EX_ILLEGAL_ARGUMENT);
    assert_eq!(
        e.message,
        "Unknown component: ComponentInfo{org.example.none/X}"
    );
}

/// A `ComponentName` as a caller writes it.
struct Named(String, String);

impl aim_service_aidl::WriteParcelable for Named {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some(&self.0));
        p.write_string16(Some(&self.1));
    }
}

#[test]
fn signatures_uids_and_names() {
    let state = state();
    let check = |a: &str, b: &str| {
        let reply = call(&state, 1000, false, pm::CHECK_SIGNATURES, |p| {
            pm::CheckSignatures {
                pkg1: Some(a.into()),
                pkg2: Some(b.into()),
                user_id: 0,
            }
            .write(p)
        });
        let mut r = Reader::new(reply.data(), reply.objects());
        pm::read_check_signatures_reply(&mut r).unwrap().unwrap()
    };
    assert_eq!(check(APP, OTHER), SIGNATURE_MATCH);
    assert_eq!(check(APP, "android"), SIGNATURE_NO_MATCH);
    assert_eq!(check(APP, "org.example.none"), SIGNATURE_UNKNOWN_PACKAGE);

    let reply = call(&state, 10100, false, pm::GET_NAME_FOR_UID, |p| {
        pm::GetNameForUid { uid: 1000 }.write(p)
    });
    let mut r = Reader::new(reply.data(), reply.objects());
    assert_eq!(
        pm::read_get_name_for_uid_reply(&mut r)
            .unwrap()
            .unwrap()
            .as_deref(),
        Some("android.uid.system:1000")
    );
    let reply = call(&state, 1000, true, native::GET_NAMES_FOR_UIDS, |p| {
        native::GetNamesForUids {
            uids: Some(vec![1000, 10100, 10999]),
        }
        .write(p)
    });
    let mut r = Reader::new(reply.data(), reply.objects());
    assert_eq!(
        native::read_get_names_for_uids_reply(&mut r)
            .unwrap()
            .unwrap(),
        Some(vec![
            Some("shared:android.uid.system".into()),
            Some(APP.into()),
            Some(String::new())
        ])
    );
    let reply = call(&state, 10100, false, pm::GET_PACKAGES_FOR_UID, |p| {
        pm::GetPackagesForUid { uid: 1000 }.write(p)
    });
    let mut r = Reader::new(reply.data(), reply.objects());
    assert_eq!(
        pm::read_get_packages_for_uid_reply(&mut r)
            .unwrap()
            .unwrap(),
        Some(vec![Some("android".into())])
    );
}

#[test]
fn features_and_cross_user_calls() {
    let state = state();
    let has = |name: &str, version: i32| {
        let reply = call(&state, 10100, false, pm::HAS_SYSTEM_FEATURE, |p| {
            pm::HasSystemFeature {
                name: Some(name.into()),
                version,
            }
            .write(p)
        });
        let mut r = Reader::new(reply.data(), reply.objects());
        pm::read_has_system_feature_reply(&mut r).unwrap().unwrap()
    };
    assert!(has("android.hardware.vulkan.version", 4206592));
    assert!(!has("android.hardware.vulkan.version", 4206593));
    assert!(!has("android.hardware.nfc", 0));

    // Another user's package needs INTERACT_ACROSS_USERS.
    let reply = call(&state, 10100, false, pm::GET_PACKAGE_INFO, |p| {
        pm::GetPackageInfo {
            package_name: Some(APP.into()),
            flags: 0,
            user_id: 10,
        }
        .write(p)
    });
    let mut r = Reader::new(reply.data(), reply.objects());
    let e = r.read_exception().unwrap().unwrap_err();
    assert_eq!(e.code, EX_SECURITY);
    assert_eq!(
        e.message,
        "get package info: UID 10100 requires android.permission.INTERACT_ACROSS_USERS_FULL \
         or android.permission.INTERACT_ACROSS_USERS to access user 10."
    );
    // The system crosses users; a user without a state of the package
    // has the default one, installed.
    let reply = call(&state, 1000, false, pm::GET_PACKAGE_INFO, |p| {
        pm::GetPackageInfo {
            package_name: Some(APP.into()),
            flags: 0,
            user_id: 10,
        }
        .write(p)
    });
    let pi = decoded(pm::GET_PACKAGE_INFO, &reply);
    assert_eq!(
        field(field(&pi, "applicationInfo"), "uid"),
        &Value::Int(1010100)
    );
}

#[test]
fn installed_packages_in_the_original_order() {
    let state = state();
    let reply = call(&state, 1000, false, pm::GET_INSTALLED_PACKAGES, |p| {
        pm::GetInstalledPackages {
            flags: 0,
            user_id: 0,
        }
        .write(p)
    });
    let list = decoded(pm::GET_INSTALLED_PACKAGES, &reply);
    let Value::List(items) = field(&list, "items") else {
        panic!()
    };
    let names: Vec<&Value> = items.iter().map(|i| field(i, "packageName")).collect();
    // ArrayMap order: by String.hashCode.
    let mut expected = ["android", APP, OTHER];
    expected.sort_by_key(|n| info::java_hash(n));
    let expected: Vec<Value> = expected.iter().map(|n| Value::Str(n.to_string())).collect();
    assert_eq!(names, expected.iter().collect::<Vec<_>>());
}
