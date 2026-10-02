//! Exercise scan inputs with original signed code. Explicit runs require
//! the pinned image; no parser cache, feed or writable guest data is used.
use aim_services::package::{
    State,
    parse::Platform,
    scan::{Image, Inputs, Kind, Partition},
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
    let mut group = SharedUser {
        app_id: 1000,
        flags: 1,
        private_flags: 8,
        signatures: None,
        signatures_changed: None,
    };
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
