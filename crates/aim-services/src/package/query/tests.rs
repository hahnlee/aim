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
            ..SigningDetails::default()
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
    let filter = AppsFilter::new(state, &crate::package::apps_filter::Config::default()).unwrap();
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

#[test]
fn library_optional_owners_preserve_raw_null_empty_and_nested_values() {
    use crate::package::model::SharedLibrary;
    let populated = SharedLibrary {
        optional_dependents: Some(vec![None, Some(("consumer".into(), i64::MAX))]),
        cert_digests: Some(vec![None, Some("digest".into())]),
        ..Default::default()
    };
    let libraries = vec![
        SharedLibrary::default(),
        SharedLibrary {
            optional_dependents: Some(vec![]),
            cert_digests: Some(vec![]),
            ..Default::default()
        },
        SharedLibrary {
            dependencies: vec![None, Some(populated.clone())],
            dependents: vec![None, Some(("dependent".into(), 5))],
            code_paths: Some(vec![None, Some("/system/raw.jar".into())]),
            ..populated
        },
    ];
    let info = info::ApplicationInfo {
        shared_library_infos: Some(libraries),
        ..Default::default()
    };
    let mut reply = Parcel::new();
    reply.write_no_exception();
    reply.write_i32(1);
    info.write(&mut reply, None);
    let decoded = decoded(pm::GET_APPLICATION_INFO, &reply);
    let Value::List(libraries) = field(&decoded, "sharedLibraryInfos") else {
        panic!("missing libraries")
    };
    for name in ["optionalDependentPackages", "certDigests"] {
        assert_eq!(field(&libraries[0], name), &Value::Null);
        assert_eq!(field(&libraries[1], name), &Value::List(vec![]));
    }
    assert_eq!(
        field(&libraries[2], "codePaths"),
        &Value::List(vec![Value::Null, Value::Str("/system/raw.jar".into())])
    );
    assert_eq!(
        field(&libraries[2], "dependentPackages"),
        &Value::List(vec![
            Value::Null,
            Value::List(vec![Value::Str("dependent".into()), Value::Long(5)])
        ])
    );
    let optional = Value::List(vec![
        Value::Null,
        Value::List(vec![Value::Str("consumer".into()), Value::Long(i64::MAX)]),
    ]);
    let certificates = Value::List(vec![Value::Null, Value::Str("digest".into())]);
    assert_eq!(field(&libraries[2], "optionalDependentPackages"), &optional);
    assert_eq!(field(&libraries[2], "certDigests"), &certificates);
    let Value::List(dependencies) = field(&libraries[2], "dependencies") else {
        panic!("missing nested dependency")
    };
    assert_eq!(dependencies[0], Value::Null);
    assert_eq!(
        field(&dependencies[1], "optionalDependentPackages"),
        &optional
    );
    assert_eq!(field(&dependencies[1], "certDigests"), &certificates);
}

#[test]
fn queued_comparison_uses_publication_history_for_intermediate_apk_state() {
    let mut first = state();
    let ps = first.packages.get_mut(APP).unwrap();
    ps.category_override = -1;
    Arc::make_mut(ps.pkg.as_mut().unwrap()).category = -1;
    let first = Arc::new(first);
    let mut intermediate = (*first).clone();
    Arc::make_mut(
        intermediate
            .packages
            .get_mut(APP)
            .unwrap()
            .pkg
            .as_mut()
            .unwrap(),
    )
    .version_code += 1;
    let intermediate = Arc::new(intermediate);
    let mut latest = (*intermediate).clone();
    latest.packages.get_mut(APP).unwrap().category_override = 7;
    let latest = Arc::new(latest);
    let current = latest.clone();
    let prior = intermediate.clone();
    let model = PackageModel::from_states(
        Box::new(move |_, _| Some(current.clone())),
        Some(Box::new(move |_, context| {
            assert_eq!(context.packages[APP].pkg.as_ref().unwrap().version_code, 7);
            Some(prior.clone())
        })),
    );
    model.writes.observe(&first, 0);
    let sent = Instant::now();
    let mut parcel = Parcel::new();
    pm::GetApplicationInfo {
        package_name: Some(APP.into()),
        flags: 0,
        user_id: 0,
    }
    .write(&mut parcel);
    let make_call = || ShadowCall {
        service: "package",
        descriptor: pm::DESCRIPTOR,
        code: pm::GET_APPLICATION_INFO,
        flags: 0,
        sender_pid: 1,
        sender_euid: 1000,
        seq: 1,
        sent,
        dropped: 0,
        data: Reader::new(parcel.data(), parcel.objects()),
    };
    let Answer::Reply(current) = model.answer(&mut make_call()) else {
        panic!("current query was not answered")
    };
    assert_eq!(
        decoded(pm::GET_APPLICATION_INFO, &current),
        get_application_info(&latest, 1000, APP, 0)
    );
    let Answer::Reply(before) = model.answer_before(&mut make_call()).unwrap() else {
        panic!("previous query was not answered")
    };
    let expected = get_application_info(&intermediate, 1000, APP, 0);
    assert_eq!(decoded(pm::GET_APPLICATION_INFO, &before), expected);
    assert_ne!(expected, get_application_info(&first, 1000, APP, 0));
}

#[test]
fn native_uid_slots_preserve_detached_and_shared_member_states() {
    use crate::package::model::UidOwner;
    let mut state = state();
    let mut old = state.packages[APP].clone();
    old.app_id = 10102;
    old.users.get_mut(&0).unwrap().installed = false;
    state.uid_owners = Some(
        [
            (10102, UidOwner::Package(Box::new(old.clone()))),
            (1000, UidOwner::SharedUser("android.uid.system".into())),
        ]
        .into(),
    );
    let mut member = old;
    member.users.get_mut(&0).unwrap().installed = true;
    state
        .shared_users
        .get_mut("android.uid.system")
        .unwrap()
        .native_packages = Some(vec![member]);
    let packages = |uid| {
        let reply = call(&state, 1000, false, pm::GET_PACKAGES_FOR_UID, |p| {
            pm::GetPackagesForUid { uid }.write(p)
        });
        let mut r = Reader::new(reply.data(), reply.objects());
        pm::read_get_packages_for_uid_reply(&mut r)
            .unwrap()
            .unwrap()
    };
    assert_eq!(packages(10100), None);
    assert_eq!(packages(10102), None);
    assert_eq!(packages(1000), Some(vec![Some(APP.into())]));
    let reply = call(&state, 1000, true, native::GET_NAMES_FOR_UIDS, |p| {
        native::GetNamesForUids {
            uids: Some(vec![10100, 10102, 1000]),
        }
        .write(p)
    });
    let mut r = Reader::new(reply.data(), reply.objects());
    assert_eq!(
        native::read_get_names_for_uids_reply(&mut r)
            .unwrap()
            .unwrap(),
        Some(vec![
            Some(String::new()),
            Some(APP.into()),
            Some("shared:android.uid.system".into())
        ])
    );
}

#[test]
fn additional_queries_preserve_user_visibility_and_permission_selection() {
    let mut s = state();
    let ps = s.packages.get_mut(APP).unwrap();
    ps.is.privileged = true;
    ps.mime_groups = vec![(Some("images".into()), vec![Some("image/png".into())])];
    let u = ps.users.get_mut(&0).unwrap();
    u.gids = vec![3003, 3003];
    u.install_reason = 4;
    u.granted_permissions = vec!["p.granted".into()];
    let available = call(&s, 1000, false, pm::IS_PACKAGE_AVAILABLE, |p| pm::IsPackageAvailable { package_name: Some(APP.into()), user_id: 0 }.write(p));
    assert_eq!(pm::read_is_package_available_reply(&mut Reader::new(available.data(), available.objects())).unwrap().unwrap(), true);
    let gids = call(&s, 1000, false, pm::GET_PACKAGE_GIDS, |p| pm::GetPackageGids { package_name: Some(APP.into()), flags: 0, user_id: 0 }.write(p));
    assert_eq!(pm::read_get_package_gids_reply(&mut Reader::new(gids.data(), gids.objects())).unwrap().unwrap(), Some(vec![3003,3003]));
    let hidden = s.packages.get_mut(APP).unwrap().users.get_mut(&0).unwrap();
    hidden.hidden = true;
    let available = call(&s, 1000, false, pm::IS_PACKAGE_AVAILABLE, |p| pm::IsPackageAvailable { package_name: Some(APP.into()), user_id: 0 }.write(p));
    assert!(!pm::read_is_package_available_reply(&mut Reader::new(available.data(), available.objects())).unwrap().unwrap());
    s.packages.get_mut(APP).unwrap().users.get_mut(&0).unwrap().hidden = false;
    let list = call(&s, 1000, false, pm::GET_PACKAGES_HOLDING_PERMISSIONS, |p| pm::GetPackagesHoldingPermissions { permissions: Some(vec![Some("p.denied".into()),Some("p.granted".into()),Some("p.granted".into())]), flags: 0, user_id: 0 }.write(p));
    assert!(Reader::new(list.data(), list.objects()).read_exception().unwrap().is_ok());
    let filter = AppsFilter::new(&s, &Default::default()).unwrap();
    let query = Query { state: &s, filter: &filter, calling_uid: 1000 };
    let result = query.packages_holding_permissions(&[Some("p.denied".into()),Some("p.granted".into()),Some("p.granted".into())],0,0).unwrap().unwrap();
    assert_eq!(result.len(),1);
    assert_eq!(result[0].requested_permissions,Some(vec!["p.granted".into(),"p.granted".into()]));
    let owned = call(&s,10100,false,pm::GET_MIME_GROUP, |p| pm::GetMimeGroup { package_name:Some(APP.into()),group:Some("images".into()) }.write(p));
    assert_eq!(pm::read_get_mime_group_reply(&mut Reader::new(owned.data(),owned.objects())).unwrap().unwrap(),Some(vec![Some("image/png".into())]));
    let denied = call(&s,10101,false,pm::GET_MIME_GROUP, |p| pm::GetMimeGroup { package_name:Some(APP.into()),group:Some("images".into()) }.write(p));
    assert_eq!(Reader::new(denied.data(),denied.objects()).read_exception().unwrap().unwrap_err().code,EX_SECURITY);
}

#[test]
fn uid_signatures_use_settings_lineage_without_loaded_code() {
    let mut s = state();
    for name in [APP,OTHER] {
        let ps = s.packages.get_mut(name).unwrap();
        ps.pkg = None;
        ps.signatures = Some(super::super::settings::Signatures { signatures: vec![vec![2]],past_signatures:Some(vec![(vec![1],0),(vec![2],0)]),..Default::default() });
    }
    let same = call(&s,1000,false,pm::CHECK_UID_SIGNATURES, |p| pm::CheckUidSignatures {uid1:10100,uid2:10101}.write(p));
    assert_eq!(pm::read_check_uid_signatures_reply(&mut Reader::new(same.data(),same.objects())).unwrap().unwrap(),SIGNATURE_MATCH);
    let old = call(&s,1000,false,pm::HAS_UID_SIGNING_CERTIFICATE, |p| pm::HasUidSigningCertificate {uid:10100,signing_certificate:Some(vec![1]),flags:0}.write(p));
    assert!(pm::read_has_uid_signing_certificate_reply(&mut Reader::new(old.data(),old.objects())).unwrap().unwrap());
    for name in [APP,OTHER] { s.packages.get_mut(name).unwrap().signatures = None; }
    let unsigned = call(&s,1000,false,pm::CHECK_UID_SIGNATURES, |p| pm::CheckUidSignatures {uid1:10100,uid2:10101}.write(p));
    assert_eq!(pm::read_check_uid_signatures_reply(&mut Reader::new(unsigned.data(),unsigned.objects())).unwrap().unwrap(),SIGNATURE_NEITHER_SIGNED);
    s.packages.get_mut(APP).unwrap().signatures = Some(super::super::settings::Signatures {signatures:vec![vec![2]],..Default::default()});
    s.packages.get_mut(APP).unwrap().signatures.as_mut().unwrap().signatures.push(vec![3]);
    s.packages.get_mut(APP).unwrap().signatures.as_mut().unwrap().past_signatures=None;
    let multi = call(&s,1000,false,pm::HAS_UID_SIGNING_CERTIFICATE, |p| pm::HasUidSigningCertificate {uid:10100,signing_certificate:Some(vec![2]),flags:0}.write(p));
    assert!(!pm::read_has_uid_signing_certificate_reply(&mut Reader::new(multi.data(),multi.objects())).unwrap().unwrap());
}

#[test]
fn raw_uid_flags_and_name_translation_use_captured_settings() {
    let mut s = state();
    let ps = s.packages.get_mut(APP).unwrap();
    ps.setting_flags = Some((0x1240,0x4560));
    ps.real_name = Some(Some("canonical.app".into()));
    s.renamed_packages = Some(vec![("canonical.app".into(),APP.into())]);
    let flags = call(&s,1000,false,pm::GET_FLAGS_FOR_UID,|p| pm::GetFlagsForUid {uid:10100}.write(p));
    assert_eq!(pm::read_get_flags_for_uid_reply(&mut Reader::new(flags.data(),flags.objects())).unwrap().unwrap(),0x1240);
    let private = call(&s,1000,false,pm::GET_PRIVATE_FLAGS_FOR_UID,|p| pm::GetPrivateFlagsForUid {uid:10100}.write(p));
    assert_eq!(pm::read_get_private_flags_for_uid_reply(&mut Reader::new(private.data(),private.objects())).unwrap().unwrap(),0x4560);
    s.system.sdk_sandbox_package = Some(Some(APP.into()));
    let sdk = call(&s,1000,false,pm::GET_FLAGS_FOR_UID,|p| pm::GetFlagsForUid {uid:20000}.write(p));
    assert_eq!(pm::read_get_flags_for_uid_reply(&mut Reader::new(sdk.data(),sdk.objects())).unwrap().unwrap(),0x1240);
    let canonical = call(&s,1000,false,pm::CURRENT_TO_CANONICAL_PACKAGE_NAMES,|p| pm::CurrentToCanonicalPackageNames {names:Some(vec![Some(APP.into()),None,Some("unknown".into())])}.write(p));
    assert_eq!(pm::read_current_to_canonical_package_names_reply(&mut Reader::new(canonical.data(),canonical.objects())).unwrap().unwrap(),Some(vec![Some("canonical.app".into()),None,Some("unknown".into())]));
    let current = call(&s,1000,false,pm::CANONICAL_TO_CURRENT_PACKAGE_NAMES,|p| pm::CanonicalToCurrentPackageNames {names:Some(vec![Some("canonical.app".into())])}.write(p));
    assert_eq!(pm::read_canonical_to_current_package_names_reply(&mut Reader::new(current.data(),current.objects())).unwrap().unwrap(),Some(vec![Some(APP.into())]));
}

#[test]
fn permission_queries_follow_fuller_grants_and_manifest_appop_requests() {
    let mut s = state();
    let ps = s.packages.get_mut(APP).unwrap();
    ps.users.get_mut(&0).unwrap().granted_permissions = vec!["android.permission.ACCESS_FINE_LOCATION".into()];
    Arc::make_mut(ps.pkg.as_mut().unwrap()).requested_permissions = vec!["p.appop".into()];
    let package_grant = call(&s,1000,false,pm::CHECK_PERMISSION,|p| pm::CheckPermission {perm_name:Some("android.permission.ACCESS_COARSE_LOCATION".into()),pkg_name:Some(APP.into()),user_id:0}.write(p));
    assert_eq!(pm::read_check_permission_reply(&mut Reader::new(package_grant.data(),package_grant.objects())).unwrap().unwrap(),0);
    let uid_grant = call(&s,10101,false,pm::CHECK_UID_PERMISSION,|p| pm::CheckUidPermission {perm_name:Some("android.permission.ACCESS_COARSE_LOCATION".into()),uid:10100}.write(p));
    assert_eq!(pm::read_check_uid_permission_reply(&mut Reader::new(uid_grant.data(),uid_grant.objects())).unwrap().unwrap(),0);
    let no_user = call(&s,1000,false,pm::CHECK_PERMISSION,|p| pm::CheckPermission {perm_name:Some("android.permission.ACCESS_FINE_LOCATION".into()),pkg_name:Some(APP.into()),user_id:42}.write(p));
    assert_eq!(pm::read_check_permission_reply(&mut Reader::new(no_user.data(),no_user.objects())).unwrap().unwrap(),-1);
    let request = call(&s,1000,false,pm::GET_APP_OP_PERMISSION_PACKAGES,|p| pm::GetAppOpPermissionPackages {permission_name:Some("p.appop".into()),user_id:0}.write(p));
    assert_eq!(pm::read_get_app_op_permission_packages_reply(&mut Reader::new(request.data(),request.objects())).unwrap().unwrap(),Some(vec![Some(APP.into())]));
    Arc::make_mut(s.packages.get_mut(APP).unwrap().pkg.as_mut().unwrap()).booleans2 |= APEX;
    let apex = call(&s,1000,false,pm::GET_APP_OP_PERMISSION_PACKAGES,|p| pm::GetAppOpPermissionPackages {permission_name:Some("p.appop".into()),user_id:0}.write(p));
    assert_eq!(pm::read_get_app_op_permission_packages_reply(&mut Reader::new(apex.data(),apex.objects())).unwrap().unwrap(),Some(vec![]));
}

#[test]
fn shared_library_names_use_complete_registry_and_exclude_hidden_static_library() {
    let mut s = state();
    s.shared_libraries = Some(vec![
        super::super::model::SharedLibrary {name:Some("builtin".into()),kind:0,..Default::default()},
        super::super::model::SharedLibrary {name:Some("builtin".into()),kind:1,..Default::default()},
        super::super::model::SharedLibrary {name:Some("static.missing".into()),kind:2,package_name:Some("missing".into()),..Default::default()},
    ]);
    let names = call(&s,1000,false,pm::GET_SYSTEM_SHARED_LIBRARY_NAMES,|p| pm::GetSystemSharedLibraryNames {}.write(p));
    assert_eq!(pm::read_get_system_shared_library_names_reply(&mut Reader::new(names.data(),names.objects())).unwrap().unwrap(),Some(vec![Some("builtin".into())]));
    s.shared_libraries=Some(vec![]);
    let empty = call(&s,1000,false,pm::GET_SYSTEM_SHARED_LIBRARY_NAMES,|p| pm::GetSystemSharedLibraryNames {}.write(p));
    assert_eq!(pm::read_get_system_shared_library_names_reply(&mut Reader::new(empty.data(),empty.objects())).unwrap().unwrap(),None);
}
