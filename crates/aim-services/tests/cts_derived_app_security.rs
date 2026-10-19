//! CTS-derived tests (ADR 0013, "Conformance bar") of CtsAppSecurityHostTestCases
//! (android-cts-16.0_r1), run against the native parser with the module's own
//! APKs and the pinned image's platform, without booting Android.
use aim_services::package::parse::{self, Platform};

fn apk(name: &str) -> std::path::PathBuf {
    let path = aim_paths::fetched()
        .join("cts-tradefed/android-cts/testcases/CtsAppSecurityHostTestCases/zip-aligned")
        .join(name);
    assert!(
        path.is_file(),
        "pinned CTS input {} must be available",
        path.display()
    );
    path
}

fn parse(name: &str) -> Result<String, parse::Error> {
    let platform = Platform::load(&aim_paths::original_image(), Default::default()).unwrap();
    parse::parse(&apk(name), "/data/app/~~test/base.apk", 0, &platform)
        .map(|package| package.package_name)
}

/// AppSecurityTests#testInstallDuplicatePermission_differentProtectionLevel_fail and
/// #testInstallDuplicatePermission_differentPermissionGroup_fail expect
/// INSTALL_PARSE_FAILED_MANIFEST_MALFORMED; #testInstallDuplicatePermission_sameProtectionLevel_success
/// installs. ParsedPermissionUtils.declareDuplicatePermission (android-16.0.0_r1).
#[test]
fn duplicate_permissions_fail_only_with_different_attributes() {
    for name in [
        "CtsDuplicatePermissionDeclareApp_DifferentProtectionLevel.apk",
        "CtsMalformedDuplicatePermission_DifferentPermissionGroup.apk",
    ] {
        match parse(name) {
            Err(parse::Error::Status(-108, message)) => {
                assert_eq!(
                    message,
                    "Found duplicate permission with a different attribute value."
                )
            }
            other => panic!("{name}: {other:?}"),
        }
    }
    assert_eq!(
        parse("CtsDuplicatePermissionDeclareApp_SameProtectionLevel.apk").unwrap(),
        "com.android.cts.duplicatepermission.sameprotectionlevel"
    );
}
