//! First native system scan with integrity-verified original APKs. No original
//! image, parser feed or persisted PackageSettings is changed or consulted.
use aim_services::package::{
    parse::Platform,
    scan::{
        AbiPolicy, FirstBootSystemInputs, Image, LibraryCompatibility, NativeLibraryInstallPolicy,
        ScanClock, SigningError, SystemImageScan, UserPolicy,
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
    let scan = SystemImageScan::first_boot(image, &apks, &config, inputs(&domain_ids)).unwrap();
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
