use std::sync::Arc;

use super::*;
use crate::package::pkg::AndroidPackage;
use crate::package::system_config::Library;

fn policy(native: bool, independence: bool) -> Policy {
    Policy {
        enforce_native_dependencies: native,
        sdk_library_independence: independence,
    }
}

#[test]
fn graph_resolves_multihop_paths_and_nested_apk_dependencies() {
    let mut registry = Registry::new(&SystemConfig::default());
    registry.insert(SharedLibrary {
        name: Some("builtin".into()),
        version: VERSION_UNDEFINED,
        path: Some("/system/framework/builtin.jar".into()),
        ..Default::default()
    });
    let mut available = BTreeMap::new();
    for (name, dependencies) in [("a", vec!["b", "builtin"]), ("b", vec!["c"]), ("c", vec![])] {
        let mut ps = package(name, |p| {
            p.library_names = vec![name.into()];
            p.uses_libraries = dependencies.iter().map(|n| (*n).into()).collect();
        });
        ps.is.system = true;
        registry.add_package(&ps, None).unwrap();
        available.insert(name.into(), ps);
    }
    available.insert(
        "app".into(),
        package("app", |p| p.uses_libraries = vec!["a".into(), "b".into()]),
    );
    let resolved = registry
        .resolve(&available, &|_| Ok(policy(false, false)))
        .unwrap();
    assert_eq!(
        resolved.packages["app"].uses_library_files,
        [
            "/data/app/a/base.apk",
            "/data/app/b/base.apk",
            "/data/app/c/base.apk",
            "/system/framework/builtin.jar"
        ]
    );
    let a = resolved.registry.get("a", VERSION_UNDEFINED).unwrap();
    assert_eq!(a.dependencies.len(), 1);
    assert_eq!(a.dependencies[0].name.as_deref(), Some("b"));
    assert_eq!(a.dependencies[0].dependencies[0].name.as_deref(), Some("c"));
    assert_eq!(resolved.packages["app"].uses_library_infos[0], *a);
    assert!(available["app"].uses_library_files.is_empty());
}

#[test]
fn graph_marks_static_libraries_installed_for_the_consumers_users() {
    use crate::package::{model::PackageUserState, settings::Signatures};
    use sha2::{Digest, Sha256};
    let mut registry = Registry::new(&SystemConfig::default());
    let mut provider = package("provider", |p| {
        p.static_shared_library_name = Some("static".into());
        p.static_shared_lib_version = 1;
    });
    provider.signatures = Some(Signatures {
        signatures: vec![b"certificate".to_vec()],
        ..Default::default()
    });
    provider.users = BTreeMap::from([
        (
            0,
            PackageUserState {
                installed: false,
                ..Default::default()
            },
        ),
        (
            10,
            PackageUserState {
                installed: false,
                ..Default::default()
            },
        ),
    ]);
    registry.add_package(&provider, None).unwrap();
    let digest = Sha256::digest(b"certificate")
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect();
    let mut app = package("app", |p| {
        p.uses_static_libraries = vec!["static".into()];
        p.uses_static_libraries_versions = Some(vec![1]);
        p.uses_static_libraries_cert_digests = Some(vec![Some(vec![Some(digest)])]);
    });
    app.users = BTreeMap::from([
        (0, Default::default()),
        (
            10,
            PackageUserState {
                installed: false,
                ..Default::default()
            },
        ),
    ]);
    let mut available = BTreeMap::from([("provider".into(), provider), ("app".into(), app)]);
    let resolved = registry
        .resolve(&available, &|_| Ok(policy(false, false)))
        .unwrap();
    assert!(resolved.packages["provider"].users[&0].installed);
    assert!(!resolved.packages["provider"].users[&10].installed);
    assert!(!available["provider"].users[&0].installed);
    available.get_mut("provider").unwrap().users.remove(&0);
    assert_eq!(
        registry
            .resolve(&available, &|_| Ok(policy(false, false)))
            .unwrap_err()
            .cause,
        ResolveError::Incomplete("static library user state")
    );
}

#[test]
fn graph_reports_cyclic_scan_order_without_publishing_partial_state() {
    let mut registry = Registry::new(&SystemConfig::default());
    let mut available = BTreeMap::new();
    for (name, dep) in [("a", "b"), ("b", "a")] {
        let mut ps = package(name, |p| {
            p.library_names = vec![name.into()];
            p.uses_libraries = vec![dep.into()];
        });
        ps.is.system = true;
        registry.add_package(&ps, None).unwrap();
        available.insert(name.into(), ps);
    }
    let before = available.clone();
    assert_eq!(
        registry
            .resolve(&available, &|_| Ok(policy(false, false)))
            .unwrap_err()
            .cause,
        ResolveError::Incomplete("cyclic library provider scan order")
    );
    assert_eq!(available, before);
    assert!(registry.entries().all(|l| l.dependencies.is_empty()));
}

#[test]
fn dependency_order_and_provider_files_preserve_first_occurrence() {
    let mut registry = Registry::new(&SystemConfig::default());
    let mut provider = package("provider", |p| {
        p.library_names = vec!["dynamic".into()];
        p.split_code_paths = Some(vec![Some("/data/app/provider/feature.apk".into())]);
    });
    provider.is.system = true;
    provider.uses_library_files = vec![
        "/system/framework/transitive.jar".into(),
        "/data/app/provider/base.apk".into(),
    ];
    registry.add_package(&provider, None).unwrap();
    registry.insert(SharedLibrary {
        name: Some("builtin".into()),
        version: VERSION_UNDEFINED,
        path: Some("/system/framework/transitive.jar".into()),
        ..Default::default()
    });
    let available = BTreeMap::from([(provider.name.clone(), provider)]);
    let app = package("app", |p| {
        p.uses_libraries = vec!["dynamic".into(), "builtin".into()];
        p.uses_optional_libraries = vec!["absent".into()];
    });
    let selection = registry
        .collect(app.pkg.as_ref().unwrap(), &available, policy(false, false))
        .unwrap();
    assert_eq!(
        selection
            .libraries
            .iter()
            .map(|p| p.name.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["dynamic", "builtin"]
    );
    assert_eq!(
        selection.files(&available).unwrap(),
        [
            "/data/app/provider/base.apk",
            "/data/app/provider/feature.apk",
            "/system/framework/transitive.jar"
        ]
    );
}

#[test]
fn native_and_sdk_missing_dependencies_follow_explicit_policy() {
    let registry = Registry::new(&SystemConfig::default());
    let available = BTreeMap::new();
    let app = package("app", |p| p.uses_native_libraries = vec!["native".into()]);
    assert!(
        registry
            .collect(app.pkg.as_ref().unwrap(), &available, policy(false, false))
            .unwrap()
            .libraries
            .is_empty()
    );
    assert_eq!(
        registry
            .collect(app.pkg.as_ref().unwrap(), &available, policy(true, false))
            .unwrap_err(),
        ResolveError::MissingLibrary("native".into())
    );
    let mut app = package("app", |p| {
        p.uses_sdk_libraries = vec!["sdk".into()];
        p.uses_sdk_libraries_versions_major = Some(vec![1]);
        p.uses_sdk_libraries_optional = Some(vec![true]);
    });
    assert!(
        registry
            .collect(app.pkg.as_ref().unwrap(), &available, policy(false, true))
            .unwrap()
            .libraries
            .is_empty()
    );
    assert_eq!(
        registry
            .collect(app.pkg.as_ref().unwrap(), &available, policy(false, false))
            .unwrap_err(),
        ResolveError::MissingLibrary("sdk".into())
    );
    Arc::make_mut(app.pkg.as_mut().unwrap()).uses_sdk_libraries_optional = Some(vec![false]);
    assert_eq!(
        registry
            .collect(app.pkg.as_ref().unwrap(), &available, policy(false, true))
            .unwrap_err(),
        ResolveError::MissingLibrary("sdk".into())
    );
}

#[test]
fn versioned_library_checks_rotation_and_rejects_bad_or_different_digests() {
    use crate::package::settings::Signatures;
    use sha2::{Digest, Sha256};
    let digest = |der: &[u8]| {
        Sha256::digest(der)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let mut registry = Registry::new(&SystemConfig::default());
    let mut provider = package("provider", |p| {
        p.static_shared_library_name = Some("static".into());
        p.static_shared_lib_version = 12;
    });
    provider.signatures = Some(Signatures {
        signatures: vec![b"current".to_vec()],
        past_signatures: Some(vec![(b"old".to_vec(), 0), (b"current".to_vec(), 0)]),
        ..Default::default()
    });
    registry.add_package(&provider, None).unwrap();
    let mut available = BTreeMap::from([(provider.name.clone(), provider)]);
    let mut app = package("app", |p| {
        p.uses_static_libraries = vec!["static".into()];
        p.uses_static_libraries_versions = Some(vec![12]);
        p.uses_static_libraries_cert_digests = Some(vec![Some(vec![Some(digest(b"old"))])]);
    });
    assert!(
        registry
            .collect(app.pkg.as_ref().unwrap(), &available, policy(false, false))
            .is_ok()
    );
    Arc::make_mut(app.pkg.as_mut().unwrap()).uses_static_libraries_cert_digests =
        Some(vec![Some(vec![Some("odd".into())])]);
    assert_eq!(
        registry
            .collect(app.pkg.as_ref().unwrap(), &available, policy(false, false))
            .unwrap_err(),
        ResolveError::BadCertificateDigest("static".into())
    );
    Arc::make_mut(app.pkg.as_mut().unwrap()).uses_static_libraries_cert_digests =
        Some(vec![Some(vec![Some(digest(b"other"))])]);
    assert_eq!(
        registry
            .collect(app.pkg.as_ref().unwrap(), &available, policy(false, false))
            .unwrap_err(),
        ResolveError::DifferentSigners("static".into())
    );
    available.get_mut("provider").unwrap().signatures = Some(Signatures {
        signatures: vec![b"first".to_vec(), b"second".to_vec()],
        ..Default::default()
    });
    let pkg = Arc::make_mut(app.pkg.as_mut().unwrap());
    pkg.target_sdk_version = 27;
    pkg.uses_static_libraries_cert_digests = Some(vec![Some(vec![
        Some(digest(b"second").to_uppercase()),
        Some(digest(b"first").to_uppercase()),
    ])]);
    assert!(
        registry
            .collect(app.pkg.as_ref().unwrap(), &available, policy(false, false))
            .is_ok()
    );
    Arc::make_mut(app.pkg.as_mut().unwrap()).target_sdk_version = 26;
    assert_eq!(
        registry
            .collect(app.pkg.as_ref().unwrap(), &available, policy(false, false))
            .unwrap_err(),
        ResolveError::DifferentSigners("static".into())
    );
}

fn package(name: &str, edit: impl FnOnce(&mut AndroidPackage)) -> PackageState {
    let mut pkg = AndroidPackage {
        package_name: name.into(),
        manifest_package_name: Some(name.into()),
        base_apk_path: Some(format!("/data/app/{name}/base.apk")),
        version_code: -1,
        version_code_major: 2,
        ..Default::default()
    };
    edit(&mut pkg);
    PackageState {
        name: name.into(),
        pkg: Some(Arc::new(pkg)),
        ..Default::default()
    }
}

#[test]
fn builtin_and_dynamic_registration_obey_system_ownership() {
    let mut config = SystemConfig::default();
    config.libraries = [(
        "builtin".into(),
        Library {
            name: "builtin".into(),
            filename: "/system/framework/builtin.jar".into(),
            dependencies: vec![],
            on_bootclasspath_since: None,
            on_bootclasspath_before: None,
            can_be_safely_ignored: false,
            native: true,
        },
    )]
    .into();
    let mut registry = Registry::new(&config);
    let mut app = package("app", |p| {
        p.library_names = vec!["builtin".into(), "dynamic".into()]
    });
    registry.add_package(&app, None).unwrap();
    assert!(registry.get("dynamic", VERSION_UNDEFINED).is_none());
    app.is.system = true;
    registry.add_package(&app, None).unwrap();
    let builtin = registry.get("builtin", VERSION_UNDEFINED).unwrap();
    assert_eq!(
        builtin.path.as_deref(),
        Some("/system/framework/builtin.jar")
    );
    assert!(builtin.native);
    assert_eq!(builtin.declaring, ("android".into(), 0));
    let dynamic = registry.get("dynamic", VERSION_UNDEFINED).unwrap();
    assert_eq!(dynamic.package_name.as_deref(), Some("app"));
    assert_eq!(dynamic.kind, TYPE_DYNAMIC);
    assert_eq!(dynamic.declaring, ("app".into(), 0x2_ffff_ffff));
    let mut other = package("other", |p| p.library_names = vec!["dynamic".into()]);
    other.is.system = true;
    registry.add_package(&other, None).unwrap();
    assert_eq!(
        registry
            .get("dynamic", VERSION_UNDEFINED)
            .unwrap()
            .package_name
            .as_deref(),
        Some("app")
    );
}

#[test]
fn static_and_sdk_libraries_keep_versions_internal_names_and_code_paths() {
    let mut registry = Registry::default();
    for version in [7, 8] {
        let mut ps = package("manifest", |p| {
            p.static_shared_library_name = Some("static".into());
            p.static_shared_lib_version = version;
            p.split_code_paths = Some(vec![Some("/data/app/manifest/split.apk".into())]);
        });
        ps.name = format!("manifest_{version}");
        registry.add_package(&ps, None).unwrap();
    }
    assert_eq!(registry.entries().count(), 2);
    let static_lib = registry.get("static", 7).unwrap();
    assert_eq!(static_lib.package_name.as_deref(), Some("manifest_7"));
    assert_eq!(static_lib.declaring, ("manifest".into(), 0x2_ffff_ffff));
    assert_eq!(static_lib.kind, TYPE_STATIC);
    assert_eq!(
        static_lib.code_paths.as_ref().unwrap(),
        &[
            "/data/app/manifest/base.apk",
            "/data/app/manifest/split.apk"
        ]
    );
    let sdk = package("sdk.app", |p| {
        p.sdk_library_name = Some("sdk".into());
        p.sdk_lib_version_major = 3;
    });
    registry.add_package(&sdk, None).unwrap();
    assert_eq!(registry.get("sdk", 3).unwrap().kind, TYPE_SDK_PACKAGE);
}

#[test]
fn system_updates_cannot_add_dynamic_libraries() {
    let mut registry = Registry::default();
    let original = package("app", |p| p.library_names = vec!["original".into()]);
    let mut update = package("app", |p| {
        p.library_names = vec!["original".into(), "new".into()]
    });
    update.is.system = true;
    update.is.updated_system_app = true;
    assert!(registry.add_package(&update, None).is_err());
    assert_eq!(registry.entries().count(), 0);
    registry.add_package(&update, Some(&original)).unwrap();
    assert!(registry.get("original", VERSION_UNDEFINED).is_some());
    assert!(registry.get("new", VERSION_UNDEFINED).is_none());
    let mut ordinary = package("ordinary", |_| {});
    ordinary.is.system = true;
    ordinary.is.updated_system_app = true;
    registry.add_package(&ordinary, None).unwrap();
}

#[test]
fn invalid_code_paths_leave_the_registry_unchanged() {
    let mut registry = Registry::default();
    let malformed = package("static.app", |p| {
        p.static_shared_library_name = Some("static".into());
        p.split_code_paths = Some(vec![None]);
    });
    assert!(registry.add_package(&malformed, None).is_err());
    assert_eq!(registry.entries().count(), 0);
}

#[test]
fn static_signer_selection_uses_the_greatest_strictly_older_nonnegative_version() {
    let mut registry = Registry::default();
    let mut settings = crate::package::settings::Settings::default();
    for version in [-2, -1, 0, 1, 4, 9, i64::MAX] {
        let name = format!("provider_{version}");
        let ps = package(&name, |p| {
            p.static_shared_library_name = Some("static".into());
            p.static_shared_lib_version = version;
        });
        registry.add_package(&ps, None).unwrap();
        settings.packages.push(crate::package::settings::Package {
            name,
            ..Default::default()
        });
    }
    let mut incoming = AndroidPackage {
        static_shared_library_name: Some("static".into()),
        ..Default::default()
    };
    for (version, expected) in [
        (i64::MIN, None),
        (-1, None),
        (0, None),
        (1, Some(0)),
        (4, Some(1)),
        (5, Some(4)),
        (9, Some(4)),
        (i64::MAX, Some(9)),
    ] {
        incoming.static_shared_lib_version = version;
        assert_eq!(
            registry
                .latest_static_setting(&incoming, &settings)
                .map(|p| p.name.clone()),
            expected.map(|v| format!("provider_{v}"))
        );
    }
    incoming.static_shared_lib_version = 5;
    settings.packages.retain(|p| p.name != "provider_4");
    assert!(
        registry
            .latest_static_setting(&incoming, &settings)
            .is_none()
    );
    assert!(settings.packages.iter().any(|p| p.name == "provider_1")); // no second fallback
    incoming.static_shared_library_name = None;
    assert!(
        registry
            .latest_static_setting(&incoming, &settings)
            .is_none()
    );
}
