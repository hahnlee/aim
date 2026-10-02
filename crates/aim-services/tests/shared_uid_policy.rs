//! Pin the inspected shared UID reconciliation and commit control flow
//! in the original image. This is not a native-PMS CTS or OTA boot result.
use aim_android_image::dex::{Dex, units};
use aim_apps::apk::Apk;

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn shared_uid_signature_scan_policy_matches_the_pinned_image() {
    use sha2::{Digest, Sha256};
    let jar =
        Apk::open(&aim_paths::original_image().join("system/framework/services.jar")).unwrap();
    let mut checked = 0;
    for name in ["classes.dex", "classes2.dex", "classes3.dex"] {
        let bytes = jar.file(name).unwrap();
        let dex = Dex::parse(&bytes).unwrap();
        for (owner, method, length, expected) in [
            (
                "Lcom/android/server/pm/ReconcilePackageUtils;",
                "reconcilePackages",
                978,
                "a8789e2ce5d20ce0d1d8515bfdfabb7a0751d7776bac68562a9f5d1df9a194e5",
            ),
            (
                "Lcom/android/server/pm/Settings;",
                "insertPackageSettingLPw",
                45,
                "a403bfb4d368114c1596842532c664b3c16292d6ac1ac86789e608e80beee373",
            ),
        ] {
            let Some(class) = dex.class(owner) else {
                continue;
            };
            let code = dex.methods_named(class, method).unwrap();
            assert_eq!(code.len(), 1);
            let words = units(&bytes, &code[0]).unwrap();
            assert_eq!(words.len(), length);
            let bytes = words
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>();
            assert_eq!(
                format!("{:x}", Sha256::digest(&bytes)),
                expected,
                "inspected {method} control flow changed"
            );
            if method == "reconcilePackages" {
                assert_eq!(
                    dex.method(words[0x284] as u32).unwrap().1,
                    "mergeLineageWith"
                );
                // Only a changed first merge enters the restricted-member loop.
                assert_eq!(&words[0x287..0x289], &[0x4532, 0x49]);
                assert_eq!(0x287 + words[0x288] as usize, 0x2d0);
                assert_eq!(words[0x2b9], 0x2412); // v4 = RESTRICTED (2)
                assert_eq!(
                    dex.method(words[0x2bb] as u32).unwrap().2,
                    "(Landroid/content/pm/SigningDetails;I)Landroid/content/pm/SigningDetails;"
                );
                assert_eq!(
                    dex.field(words[0x2d3] as u32).unwrap().1,
                    "signaturesChanged"
                );
                assert_eq!(dex.field(words[0x2d7] as u32).unwrap().1, "FALSE");
                // Normal failures of non-system code bypass OTA replacement.
                assert_eq!(&words[0x2f8..0x2fa], &[0x0738, 0xd3]);
                assert_eq!(
                    dex.method(words[0x3ce] as u32).unwrap().0,
                    "Lcom/android/server/pm/ReconcileFailure;"
                );
                // Uninitialized group bypasses SYSTEM join; initialized groups
                // must pass, then replace both details and the true marker.
                assert_eq!(
                    dex.field(words[0x301] as u32).unwrap().1,
                    "signaturesChanged"
                );
                assert_eq!(&words[0x302..0x304], &[0x0538, 0x59]);
                assert_eq!(0x302 + words[0x303] as usize, 0x35b);
                assert_eq!(words[0x30c], 0x2712); // v7 = SYSTEM (2)
                assert_eq!(
                    dex.method(words[0x30e] as u32).unwrap().1,
                    "canJoinSharedUserId"
                );
                assert_eq!(words[0x30f], 0x7365); // join-type argument is v7
                assert_eq!(
                    dex.string(words[0x314] as u32 | ((words[0x315] as u32) << 16))
                        .unwrap(),
                    "ro.product.first_api_level"
                );
                assert_eq!(dex.method(words[0x318] as u32).unwrap().1, "getInt");
                assert_eq!(&words[0x31b..0x31f], &[0x0113, 29, 0x1036, 0x1b]);
                assert_eq!(&words[0x332..0x334], &[0x0213, (-104i16) as u16]);
                assert_eq!(
                    dex.method(words[0x358] as u32).unwrap().0,
                    "Ljava/lang/IllegalStateException;"
                );
                assert_eq!(dex.field(words[0x362] as u32).unwrap().1, "mSigningDetails");
                assert_eq!(dex.field(words[0x364] as u32).unwrap().1, "TRUE");
                assert_eq!(
                    dex.field(words[0x366] as u32).unwrap().1,
                    "signaturesChanged"
                );
            } else {
                // Commit initializes only a group with null current signers.
                assert_eq!(dex.method(words[0x1c] as u32).unwrap().1, "getSignatures");
                assert_eq!(&words[0x1f..0x21], &[0x0139, 10]);
                assert_eq!(0x1f + words[0x20] as usize, 0x29);
                assert_eq!(dex.field(words[0x28] as u32).unwrap().1, "mSigningDetails");
                assert_eq!(
                    dex.method(words[0x2a] as u32).unwrap().1,
                    "addPackageSettingLPw"
                );
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 2);
}

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn single_shared_uid_migration_matches_the_pinned_image() {
    use sha2::{Digest, Sha256};
    let jar =
        Apk::open(&aim_paths::original_image().join("system/framework/services.jar")).unwrap();
    let mut checked = 0;
    for name in ["classes.dex", "classes2.dex", "classes3.dex"] {
        let bytes = jar.file(name).unwrap();
        let dex = Dex::parse(&bytes).unwrap();
        for (owner, method, length, expected, calls) in [
            (
                "Lcom/android/server/pm/SharedUserSetting;",
                "isSingleUser",
                51,
                "2d2c9c51a1dc31736591076174b35cf6c4521e88d6a2a414bb408efbfb85dbfb",
                &[
                    (0x2, "size"),
                    (0xd, "size"),
                    (0x16, "size"),
                    (0x24, "getPkg"),
                    (0x2a, "isLeavingSharedUser"),
                ][..],
            ),
            (
                "Lcom/android/server/pm/Settings;",
                "convertSharedUserSettingsLPw",
                57,
                "407d1321ce1eea64fc37b19618218855867621f535ed95100f6df6e8c1382472",
                &[
                    (0xd, "getAppId"),
                    (0x11, "replaceSetting"),
                    (0x15, "setSharedUserAppId"),
                    (0x2c, "setSharedUserAppId"),
                    (0x35, "remove"),
                ][..],
            ),
            (
                "Lcom/android/server/pm/Settings;",
                "checkAndConvertSharedUserSettingsLPw",
                41,
                "3a52ac1dd5a7dbae83a372423371b44997fb2a49b98342f33018e8bd49e75620",
                &[
                    (0, "isSingleUser"),
                    (0x12, "getPkg"),
                    (0x18, "isLeavingSharedUser"),
                    (0x1f, "applyStrategy"),
                    (0x25, "convertSharedUserSettingsLPw"),
                ][..],
            ),
        ] {
            let Some(class) = dex.class(owner) else {
                continue;
            };
            let code = dex.methods_named(class, method).unwrap();
            assert_eq!(code.len(), 1);
            let words = units(&bytes, &code[0]).unwrap();
            assert_eq!(words.len(), length);
            let raw: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            assert_eq!(format!("{:x}", Sha256::digest(&raw)), expected);
            for (at, called) in calls {
                assert_eq!(dex.method(words[at + 1] as u32).unwrap().1, *called);
            }
            match method {
                "isSingleUser" => {
                    assert_eq!(&words[8..10], &[0x2032, 3]); // Active count must be one.
                    assert_eq!(&words[17..19], &[0x2037, 3]); // Disabled count must not exceed one.
                    assert_eq!(&words[40..42], &[0x0338, 9]); // Null disabled parsed package rejects.
                }
                "convertSharedUserSettingsLPw" => assert_eq!(words[0x14], 0xf212), // INVALID_UID = -1.
                _ => assert_eq!(words[0x1e], 0x2012), // BEST_EFFORT = 2.
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 3);
}

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn uid_creation_matches_the_pinned_settings_policy() {
    use sha2::{Digest, Sha256};
    let jar =
        Apk::open(&aim_paths::original_image().join("system/framework/services.jar")).unwrap();
    let mut checked = 0;
    for name in ["classes.dex", "classes2.dex", "classes3.dex"] {
        let bytes = jar.file(name).unwrap();
        let dex = Dex::parse(&bytes).unwrap();
        let Some(class) = dex.class("Lcom/android/server/pm/Settings;") else {
            continue;
        };
        for (method, length, expected) in [
            (
                "getSharedUserLPw",
                95,
                "3dfb87628423c895d4c08df45298df83472a2f3deecd1b9d2d61b1955be80df0",
            ),
            (
                "registerAppIdLPw",
                100,
                "ff8181fc1924c3faca9761f417a137605154bb8193dc836d3e1befcc986f7203",
            ),
        ] {
            let code = dex.methods_named(class, method).unwrap();
            assert_eq!(code.len(), 1);
            let words = units(&bytes, &code[0]).unwrap();
            assert_eq!(words.len(), length);
            let raw: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            assert_eq!(format!("{:x}", Sha256::digest(&raw)), expected);
            if method == "getSharedUserLPw" {
                assert_eq!(dex.method(words[3] as u32).unwrap().1, "get");
                assert_eq!(&words[8..12], &[0x0039, 0x56, 0x0538, 0x54]); // Existing or !create bypasses allocation.
                assert_eq!(
                    dex.method(words[0x14] as u32).unwrap().1,
                    "acquireAndRegisterNewAppId"
                );
                assert_eq!(dex.method(words[0x3e] as u32).unwrap().1, "put");
                assert_eq!(words[0x59], 0xc312); // Insufficient storage (-4).
            } else {
                assert_eq!(
                    dex.method(words[0x14] as u32).unwrap().1,
                    "registerExistingAppId"
                );
                assert_eq!(
                    dex.method(words[0x1b] as u32).unwrap().1,
                    "acquireAndRegisterNewAppId"
                );
                assert_eq!(dex.method(words[0x1f] as u32).unwrap().1, "setAppId");
                assert_eq!(words[0x5f], 0xc412); // Insufficient storage (-4).
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 2);
}

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn uid_cleanup_matches_the_pinned_owner_policy() {
    use sha2::{Digest, Sha256};
    let jar =
        Apk::open(&aim_paths::original_image().join("system/framework/services.jar")).unwrap();
    let mut checked = 0;
    for name in ["classes.dex", "classes2.dex", "classes3.dex"] {
        let bytes = jar.file(name).unwrap();
        let dex = Dex::parse(&bytes).unwrap();
        for (owner, method, length, expected) in [
            (
                "Lcom/android/server/pm/InstallPackageHelper;",
                "cleanUpAppIdCreation",
                51,
                "39a4a930e641c04299d23719d9936c2fe32f624e76a3d38a67c798f66f0348c8",
            ),
            (
                "Lcom/android/server/pm/Settings;",
                "checkAndPruneSharedUserLPw",
                41,
                "945f56ed3afd114b52e71487416295195a8a403c46e8b2c49bc4c23163dd8cbb",
            ),
        ] {
            let Some(class) = dex.class(owner) else {
                continue;
            };
            let code = dex.methods_named(class, method).unwrap();
            assert_eq!(code.len(), 1);
            let words = units(&bytes, &code[0]).unwrap();
            assert_eq!(words.len(), length);
            let raw: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            assert_eq!(format!("{:x}", Sha256::digest(&raw)), expected);
            if method == "cleanUpAppIdCreation" {
                assert_eq!(
                    dex.method(words[1] as u32).unwrap().1,
                    "getScannedPackageSetting"
                );
                assert_eq!(&words[4..6], &[0x0038, 0x2e]); // Null setting returns without removal.
                assert_eq!(&words[0xe..0x10], &[0x003d, 0x24]); // Nonpositive UID returns.
                assert_eq!(dex.method(words[0x21] as u32).unwrap().1, "getAppId");
                assert_eq!(dex.method(words[0x25] as u32).unwrap().1, "removeAppIdLPw");
            } else {
                assert_eq!(dex.method(words[3] as u32).unwrap().1, "getPackageStates");
                assert_eq!(
                    dex.method(words[0xd] as u32).unwrap().1,
                    "getDisabledPackageStates"
                );
                assert_eq!(&words[0xa..0xc], &[0x0338, 0x1d]); // Nonempty active group returns false.
                assert_eq!(&words[0x14..0x16], &[0x0338, 0x13]); // Nonempty disabled group returns false.
                assert_eq!(dex.method(words[0x1b] as u32).unwrap().1, "remove");
                assert_eq!(dex.method(words[0x23] as u32).unwrap().1, "removeAppIdLPw");
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 2);
}

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn static_library_signature_setting_selection_matches_the_pinned_image() {
    use sha2::{Digest, Sha256};
    let jar =
        Apk::open(&aim_paths::original_image().join("system/framework/services.jar")).unwrap();
    let mut checked = 0;
    for name in ["classes.dex", "classes2.dex", "classes3.dex"] {
        let bytes = jar.file(name).unwrap();
        let dex = Dex::parse(&bytes).unwrap();
        let Some(class) = dex.class("Lcom/android/server/pm/SharedLibrariesImpl;") else {
            continue;
        };
        for (method, length, hash) in [
            (
                "getLatestStaticSharedLibraVersionLPr",
                58,
                "b5296449707aa3eeb37ae91bf3fb6c1da86d5e8c143cc24c11f181af8ab7f807",
            ),
            (
                "getStaticSharedLibLatestVersionSetting",
                51,
                "1a9ba3cadf25567c2fd6b0f9e920d4233dc58aa08205f27e02f884c6cfd46a20",
            ),
        ] {
            let methods = dex.methods_named(class, method).unwrap();
            assert_eq!(methods.len(), 1);
            let words = units(&bytes, &methods[0]).unwrap();
            assert_eq!(words.len(), length);
            let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            assert_eq!(
                format!("{:x}", Sha256::digest(bytes)),
                hash,
                "inspected {method} policy changed"
            );
            if method == "getLatestStaticSharedLibraVersionLPr" {
                assert_eq!(&words[0x14..0x16], &[0x0216, 0xffff]); // previous version = -1
                assert_eq!(&words[0x23..0x25], &[0x073b, 6]); // skip version >= incoming
                assert_eq!(dex.method(words[0x26] as u32).unwrap().1, "max");
                assert_eq!(&words[0x2c..0x2e], &[0x0416, 0]);
                assert_eq!(&words[0x30..0x32], &[0x0a3a, 9]); // negative selection returns null
                assert_eq!(dex.method(words[0x33] as u32).unwrap().1, "get");
            } else {
                assert_eq!(
                    dex.method(words[0x15] as u32).unwrap().1,
                    "getLatestStaticSharedLibraVersionLPr"
                );
                assert_eq!(dex.method(words[0x1f] as u32).unwrap().1, "getPackageName");
                assert_eq!(dex.method(words[0x23] as u32).unwrap().1, "getPackageLPr");
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 2);
}

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn static_library_declaration_constraints_match_the_pinned_image() {
    use sha2::{Digest, Sha256};
    let jar =
        Apk::open(&aim_paths::original_image().join("system/framework/services.jar")).unwrap();
    let mut checked = 0;
    for name in ["classes.dex", "classes2.dex", "classes3.dex"] {
        let bytes = jar.file(name).unwrap();
        let dex = Dex::parse(&bytes).unwrap();
        let Some(class) = dex.class("Lcom/android/server/pm/ScanPackageUtils;") else {
            continue;
        };
        let methods = dex
            .methods_named(class, "assertStaticSharedLibraryIsValid")
            .unwrap();
        assert_eq!(methods.len(), 1);
        let words = units(&bytes, &methods[0]).unwrap();
        assert_eq!(words.len(), 251);
        let bytes: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        assert_eq!(
            format!("{:x}", Sha256::digest(bytes)),
            "11d48b036180d1095a59f44bab56eb82ef658204c11560aeb83c317732b64b4d"
        );
        for (at, method) in [
            (0x00, "getTargetSdkVersion"),
            (0x0c, "getOriginalPackages"),
            (0x16, "getLibraryNames"),
            (0x20, "getSharedUserId"),
            (0x26, "getActivities"),
            (0x30, "getServices"),
            (0x3a, "getProviders"),
            (0x44, "getReceivers"),
            (0x4e, "getPermissionGroups"),
            (0x58, "getAttributions"),
            (0x62, "getPermissions"),
            (0x6c, "getProtectedBroadcasts"),
            (0x76, "getOverlayTarget"),
        ] {
            assert_eq!(dex.method(words[at + 1] as u32).unwrap().1, method);
        }
        checked += 1;
    }
    assert_eq!(checked, 1);
}
