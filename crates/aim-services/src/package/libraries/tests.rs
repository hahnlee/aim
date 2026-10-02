use std::sync::Arc;

use super::*;
use crate::package::pkg::AndroidPackage;
use crate::package::system_config::Library;

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
