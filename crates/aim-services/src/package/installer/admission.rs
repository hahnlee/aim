//! InstallPackageHelper.preparePackage / DexMetadataHelper, android-16.0.0_r1.
//! Ported from AOSP (Apache-2.0). This phase only reads immutable staged code.
use super::pipeline::Failure;
use crate::package::{
    pkg::{AndroidPackage, booleans},
    sign::SigningDetails,
    write::Apks,
};
use android_image_extract::{source::FileSource, zip::Archive};

fn failure(status: i32, message: impl Into<String>) -> Failure {
    Failure {
        legacy_status: status,
        committed: false,
        message: message.into(),
    }
}

pub(super) struct Policy {
    pub min_target: i32,
    pub require_manifest: bool,
}
impl Policy {
    pub fn capture(apks: &Apks, lite: &super::native::LitePolicy) -> Self {
        Self {
            min_target: if apks
                .platform
                .flags
                .get("android.content.pm.min_target_sdk_24")
                == Some(&true)
            {
                24
            } else {
                23
            },
            require_manifest: lite
                .environment
                .properties
                .get("pm.dexopt.dm.require_manifest")
                .is_some_and(|v| matches!(v.as_str(), "1" | "y" | "yes" | "on" | "true")),
        }
    }
}

pub(super) fn dex_metadata(
    files: &crate::package::write::Files,
    pkg: &AndroidPackage,
    policy: &Policy,
) -> Result<(), Failure> {
    let paths: Vec<_> = pkg
        .base_apk_path
        .iter()
        .chain(pkg.split_code_paths.iter().flatten().flatten())
        .collect();
    for apk in paths.into_iter().rev() {
        let dm = format!("{}.dm", apk.strip_suffix(".apk").unwrap_or(apk));
        let error = |message: String| {
            failure(
                -117,
                format!("Failed parse during installPackageLI: {message}"),
            )
        };
        let host = files(&dm).ok_or_else(|| error(format!("Error opening {dm}")))?;
        if !host.exists() {
            continue;
        }
        let source = FileSource::open(&host).map_err(|_| error(format!("Error opening {dm}")))?;
        let zip = Archive::open(&source).map_err(|_| error(format!("Error opening {dm}")))?;
        if !policy.require_manifest {
            continue;
        }
        let entry = zip
            .find(b"manifest.json")
            .ok_or_else(|| error(format!("Missing manifest.json in {dm}")))?;
        let bytes = zip
            .read(entry, u64::MAX)
            .map_err(|_| error(format!("Error opening {dm}")))?;
        let manifest = serde_json::Deserializer::from_slice(&bytes)
            .into_iter::<serde_json::Value>()
            .next()
            .and_then(Result::ok)
            .filter(|v| v.is_object())
            .ok_or_else(|| error(format!("Error opening {dm}")))?;
        let name = manifest.get("packageName").and_then(|v| match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Number(n) => Some(n.to_string()),
            _ => None,
        });
        let version = manifest
            .get("versionCode")
            .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
            .unwrap_or(-1);
        let Some(name) = name.filter(|_| version != -1) else {
            return Err(error(format!(
                "manifest.json in {dm} is missing 'packageName' and/or 'versionCode'"
            )));
        };
        if name != pkg.package_name {
            return Err(error(format!(
                "manifest.json in {dm} has invalid packageName: {name}, expected: {}",
                pkg.package_name
            )));
        }
        let expected =
            (i64::from(pkg.version_code_major) << 32) | i64::from(pkg.version_code as u32);
        if version != expected {
            return Err(error(format!(
                "manifest.json in {dm} has invalid versionCode: {version}, expected: {expected}"
            )));
        }
    }
    Ok(())
}

pub(super) fn prepare(
    pkg: &AndroidPackage,
    flags: i32,
    external: bool,
    policy: &Policy,
    collect: impl FnOnce() -> Result<SigningDetails, Failure>,
) -> Result<SigningDetails, Failure> {
    let instant = flags & 0x800 != 0;
    if instant && external {
        return Err(failure(-116, "Failed to prepare for install."));
    }
    if flags & 0x1000000 == 0
        && !pkg.is(booleans::TEST_ONLY)
        && pkg.target_sdk_version < policy.min_target
    {
        return Err(failure(
            -29,
            format!(
                "App package must target at least SDK version {}, but found {}",
                policy.min_target, pkg.target_sdk_version
            ),
        ));
    }
    if instant && pkg.target_sdk_version < 26 {
        return Err(failure(-116, "Instant app package must target at least O"));
    }
    if instant && pkg.shared_user_id.is_some() {
        return Err(failure(
            -116,
            "Instant app package may not declare a sharedUserId",
        ));
    }
    if pkg.static_shared_library_name.is_some() && external {
        return Err(failure(
            -19,
            "Static shared libs can only be installed on internal storage.",
        ));
    }
    if pkg.is(booleans::TEST_ONLY) && flags & 4 == 0 {
        return Err(failure(
            -15,
            "Failed to install test-only apk. Did you forget to add -t?",
        ));
    }
    let signing = collect()?;
    if instant && signing.scheme_version < 2 {
        return Err(failure(
            -116,
            "Instant app package must be signed with APK Signature Scheme v2 or greater",
        ));
    }
    Ok(signing)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dex_metadata_rejection_precedes_signing_and_leaves_stage_unchanged() {
        use std::fs;
        let root = std::env::temp_dir().join(format!("aim-admission-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let path = root.join("base.dm");
        let files: crate::package::write::Files = Box::new(move |_| Some(path.clone()));
        let pkg = AndroidPackage {
            base_apk_path: Some("/stage/base.apk".into()),
            ..Default::default()
        };
        fs::write(root.join("base.apk"), b"immutable code").unwrap();
        let mut empty_zip = [0u8; 22];
        empty_zip[..4].copy_from_slice(b"PK\x05\x06");
        fs::write(root.join("base.dm"), empty_zip).unwrap();
        assert!(dex_metadata(&files, &pkg, &policy()).is_ok());
        let required = Policy {
            require_manifest: true,
            ..policy()
        };
        let error = dex_metadata(&files, &pkg, &required)
            .and_then(|_| {
                prepare(&pkg, 0, false, &required, || {
                    panic!("rejected metadata cannot collect signatures")
                })
            })
            .unwrap_err();
        assert_eq!(error.legacy_status, -117);
        assert_eq!(
            error.message,
            "Failed parse during installPackageLI: Missing manifest.json in /stage/base.dm"
        );
        assert!(!error.committed);
        assert_eq!(fs::read(root.join("base.apk")).unwrap(), b"immutable code");
        assert_eq!(fs::read(root.join("base.dm")).unwrap(), empty_zip);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        fs::write(root.join("base.dm"), b"invalid ZIP").unwrap();
        assert_eq!(
            dex_metadata(&files, &pkg, &policy())
                .unwrap_err()
                .legacy_status,
            -117
        );
        fs::remove_dir_all(root).unwrap();
    }
    fn policy() -> Policy {
        Policy {
            min_target: 24,
            require_manifest: false,
        }
    }
    fn signing(scheme_version: i32) -> SigningDetails {
        SigningDetails {
            scheme_version,
            ..SigningDetails::unknown()
        }
    }
    #[test]
    fn crossed_declarations_keep_pinned_precedence_before_signing() {
        let mut pkg = AndroidPackage {
            target_sdk_version: 22,
            shared_user_id: Some("shared".into()),
            static_shared_library_name: Some("library".into()),
            ..Default::default()
        };
        let reject = |pkg: &AndroidPackage, flags, external, status, message| {
            let error = prepare(pkg, flags, external, &policy(), || {
                panic!("must reject before collecting certificates")
            })
            .unwrap_err();
            assert_eq!(error.legacy_status, status);
            assert_eq!(error.message, message);
            assert!(!error.committed);
        };
        reject(&pkg, 0x800, true, -116, "Failed to prepare for install.");
        reject(
            &pkg,
            0x800,
            false,
            -29,
            "App package must target at least SDK version 24, but found 22",
        );
        reject(
            &pkg,
            0x1000800,
            false,
            -116,
            "Instant app package must target at least O",
        );
        pkg.booleans |= booleans::TEST_ONLY | booleans::LEAVING_SHARED_UID;
        reject(
            &pkg,
            0x800,
            false,
            -116,
            "Instant app package must target at least O",
        );
        pkg.target_sdk_version = 26;
        reject(
            &pkg,
            0x800,
            false,
            -116,
            "Instant app package may not declare a sharedUserId",
        );
        reject(
            &pkg,
            0,
            true,
            -19,
            "Static shared libs can only be installed on internal storage.",
        );
        reject(
            &pkg,
            0,
            false,
            -15,
            "Failed to install test-only apk. Did you forget to add -t?",
        );
        assert!(prepare(&pkg, 4, false, &policy(), || Ok(signing(2))).is_ok());
    }
    #[test]
    fn sdk_policy_boundary_bypass_and_test_only_exemption() {
        for (target, min_target, flags, test_only, expected) in [
            (22, 23, 0, false, -29),
            (22, 23, 0x1000000, false, 0),
            (22, 24, 0, false, -29),
            (22, 24, 0x1000000, false, 0),
            (22, 24, 4, true, 0),
            (23, 23, 0, false, 0),
            (23, 24, 0, false, -29),
            (23, 24, 0x1000000, false, 0),
            (23, 24, 0, true, -15),
            (23, 24, 4, true, 0),
        ] {
            let pkg = AndroidPackage {
                target_sdk_version: target,
                booleans: if test_only { booleans::TEST_ONLY } else { 0 },
                ..Default::default()
            };
            let policy = Policy {
                min_target,
                ..policy()
            };
            let result = prepare(&pkg, flags, false, &policy, || Ok(signing(1)));
            assert_eq!(result.err().map_or(0, |e| e.legacy_status), expected);
        }
    }
    #[test]
    fn instant_v2_is_checked_after_collection() {
        let pkg = AndroidPackage {
            target_sdk_version: 26,
            ..Default::default()
        };
        for scheme in [1, 2, 3] {
            let result = prepare(&pkg, 0x800, false, &policy(), || Ok(signing(scheme)));
            if scheme == 1 {
                let error = result.unwrap_err();
                assert_eq!(error.legacy_status, -116);
                assert_eq!(
                    error.message,
                    "Instant app package must be signed with APK Signature Scheme v2 or greater"
                );
            } else {
                assert!(result.is_ok());
            }
        }
        let error = prepare(&pkg, 0x800, false, &policy(), || {
            Err(failure(-110, "collection failed"))
        })
        .unwrap_err();
        assert_eq!(error.message, "collection failed");
    }
}
