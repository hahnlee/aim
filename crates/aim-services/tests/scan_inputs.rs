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
