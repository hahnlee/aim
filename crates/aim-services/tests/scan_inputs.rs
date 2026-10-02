//! Exercise scan inputs with original signed code. Explicit runs require
//! the pinned image; no parser cache, feed or writable guest data is used.
use aim_services::package::{State, parse::Platform, scan::Inputs, settings, write::Apks};

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
