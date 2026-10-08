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
    let available = call(&s, 1000, false, pm::IS_PACKAGE_AVAILABLE, |p| {
        pm::IsPackageAvailable {
            package_name: Some(APP.into()),
            user_id: 0,
        }
        .write(p)
    });
    assert_eq!(
        pm::read_is_package_available_reply(&mut Reader::new(
            available.data(),
            available.objects()
        ))
        .unwrap()
        .unwrap(),
        true
    );
    let gids = call(&s, 1000, false, pm::GET_PACKAGE_GIDS, |p| {
        pm::GetPackageGids {
            package_name: Some(APP.into()),
            flags: 0,
            user_id: 0,
        }
        .write(p)
    });
    assert_eq!(
        pm::read_get_package_gids_reply(&mut Reader::new(gids.data(), gids.objects()))
            .unwrap()
            .unwrap(),
        Some(vec![3003, 3003])
    );
    let hidden = s.packages.get_mut(APP).unwrap().users.get_mut(&0).unwrap();
    hidden.hidden = true;
    let available = call(&s, 1000, false, pm::IS_PACKAGE_AVAILABLE, |p| {
        pm::IsPackageAvailable {
            package_name: Some(APP.into()),
            user_id: 0,
        }
        .write(p)
    });
    assert!(
        !pm::read_is_package_available_reply(&mut Reader::new(
            available.data(),
            available.objects()
        ))
        .unwrap()
        .unwrap()
    );
    s.packages
        .get_mut(APP)
        .unwrap()
        .users
        .get_mut(&0)
        .unwrap()
        .hidden = false;
    let list = call(&s, 1000, false, pm::GET_PACKAGES_HOLDING_PERMISSIONS, |p| {
        pm::GetPackagesHoldingPermissions {
            permissions: Some(vec![
                Some("p.denied".into()),
                Some("p.granted".into()),
                Some("p.granted".into()),
            ]),
            flags: 0,
            user_id: 0,
        }
        .write(p)
    });
    assert!(
        Reader::new(list.data(), list.objects())
            .read_exception()
            .unwrap()
            .is_ok()
    );
    let filter = AppsFilter::new(&s, &Default::default()).unwrap();
    let query = Query {
        state: &s,
        filter: &filter,
        calling_uid: 1000,
    };
    let result = query
        .packages_holding_permissions(
            &[
                Some("p.denied".into()),
                Some("p.granted".into()),
                Some("p.granted".into()),
            ],
            0,
            0,
        )
        .unwrap()
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(
        result[0].requested_permissions,
        Some(vec!["p.granted".into(), "p.granted".into()])
    );
    let owned = call(&s, 10100, false, pm::GET_MIME_GROUP, |p| {
        pm::GetMimeGroup {
            package_name: Some(APP.into()),
            group: Some("images".into()),
        }
        .write(p)
    });
    assert_eq!(
        pm::read_get_mime_group_reply(&mut Reader::new(owned.data(), owned.objects()))
            .unwrap()
            .unwrap(),
        Some(vec![Some("image/png".into())])
    );
    let denied = call(&s, 10101, false, pm::GET_MIME_GROUP, |p| {
        pm::GetMimeGroup {
            package_name: Some(APP.into()),
            group: Some("images".into()),
        }
        .write(p)
    });
    assert_eq!(
        Reader::new(denied.data(), denied.objects())
            .read_exception()
            .unwrap()
            .unwrap_err()
            .code,
        EX_SECURITY
    );
}

#[test]
fn uid_signatures_use_settings_lineage_without_loaded_code() {
    let mut s = state();
    for name in [APP, OTHER] {
        let ps = s.packages.get_mut(name).unwrap();
        ps.pkg = None;
        ps.signatures = Some(super::super::settings::Signatures {
            signatures: vec![vec![2]],
            past_signatures: Some(vec![(vec![1], 0), (vec![2], 0)]),
            ..Default::default()
        });
    }
    let same = call(&s, 1000, false, pm::CHECK_UID_SIGNATURES, |p| {
        pm::CheckUidSignatures {
            uid1: 10100,
            uid2: 10101,
        }
        .write(p)
    });
    assert_eq!(
        pm::read_check_uid_signatures_reply(&mut Reader::new(same.data(), same.objects()))
            .unwrap()
            .unwrap(),
        SIGNATURE_MATCH
    );
    let old = call(&s, 1000, false, pm::HAS_UID_SIGNING_CERTIFICATE, |p| {
        pm::HasUidSigningCertificate {
            uid: 10100,
            signing_certificate: Some(vec![1]),
            flags: 0,
        }
        .write(p)
    });
    assert!(
        pm::read_has_uid_signing_certificate_reply(&mut Reader::new(old.data(), old.objects()))
            .unwrap()
            .unwrap()
    );
    for name in [APP, OTHER] {
        s.packages.get_mut(name).unwrap().signatures = None;
    }
    let unsigned = call(&s, 1000, false, pm::CHECK_UID_SIGNATURES, |p| {
        pm::CheckUidSignatures {
            uid1: 10100,
            uid2: 10101,
        }
        .write(p)
    });
    assert_eq!(
        pm::read_check_uid_signatures_reply(&mut Reader::new(unsigned.data(), unsigned.objects()))
            .unwrap()
            .unwrap(),
        SIGNATURE_NEITHER_SIGNED
    );
    s.packages.get_mut(APP).unwrap().signatures = Some(super::super::settings::Signatures {
        signatures: vec![vec![2]],
        ..Default::default()
    });
    s.packages
        .get_mut(APP)
        .unwrap()
        .signatures
        .as_mut()
        .unwrap()
        .signatures
        .push(vec![3]);
    s.packages
        .get_mut(APP)
        .unwrap()
        .signatures
        .as_mut()
        .unwrap()
        .past_signatures = None;
    let multi = call(&s, 1000, false, pm::HAS_UID_SIGNING_CERTIFICATE, |p| {
        pm::HasUidSigningCertificate {
            uid: 10100,
            signing_certificate: Some(vec![2]),
            flags: 0,
        }
        .write(p)
    });
    assert!(
        !pm::read_has_uid_signing_certificate_reply(&mut Reader::new(
            multi.data(),
            multi.objects()
        ))
        .unwrap()
        .unwrap()
    );
}

#[test]
fn raw_uid_flags_and_name_translation_use_captured_settings() {
    let mut s = state();
    let ps = s.packages.get_mut(APP).unwrap();
    ps.setting_flags = Some((0x1240, 0x4560));
    ps.real_name = Some(Some("canonical.app".into()));
    s.renamed_packages = Some(vec![("canonical.app".into(), APP.into())]);
    let flags = call(&s, 1000, false, pm::GET_FLAGS_FOR_UID, |p| {
        pm::GetFlagsForUid { uid: 10100 }.write(p)
    });
    assert_eq!(
        pm::read_get_flags_for_uid_reply(&mut Reader::new(flags.data(), flags.objects()))
            .unwrap()
            .unwrap(),
        0x1240
    );
    let private = call(&s, 1000, false, pm::GET_PRIVATE_FLAGS_FOR_UID, |p| {
        pm::GetPrivateFlagsForUid { uid: 10100 }.write(p)
    });
    assert_eq!(
        pm::read_get_private_flags_for_uid_reply(&mut Reader::new(
            private.data(),
            private.objects()
        ))
        .unwrap()
        .unwrap(),
        0x4560
    );
    s.system.sdk_sandbox_package = Some(Some(APP.into()));
    let sdk = call(&s, 1000, false, pm::GET_FLAGS_FOR_UID, |p| {
        pm::GetFlagsForUid { uid: 20000 }.write(p)
    });
    assert_eq!(
        pm::read_get_flags_for_uid_reply(&mut Reader::new(sdk.data(), sdk.objects()))
            .unwrap()
            .unwrap(),
        0x1240
    );
    let canonical = call(
        &s,
        1000,
        false,
        pm::CURRENT_TO_CANONICAL_PACKAGE_NAMES,
        |p| {
            pm::CurrentToCanonicalPackageNames {
                names: Some(vec![Some(APP.into()), None, Some("unknown".into())]),
            }
            .write(p)
        },
    );
    assert_eq!(
        pm::read_current_to_canonical_package_names_reply(&mut Reader::new(
            canonical.data(),
            canonical.objects()
        ))
        .unwrap()
        .unwrap(),
        Some(vec![
            Some("canonical.app".into()),
            None,
            Some("unknown".into())
        ])
    );
    let current = call(
        &s,
        1000,
        false,
        pm::CANONICAL_TO_CURRENT_PACKAGE_NAMES,
        |p| {
            pm::CanonicalToCurrentPackageNames {
                names: Some(vec![Some("canonical.app".into())]),
            }
            .write(p)
        },
    );
    assert_eq!(
        pm::read_canonical_to_current_package_names_reply(&mut Reader::new(
            current.data(),
            current.objects()
        ))
        .unwrap()
        .unwrap(),
        Some(vec![Some(APP.into())])
    );
}

#[test]
fn permission_queries_follow_fuller_grants_and_manifest_appop_requests() {
    let mut s = state();
    let ps = s.packages.get_mut(APP).unwrap();
    ps.users.get_mut(&0).unwrap().granted_permissions =
        vec!["android.permission.ACCESS_FINE_LOCATION".into()];
    Arc::make_mut(ps.pkg.as_mut().unwrap()).requested_permissions = vec!["p.appop".into()];
    let package_grant = call(&s, 1000, false, pm::CHECK_PERMISSION, |p| {
        pm::CheckPermission {
            perm_name: Some("android.permission.ACCESS_COARSE_LOCATION".into()),
            pkg_name: Some(APP.into()),
            user_id: 0,
        }
        .write(p)
    });
    assert_eq!(
        pm::read_check_permission_reply(&mut Reader::new(
            package_grant.data(),
            package_grant.objects()
        ))
        .unwrap()
        .unwrap(),
        0
    );
    let uid_grant = call(&s, 10101, false, pm::CHECK_UID_PERMISSION, |p| {
        pm::CheckUidPermission {
            perm_name: Some("android.permission.ACCESS_COARSE_LOCATION".into()),
            uid: 10100,
        }
        .write(p)
    });
    assert_eq!(
        pm::read_check_uid_permission_reply(&mut Reader::new(
            uid_grant.data(),
            uid_grant.objects()
        ))
        .unwrap()
        .unwrap(),
        0
    );
    let no_user = call(&s, 1000, false, pm::CHECK_PERMISSION, |p| {
        pm::CheckPermission {
            perm_name: Some("android.permission.ACCESS_FINE_LOCATION".into()),
            pkg_name: Some(APP.into()),
            user_id: 42,
        }
        .write(p)
    });
    assert_eq!(
        pm::read_check_permission_reply(&mut Reader::new(no_user.data(), no_user.objects()))
            .unwrap()
            .unwrap(),
        -1
    );
    let request = call(&s, 1000, false, pm::GET_APP_OP_PERMISSION_PACKAGES, |p| {
        pm::GetAppOpPermissionPackages {
            permission_name: Some("p.appop".into()),
            user_id: 0,
        }
        .write(p)
    });
    assert_eq!(
        pm::read_get_app_op_permission_packages_reply(&mut Reader::new(
            request.data(),
            request.objects()
        ))
        .unwrap()
        .unwrap(),
        Some(vec![Some(APP.into())])
    );
    Arc::make_mut(s.packages.get_mut(APP).unwrap().pkg.as_mut().unwrap()).booleans2 |= APEX;
    let apex = call(&s, 1000, false, pm::GET_APP_OP_PERMISSION_PACKAGES, |p| {
        pm::GetAppOpPermissionPackages {
            permission_name: Some("p.appop".into()),
            user_id: 0,
        }
        .write(p)
    });
    assert_eq!(
        pm::read_get_app_op_permission_packages_reply(&mut Reader::new(
            apex.data(),
            apex.objects()
        ))
        .unwrap()
        .unwrap(),
        Some(vec![])
    );
}

#[test]
fn shared_library_names_use_complete_registry_and_exclude_hidden_static_library() {
    let mut s = state();
    s.shared_libraries = Some(vec![
        super::super::model::SharedLibrary {
            name: Some("builtin".into()),
            kind: 0,
            ..Default::default()
        },
        super::super::model::SharedLibrary {
            name: Some("builtin".into()),
            kind: 1,
            ..Default::default()
        },
        super::super::model::SharedLibrary {
            name: Some("static.missing".into()),
            kind: 2,
            package_name: Some("missing".into()),
            ..Default::default()
        },
    ]);
    let names = call(&s, 1000, false, pm::GET_SYSTEM_SHARED_LIBRARY_NAMES, |p| {
        pm::GetSystemSharedLibraryNames {}.write(p)
    });
    assert_eq!(
        pm::read_get_system_shared_library_names_reply(&mut Reader::new(
            names.data(),
            names.objects()
        ))
        .unwrap()
        .unwrap(),
        Some(vec![Some("builtin".into())])
    );
    s.shared_libraries = Some(vec![]);
    let empty = call(&s, 1000, false, pm::GET_SYSTEM_SHARED_LIBRARY_NAMES, |p| {
        pm::GetSystemSharedLibraryNames {}.write(p)
    });
    assert_eq!(
        pm::read_get_system_shared_library_names_reply(&mut Reader::new(
            empty.data(),
            empty.objects()
        ))
        .unwrap()
        .unwrap(),
        None
    );
}

#[test]
fn internal_metadata_keeps_permission_and_filter_identities_separate() {
    let state = state();
    let filter = AppsFilter::new(&state, &crate::package::apps_filter::Config::default()).unwrap();
    let app = Query {
        state: &state,
        filter: &filter,
        calling_uid: 10100,
    };
    let system = Query {
        state: &state,
        filter: &filter,
        calling_uid: 1000,
    };
    assert!(
        app.package_info_internal(OTHER, -1, 0, 10, 1000)
            .unwrap()
            .is_err_and(|error| error.code == EX_SECURITY)
    );
    assert!(matches!(
        app.application_info_internal(OTHER, 0, 10, 1000),
        Err(NotModelled("a cross-user call recents may make"))
    ));
    assert!(
        system
            .application_info_internal(OTHER, 0, 0, 10100)
            .unwrap()
            .unwrap()
            .is_none()
    );
    assert!(
        app.application_info_internal(OTHER, 0, 0, 1000)
            .unwrap()
            .unwrap()
            .is_some()
    );
    // PackageInfo generation also filters with the actual caller, as AOSP does.
    assert!(
        app.package_info_internal(OTHER, -1, 0, 0, 1000)
            .unwrap()
            .unwrap()
            .is_none()
    );
}

#[test]
fn user_status_queries_enforce_full_cross_user_and_hide_unknown_packages() {
    let mut state = state();
    let ps = state.packages.get_mut(APP).unwrap();
    let user = ps.users.get_mut(&0).unwrap();
    user.stopped = true;
    user.suspended_by = vec!["android".into()];
    user.quarantined = true;
    ps.users.insert(
        10,
        PackageUserState {
            stopped: true,
            ..Default::default()
        },
    );
    state
        .packages
        .get_mut(APP)
        .unwrap()
        .users
        .get_mut(&0)
        .unwrap()
        .granted_permissions
        .push(INTERACT_ACROSS_USERS.into());
    for code in [
        pm::IS_PACKAGE_STOPPED_FOR_USER,
        pm::IS_PACKAGE_SUSPENDED_FOR_USER,
        pm::IS_PACKAGE_QUARANTINED_FOR_USER,
    ] {
        let invoke = |uid, name: Option<&str>, user| {
            call(&state, uid, false, code, |p| {
                p.write_interface_token(pm::DESCRIPTOR);
                p.write_string16(name);
                p.write_i32(user);
            })
        };
        let own = invoke(10100, Some(APP), 0);
        let mut r = Reader::new(own.data(), own.objects());
        r.read_exception().unwrap().unwrap();
        assert!(r.read_bool().unwrap());
        assert_eq!(r.remaining(), 0);
        let hidden = invoke(10100, Some(OTHER), 0);
        assert_eq!(
            Reader::new(hidden.data(), hidden.objects())
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_ILLEGAL_ARGUMENT
        );
        let missing = invoke(1000, None, 0);
        assert_eq!(
            Reader::new(missing.data(), missing.objects())
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_ILLEGAL_ARGUMENT
        );
        let weak = invoke(10100, Some(APP), 10);
        assert_eq!(
            Reader::new(weak.data(), weak.objects())
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        let negative = invoke(1000, Some(APP), -1);
        assert_eq!(
            Reader::new(negative.data(), negative.objects())
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_ILLEGAL_ARGUMENT
        );
    }
}

#[test]
fn harmful_warning_requires_permission_and_preserves_nullable_text() {
    let mut state = state();
    state
        .packages
        .get_mut(APP)
        .unwrap()
        .users
        .get_mut(&0)
        .unwrap()
        .harmful_app_warning = Some("위험 <warning>".into());
    let invoke = |state: &State, uid, name| {
        call(state, uid, false, pm::GET_HARMFUL_APP_WARNING, |p| {
            pm::GetHarmfulAppWarning {
                package_name: name,
                user_id: 0,
            }
            .write(p)
        })
    };
    let denied = invoke(&state, 10100, Some(APP.into()));
    assert_eq!(
        Reader::new(denied.data(), denied.objects())
            .read_exception()
            .unwrap()
            .unwrap_err()
            .code,
        EX_SECURITY
    );
    state
        .packages
        .get_mut(APP)
        .unwrap()
        .users
        .get_mut(&0)
        .unwrap()
        .granted_permissions
        .push("android.permission.SET_HARMFUL_APP_WARNINGS".into());
    let warning = invoke(&state, 10100, Some(APP.into()));
    let mut r = Reader::new(warning.data(), warning.objects());
    r.read_exception().unwrap().unwrap();
    assert_eq!(r.read_i32().unwrap(), 1);
    assert_eq!(r.read_i32().unwrap(), 1);
    assert_eq!(r.read_string8().unwrap().as_deref(), Some("위험 <warning>"));
    assert_eq!(r.remaining(), 0);
    let missing = invoke(&state, 1000, Some("missing".into()));
    assert_eq!(
        Reader::new(missing.data(), missing.objects())
            .read_exception()
            .unwrap()
            .unwrap_err()
            .code,
        EX_ILLEGAL_ARGUMENT
    );
    let empty = invoke(&state, 1000, Some(OTHER.into()));
    let mut r = Reader::new(empty.data(), empty.objects());
    r.read_exception().unwrap().unwrap();
    assert_eq!(r.read_i32().unwrap(), 0);
    assert_eq!(r.remaining(), 0);
}

#[test]
fn native_audio_capture_uses_visible_application_info_in_input_order() {
    let mut state = state();
    let package = state.packages.get_mut(APP).unwrap();
    Arc::make_mut(package.pkg.as_mut().unwrap()).booleans |= booleans::ALLOW_AUDIO_PLAYBACK_CAPTURE;
    let value = call(
        &state,
        1000,
        true,
        native::IS_AUDIO_PLAYBACK_CAPTURE_ALLOWED,
        |p| {
            native::IsAudioPlaybackCaptureAllowed {
                package_names: Some(vec![
                    Some(OTHER.into()),
                    Some(APP.into()),
                    None,
                    Some("missing".into()),
                ]),
            }
            .write(p)
        },
    );
    let mut r = Reader::new(value.data(), value.objects());
    assert_eq!(
        native::read_is_audio_playback_capture_allowed_reply(&mut r)
            .unwrap()
            .unwrap(),
        Some(vec![false, true, false, false])
    );
    assert_eq!(r.remaining(), 0);
    let null = call(
        &state,
        1000,
        true,
        native::IS_AUDIO_PLAYBACK_CAPTURE_ALLOWED,
        |p| {
            native::IsAudioPlaybackCaptureAllowed {
                package_names: None,
            }
            .write(p)
        },
    );
    assert_eq!(
        Reader::new(null.data(), null.objects())
            .read_exception()
            .unwrap()
            .unwrap_err()
            .code,
        -4
    );
}

#[test]
fn internal_uid_query_uses_fixed_filter_without_public_user_or_flag_checks() {
    let state = state();
    let filter = AppsFilter::new(&state, &Default::default()).unwrap();
    let query = Query {
        state: &state,
        filter: &filter,
        calling_uid: 10100,
    };
    assert_eq!(
        query.package_uid_internal(OTHER, 0, 42, 1000).unwrap(),
        4210101
    );
    assert_eq!(query.package_uid(OTHER, 0, 42).unwrap().unwrap(), -1);
    assert_eq!(
        query
            .package_uid_internal(OTHER, MATCH_SYSTEM_ONLY, 0, 1000)
            .unwrap(),
        -1
    );
    assert_eq!(
        query.package_uid_internal("missing", 0, 0, 1000).unwrap(),
        -1
    );
}

#[test]
fn internal_identity_reads_separate_actual_comparison_and_query_uids() {
    let mut state = state();
    Arc::make_mut(state.packages.get_mut(APP).unwrap().pkg.as_mut().unwrap()).queries_packages =
        vec!["new.target".into()];
    let filter = AppsFilter::new(&state, &Default::default()).unwrap();
    let system = Query {
        state: &state,
        filter: &filter,
        calling_uid: 1000,
    };
    assert!(
        system
            .internal_same_app(Some(APP), 0, 110100, 0)
            .unwrap()
            .unwrap()
    );
    assert!(
        !system
            .internal_same_app(None, 0, 10100, 0)
            .unwrap()
            .unwrap()
    );
    assert!(
        !system
            .internal_same_app(Some("missing"), 0, 10100, 0)
            .unwrap()
            .unwrap()
    );
    assert!(!system.internal_filter_uid(20100, 20100).unwrap());
    assert!(system.internal_filter_uid(20100, 10100).unwrap());
    assert!(system.internal_filter_uid(42000, 1000).unwrap());
    assert!(
        system
            .internal_can_query(10100, Some("new.target"))
            .unwrap()
            .unwrap()
    );
    assert!(
        !system
            .internal_can_query(10101, Some("new.target"))
            .unwrap()
            .unwrap()
    );
    assert!(system.internal_can_query(42000, None).unwrap().unwrap());
    assert!(
        !system
            .internal_can_query(42000, Some(APP))
            .unwrap()
            .unwrap()
    );
}

#[test]
fn instrumentation_queries_keep_last_component_and_preserve_requested_metadata_and_users() {
    let mut state = state();
    let package = state.packages.get_mut(APP).unwrap();
    let code = Arc::make_mut(package.pkg.as_mut().unwrap());
    let instrumentation = |target: &str| super::super::pkg::Instrumentation {
        component: Component {
            package_name: APP.into(),
            name: "p.Instrument".into(),
            meta_data: Some(MetaData(vec![("instrument-key".into(), Meta::Int(7))])),
            ..Default::default()
        },
        target_package: Some(target.into()),
        handle_profiling: true,
        ..Default::default()
    };
    code.instrumentations = vec![instrumentation("old-target"), instrumentation("new-target")];
    let filter = AppsFilter::new(&state, &Default::default()).unwrap();
    let query = Query {
        state: &state,
        filter: &filter,
        calling_uid: 1000,
    };
    let component = ComponentName {
        package: APP.into(),
        class: "p.Instrument".into(),
    };
    let info = query
        .instrumentation_info(Some(&component), GET_META_DATA, 0)
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(info.0.target_package.as_deref(), Some("new-target"));
    assert!(info.0.handle_profiling);
    assert!(info.0.item.meta_data.is_some());
    assert_eq!(
        info.0.source_dir.as_deref(),
        Some("/data/app/org.example.app/base.apk")
    );
    let infos = query.instrumentations(None, 0, 0).unwrap().unwrap();
    assert_eq!(infos.len(), 1);
    assert!(infos[0].0.item.meta_data.is_none());
    assert!(
        query
            .instrumentations(Some("old-target"), 0, 0)
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert!(
        query
            .instrumentation_info(Some(&component), 0, 42)
            .unwrap()
            .unwrap()
            .is_none()
    );
    let mut args = Parcel::new();
    args.write_interface_token(pm::DESCRIPTOR);
    args.write_i32(1);
    args.write_string16(Some(APP));
    args.write_string16(Some("p.Instrument"));
    args.write_i32(0);
    args.write_i32(0);
    let reply = query
        .answer(
            pm::DESCRIPTOR,
            pm::GET_INSTRUMENTATION_INFO_AS_USER,
            &mut Reader::new(args.data(), args.objects()),
        )
        .unwrap();
    let mut reader = Reader::new(reply.data(), reply.objects());
    reader.read_exception().unwrap().unwrap();
    assert_eq!(reader.read_i32().unwrap(), 1);
}

#[test]
fn property_routes_use_location_priority_visibility_and_reject_unknown_types() {
    use crate::package::pkg::{Property, PropertyValue, Provider};
    let mut state = state();
    let code = Arc::make_mut(state.packages.get_mut(APP).unwrap().pkg.as_mut().unwrap());
    let property = |class: Option<&str>, value| Property {
        name: Some("owned".into()),
        package_name: Some(APP.into()),
        class_name: class.map(str::to_owned),
        value,
    };
    code.properties = Some(vec![(
        "owned".into(),
        property(None, PropertyValue::Resource(0x7f010001)),
    )]);
    code.activities[0].main.component.properties = Some(vec![(
        "owned".into(),
        property(Some("org.example.app.Main"), PropertyValue::Int(1)),
    )]);
    let mut provider = Provider::default();
    provider.main.component.name = "org.example.app.Main".into();
    provider.main.component.package_name = APP.into();
    provider.main.component.properties = Some(vec![(
        "owned".into(),
        property(Some("org.example.app.Main"), PropertyValue::Int(2)),
    )]);
    code.providers.push(provider);
    let filter = AppsFilter::new(&state, &Default::default()).unwrap();
    let query = Query {
        state: &state,
        filter: &filter,
        calling_uid: 1000,
    };
    let application = query
        .package_property(Some("owned"), Some(APP), None, 0)
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(application.0.value, PropertyValue::Resource(0x7f010001));
    let component = query
        .package_property(Some("owned"), Some(APP), Some("org.example.app.Main"), 0)
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(component.0.value, PropertyValue::Int(1));
    let providers = query.query_properties(Some("owned"), 4).unwrap().unwrap();
    assert_eq!(providers.len(), 1);
    assert_eq!(providers[0].0.value, PropertyValue::Int(2));
    assert!(
        query
            .query_properties(Some("owned"), 0)
            .unwrap()
            .unwrap()
            .is_empty()
    );
    let mut args = Parcel::new();
    pm::GetPropertyAsUser {
        property_name: Some("owned".into()),
        package_name: Some(APP.into()),
        class_name: None,
        user_id: 0,
    }
    .write(&mut args);
    let reply = query
        .answer(
            pm::DESCRIPTOR,
            pm::GET_PROPERTY_AS_USER,
            &mut Reader::new(args.data(), args.objects()),
        )
        .unwrap();
    let mut reader = Reader::new(reply.data(), reply.objects());
    reader.read_exception().unwrap().unwrap();
    assert_eq!(reader.read_i32().unwrap(), 1);
    assert_eq!(reader.read_string16().unwrap().as_deref(), Some("owned"));
    assert_eq!(reader.read_i32().unwrap(), 4);
    assert_eq!(reader.read_string16().unwrap().as_deref(), Some(APP));
    assert_eq!(reader.read_string16().unwrap(), None);
    assert_eq!(reader.read_i32().unwrap(), 0x7f010001);
    assert_eq!(reader.remaining(), 0);
    drop(query);
    drop(filter);
    Arc::make_mut(state.packages.get_mut(APP).unwrap().pkg.as_mut().unwrap())
        .properties
        .as_mut()
        .unwrap()[0]
        .1
        .value = PropertyValue::Unknown(99);
    let filter = AppsFilter::new(&state, &Default::default()).unwrap();
    let query = Query {
        state: &state,
        filter: &filter,
        calling_uid: 1000,
    };
    assert!(
        query
            .package_property(Some("owned"), Some(APP), None, 0)
            .is_err()
    );
    assert!(query.query_properties(Some("owned"), 5).is_err());
}

#[test]
fn content_provider_query_filters_process_uid_metadata_and_orders_initialization() {
    use crate::package::pkg::Provider;
    let mut state = state();
    let code = Arc::make_mut(state.packages.get_mut(APP).unwrap().pkg.as_mut().unwrap());
    code.providers = [("p.Low", 1), ("p.High", 9)]
        .map(|(name, order)| {
            let mut provider = Provider::default();
            provider.main.component.name = name.into();
            provider.main.component.package_name = APP.into();
            provider.main.enabled = true;
            provider.main.exported = true;
            provider.main.direct_boot_aware = true;
            provider.main.process_name = Some("process".into());
            provider.main.component.meta_data =
                Some(MetaData(vec![("metadata-key".into(), Meta::Int(7))]));
            provider.authority = Some(format!("authority.{order}"));
            provider.init_order = order;
            provider
        })
        .into();
    let filter = AppsFilter::new(&state, &Default::default()).unwrap();
    let query = Query {
        state: &state,
        filter: &filter,
        calling_uid: 1000,
    };
    let providers = query
        .content_providers(Some("process"), 10100, GET_META_DATA, Some("metadata-key"))
        .unwrap()
        .unwrap();
    assert_eq!(providers.len(), 2);
    assert_eq!(providers[0].init_order, 9);
    assert_eq!(providers[1].init_order, 1);
    assert!(providers[0].info.item.meta_data.is_some());
    assert!(
        query
            .content_providers(Some("process"), 10101, 0, None)
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert!(
        query
            .content_providers(Some("different"), 10100, 0, None)
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert!(
        query
            .content_providers(None, 99999999, 0, Some("missing-key"))
            .unwrap()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        query
            .content_providers(None, 99999999, 0, None)
            .unwrap()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn retired_aosp_reads_preserve_exact_empty_slice_and_false_contracts() {
    let state = state();
    let reply = call(&state, 10100, false, pm::HAS_SYSTEM_UID_ERRORS, |p| {
        pm::HasSystemUidErrors {}.write(p)
    });
    assert!(
        !pm::read_has_system_uid_errors_reply(&mut Reader::new(reply.data(), reply.objects()))
            .unwrap()
            .unwrap()
    );
    let reply = call(
        &state,
        10100,
        false,
        pm::GET_INTENT_FILTER_VERIFICATIONS,
        |p| pm::GetIntentFilterVerifications { package_name: None }.write(p),
    );
    let mut reader = Reader::new(reply.data(), reply.objects());
    reader.read_exception().unwrap().unwrap();
    assert_eq!(reader.read_i32().unwrap(), 1);
    assert_eq!(reader.read_i32().unwrap(), 0);
    assert_eq!(reader.remaining(), 0);
}

struct KeySetProcesses {
    driver: Arc<aim_binder_driver::Driver>,
    process: Arc<aim_binder_host::local::LocalProcess>,
}
impl Drop for KeySetProcesses {
    fn drop(&mut self) {
        self.driver.release(self.process.proc_handle());
    }
}
fn keyset_state() -> (State, KeySetProcesses) {
    let driver = aim_binder_driver::Driver::new();
    let process = aim_binder_host::local::LocalProcess::open(
        &driver,
        aim_binder_driver::Device::Binder,
        aim_binder_driver::Credentials {
            pid: 88031,
            euid: 1000,
            security_context: None,
        },
    );
    let mut state = state();
    state.system.key_set_tokens = Some(Arc::new(crate::package::keysets::Tokens::new(
        process.clone(),
    )));
    state.key_sets = Some(crate::package::settings::KeySets {
        key_sets: vec![(1, vec![10, 20]), (2, vec![10]), (3, vec![30])],
        public_keys: vec![(10, vec![1]), (20, vec![2]), (30, vec![3])],
        ..Default::default()
    });
    state.packages.get_mut(APP).unwrap().key_set_data =
        Some(crate::package::settings::KeySetData {
            proper_signing_key_set: 1,
            defined_key_sets: vec![(Some("subset".into()), 2), (Some("wrong".into()), 3)],
            ..Default::default()
        });
    (state, KeySetProcesses { driver, process })
}
fn alias_keyset(state: &State, alias: &str) -> crate::package::keysets::KeySet {
    let reply = call(state, 10100, false, pm::GET_KEY_SET_BY_ALIAS, |p| {
        pm::GetKeySetByAlias {
            package_name: Some(APP.into()),
            alias: Some(alias.into()),
        }
        .write(p)
    });
    pm::read_get_key_set_by_alias_reply::<crate::package::keysets::KeySet>(&mut Reader::new(
        reply.data(),
        reply.objects(),
    ))
    .unwrap()
    .unwrap()
    .unwrap()
}
fn signed_keyset(state: &State, token: crate::package::keysets::KeySet, exact: bool) -> bool {
    let code = if exact {
        pm::IS_PACKAGE_SIGNED_BY_KEY_SET_EXACTLY
    } else {
        pm::IS_PACKAGE_SIGNED_BY_KEY_SET
    };
    let reply = call(state, 10100, false, code, |p| {
        if exact {
            pm::IsPackageSignedByKeySetExactly {
                package_name: Some(APP.into()),
                ks: Some(token),
            }
            .write(p);
        } else {
            pm::IsPackageSignedByKeySet {
                package_name: Some(APP.into()),
                ks: Some(token),
            }
            .write(p);
        }
    });
    pm::read_is_package_signed_by_key_set_reply(&mut Reader::new(reply.data(), reply.objects()))
        .unwrap()
        .unwrap()
}
#[test]
fn keyset_capabilities_preserve_identity_subset_exact_and_retired_pool_semantics() {
    let (mut state, _process) = keyset_state();
    let subset = alias_keyset(&state, "subset");
    assert_eq!(subset.token, alias_keyset(&state, "subset").token);
    assert!(signed_keyset(&state, subset, false));
    assert!(!signed_keyset(&state, subset, true));
    assert!(!signed_keyset(&state, alias_keyset(&state, "wrong"), false));
    let reply = call(&state, 10100, false, pm::GET_SIGNING_KEY_SET, |p| {
        pm::GetSigningKeySet {
            package_name: Some(APP.into()),
        }
        .write(p)
    });
    let proper = pm::read_get_signing_key_set_reply::<crate::package::keysets::KeySet>(
        &mut Reader::new(reply.data(), reply.objects()),
    )
    .unwrap()
    .unwrap()
    .unwrap();
    assert!(signed_keyset(&state, proper, true));
    state
        .key_sets
        .as_mut()
        .unwrap()
        .key_sets
        .retain(|(id, _)| *id != 2);
    assert!(!signed_keyset(&state, subset, false));
    let foreign = crate::package::keysets::Tokens::new(_process.process.clone()).token(1);
    assert!(!signed_keyset(
        &state,
        crate::package::keysets::KeySet {
            token: Some(foreign)
        },
        true
    ));
}
#[test]
fn keyset_signing_access_checks_exact_system_uid_or_same_app() {
    let (state, _process) = keyset_state();
    for uid in [0, 10101, 101000] {
        let reply = call(&state, uid, false, pm::GET_SIGNING_KEY_SET, |p| {
            pm::GetSigningKeySet {
                package_name: Some(APP.into()),
            }
            .write(p)
        });
        let error = Reader::new(reply.data(), reply.objects())
            .read_exception()
            .unwrap()
            .unwrap_err();
        assert!(matches!(error.code, EX_SECURITY | EX_ILLEGAL_ARGUMENT));
    }
    for uid in [1000, 10100] {
        let reply = call(&state, uid, false, pm::GET_SIGNING_KEY_SET, |p| {
            pm::GetSigningKeySet {
                package_name: Some(APP.into()),
            }
            .write(p)
        });
        assert!(
            pm::read_get_signing_key_set_reply::<crate::package::keysets::KeySet>(
                &mut Reader::new(reply.data(), reply.objects())
            )
            .unwrap()
            .unwrap()
            .is_some()
        );
    }
}
#[test]
fn keyset_unknown_alias_and_null_token_throw_source_exceptions() {
    let (state, _process) = keyset_state();
    let reply = call(&state, 10100, false, pm::GET_KEY_SET_BY_ALIAS, |p| {
        pm::GetKeySetByAlias {
            package_name: Some(APP.into()),
            alias: Some("missing".into()),
        }
        .write(p)
    });
    let error = Reader::new(reply.data(), reply.objects())
        .read_exception()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, EX_ILLEGAL_ARGUMENT);
    assert!(
        error
            .message
            .starts_with("Unknown KeySet alias: missing, aliases = {")
    );
    let reply = call(
        &state,
        10100,
        false,
        pm::IS_PACKAGE_SIGNED_BY_KEY_SET,
        |p| {
            pm::IsPackageSignedByKeySet {
                package_name: None,
                ks: Some(crate::package::keysets::KeySet { token: None }),
            }
            .write(p)
        },
    );
    let error = Reader::new(reply.data(), reply.objects())
        .read_exception()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, aim_binder_host::parcel::EX_NULL_POINTER);
    assert_eq!(error.message, "null value for KeySet IBinder token");
}

#[test]
fn legacy_domain_query_reads_native_owner_and_enforces_user_scope() {
    let mut state = state();
    state.legacy_domains = Some(vec![(Some(APP.into()), vec![(0, 2), (10, 3)])]);
    let get = |uid, user| {
        let reply = call(
            &state,
            uid,
            false,
            pm::GET_INTENT_VERIFICATION_STATUS,
            |p| {
                pm::GetIntentVerificationStatus {
                    package_name: Some(APP.into()),
                    user_id: user,
                }
                .write(p)
            },
        );
        pm::read_get_intent_verification_status_reply(&mut Reader::new(
            reply.data(),
            reply.objects(),
        ))
        .unwrap()
    };
    assert_eq!(get(10100, 0).unwrap(), 2);
    assert_eq!(get(1000, 10).unwrap(), 3);
    assert_eq!(get(10100, 10).unwrap_err().code, EX_SECURITY);
    assert_eq!(get(1000, 11).unwrap_err().code, EX_SECURITY);
}

#[test]
fn all_user_uid_signature_helper_enforces_each_uid_user() {
    let state = state();
    let filter = AppsFilter::new(&state, &crate::package::apps_filter::Config::default()).unwrap();
    let system = Query {
        state: &state,
        filter: &filter,
        calling_uid: 1000,
    };
    assert_eq!(
        system
            .check_uid_signatures_captured(10100, 1010100, true)
            .unwrap()
            .unwrap(),
        SIGNATURE_NEITHER_SIGNED
    );
    let app = Query {
        state: &state,
        filter: &filter,
        calling_uid: 10100,
    };
    assert_eq!(
        app.check_uid_signatures_captured(10100, 1010100, true)
            .unwrap()
            .unwrap_err()
            .code,
        EX_SECURITY
    );
}

fn suspended_state(entries: Vec<crate::package::restrictions::Suspension>) -> State {
    let mut state = state();
    let saved = crate::package::restrictions::UserState {
        suspensions: Some(entries),
        ..Default::default()
    };
    let owned = saved
        .resolved_suspensions(0, false)
        .into_iter()
        .map(|(user, row)| crate::package::restrictions::Suspension {
            package: row.package.clone(),
            user: crate::package::restrictions::SuspendingUser::Resolved(user),
            params: row.params.clone(),
        })
        .collect::<Vec<_>>();
    let user = state
        .packages
        .get_mut(APP)
        .unwrap()
        .users
        .get_mut(&0)
        .unwrap();
    user.suspended_by = owned.iter().map(|row| row.package.clone()).collect();
    user.suspensions = Some(owned);
    state
}
fn suspender(
    package: &str,
    quarantined: bool,
    extras: Option<crate::package::restrictions::persistable::Bundle>,
) -> crate::package::restrictions::Suspension {
    crate::package::restrictions::Suspension {
        package: package.into(),
        user: crate::package::restrictions::SuspendingUser::Current,
        params: Some(crate::package::restrictions::SuspendParams {
            quarantined,
            app_extras: extras,
            ..Default::default()
        }),
    }
}
#[test]
fn suspending_package_uses_registered_userpackage_order_and_quarantine_precedence() {
    let mut state = suspended_state(vec![
        suspender("android", false, None),
        suspender("BB", false, None),
        suspender("Aa", false, None),
    ]);
    let get = |state: &State, uid, user, name: &str| {
        let reply = call(state, uid, false, pm::GET_SUSPENDING_PACKAGE, |p| {
            pm::GetSuspendingPackage {
                package_name: Some(name.into()),
                user_id: user,
            }
            .write(p)
        });
        pm::read_get_suspending_package_reply(&mut Reader::new(reply.data(), reply.objects()))
            .unwrap()
    };
    assert_eq!(
        get(&state, 10100, 0, APP).unwrap().as_deref(),
        Some("android")
    );
    assert_eq!(get(&state, 10100, 0, "unknown").unwrap(), None);
    assert_eq!(get(&state, 10100, 10, APP).unwrap_err().code, EX_SECURITY);
    let params = state
        .packages
        .get_mut(APP)
        .unwrap()
        .users
        .get_mut(&0)
        .unwrap()
        .suspensions
        .as_mut()
        .unwrap();
    params
        .iter_mut()
        .find(|row| row.package == "BB")
        .unwrap()
        .params
        .as_mut()
        .unwrap()
        .quarantined = true;
    params
        .iter_mut()
        .find(|row| row.package == "Aa")
        .unwrap()
        .params
        .as_mut()
        .unwrap()
        .quarantined = true;
    assert_eq!(get(&state, 10100, 0, APP).unwrap().as_deref(), Some("BB"));
    let params = state
        .packages
        .get_mut(APP)
        .unwrap()
        .users
        .get_mut(&0)
        .unwrap()
        .suspensions
        .as_mut()
        .unwrap();
    params
        .iter_mut()
        .find(|row| row.package == "Aa")
        .unwrap()
        .params = None;
    assert_eq!(
        get(&state, 10100, 0, APP).unwrap_err().code,
        aim_binder_host::parcel::EX_NULL_POINTER
    );
}
#[test]
fn suspended_app_extras_merge_bundles_in_native_owner_order_and_require_exact_package_uid() {
    use crate::package::restrictions::persistable::{Bundle, Value};
    let entries = |value, text: &str| Bundle {
        entries: vec![
            (Some("BB".into()), Value::Int(value)),
            (Some("Aa".into()), Value::String(text.into())),
        ],
    };
    let state = suspended_state(vec![
        suspender("BB", false, Some(entries(1, "first"))),
        suspender("Aa", false, Some(entries(3, "last"))),
    ]);
    let reply = call(
        &state,
        10100,
        false,
        pm::GET_SUSPENDED_PACKAGE_APP_EXTRAS,
        |p| {
            pm::GetSuspendedPackageAppExtras {
                package_name: Some(APP.into()),
                user_id: 0,
            }
            .write(p)
        },
    );
    let mut reader = Reader::new(reply.data(), reply.objects());
    reader.read_exception().unwrap().unwrap();
    assert_eq!(reader.read_i32().unwrap(), 1);
    assert!(reader.read_i32().unwrap() > 0);
    assert_eq!(reader.read_i32().unwrap(), crate::bundle::MAGIC);
    assert_eq!(reader.read_i32().unwrap(), 2);
    assert_eq!(reader.read_string16().unwrap().as_deref(), Some("BB"));
    assert_eq!(reader.read_i32().unwrap(), 1);
    assert_eq!(reader.read_i32().unwrap(), 3);
    assert_eq!(reader.read_string16().unwrap().as_deref(), Some("Aa"));
    assert_eq!(reader.read_i32().unwrap(), 0);
    assert_eq!(reader.read_string16().unwrap().as_deref(), Some("last"));
    assert!(!reader.read_bool().unwrap());
    assert_eq!(reader.remaining(), 0);
    for uid in [0, 1000, 10101] {
        let reply = call(
            &state,
            uid,
            false,
            pm::GET_SUSPENDED_PACKAGE_APP_EXTRAS,
            |p| {
                pm::GetSuspendedPackageAppExtras {
                    package_name: Some(APP.into()),
                    user_id: 0,
                }
                .write(p)
            },
        );
        assert_eq!(
            Reader::new(reply.data(), reply.objects())
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_SECURITY
        );
    }
}

#[test]
fn native_block_uninstall_reads_global_user_set_without_cross_user_gate() {
    let mut state = state();
    state.platform.query_filtering_disabled = true;
    state.system.uninstall_blocks = Some(Arc::new(crate::package::mutations::UninstallBlocks(
        [(10, vec![Some(APP.into())])].into())));
    let parcel = call(
        &state,
        10100,
        false,
        pm::GET_BLOCK_UNINSTALL_FOR_USER,
        |p| {
            pm::GetBlockUninstallForUser {
                package_name: Some(APP.into()),
                user_id: 10,
            }
            .write(p);
        },
    );
    assert!(
        pm::read_get_block_uninstall_for_user_reply(&mut Reader::new(
            parcel.data(),
            parcel.objects()
        ))
        .unwrap()
        .unwrap()
    );
    let parcel = call(
        &state,
        10100,
        false,
        pm::GET_BLOCK_UNINSTALL_FOR_USER,
        |p| {
            pm::GetBlockUninstallForUser {
                package_name: Some("missing".into()),
                user_id: 10,
            }
            .write(p);
        },
    );
    assert!(
        !pm::read_get_block_uninstall_for_user_reply(&mut Reader::new(
            parcel.data(),
            parcel.objects()
        ))
        .unwrap()
        .unwrap()
    );
    state.system.uninstall_blocks = None;
    let filter = AppsFilter::new(&state, &Default::default()).unwrap();
    let query = Query {
        state: &state,
        filter: &filter,
        calling_uid: 1000,
    };
    let mut args = Parcel::new();
    pm::GetBlockUninstallForUser {
        package_name: Some(APP.into()),
        user_id: 0,
    }
    .write(&mut args);
    assert!(
        query
            .extra_package(
                pm::GET_BLOCK_UNINSTALL_FOR_USER,
                &mut Reader::new(args.data(), args.objects())
            )
            .is_err()
    );
}

#[test]
fn can_package_query_uses_source_visibility_and_original_missing_package_exception() {
    let mut state = state();
    let targets = Some(vec![Some(OTHER.into()), Some(APP.into())]);
    let parcel = call(&state, 1000, false, pm::CAN_PACKAGE_QUERY, |p| {
        pm::CanPackageQuery {
            source_package_name: Some(APP.into()),
            target_package_names: targets.clone(),
            user_id: 0,
        }
        .write(p);
    });
    assert_eq!(
        pm::read_can_package_query_reply(&mut Reader::new(parcel.data(), parcel.objects()))
            .unwrap()
            .unwrap(),
        Some(vec![false, true])
    );
    state.platform.query_filtering_disabled = true;
    let parcel = call(&state, 1000, false, pm::CAN_PACKAGE_QUERY, |p| {
        pm::CanPackageQuery {
            source_package_name: Some(APP.into()),
            target_package_names: targets,
            user_id: 0,
        }
        .write(p);
    });
    assert_eq!(
        pm::read_can_package_query_reply(&mut Reader::new(parcel.data(), parcel.objects()))
            .unwrap()
            .unwrap(),
        Some(vec![true, true])
    );
    let parcel = call(&state, 10100, false, pm::CAN_PACKAGE_QUERY, |p| {
        pm::CanPackageQuery {
            source_package_name: Some(APP.into()),
            target_package_names: Some(vec![Some("missing".into())]),
            user_id: 0,
        }
        .write(p);
    });
    let error = pm::read_can_package_query_reply(&mut Reader::new(parcel.data(), parcel.objects()))
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, aim_binder_host::parcel::EX_PARCELABLE);
    let payload = error.parcelable.unwrap();
    let mut reader = Reader::new(payload.bytes(), &[]);
    assert_eq!(
        reader.read_string16().unwrap().as_deref(),
        Some("android.os.ParcelableException")
    );
    assert_eq!(
        reader.read_string16().unwrap().as_deref(),
        Some("android.content.pm.PackageManager$NameNotFoundException")
    );
    assert_eq!(
        reader.read_string16().unwrap().as_deref(),
        Some("Package(s) org.example.app and/or [missing] not found.")
    );
    let parcel = call(&state, 10100, false, pm::CAN_PACKAGE_QUERY, |p| {
        pm::CanPackageQuery {
            source_package_name: None,
            target_package_names: Some(vec![None]),
            user_id: 999,
        }
        .write(p);
    });
    assert_eq!(
        pm::read_can_package_query_reply(&mut Reader::new(parcel.data(), parcel.objects()))
            .unwrap()
            .unwrap(),
        Some(vec![false])
    );
}

#[test]
fn activity_supports_intent_matches_registered_filters_without_enabled_state_check() {
    use crate::package::{intent::Intent, intent_filter::{IntentFilter, ParsedIntentInfo}};
    let mut state = state();
    let app = state.packages.get_mut(APP).unwrap();
    let package = Arc::make_mut(app.pkg.as_mut().unwrap());
    package.activities[1].main.component.intents.push(ParsedIntentInfo {
        filter: IntentFilter { actions: vec!["p.action".into()], ..Default::default() }, ..Default::default() });
    let filter = AppsFilter::new(&state, &Default::default()).unwrap();
    let query = Query { state: &state, filter: &filter, calling_uid: 1000 };
    assert!(query.activity_supports_intent(Some(&ComponentName {
        package: APP.into(), class: "org.example.app.B".into() }),
        Some(&Intent { action: Some("p.action".into()), ..Default::default() }), None, 0).unwrap().unwrap());
    assert!(query.activity_supports_intent(Some(&ComponentName {
        package: "android".into(), class: "com.android.internal.app.ResolverActivity".into() }),
        None, None, 0).unwrap().unwrap());
}

#[test]
fn lifecycle_queries_use_real_safe_mode_ce_storage_and_counted_freeze_owners() {
    use crate::package::{lifecycle::Owner, settings::{Settings, Version}};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let ce_calls = Arc::new(AtomicUsize::new(0));
    let calls = ce_calls.clone();
    let owner = Owner::from_settings(&Settings { versions: vec![Version {
        fingerprint: Some("current".into()), ..Default::default() }], ..Default::default() },
        true, "current", Box::new(|_| None), Box::new(move |_| { calls.fetch_add(1, Ordering::SeqCst); Ok(false) })).unwrap();
    let published = Arc::new(std::sync::Mutex::new((0, std::collections::BTreeMap::new())));
    let publication = published.clone();
    owner.install_frozen_publisher(Arc::new(move |revision, frozen| {
        *publication.lock().unwrap() = (revision, frozen);
        Ok(())
    })).unwrap();
    let mut state = state();
    state.system.lifecycle = Some(owner.clone());
    let pkg = Arc::make_mut(state.packages.get_mut(APP).unwrap().pkg.as_mut().unwrap());
    pkg.booleans |= booleans::PERSISTENT;
    let filter = AppsFilter::new(&state, &Default::default()).unwrap();
    let query = Query { state: &state, filter: &filter, calling_uid: 1000 };
    assert!(query.check_startable(Some(APP), 0).unwrap().unwrap_err().message.contains("not encryption aware"));
    assert_eq!(ce_calls.load(Ordering::SeqCst), 1);
    assert!(query.check_startable(Some("missing"), 0).unwrap().unwrap_err().message.contains("was not found"));
    assert_eq!(ce_calls.load(Ordering::SeqCst), 2);
    let frozen = owner.freeze(APP.into()).unwrap();
    assert_eq!(published.lock().unwrap().1.get(APP), Some(&1));
    assert!(query.check_startable(Some(APP), 0).unwrap().unwrap_err().message.contains("currently frozen"));
    frozen.close().unwrap();
    assert!(published.lock().unwrap().1.is_empty());
    assert_eq!(published.lock().unwrap().0, 2);
    assert_eq!(query.persistent_applications(MATCH_DIRECT_BOOT_UNAWARE as i32).unwrap().len(), 1);
    assert!(query.persistent_applications(MATCH_DIRECT_BOOT_AWARE as i32).unwrap().is_empty());
    assert_eq!(query.package_startability_captured(false, Some(APP), 10101, 0).unwrap(), 1);
    assert_eq!(query.package_startability_captured(false, Some(APP), 10100, 0).unwrap(), 4);
    owner.enter_safe_mode(1000).unwrap();
    assert_eq!(query.persistent_applications_captured(false, MATCH_DIRECT_BOOT_UNAWARE as i32).unwrap().len(), 1);
    assert_eq!(query.package_startability_captured(false, Some(APP), 10100, 0).unwrap(), 4);
    assert_eq!(query.package_startability_captured(true, Some(APP), 10100, 0).unwrap(), 2);
    assert!(query.persistent_applications(MATCH_DIRECT_BOOT_UNAWARE as i32).unwrap().is_empty());
    assert!(query.check_startable(Some(APP), 0).unwrap().unwrap_err().message.contains("not a system app"));
}

#[test]
fn provider_info_uses_live_user_unlock_state_after_initial_locked_capture() {
    use aim_binder_driver::{Driver,Device,Credentials,GuestProcess,Errno,File,errno,uapi::*};
    use aim_binder_host::local::{LocalProcess,Service,Call,Reply};
    use aim_binder_host::parcel::{Binder,UNKNOWN_TRANSACTION,EX_ILLEGAL_STATE};
    use aim_service_aidl::dev_aim_server_ipackagebootcontextleaf as leaf;
    use std::sync::atomic::{AtomicI32,AtomicUsize,Ordering};
    struct NoMemory;
    impl GuestProcess for NoMemory {
        fn copy_from_user(&mut self,_:u64,_:&mut[u8])->Result<(),Errno>{Err(errno::EFAULT)}
        fn copy_to_user(&mut self,_:u64,_:&[u8])->Result<(),Errno>{Err(errno::EFAULT)}
        fn get_file(&mut self,_:u32)->Result<File,Errno>{Err(errno::EBADF)}
        fn install_file(&mut self,_:File)->Result<u32,Errno>{Err(errno::EBADF)}
        fn close_fd(&mut self,_:u32){panic!("unexpected fd")}
    }
    struct UserState {phase:Arc<AtomicI32>,reads:Arc<AtomicUsize>}
    impl Service for UserState {
        fn descriptor(&self)->&str{leaf::DESCRIPTOR}
        fn transact(&self,call:&mut Call<'_>)->Reply {
            if call.code!=leaf::USER_UNLOCKING_OR_UNLOCKED{return Err(UNKNOWN_TRANSACTION)}
            assert_eq!(call.sender_euid,1000);
            let args=leaf::UserUnlockingOrUnlocked::read(&mut call.data)?;
            assert_eq!(args.user_id,0);assert_eq!(call.data.remaining(),0);
            self.reads.fetch_add(1,Ordering::AcqRel);
            let mut reply=Parcel::new();
            if self.phase.load(Ordering::Acquire)==2{reply.write_exception(&aim_binder_host::parcel::Exception::new(EX_ILLEGAL_STATE,"UM owner read failed"));}
            else{leaf::write_user_unlocking_or_unlocked_reply(&mut reply,self.phase.load(Ordering::Acquire)==1);}
            Ok(reply)
        }
    }
    struct Processes {driver:Arc<Driver>,server:Arc<LocalProcess>,client:Arc<LocalProcess>}
    impl Drop for Processes {fn drop(&mut self){self.driver.release(self.client.proc_handle());self.driver.release(self.server.proc_handle());}}
    let driver=Driver::new();
    let open=|pid|LocalProcess::open(&driver,Device::Binder,Credentials{pid,euid:1000,security_context:None});
    let server=open(99101);let client=open(99102);
    let _processes=Processes{driver:driver.clone(),server:server.clone(),client:client.clone()};
    let phase=Arc::new(AtomicI32::new(0));let reads=Arc::new(AtomicUsize::new(0));
    let Binder::Local(ptr)=server.add_service(Arc::new(UserState{phase:phase.clone(),reads:reads.clone()}))else{unreachable!()};
    let mut object=FlatBinderObject{kind:BINDER_TYPE_BINDER,flags:0,binder:ptr,cookie:ptr}.encode();
    driver.ioctl(server.proc_handle(),99103,BINDER_SET_CONTEXT_MGR_EXT,&mut object,&mut NoMemory).unwrap();
    server.start();client.start();
    let mut state=state();state.users.get_mut(&0).unwrap().unlocking_or_unlocked=false;
    state.platform.settings_owner=Some(crate::package::resolve::settings::Owner::new(Arc::new(client.strong(0)),false));
    let code=Arc::make_mut(state.packages.get_mut(APP).unwrap().pkg.as_mut().unwrap());
    let mut provider=crate::package::pkg::Provider::default();
    provider.main.component.name="fixture.StartupProvider".into();provider.main.component.package_name=APP.into();
    provider.main.enabled=true;provider.main.direct_boot_aware=false;provider.main.exported=false;
    provider.main.component.meta_data=Some(MetaData(vec![("fixture.initializer".into(),Meta::Bool(true))]));
    provider.authority=Some("fixture.startup".into());code.providers.push(provider);
    let component=ComponentName{package:APP.into(),class:"fixture.StartupProvider".into()};
    let filter=AppsFilter::new(&state,&Default::default()).unwrap();
    let query=Query{state:&state,filter:&filter,calling_uid:10100};
    assert!(query.provider_info(&component,GET_META_DATA,0).unwrap().unwrap().is_none());
    phase.store(1,Ordering::Release);
    let info=query.provider_info(&component,GET_META_DATA,0).unwrap().unwrap().unwrap();
    assert_eq!(info.info.item.meta_data.as_ref().unwrap().0[0].0,"fixture.initializer");
    assert!(!state.users[&0].unlocking_or_unlocked);
    assert_eq!(reads.load(Ordering::Acquire),2);
    assert!(query.provider_info(&component,GET_META_DATA|MATCH_DIRECT_BOOT_AWARE,0).unwrap().unwrap().is_none());
    phase.store(0,Ordering::Release);
    assert!(query.provider_info(&component,GET_META_DATA|MATCH_DIRECT_BOOT_UNAWARE,0).unwrap().unwrap().is_some());
    assert_eq!(reads.load(Ordering::Acquire),2);
    phase.store(2,Ordering::Release);
    assert!(query.provider_info(&component,GET_META_DATA,0).is_err());
    assert_eq!(reads.load(Ordering::Acquire),3);
}

#[test]
fn metadata_provider_query_skips_absent_empty_and_unrelated_bundles() {
    let mut state=state();
    let code=Arc::make_mut(state.packages.get_mut(APP).unwrap().pkg.as_mut().unwrap());
    code.providers=[("fixture.Absent",None),("fixture.Empty",Some(MetaData(vec![]))),
        ("fixture.Unrelated",Some(MetaData(vec![("another-key".into(),Meta::Bool(true))]))),
        ("fixture.Directory",Some(MetaData(vec![("directory-key".into(),Meta::Bool(true))])))]
        .into_iter().map(|(name,metadata)|{
            let mut provider=crate::package::pkg::Provider::default();
            provider.main.component.package_name=APP.into();provider.main.component.name=name.into();
            provider.main.component.meta_data=metadata;provider.main.enabled=true;
            provider.main.direct_boot_aware=true;provider.authority=Some(format!("{APP}.{name}"));provider
        }).collect();
    let filter=AppsFilter::new(&state,&Default::default()).unwrap();
    let query=Query{state:&state,filter:&filter,calling_uid:10100};
    let result=query.content_providers(None,0,GET_META_DATA,Some("directory-key")).unwrap().unwrap();
    assert_eq!(result.len(),1);
    assert_eq!(result[0].info.item.name.as_deref(),Some("fixture.Directory"));
    assert_eq!(result[0].info.item.meta_data.as_ref().unwrap().0,vec![("directory-key".into(),Meta::Bool(true))]);
    assert_eq!(query.content_providers(None,0,0,None).unwrap().unwrap().len(),4);
    let without_metadata=query.content_providers(None,0,0,Some("directory-key")).unwrap().unwrap();
    assert_eq!(without_metadata.len(),1);assert!(without_metadata[0].info.item.meta_data.is_none());
    let reply=call(&state,10100,false,pm::QUERY_CONTENT_PROVIDERS,|parcel|{
        pm::QueryContentProviders{process_name:None,uid:0,flags:GET_META_DATA,meta_data_key:Some("directory-key".into())}.write(parcel);
    });
    let mut reader=reply.reader();reader.read_exception().unwrap().unwrap();
    assert_eq!(reader.read_i32().unwrap(),1);assert_eq!(reader.read_i32().unwrap(),1);
    assert_eq!(reader.read_string16().unwrap().as_deref(),Some("android.content.pm.ProviderInfo"));
}

#[test]
fn uid_package_queries_distinguish_regular_isolated_compute_and_sdk_sandbox() {
    use aim_binder_driver::{Driver,Device,Credentials,GuestProcess,Errno,File,errno,uapi::*};
    use aim_binder_host::local::{LocalProcess,Service,Call,Reply};
    use aim_binder_host::parcel::{Binder,UNKNOWN_TRANSACTION,EX_ILLEGAL_STATE};
    use aim_service_aidl::dev_aim_server_ipackageresolutionpolicy as leaf;
    struct NoMemory;
    impl GuestProcess for NoMemory {
        fn copy_from_user(&mut self,_:u64,_:&mut[u8])->Result<(),Errno>{Err(errno::EFAULT)}
        fn copy_to_user(&mut self,_:u64,_:&[u8])->Result<(),Errno>{Err(errno::EFAULT)}
        fn get_file(&mut self,_:u32)->Result<File,Errno>{Err(errno::EBADF)}
        fn install_file(&mut self,_:File)->Result<u32,Errno>{Err(errno::EBADF)}
        fn close_fd(&mut self,_:u32){panic!("unexpected fd")}
    }
    struct Classification;
    impl Service for Classification {
        fn descriptor(&self)->&str{leaf::DESCRIPTOR}
        fn transact(&self,call:&mut Call<'_>)->Reply {
            if call.code!=leaf::IS_KNOWN_ISOLATED_COMPUTE_APP{return Err(UNKNOWN_TRANSACTION)}
            assert_eq!(call.sender_euid,1000);
            let args=leaf::IsKnownIsolatedComputeApp::read(&mut call.data)?;
            assert_eq!(call.data.remaining(),0);
            let mut reply=Parcel::new();leaf::write_is_known_isolated_compute_app_reply(&mut reply,args.uid==99002);Ok(reply)
        }
    }
    struct Processes {driver:Arc<Driver>,server:Arc<LocalProcess>,client:Arc<LocalProcess>}
    impl Drop for Processes {fn drop(&mut self){self.driver.release(self.client.proc_handle());self.driver.release(self.server.proc_handle());}}
    let driver=Driver::new();let open=|pid|LocalProcess::open(&driver,Device::Binder,Credentials{pid,euid:1000,security_context:None});
    let server=open(99301);let client=open(99302);
    let _guard=Processes{driver:driver.clone(),server:server.clone(),client:client.clone()};
    let Binder::Local(ptr)=server.add_service(Arc::new(Classification))else{unreachable!()};
    let mut object=FlatBinderObject{kind:BINDER_TYPE_BINDER,flags:0,binder:ptr,cookie:ptr}.encode();
    driver.ioctl(server.proc_handle(),99303,BINDER_SET_CONTEXT_MGR_EXT,&mut object,&mut NoMemory).unwrap();server.start();client.start();
    let mut state=state();state.system.resolution_policy=Some(crate::package::resolve::policy::Owner::new(client.strong(0)));
    state.system.isolated_owners=vec![(99000,10100),(99002,10100)];
    state.packages.insert("fixture.sdk".into(),package("fixture.sdk",10300,|ps,_|{
        ps.users.insert(1,PackageUserState{installed:false,..Default::default()});
    }));
    state.system.sdk_sandbox_package=Some(Some("fixture.sdk".into()));
    state.users.insert(1,User{id:1,..Default::default()});
    let filter=AppsFilter::new(&state,&Default::default()).unwrap();
    let query=Query{state:&state,filter:&filter,calling_uid:1000};
    assert_eq!(query.packages_for_uid(99000).unwrap(),None); // AM owner alone is not compute classification.
    assert_eq!(query.packages_for_uid(99001).unwrap(),None); // Unregistered isolated UID is an ordinary lookup miss.
    assert_eq!(query.packages_for_uid(99002).unwrap(),Some(vec![Some(APP.into())]));
    assert_eq!(query.packages_for_uid(20017).unwrap(),Some(vec![Some("fixture.sdk".into())]));
    assert_eq!(query.packages_for_uid(120017).unwrap(),None); // Keep target user before base SDK UID substitution.
    assert_eq!(query.name_for_uid(20017).unwrap().as_deref(),Some("fixture.sdk"));
    assert_eq!(query.name_for_uid(99000).unwrap(),None);
    assert_eq!(query.names_for_uids(Some(&[20017,99000,99002])).unwrap(),Some(vec![Some("fixture.sdk".into()),None,Some(APP.into())]));
    let isolated=Query{state:&state,filter:&filter,calling_uid:99000};
    assert_eq!(isolated.packages_for_uid(99000).unwrap(),None);
    let reply=call(&state,1000,false,pm::GET_PACKAGES_FOR_UID,|parcel|pm::GetPackagesForUid{uid:99000}.write(parcel));
    assert_eq!(pm::read_get_packages_for_uid_reply(&mut reply.reader()).unwrap().unwrap(),None);
    state.system.isolated_owners.retain(|(uid,_)|*uid!=99002);
    let filter=AppsFilter::new(&state,&Default::default()).unwrap();
    let query=Query{state:&state,filter:&filter,calling_uid:1000};
    assert_eq!(query.name_for_uid(99002).unwrap(),None); // Original name getter catches missing compute owner.
    let reply=call(&state,1000,false,pm::GET_PACKAGES_FOR_UID,|parcel|pm::GetPackagesForUid{uid:99002}.write(parcel));
    let error=reply.reader().read_exception().unwrap().unwrap_err();
    assert_eq!(error.code,EX_ILLEGAL_STATE);assert_eq!(error.message,"No owner UID found for isolated UID 99002");
}

#[test]
fn am_default_only_resolution_differs_from_unfiltered_launcher_query() {
    use crate::package::{intent::{Intent,ComponentName},intent_filter::{IntentFilter,ParsedIntentInfo},resolve::Resolution};
    let mut state=state();state.users.retain(|user,_|*user==0);
    let package=state.packages.get_mut(APP).unwrap();package.users.get_mut(&0).unwrap().stopped=true;
    let code=Arc::make_mut(package.pkg.as_mut().unwrap());code.target_sdk_version=28;
    let mut filter=IntentFilter::default();filter.add_action("android.intent.action.MAIN");filter.add_category("android.intent.category.LAUNCHER");
    let mut activity=crate::package::pkg::Activity::default();activity.main.component.name="fixture.Launcher".into();
    activity.main.component.package_name=APP.into();activity.main.component.intents=vec![ParsedIntentInfo{filter,has_default:false,..Default::default()}];
    activity.main.enabled=true;activity.main.exported=true;activity.main.direct_boot_aware=true;code.activities=vec![activity];
    let resolution=Resolution::new(Arc::new(state),&Default::default()).unwrap();
    let intent=Intent{action:Some("android.intent.action.MAIN".into()),categories:Some(vec!["android.intent.category.LAUNCHER".into()]),package:Some(APP.into()),flags:0x10000000,..Default::default()};
    let ordinary=resolution.resolve_intent(&intent,None,0,0,0).unwrap().unwrap();
    assert_eq!(ordinary.component().1,"fixture.Launcher");assert!(!ordinary.is_default);
    // Actual ActivityTaskSupervisor image uses MATCH_DEFAULT_ONLY | GET_SHARED_LIBRARY_FILES.
    let flags=0x10400;
    assert!(resolution.resolve_intent_internal_record(Some(&intent),None,flags,0,0,true,0,1,1000,1).unwrap().is_none());
    let explicit=Intent{component:Some(ComponentName{package:APP.into(),class:"fixture.Launcher".into()}),..intent.clone()};
    assert!(resolution.resolve_intent_internal_record(Some(&explicit),None,flags,0,0,true,0,1,1000,1).unwrap().is_some());
}
