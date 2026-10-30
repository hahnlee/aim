//! Rejections from android-16.0.0_r1 ParsingPackageUtils and ApkLiteParseUtils.
use super::*;
use aim_apps::res::{Attr, Value};

fn check(tag: &str, attr: &str, value: &str, status: i32, message: &str) {
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
        framework_attrs: [(attr.into(), 1)].into(),
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
    let result = match tag {
        "application" => p.parse_base_application(&mut pkg, &e),
        "uses-sdk" => {
            e.attrs[0].kind = TYPE_FIRST_INT;
            e.attrs[0].data = value.parse().unwrap();
            p.parse_uses_sdk(&mut pkg, &e)
        }
        "attribution" => {
            for _ in 0..2 {
                pkg.attributions.push(component::Attribution {
                    tag: "duplicate".into(),
                    label: 1,
                    inherit_from: vec![],
                });
            }
            p.validate_base_apk_tags(&mut pkg)
        }
        _ => {
            if attr == "sharedUserId" {
                p.parse_base_apk_tags(&mut pkg, &e)
            } else {
                p.parse_base_apk(&e, "base.apk", "/code").map(|_| ())
            }
        }
    };
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
