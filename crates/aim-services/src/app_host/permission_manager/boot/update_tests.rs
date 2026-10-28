use super::super::tests::PersistedAccess;
use super::tests::{admissions, declaration, package};
use super::*;

fn runtime_state(names: &[&str], implicit: &[&str]) -> State {
    let mut client = package("client", 10002, vec![], names);
    Arc::make_mut(client.pkg.as_mut().unwrap()).implicit_permissions =
        implicit.iter().map(|s| (*s).into()).collect();
    State {
        packages: [
            (
                "owner".into(),
                package(
                    "owner",
                    10001,
                    names.iter().map(|n| declaration(n, 1, false)).collect(),
                    &[],
                ),
            ),
            ("client".into(), client),
        ]
        .into(),
        ..Default::default()
    }
}

fn evaluate(
    base: &permissions::AccessView,
    state: &State,
    policy: &Policy,
    updated: bool,
) -> permissions::AccessView {
    // Real old-package receipts prove an update without code changes.
    let mut receipts = admissions(state);
    for receipt in &mut receipts {
        receipt.old_package = state.packages[&receipt.name].pkg.as_deref().cloned();
    }
    reconcile(
        base,
        state,
        &receipts.iter().collect::<Vec<_>>(),
        updated,
        policy,
        &|p, info, target, old, new| Ok(evaluate_permission(p, info, target, old, new)),
    )
    .unwrap()
}

fn existing(state: &State) -> permissions::AccessView {
    let empty = permissions::AccessView {
        system: Arc::default(),
        user0: Arc::default(),
    };
    evaluate(&empty, state, &Policy::default(), false)
}

fn splits(sources: &[&str], name: &str) -> Policy {
    Policy {
        splits: sources
            .iter()
            .map(|s| SplitPermission {
                name: (*s).into(),
                new_permissions: vec![name.into()],
                target_sdk: 33,
            })
            .collect(),
        ..Default::default()
    }
}

#[test]
fn update_split_inheritance_uses_evaluated_sources_and_keeps_existing_decisions() {
    // Policy.kt:1238-1287; PMSImpl.java:3076-3114. Destination precedes
    // sources by name; a shared UID sees the union of implicit requests.
    let mut state = runtime_state(&["a.NEW", "z.SOURCE", "z.OTHER"], &["a.NEW"]);
    let mut peer = package("peer", 10003, vec![], &["z.SOURCE"]);
    peer.shared_user_app_id = Some(10002);
    state.packages.insert("peer".into(), peer);
    let policy = splits(&["z.OTHER", "z.SOURCE"], "a.NEW");
    for (source, other, expected) in [
        (
            RUNTIME_GRANTED | USER_SET,
            USER_FIXED,
            RUNTIME_GRANTED | USER_SET | IMPLICIT,
        ),
        (USER_SET, USER_FIXED, USER_SET | USER_FIXED | IMPLICIT),
        (
            RUNTIME_GRANTED | SYSTEM_FIXED,
            RUNTIME_GRANTED | PREGRANT,
            RUNTIME_GRANTED | SYSTEM_FIXED | PREGRANT | IMPLICIT,
        ),
        (
            USER_SET | USER_FIXED,
            POLICY_FIXED,
            USER_SET | USER_FIXED | POLICY_FIXED | IMPLICIT,
        ),
    ] {
        let mut base = existing(&state);
        let user = Arc::make_mut(&mut base.user0);
        user.set_permission_flags(10002, "a.NEW", 0);
        user.set_permission_flags(10002, "z.SOURCE", source);
        user.set_permission_flags(10002, "z.OTHER", other);
        let after = evaluate(&base, &state, &policy, true);
        assert_eq!(after.user0.permission_flags(10002, "a.NEW"), expected);
        let again = evaluate(&after, &state, &policy, true);
        assert_eq!(*again.user0, *after.user0);
        let mut denied = after.clone();
        Arc::make_mut(&mut denied.user0).set_permission_flags(
            10002,
            "a.NEW",
            USER_FIXED | IMPLICIT,
        );
        assert_eq!(
            evaluate(&denied, &state, &policy, true)
                .user0
                .permission_flags(10002, "a.NEW"),
            USER_FIXED | IMPLICIT
        );
    }
}

#[test]
fn implicit_static_sources_media_exceptions_and_explicit_request_revocation() {
    let mut state = runtime_state(&["a.NEW", "z.SOURCE"], &["a.NEW"]);
    Arc::make_mut(
        state
            .packages
            .get_mut("owner")
            .unwrap()
            .pkg
            .as_mut()
            .unwrap(),
    )
    .permissions[1]
        .protection_level = 0;
    let policy = splits(&["z.SOURCE"], "a.NEW");
    let mut base = existing(&state);
    Arc::make_mut(&mut base.user0).set_permission_flags(10002, "a.NEW", USER_SET | IMPLICIT);
    let after = evaluate(&base, &state, &policy, true);
    assert_eq!(
        after.user0.permission_flags(10002, "a.NEW"),
        USER_SET | IMPLICIT | IMPLICIT_GRANTED | APP_OP_REVOKED
    );
    // Implicit -> explicit drops compatibility grants and the user's old denial.
    Arc::make_mut(
        state
            .packages
            .get_mut("client")
            .unwrap()
            .pkg
            .as_mut()
            .unwrap(),
    )
    .implicit_permissions
    .clear();
    assert_eq!(
        evaluate(&after, &state, &policy, true)
            .user0
            .permission_flags(10002, "a.NEW"),
        0
    );

    for name in [
        "android.permission.ACTIVITY_RECOGNITION",
        "android.permission.ACCESS_MEDIA_LOCATION",
        "android.permission.READ_MEDIA_AUDIO",
        "android.permission.READ_MEDIA_IMAGES",
        "android.permission.READ_MEDIA_VIDEO",
        "android.permission.READ_MEDIA_VISUAL_USER_SELECTED",
    ] {
        for target in [22, 36] {
            let mut state = runtime_state(&[name, "z.SOURCE"], &[name]);
            Arc::make_mut(
                state
                    .packages
                    .get_mut("client")
                    .unwrap()
                    .pkg
                    .as_mut()
                    .unwrap(),
            )
            .target_sdk_version = target;
            let mut base = existing(&state);
            Arc::make_mut(&mut base.user0).set_permission_flags(10002, name, 0);
            Arc::make_mut(&mut base.user0).set_permission_flags(
                10002,
                "z.SOURCE",
                RUNTIME_GRANTED | USER_SET,
            );
            let after = evaluate(&base, &state, &splits(&["z.SOURCE"], name), true);
            assert_eq!(
                after.user0.permission_flags(10002, name) & IMPLICIT != 0,
                target < M
            );
            assert!(permission_granted(
                after.user0.permission_flags(10002, name)
            ));
        }
    }
    // A split from an ungranted non-runtime source is still compatibility-granted.
    Arc::make_mut(
        state
            .packages
            .get_mut("client")
            .unwrap()
            .pkg
            .as_mut()
            .unwrap(),
    )
    .implicit_permissions
    .push("a.NEW".into());
    Arc::make_mut(
        state
            .packages
            .get_mut("owner")
            .unwrap()
            .pkg
            .as_mut()
            .unwrap(),
    )
    .permissions[1]
        .protection_level = 4;
    let mut base = existing(&state);
    Arc::make_mut(&mut base.user0).set_permission_flags(10002, "a.NEW", 0);
    assert_eq!(
        evaluate(&base, &state, &policy, true)
            .user0
            .permission_flags(10002, "a.NEW"),
        IMPLICIT_GRANTED | IMPLICIT
    );
}

#[test]
fn explicit_nearby_requests_retain_only_fixed_or_background_location_grants() {
    let name = "android.permission.BLUETOOTH_SCAN";
    let state = runtime_state(&[name, ACCESS_BACKGROUND_LOCATION], &[]);
    for background in [
        0,
        RUNTIME_GRANTED,
        RUNTIME_GRANTED | APP_OP_REVOKED,
        RUNTIME_GRANTED | IMPLICIT,
    ] {
        for fixed in [0, SYSTEM_FIXED, POLICY_FIXED] {
            let mut base = existing(&state);
            let user = Arc::make_mut(&mut base.user0);
            user.set_permission_flags(10002, name, IMPLICIT_GRANTED | IMPLICIT | USER_SET | fixed);
            user.set_permission_flags(10002, ACCESS_BACKGROUND_LOCATION, background);
            let after = evaluate(&base, &state, &Policy::default(), true);
            let retain = fixed != 0 || background == RUNTIME_GRANTED;
            assert_eq!(
                after.user0.permission_flags(10002, name),
                if retain {
                    RUNTIME_GRANTED | USER_SET | fixed
                } else {
                    0
                }
            );
        }
    }
}

#[test]
fn internal_protection_uses_pinned_flag_values_and_never_signature_trust() {
    for (flag, kind) in [
        (0x100, 2),
        (0x200, 4),
        (0x800, 1),
        (0x10000, 6),
        (0x80000, 10),
        (0x100000, 11),
        (0x200000, 12),
        (0x800000, 15),
        (0x2000000, 17),
    ] {
        for selected in [false, true] {
            let mut state = runtime_state(&["a.INTERNAL"], &[]);
            Arc::make_mut(
                state
                    .packages
                    .get_mut("owner")
                    .unwrap()
                    .pkg
                    .as_mut()
                    .unwrap(),
            )
            .permissions[0]
                .protection_level = 4 | flag;
            state.system.roles = Some(crate::package::roles::Owner::known_packages_fixture(
                [
                    (
                        kind,
                        if selected {
                            vec![Some("client".into())]
                        } else {
                            vec![]
                        },
                    ),
                    (7, vec![]),
                ]
                .into(),
            ));
            // Selection is authoritative even for a non-system requester.
            let after = evaluate(&existing(&state), &state, &Policy::default(), true);
            assert_eq!(
                after.user0.permission_flags(10002, "a.INTERNAL"),
                if selected { PROTECTION_GRANTED } else { 0 }
            );
        }
    }
    for (level, module, system, expected) in [
        (4, false, false, 0),
        (4 | 0x400000, false, true, 0),
        (4 | 0x400000, true, false, PROTECTION_GRANTED),
        (4 | 0x400, false, true, PROTECTION_GRANTED),
        (
            4 | PROTECTION_FLAG_ROLE,
            false,
            true,
            ROLE | RUNTIME_GRANTED,
        ),
    ] {
        let mut state = runtime_state(&["a.INTERNAL"], &[]);
        Arc::make_mut(
            state
                .packages
                .get_mut("owner")
                .unwrap()
                .pkg
                .as_mut()
                .unwrap(),
        )
        .permissions[0]
            .protection_level = level;
        let client = state.packages.get_mut("client").unwrap();
        client.signatures = state_signatures();
        client.apex_module_name = module.then(|| "module".into());
        client.is.system = system;
        state.packages.get_mut("owner").unwrap().signatures = state_signatures();
        let mut base = existing(&state);
        if level & PROTECTION_FLAG_ROLE != 0 {
            Arc::make_mut(&mut base.user0).set_permission_flags(
                10002,
                "a.INTERNAL",
                ROLE | RUNTIME_GRANTED,
            );
        }
        assert_eq!(
            evaluate(&base, &state, &Policy::default(), true)
                .user0
                .permission_flags(10002, "a.INTERNAL"),
            expected
        );
    }
}

fn state_signatures() -> Option<crate::package::settings::Signatures> {
    Some(crate::package::settings::Signatures {
        signatures: vec![vec![1]],
        ..Default::default()
    })
}

#[test]
fn access_version_upgrade_runs_after_evaluation_and_preserves_fixed_health_grants() {
    let names = [
        "android.permission.BODY_SENSORS",
        "android.permission.health.READ_HEART_RATE",
        "android.permission.BODY_SENSORS_BACKGROUND",
        "android.permission.health.READ_HEALTH_DATA_IN_BACKGROUND",
    ];
    for version in [15, 16, 17, 18] {
        for fixed in [0, SYSTEM_FIXED, POLICY_FIXED] {
            let mut state = runtime_state(&names, &[]);
            Arc::make_mut(
                state
                    .packages
                    .get_mut("client")
                    .unwrap()
                    .pkg
                    .as_mut()
                    .unwrap(),
            )
            .target_sdk_version = 35;
            let mut base = existing(&state);
            let user = Arc::make_mut(&mut base.user0);
            user.package_versions
                .iter_mut()
                .find(|(name, _)| name == "client")
                .unwrap()
                .1 = version;
            user.set_permission_flags(
                10002,
                names[0],
                RUNTIME_GRANTED | USER_SET | PREGRANT | fixed,
            );
            user.set_permission_flags(
                10002,
                names[2],
                RUNTIME_GRANTED | USER_SET | PREGRANT | fixed,
            );
            for updated in [false, true] {
                let after = evaluate(&base, &state, &Policy::default(), updated);
                let expected = if version < 17 && fixed == 0 {
                    0
                } else {
                    RUNTIME_GRANTED | USER_SET | PREGRANT | fixed
                };
                assert_eq!(after.user0.permission_flags(10002, names[0]), expected);
                assert_eq!(after.user0.permission_flags(10002, names[2]), expected);
                assert_eq!(
                    after
                        .user0
                        .package_versions
                        .iter()
                        .find(|(name, _)| name == "client")
                        .unwrap()
                        .1,
                    version.max(17)
                );
            }
        }
    }
}

#[test]
fn access_upgrade_version_boundaries_and_grant_guards() {
    let names = [
        "android.permission.ACCESS_FINE_LOCATION",
        ACCESS_BACKGROUND_LOCATION,
        "android.permission.READ_EXTERNAL_STORAGE",
        "android.permission.WRITE_EXTERNAL_STORAGE",
        "android.permission.ACCESS_MEDIA_LOCATION",
        "android.permission.READ_MEDIA_IMAGES",
        "android.permission.READ_MEDIA_VISUAL_USER_SELECTED",
        "android.permission.BODY_SENSORS",
        "android.permission.BODY_SENSORS_BACKGROUND",
        "android.permission.SEND_SMS",
    ];
    let state = runtime_state(&names, &[]);
    let base = existing(&state);
    for version in [3, 4, 10, 11, 12, 13, 14, 15, 16] {
        let mut user = (*base.user0).clone();
        for name in [names[0], names[2], names[7]] {
            user.set_permission_flags(10002, name, RUNTIME_GRANTED | USER_SET);
        }
        upgrade::package(&base.system, &mut user, &state.packages["client"], version).unwrap();
        assert_eq!(
            user.permission_flags(10002, ACCESS_BACKGROUND_LOCATION) & RUNTIME_GRANTED != 0,
            version <= 3
        );
        assert_eq!(
            user.permission_flags(10002, names[9]) & UPGRADE_EXEMPT != 0,
            version <= 3
        );
        assert_eq!(
            user.permission_flags(10002, names[4]) & RUNTIME_GRANTED != 0,
            version <= 12
        );
        assert_eq!(
            user.permission_flags(10002, names[5]) & RUNTIME_GRANTED != 0,
            version <= 12
        );
        assert_eq!(
            user.permission_flags(10002, names[6]) & RUNTIME_GRANTED != 0,
            version <= 12
        );
        assert_eq!(
            user.permission_flags(10002, names[8]) & (UPGRADE_EXEMPT | RUNTIME_GRANTED) != 0,
            version <= 12
        );
    }
    for guard in [USER_SET, USER_FIXED, POLICY_FIXED, SYSTEM_FIXED, ONE_TIME] {
        let mut user = (*base.user0).clone();
        user.set_permission_flags(10002, names[2], RUNTIME_GRANTED);
        user.set_permission_flags(10002, names[4], guard);
        upgrade::package(&base.system, &mut user, &state.packages["client"], 10).unwrap();
        assert_eq!(user.permission_flags(10002, names[4]), guard);
    }
    let mut client = state.packages["client"].clone();
    client.users.get_mut(&0).unwrap().instant_app = true;
    let mut user = (*base.user0).clone();
    user.set_permission_flags(10002, names[2], RUNTIME_GRANTED);
    upgrade::package(&base.system, &mut user, &client, 10).unwrap();
    assert_eq!(user.permission_flags(10002, names[4]), 0);
    client.users.get_mut(&0).unwrap().instant_app = false;
    user.set_permission_flags(10002, names[5], RUNTIME_GRANTED | USER_SET);
    upgrade::package(&base.system, &mut user, &client, 14).unwrap();
    assert_eq!(user.permission_flags(10002, names[6]), RUNTIME_GRANTED);
}

#[test]
fn missing_split_source_fails_without_publishing_partial_access() {
    let state = runtime_state(&["a.NEW"], &["a.NEW"]);
    let fixture = PersistedAccess::new();
    let manager = PermissionManager::new(fixture.host.clone());
    let base = manager.retained_access().unwrap();
    let receipts = admissions(&state);
    let error = reconcile(
        &base,
        &state,
        &receipts.iter().collect::<Vec<_>>(),
        true,
        &splits(&["unknown"], "a.NEW"),
        &|_, _, _, _, _| unreachable!(),
    )
    .unwrap_err();
    assert_eq!(error.code, EX_ILLEGAL_STATE);
    assert!(manager.retained_access().unwrap().same_base(&base));
}

#[test]
fn pinned_image_packages_update_existing_access_without_a_system_process() {
    use crate::package::{
        parse::Platform,
        scan::{Apex, Image, Partition},
        write::Apks,
    };
    let Some(root) = aim_paths::original_image_with("system/framework/framework-res.apk") else {
        return;
    };
    let config = crate::package::system_config::SystemConfig::read(&root, &|_| None);
    let mut platform = Platform::load(
        &root,
        config
            .features
            .iter()
            .map(|(name, _)| name.clone())
            .collect(),
    )
    .unwrap();
    // Supply this host fixture's display metrics for manifest dimensions.
    platform.density_dpi = Some(320);
    let files = root.clone();
    let apks = Apks {
        platform,
        signing_overrides: None,
        // No parser cache or writable path is mapped into the original image.
        files: Box::new(move |guest| {
            (!guest.starts_with("/data/") && guest != "/.identity")
                .then(|| files.join(guest.trim_start_matches('/')))
        }),
    };
    let mut apexes = std::fs::read_dir(root.join("apex"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| !name.contains('@') && root.join("apex").join(name).is_dir())
        .map(|name| Apex {
            module_name: Some(name.clone()),
            mount_path: format!("/apex/{name}"),
            partition: Partition::System,
            factory: true,
            active_changed: false,
        })
        .collect::<Vec<_>>();
    apexes.sort_by(|a, b| a.mount_path.cmp(&b.mount_path));
    let image = Image::parse(&apks, &apexes).unwrap();
    // The scan also considers framework ABI directories and compressed-code
    // directories. They contain no APK; their stub packages are parsed instead.
    for rejected in &image.rejected {
        assert_eq!(
            rejected.reason, "No packages found in split",
            "{rejected:?}"
        );
        assert!(
            std::fs::read_dir(root.join(rejected.location.path.trim_start_matches('/')))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|e| e == "apk"))
        );
    }
    assert!(image.packages.len() > 100);
    let mut state = State::default();
    state.users.insert(
        0,
        crate::package::model::User {
            id: 0,
            unlocking_or_unlocked: true,
            ..Default::default()
        },
    );
    state.shared_libraries = Some(Vec::new());
    state.system.flags = apks
        .platform
        .flags
        .iter()
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    let mut shared = BTreeMap::new();
    for (index, code) in image.packages.into_iter().enumerate() {
        let mut parsed = code.parsed;
        // System-image boot collects the real certificate history without
        // rehashing trusted partition contents (collectCertificatesLI).
        let signing = apks
            .collect_signing_details(
                &parsed,
                crate::package::write::CertificateCollection {
                    saved: None,
                    database_version: 3,
                    force_collect: false,
                    skip_verify: true,
                    pre_n_mr1_upgrade: false,
                },
            )
            .unwrap();
        let signatures = crate::package::settings::Signatures {
            public_keys: signing.serialized_public_keys().unwrap(),
            scheme_version: signing.scheme_version,
            signatures: signing.signatures,
            current_flags: signing.current_flags,
            past_signatures: signing.past_signing_certificates,
        };
        let name = parsed.package_name.clone();
        let id = if name == "android" {
            1000
        } else {
            10000 + index as i32
        };
        let shared_id = parsed
            .shared_user_id
            .as_ref()
            .map(|name| *shared.entry(name.clone()).or_insert(id));
        parsed.uid = shared_id.unwrap_or(id);
        parsed.booleans |= crate::package::pkg::booleans::SYSTEM;
        parsed.signing_details = crate::package::sign::SigningDetails::from_saved(&signatures)
            .unwrap()
            .package_details()
            .unwrap();
        for library in &parsed.library_names {
            state
                .shared_libraries
                .as_mut()
                .unwrap()
                .push(crate::package::model::SharedLibrary {
                    name: Some(library.clone()),
                    package_name: Some(name.clone()),
                    version: -1,
                    ..Default::default()
                });
        }
        let ps = PackageState {
            name: name.clone(),
            app_id: id,
            shared_user_app_id: shared_id,
            shared_user: parsed.shared_user_id.clone(),
            path: code.location.path.clone(),
            target_sdk_version: parsed.target_sdk_version,
            signatures: Some(signatures),
            apex_module_name: code
                .location
                .apex
                .as_ref()
                .and_then(|a| a.module_name.clone()),
            is: crate::package::model::StateFlags {
                system: true,
                privileged: code.location.privileged(),
                vendor: code.location.partition == Partition::Vendor,
                odm: code.location.partition == Partition::Odm,
                oem: code.location.partition == Partition::Oem,
                product: code.location.partition == Partition::Product,
                system_ext: code.location.partition == Partition::SystemExt,
                ..Default::default()
            },
            pkg: Some(Arc::new(parsed)),
            users: [(0, Default::default())].into(),
            ..Default::default()
        };
        assert!(
            state.packages.insert(name, ps).is_none(),
            "duplicate image package requires scan reconciliation"
        );
    }
    let roles = crate::package::roles::Owner::load(
        &Arc::new(state.clone()),
        &apks.platform,
        apks.platform.config(),
    )
    .unwrap();
    state.system.roles = Some(Arc::new(
        roles
            .prepare_known_packages(
                &Arc::new(state.clone()),
                &apks.platform,
                apks.platform.config(),
                config.overlay_config_signature_package().unwrap(),
            )
            .unwrap(),
    ));
    let policy = Policy {
        splits: crate::package::parse::platform::split_permissions(&root).unwrap(),
        allowlist: config.permission_allowlist,
        ..Default::default()
    };
    // Establish the same package set at version 17, then persist real user
    // decisions, including denials and dynamic role grants, before updating.
    let empty = permissions::AccessView {
        system: Arc::default(),
        user0: Arc::default(),
    };
    let mut before = evaluate(&empty, &state, &policy, false);
    let mut implicit_count = 0;
    let mut internal_count = 0;
    for ps in state.packages.values() {
        let id = ps.shared_user_app_id.unwrap_or(ps.app_id);
        for name in &ps.pkg.as_ref().unwrap().requested_permissions {
            let Some(p) = before.system.permissions.iter().find(|p| &p.name == name) else {
                continue;
            };
            let old = before.user0.permission_flags(id, name);
            if is_runtime(p) {
                let decision = if info::java_hash(name) & 1 == 0 {
                    RUNTIME_GRANTED | USER_SET
                } else {
                    USER_FIXED
                };
                Arc::make_mut(&mut before.user0).set_permission_flags(id, name, old | decision);
                implicit_count +=
                    usize::from(ps.pkg.as_ref().unwrap().implicit_permissions.contains(name));
            } else if p.protection_level & PROTECTION_MASK_BASE == 4 {
                internal_count += 1;
                if is_role(p) {
                    Arc::make_mut(&mut before.user0).set_permission_flags(
                        id,
                        name,
                        old | ROLE | RUNTIME_GRANTED,
                    );
                }
            }
        }
    }
    assert!(implicit_count > 0 && internal_count > 0);
    let data = crate::app_host::test_temp_dir("permission-image-update");
    let host = AppHost::new(crate::test_process(1000));
    crate::app_host::configure_test_host(&host, &root, &data, &data, Box::new(|_| None));
    let manager = PermissionManager::new(host.clone());
    manager.write_system(&before.system).unwrap();
    manager.write_user0(&before.user0).unwrap();
    let mut receipts = admissions(&state);
    for receipt in &mut receipts {
        receipt.old_package = state.packages[&receipt.name].pkg.as_deref().cloned();
    }
    let after = manager
        .reconcile_boot(&state, &receipts.iter().collect::<Vec<_>>(), true)
        .unwrap();
    // No source, target, declaration or version changed, so even implicit
    // user decisions survive the update; the next mount makes no writes.
    assert_eq!(*after.system, *before.system);
    assert_eq!(*after.user0, *before.user0);
    let reload = PermissionManager::new(host).retained_access().unwrap();
    assert_eq!(*after.user0, *reload.user0);
    let writes = manager.access_writes.lock().unwrap().len();
    assert!(
        manager
            .reconcile_boot(&state, &receipts.iter().collect::<Vec<_>>(), false)
            .unwrap()
            .same_base(&after)
    );
    assert_eq!(manager.access_writes.lock().unwrap().len(), writes);
    eprintln!(
        "pinned inventory: {} packages, {implicit_count} implicit runtime requests, {internal_count} internal requests",
        state.packages.len()
    );
    std::fs::remove_dir_all(data).unwrap();
}
