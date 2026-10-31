//! AppIdPermissionPolicy.kt:1023-1071, 1310-1427, 1600-1732, Android 16 r1.
use super::super::boot::tests::{declaration, package};
use super::*;
use crate::package::{roles, settings::Signatures};
use sha2::{Digest, Sha256};

fn fixture(level: i32, old: i32) -> (State, permissions::AccessView) {
    let state = State {
        packages: [
            (
                "owner".into(),
                package(
                    "owner",
                    10001,
                    vec![declaration("owner.TEST", level, false)],
                    &[],
                ),
            ),
            ("p".into(), package("p", 10002, vec![], &["owner.TEST"])),
        ]
        .into(),
        ..Default::default()
    };
    let mut user = AccessUser::default();
    user.set_permission_flags(10002, "owner.TEST", old);
    let base = permissions::AccessView {
        system: Arc::new(AccessSystem {
            permissions: vec![Permission {
                name: "owner.TEST".into(),
                package: "owner".into(),
                protection_level: level,
                dynamic: None,
            }],
            ..Default::default()
        }),
        user0: Arc::new(user),
    };
    (state, base)
}

fn evaluate(
    state: &State,
    base: &permissions::AccessView,
    policy: &signature::Policy,
) -> Result<Arc<permissions::AccessView>, Exception> {
    let ps = state.packages.values().find(|p| p.app_id == 10002).unwrap();
    prepare(
        base,
        state,
        &[Install {
            name: ps.name.clone(),
            app_id: ps.app_id,
            path: ps.path.clone(),
            version: ps.version_code,
            code: ps.pkg.as_deref().unwrap().clone(),
            previous_code: None,
        }],
        policy,
    )
}

fn flags(state: &State, base: &permissions::AccessView) -> i32 {
    evaluate(state, base, &signature::Policy::default())
        .unwrap()
        .user0
        .permission_flags(10002, "owner.TEST")
}

#[test]
fn signature_and_internal_static_and_dynamic_flags_follow_install_evaluation() {
    for protection in [PROTECTION_SIGNATURE, 4] {
        // These flags alone confer no static grant on a non-system, target-36 install.
        // retailDemo has no grant branch in the pinned original.
        for flag in [
            0,
            0x10,
            0x20,
            0x40,
            0x80,
            0x400,
            0x1000,
            0x2000,
            0x4000,
            0x8000,
            0x20000,
            0x40000,
            0x400000,
            0x1000000,
            PROTECTION_FLAG_ROLE,
            0x8000000,
        ] {
            let old = INSTALL_GRANTED
                | INSTALL_REVOKED
                | PROTECTION_GRANTED
                | RUNTIME_GRANTED
                | ROLE
                | USER_SET
                | USER_FIXED;
            let (state, base) = fixture(protection | flag, old);
            let mut expected = 0;
            if protection == PROTECTION_SIGNATURE && flag == PROTECTION_FLAG_DEVELOPMENT {
                expected |= RUNTIME_GRANTED;
            }
            if flag == PROTECTION_FLAG_ROLE {
                expected |= ROLE | RUNTIME_GRANTED;
            }
            if flag == 0x40 {
                expected |= ROLE | USER_SET;
            }
            assert_eq!(flags(&state, &base), expected, "{protection:x}|{flag:x}");
            let (state, base) = fixture(protection | flag, 0);
            assert_eq!(flags(&state, &base), 0, "fresh {protection:x}|{flag:x}");
        }
        let (mut state, base) = fixture(protection | 0x400000, 0);
        state.packages.get_mut("p").unwrap().apex_module_name = Some("module".into());
        assert_eq!(flags(&state, &base), PROTECTION_GRANTED);
    }
    let (mut state, base) = fixture(PROTECTION_SIGNATURE, PROTECTION_GRANTED);
    state.packages.get_mut("p").unwrap().signatures = None;
    assert_eq!(flags(&state, &base), 0);
}

#[test]
fn signature_matching_lineages_platform_trust_and_internal_exclusion() {
    for protection in [PROTECTION_SIGNATURE, 4] {
        let (mut state, base) = fixture(protection, 0);
        let trusted = state.packages["p"].signatures.clone();
        state.packages.get_mut("owner").unwrap().signatures = trusted;
        assert_eq!(
            flags(&state, &base),
            if protection == 2 {
                PROTECTION_GRANTED
            } else {
                0
            }
        );
    }
    let (mut state, base) = fixture(PROTECTION_SIGNATURE, 0);
    // A rotated definer must retain PERMISSION capability for the common ancestor.
    for capability in [0, 4] {
        state.packages.get_mut("owner").unwrap().signatures = Some(Signatures {
            signatures: vec![vec![3]],
            past_signatures: Some(vec![(vec![1], capability), (vec![3], 0)]),
            ..Default::default()
        });
        state.packages.get_mut("p").unwrap().signatures = Some(Signatures {
            signatures: vec![vec![2]],
            past_signatures: Some(vec![(vec![1], 0), (vec![2], 0)]),
            ..Default::default()
        });
        assert_eq!(
            flags(&state, &base),
            if capability == 4 {
                PROTECTION_GRANTED
            } else {
                0
            }
        );
    }
    // The platform signer can authorize a permission defined by another signer.
    let mut platform = package("android", 1000, vec![], &[]);
    platform.signatures = state.packages["p"].signatures.clone();
    state.packages.insert("android".into(), platform);
    state.packages.get_mut("owner").unwrap().signatures = None;
    assert_eq!(flags(&state, &base), PROTECTION_GRANTED);
}

#[test]
fn platform_signature_allowlist_and_combined_dynamic_flags_apply_on_install() {
    let (mut state, mut base) =
        fixture(PROTECTION_SIGNATURE | 0x20 | 0x40 | PROTECTION_FLAG_ROLE, 0);
    let mut platform = state.packages.remove("owner").unwrap();
    platform.name = "android".into();
    platform.signatures = state.packages["p"].signatures.clone();
    state.packages.insert("android".into(), platform);
    Arc::make_mut(&mut base.system).permissions[0].package = "android".into();
    Arc::make_mut(&mut base.user0).set_permission_flags(
        10002,
        "owner.TEST",
        ROLE | RUNTIME_GRANTED | USER_SET | USER_FIXED,
    );
    state.system.flags.push((
        "android.permission.flags.signature_permission_allowlist_enabled".into(),
        true,
    ));
    let retained = ROLE | RUNTIME_GRANTED | USER_SET;
    assert_eq!(flags(&state, &base), retained);
    let policy = signature::Policy {
        debuggable: true,
        ..Default::default()
    };
    assert_eq!(
        evaluate(&state, &base, &policy)
            .unwrap()
            .user0
            .permission_flags(10002, "owner.TEST"),
        PROTECTION_GRANTED | retained
    );
    let dir = crate::app_host::test_temp_dir("install-signature-allowlist");
    std::fs::create_dir_all(dir.join("system/etc/permissions")).unwrap();
    std::fs::write(dir.join("system/etc/permissions/signature.xml"), "<permissions><signature-permissions package='p'><permission name='owner.TEST'/></signature-permissions></permissions>").unwrap();
    let policy = signature::Policy {
        allowlist: crate::package::system_config::SystemConfig::read(&dir, &|_| None)
            .permission_allowlist,
        ..Default::default()
    };
    assert_eq!(
        evaluate(&state, &base, &policy)
            .unwrap()
            .user0
            .permission_flags(10002, "owner.TEST"),
        PROTECTION_GRANTED | retained
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn known_package_flags_use_selections_or_name_the_missing_flag() {
    for (flag, kind, label) in [
        (0x100, 2, "installer"),
        (0x100, 7, "installer"),
        (0x200, 4, "verifier"),
        (0x800, 1, "setup"),
        (0x10000, 6, "systemTextClassifier"),
        (0x80000, 10, "configurator"),
        (0x100000, 11, "incidentReportApprover"),
        (0x200000, 12, "appPredictor"),
        (0x800000, 15, "companion"),
        (0x2000000, 17, "recents"),
    ] {
        for protection in [PROTECTION_SIGNATURE, 4] {
            let (mut state, base) = fixture(protection | flag, 0);
            let error = evaluate(&state, &base, &signature::Policy::default())
                .err()
                .unwrap();
            assert_eq!(error.code, EX_UNSUPPORTED_OPERATION);
            assert!(
                error.message.contains(label) && error.message.contains("#2161"),
                "{}",
                error.message
            );
            for selected in [false, true] {
                let mut known = [(2, vec![]), (7, vec![])]
                    .into_iter()
                    .collect::<BTreeMap<_, _>>();
                known.insert(
                    kind,
                    if selected {
                        vec![Some("p".into())]
                    } else {
                        vec![Some("other".into())]
                    },
                );
                state.system.roles = Some(roles::Owner::known_packages_fixture(known));
                assert_eq!(
                    flags(&state, &base),
                    if selected { PROTECTION_GRANTED } else { 0 }
                );
            }
        }
    }
}

#[test]
fn known_signer_checks_current_and_past_certificates_and_all_multiple_signers() {
    let digest = |cert: &[u8]| Some(format!("{:X}", Sha256::digest(cert)));
    for protection in [PROTECTION_SIGNATURE, 4] {
        for (signatures, past, certs, expected) in [
            (vec![vec![2]], None, vec![digest(&[2])], true),
            (
                vec![vec![2]],
                Some(vec![(vec![1], 0), (vec![2], 0)]),
                vec![digest(&[1])],
                true,
            ),
            (vec![vec![2]], None, vec![digest(&[1])], false),
            (vec![vec![1], vec![2]], None, vec![digest(&[1])], false),
            (
                vec![vec![1], vec![2]],
                None,
                vec![digest(&[1]), digest(&[2])],
                true,
            ),
        ] {
            let (mut state, base) = fixture(protection | 0x8000000, 0);
            state.packages.get_mut("p").unwrap().signatures = Some(Signatures {
                signatures,
                past_signatures: past,
                ..Default::default()
            });
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
                .known_certs = Some(certs);
            assert_eq!(
                flags(&state, &base),
                if expected { PROTECTION_GRANTED } else { 0 }
            );
        }
    }
}

#[test]
fn signed_apk_requests_platform_and_own_signature_permissions() {
    use std::{fs, process::Command};
    let root =
        aim_paths::original_image_with("system/framework/framework-res.apk").expect("pinned image");
    let dir = crate::app_host::test_temp_dir("install-signature-apk");
    let tools = aim_paths::fetched().join("java/build-tools-36.0.0/android-16");
    let jdk = aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let manifest = dir.join("AndroidManifest.xml");
    fs::write(
        &manifest,
        r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="fixture.p">
        <uses-sdk android:minSdkVersion="36" android:targetSdkVersion="36"/>
        <permission android:name="fixture.p.SIGNATURE" android:protectionLevel="signature"/>
        <uses-permission android:name="fixture.p.SIGNATURE"/>
        <uses-permission android:name="android.permission.WRITE_SECURE_SETTINGS"/>
        <application android:hasCode="false"/>
    </manifest>"#,
    )
    .unwrap();
    let unsigned = dir.join("unsigned.apk");
    let signed = dir.join("signed.apk");
    let key = dir.join("fixture.keystore");
    let run = |command: &mut Command| {
        let result = command.output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    run(Command::new(tools.join("aapt2"))
        .args(["link", "--manifest"])
        .arg(&manifest)
        .arg("-I")
        .arg(root.join("system/framework/framework-res.apk"))
        .arg("-o")
        .arg(&unsigned));
    run(Command::new(jdk.join("bin/keytool"))
        .args([
            "-genkeypair",
            "-alias",
            "fixture",
            "-keyalg",
            "RSA",
            "-keysize",
            "2048",
            "-validity",
            "3650",
            "-dname",
            "CN=AIM install permission test",
            "-storepass",
            "fixture-only",
            "-keypass",
            "fixture-only",
            "-keystore",
        ])
        .arg(&key));
    run(Command::new(jdk.join("bin/java"))
        .arg("-jar")
        .arg(tools.join("lib/apksigner.jar"))
        .args(["sign", "--ks"])
        .arg(&key)
        .args([
            "--ks-pass",
            "pass:fixture-only",
            "--key-pass",
            "pass:fixture-only",
            "--out",
        ])
        .arg(&signed)
        .arg(&unsigned));
    let apks = crate::package::write::Apks {
        signing_overrides: None,
        files: Box::new(move |path| {
            Some(if path == "/data/app/p/base.apk" {
                signed.clone()
            } else {
                root.join(path.trim_start_matches('/'))
            })
        }),
        platform: crate::package::parse::Platform::load(
            &aim_paths::original_image(),
            Default::default(),
        )
        .unwrap(),
    };
    let mut state = State::default();
    state.system.roles = Some(roles::Owner::known_packages_fixture(
        [1, 2, 4, 6, 7, 10, 11, 12, 15, 17]
            .into_iter()
            .map(|kind| (kind, vec![]))
            .collect(),
    ));
    for (name, id, path) in [
        ("android", 1000, "/system/framework/framework-res.apk"),
        ("fixture.p", 10002, "/data/app/p/base.apk"),
    ] {
        let code = apks.parsed_path(path, 0).unwrap();
        let mut ps = package(name, id, vec![], &[]);
        ps.signatures = Some(apks.signatures(&code).unwrap());
        ps.path = path.into();
        ps.pkg = Some(Arc::new(code));
        state.packages.insert(name.into(), ps);
    }
    let declared = state.packages["android"]
        .pkg
        .as_ref()
        .unwrap()
        .permissions
        .iter()
        .find(|p| p.component.name == "android.permission.WRITE_SECURE_SETTINGS")
        .unwrap();
    assert_eq!(
        declared.protection_level & PROTECTION_MASK_BASE,
        PROTECTION_SIGNATURE
    );
    let base = permissions::AccessView {
        system: Arc::new(AccessSystem {
            permissions: vec![Permission {
                name: declared.component.name.clone(),
                package: "android".into(),
                protection_level: declared.protection_level,
                dynamic: None,
            }],
            ..Default::default()
        }),
        user0: Arc::default(),
    };
    let after = evaluate(&state, &base, &signature::Policy::default()).unwrap();
    assert_eq!(
        after
            .user0
            .permission_flags(10002, "android.permission.WRITE_SECURE_SETTINGS"),
        0
    );
    assert_eq!(
        after.user0.permission_flags(10002, "fixture.p.SIGNATURE"),
        PROTECTION_GRANTED
    );
    assert!(
        base.system
            .permissions
            .iter()
            .all(|p| p.name != "fixture.p.SIGNATURE")
    );
    fs::remove_dir_all(dir).unwrap();
}
