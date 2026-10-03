//! Exercise scan inputs with original signed code. Explicit runs require
//! the pinned image; no parser cache, feed or writable guest data is used.
mod common {
    pub mod seinfo;
}

use aim_services::package::{
    State,
    parse::Platform,
    scan::{DataImage, Image, Inputs, Kind, Partition, ScanPolicy},
    settings,
    write::Apks,
};

struct Fixture(std::path::PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn image_scan_keeps_locations_duplicates_and_rejections_without_settings() {
    let original = aim_paths::original_image();
    let dir = std::env::temp_dir().join(format!("aim-image-scan-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let fixture = Fixture(dir);
    let framework = fixture.0.join("system/framework");
    let overlay = fixture.0.join("product/overlay");
    let app = fixture.0.join("product/priv-app/GSF");
    for path in [&framework, &overlay, &app] {
        std::fs::create_dir_all(path).unwrap();
    }
    let gsf =
        original.join("system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk");
    std::os::unix::fs::symlink(
        original.join("system/framework/framework-res.apk"),
        framework.join("framework-res.apk"),
    )
    .unwrap();
    std::os::unix::fs::symlink(&gsf, overlay.join("gsf.apk")).unwrap();
    std::os::unix::fs::symlink(&gsf, app.join("original.apk")).unwrap();
    std::fs::create_dir(framework.join("arm64")).unwrap();
    std::fs::create_dir(framework.join("vmdl1.tmp")).unwrap();
    let root = fixture.0.clone();
    let apks = Apks {
        files: Box::new(move |p| Some(root.join(p.trim_start_matches('/')))),
        platform: Platform::load(&original, Default::default()).unwrap(),
    };
    let image = Image::load(&apks, &[]).unwrap();
    assert_eq!(image.packages.len(), 3);
    let mut uids =
        aim_services::package::scan::UidScan::new(&Default::default(), &Default::default())
            .unwrap();
    let prepared: Vec<_> = image
        .packages
        .iter()
        .map(|code| uids.apply(code).unwrap())
        .collect();
    assert_eq!(prepared[0].1, prepared[2].1);
    assert_eq!(prepared[1].1.app_id, 1000);
    assert_eq!(
        prepared[1].1.shared_user.as_deref(),
        Some("android.uid.system")
    );
    assert_eq!(uids.packages.len(), 2);

    assert_eq!(image.packages[0].location.kind, Kind::Overlay);
    assert_eq!(image.packages[1].location.kind, Kind::Framework);
    assert_eq!(image.packages[1].parsed.package_name, "android");
    assert_eq!(image.packages[2].location.partition, Partition::Product);
    assert!(image.packages[2].location.privileged());
    assert_eq!(
        image.packages[0].parsed.package_name,
        image.packages[2].parsed.package_name
    );
    assert_eq!(image.packages[0].signing, image.packages[2].signing);
    assert_eq!(image.rejected.len(), 1);
    assert_eq!(image.rejected[0].location.path, "/system/framework/arm64");
    assert!(image.rejected[0].reason.contains("No packages found"));
    std::fs::remove_file(framework.join("framework-res.apk")).unwrap();
    let error = Image::load(&apks, &[]).unwrap_err();
    assert_eq!(error.phase, "framework");
    assert_eq!(error.package, "android");
    std::fs::remove_dir_all(&framework).unwrap();
    assert_eq!(Image::load(&apks, &[]).unwrap_err().phase, "framework");
}

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn persisted_active_and_disabled_apks_are_parsed_and_verified() {
    const APK: &str = "system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk";
    const SYSTEM: &str = "/system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk";
    const UPDATE: &str = "/data/app/updated-gsf/nonstandard.apk";
    let root = aim_paths::original_image();
    let source = root.join(APK);
    let disappearing_source = source.clone();
    assert!(source.is_file(), "missing original signed APK");
    let apks = Apks {
        files: Box::new(move |p| [SYSTEM, UPDATE].contains(&p).then(|| source.clone())),
        platform: Platform::load(&root, Default::default()).unwrap(),
    };
    let package = |path: &str| settings::Package {
        name: "com.google.android.gsf".into(),
        code_path: path.into(),
        flags: 1,
        ..Default::default()
    };
    let mut state = State {
        settings: settings::Settings {
            packages: vec![package(UPDATE)],
            disabled_system_packages: vec![package(SYSTEM)],
            ..Default::default()
        },
        list: vec![],
        access: None,
        users: vec![],
    };
    let before = state.clone();
    let inputs = Inputs::load(&state, &apks).unwrap();
    assert_eq!(state, before);
    assert_eq!(inputs.active.len(), 1);
    assert_eq!(inputs.disabled.len(), 1);
    assert!(inputs.apex.is_empty());
    let active = &inputs.active["com.google.android.gsf"];
    let disabled = &inputs.disabled["com.google.android.gsf"];
    assert_eq!(active.parsed.package_name, "com.google.android.gsf");
    assert_eq!(active.parsed.base_apk_path.as_deref(), Some(UPDATE));
    assert_eq!(disabled.parsed.base_apk_path.as_deref(), Some(SYSTEM));
    assert_eq!(active.signing, disabled.signing);
    assert_eq!(
        active.origin,
        aim_services::package::owner::shared_users::ScanOrigin::Data
    );
    assert_eq!(
        disabled.origin,
        aim_services::package::owner::shared_users::ScanOrigin::SystemDirectory
    );
    assert_eq!(active.signing.scheme_version, 3);
    assert_eq!(active.signing.signatures.len(), 1);
    assert_eq!(
        active
            .signing
            .past_signing_certificates
            .as_ref()
            .unwrap()
            .len(),
        2
    );
    assert!(!active.signing.public_keys.is_empty());

    // Cryptographic validity does not authorize reuse of saved code/UID
    // ownership with unrelated package or disabled-system certificates.
    let unrelated = settings::Signatures {
        signatures: vec![vec![1]],
        ..Default::default()
    };
    state.settings.packages[0].signatures = Some(unrelated.clone());
    let before = state.clone();
    let error = Inputs::load(&state, &apks).unwrap_err();
    assert_eq!(error.phase, "authorization");
    assert!(
        error
            .message
            .contains("existing package signatures mismatch")
    );
    assert_eq!(state, before);
    state.settings.packages[0].signatures = Some(settings::Signatures {
        signatures: active.signing.signatures.clone(),
        past_signatures: active.signing.past_signing_certificates.clone(),
        scheme_version: active.signing.scheme_version,
        ..Default::default()
    });
    state.settings.disabled_system_packages[0].signatures = Some(unrelated);
    let before = state.clone();
    let error = Inputs::load(&state, &apks).unwrap_err();
    assert_eq!(error.phase, "authorization");
    assert!(
        error
            .message
            .contains("updated system package signatures mismatch")
    );
    assert_eq!(state, before);
    state.settings.disabled_system_packages[0].signatures = None;
    assert!(Inputs::load(&state, &apks).is_ok());

    // A valid signed APK cannot inherit a different persisted identity.
    state.settings.packages[0].name = "unrelated.saved.package".into();
    let before = state.clone();
    let error = Inputs::load(&state, &apks).unwrap_err();
    assert_eq!(error.phase, "identity");
    assert!(error.message.contains("com.google.android.gsf"));
    assert_eq!(state, before);
    state.settings.packages[0].name = "com.google.android.gsf".into();

    state.settings.packages[0].code_path = "/data/app/missing/base.apk".into();
    let error = Inputs::load(&state, &apks).unwrap_err();
    assert_eq!(error.package, "com.google.android.gsf");
    assert_eq!(error.phase, "parse");
    assert!(error.message.contains("not readable"));

    // Code disappearing after manifest parsing must fail verification;
    // a parsed manifest alone must never become a successful scan input.
    let reads = std::sync::atomic::AtomicUsize::new(0);
    let disappearing = Apks {
        files: Box::new(move |_| {
            (reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0)
                .then(|| disappearing_source.clone())
        }),
        platform: apks.platform,
    };
    state.settings.packages[0].code_path = UPDATE.into();
    let before = state.clone();
    let error = Inputs::load(&state, &disappearing).unwrap_err();
    assert_eq!(error.phase, "signatures");
    assert_eq!(state, before);
}

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn shared_uid_scan_signatures_follow_commit_and_ota_order() {
    use aim_services::package::owner::shared_users::{ScanOrigin, SharedUser, SignatureError};
    let root = aim_paths::original_image();
    let image = root.clone();
    let apks = Apks {
        files: Box::new(move |p| Some(image.join(p.trim_start_matches('/')))),
        platform: Platform::load(&root, Default::default()).unwrap(),
    };
    let verified = |path| {
        let parsed = apks
            .parsed_path(path, aim_services::package::parse::PARSE_IS_SYSTEM_DIR)
            .unwrap();
        apks.signing_details(&parsed).unwrap()
    };
    let google =
        verified("/system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk");
    let platform = verified("/system/framework/framework-res.apk");
    assert_ne!(google.signatures, platform.signatures);
    let mut group = SharedUser::new(1000, 1, 8);
    assert!(!group.merge_authorized_lineage(&google, &[]).unwrap());
    assert_eq!(group.signatures_changed, Some(false));
    assert!(group.signatures.is_none());
    assert!(group.commit_initial_signatures(&google).unwrap());
    let initialized = group.clone();
    assert!(!group.commit_initial_signatures(&platform).unwrap());
    assert_eq!(group, initialized);
    assert_eq!(
        group.signatures.as_ref().unwrap().signatures,
        google.signatures
    );

    // A /data update of a system package cannot take the OTA branch.
    let mut first = initialized.clone();
    first.signatures_changed = None;
    let before = first.clone();
    assert_eq!(
        first.replace_after_signature_failure(&platform, ScanOrigin::Data, 36),
        Err(SignatureError::NonSystemMismatch)
    );
    assert_eq!(first, before);
    first
        .replace_after_signature_failure(&platform, ScanOrigin::SystemDirectory, 36)
        .unwrap();
    assert_eq!(first.signatures_changed, Some(true));
    assert_eq!(first.app_id, initialized.app_id);
    assert_eq!(
        (first.flags, first.private_flags),
        (initialized.flags, initialized.private_flags)
    );
    assert_eq!(
        first.signatures.as_ref().unwrap().signatures,
        platform.signatures
    );
    let before = first.clone();
    assert_eq!(
        first.replace_after_signature_failure(&google, ScanOrigin::SystemDirectory, 36),
        Err(SignatureError::FatalSystemMismatch)
    );
    assert_eq!(first, before);
    assert_eq!(
        first.replace_after_signature_failure(&google, ScanOrigin::SystemDirectory, 29),
        Err(SignatureError::Rejected { code: -104 })
    );
    assert_eq!(first, before);
    first
        .replace_after_signature_failure(&platform, ScanOrigin::SystemDirectory, 36)
        .unwrap();
    assert_eq!(first, before);

    // A previous normal check also ends the first-package exemption.
    let before = group.clone();
    assert_eq!(
        group.replace_after_signature_failure(&platform, ScanOrigin::SystemDirectory, 36),
        Err(SignatureError::FatalSystemMismatch)
    );
    assert_eq!(group, before);
    let snapshot = group.clone();
    group.signatures_changed = None;
    assert_eq!(snapshot.signatures_changed, Some(false));
    assert_eq!(group.signatures_changed, None);
    assert_eq!(group.signatures, snapshot.signatures);
}

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn ordered_signing_scan_commits_groups_and_preserves_prior_records_on_failure() {
    use aim_services::package::scan::{SigningError, SigningScan};
    let root = aim_paths::original_image();
    let image = root.clone();
    let apks = Apks {
        files: Box::new(move |p| {
            Some(image.join(if p == "/data/app/gsf/base.apk" {
                "system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk"
            } else {
                p.trim_start_matches('/')
            }))
        }),
        platform: Platform::load(&root, Default::default()).unwrap(),
    };
    let package = |name: &str, path: &str| settings::Package {
        name: name.into(),
        code_path: path.into(),
        app_id: 10001,
        shared_user: true,
        ..Default::default()
    };
    let mut state = State {
        settings: settings::Settings {
            packages: vec![
                package(
                    "com.google.android.gsf",
                    "/system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk",
                ),
                package("android", "/system/framework/framework-res.apk"),
            ],
            shared_users: vec![settings::SharedUser {
                name: "group".into(),
                app_id: 10001,
                ..Default::default()
            }],
            ..Default::default()
        },
        list: vec![],
        access: None,
        users: vec![],
    };
    let inputs = Inputs::load_verified_code(&state, &apks).unwrap();
    let google = &inputs.active["com.google.android.gsf"];
    let platform = &inputs.active["android"];
    let group_name = google.parsed.shared_user_id.as_deref().unwrap();
    assert_ne!(platform.parsed.shared_user_id.as_deref(), Some(group_name));
    state.settings.shared_users[0].name = group_name.into();
    // This synthetic signer candidate exercises reconciliation without inventing
    // a shared UID declaration in either original APK's parsed manifest.
    let incompatible = aim_services::package::scan::Record {
        settings: google.settings.clone(),
        parsed: google.parsed.clone(),
        signing: platform.signing.clone(),
        identity: google.identity.clone(),
        origin: google.origin,
    };
    let before = state.clone();
    let mut scan = SigningScan::new(&Default::default(), &state.settings, 36).unwrap();
    for declaration in [None, Some("other.shared.uid".into())] {
        let mut changed = aim_services::package::scan::Record {
            settings: google.settings.clone(),
            parsed: google.parsed.clone(),
            signing: google.signing.clone(),
            identity: google.identity.clone(),
            origin: google.origin,
        };
        changed.parsed.shared_user_id = declaration;
        let snapshot = scan.clone();
        let error = scan.apply(&changed).unwrap_err();
        assert!(matches!(error, SigningError::Rejected(ref e) if e.phase == "identity"));
        assert_eq!(scan, snapshot);
    }
    let mut leaving = aim_services::package::scan::Record {
        settings: google.settings.clone(),
        parsed: google.parsed.clone(),
        signing: google.signing.clone(),
        identity: google.identity.clone(),
        origin: google.origin,
    };
    leaving.parsed.booleans |= aim_services::package::pkg::booleans::LEAVING_SHARED_UID;
    let mut still_shared = scan.clone();
    still_shared.apply(&leaving).unwrap();
    assert!(still_shared.settings.packages[0].shared_user);
    let mut left_settings = state.settings.clone();
    left_settings
        .packages
        .retain(|p| p.name == google.settings.name);
    left_settings.packages[0].shared_user = false;
    left_settings.shared_users.clear();
    leaving.settings.shared_user = false;
    let mut left = SigningScan::new(&Default::default(), &left_settings, 36).unwrap();
    let snapshot = left.clone();
    leaving.parsed.booleans &= !aim_services::package::pkg::booleans::LEAVING_SHARED_UID;
    let error = left.apply(&leaving).unwrap_err();
    assert!(matches!(error, SigningError::Rejected(ref e) if e.phase == "identity"));
    assert_eq!(left, snapshot);
    leaving.parsed.booleans |= aim_services::package::pkg::booleans::LEAVING_SHARED_UID;
    left.apply(&leaving).unwrap();
    assert!(!left.settings.packages[0].shared_user);
    assert!(
        scan.apply(google)
            .unwrap()
            .system_signature_mismatch
            .is_none()
    );
    assert_eq!(
        scan.identities.shared_users[group_name].signatures_changed,
        Some(false)
    );
    assert_eq!(
        scan.settings.shared_users[0]
            .signatures
            .as_ref()
            .unwrap()
            .signatures,
        google.signing.signatures
    );
    assert_eq!(
        scan.settings.packages[0]
            .signatures
            .as_ref()
            .unwrap()
            .signatures,
        google.signing.signatures
    );
    let committed = scan.clone();
    let error = scan.apply(platform).unwrap_err();
    assert!(matches!(error, SigningError::Rejected(ref e) if e.phase == "identity"));
    assert_eq!(scan, committed);
    assert!(matches!(
        scan.apply(&incompatible),
        Err(SigningError::Fatal(_))
    ));
    assert_eq!(scan, committed);
    assert_eq!(state, before);

    // First system mismatch replaces a previously saved group signer.
    let old = settings::Signatures {
        signatures: platform.signing.signatures.clone(),
        scheme_version: platform.signing.scheme_version,
        ..Default::default()
    };
    state.settings.shared_users[0].signatures = Some(old.clone());
    state.settings.packages[0].signatures = Some(old);
    let mut scan = SigningScan::new(&Default::default(), &state.settings, 36).unwrap();
    assert!(
        scan.apply(google)
            .unwrap()
            .system_signature_mismatch
            .is_some()
    );
    assert_eq!(
        scan.identities.shared_users[group_name].signatures_changed,
        Some(true)
    );
    assert_eq!(scan.settings.packages[0].app_id, 10001);
    let committed = scan.clone();
    assert!(matches!(
        scan.apply(&incompatible),
        Err(SigningError::Fatal(_))
    ));
    assert_eq!(scan, committed);
    let mut old_api = SigningScan::new(&Default::default(), &state.settings, 29).unwrap();
    old_api.apply(google).unwrap();
    assert!(matches!(
        old_api.apply(&incompatible),
        Err(SigningError::Rejected(_))
    ));

    let mut data_state = state.clone();
    data_state.settings.packages[0].code_path = "/data/app/gsf/base.apk".into();
    data_state.settings.packages[0].flags = settings::FLAG_SYSTEM;
    let mut data_inputs = Inputs::load_verified_code(&data_state, &apks).unwrap();
    let mut data_scan = SigningScan::new(&Default::default(), &data_state.settings, 36).unwrap();
    let before = data_scan.clone();
    assert!(matches!(
        data_scan.apply(&data_inputs.active["com.google.android.gsf"]),
        Err(SigningError::Rejected(_))
    ));
    assert_eq!(data_scan, before);
    let record = data_inputs
        .active
        .get_mut("com.google.android.gsf")
        .unwrap();
    record.origin = aim_services::package::owner::shared_users::ScanOrigin::SystemDirectory;
    assert!(matches!(
        data_scan.apply(record),
        Err(SigningError::Rejected(_))
    ));
    assert_eq!(data_scan, before);
    record.origin = aim_services::package::owner::shared_users::ScanOrigin::Data;
    record.settings.app_id += 1;
    assert!(matches!(
        data_scan.apply(record),
        Err(SigningError::Rejected(_))
    ));
    assert_eq!(data_scan, before);
}

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn single_shared_uid_migration_preserves_ids_and_requires_both_versions_to_leave() {
    use aim_services::package::{
        owner::app_ids::Owner,
        pkg::booleans,
        scan::{SharedUidMigration, SigningScan},
    };
    let root = aim_paths::original_image();
    let image = root.clone();
    let apks = Apks {
        files: Box::new(move |p| Some(image.join(p.trim_start_matches('/')))),
        platform: Platform::load(&root, Default::default()).unwrap(),
    };
    let mut state = State {
        settings: settings::Settings {
            packages: vec![settings::Package {
                name: "com.google.android.gsf".into(),
                code_path:
                    "/system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk"
                        .into(),
                app_id: 10001,
                shared_user: true,
                ..Default::default()
            }],
            shared_users: vec![settings::SharedUser {
                name: "pending".into(),
                app_id: 10001,
                ..Default::default()
            }],
            ..Default::default()
        },
        list: vec![],
        access: None,
        users: vec![],
    };
    let mut inputs = Inputs::load_verified_code(&state, &apks).unwrap();
    let record = inputs.active.get_mut("com.google.android.gsf").unwrap();
    let group = record.parsed.shared_user_id.clone().unwrap();
    state.settings.shared_users[0].name = group.clone();
    record.parsed.booleans &= !booleans::LEAVING_SHARED_UID;
    let mut scan = SigningScan::new(&Default::default(), &state.settings, 36).unwrap();
    let snapshot = scan.clone();
    assert!(
        !scan
            .migrate_single_shared_user(&group, SharedUidMigration::BestEffort, &inputs.disabled)
            .unwrap()
    );
    assert_eq!(scan, snapshot); // Unparsed active setting cannot migrate.
    scan.apply(&inputs.active["com.google.android.gsf"])
        .unwrap();
    let snapshot = scan.clone();
    assert!(
        !scan
            .migrate_single_shared_user(&group, SharedUidMigration::BestEffort, &inputs.disabled)
            .unwrap()
    );
    assert_eq!(scan, snapshot); // Parsed package is not leaving.

    // Synthetic parser-policy inputs; the original APK and its verified signers
    // stay untouched. Both version states are checked before unlinking either.
    inputs
        .active
        .get_mut("com.google.android.gsf")
        .unwrap()
        .parsed
        .booleans |= booleans::LEAVING_SHARED_UID;
    let mut no_disabled = SigningScan::new(&Default::default(), &state.settings, 36).unwrap();
    no_disabled
        .apply(&inputs.active["com.google.android.gsf"])
        .unwrap();
    assert!(
        no_disabled
            .migrate_single_shared_user(&group, SharedUidMigration::BestEffort, &Default::default())
            .unwrap()
    );
    assert!(!no_disabled.settings.packages[0].shared_user);
    assert_eq!(no_disabled.settings.packages[0].app_id, 10001);
    state.settings.disabled_system_packages = state.settings.packages.clone();
    let old = &inputs.active["com.google.android.gsf"];
    inputs.disabled.insert(
        old.settings.name.clone(),
        aim_services::package::scan::Record {
            settings: old.settings.clone(),
            parsed: old.parsed.clone(),
            signing: old.signing.clone(),
            identity: old.identity.clone(),
            origin: old.origin,
        },
    );
    let mut scan = SigningScan::new(&Default::default(), &state.settings, 36).unwrap();
    scan.apply(&inputs.active["com.google.android.gsf"])
        .unwrap();
    let snapshot = scan.clone();
    assert!(
        !scan
            .migrate_single_shared_user(
                &group,
                SharedUidMigration::NewInstallOnly,
                &inputs.disabled
            )
            .unwrap()
    );
    assert_eq!(scan, snapshot);
    inputs
        .disabled
        .get_mut("com.google.android.gsf")
        .unwrap()
        .parsed
        .booleans &= !booleans::LEAVING_SHARED_UID;
    assert!(
        !scan
            .migrate_single_shared_user(&group, SharedUidMigration::BestEffort, &inputs.disabled)
            .unwrap()
    );
    assert_eq!(scan, snapshot);
    assert!(
        !scan
            .migrate_single_shared_user(&group, SharedUidMigration::BestEffort, &Default::default())
            .unwrap()
    );
    assert_eq!(scan, snapshot);
    inputs
        .disabled
        .get_mut("com.google.android.gsf")
        .unwrap()
        .parsed
        .booleans |= booleans::LEAVING_SHARED_UID;
    for disabled_member in [false, true] {
        let mut multiple = scan.clone();
        let packages = if disabled_member {
            &mut multiple.settings.disabled_system_packages
        } else {
            &mut multiple.settings.packages
        };
        let mut other = packages[0].clone();
        other.name = "other.member".into();
        packages.push(other);
        let before = multiple.clone();
        assert!(
            !multiple
                .migrate_single_shared_user(
                    &group,
                    SharedUidMigration::BestEffort,
                    &inputs.disabled
                )
                .unwrap()
        );
        assert_eq!(multiple, before);
    }
    assert!(
        scan.migrate_single_shared_user(&group, SharedUidMigration::BestEffort, &inputs.disabled)
            .unwrap()
    );
    assert!(!scan.settings.packages[0].shared_user);
    assert!(!scan.settings.disabled_system_packages[0].shared_user);
    assert_eq!(scan.settings.packages[0].app_id, 10001);
    assert_eq!(scan.settings.disabled_system_packages[0].app_id, 10001);
    assert_eq!(
        scan.identities.ids.get(10001),
        Some(&Owner::Package("com.google.android.gsf".into()))
    );
    assert!(!scan.identities.shared_users.contains_key(&group));
    assert!(scan.settings.shared_users.is_empty());
    let mut expected = snapshot.settings.clone();
    expected.packages[0].shared_user = false;
    expected.disabled_system_packages[0].shared_user = false;
    expected.shared_users.clear();
    assert_eq!(scan.settings, expected); // All other persisted metadata survives.
    let mut before_ids = snapshot.identities.ids.clone();
    assert_eq!(
        scan.identities.ids.acquire(Owner::Package("next".into())),
        before_ids.acquire(Owner::Package("next".into()))
    );
}

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn new_uid_scan_creates_manifest_groups_and_keeps_leaving_new_packages_independent() {
    use aim_services::package::{
        owner::app_ids::Owner,
        pkg::booleans,
        scan::{Code, Kind, Location, Partition, UidScan},
    };
    let root = aim_paths::original_image();
    let image = root.clone();
    let apks = Apks {
        files: Box::new(move |p| Some(image.join(p.trim_start_matches('/')))),
        platform: Platform::load(&root, Default::default()).unwrap(),
    };
    let state = State {
        settings: settings::Settings {
            packages: vec![settings::Package {
                name: "com.google.android.gsf".into(),
                code_path:
                    "/system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk"
                        .into(),
                ..Default::default()
            }],
            ..Default::default()
        },
        list: vec![],
        access: None,
        users: vec![],
    };
    let mut inputs = Inputs::load_verified_code(&state, &apks).unwrap();
    let record = inputs.active.remove("com.google.android.gsf").unwrap();
    let mut code = Code {
        location: Location {
            path: record.settings.code_path,
            partition: Partition::SystemExt,
            kind: Kind::PrivApp,
            apex: None,
        },
        parsed: record.parsed,
        signing: record.signing,
    };
    assert!(!code.parsed.is(booleans::LEAVING_SHARED_UID));
    let group = code.parsed.shared_user_id.clone().unwrap();
    let mut scan = UidScan::new(&Default::default(), &Default::default()).unwrap();
    let (_, uid) = scan.apply(&code).unwrap();
    assert_eq!(uid.app_id, 10000);
    assert_eq!(uid.shared_user.as_deref(), Some(group.as_str()));
    assert_eq!(
        scan.identities.ids.get(uid.app_id),
        Some(&Owner::SharedUser(group.clone()))
    );
    assert_eq!(scan.identities.shared_users[&group].flags, 0);
    assert!(scan.identities.shared_users[&group].signatures.is_none()); // UID allocation grants no signing authorization.
    let snapshot = scan.clone();
    assert_eq!(scan.apply(&code).unwrap().1, uid);
    assert_eq!(scan, snapshot);
    code.parsed.shared_user_id = Some("changed.group".into());
    assert_eq!(scan.apply(&code).unwrap_err().phase, "identity");
    assert_eq!(scan, snapshot);
    code.parsed.shared_user_id = Some(group.clone());
    assert!(scan.reject_pending(&code.parsed.package_name).unwrap());
    assert_eq!(
        scan.identities.ids.get(10000),
        Some(&Owner::SharedUser(group.clone()))
    );
    assert!(scan.prune_unused_groups().unwrap().contains(&group));
    let (_, retried) = scan.apply(&code).unwrap();
    assert_eq!(retried.app_id, 10001);
    assert!(scan.accept_uid(&code.parsed.package_name).unwrap());
    assert!(!scan.prune_unused_groups().unwrap().contains(&group));
    assert_eq!(
        scan.identities.ids.get(10001),
        Some(&Owner::SharedUser(group.clone()))
    );

    code.parsed.booleans |= booleans::LEAVING_SHARED_UID;
    let mut leaving = UidScan::new(&Default::default(), &Default::default()).unwrap();
    let (_, uid) = leaving.apply(&code).unwrap();
    assert_eq!(uid.app_id, 10000);
    assert_eq!(uid.shared_user, None);
    assert!(!leaving.identities.shared_users.contains_key(&group));
    assert_eq!(
        leaving.identities.ids.get(uid.app_id),
        Some(&Owner::Package(code.parsed.package_name.clone()))
    );
    // Already allocated independent identity ignores the leaving declaration.
    let before = leaving.clone();
    assert_eq!(leaving.apply(&code).unwrap().1, uid);
    assert_eq!(leaving, before);
}

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn new_system_scan_connects_uid_settings_signing_and_rejection_cleanup() {
    use aim_services::package::{
        owner::app_ids::Owner,
        pkg::booleans,
        scan::{Code, Location, SettingMetadata, SigningError, SigningScan, UserPolicy},
        sign::SigningDetails,
    };
    let root = aim_paths::original_image();
    let image = root.clone();
    let apks = Apks {
        files: Box::new(move |p| Some(image.join(p.trim_start_matches('/')))),
        platform: Platform::load(&root, Default::default()).unwrap(),
    };
    let state = State {
        settings: settings::Settings {
            packages: [
                (
                    "com.google.android.gsf",
                    "/system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk",
                ),
                ("android", "/system/framework/framework-res.apk"),
            ]
            .into_iter()
            .map(|(name, path)| settings::Package {
                name: name.into(),
                code_path: path.into(),
                ..Default::default()
            })
            .collect(),
            ..Default::default()
        },
        list: vec![],
        access: None,
        users: vec![],
    };
    let mut inputs = Inputs::load_verified_code(&state, &apks).unwrap();
    let code = |r: aim_services::package::scan::Record, partition, kind| Code {
        location: Location {
            path: r.settings.code_path,
            partition,
            kind,
            apex: None,
        },
        parsed: r.parsed,
        signing: r.signing,
    };
    let mut google = code(
        inputs.active.remove("com.google.android.gsf").unwrap(),
        Partition::SystemExt,
        Kind::PrivApp,
    );
    let platform = code(
        inputs.active.remove("android").unwrap(),
        Partition::System,
        Kind::Framework,
    );
    // Constructor inputs are explicit test policy. Code and declarations are
    // original, parsed natively and fully integrity-verified above.
    let metadata = |c: &Code| SettingMetadata {
        code_path: c.location.path.clone(),
        legacy_native_library_path: None,
        primary_cpu_abi: None,
        secondary_cpu_abi: None,
        version_code: ((c.parsed.version_code_major as i64) << 32)
            | c.parsed.version_code as u32 as i64,
        flags: settings::FLAG_SYSTEM,
        private_flags: settings::PRIVATE_FLAG_PRIVILEGED,
        last_modified_time: 0,
        uses_sdk_libraries: vec![],
        uses_static_libraries: vec![],
        mime_groups: c.parsed.mime_groups.clone(),
        domain_set_id: [1; 16],
        target_sdk_version: c.parsed.target_sdk_version,
        restrict_update_hash: c.parsed.restrict_update_hash.clone(),
    };
    let policy = UserPolicy {
        install_user: None,
        users: Some(&[]),
        allow_install: true,
        instant_app: false,
        virtual_preload: false,
        stopped_system_app: true,
    };
    let mut scan = SigningScan::new(&Default::default(), &Default::default(), 36).unwrap();
    let accepted = scan
        .apply_new_system(&google, metadata(&google), policy)
        .unwrap();
    let group = google.parsed.shared_user_id.as_ref().unwrap();
    let uid = accepted.record.settings.app_id;
    assert_eq!(uid, 10000);
    assert_eq!(
        (
            scan.identities.shared_users[group].flags,
            scan.identities.shared_users[group].private_flags
        ),
        (0, 0),
    );
    assert!(accepted.record.settings.shared_user);
    assert!(accepted.signing.system_signature_mismatch.is_none());
    assert_eq!(
        accepted.users[&0],
        aim_services::package::restrictions::UserState {
            stopped: true,
            ..Default::default()
        }
    );
    assert_eq!(
        accepted
            .record
            .settings
            .signatures
            .as_ref()
            .unwrap()
            .signatures,
        google.signing.signatures
    );
    assert_eq!(accepted.record.parsed, google.parsed);
    assert_eq!(
        scan.identities.shared_users[group].signatures_changed,
        Some(false)
    );
    assert_eq!(
        scan.identities.ids.get(uid),
        Some(&Owner::SharedUser(group.clone()))
    );
    let mut absent = scan.identities.shared_users[group].clone();
    assert!(!absent.remove_package(&accepted.record.settings.name));
    let mut accepted = accepted;
    accepted
        .record
        .parsed
        .requested_permissions
        .push("android.permission.FACTORY_TEST".into());
    let completed = scan
        .finish_application_metadata(accepted, true, false)
        .unwrap();
    let final_flags = (
        completed.record.settings.flags,
        completed.record.settings.private_flags,
    );
    assert_ne!(final_flags.0 & 16, 0);
    assert_eq!(
        (
            scan.identities.shared_users[group].flags,
            scan.identities.shared_users[group].private_flags
        ),
        final_flags
    );
    let retained = scan
        .finish_application_metadata(completed, false, true)
        .unwrap();
    assert_eq!(
        (
            scan.identities.shared_users[group].flags,
            scan.identities.shared_users[group].private_flags
        ),
        final_flags
    );
    let mut removed = scan.identities.shared_users[group].clone();
    assert!(removed.remove_package(&retained.record.settings.name));
    assert_eq!((removed.flags, removed.private_flags), (0, 0));

    use aim_services::package::scan::{
        AbiPolicy, AbiScanContext, AbiScanMode, NativeLibraryEnvironment,
        NativeLibraryInstallPolicy, ScanClock, ScanMetadataCompletion,
    };
    let abi_policy = AbiPolicy {
        all: vec!["arm64-v8a".into()],
        bit32: vec![],
        bit64: vec!["arm64-v8a".into()],
        native32: vec![],
        native64: vec!["arm64-v8a".into()],
        force_multi_arch_match: false,
    };
    let environment = NativeLibraryEnvironment {
        preferred_abi: "arm64-v8a",
        app_lib32_install_dir: "/data/app-lib",
        code_is_directory: false,
        canonical_source: None,
    };
    let completion = || ScanMetadataCompletion {
        seinfo: common::seinfo::scan(),
        abi_policy: &abi_policy,
        native_environment: &environment,
        context: AbiScanContext {
            mode: AbiScanMode::Install { moved: None },
            system: true,
            updated: false,
            override_abi: None,
            platform_runtime_64bit: None,
        },
        install: NativeLibraryInstallPolicy {
            page_size: 4096,
            extract: false,
            debuggable: false,
            compat_16kb_disabled: false,
            manifest_compat_disabled: false,
        },
        destination: None,
        clock: ScanClock {
            current_time: 0,
            user_id: -1,
            update_time: false,
        },
        factory_test: true,
    };
    let unreadable = Apks {
        files: Box::new(|_| None),
        platform: Platform::load(&root, Default::default()).unwrap(),
    };
    for shared in [true, false] {
        let mut code = Code {
            location: google.location.clone(),
            parsed: google.parsed.clone(),
            signing: google.signing.clone(),
        };
        code.parsed.package_name = "late.failure".into();
        code.parsed
            .requested_permissions
            .push("android.permission.FACTORY_TEST".into());
        if !shared {
            code.parsed.shared_user_id = None;
        }
        let mut transaction =
            SigningScan::new(&Default::default(), &Default::default(), 36).unwrap();
        let before = transaction.clone();
        assert!(
            matches!(transaction.scan_new_system(&code, metadata(&code), policy, &unreadable, completion()), Err(SigningError::Rejected(ref e)) if e.phase == "code-time")
        );
        assert_eq!(transaction.settings, before.settings);
        assert_eq!(transaction.libraries, before.libraries);
        if shared {
            let mut group = transaction.identities.shared_users[group].clone();
            assert!(!group.remove_package("late.failure"));
            assert_eq!((group.flags, group.private_flags), (0, 0));
            assert!(group.signatures.is_none());
        } else {
            assert!(transaction.identities.ids.get(10000).is_none());
        }
        let completed = transaction
            .scan_new_system(&code, metadata(&code), policy, &apks, completion())
            .unwrap();
        assert_eq!(
            completed.candidate.record.settings.app_id,
            if shared { 10000 } else { 10001 }
        );
        if shared {
            let group = &transaction.identities.shared_users[group];
            assert_eq!(
                (group.flags, group.private_flags),
                (
                    completed.candidate.record.settings.flags,
                    completed.candidate.record.settings.private_flags
                )
            );
            assert_ne!(group.flags & 16, 0);
        }
    }

    let snapshot = scan.clone();
    assert!(matches!(
        scan.apply_new_system(&google, metadata(&google), policy),
        Err(SigningError::Rejected(_))
    ));
    assert_eq!(scan, snapshot);
    scan.apply_new_system(&platform, metadata(&platform), policy)
        .unwrap();
    assert_eq!(scan.settings.packages.len(), 2);
    assert_eq!(scan.settings.packages[1].app_id, 1000);
    assert_eq!(
        scan.identities.shared_users[group].signatures_changed,
        Some(false)
    );
    assert_eq!(scan.settings.packages[0], snapshot.settings.packages[0]);

    // Synthetic name/signer policy candidates exercise failure without modifying
    // either original APK. Initialized groups cannot accept an unrelated signer.
    google.parsed.package_name = "new.member".into();
    let original_signer = google.signing.clone();
    google.signing = platform.signing.clone();
    let snapshot = scan.clone();
    assert!(matches!(
        scan.apply_new_system(&google, metadata(&google), policy),
        Err(SigningError::Fatal(_))
    ));
    assert_eq!(scan, snapshot);
    google.signing = original_signer;
    scan.apply_new_system(&google, metadata(&google), policy)
        .unwrap();
    assert_eq!(scan.settings.packages.last().unwrap().app_id, uid);

    // First system mismatch can replace a restored group; subsequent members
    // retain the per-scan marker and follow the pinned first-API failure policy.
    let original_signer = google.signing.clone();
    let restored = settings::Settings {
        shared_users: vec![settings::SharedUser {
            name: group.clone(),
            app_id: 10010,
            flags: 0,
            signatures: Some(settings::Signatures {
                signatures: platform.signing.signatures.clone(),
                scheme_version: platform.signing.scheme_version,
                ..Default::default()
            }),
        }],
        ..Default::default()
    };
    for first_api in [29, 36] {
        google.parsed.package_name = "first.new.member".into();
        google.signing = original_signer.clone();
        let mut ota = SigningScan::new(&Default::default(), &restored, first_api).unwrap();
        let first = ota
            .apply_new_system(&google, metadata(&google), policy)
            .unwrap();
        assert!(first.signing.system_signature_mismatch.is_some());
        assert_eq!(first.record.settings.app_id, 10010);
        assert_eq!(
            ota.identities.shared_users[group].signatures_changed,
            Some(true)
        );
        google.parsed.package_name = "second.new.member".into();
        google.signing = platform.signing.clone();
        let snapshot = ota.clone();
        match ota
            .apply_new_system(&google, metadata(&google), policy)
            .unwrap_err()
        {
            SigningError::Rejected(e) => {
                assert_eq!(first_api, 29);
                assert!(e.message.contains("-104"));
            }
            SigningError::Fatal(_) => assert_eq!(first_api, 36),
            error => panic!("unexpected signing rejection: {error:?}"),
        }
        assert_eq!(ota, snapshot);
    }
    google.signing = original_signer;

    // Failed independent allocation removes the slot and advances the cursor.
    google.parsed.package_name = "new.independent".into();
    google.parsed.booleans |= booleans::LEAVING_SHARED_UID;
    let original_signer = google.signing.clone();
    google.signing = SigningDetails::from_saved(&Default::default()).unwrap();
    let snapshot = scan.clone();
    assert!(
        matches!(scan.apply_new_system(&google, metadata(&google), policy), Err(SigningError::Rejected(ref e)) if e.phase == "signatures")
    );
    assert_eq!(scan.settings, snapshot.settings);
    assert_eq!(
        scan.identities.shared_users,
        snapshot.identities.shared_users
    );
    assert!(scan.identities.ids.get(10001).is_none());
    google.signing = original_signer;
    let independent = scan
        .apply_new_system(&google, metadata(&google), policy)
        .unwrap();
    assert_eq!(independent.record.settings.app_id, 10002);
    assert!(!independent.record.settings.shared_user);
    assert_eq!(
        scan.identities.ids.get(10002),
        Some(&Owner::Package("new.independent".into()))
    );

    // A failed new shared member retains only the group's allocation until prune.
    google.parsed.package_name = "new.empty.group".into();
    google.parsed.booleans &= !booleans::LEAVING_SHARED_UID;
    google.parsed.shared_user_id = Some("new.group".into());
    let original_signer = google.signing.clone();
    google.signing = SigningDetails::from_saved(&Default::default()).unwrap();
    let snapshot = scan.clone();
    assert!(matches!(
        scan.apply_new_system(&google, metadata(&google), policy),
        Err(SigningError::Rejected(_))
    ));
    assert_eq!(scan.settings, snapshot.settings);
    assert_eq!(
        scan.identities.ids.get(10003),
        Some(&Owner::SharedUser("new.group".into()))
    );
    assert!(
        scan.identities.shared_users["new.group"]
            .signatures
            .is_none()
    );
    assert!(
        scan.identities
            .prune_unused(&scan.settings)
            .contains(&"new.group".into())
    );
    google.signing = original_signer;
    let retried = scan
        .apply_new_system(&google, metadata(&google), policy)
        .unwrap();
    assert_eq!(retried.record.settings.app_id, 10004);
    assert_eq!(scan.settings.shared_users.last().unwrap().name, "new.group");

    // Physical-origin and invalid static shared-UID checks run before UID allocation.
    let snapshot = scan.clone();
    google.location.path = "/data/app/new/base.apk".into();
    assert!(
        matches!(scan.apply_new_system(&google, metadata(&google), policy), Err(SigningError::Rejected(ref e)) if e.phase == "location")
    );
    assert_eq!(scan, snapshot);
    google.location.path = "/system/app/new/base.apk".into();
    google.parsed.static_shared_library_name = Some("static".into());
    assert!(
        matches!(scan.apply_new_system(&google, metadata(&google), policy), Err(SigningError::Rejected(ref e)) if e.message.contains("shared UID"))
    );
    assert_eq!(scan, snapshot);
    // Original-package declarations and metadata are synthetic policy inputs;
    // APKs remain untouched and the signer is the verified original GSF signer.
    google.parsed.static_shared_library_name = None;
    google.parsed.shared_user_id = None;
    google.parsed.package_name = "incoming".into();
    google.parsed.manifest_package_name = Some("incoming".into());
    google.parsed.original_packages = Some(vec![Some("old".into())]);
    let old = settings::Package {
        name: "old".into(),
        code_path: "/system/app/old/base.apk".into(),
        app_id: 11000,
        flags: settings::FLAG_SYSTEM,
        mime_groups: vec![("old.mime".into(), vec!["text/plain".into()])],
        pending_restore: true,
        category_hint: 8,
        ..Default::default()
    };
    let old_users = std::collections::BTreeMap::from([(
        10,
        aim_services::package::restrictions::UserState {
            installed: false,
            enabled: 2,
            disabled_components: Some(vec!["old.Component".into()]),
            ..Default::default()
        },
    )]);
    let users = std::collections::BTreeMap::from([("old".into(), old_users.clone())]);
    let saved = settings::Settings {
        packages: vec![old.clone()],
        ..Default::default()
    };
    let mut scan = SigningScan::new(&Default::default(), &saved, 36).unwrap();
    let snapshot = scan.clone();
    assert!(
        matches!(scan.apply_original_system(&google, metadata(&google), &Default::default()),
        Err(SigningError::Rejected(ref e)) if e.phase == "setting")
    );
    assert_eq!(scan, snapshot);
    // Fallible declaration work must not publish the copied original setting.
    google.parsed.library_names = vec!["adopted.library".into()];
    let base = google.parsed.base_apk_path.take();
    assert!(
        matches!(scan.apply_original_system(&google, metadata(&google), &users),
        Err(SigningError::Rejected(ref e)) if e.phase == "libraries")
    );
    assert_eq!(scan, snapshot);
    google.parsed.base_apk_path = base;
    let adopted = scan
        .apply_original_system(&google, metadata(&google), &users)
        .unwrap();
    assert_eq!(adopted.record.settings.name, "old");
    assert_eq!(
        adopted.record.settings.real_name.as_deref(),
        Some("incoming")
    );
    assert_eq!(adopted.record.settings.app_id, 11000);
    assert_eq!(adopted.record.settings.mime_groups, old.mime_groups);
    assert_eq!(adopted.record.settings.category_hint, 8);
    assert!(adopted.record.settings.pending_restore);
    assert_eq!(adopted.record.parsed.package_name, "old");
    assert_eq!(adopted.users, old_users);
    assert_eq!(scan.settings.packages.len(), 1);
    assert_eq!(
        scan.settings.renamed_packages,
        vec![("incoming".into(), "old".into())]
    );
    assert_eq!(scan.identities, snapshot.identities);
    assert_eq!(
        scan.libraries
            .get("adopted.library", -1)
            .unwrap()
            .package_name
            .as_deref(),
        Some("old")
    );
    assert_eq!(saved.packages[0], old);
    // A subsequent declaration cannot adopt an original already scanned.
    google.parsed.package_name = "another".into();
    google.parsed.manifest_package_name = Some("another".into());
    let snapshot = scan.clone();
    assert!(
        matches!(scan.apply_original_system(&google, metadata(&google), &users),
        Err(SigningError::Rejected(ref e)) if e.message.contains("eligible original"))
    );
    assert_eq!(scan, snapshot);
    // Adoption retains a shared group's UID and still runs the INSTALL signer gate.
    google.parsed.package_name = "incoming".into();
    google.parsed.manifest_package_name = Some("incoming".into());
    google.parsed.shared_user_id = Some("adopt.group".into());
    let signatures = adopted.record.settings.signatures.clone();
    let saved = settings::Settings {
        packages: vec![settings::Package {
            shared_user: true,
            signatures: signatures.clone(),
            ..old.clone()
        }],
        shared_users: vec![settings::SharedUser {
            name: "adopt.group".into(),
            app_id: 11000,
            flags: 0,
            signatures,
        }],
        ..Default::default()
    };
    let mut saved = saved;
    let peer = settings::Package {
        name: "peer".into(),
        code_path: google.location.path.clone(),
        ..saved.packages[0].clone()
    };
    saved.packages.push(peer.clone());
    let mut scan = SigningScan::new(&Default::default(), &saved, 29).unwrap();
    let peer_identity = aim_services::package::scan::Identity {
        manifest_name: "incoming".into(),
        internal_name: "peer".into(),
        real_name: None,
    };
    let mut parsed = google.parsed.clone();
    peer_identity.apply(&mut parsed);
    scan.apply(&aim_services::package::scan::Record {
        settings: peer,
        parsed,
        signing: google.signing.clone(),
        identity: peer_identity,
        origin: aim_services::package::owner::shared_users::ScanOrigin::SystemDirectory,
    })
    .unwrap();
    let snapshot = scan.clone();
    let signer = google.signing.clone();
    google.signing = platform.signing.clone();
    let rejected = scan.apply_original_system(&google, metadata(&google), &users);
    assert!(
        matches!(rejected, Err(SigningError::Rejected(ref e)) if e.phase == "authorization" && e.message.contains("-104")),
        "unrelated signer should reject the initialized group"
    );
    assert_eq!(scan, snapshot);
    google.signing = signer;
    let accepted = scan
        .apply_original_system(&google, metadata(&google), &users)
        .unwrap();
    assert_eq!(accepted.record.settings.app_id, 11000);
    assert!(accepted.record.settings.shared_user);
    assert_eq!(accepted.users, old_users);
    assert_eq!(scan.identities.ids, snapshot.identities.ids);
    assert!(accepted.signing.system_signature_mismatch.is_none());
    // A verified update on /data must retain the old signer gate while staging
    // code/ABI/MIME changes; signer rejection publishes none of those changes.
    google.location.path = "/data/app/update/base.apk".into();
    let update = |code: &Code| aim_services::package::scan::SettingUpdate {
        code_path: code.location.path.clone(),
        legacy_native_library_path: Some("new.lib".into()),
        primary_cpu_abi: Some("arm64-v8a".into()),
        secondary_cpu_abi: None,
        flags: settings::FLAG_SYSTEM,
        private_flags: 8,
        uses_sdk_libraries: vec![],
        uses_static_libraries: vec![],
        mime_groups: vec!["old.mime".into(), "new.mime".into()],
        domain_set_id: [2; 16],
        target_sdk_version: 36,
        restrict_update_hash: None,
    };
    let snapshot = scan.clone();
    let signer = google.signing.clone();
    google.signing = platform.signing.clone();
    assert!(
        matches!(scan.apply_existing(&google, update(&google), &users, None, None),
        Err(SigningError::Rejected(ref e)) if e.phase == "authorization")
    );
    assert_eq!(scan, snapshot);
    google.signing = signer;
    let updated = scan
        .apply_existing(&google, update(&google), &users, None, None)
        .unwrap();
    assert_eq!(updated.record.settings.code_path, google.location.path);
    assert_eq!(updated.record.settings.app_id, 11000);
    assert_eq!(
        updated.record.settings.primary_cpu_abi.as_deref(),
        Some("arm64-v8a")
    );
    assert_eq!(
        updated.record.settings.mime_groups,
        vec![
            ("old.mime".into(), vec!["text/plain".into()]),
            ("new.mime".into(), vec![])
        ]
    );
    assert_eq!(updated.users, old_users);
    assert_eq!(scan.identities.ids, snapshot.identities.ids);
    assert!(updated.signing.system_signature_mismatch.is_none());
    let stale = aim_services::package::scan::NewPackageOutcome {
        record: aim_services::package::scan::Record {
            settings: settings::Package {
                last_update_time: -1,
                ..updated.record.settings.clone()
            },
            parsed: updated.record.parsed.clone(),
            signing: updated.record.signing.clone(),
            identity: updated.record.identity.clone(),
            origin: updated.record.origin,
        },
        users: updated.users.clone(),
        signing: aim_services::package::scan::SigningOutcome {
            system_signature_mismatch: None,
        },
    };
    let time = aim_services::package::scan::ScanTime {
        current_time: 0,
        file_time: 77,
        user_id: -1,
        update_time: false,
    };
    let snapshot = scan.clone();
    assert!(
        matches!(scan.finish_metadata(stale, time), Err(SigningError::Rejected(ref e)) if e.phase == "metadata")
    );
    assert_eq!(scan, snapshot);
    let expected_version = (i64::from(updated.record.parsed.version_code_major) << 32)
        | i64::from(updated.record.parsed.version_code as u32);
    let completed = scan.finish_metadata(updated, time).unwrap();
    let mut expected_users = old_users;
    expected_users.get_mut(&10).unwrap().first_install_time = 77;
    assert_eq!(completed.users, expected_users);
    assert_eq!(completed.record.settings.last_update_time, 77);
    assert_eq!(completed.record.settings.last_modified_time, 77);
    assert_eq!(completed.record.settings.version_code, expected_version);
    assert_eq!(
        completed.record.settings.volume_uuid,
        completed.record.parsed.volume_uuid
    );
    assert!(completed.record.settings.install_source.is_orphaned);
    assert_eq!(scan.settings.packages[0], completed.record.settings);
    assert_eq!(scan.identities.ids, snapshot.identities.ids);
    assert_eq!(scan.libraries, snapshot.libraries);
}

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn static_library_scan_checks_the_previous_version_and_commits_the_target() {
    use aim_services::package::{
        libraries::Registry,
        model::PackageState,
        owner::shared_users::ScanOrigin,
        scan::{
            Code, Identity, Location, Record, SettingMetadata, SigningError, SigningScan,
            UserPolicy,
        },
    };
    use std::sync::Arc;
    let root = aim_paths::original_image();
    let image = root.clone();
    let apks = Apks {
        files: Box::new(move |p| Some(image.join(p.trim_start_matches('/')))),
        platform: Platform::load(&root, Default::default()).unwrap(),
    };
    let input = State {
        settings: settings::Settings {
            packages: [
                (
                    "com.google.android.gsf",
                    "/system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk",
                ),
                ("android", "/system/framework/framework-res.apk"),
            ]
            .into_iter()
            .map(|(name, path)| settings::Package {
                name: name.into(),
                code_path: path.into(),
                ..Default::default()
            })
            .collect(),
            ..Default::default()
        },
        list: vec![],
        access: None,
        users: vec![],
    };
    let mut verified = Inputs::load_verified_code(&input, &apks).unwrap();
    let google = verified.active.remove("com.google.android.gsf").unwrap();
    let platform = verified.active.remove("android").unwrap();
    let saved = |signing: &aim_services::package::sign::SigningDetails| settings::Signatures {
        signatures: signing.signatures.clone(),
        scheme_version: signing.scheme_version,
        past_signatures: signing.past_signing_certificates.clone(),
        ..Default::default()
    };
    // Synthetic static declarations isolate registry/signature policy using
    // verified original signer material. They are not edited original APKs.
    let mut raw = google.parsed.clone();
    raw.package_name = "test".into();
    raw.manifest_package_name = Some("test".into());
    raw.shared_user_id = None;
    raw.static_shared_library_name = Some("test.static".into());
    raw.static_shared_lib_version = 7;
    raw.original_packages = None;
    raw.library_names.clear();
    raw.activities.clear();
    raw.services.clear();
    raw.providers.clear();
    raw.receivers.clear();
    raw.permission_groups.clear();
    raw.attributions.clear();
    raw.permissions.clear();
    raw.protected_broadcasts.clear();
    raw.overlay_target = None;

    let mut registry = Registry::default();
    let mut settings = settings::Settings::default();
    for (version, uid) in [(1, 10001), (5, 10002), (7, 10003), (9, 10004)] {
        let mut parsed = raw.clone();
        parsed.static_shared_lib_version = version;
        let name = format!("test_{version}");
        registry
            .add_package(
                &PackageState {
                    name: name.clone(),
                    pkg: Some(Arc::new(parsed)),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        if version != 7 {
            settings.packages.push(settings::Package {
                name,
                app_id: uid,
                code_path: format!("/system/app/lib{version}"),
                signatures: Some(saved(if version == 5 {
                    &platform.signing
                } else {
                    &google.signing
                })),
                ..Default::default()
            });
        }
    }
    let code = Code {
        location: Location {
            path: google.settings.code_path.clone(),
            partition: Partition::SystemExt,
            kind: Kind::PrivApp,
            apex: None,
        },
        parsed: raw.clone(),
        signing: google.signing.clone(),
    };
    let mut scan = SigningScan::new(&Default::default(), &settings, 36).unwrap();
    // Populate the owned registry in accepted scan order, rather than injecting
    // declarations ahead of signer reconciliation.
    for version in [1, 5, 9] {
        let mut parsed = raw.clone();
        parsed.static_shared_lib_version = version;
        let identity = Identity::select(&parsed, &settings, true);
        identity.apply(&mut parsed);
        let record = Record {
            settings: settings
                .packages
                .iter()
                .find(|p| p.name == identity.internal_name)
                .unwrap()
                .clone(),
            parsed,
            identity,
            signing: if version == 5 {
                platform.signing.clone()
            } else {
                google.signing.clone()
            },
            origin: ScanOrigin::SystemDirectory,
        };
        let outcome = scan.apply(&record).unwrap();
        assert_eq!(outcome.system_signature_mismatch.is_some(), version != 1);
        assert_eq!(
            scan.libraries
                .get("test.static", version)
                .unwrap()
                .package_name
                .as_deref(),
            Some(record.settings.name.as_str())
        );
    }
    let prior = scan.settings.clone();
    let metadata = |c: &Code| SettingMetadata {
        code_path: c.location.path.clone(),
        legacy_native_library_path: None,
        primary_cpu_abi: None,
        secondary_cpu_abi: None,
        version_code: c.parsed.static_shared_lib_version,
        flags: settings::FLAG_SYSTEM,
        private_flags: 0,
        last_modified_time: 0,
        uses_sdk_libraries: vec![],
        uses_static_libraries: vec![],
        mime_groups: vec![],
        domain_set_id: [2; 16],
        target_sdk_version: c.parsed.target_sdk_version,
        restrict_update_hash: None,
    };
    let policy = UserPolicy {
        install_user: None,
        users: None,
        allow_install: true,
        instant_app: false,
        virtual_preload: false,
        stopped_system_app: false,
    };
    let accepted = scan
        .apply_new_system(&code, metadata(&code), policy)
        .unwrap();
    // System-directory mismatch follows the original OTA exception, with an
    // explicit diagnostic. The selected previous version is not overwritten.
    assert!(accepted.signing.system_signature_mismatch.is_some());
    assert_eq!(accepted.record.settings.name, "test_7");
    assert_eq!(accepted.record.settings.app_id, 10000);
    assert_eq!(
        accepted
            .record
            .settings
            .signatures
            .as_ref()
            .unwrap()
            .signatures,
        google.signing.signatures
    );
    assert_eq!(
        &scan.settings.packages[..prior.packages.len()],
        prior.packages.as_slice()
    );
    assert_eq!(accepted.record.parsed.package_name, "test_7");
    assert_eq!(
        accepted.record.parsed.manifest_package_name.as_deref(),
        Some("test")
    );

    // A declaration failure after signer reconciliation publishes neither
    // signatures nor libraries, while new UID cleanup advances the cursor.
    let mut broken = Code {
        location: code.location.clone(),
        parsed: code.parsed.clone(),
        signing: code.signing.clone(),
    };
    broken.parsed.static_shared_lib_version = 8;
    broken.parsed.base_apk_path = None;
    let snapshot = scan.clone();
    assert!(
        matches!(scan.apply_new_system(&broken, metadata(&broken), policy), Err(SigningError::Rejected(ref e)) if e.phase == "libraries")
    );
    assert_eq!(scan.settings, snapshot.settings);
    assert_eq!(scan.libraries, snapshot.libraries);
    assert_eq!(
        scan.identities.shared_users,
        snapshot.identities.shared_users
    );
    assert!(scan.identities.ids.get(10003).is_none());
    broken.parsed.base_apk_path = code.parsed.base_apk_path.clone();
    let retried = scan
        .apply_new_system(&broken, metadata(&broken), policy)
        .unwrap();
    assert_eq!(retried.record.settings.app_id, 10005);
    assert_eq!(
        scan.libraries
            .get("test.static", 8)
            .unwrap()
            .package_name
            .as_deref(),
        Some("test_8")
    );

    // Same input under /data must reject the version-5 signer mismatch even
    // though version 1, version 9 and the target's own signer could match.
    let target = settings::Package {
        name: "test_7".into(),
        app_id: 10003,
        code_path: "/data/app/lib7/base.apk".into(),
        signatures: Some(saved(&google.signing)),
        ..Default::default()
    };
    settings.packages.push(target.clone());
    let identity = Identity::select(&raw, &settings, false);
    identity.apply(&mut raw);
    let record = Record {
        settings: target,
        parsed: raw,
        signing: google.signing.clone(),
        identity,
        origin: ScanOrigin::Data,
    };
    let mut scan = SigningScan::new(&Default::default(), &settings, 36).unwrap();
    scan.libraries = registry.clone();
    let snapshot = scan.clone();
    assert!(
        matches!(scan.apply(&record), Err(SigningError::Rejected(ref e)) if e.phase == "authorization")
    );
    assert_eq!(scan, snapshot);
    // The real GSF lineage grants installed-data capability to its older
    // certificate. Selection must use that previous library signer, rather
    // than the target version's already-matching current certificate.
    let mut rotated = settings.clone();
    let lineage = google.signing.past_signing_certificates.as_ref().unwrap();
    assert_ne!(lineage[0].1 & 1, 0);
    rotated
        .packages
        .iter_mut()
        .find(|p| p.name == "test_5")
        .unwrap()
        .signatures = Some(settings::Signatures {
        scheme_version: 3,
        signatures: vec![lineage[0].0.clone()],
        ..Default::default()
    });
    let mut scan = SigningScan::new(&Default::default(), &rotated, 36).unwrap();
    scan.libraries = registry.clone();
    assert!(
        scan.apply(&record)
            .unwrap()
            .system_signature_mismatch
            .is_none()
    );
    assert_eq!(
        scan.settings
            .packages
            .iter()
            .find(|p| p.name == "test_5")
            .unwrap(),
        rotated
            .packages
            .iter()
            .find(|p| p.name == "test_5")
            .unwrap()
    );
    // Synthetic capability revocation leaves the verified original APK intact.
    let mut revoked = Record {
        settings: record.settings.clone(),
        parsed: record.parsed.clone(),
        signing: record.signing.clone(),
        identity: record.identity.clone(),
        origin: record.origin,
    };
    revoked.signing.past_signing_certificates.as_mut().unwrap()[0].1 &= !1;
    let mut scan = SigningScan::new(&Default::default(), &rotated, 36).unwrap();
    scan.libraries = registry.clone();
    let snapshot = scan.clone();
    assert!(
        matches!(scan.apply(&revoked), Err(SigningError::Rejected(ref e)) if e.phase == "authorization")
    );
    assert_eq!(scan, snapshot);

    // A disabled version-5 record is not this version-7 request's disabled
    // setting; selecting it accidentally would reject this matching update.
    let old = settings
        .packages
        .iter_mut()
        .find(|p| p.name == "test_5")
        .unwrap();
    old.signatures = Some(saved(&google.signing));
    let mut disabled = old.clone();
    disabled.signatures = Some(saved(&platform.signing));
    settings.disabled_system_packages.push(disabled);
    let mut scan = SigningScan::new(&Default::default(), &settings, 36).unwrap();
    scan.libraries = registry.clone();
    assert!(
        scan.apply(&record)
            .unwrap()
            .system_signature_mismatch
            .is_none()
    );
    assert_eq!(
        scan.settings
            .packages
            .iter()
            .find(|p| p.name == "test_5")
            .unwrap(),
        settings
            .packages
            .iter()
            .find(|p| p.name == "test_5")
            .unwrap()
    );
    let mut disabled = record.settings.clone();
    disabled.signatures = Some(saved(&platform.signing));
    settings.disabled_system_packages.push(disabled);
    let mut scan = SigningScan::new(&Default::default(), &settings, 36).unwrap();
    scan.libraries = registry.clone();
    let snapshot = scan.clone();
    assert!(
        matches!(scan.apply(&record), Err(SigningError::Rejected(ref e)) if e.message.contains("updated system"))
    );
    assert_eq!(scan, snapshot);
    // Updated-system dynamic declarations need the verified disabled original;
    // only names from that original survive. Inputs here are labeled policy
    // records using original verified signer material, not edited APKs.
    let active_setting = settings::Package {
        name: "provider".into(),
        app_id: 10020,
        code_path: "/data/app/provider/base.apk".into(),
        flags: settings::FLAG_SYSTEM,
        signatures: Some(saved(&google.signing)),
        ..Default::default()
    };
    let old_setting = settings::Package {
        code_path: "/system/app/provider".into(),
        signatures: None,
        ..active_setting.clone()
    };
    let mut old_parsed = google.parsed.clone();
    old_parsed.package_name = "provider".into();
    old_parsed.manifest_package_name = Some("provider".into());
    old_parsed.shared_user_id = None;
    old_parsed.library_names = vec!["allowed".into(), "builtin".into()];
    let identity = Identity::select(&old_parsed, &Default::default(), true);
    let old = Record {
        settings: old_setting.clone(),
        parsed: old_parsed.clone(),
        signing: google.signing.clone(),
        identity: identity.clone(),
        origin: ScanOrigin::SystemDirectory,
    };
    let mut parsed = old_parsed.clone();
    parsed.library_names.push("added.by.update".into());
    parsed.base_apk_path = Some(active_setting.code_path.clone());
    let updated = Record {
        settings: active_setting.clone(),
        parsed,
        signing: google.signing.clone(),
        identity,
        origin: ScanOrigin::Data,
    };
    let settings = settings::Settings {
        packages: vec![active_setting],
        disabled_system_packages: vec![old_setting],
        ..Default::default()
    };
    let mut config = aim_services::package::system_config::SystemConfig::default();
    config.libraries.insert(
        "builtin".into(),
        aim_services::package::system_config::Library {
            name: "builtin".into(),
            filename: "/system/framework/builtin.jar".into(),
            dependencies: vec![],
            on_bootclasspath_since: None,
            on_bootclasspath_before: None,
            can_be_safely_ignored: false,
            native: false,
        },
    );
    let mut scan = SigningScan::new(&config, &settings, 36).unwrap();
    let snapshot = scan.clone();
    assert!(
        matches!(scan.apply(&updated), Err(SigningError::Rejected(ref e)) if e.phase == "libraries")
    );
    assert_eq!(scan, snapshot);
    let mut wrong = Record {
        settings: old.settings.clone(),
        parsed: old.parsed.clone(),
        signing: old.signing.clone(),
        identity: old.identity.clone(),
        origin: old.origin,
    };
    wrong.settings.code_path = "/system/app/other".into();
    assert!(
        matches!(scan.apply_with_disabled(&updated, Some(&wrong)), Err(SigningError::Rejected(ref e)) if e.phase == "identity")
    );
    assert_eq!(scan, snapshot);
    scan.apply_with_disabled(&updated, Some(&old)).unwrap();
    let allowed = scan.libraries.get("allowed", -1).unwrap();
    assert_eq!(allowed.package_name.as_deref(), Some("provider"));
    assert_eq!(
        allowed.code_paths.as_ref().unwrap(),
        &vec!["/data/app/provider/base.apk".to_string()]
    );
    assert!(scan.libraries.get("added.by.update", -1).is_none());
    assert_eq!(
        scan.libraries.get("builtin", -1).unwrap().path.as_deref(),
        Some("/system/framework/builtin.jar")
    );
    assert_eq!(snapshot.libraries.entries().count(), 1);
    assert_eq!(scan.libraries.entries().count(), 2);
}

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn data_inventory_descends_containers_and_preserves_rejected_scan_paths() {
    let original = aim_paths::original_image();
    let dir = std::env::temp_dir().join(format!("aim-data-inventory-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let fixture = Fixture(dir);
    let gsf =
        original.join("system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk");
    assert!(gsf.is_file(), "missing original signed APK");
    for path in [
        "data/app/~~container/gsf",
        "mnt/expand/volume/app/other",
        "data/app/vmdl1.tmp",
        "data/app/smdl2tmp12",
        "data/app/empty",
    ] {
        std::fs::create_dir_all(fixture.0.join(path)).unwrap();
    }
    for path in [
        "data/app/~~container/gsf/base.apk",
        "mnt/expand/volume/app/other/base.apk",
        "data/app/vmdl1.tmp/base.apk",
        "data/app/smdl2tmp12/base.apk",
    ] {
        std::os::unix::fs::symlink(&gsf, fixture.0.join(path)).unwrap();
    }
    std::fs::write(fixture.0.join("data/app/unrelated.txt"), b"not an APK").unwrap();
    let root = fixture.0.clone();
    let apks = Apks {
        files: Box::new(move |path| Some(root.join(path.trim_start_matches('/')))),
        platform: Platform::load(&original, Default::default()).unwrap(),
    };
    let image = DataImage::load(&apks, &["volume".into()]).unwrap();
    assert_eq!(image.packages.len(), 2);
    assert_eq!(image.packages[0].scan_path, "/data/app/~~container");
    assert_eq!(image.packages[1].scan_path, "/mnt/expand/volume/app/other");
    assert_eq!(
        image.packages[0].code.location.path,
        "/data/app/~~container/gsf"
    );
    assert_eq!(
        image.packages[1].code.location.path,
        "/mnt/expand/volume/app/other"
    );
    for entry in &image.packages {
        let code = &entry.code;
        assert_eq!(
            code.parsed.path.as_deref(),
            Some(code.location.path.as_str())
        );
        assert_eq!(code.location.partition, Partition::Data);
        assert_eq!(code.location.parse_flags(), 0);
        assert_eq!(
            ScanPolicy::for_location(&code.location),
            ScanPolicy::default()
        );
        assert!(!code.location.privileged());
        assert_eq!(code.parsed.package_name, "com.google.android.gsf");
    }
    assert_eq!(
        image.packages[0].code.signing,
        image.packages[1].code.signing
    );
    assert_eq!(image.rejected.len(), 1);
    assert_eq!(image.rejected[0].location.path, "/data/app/empty");
    assert!(fixture.0.join("data/app/empty").exists());
    for volumes in [
        vec!["../bad".into()],
        vec![".".into()],
        vec!["".into()],
        vec!["volume".into(), "volume".into()],
    ] {
        assert_eq!(
            DataImage::load(&apks, &volumes).unwrap_err().phase,
            "location"
        );
    }
    let source_root = fixture.0.clone();
    let unavailable = Apks {
        files: Box::new(move |path| {
            (!path.ends_with("base.apk")).then(|| source_root.join(path.trim_start_matches('/')))
        }),
        platform: Platform::load(&original, Default::default()).unwrap(),
    };
    assert_eq!(
        DataImage::load(&unavailable, &[]).unwrap_err().phase,
        "signatures"
    );
    assert!(fixture.0.join("data/app/~~container/gsf/base.apk").exists());
    std::fs::remove_dir_all(fixture.0.join("data/app")).unwrap();
    assert!(DataImage::load(&apks, &[]).unwrap().packages.is_empty());
    std::fs::write(fixture.0.join("data/app"), b"not a directory").unwrap();
    assert_eq!(DataImage::load(&apks, &[]).unwrap_err().phase, "directory");
}
