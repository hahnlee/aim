//! First native system scan with integrity-verified original APKs. No original
//! image, parser feed or persisted PackageSettings is changed or consulted.
use aim_services::package::{
    parse::Platform,
    scan::{
        AbiPolicy, AbiScanContext, AbiScanMode, FirstBootSystemInputs, Image, LibraryCompatibility,
        NativeLibraryEnvironment, NativeLibraryInstallPolicy, ScanClock, ScanMetadataCompletion,
        SettingUpdate, SigningError, SystemImageScan, UserPolicy,
    },
    system_config::SystemConfig,
    write::Apks,
};
use std::sync::atomic::{AtomicU8, Ordering};

struct Fixture(std::path::PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn first_system_scan_applies_ordered_policy_uid_and_final_metadata() {
    let original = aim_paths::original_image();
    let dir = std::env::temp_dir().join(format!("aim-first-system-scan-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let fixture = Fixture(dir);
    let framework = fixture.0.join("system/framework");
    let app = fixture.0.join("product/priv-app/GSF");
    std::fs::create_dir_all(&framework).unwrap();
    std::fs::create_dir_all(&app).unwrap();
    std::os::unix::fs::symlink(
        original.join("system/framework/framework-res.apk"),
        framework.join("framework-res.apk"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        original.join("system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk"),
        app.join("GSF.apk"),
    )
    .unwrap();
    let root = fixture.0.clone();
    let apks = Apks {
        files: Box::new(move |path| Some(root.join(path.trim_start_matches('/')))),
        platform: Platform::load(&original, Default::default()).unwrap(),
    };
    let config = SystemConfig::default();
    let compatibility = LibraryCompatibility::new(&config, &|_| None, true).unwrap();
    let abi_policy = AbiPolicy {
        all: vec!["arm64-v8a".into()],
        bit32: vec![],
        bit64: vec!["arm64-v8a".into()],
        native32: vec![],
        native64: vec!["arm64-v8a".into()],
        force_multi_arch_match: false,
    };
    let next_id = AtomicU8::new(1);
    let domain_ids = || Ok([next_id.fetch_add(1, Ordering::SeqCst); 16]);
    let apex_settings = Default::default();
    let inputs = |new_domain_id| FirstBootSystemInputs {
        apex_settings: &apex_settings,
        first_api_level: 36,
        vendor_sdk: 36,
        abi_policy: &abi_policy,
        compatibility: &compatibility,
        preferred_abi: "arm64-v8a",
        app_lib32_install_dir: "/data/app-lib",
        platform_runtime_64bit: true,
        install: NativeLibraryInstallPolicy {
            page_size: 4096,
            extract: false,
            debuggable: false,
            compat_16kb_disabled: false,
            manifest_compat_disabled: false,
        },
        clock: ScanClock {
            current_time: 0,
            user_id: 0,
            update_time: false,
        },
        factory_test: false,
        users: UserPolicy {
            install_user: Some(0),
            users: None,
            allow_install: true,
            instant_app: false,
            virtual_preload: false,
            stopped_system_app: false,
        },
        new_domain_id,
    };
    let image = Image::load(&apks, &[]).unwrap();
    let source_times: Vec<_> = image
        .packages
        .iter()
        .map(|code| apks.scan_file_time(&code.parsed).unwrap())
        .collect();
    let mut scan = SystemImageScan::first_boot(image, &apks, &config, inputs(&domain_ids)).unwrap();
    assert!(scan.rejected.is_empty());
    assert_eq!(
        scan.packages
            .iter()
            .map(|p| p.candidate.record.settings.name.as_str())
            .collect::<Vec<_>>(),
        ["android", "com.google.android.gsf"]
    );
    assert_eq!(scan.packages[0].candidate.record.settings.app_id, 1000);
    assert_eq!(scan.packages[1].candidate.record.settings.app_id, 10000);
    assert_eq!(
        scan.packages[0]
            .candidate
            .record
            .settings
            .primary_cpu_abi
            .as_deref(),
        Some("arm64-v8a")
    );
    for (at, package) in scan.packages.iter().enumerate() {
        let record = &package.candidate.record;
        assert_eq!(record.settings.last_modified_time, source_times[at]);
        assert_eq!(record.settings.last_update_time, source_times[at]);
        assert_ne!(record.settings.flags & 1, 0);
        assert_ne!(record.settings.private_flags & 8, 0);
        let group =
            &scan.owner.identities.shared_users[record.parsed.shared_user_id.as_ref().unwrap()];
        assert_eq!(
            group.flags,
            record.settings.flags | if at == 0 { 1 } else { 0 }
        );
        assert_eq!(
            group.private_flags,
            record.settings.private_flags | if at == 0 { 8 } else { 0 }
        );
        assert!(
            record
                .settings
                .signatures
                .as_ref()
                .unwrap()
                .signatures
                .len()
                > 0
        );
        assert!(package.copies.is_empty());
    }
    // A retained scan changes its domain setting before the code-time gate.
    // Failure there must roll back every owner, including shared membership.
    let prior = &scan.packages[1].candidate;
    let saved = prior.record.settings.clone();
    let saved_users = std::collections::BTreeMap::from([(saved.name.clone(), prior.users.clone())]);
    let mut code = Image::load(&apks, &[]).unwrap().packages.remove(1);
    code.parsed = prior.record.parsed.clone();
    let update = || SettingUpdate {
        code_path: saved.code_path.clone(),
        legacy_native_library_path: saved.legacy_native_library_path.clone(),
        primary_cpu_abi: saved.primary_cpu_abi.clone(),
        secondary_cpu_abi: saved.secondary_cpu_abi.clone(),
        flags: saved.flags,
        private_flags: saved.private_flags,
        uses_sdk_libraries: saved.uses_sdk_libraries.clone(),
        uses_static_libraries: saved.uses_static_libraries.clone(),
        mime_groups: code.parsed.mime_groups.clone(),
        domain_set_id: [99; 16],
        target_sdk_version: saved.target_sdk_version,
        restrict_update_hash: saved.restrict_update_hash.clone(),
    };
    let environment = NativeLibraryEnvironment {
        preferred_abi: "arm64-v8a",
        app_lib32_install_dir: "/data/app-lib",
        code_is_directory: true,
        canonical_source: None,
    };
    let completion = || ScanMetadataCompletion {
        abi_policy: &abi_policy,
        native_environment: &environment,
        context: AbiScanContext {
            mode: AbiScanMode::Existing {
                first_boot_or_upgrade: false,
                old_was_stub: false,
                saved: Some(&saved),
            },
            system: true,
            updated: false,
            override_abi: None,
            platform_runtime_64bit: None,
        },
        install: inputs(&domain_ids).install,
        destination: None,
        clock: inputs(&domain_ids).clock,
        factory_test: false,
    };
    let unreadable = Apks {
        files: Box::new(|_| None),
        platform: Platform::load(&original, Default::default()).unwrap(),
    };
    let before = scan.owner.clone();
    assert!(matches!(
        scan.owner.scan_existing(&code, update(), &saved_users, None, None, &unreadable, completion()),
        Err(SigningError::Rejected(ref error)) if error.phase == "code-time"
    ));
    assert_eq!(scan.owner, before);
    let retained = scan
        .owner
        .scan_existing(
            &code,
            update(),
            &saved_users,
            None,
            None,
            &apks,
            completion(),
        )
        .unwrap();
    assert_eq!(
        retained.candidate.record.settings.domain_set_id.as_deref(),
        Some("63636363-6363-6363-6363-636363636363")
    );
    assert_eq!(retained.candidate.record.settings.app_id, saved.app_id);
    assert_eq!(retained.candidate.users, saved_users[&saved.name]);
    assert_eq!(scan.owner.identities, before.identities);
    assert_eq!(scan.owner.libraries, before.libraries);
    assert!(retained.copies.is_empty());

    let reserved = aim_services::package::settings::Settings {
        packages: vec![aim_services::package::settings::Package {
            name: "fixture.apex.module".into(),
            code_path: "/system/apex/fixture.apex".into(),
            app_id: 10000,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut with_apex = inputs(&domain_ids);
    with_apex.apex_settings = &reserved;
    let scan =
        SystemImageScan::first_boot(Image::load(&apks, &[]).unwrap(), &apks, &config, with_apex)
            .unwrap();
    assert_eq!(scan.packages[1].candidate.record.settings.app_id, 10001);
    assert_eq!(scan.owner.settings.packages[0], reserved.packages[0]);
    let mut invalid_seed = reserved.clone();
    invalid_seed.packages[0].code_path = "/data/app/fixture".into();
    let mut inputs_with_apk = inputs(&domain_ids);
    inputs_with_apk.apex_settings = &invalid_seed;
    assert!(
        matches!(SystemImageScan::first_boot(Image::load(&apks, &[]).unwrap(), &apks, &config, inputs_with_apk), Err(SigningError::Rejected(ref error)) if error.phase == "apex")
    );

    let mut early = Image::load(&apks, &[]).unwrap();
    let mut overlay = early.packages.remove(1);
    overlay.location.kind = aim_services::package::scan::Kind::Overlay;
    overlay.parsed.shared_user_id = Some("android.uid.system".into());
    early.packages.insert(0, overlay);
    assert!(
        matches!(SystemImageScan::first_boot(early, &apks, &config, inputs(&domain_ids)), Err(SigningError::Rejected(ref error)) if error.phase == "policy" && error.message.contains("scanned platform"))
    );
    assert!(
        apks.platform
            .framework_boolean("fixture_missing_boolean")
            .is_err()
    );

    let fail_domain = || Err("domain owner failure".into());
    assert!(
        matches!(SystemImageScan::first_boot(Image::load(&apks, &[]).unwrap(), &apks, &config, inputs(&fail_domain)), Err(SigningError::Rejected(ref error)) if error.phase == "domain")
    );
    let mut image = Image::load(&apks, &[]).unwrap();
    image
        .packages
        .retain(|code| code.parsed.package_name != "android");
    assert!(
        matches!(SystemImageScan::first_boot(image, &apks, &config, inputs(&domain_ids)), Err(SigningError::Rejected(ref error)) if error.phase == "framework")
    );
    assert_eq!(std::fs::read_dir(&framework).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(&app).unwrap().count(), 1);
}
