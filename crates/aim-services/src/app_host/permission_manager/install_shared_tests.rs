//! AppIdPermissionPolicy request unions and shared install grants, android-16.0.0_r1.
use super::*;
use crate::package::pkg::{AndroidPackage, Component, Permission as ParsedPermission};

fn member(name: &str, requested: &[&str]) -> PackageState {
    PackageState {
        name: name.into(),
        app_id: 10100,
        shared_user: Some("shared".into()),
        shared_user_app_id: Some(10100),
        pkg: Some(Arc::new(AndroidPackage {
            package_name: name.into(),
            target_sdk_version: 36,
            requested_permissions: requested.iter().map(|p| (*p).into()).collect(),
            ..Default::default()
        })),
        ..Default::default()
    }
}
fn installation(ps: &PackageState, grant: bool) -> Install {
    Install {
        name: ps.name.clone(),
        app_id: ps.app_id,
        path: ps.path.clone(),
        version: 1,
        code: (**ps.pkg.as_ref().unwrap()).clone(),
        previous_code: None,
        states: if grant {
            ps.pkg
                .as_ref()
                .unwrap()
                .requested_permissions
                .iter()
                .map(|p| (p.clone(), 1))
                .collect()
        } else {
            BTreeMap::new()
        },
        allowlist: BTreeSet::new(),
    }
}
#[test]
fn shared_install_unions_grants_then_update_trims_only_unused_requests() {
    let mut state = State::default();
    state
        .packages
        .insert("a".into(), member("a", &["normal", "camera"]));
    let empty = permissions::AccessView {
        system: Arc::new(AccessSystem {
            permissions: [("normal", 0), ("camera", 1), ("other", 1)]
                .into_iter()
                .map(|(name, level)| Permission {
                    name: name.into(),
                    package: "android".into(),
                    protection_level: level,
                    dynamic: None,
                })
                .collect(),
            ..Default::default()
        }),
        user0: Arc::new(AccessUser::default()),
    };
    let policy = signature::Policy::default();
    let first = prepare(
        &empty,
        &state,
        &[installation(&state.packages["a"], true)],
        &policy,
    )
    .unwrap();
    state.packages.insert("b".into(), member("b", &["other"]));
    let second = prepare(
        &first,
        &state,
        &[installation(&state.packages["b"], true)],
        &policy,
    )
    .unwrap();
    assert_eq!(
        second.user0.permission_flags(10100, "normal"),
        INSTALL_GRANTED
    );
    assert_eq!(
        second.user0.permission_flags(10100, "camera"),
        RUNTIME_GRANTED
    );
    assert_eq!(
        second.user0.permission_flags(10100, "other"),
        RUNTIME_GRANTED
    );
    // Both packages see exactly the same UID-scoped grants.
    crate::system::with_discovery_session(|host, _, _| {
        let mut context = (**host.package_capture().unwrap().context()).clone();
        context.packages.clear();
        for name in ["a", "b"] {
            context.packages.insert(
                (name.into(), false),
                crate::package::scan_snapshot::query_state::PackageInputs {
                    app_id: 10100,
                    path: String::new(),
                    version: 1,
                    installed_permissions: vec![],
                    domain_verification: None,
                    uri_relative_filter_groups: vec![],
                    filter_application_query: false,
                    syncable_authorities: vec![],
                    users: [(
                        0,
                        crate::package::scan_snapshot::query_state::UserInputs {
                            gids: vec![],
                            granted_permissions: vec![],
                            domain_selection: None,
                        },
                    )]
                    .into(),
                },
            );
        }
        second.project_context(&mut context, &state, &Default::default());
        assert_eq!(
            context.packages[&("a".into(), false)].users[&0].granted_permissions,
            context.packages[&("b".into(), false)].users[&0].granted_permissions
        );
        assert_eq!(
            context.packages[&("b".into(), false)].users[&0]
                .granted_permissions
                .len(),
            3
        );
    });
    state.packages.insert("a".into(), member("a", &["normal"]));
    let update = prepare(
        &second,
        &state,
        &[installation(&state.packages["a"], false)],
        &policy,
    )
    .unwrap();
    assert_eq!(update.user0.permission_flags(10100, "camera"), 0);
    assert_eq!(
        update.user0.permission_flags(10100, "other"),
        RUNTIME_GRANTED
    );
    // A multi-package atomic install evaluates one UID; it cannot trim a sibling's grants.
    let batch = prepare(
        &empty,
        &state,
        &state
            .packages
            .values()
            .map(|p| installation(p, true))
            .collect::<Vec<_>>(),
        &policy,
    )
    .unwrap();
    assert_eq!(
        batch.user0.permission_flags(10100, "normal"),
        INSTALL_GRANTED
    );
    assert_eq!(
        batch.user0.permission_flags(10100, "other"),
        RUNTIME_GRANTED
    );
}
#[test]
fn shared_reevaluation_uses_all_requesters_and_minimum_target_sdk() {
    let mut state = State::default();
    let mut a = member("a", &["runtime", "signature"]);
    a.signatures = Some(crate::package::settings::Signatures {
        signatures: vec![vec![1]],
        ..Default::default()
    });
    let code = Arc::make_mut(a.pkg.as_mut().unwrap());
    code.target_sdk_version = 22;
    code.permissions.push(ParsedPermission {
        component: Component {
            name: "signature".into(),
            package_name: "a".into(),
            ..Default::default()
        },
        protection_level: PROTECTION_SIGNATURE,
        ..Default::default()
    });
    state.packages.insert("a".into(), a);
    state
        .packages
        .insert("b".into(), member("b", &["runtime", "signature"]));
    let base = permissions::AccessView {
        system: Arc::new(AccessSystem {
            permissions: vec![Permission {
                name: "runtime".into(),
                package: "android".into(),
                protection_level: 1,
                dynamic: None,
            }],
            ..Default::default()
        }),
        user0: Arc::new(AccessUser::default()),
    };
    // Add the declaration through the install policy, also reevaluating existing shared users.
    let view = prepare(
        &base,
        &state,
        &[
            installation(&state.packages["a"], false),
            installation(&state.packages["b"], false),
        ],
        &signature::Policy::default(),
    )
    .unwrap();
    assert_ne!(
        view.user0.permission_flags(10100, "runtime") & LEGACY_GRANTED,
        0
    );
    assert_eq!(
        view.user0.permission_flags(10100, "signature"),
        PROTECTION_GRANTED
    );
    let persisted = super::super::tests::PersistedAccess::new();
    let manager = PermissionManager::new(persisted.host.clone());
    manager.write_system(&view.system).unwrap();
    manager.write_user0(&view.user0).unwrap();
    let reload = PermissionManager::new(persisted.host.clone())
        .retained_access()
        .unwrap();
    assert_eq!(*reload.system, *view.system);
    assert_eq!(*reload.user0, *view.user0);
}
