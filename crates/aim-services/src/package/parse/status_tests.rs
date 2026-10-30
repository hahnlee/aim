//! Rejections from android-16.0.0_r1 ParsingPackageUtils and ApkLiteParseUtils.
use super::*;
use aim_apps::res::{Attr, Value};

fn check(tag: &str, attr: &str, value: &str, status: i32, message: &str) {
    check_with(tag, attr, value, status, message, |p, pkg, e| match tag {
        "application" => p.parse_base_application(pkg, e),
        "uses-sdk" => {
            e.attrs[0].kind = TYPE_FIRST_INT;
            e.attrs[0].data = value.parse().unwrap();
            p.parse_uses_sdk(pkg, e)
        }
        "attribution" => {
            for _ in 0..2 {
                pkg.attributions.push(component::Attribution {
                    tag: "duplicate".into(),
                    label: 1,
                    inherit_from: vec![],
                });
            }
            p.validate_base_apk_tags(pkg)
        }
        _ => {
            if attr == "sharedUserId" {
                p.parse_base_apk_tags(pkg, e)
            } else {
                p.parse_base_apk(e, "base.apk", "/code").map(|_| ())
            }
        }
    });
}

fn check_with(
    tag: &str,
    attr: &str,
    value: &str,
    status: i32,
    message: &str,
    read: impl FnOnce(&Parser<'_>, &mut Package, &mut Element) -> Result<()>,
) {
    let mut e = Element {
        name: tag.into(),
        text: String::new(),
        children: vec![],
        attrs: vec![Attr {
            ns: if attr == "package" || attr == "split" {
                ""
            } else {
                ANDROID
            }
            .into(),
            name: attr.into(),
            id: 1,
            value: Value::String(value.into()),
            kind: TYPE_STRING,
            data: 0,
        }],
    };
    let table = hex_bytes(
        "02000c00280000000000000001001c001c0000000000000000000000000000001c00000000000000",
    );
    let platform = Platform {
        sdk: 36,
        codenames: vec![],
        sdk_extensions: Some(HashMap::new()),
        features: HashSet::new(),
        flags: HashMap::new(),
        flag_packages: HashSet::new(),
        split_permissions: vec![],
        locale: (*b"en", *b"US"),
        use_round_icon: false,
        recents_limit: 16,
        framework: Table::parse(&table).unwrap(),
        framework_overlays: vec![],
        framework_overlay_apks: vec![],
        framework_attrs: std::iter::once((attr.into(), 1))
            .chain(
                "name version versionMajor host port mimeType certDigest priority order label \
                 icon autoVerify mimeGroup scheme ssp sspPrefix sspPattern sspAdvancedPattern \
                 sspSuffix path pathPrefix pathPattern pathAdvancedPattern pathSuffix \
                 protectionLevel permissionFlags"
                    .split_whitespace()
                    .filter(|name| *name != attr)
                    .enumerate()
                    .map(|(i, name)| (name.into(), i as u32 + 2)),
            )
            .collect(),
        density_dpi: Some(160),
    };
    let res = Resources {
        tables: vec![&platform.framework],
        overlays: &[],
        config: platform.config(),
    };
    let input = RefCell::new(Input::default());
    let p = Parser {
        platform: &platform,
        flags: PARSE_APK_IN_APEX,
        ctx: Ctx {
            res: &res,
            attrs: &platform.framework_attrs,
            error: RefCell::new(None),
        },
        input: &input,
    };
    let mut pkg = Package::new("org.example.test", "base.apk", "/code");
    let result = read(&p, &mut pkg, &mut e);
    match result {
        Err(Error::Status(code, text)) => assert_eq!((code, text.as_str()), (status, message)),
        Err(error) => panic!("status-less error: {error:?}"),
        Ok(_) => panic!("manifest accepted"),
    }
}
#[test]
fn malformed_manifest_and_application() {
    let message = "Empty class name in package org.example.test";
    check("application", "name", "", -108, message);
    let message = "Bad process name : in package org.example.test: must be at least two characters";
    check("application", "process", ":", -108, message);
    let message = "Invalid class loader name: bad";
    check("application", "classLoader", "bad", -108, message);
}
#[test]
fn bad_package_name() {
    let message = "Invalid manifest package: Invalid filename";
    check("manifest", "package", "", -106, message);
}
#[test]
fn missing_package_is_unexpected_exception() {
    let message = "Failed to read manifest from base.apk";
    check("manifest", "other", "", -102, message);
}
#[test]
fn bad_shared_user_id() {
    let message =
        "<manifest> specifies bad sharedUserId name \"bad\": must have at least one '.' separator";
    check("manifest", "sharedUserId", "bad", -107, message);
}
#[test]
fn bad_attribution_combination() {
    let message = "Combination <attribution> tags are not valid";
    check("attribution", "", "", -101, message);
}
#[test]
fn requires_older_sdk() {
    let message = "Requires newer sdk version #37 (current version is #36)";
    check("uses-sdk", "minSdkVersion", "37", -12, message);
}
#[test]
fn requires_newer_sdk() {
    let message = "Requires max SDK version 35 but is 36";
    check("uses-sdk", "maxSdkVersion", "35", -14, message);
}

fn add_int(p: &Parser<'_>, e: &mut Element, name: &str, data: u32) {
    let mut a = e.attrs[0].clone();
    a.id = p.ctx.attrs[name];
    a.name = name.into();
    a.kind = TYPE_FIRST_INT;
    a.data = data;
    e.attrs.push(a);
}

#[test]
fn malformed_child_checks_keep_status_and_message() {
    check_with(
        "intent-filter",
        "mimeType",
        "bad",
        -108,
        "android.content.IntentFilter$MalformedMimeTypeException: bad",
        |p, pkg, e| {
            e.children.push(Element {
                name: "data".into(),
                ..e.clone()
            });
            p.parse_intent_info(pkg, e, true, false).map(|_| ())
        },
    );
    check_with(
        "uses-static-library",
        "name",
        "lib",
        -108,
        "Bad uses-static-library declaration name: lib version: -1 certDigestnull",
        |p, pkg, e| p.parse_base_app_child_tag(pkg, e),
    );
}

#[test]
fn shared_user_is_rejected_by_both_library_owners() {
    for (tag, version, kind) in [
        ("sdk-library", "versionMajor", "SDK library"),
        ("static-library", "version", "static shared library"),
    ] {
        check_with(
            tag,
            "name",
            "lib",
            -107,
            &format!("sharedUserId not allowed in {kind}"),
            |p, pkg, e| {
                pkg.shared_user_id = Some("org.example.shared".into());
                add_int(p, e, version, 1);
                p.parse_base_app_child_tag(pkg, e)
            },
        );
    }
}

#[test]
fn thrown_attribute_and_port_errors_use_the_current_apk_path() {
    for path in ["base.apk", "split_feature.apk"] {
        let message = format!("Failed to read manifest from {path}");
        check_with(
            "manifest",
            "versionCode",
            "bad",
            -102,
            &message,
            |p, _, e| {
                p.read_manifest(path, || {
                    p.obtain(e).integer("versionCode", 0);
                    Ok(())
                })
            },
        );
        check_with(
            "intent-filter",
            "port",
            "bad",
            -102,
            &message,
            |p, pkg, e| {
                let mut data = e.clone();
                data.name = "data".into();
                let mut host = data.attrs[0].clone();
                host.id = p.ctx.attrs["host"];
                host.value = Value::String("example.com".into());
                data.attrs.push(host);
                e.children.push(data);
                let result = p.parse_intent_info(pkg, e, true, false).map(|_| ());
                assert!(matches!(result, Err(Error::Status(-102, _))));
                assert!(p.ctx.error.borrow().is_none());
                p.read_manifest(path, || result)
            },
        );
    }
}

#[test]
fn conflicting_platform_permission_restrictions_throw() {
    check_with(
        "permission",
        "name",
        "android.permission.TEST",
        -102,
        "Failed to read manifest from base.apk",
        |p, pkg, e| {
            pkg.package_name = "android".into();
            add_int(p, e, "protectionLevel", 1);
            add_int(p, e, "permissionFlags", 12);
            let result = p.parse_permission(pkg, e).map(|_| ());
            assert!(matches!(result, Err(Error::Status(-102, _))));
            p.read_manifest("base.apk", || result)
        },
    );
}
