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
    let driver = aim_binder_driver::Driver::new();
    let process = aim_binder_host::local::LocalProcess::open(
        &driver,
        aim_binder_driver::Device::Binder,
        aim_binder_driver::Credentials {
            pid: std::process::id() as i32,
            euid: 1000,
            security_context: None,
        },
    );
    let writable = fixture.0.join("writable-data");
    std::fs::create_dir_all(writable.join("app")).unwrap();
    let resources = aim_services::package::owner::resources::CodeResources::new(
        process,
        writable.clone(),
        None,
    );
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
    // Exercise a declared-key DTO independently of the unchanged APKs. Native
    // manifest parsing is tested separately; this covers scan decoder wiring.
    let mut declared = Image {
        packages: image.packages.clone(),
        rejected: Vec::new(),
    };
    let code = &mut declared.packages[1];
    let keys =
        aim_services::package::sign::serialize_public_keys(&code.signing.public_keys).unwrap();
    code.parsed.key_set_mapping = Some(vec![(
        Some("next".into()),
        Some(keys.into_iter().map(Some).collect()),
    )]);
    code.parsed.upgrade_key_sets = vec!["next".into()];
    let mut corrupt = Image {
        packages: declared.packages.clone(),
        rejected: Vec::new(),
    };
    corrupt.packages[1].parsed.key_set_mapping.as_mut().unwrap()[0]
        .1
        .as_mut()
        .unwrap()[0]
        .as_mut()
        .unwrap()
        .bytes
        .push(0);
    let declared =
        SystemImageScan::first_boot(declared, &apks, &config, inputs(&domain_ids)).unwrap();
    let keydata = &declared.packages[1].candidate.record.settings.key_set_data;
    assert_eq!(
        keydata.defined_key_sets,
        [("next".into(), keydata.proper_signing_key_set)]
    );
    assert_eq!(keydata.upgrade_key_sets, [keydata.proper_signing_key_set]);
    let failure = SystemImageScan::first_boot(corrupt, &apks, &config, inputs(&domain_ids))
        .err()
        .unwrap();
    let SigningError::Fatal(failure) = failure else {
        panic!("expected fatal keyset error")
    };
    assert_eq!(failure.phase, "keysets");
    assert!(failure.message.contains("noncanonical"));
    let source_times: Vec<_> = image
        .packages
        .iter()
        .map(|code| apks.scan_file_time(&code.parsed).unwrap())
        .collect();
    let mut scan = SystemImageScan::first_boot(image, &apks, &config, inputs(&domain_ids)).unwrap();
    assert!(scan.rejected.is_empty());
    assert_eq!(scan.owner.loaded_packages().len(), scan.packages.len());
    assert!(scan.owner.disabled_loaded_packages().is_empty());
    for completed in &scan.packages {
        let record = &completed.candidate.record;
        let loaded = &scan.owner.loaded_packages()[&record.settings.name];
        assert_eq!(loaded.collected_signing, record.signing);
        assert_eq!(
            loaded.facade_entry().unwrap().past_signing_certificates,
            record.signing.past_signing_certificates
        );
        assert_eq!(record.parsed.uid, record.settings.app_id);
        assert_eq!(
            record.parsed.signing_details,
            Some(record.signing.parcel_details().unwrap())
        );
        let parcel = record.parsed.to_cache_entry().unwrap();
        assert!(
            aim_services::package::pkg::AndroidPackage::read_cache_entry(&parcel.bytes).unwrap()
                == record.parsed
        );
        assert_eq!(
            scan.owner.loaded_packages()[&record.settings.name].package,
            record.parsed
        );
    }
    assert_eq!(
        scan.packages
            .iter()
            .map(|p| p.candidate.record.settings.name.as_str())
            .collect::<Vec<_>>(),
        ["android", "com.google.android.gsf"]
    );
    assert_eq!(scan.packages[0].candidate.record.settings.app_id, 1000);
    assert_eq!(scan.packages[1].candidate.record.settings.app_id, 10000);
    for package in &scan.packages {
        let id = package
            .candidate
            .record
            .settings
            .key_set_data
            .proper_signing_key_set;
        assert!(id > 0);
        let keys = &scan.owner.settings.key_sets;
        assert!(
            keys.key_sets
                .iter()
                .any(|(set, ids)| *set == id && !ids.is_empty())
        );
    }
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
    let full_users: std::collections::BTreeMap<_, _> = scan
        .packages
        .iter()
        .map(|p| {
            (
                p.candidate.record.settings.name.clone(),
                p.candidate.users.clone(),
            )
        })
        .collect();
    let empty_packages = std::collections::BTreeSet::new();
    let saved_inputs = || aim_services::package::scan::SavedSystemScanInputs {
        users: &full_users,
        first_boot_or_upgrade: false,
        old_stub_packages: &empty_packages,
        incremental_packages: &empty_packages,
        resources: &resources,
    };
    let baseline = scan.owner.settings.clone();
    let mut reboot = aim_services::package::scan::SigningScan::new(&config, &baseline, 36).unwrap();
    let batch = reboot
        .scan_saved_system_image(
            Image::load(&apks, &[]).unwrap(),
            &apks,
            &config,
            inputs(&domain_ids),
            saved_inputs(),
        )
        .unwrap();
    assert_eq!(batch.packages.len(), 2);
    assert!(batch.retained_data.is_empty());
    for (original, current) in scan.packages.iter().zip(&batch.packages) {
        let mut expected = original.candidate.record.settings.clone();
        expected.domain_set_id = current.candidate.record.settings.domain_set_id.clone();
        assert_eq!(expected, current.candidate.record.settings);
        assert_eq!(original.candidate.users, current.candidate.users);
    }
    // Stub policy comes from a fixture compressed-sibling inventory.
    // The original signed APK remains an unchanged symlink target.
    let stub = fixture.0.join("product/priv-app/GSF-Stub");
    std::fs::create_dir(&stub).unwrap();
    std::os::unix::fs::symlink(
        original.join("system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk"),
        stub.join("GSF.apk"),
    )
    .unwrap();
    std::fs::write(app.join("GSF.gz"), b"compressed inventory fixture").unwrap();
    let mut duplicate = Image::load(&apks, &[]).unwrap();
    let stub_at = duplicate
        .packages
        .iter()
        .position(|code| code.location.path.ends_with("-Stub"))
        .unwrap();
    duplicate.packages.swap(1, stub_at);
    let mut stub_settings = baseline.clone();
    stub_settings.packages[1].primary_cpu_abi = Some("arm64-v8a".into());
    let mut owner =
        aim_services::package::scan::SigningScan::new(&config, &stub_settings, 36).unwrap();
    let duplicate = owner
        .scan_saved_system_image(
            duplicate,
            &apks,
            &config,
            inputs(&domain_ids),
            saved_inputs(),
        )
        .unwrap();
    assert_eq!(duplicate.packages.len(), 2);
    assert_eq!(duplicate.rejected.len(), 1);
    assert!(
        duplicate.rejected[0]
            .reason
            .contains("INSTALL_FAILED_DUPLICATE_PACKAGE")
    );
    assert_eq!(
        duplicate.packages[1]
            .candidate
            .record
            .settings
            .primary_cpu_abi
            .as_deref(),
        Some("arm64-v8a")
    );
    assert!(
        duplicate.packages[1]
            .candidate
            .record
            .parsed
            .is2(aim_services::package::pkg::booleans2::STUB)
    );
    std::fs::remove_dir_all(stub).unwrap();
    std::fs::remove_file(app.join("GSF.gz")).unwrap();
    for keep_data in [true, false] {
        let mut settings = baseline.clone();
        let factory = settings.packages[1].clone();
        settings.disabled_system_packages.push(factory.clone());
        settings.packages[1].code_path = "/data/app/fixture-update".into();
        settings.packages[1].version_code += if keep_data { 1 } else { -1 };
        settings.packages[1].flags |= 1 << 7;
        let active = settings.packages[1].clone();
        let data_code = writable.join("app/fixture-update");
        std::fs::write(&data_code, b"disposable updated code").unwrap();
        let mut owner =
            aim_services::package::scan::SigningScan::new(&config, &settings, 36).unwrap();
        let batch = owner
            .scan_saved_system_image(
                Image::load(&apks, &[]).unwrap(),
                &apks,
                &config,
                inputs(&domain_ids),
                saved_inputs(),
            )
            .unwrap();
        if keep_data {
            assert_eq!(batch.packages.len(), 1);
            assert_eq!(batch.retained_data.len(), 1);
            assert_eq!(batch.retained_data[0].record.settings.name, factory.name);
            assert_eq!(owner.settings.packages[1], active);
            assert!(data_code.exists());
            let physical_data = fixture.0.join("data/app/fixture-update");
            std::fs::create_dir_all(&physical_data).unwrap();
            std::os::unix::fs::symlink(
                original.join(
                    "system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk",
                ),
                physical_data.join("base.apk"),
            )
            .unwrap();
            let inventory = aim_services::package::scan::DataImage::load(&apks, &[]).unwrap();
            assert_eq!(inventory.packages.len(), 1);
            let raw = &inventory.packages[0].code;
            let platform = &batch.packages[0].candidate.record.signing;
            let native_environment = NativeLibraryEnvironment {
                preferred_abi: "arm64-v8a",
                app_lib32_install_dir: "/data/app-lib",
                code_is_directory: true,
                canonical_source: None,
            };
            let data_inputs = || aim_services::package::scan::DataScanInputs {
                factory: Some(&batch.retained_data[0]),
                platform,
                vendor_sdk: 36,
                compatibility: &compatibility,
                remove_test_base: None,
                expecting_better: &empty_packages,
                new_domain_id: &domain_ids,
                completion: ScanMetadataCompletion {
                    abi_policy: &abi_policy,
                    native_environment: &native_environment,
                    context: AbiScanContext {
                        mode: AbiScanMode::Existing {
                            first_boot_or_upgrade: false,
                            old_was_stub: false,
                            saved: None,
                        },
                        // The data admission owner replaces these supplied flags.
                        system: false,
                        updated: false,
                        override_abi: None,
                        platform_runtime_64bit: None,
                    },
                    install: inputs(&domain_ids).install,
                    destination: None,
                    clock: inputs(&domain_ids).clock,
                    factory_test: false,
                },
            };
            let before = owner.clone();
            let copy_code = || aim_services::package::scan::Code {
                location: raw.location.clone(),
                parsed: raw.parsed.clone(),
                signing: raw.signing.clone(),
            };
            let mut unknown = copy_code();
            unknown.parsed.package_name = "unknown.data.package".into();
            unknown.parsed.manifest_package_name = Some("unknown.data.package".into());
            assert!(
                matches!(owner.scan_known_data(&unknown, &full_users, None, &apks, aim_services::package::scan::DataScanInputs { factory: None, ..data_inputs() }), Err(SigningError::Rejected(e)) if e.phase == "require-known")
            );
            assert_eq!(owner, before);
            let mut moved = copy_code();
            moved.location.path = "/data/app/unexpected".into();
            moved.parsed.path = Some(moved.location.path.clone());
            assert!(
                matches!(owner.scan_known_data(&moved, &full_users, None, &apks, data_inputs()), Err(SigningError::Rejected(e)) if e.phase == "require-known")
            );
            assert_eq!(owner, before);
            let mut wrong_signer = copy_code();
            wrong_signer.signing = platform.clone();
            assert!(
                owner
                    .scan_known_data(&wrong_signer, &full_users, None, &apks, data_inputs())
                    .is_err()
            );
            assert_eq!(owner, before);
            let mut stale = owner.clone();
            stale.settings.disabled_system_packages[0].version_code += 1;
            let stale_before = stale.clone();
            assert!(
                matches!(stale.scan_known_data(raw, &full_users, None, &apks, data_inputs()), Err(SigningError::Fatal(e)) if e.phase == "factory")
            );
            assert_eq!(stale, stale_before);
            use aim_services::package::scan::{DataCandidateOutcome, DataCode};
            let moved_candidate = DataCode {
                scan_path: moved.location.path.clone(),
                code: moved,
            };
            let invalid_file = writable.join("app/unexpected");
            std::fs::write(&invalid_file, b"disposable invalid code").unwrap();
            let failed_cleanup = owner.scan_data_candidate(
                &moved_candidate,
                &full_users,
                None,
                &apks,
                data_inputs(),
                &resources,
                true,
            );
            assert!(
                matches!(failed_cleanup, Err(SigningError::Fatal(e)) if e.phase == "data-cleanup")
            );
            assert!(invalid_file.exists());
            assert_eq!(owner, before);
            let removed = owner
                .scan_data_candidate(
                    &moved_candidate,
                    &full_users,
                    None,
                    &apks,
                    data_inputs(),
                    &resources,
                    false,
                )
                .unwrap();
            assert!(
                matches!(removed, DataCandidateOutcome::Removed(SigningError::Rejected(e)) if e.phase == "require-known")
            );
            assert!(!invalid_file.exists());
            assert_eq!(owner, before);
            let valid_candidate = &inventory.packages[0];
            let no_users = std::collections::BTreeMap::new();
            assert!(
                matches!(owner.scan_data_candidate(valid_candidate, &no_users, None, &apks, data_inputs(), &resources, false), Err(SigningError::Fatal(e)) if e.phase == "setting")
            );
            let domain_failure = || Err("domain owner unavailable".to_owned());
            assert!(
                matches!(owner.scan_data_candidate(valid_candidate, &full_users, None, &apks,
                aim_services::package::scan::DataScanInputs { new_domain_id: &domain_failure, ..data_inputs() }, &resources, false), Err(SigningError::Fatal(e)) if e.phase == "domain")
            );
            assert!(
                matches!(stale.scan_data_candidate(valid_candidate, &full_users, None, &apks, data_inputs(), &resources, false), Err(SigningError::Fatal(e)) if e.phase == "factory")
            );
            assert!(data_code.exists());
            assert_eq!(owner, before);
            assert_eq!(stale, stale_before);
            let mut expecting = before.clone();
            expecting
                .settings
                .packages
                .iter_mut()
                .find(|p| p.name == active.name)
                .unwrap()
                .code_path = factory.code_path.clone();
            let expecting_names = std::collections::BTreeSet::from([active.name.clone()]);
            let relaxed = expecting
                .scan_known_data(
                    raw,
                    &full_users,
                    None,
                    &apks,
                    aim_services::package::scan::DataScanInputs {
                        expecting_better: &expecting_names,
                        ..data_inputs()
                    },
                )
                .unwrap();
            assert_eq!(
                relaxed.candidate.record.settings.code_path,
                raw.location.path
            );
            assert_eq!(relaxed.candidate.record.settings.app_id, active.app_id);
            let mut ordinary = before.clone();
            ordinary.settings.disabled_system_packages.clear();
            let ordinary_data = ordinary
                .scan_known_data(
                    raw,
                    &full_users,
                    None,
                    &apks,
                    aim_services::package::scan::DataScanInputs {
                        factory: None,
                        ..data_inputs()
                    },
                )
                .unwrap();
            assert_eq!(
                ordinary_data.candidate.record.settings.flags & (1 | (1 << 7)),
                0
            );
            assert_eq!(ordinary_data.candidate.users, full_users[&active.name]);
            let DataCandidateOutcome::Accepted(accepted) = owner
                .scan_data_candidate(
                    valid_candidate,
                    &full_users,
                    None,
                    &apks,
                    data_inputs(),
                    &resources,
                    false,
                )
                .unwrap()
            else {
                panic!("valid updated data must be accepted")
            };
            assert_eq!(accepted.candidate.record.settings.app_id, active.app_id);
            assert_eq!(
                accepted.candidate.record.settings.code_path,
                active.code_path
            );
            assert_eq!(accepted.candidate.users, full_users[&active.name]);
            assert_ne!(accepted.candidate.record.settings.flags & 1, 0);
            assert_ne!(accepted.candidate.record.settings.flags & (1 << 7), 0);
            assert_ne!(accepted.candidate.record.settings.private_flags & 8, 0);
            assert_eq!(accepted.candidate.record.signing, raw.signing);
            assert!(accepted.copies.is_empty());
            assert!(physical_data.join("base.apk").exists());
            use aim_services::package::scan::{DataImage, DataImageScanInputs};
            let destinations = std::collections::BTreeMap::new();
            let non_incremental = |_: &str| Ok(false);
            let remove_test_base = |_: &aim_services::package::pkg::AndroidPackage| Ok(None);
            let loop_inputs = || DataImageScanInputs {
                factories: &batch,
                platform,
                vendor_sdk: 36,
                abi_policy: &abi_policy,
                compatibility: &compatibility,
                preferred_abi: "arm64-v8a",
                app_lib32_install_dir: "/data/app-lib",
                install: inputs(&domain_ids).install,
                clock: inputs(&domain_ids).clock,
                factory_test: false,
                users: &full_users,
                all_users: None,
                first_boot_or_upgrade: false,
                old_stub_packages: &empty_packages,
                expecting_better: &empty_packages,
                is_incremental: &non_incremental,
                remove_test_base: &remove_test_base,
                destinations: &destinations,
                resources: &resources,
                new_domain_id: &domain_ids,
            };
            let mut data_loop = before.clone();
            let complete = data_loop
                .scan_data_image(DataImage::load(&apks, &[]).unwrap(), &apks, loop_inputs())
                .unwrap();
            assert_eq!(complete.packages.len(), 1);
            assert!(complete.recovered.is_empty());
            assert!(complete.removed.is_empty());
            assert!(data_code.exists());
            let missing_factory = aim_services::package::scan::SystemImagePackages {
                packages: Vec::new(),
                retained_data: Vec::new(),
                retained_code: Vec::new(),
                rejected: Vec::new(),
            };
            let mut ex_system = before.clone();
            let mut unupdated = before.clone();
            unupdated.settings.disabled_system_packages.clear();
            let untouched = unupdated.clone();
            assert!(
                matches!(unupdated.scan_data_image(DataImage::load(&apks, &[]).unwrap(), &apks,
                DataImageScanInputs { factories: &missing_factory, ..loop_inputs() }),
                Err(SigningError::Fatal(e)) if e.phase == "package-data" && e.message.contains("resolved user"))
            );
            assert_eq!(unupdated, untouched);
            assert!(data_code.exists());
            let demoted = ex_system
                .scan_data_image(
                    DataImage::load(&apks, &[]).unwrap(),
                    &apks,
                    DataImageScanInputs {
                        factories: &missing_factory,
                        ..loop_inputs()
                    },
                )
                .unwrap();
            assert_eq!(demoted.packages.len(), 1);
            assert!(demoted.recovered.is_empty());
            assert!(ex_system.settings.disabled_system_packages.is_empty());
            let ordinary = &demoted.packages[0].candidate;
            assert_eq!(ordinary.record.settings.flags & (1 | (1 << 7)), 0);
            assert_eq!(
                ordinary.record.settings.private_flags,
                ordinary_data.candidate.record.settings.private_flags
            );
            assert_eq!(ordinary.record.settings.app_id, active.app_id);
            assert_eq!(ordinary.record.settings.code_path, active.code_path);
            assert_eq!(ordinary.users, full_users[&active.name]);
            assert!(data_code.exists());
            let calls = std::cell::Cell::new(0);
            let fail_rescan = |_: &aim_services::package::pkg::AndroidPackage| {
                calls.set(calls.get() + 1);
                if calls.get() == 2 {
                    Err("ex-system policy owner unavailable".into())
                } else {
                    Ok(None)
                }
            };
            let mut failed_demotion = before.clone();
            assert!(
                matches!(failed_demotion.scan_data_image(DataImage::load(&apks, &[]).unwrap(), &apks,
                DataImageScanInputs { factories: &missing_factory, remove_test_base: &fail_rescan, ..loop_inputs() }),
                Err(SigningError::Fatal(e)) if e.phase == "policy")
            );
            assert_eq!(calls.get(), 2);
            assert!(failed_demotion.settings.disabled_system_packages.is_empty());
            assert!(failed_demotion.scanned_user_states(&active.name).is_none());
            assert_eq!(
                failed_demotion
                    .settings
                    .packages
                    .iter()
                    .find(|p| p.name == active.name)
                    .unwrap()
                    .app_id,
                active.app_id
            );
            assert!(data_code.exists());
            let mut gone = before.clone();
            assert!(matches!(gone.scan_data_image(DataImage::default(), &apks,
                DataImageScanInputs { factories: &missing_factory, ..loop_inputs() }),
                Err(SigningError::Fatal(e)) if e.phase == "package-data"));
            assert!(gone.settings.disabled_system_packages.is_empty());
            assert!(gone.settings.packages.iter().any(|p| p.name == active.name));
            assert!(data_code.exists());
            let changed_path = fixture.0.join("data/app/other-path");
            std::fs::create_dir_all(&changed_path).unwrap();
            std::os::unix::fs::symlink(
                original.join(
                    "system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk",
                ),
                changed_path.join("base.apk"),
            )
            .unwrap();
            let mut changed_inventory = DataImage::load(&apks, &[]).unwrap();
            changed_inventory
                .packages
                .retain(|e| e.code.location.path == "/data/app/other-path");
            let mut changed = before.clone();
            let accepted_changed = changed
                .scan_data_image(changed_inventory, &apks, loop_inputs())
                .unwrap();
            assert_eq!(accepted_changed.packages.len(), 1);
            assert_eq!(
                accepted_changed.packages[0]
                    .candidate
                    .record
                    .settings
                    .code_path,
                "/data/app/other-path"
            );
            assert_eq!(
                accepted_changed.packages[0]
                    .candidate
                    .record
                    .settings
                    .app_id,
                active.app_id
            );
            assert_eq!(
                accepted_changed.packages[0].candidate.users,
                full_users[&active.name]
            );
            let duplicate_file = writable.join("app/other-path");
            std::fs::write(&duplicate_file, b"disposable duplicate code").unwrap();
            let mut duplicate_data = before.clone();
            let duplicate_result = duplicate_data
                .scan_data_image(DataImage::load(&apks, &[]).unwrap(), &apks, loop_inputs())
                .unwrap();
            assert_eq!(duplicate_result.packages.len(), 1);
            assert_eq!(duplicate_result.removed.len(), 1);
            assert!(
                matches!(&duplicate_result.removed[0].1, SigningError::Rejected(e) if e.phase == "validation" && e.message.contains("INSTALL_FAILED_DUPLICATE_PACKAGE"))
            );
            assert!(!duplicate_file.exists());
            assert!(data_code.exists());
            assert_eq!(
                duplicate_data
                    .settings
                    .packages
                    .iter()
                    .find(|p| p.name == active.name)
                    .unwrap()
                    .code_path,
                active.code_path
            );
            std::fs::remove_dir_all(changed_path).unwrap();
            let mut absent = before.clone();
            let recovered = absent
                .scan_data_image(DataImage::default(), &apks, loop_inputs())
                .unwrap();
            assert!(recovered.packages.is_empty());
            assert_eq!(recovered.recovered.len(), 1);
            assert_eq!(
                recovered.recovered[0].candidate.record.settings.code_path,
                factory.code_path
            );
            assert_eq!(
                recovered.recovered[0].candidate.record.settings.app_id,
                active.app_id
            );
            assert_eq!(
                recovered.recovered[0].candidate.record.settings.flags & (1 << 7),
                0
            );
            assert_eq!(
                recovered.recovered[0].candidate.users,
                full_users[&active.name]
            );
            assert!(absent.settings.disabled_system_packages.is_empty());
            // checkExistingBetterPackages enables/rescans; it performs no additional data code removal.
            assert!(data_code.exists());
            let attempt = std::cell::Cell::new(0);
            let fail_later = |_: &aim_services::package::pkg::AndroidPackage| {
                attempt.set(attempt.get() + 1);
                if attempt.get() == 2 {
                    Err("policy owner unavailable".to_owned())
                } else {
                    Ok(None)
                }
            };
            let repeated = DataImage {
                packages: vec![
                    DataCode {
                        scan_path: valid_candidate.scan_path.clone(),
                        code: raw.clone(),
                    },
                    DataCode {
                        scan_path: valid_candidate.scan_path.clone(),
                        code: raw.clone(),
                    },
                ],
                rejected: Vec::new(),
            };
            let mut partial = before.clone();
            assert!(
                matches!(partial.scan_data_image(repeated, &apks, DataImageScanInputs { remove_test_base: &fail_later, ..loop_inputs() }), Err(SigningError::Fatal(e)) if e.phase == "policy")
            );
            assert!(partial.scanned_user_states(&active.name).is_some());
            assert_eq!(
                partial.scanned_user_states(&active.name),
                Some(&full_users[&active.name])
            );
            assert_ne!(partial, before);
            assert!(data_code.exists());
            // An unchanged original signed APK with the wrong identity at this data
            // location is rejected, removed, then the selected GSF factory recovers.
            std::fs::remove_file(physical_data.join("base.apk")).unwrap();
            std::os::unix::fs::symlink(
                original.join("system/framework/framework-res.apk"),
                physical_data.join("base.apk"),
            )
            .unwrap();
            let mut invalid = before.clone();
            let recovered = invalid
                .scan_data_image(DataImage::load(&apks, &[]).unwrap(), &apks, loop_inputs())
                .unwrap();
            assert!(recovered.packages.is_empty());
            assert_eq!(recovered.removed.len(), 1);
            assert_eq!(recovered.recovered.len(), 1);
            assert!(!data_code.exists());
            assert_eq!(
                recovered.recovered[0].candidate.record.settings.code_path,
                factory.code_path
            );
            std::fs::write(&data_code, b"disposable retained code").unwrap();
            std::fs::remove_dir_all(physical_data).unwrap();
            std::fs::remove_file(data_code).unwrap();
        } else {
            assert_eq!(batch.packages.len(), 2);
            assert!(batch.retained_data.is_empty());
            assert!(!data_code.exists());
            assert_eq!(owner.settings.packages[1].code_path, factory.code_path);
            assert_eq!(owner.settings.packages[1].app_id, factory.app_id);
            assert_eq!(owner.settings.packages[1].flags & (1 << 7), 0);
            assert_eq!(batch.packages[1].candidate.users, full_users[&factory.name]);
            assert!(owner.settings.disabled_system_packages.is_empty());
        }
    }
    let mut missing_data = baseline.clone();
    missing_data
        .disabled_system_packages
        .push(missing_data.packages[1].clone());
    let previous_id = missing_data.packages[1].app_id;
    missing_data.packages.remove(1);
    let mut owner =
        aim_services::package::scan::SigningScan::new(&config, &missing_data, 36).unwrap();
    let batch = owner
        .scan_saved_system_image(
            Image::load(&apks, &[]).unwrap(),
            &apks,
            &config,
            inputs(&domain_ids),
            saved_inputs(),
        )
        .unwrap();
    assert_eq!(batch.packages.len(), 2);
    assert!(batch.retained_data.is_empty());
    assert!(owner.settings.disabled_system_packages.is_empty());
    assert_eq!(
        batch.packages[1].candidate.record.settings.app_id,
        previous_id
    );
    let mut non_system = baseline.clone();
    non_system.packages[1].flags &= !1;
    non_system.packages[1].code_path = "/data/app/fixture-update".into();
    let preserved = non_system.packages[1].clone();
    let mut owner =
        aim_services::package::scan::SigningScan::new(&config, &non_system, 36).unwrap();
    assert!(
        matches!(owner.scan_saved_system_image(Image::load(&apks, &[]).unwrap(), &apks,
        &config, inputs(&domain_ids), saved_inputs()), Err(SigningError::Rejected(ref e)) if e.phase == "system-source")
    );
    assert_eq!(owner.settings.packages[1], preserved);
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
    let mut mismatched_owner = scan.owner.clone();
    let mut mismatched = mismatched_owner
        .apply_existing(&code, update(), &saved_users, None, None)
        .unwrap();
    let before_finalization = mismatched_owner.clone();
    mismatched.record.parsed.signing_details = None;
    assert!(matches!(
        mismatched_owner.finish_scan_metadata(mismatched, &apks, completion()),
        Err(SigningError::Rejected(ref error)) if error.phase == "package-finalization"
    ));
    assert_eq!(mismatched_owner, before_finalization);
    let mut retained = scan
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
    assert_eq!(
        scan.owner.loaded_packages()[&saved.name].package,
        retained.candidate.record.parsed
    );
    assert_eq!(
        before.loaded_packages()[&saved.name].package,
        scan.packages[1].candidate.record.parsed
    );

    // The factory scan updates only the disabled copy while a data update is
    // active. It neither reconciles signatures nor admits a live group member.
    let mut state = scan.owner.settings.clone();
    let active = state
        .packages
        .iter_mut()
        .find(|p| p.name == saved.name)
        .unwrap();
    active.code_path = "/data/app/fixture-update".into();
    active.flags |= 1 << 7;
    state.disabled_system_packages.push(saved.clone());
    let mut factory_owner =
        aim_services::package::scan::SigningScan::new(&config, &state, 36).unwrap();
    let mut factory_code = Image::load(&apks, &[]).unwrap().packages.remove(1);
    aim_services::package::scan::ScanPolicy::for_location(&factory_code.location)
        .apply(
            &mut factory_code.parsed,
            &factory_code.signing,
            Some(&scan.packages[0].candidate.record.signing),
            true,
            &apks,
            &compatibility,
            None,
        )
        .unwrap();
    let factory_before = factory_owner.clone();
    assert!(matches!(
        factory_owner.scan_disabled_system(&factory_code, update(), None, &unreadable, completion()),
        Err(SigningError::Rejected(ref error)) if error.phase == "code-time"
    ));
    assert_eq!(factory_owner, factory_before);
    let factory = factory_owner
        .scan_disabled_system(&factory_code, update(), None, &apks, completion())
        .unwrap();
    assert_eq!(
        factory.record.settings.domain_set_id.as_deref(),
        Some("63636363-6363-6363-6363-636363636363")
    );
    assert_eq!(factory.record.settings.signatures, saved.signatures);
    assert_eq!(
        factory_owner.disabled_loaded_packages()[&saved.name].package,
        factory.record.parsed
    );
    assert_eq!(
        factory_owner.loaded_packages(),
        factory_before.loaded_packages()
    );
    assert_eq!(factory.users[&0].first_install_time, -1);
    assert_eq!(factory.record.settings.last_update_time, -1);
    assert_eq!(
        factory_owner.disabled_user_states(&saved.name),
        Some(&factory.users)
    );
    assert_ne!(factory.users, saved_users[&saved.name]);
    assert_eq!(
        factory_owner.settings.packages,
        factory_before.settings.packages
    );
    assert_eq!(factory_owner.identities, factory_before.identities);
    assert_eq!(factory_owner.libraries, factory_before.libraries);
    let mut expected = factory_before;
    expected.settings.disabled_system_packages[0] = factory.record.settings;
    assert_eq!(factory_owner.settings, expected.settings);

    let mut hot_owner = scan.owner.clone();
    hot_owner
        .settings
        .disabled_system_packages
        .push(retained.candidate.record.settings.clone());
    hot_owner.copy_disabled_user_states(&retained).unwrap();
    assert!(std::sync::Arc::ptr_eq(
        &hot_owner.loaded_packages()[&saved.name],
        &hot_owner.disabled_loaded_packages()[&saved.name],
    ));
    let frozen = hot_owner.clone();
    let original_version = retained.candidate.record.parsed.version_name.clone();
    retained.candidate.record.parsed.version_name = Some("detached-stale-copy".into());
    assert_eq!(
        hot_owner.loaded_packages()[&saved.name].package,
        frozen.loaded_packages()[&saved.name].package
    );
    assert!(hot_owner.copy_disabled_user_states(&retained).is_err());
    assert_eq!(hot_owner, frozen);
    retained.candidate.record.parsed.version_name = original_version;
    assert_eq!(
        hot_owner.disabled_loaded_packages()[&saved.name].package,
        retained.candidate.record.parsed
    );
    assert_eq!(
        hot_owner.disabled_user_states(&saved.name),
        Some(&retained.candidate.users)
    );
    let hot_saved = retained.candidate.record.settings.clone();
    let mut hot_inputs = completion();
    hot_inputs.context.mode = AbiScanMode::Existing {
        first_boot_or_upgrade: false,
        old_was_stub: false,
        saved: Some(&hot_saved),
    };
    let hot = hot_owner
        .scan_disabled_system(&factory_code, update(), None, &apks, hot_inputs)
        .unwrap();
    assert_eq!(hot.users, retained.candidate.users);
    assert_eq!(
        hot.record.settings.last_update_time,
        hot_saved.last_update_time
    );
    let mut changed_users = saved_users.clone();
    changed_users
        .get_mut(&saved.name)
        .unwrap()
        .get_mut(&0)
        .unwrap()
        .first_install_time = 456;
    changed_users.get_mut(&saved.name).unwrap().insert(
        10,
        aim_services::package::restrictions::UserState {
            first_install_time: 789,
            ..Default::default()
        },
    );
    let mut hot_inputs = completion();
    hot_inputs.context.mode = AbiScanMode::Existing {
        first_boot_or_upgrade: false,
        old_was_stub: false,
        saved: Some(&hot_saved),
    };
    let changed = hot_owner
        .scan_existing(
            &factory_code,
            update(),
            &changed_users,
            None,
            Some(&hot.record),
            &apks,
            hot_inputs,
        )
        .unwrap();
    assert_eq!(changed.candidate.users[&0].first_install_time, 456);
    assert_eq!(
        hot_owner.scanned_user_states(&saved.name),
        Some(&changed.candidate.users)
    );
    assert_eq!(
        hot_owner.disabled_user_states(&saved.name).unwrap()[&0].first_install_time,
        456
    );
    assert!(
        !hot_owner
            .disabled_user_states(&saved.name)
            .unwrap()
            .contains_key(&10)
    );
    hot_owner.settings.disabled_system_packages[0].version_code += 1;
    let before = hot_owner.clone();
    assert!(hot_owner.copy_disabled_user_states(&retained).is_err());
    assert_eq!(hot_owner, before);

    let mut stale_factory = saved.clone();
    stale_factory.signatures = scan.packages[0]
        .candidate
        .record
        .settings
        .signatures
        .clone();
    assert_ne!(stale_factory.signatures, saved.signatures);
    let selection_inputs = || {
        let mut inputs = completion();
        inputs.context.mode = AbiScanMode::Existing {
            first_boot_or_upgrade: false,
            old_was_stub: false,
            saved: Some(&stale_factory),
        };
        inputs
    };
    let raw_factory = Image::load(&apks, &[]).unwrap().packages.remove(1);
    let restore_inputs = || aim_services::package::scan::UpdatedSystemBootInputs {
        completion: completion(),
        compatibility: &compatibility,
        platform: Some(&scan.packages[0].candidate.record.signing),
        vendor_sdk: 36,
        remove_test_base: None,
        resources: &resources,
        incremental: false,
        new_domain_id: &domain_ids,
    };
    use aim_services::package::scan::UpdatedSystemSource::{KeepData, RestoreFactory};
    for strict in [false, true] {
        let mut policy = SystemConfig::default();
        if strict {
            policy
                .preinstall_packages_with_strict_signature_check
                .insert(saved.name.clone());
        }
        for (version, same_path, source) in [
            (saved.version_code + 1, false, KeepData),
            (saved.version_code, false, KeepData),
            (saved.version_code - 1, false, RestoreFactory),
            (saved.version_code - 1, true, KeepData),
        ] {
            let mut settings = state.clone();
            settings.disabled_system_packages[0] = stale_factory.clone();
            let active = settings
                .packages
                .iter_mut()
                .find(|p| p.name == saved.name)
                .unwrap();
            active.version_code = version;
            if same_path {
                active.code_path.clone_from(&saved.code_path);
            }
            let mut owner =
                aim_services::package::scan::SigningScan::new(&config, &settings, 36).unwrap();
            let before = owner.clone();
            assert!(matches!(owner.scan_updated_system(&factory_code, update(),
                None, &policy, &unreadable, selection_inputs()),
                Err(SigningError::Rejected(ref error)) if error.phase == "code-time"));
            assert_eq!(owner, before);
            let selected = owner
                .scan_updated_system(
                    &factory_code,
                    update(),
                    None,
                    &policy,
                    &apks,
                    selection_inputs(),
                )
                .unwrap();
            assert_eq!(selected.source, source);
            assert_eq!(
                selected.factory.record.settings.signatures,
                if strict && source == KeepData {
                    saved.signatures.clone()
                } else {
                    stale_factory.signatures.clone()
                }
            );
            if source == KeepData {
                let mut data_location = factory_code.location.clone();
                data_location.path = "/data/app/fixture-update".into();
                let data_code = aim_services::package::scan::Code {
                    location: data_location,
                    parsed: factory_code.parsed.clone(),
                    signing: factory_code.signing.clone(),
                };
                let mut data_update = update();
                data_update.code_path.clone_from(&data_code.location.path);
                let mut authorization_owner = owner.clone();
                let result = authorization_owner.apply_existing(
                    &data_code,
                    data_update,
                    &saved_users,
                    None,
                    Some(&selected.factory.record),
                );
                if strict {
                    assert!(result.is_ok());
                } else {
                    assert!(
                        matches!(result, Err(SigningError::Rejected(ref error)) if error.phase == "authorization")
                    );
                    assert_eq!(authorization_owner, owner);
                }
            }
            let mut expected = before;
            expected.settings.disabled_system_packages[0] =
                selected.factory.record.settings.clone();
            assert_eq!(owner.settings, expected.settings);
            assert_eq!(owner.identities, expected.identities);
            assert_eq!(owner.libraries, expected.libraries);
            assert_eq!(
                owner.disabled_user_states(&saved.name),
                Some(&selected.factory.users)
            );
            assert_eq!(selected.factory.users[&0].first_install_time, -1);
            if source == KeepData {
                let before = owner.clone();
                let next_domain = next_id.load(Ordering::SeqCst);
                assert!(matches!(
                    owner
                        .complete_updated_system_boot(
                            &selected,
                            &raw_factory,
                            &saved_users,
                            None,
                            &apks,
                            restore_inputs()
                        )
                        .unwrap(),
                    aim_services::package::scan::UpdatedSystemBootOutcome::KeepData
                ));
                assert_eq!(owner, before);
                assert_eq!(next_id.load(Ordering::SeqCst), next_domain);
            }
            if source == RestoreFactory {
                let active = owner
                    .settings
                    .packages
                    .iter()
                    .find(|p| p.name == saved.name)
                    .unwrap()
                    .clone();
                let ids = owner.identities.clone();
                let disposable_code = writable.join("app/fixture-update");
                std::fs::write(&disposable_code, b"disposable replaced code").unwrap();
                let mut stale = owner.clone();
                stale
                    .settings
                    .packages
                    .iter_mut()
                    .find(|p| p.name == saved.name)
                    .unwrap()
                    .version_code += 1;
                let before = stale.clone();
                assert!(
                    stale
                        .restore_updated_system_setting(&selected, &resources, false, [98; 16])
                        .is_err()
                );
                assert_eq!(stale, before);
                assert!(disposable_code.exists());
                let before = owner.clone();
                assert!(
                    owner
                        .restore_updated_system_setting(&selected, &resources, true, [98; 16])
                        .is_err()
                );
                assert_eq!(owner, before);
                assert!(disposable_code.exists());
                let before = owner.clone();
                let domain_before = next_id.load(Ordering::SeqCst);
                let mut incremental_inputs = restore_inputs();
                incremental_inputs.incremental = true;
                assert!(
                    owner
                        .complete_updated_system_boot(
                            &selected,
                            &raw_factory,
                            &saved_users,
                            None,
                            &apks,
                            incremental_inputs
                        )
                        .is_err()
                );
                assert_eq!(owner, before);
                assert_eq!(next_id.load(Ordering::SeqCst), domain_before);
                assert!(disposable_code.exists());
                let mut other_code = aim_services::package::scan::Code {
                    location: raw_factory.location.clone(),
                    parsed: raw_factory.parsed.clone(),
                    signing: raw_factory.signing.clone(),
                };
                other_code.parsed.version_code += 1;
                assert!(
                    owner
                        .complete_updated_system_boot(
                            &selected,
                            &other_code,
                            &saved_users,
                            None,
                            &apks,
                            restore_inputs()
                        )
                        .is_err()
                );
                assert_eq!(owner, before);
                assert!(disposable_code.exists());
                let mut failed = owner.clone();
                assert!(
                    failed
                        .complete_updated_system_boot(
                            &selected,
                            &raw_factory,
                            &saved_users,
                            None,
                            &unreadable,
                            restore_inputs()
                        )
                        .is_err()
                );
                assert!(!disposable_code.exists());
                assert!(failed.settings.disabled_system_packages.is_empty());
                let after_cleanup = failed
                    .settings
                    .packages
                    .iter()
                    .find(|p| p.name == saved.name)
                    .unwrap();
                assert_eq!(after_cleanup.code_path, active.code_path);
                assert_eq!(after_cleanup.version_code, saved.version_code);
                assert_eq!(failed.identities, ids);
                assert_eq!(failed.libraries, owner.libraries);
                std::fs::write(&disposable_code, b"disposable replaced code").unwrap();
                let result = owner
                    .complete_updated_system_boot(
                        &selected,
                        &raw_factory,
                        &saved_users,
                        None,
                        &apks,
                        restore_inputs(),
                    )
                    .unwrap();
                let aim_services::package::scan::UpdatedSystemBootOutcome::Factory(restored) =
                    result
                else {
                    panic!("factory restoration retained data");
                };
                assert!(!disposable_code.exists());
                assert_eq!(owner.identities.ids, ids.ids);
                assert_eq!(
                    restored.candidate.record.settings.code_path,
                    saved.code_path
                );
                assert_eq!(restored.candidate.record.settings.app_id, saved.app_id);
                assert_eq!(restored.candidate.record.settings.flags & (1 << 7), 0);
                assert_eq!(restored.candidate.users, saved_users[&saved.name]);
                assert!(owner.settings.disabled_system_packages.is_empty());
            }
        }
    }

    // Missing data settings must not leave the factory marked as updated.
    let mut fresh_code = Image::load(&apks, &[]).unwrap().packages.remove(1);
    aim_services::package::scan::ScanPolicy::for_location(&fresh_code.location)
        .apply(
            &mut fresh_code.parsed,
            &fresh_code.signing,
            Some(&scan.packages[0].candidate.record.signing),
            false,
            &apks,
            &compatibility,
            None,
        )
        .unwrap();
    let mut orphan = state.clone();
    orphan.packages.retain(|p| p.name != saved.name);
    let mut recovered =
        aim_services::package::scan::SigningScan::new(&config, &orphan, 36).unwrap();
    let before_ids = recovered.identities.ids.clone();
    let metadata = || aim_services::package::scan::SettingMetadata {
        code_path: fresh_code.location.path.clone(),
        legacy_native_library_path: None,
        primary_cpu_abi: None,
        secondary_cpu_abi: None,
        version_code: saved.version_code,
        flags: saved.flags,
        private_flags: saved.private_flags,
        last_modified_time: 0,
        uses_sdk_libraries: saved.uses_sdk_libraries.clone(),
        uses_static_libraries: saved.uses_static_libraries.clone(),
        mime_groups: fresh_code.parsed.mime_groups.clone(),
        domain_set_id: [77; 16],
        target_sdk_version: fresh_code.parsed.target_sdk_version,
        restrict_update_hash: fresh_code.parsed.restrict_update_hash.clone(),
    };
    let recovery_inputs = || {
        let mut inputs = completion();
        inputs.context.mode = AbiScanMode::Existing {
            first_boot_or_upgrade: true,
            old_was_stub: false,
            saved: None,
        };
        inputs
    };
    assert!(
        recovered
            .scan_new_system(
                &fresh_code,
                metadata(),
                inputs(&domain_ids).users,
                &unreadable,
                recovery_inputs()
            )
            .is_err()
    );
    assert!(recovered.settings.disabled_system_packages.is_empty());
    assert!(recovered.disabled_user_states(&saved.name).is_none());
    assert!(
        !recovered
            .settings
            .packages
            .iter()
            .any(|p| p.name == saved.name)
    );
    assert_eq!(recovered.identities.ids, before_ids);
    let restored = recovered
        .scan_new_system(
            &fresh_code,
            metadata(),
            inputs(&domain_ids).users,
            &apks,
            recovery_inputs(),
        )
        .unwrap();
    assert_eq!(restored.candidate.record.settings.app_id, saved.app_id);
    assert_eq!(restored.candidate.record.settings.flags & (1 << 7), 0);
    assert!(recovered.settings.disabled_system_packages.is_empty());
    assert!(restored.candidate.users[&0].first_install_time > 0);
    let mut live = aim_services::package::scan::SigningScan::new(&config, &state, 36).unwrap();
    let before = live.clone();
    assert!(!live.remove_stale_disabled_system(&fresh_code).unwrap());
    assert_eq!(live, before);
    let mut invalid_location = fresh_code.location.clone();
    invalid_location.path = "/data/app/fixture-update".into();
    let invalid_code = aim_services::package::scan::Code {
        location: invalid_location,
        parsed: fresh_code.parsed.clone(),
        signing: fresh_code.signing.clone(),
    };
    assert!(live.remove_stale_disabled_system(&invalid_code).is_err());
    assert_eq!(live, before);

    // Original-name adoption is an existing package, not stale recovery.
    let mut adoption_code = aim_services::package::scan::Code {
        location: fresh_code.location.clone(),
        parsed: fresh_code.parsed.clone(),
        signing: fresh_code.signing.clone(),
    };
    adoption_code.parsed.package_name = "fixture.incoming".into();
    adoption_code.parsed.manifest_package_name = Some("fixture.incoming".into());
    adoption_code.parsed.original_packages = Some(vec![Some(saved.name.clone())]);
    let mut adoption_settings = state.clone();
    adoption_settings.disabled_system_packages[0].name = "fixture.incoming".into();
    let mut adoption =
        aim_services::package::scan::SigningScan::new(&config, &adoption_settings, 36).unwrap();
    let before = adoption.clone();
    assert!(
        !adoption
            .remove_stale_disabled_system(&adoption_code)
            .unwrap()
    );
    assert!(
        matches!(adoption.apply_new_system(&adoption_code, metadata(), inputs(&domain_ids).users),
        Err(SigningError::Rejected(ref error)) if error.phase == "identity" && error.message.contains("eligible original"))
    );
    assert_eq!(adoption, before);

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
