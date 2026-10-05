//! First native system scan with integrity-verified original APKs. No original
//! image, parser feed or persisted PackageSettings is changed or consulted.
mod common {
    pub mod seinfo;
}

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
    let apex_image = Default::default();
    let notified_apex = std::cell::Cell::new(0);
    let notify_apex = |results: &[aim_services::package::scan::ApexScanResult]| {
        for result in results {
            assert_eq!(result.package.uid, -1);
            assert!(
                result
                    .package
                    .is2(aim_services::package::pkg::booleans2::APEX)
            );
        }
        notified_apex.set(results.len());
        Ok(())
    };
    let inputs = |new_domain_id| FirstBootSystemInputs {
        certificates: Default::default(),
        seinfo: common::seinfo::scan(),
        apex_image: &apex_image,
        notify_apex_scan: &notify_apex,
        first_api_level: 36,
        vendor_sdk: 36,
        shared_uid_migration: aim_services::package::scan::SharedUidMigration::NewInstallOnly,
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
        SystemImageScan::first_boot(|| Ok(declared), &apks, &config, inputs(&domain_ids)).unwrap();
    let keydata = &declared.packages[1].candidate.record.settings.key_set_data;
    assert_eq!(
        keydata.defined_key_sets,
        [(Some("next".into()), keydata.proper_signing_key_set)]
    );
    assert_eq!(keydata.upgrade_key_sets, [keydata.proper_signing_key_set]);
    let failure = SystemImageScan::first_boot(|| Ok(corrupt), &apks, &config, inputs(&domain_ids))
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
    let mut scan = SystemImageScan::first_boot_parsed(
        || Image::parse(&apks, &[]),
        &apks,
        &config,
        inputs(&domain_ids),
    )
    .unwrap();
    assert!(scan.rejected.is_empty());
    scan.owner.fix_shared_seinfo_target_sdks_at_boot().unwrap();
    for group in scan.owner.identities.shared_users.values() {
        let targets = scan
            .owner
            .settings
            .packages
            .iter()
            .filter(|p| p.shared_user && p.app_id == group.app_id)
            .filter_map(|p| scan.owner.loaded_packages().get(&p.name))
            .map(|code| code.package.target_sdk_version);
        let expected = targets.fold(10000, i32::min);
        assert_eq!(group.seinfo_target_sdk(), expected);
    }
    let mut usage = aim_services::package::owner::usage::Usage::new(
        scan.owner.settings.packages.iter().map(|p| p.name.as_str()),
    );
    usage.notify("android", 0, 17);
    let captures =
        aim_services::package::scan_snapshot::Store::new(scan.owner.clone(), usage).unwrap();
    let original_capture = captures.capture();
    assert_eq!(original_capture.version(), 1);
    // First native settings write starts with no package inventory and retains
    // unrelated complete XML. The immutable captured scan is its sole source.
    let fresh_data = writable.join("fresh-owner");
    let mut fresh = aim_services::package::owner::Store::create(&fresh_data, &[0]).unwrap();
    assert!(!fresh_data.join("system/packages.xml").exists());
    fresh.commit_scan_settings(&original_capture).unwrap();
    let fresh_reopened = aim_services::package::owner::Store::open(&fresh_data, &[0])
        .unwrap()
        .unwrap();
    assert_eq!(fresh.state().settings, fresh_reopened.state().settings);
    let settings_path = writable.join("system/packages.xml");
    std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
    std::fs::write(
        &settings_path,
        b"<packages><vendor-extension keep='true'/></packages>",
    )
    .unwrap();
    let mut persistence = aim_services::package::owner::Store::open(&writable, &[0])
        .unwrap()
        .unwrap();
    persistence.commit_scan_settings(&original_capture).unwrap();
    let reopened = aim_services::package::owner::Store::open(&writable, &[0])
        .unwrap()
        .unwrap();
    assert_eq!(reopened.state().settings, persistence.state().settings);
    assert_eq!(
        reopened.state().settings.packages.len(),
        scan.packages.len()
    );
    for actual in &reopened.state().settings.packages {
        let expected = &scan
            .owner
            .settings
            .packages
            .iter()
            .find(|p| p.name == actual.name)
            .unwrap();
        assert_eq!(actual.app_id, expected.app_id);
        assert_eq!(actual.code_path, expected.code_path);
        assert_eq!(actual.flags, expected.flags);
        assert_eq!(actual.private_flags, expected.private_flags);
        assert_eq!(actual.domain_set_id, expected.domain_set_id);
        assert_eq!(
            actual.signatures.as_ref().unwrap().signatures,
            expected.signatures.as_ref().unwrap().signatures
        );
        assert_eq!(actual.key_set_data, expected.key_set_data);
    }
    let native_settings = std::fs::read(&settings_path).unwrap();
    assert!(native_settings.starts_with(aim_android_xml::abx::MAGIC));
    assert_eq!(
        native_settings,
        std::fs::read(settings_path.with_file_name("packages.xml.reservecopy")).unwrap()
    );
    let document = aim_android_xml::read(&native_settings).unwrap();
    assert!(document.children().any(|e| e.name == "vendor-extension"));
    persistence.commit_scan_settings(&original_capture).unwrap();
    assert_eq!(std::fs::read(&settings_path).unwrap(), native_settings);
    let mut invalid_capture = scan.owner.clone();
    invalid_capture.settings.packages[0]
        .code_path
        .push_str("/different");
    assert!(
        matches!(captures.publish(&original_capture, invalid_capture, original_capture.usage().clone()),
        Err(aim_services::package::scan_snapshot::Error::Invalid(ref message))
            if message == "loaded code differs from its owner")
    );
    assert!(std::sync::Arc::ptr_eq(
        &original_capture,
        &captures.capture()
    ));
    assert_eq!(scan.owner.loaded_packages().len(), scan.packages.len());
    assert!(scan.owner.disabled_loaded_packages().is_empty());
    // This fixture supplies a controlled policy; live PlatformCompat is a
    // separate owner. Exercise dependency inputs from the actual native scan.
    let dependencies = scan
        .owner
        .resolve_library_dependencies(&|_, _| {
            Ok(aim_services::package::libraries::Policy::pinned(false))
        })
        .unwrap();
    assert_eq!(
        dependencies.packages.len(),
        scan.owner.loaded_packages().len()
    );
    let mut graph_owner = scan.owner.clone();
    assert!(graph_owner.library_dependencies("android").is_err());
    let before_graph = graph_owner.clone();
    assert!(
        graph_owner
            .complete_runtime_at_boot(original_capture.usage(), std::collections::BTreeMap::new())
            .is_err()
    );
    assert_eq!(graph_owner, before_graph);
    assert!(
        graph_owner
            .complete_library_dependencies(&|_, _| Err(
                aim_services::package::libraries::ResolveError::Incomplete(
                    "unavailable compatibility owner"
                )
            ))
            .is_err()
    );
    assert_eq!(graph_owner, before_graph);
    graph_owner
        .complete_library_dependencies(&|_, _| {
            Ok(aim_services::package::libraries::Policy::pinned(false))
        })
        .unwrap();
    for (name, dependency) in &dependencies.packages {
        let (files, infos) = graph_owner.library_dependencies(name).unwrap().unwrap();
        assert_eq!(files, dependency.uses_library_files);
        assert_eq!(infos, dependency.uses_library_infos);
        assert_eq!(
            graph_owner.scanned_user_states(name).unwrap(),
            &dependency.users
        );
    }
    assert!(
        graph_owner
            .library_dependencies("unknown")
            .unwrap()
            .is_none()
    );
    graph_owner
        .complete_runtime_at_boot(original_capture.usage(), std::collections::BTreeMap::new())
        .unwrap();
    for setting in &graph_owner.settings.packages {
        let runtime = graph_owner
            .replica_runtime(&setting.name, false)
            .unwrap()
            .unwrap();
        assert_eq!(
            runtime.usage,
            *original_capture.usage().times(&setting.name).unwrap()
        );
        let labels = graph_owner.seinfo_state(&setting.name).unwrap().unwrap();
        assert_eq!(runtime.seinfo, labels.base);
        assert_eq!(runtime.override_seinfo, labels.override_label);
        let (files, infos) = graph_owner
            .library_dependencies(&setting.name)
            .unwrap()
            .unwrap();
        assert_eq!(runtime.library_files, files);
        assert_eq!(runtime.libraries, infos);
    }
    let graph_store = aim_services::package::scan_snapshot::Store::new(
        graph_owner.clone(),
        original_capture.usage().clone(),
    )
    .unwrap();
    let old_graph = graph_store.capture();
    let mut changed_user = graph_owner.scanned_user_states("android").unwrap()[&0].clone();
    changed_user.installed = !changed_user.installed;
    graph_owner
        .set_user_state("android", 0, changed_user)
        .unwrap();
    assert!(graph_owner.library_dependencies("android").is_err());
    assert!(
        graph_store
            .publish(&old_graph, graph_owner.clone(), old_graph.usage().clone())
            .is_err()
    );
    assert!(std::sync::Arc::ptr_eq(&old_graph, &graph_store.capture()));
    graph_owner
        .complete_library_dependencies(&|_, _| {
            Ok(aim_services::package::libraries::Policy::pinned(false))
        })
        .unwrap();
    let new_graph = graph_store
        .publish(&old_graph, graph_owner, old_graph.usage().clone())
        .unwrap();
    assert_ne!(
        old_graph.owner().scanned_user_states("android").unwrap()[&0].installed,
        new_graph.owner().scanned_user_states("android").unwrap()[&0].installed
    );
    assert!(
        old_graph
            .owner()
            .library_dependencies("android")
            .unwrap()
            .is_some()
    );
    assert!(
        new_graph
            .owner()
            .library_dependencies("android")
            .unwrap()
            .is_some()
    );
    for completed in &scan.packages {
        let record = &completed.candidate.record;
        let loaded = &scan.owner.loaded_packages()[&record.settings.name];
        assert!(std::sync::Arc::ptr_eq(
            loaded,
            &original_capture.owner().loaded_packages()[&record.settings.name]
        ));
        assert_eq!(loaded.collected_signing, record.signing);
        assert_eq!(
            loaded.facade_entry().unwrap().past_signing_certificates,
            record.signing.past_signing_certificates
        );
        assert_eq!(record.parsed.uid, record.settings.app_id);
        assert_eq!(
            record.settings.leaving_shared_user,
            Some(
                record
                    .parsed
                    .is(aim_services::package::pkg::booleans::LEAVING_SHARED_UID)
            )
        );
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
    // Use unchanged original code at a disposable APEX mount to exercise the
    // complete scan and skipped-factory registration order, not just setters.
    let static_model = |image: &mut Image| {
        let pkg = &mut image.packages[1].parsed;
        pkg.static_shared_library_name = Some("fixture.static".into());
        pkg.static_shared_lib_version = 0x100000007;
        pkg.shared_user_id = None;
        pkg.original_packages = None;
        pkg.library_names.clear();
        pkg.activities.clear();
        pkg.services.clear();
        pkg.providers.clear();
        pkg.receivers.clear();
        pkg.permission_groups.clear();
        pkg.attributions.clear();
        pkg.permissions.clear();
        pkg.protected_broadcasts.clear();
        pkg.overlay_target = None;
    };
    let apex_app = fixture.0.join("apex/different.mount/priv-app/GSF");
    std::fs::create_dir_all(&apex_app).unwrap();
    std::os::unix::fs::symlink(
        original.join("system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk"),
        apex_app.join("GSF.apk"),
    )
    .unwrap();
    std::fs::remove_file(app.join("GSF.apk")).unwrap();
    for module in [None, Some("raw.module")] {
        for factory in [false, true] {
            let apex = aim_services::package::scan::Apex {
                module_name: module.map(str::to_owned),
                mount_path: "/apex/different.mount".into(),
                partition: aim_services::package::scan::Partition::Product,
                factory,
                active_changed: !factory,
            };
            let registered = SystemImageScan::first_boot(
                || Image::load(&apks, &[apex.clone()]),
                &apks,
                &config,
                inputs(&domain_ids),
            )
            .unwrap();
            let record = &registered.packages[1].candidate.record;
            assert_eq!(
                record.settings.transient.apex_module_name.as_deref(),
                module
            );
            assert_eq!(record.settings.transient.apk_in_updated_apex, !factory);
            assert!(
                !record
                    .parsed
                    .is2(aim_services::package::pkg::booleans2::APEX)
            );
            assert_eq!(
                registered.owner.settings.packages[1].transient,
                record.settings.transient
            );
            if factory && module.is_some() {
                let mut static_image = Image::load(&apks, &[apex.clone()]).unwrap();
                static_model(&mut static_image);
                let static_scan = SystemImageScan::first_boot(
                    || Ok(static_image),
                    &apks,
                    &config,
                    inputs(&domain_ids),
                )
                .unwrap();
                let record = &static_scan.packages[1].candidate.record;
                assert_eq!(record.settings.name, "com.google.android.gsf");
                assert_eq!(record.identity.internal_name, record.settings.name);
                assert_eq!(
                    record.settings.transient.apex_module_name.as_deref(),
                    module
                );
                assert_eq!(
                    static_scan.owner.loaded_packages()[&record.settings.name].package,
                    record.parsed
                );
            }
            let mut settings = registered.owner.settings.clone();
            let mut disabled = settings.packages[1].clone();
            disabled.transient.apex_module_name = Some("stale.factory".into());
            settings.disabled_system_packages.push(disabled);
            settings.packages[1].code_path = "/data/app/fixture-apex-update".into();
            settings.packages[1].version_code += 1;
            settings.packages[1].flags |= 1 << 7;
            settings.packages[1].transient.apex_module_name = Some("active.before".into());
            let active = settings.packages[1].clone();
            let path = writable.join("app/fixture-apex-update");
            std::fs::write(&path, b"disposable updated code").unwrap();
            if module.is_some() && !factory {
                let mut failed =
                    aim_services::package::scan::SigningScan::new(&config, &settings, 36).unwrap();
                let calls = AtomicU8::new(0);
                let reject_factory_domain = || {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        Ok([1; 16])
                    } else {
                        Err("domain owner unavailable".into())
                    }
                };
                assert!(matches!(failed.scan_saved_system_image(
                    Image::load(&apks, &[apex.clone()]).unwrap(), &apks, &config,
                    FirstBootSystemInputs { new_domain_id: &reject_factory_domain, ..inputs(&domain_ids) }, saved_inputs()),
                    Err(SigningError::Rejected(error)) if error.phase == "domain"));
                assert_eq!(
                    failed.settings.disabled_system_packages[0]
                        .transient
                        .apex_module_name
                        .as_deref(),
                    module
                );
                assert_eq!(failed.settings.packages[1], active);
            }
            let mut owner =
                aim_services::package::scan::SigningScan::new(&config, &settings, 36).unwrap();
            let batch = owner
                .scan_saved_system_image(
                    Image::load(&apks, &[apex]).unwrap(),
                    &apks,
                    &config,
                    inputs(&domain_ids),
                    saved_inputs(),
                )
                .unwrap();
            assert_eq!(batch.retained_data.len(), 1);
            assert_eq!(owner.settings.packages[1], active);
            assert_eq!(
                owner.settings.disabled_system_packages[0]
                    .transient
                    .apex_module_name
                    .as_deref(),
                module
            );
            assert_eq!(
                batch.retained_data[0]
                    .record
                    .settings
                    .transient
                    .apex_module_name
                    .as_deref(),
                module
            );
            // scanPackageOnly does not commit a new APK-in-updated-APEX bit.
            assert_eq!(
                owner.settings.disabled_system_packages[0]
                    .transient
                    .apk_in_updated_apex,
                !factory
            );
            std::fs::remove_file(path).unwrap();
        }
    }
    std::fs::remove_dir_all(fixture.0.join("apex")).unwrap();
    std::os::unix::fs::symlink(
        original.join("system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk"),
        app.join("GSF.apk"),
    )
    .unwrap();
    // A controlled parsed static-library DTO exercises scan identity with
    // original verified code; APK bytes and parser fixtures remain unchanged.

    let mut ordinary_static = Image::load(&apks, &[]).unwrap();
    static_model(&mut ordinary_static);
    let ordinary_static =
        SystemImageScan::first_boot(|| Ok(ordinary_static), &apks, &config, inputs(&domain_ids))
            .unwrap();
    assert_eq!(
        ordinary_static.packages[1].candidate.record.settings.name,
        "com.google.android.gsf_4294967303"
    );
    // Controlled request DTOs use unchanged original APKs. A failed signer
    // reserves its otherwise unused shared UID before the later GSF admission.
    let rejected_dir = fixture.0.join("product/priv-app/EarlyReject");
    std::fs::create_dir(&rejected_dir).unwrap();
    std::os::unix::fs::symlink(
        original.join("system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk"),
        rejected_dir.join("GSF.apk"),
    )
    .unwrap();
    let mut raw = Image::parse(&apks, &[]).unwrap();
    let early = raw
        .packages
        .iter_mut()
        .find(|p| p.location.path.ends_with("EarlyReject"))
        .unwrap();
    early.parsed.shared_user_id = Some("fixture.rejected.shared".into());
    let root = fixture.0.clone();
    let bad_signing = Apks {
        files: Box::new(move |path| {
            Some(
                root.join(if path == "/product/priv-app/EarlyReject/GSF.apk" {
                    "product/priv-app/EarlyReject"
                } else {
                    path.trim_start_matches('/')
                }),
            )
        }),
        platform: Platform::load(&original, Default::default()).unwrap(),
    };
    let continued =
        SystemImageScan::first_boot_parsed(|| Ok(raw), &bad_signing, &config, inputs(&domain_ids))
            .unwrap();
    assert_eq!(continued.packages.len(), 2);
    assert_eq!(continued.rejected.len(), 1);
    assert!(continued.rejected[0].reason.ends_with("(-103)"));
    let failed_group = &continued.owner.identities.shared_users["fixture.rejected.shared"];
    assert_eq!(
        (
            failed_group.app_id,
            failed_group.flags,
            failed_group.private_flags
        ),
        (10000, 0, 0)
    );
    assert!(failed_group.signatures.is_none());
    assert_eq!(failed_group.member_count(), 0);
    assert_eq!(
        continued.packages[1].candidate.record.settings.app_id,
        10001
    );
    assert!(
        continued
            .owner
            .settings
            .shared_users
            .iter()
            .any(|g| g.name == "fixture.rejected.shared"
                && g.app_id == 10000
                && g.signatures.is_none())
    );
    assert!(rejected_dir.join("GSF.apk").exists());
    // Missing native mapping is fatal, but preparation remains committed.
    let mut raw = Image::parse(&apks, &[]).unwrap();
    raw.packages
        .iter_mut()
        .find(|p| p.location.path.ends_with("EarlyReject"))
        .unwrap()
        .parsed
        .shared_user_id = Some("fixture.rejected.shared".into());
    let root = fixture.0.clone();
    let unmapped = Apks {
        files: Box::new(move |path| {
            if path == "/product/priv-app/EarlyReject/GSF.apk" {
                None
            } else {
                Some(root.join(path.trim_start_matches('/')))
            }
        }),
        platform: Platform::load(&original, Default::default()).unwrap(),
    };
    let mut stopped =
        aim_services::package::scan::SigningScan::new(&config, &Default::default(), 36).unwrap();
    assert!(
        matches!(stopped.scan_saved_parsed_system_image(raw, &unmapped, &config,
        inputs(&domain_ids), saved_inputs()), Err(SigningError::Fatal(error)) if error.phase == "certificates")
    );
    assert_eq!(
        stopped.identities.shared_users["fixture.rejected.shared"].app_id,
        10000
    );
    assert!(stopped.loaded_packages().contains_key("android"));
    assert!(
        !stopped
            .loaded_packages()
            .contains_key("com.google.android.gsf")
    );
    let mut stale = scan.owner.settings.clone();
    stale
        .disabled_system_packages
        .push(stale.packages[1].clone());
    stale.packages.clear();
    for fatal in [false, true] {
        let mut raw = Image::parse(&apks, &[]).unwrap();
        raw.packages
            .iter_mut()
            .find(|p| p.location.path.ends_with("EarlyReject"))
            .unwrap()
            .parsed
            .shared_user_id = Some("fixture.rejected.shared".into());
        let mut owner = aim_services::package::scan::SigningScan::new(&config, &stale, 36).unwrap();
        let result = owner.scan_saved_parsed_system_image(
            raw,
            if fatal { &unmapped } else { &bad_signing },
            &config,
            inputs(&domain_ids),
            saved_inputs(),
        );
        if fatal {
            assert!(
                matches!(result, Err(SigningError::Fatal(error)) if error.phase == "certificates")
            );
            assert!(
                !owner
                    .loaded_packages()
                    .contains_key("com.google.android.gsf")
            );
        } else {
            let result = result.unwrap();
            assert_eq!(result.rejected.len(), 1);
            assert!(
                owner
                    .loaded_packages()
                    .contains_key("com.google.android.gsf")
            );
        }
        assert!(owner.settings.disabled_system_packages.is_empty());
        assert!(owner.disabled_loaded_packages().is_empty());
        assert_eq!(
            owner.identities.shared_users["fixture.rejected.shared"].app_id,
            10001
        );
        assert!(rejected_dir.join("GSF.apk").exists());
    }
    std::fs::remove_file(rejected_dir.join("GSF.apk")).unwrap();
    std::fs::remove_dir(rejected_dir).unwrap();
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
    // prepareInitialScanRequest resolves both names independently. Source
    // policy selects the old system setting even when the incoming setting
    // is non-system; scanPackageNew copies the incoming UID/users unchanged.
    let mut both = baseline.clone();
    let incoming = both
        .packages
        .iter_mut()
        .find(|p| p.name == "com.google.android.gsf")
        .unwrap();
    incoming.app_id = 10002;
    incoming.shared_user = false;
    incoming.shared_user_app_id = None;
    incoming.flags &= !aim_services::package::settings::FLAG_SYSTEM;
    let mut old = incoming.clone();
    old.name = "fixture.original.gsf".into();
    old.app_id = 10003;
    old.flags |= aim_services::package::settings::FLAG_SYSTEM;
    old.code_path = "/product/priv-app/PreviousGSF".into();
    both.packages.push(old.clone());
    let mut both_users = full_users.clone();
    let mut old_users = both_users["com.google.android.gsf"].clone();
    for state in old_users.values_mut() {
        state.stopped = !state.stopped;
    }
    both_users.insert(old.name.clone(), old_users);
    let mut both_image = Image::parse(&apks, &[]).unwrap();
    let incoming_code = &mut both_image.packages[1].parsed;
    incoming_code.shared_user_id = None;
    incoming_code.original_packages = Some(vec![Some(old.name.clone())]);
    let mut both_owner = aim_services::package::scan::SigningScan::new(&config, &both, 36).unwrap();
    let result = both_owner
        .scan_saved_parsed_system_image(
            both_image,
            &apks,
            &config,
            inputs(&domain_ids),
            aim_services::package::scan::SavedSystemScanInputs {
                users: &both_users,
                ..saved_inputs()
            },
        )
        .unwrap();
    let accepted = &result.packages[1].candidate;
    assert_eq!(accepted.record.settings.name, "com.google.android.gsf");
    assert_eq!(accepted.record.settings.app_id, 10002);
    assert_eq!(accepted.users, both_users["com.google.android.gsf"]);
    assert_eq!(
        both_owner
            .settings
            .packages
            .iter()
            .find(|p| p.name == old.name),
        Some(&old)
    );
    assert!(both_owner.settings.renamed_packages.is_empty());
    assert!(!both_owner.loaded_packages().contains_key(&old.name));
    assert!(
        both_owner
            .loaded_packages()
            .contains_key("com.google.android.gsf")
    );
    // The original's disabled ScanRequest copies that setting while retaining
    // the incoming parsed name. Source choice uses the original version/path;
    // restoration enables that owner before admitting the incoming setting.
    for (restore, incoming) in [(false, true), (true, true), (false, false), (true, false)] {
        let mut both = both.clone();
        let mut selected_users = both_users.clone();
        if !incoming {
            both.packages.retain(|p| p.name != "com.google.android.gsf");
            selected_users.remove("com.google.android.gsf");
        }
        let source = both
            .packages
            .iter_mut()
            .find(|p| p.name == old.name)
            .unwrap();
        source.code_path = "/data/app/OriginalGSF".into();
        source.version_code = if restore { 0 } else { i64::MAX };
        source.transient.updated_system_app = true;
        let source_before = source.clone();
        let mut factory = old.clone();
        factory.transient.updated_system_app = false;
        if !incoming {
            factory.primary_cpu_abi = Some("armeabi-v7a".into());
        }
        both.disabled_system_packages.push(factory);
        let mut factory_owner =
            aim_services::package::scan::SigningScan::new(&config, &both, 36).unwrap();
        let captured = selected_users
            .iter()
            .map(|(name, users)| {
                (
                    (name.clone(), false),
                    aim_services::package::scan::CapturedUsers {
                        states: users.clone(),
                        active_aliases: Default::default(),
                    },
                )
            })
            .chain(std::iter::once((
                (old.name.clone(), true),
                aim_services::package::scan::CapturedUsers {
                    states: selected_users[&old.name].clone(),
                    active_aliases: Default::default(),
                },
            )))
            .collect();
        factory_owner.capture_user_states(captured).unwrap();
        let mut both_image = Image::parse(&apks, &[]).unwrap();
        both_image.packages[1].parsed.shared_user_id = None;
        both_image.packages[1].parsed.original_packages = Some(vec![Some(old.name.clone())]);
        let result = factory_owner
            .scan_saved_parsed_system_image(
                both_image,
                &apks,
                &config,
                inputs(&domain_ids),
                aim_services::package::scan::SavedSystemScanInputs {
                    users: &selected_users,
                    ..saved_inputs()
                },
            )
            .unwrap();
        if incoming || !restore {
            assert!(factory_owner.settings.renamed_packages.is_empty());
            assert!(factory_owner.transferred_packages().is_empty());
            assert!(!factory_owner.loaded_packages().contains_key(&old.name));
        }
        if restore {
            assert!(result.retained_data.is_empty());
            assert!(factory_owner.settings.disabled_system_packages.is_empty());
            let accepted = &result.packages[1].candidate;
            if incoming {
                assert_eq!(accepted.record.settings.name, "com.google.android.gsf");
                assert_eq!(accepted.record.settings.app_id, 10002);
                assert_eq!(accepted.users, selected_users["com.google.android.gsf"]);
            } else {
                assert_eq!(accepted.record.settings.name, old.name);
                assert_eq!(accepted.record.parsed.package_name, old.name);
                assert_eq!(accepted.record.settings.app_id, 10003);
                assert_eq!(accepted.record.parsed.uid, 10003);
                assert_eq!(
                    accepted.record.settings.real_name.as_deref(),
                    Some("com.google.android.gsf")
                );
                assert_eq!(accepted.users, selected_users[&old.name]);
                assert_eq!(
                    accepted.record.settings.primary_cpu_abi,
                    baseline
                        .packages
                        .iter()
                        .find(|p| p.name == "com.google.android.gsf")
                        .unwrap()
                        .primary_cpu_abi
                );
                assert_ne!(
                    accepted.record.settings.primary_cpu_abi.as_deref(),
                    Some("armeabi-v7a")
                );
                assert_eq!(
                    factory_owner.settings.renamed_packages,
                    vec![("com.google.android.gsf".into(), old.name.clone())]
                );
                assert_eq!(
                    factory_owner
                        .transferred_packages()
                        .iter()
                        .collect::<Vec<_>>(),
                    vec![&old.name]
                );
                assert!(factory_owner.loaded_packages().contains_key(&old.name));
            }
            let enabled = factory_owner
                .settings
                .packages
                .iter()
                .find(|p| p.name == old.name)
                .unwrap();
            assert_eq!(enabled.app_id, 10003);
            assert!(!enabled.transient.updated_system_app);
        } else {
            assert_eq!(result.packages.len(), 1);
            assert_eq!(result.retained_data.len(), 1);
            let factory = &result.retained_data[0];
            assert_eq!(factory.record.settings.name, old.name);
            assert_eq!(factory.record.settings.app_id, 10003);
            assert_eq!(factory.record.parsed.package_name, "com.google.android.gsf");
            assert_eq!(
                factory.record.identity.internal_name,
                "com.google.android.gsf"
            );
            assert!(factory.record.signing.unknown);
            assert_eq!(factory.users, both_users[&old.name]);
            assert_eq!(
                factory_owner
                    .settings
                    .packages
                    .iter()
                    .find(|p| p.name == old.name),
                Some(&source_before)
            );
            assert!(
                !factory_owner
                    .loaded_packages()
                    .contains_key("com.google.android.gsf")
            );
        }
    }
    // Ordinary original-name creation also derives ABI instead of borrowing
    // the old package's ABI. A later metadata failure cannot publish transfer.
    fn retained_original<'a>(
        owner: &'a aim_services::package::scan::SigningScan,
        name: &str,
        shared: bool,
    ) -> &'a aim_services::package::owner::app_ids::DetachedSetting {
        if shared {
            owner.identities.shared_users["fixture.original.group"]
                .retained_setting(name)
                .unwrap()
        } else {
            owner.identities.ids.detached_setting(10003).unwrap()
        }
    }
    for (invalid_page_size, shared, incoming, incoming_shared, keep) in [
        (false, false, false, false, 0),
        (true, false, false, false, 0),
        (false, true, false, false, 0),
        (false, true, true, false, 0),
        (true, true, true, false, 0),
        (false, false, true, true, 0),
        (false, false, true, true, 1),
        (false, true, true, true, 0),
        (false, true, true, true, 1),
        (true, false, true, true, 0),
        (true, true, true, true, 1),
        (false, false, true, true, 2),
        (false, true, true, true, 2),
    ] {
        let keep_member = keep == 1;
        let keep_disabled = keep == 2;
        let kept = keep != 0;
        let mut settings = both.clone();
        if incoming {
            let installed = settings
                .packages
                .iter_mut()
                .find(|p| p.name == "com.google.android.gsf")
                .unwrap();
            installed.primary_cpu_abi = Some("arm64-v8a".into());
            installed.pending_restore = true;
        } else {
            settings
                .packages
                .retain(|p| p.name != "com.google.android.gsf");
        }
        settings
            .packages
            .iter_mut()
            .find(|p| p.name == old.name)
            .unwrap()
            .primary_cpu_abi = Some("armeabi-v7a".into());
        if shared {
            let original = settings
                .packages
                .iter_mut()
                .find(|p| p.name == old.name)
                .unwrap();
            original.shared_user = true;
            original.shared_user_app_id = Some(10003);
            settings
                .shared_users
                .push(aim_services::package::settings::SharedUser {
                    name: "fixture.original.group".into(),
                    app_id: 10003,
                    flags: 0,
                    signatures: original.signatures.clone(),
                });
        }
        if incoming_shared {
            let installed = settings
                .packages
                .iter_mut()
                .find(|p| p.name == "com.google.android.gsf")
                .unwrap();
            installed.shared_user = true;
            installed.shared_user_app_id = Some(10002);
            if keep_disabled {
                settings.disabled_system_packages.push(installed.clone());
            }
            let mut other = installed.clone();
            other.name = "fixture.old-group-member".into();
            settings
                .shared_users
                .push(aim_services::package::settings::SharedUser {
                    name: "fixture.incoming.group".into(),
                    app_id: 10002,
                    flags: 0,
                    signatures: installed.signatures.clone(),
                });
            if keep_member {
                settings.packages.push(other);
            }
        }
        let incoming_before = settings
            .packages
            .iter()
            .find(|p| p.name == "com.google.android.gsf")
            .cloned();
        let before = settings
            .packages
            .iter()
            .find(|p| p.name == old.name)
            .unwrap()
            .clone();
        let mut owner =
            aim_services::package::scan::SigningScan::new(&config, &settings, 36).unwrap();
        if incoming_shared && !invalid_page_size {
            let scopes: Vec<_> = owner
                .settings
                .packages
                .iter()
                .map(|p| (p, false))
                .chain(
                    owner
                        .settings
                        .disabled_system_packages
                        .iter()
                        .map(|p| (p, true)),
                )
                .collect();
            let users = scopes
                .iter()
                .map(|(p, factory)| {
                    (
                        (p.name.clone(), *factory),
                        aim_services::package::scan::CapturedUsers {
                            states: both_users
                                .get(&p.name)
                                .unwrap_or(&both_users["com.google.android.gsf"])
                                .clone(),
                            active_aliases: Default::default(),
                        },
                    )
                })
                .collect();
            let migrations = scopes
                .iter()
                .map(|(p, factory)| ((p.name.clone(), *factory), Default::default()))
                .collect();
            let fixed = scopes
                .iter()
                .map(|(p, factory)| ((p.name.clone(), *factory), true))
                .collect();
            let runtime = scopes
                .iter()
                .map(|(p, factory)| {
                    (
                        (p.name.clone(), *factory),
                        aim_services::package::scan::ReplicaRuntime {
                            usage: [0; 8],
                            seinfo: None,
                            override_seinfo: None,
                            library_files: vec![],
                            libraries: vec![],
                        },
                    )
                })
                .collect();
            let groups = owner
                .identities
                .shared_users
                .keys()
                .map(|name| (name.clone(), Default::default()))
                .collect();
            let user_ids: Vec<_> = both_users[&old.name].keys().copied().collect();
            // Explicit saved constructor-state inputs for complete replica transport.
            owner.capture_user_states(users).unwrap();
            owner
                .capture_legacy_permissions(&user_ids, migrations, groups)
                .unwrap();
            owner.capture_install_permissions_fixed(fixed).unwrap();
            owner.capture_replica_runtime(runtime).unwrap();
        }
        let mut image = Image::parse(&apks, &[]).unwrap();
        image.packages[1].parsed.shared_user_id = shared.then(|| "fixture.original.group".into());
        image.packages[1].parsed.original_packages = Some(vec![Some(old.name.clone())]);
        let mut request = inputs(&domain_ids);
        if invalid_page_size {
            request.install.page_size = 8193;
            // Put the independent-UID candidate at the metadata gate before
            // framework admission, so this tests adoption's staged effects.
            image.packages.swap(0, 1);
        }
        let result = owner.scan_saved_parsed_system_image(
            image,
            &apks,
            &config,
            request,
            aim_services::package::scan::SavedSystemScanInputs {
                users: &both_users,
                ..saved_inputs()
            },
        );
        if invalid_page_size {
            assert!(
                matches!(result, Err(SigningError::NativeLibrary { package, .. })
                if package == old.name)
            );
            if incoming_shared {
                assert_eq!(
                    owner.identities.shared_users["fixture.incoming.group"].member_count(),
                    if keep_member { 2 } else { 1 }
                );
                assert_eq!(
                    owner
                        .settings
                        .packages
                        .iter()
                        .find(|p| p.name == "com.google.android.gsf"),
                    incoming_before.as_ref()
                );
            }
            assert!(owner.transferred_packages().is_empty());
            assert!(owner.settings.renamed_packages.is_empty());
            assert!(owner.identities.ids.detached_setting(10003).is_none());
            if shared {
                assert!(
                    owner.identities.shared_users["fixture.original.group"]
                        .retained_setting(&old.name)
                        .is_none()
                );
            }
            assert_eq!(
                owner.settings.packages.iter().find(|p| p.name == old.name),
                Some(&before)
            );
        } else {
            let result = result.unwrap();
            let candidate = &result.packages[1].candidate;
            assert_eq!(candidate.record.settings.name, old.name);
            assert_eq!(candidate.record.settings.app_id, 10003);
            assert_eq!(candidate.record.parsed.uid, 10003);
            assert_eq!(candidate.users, both_users[&old.name]);
            if incoming {
                assert!(candidate.record.settings.pending_restore);
                assert_eq!(
                    candidate.record.settings.primary_cpu_abi.as_deref(),
                    Some("arm64-v8a")
                );
                let installed = owner
                    .settings
                    .packages
                    .iter()
                    .find(|p| p.name == "com.google.android.gsf")
                    .unwrap();
                assert_eq!(installed.app_id, 10002);
                assert!(!owner.loaded_packages().contains_key(&installed.name));
                assert_eq!(Some(installed), incoming_before.as_ref());
                if incoming_shared {
                    assert_eq!(owner.identities.ids.get(10002).is_some(), kept);
                    if kept {
                        assert_eq!(
                            owner.identities.shared_users["fixture.incoming.group"].member_count(),
                            if keep_member { 1 } else { 0 }
                        );
                    } else {
                        assert!(
                            !owner
                                .identities
                                .shared_users
                                .contains_key("fixture.incoming.group")
                        );
                    }
                } else {
                    assert_eq!(
                        owner.identities.ids.get(10002),
                        Some(&aim_services::package::owner::app_ids::Owner::Package(
                            installed.name.clone()
                        ))
                    );
                }
            }
            if incoming_shared {
                let process_orders = owner
                    .identities
                    .shared_users
                    .iter()
                    .map(|(name, group)| {
                        let mut members: Vec<_> = owner
                            .settings
                            .packages
                            .iter()
                            .filter(|p| {
                                p.shared_app_id() == Some(group.app_id)
                                    && p.name != "com.google.android.gsf"
                            })
                            .map(|p| (p.name.clone(), false))
                            .collect();
                        if group.retained_setting(&old.name).is_some() {
                            members.push((old.name.clone(), true));
                        }
                        (name.clone(), members)
                    })
                    .collect();
                owner
                    .complete_shared_process_instances(process_orders)
                    .unwrap();
                let usage = aim_services::package::owner::usage::Usage::new(
                    owner.settings.packages.iter().map(|p| p.name.as_str()),
                );
                owner
                    .complete_library_dependencies(&|_, _| {
                        Ok(aim_services::package::libraries::Policy::pinned(false))
                    })
                    .unwrap();
                let retained = owner
                    .settings
                    .packages
                    .iter()
                    .map(|p| (p, false))
                    .filter(|(p, _)| !owner.loaded_packages().contains_key(&p.name))
                    .chain(
                        owner
                            .settings
                            .disabled_system_packages
                            .iter()
                            .map(|p| (p, true)),
                    )
                    .map(|(p, factory)| {
                        (
                            (p.name.clone(), factory),
                            aim_services::package::scan::OriginalRuntime {
                                name: p.name.clone(),
                                app_id: p.app_id,
                                path: p.code_path.clone(),
                                version: p.version_code,
                                has_code: false,
                                transient: p.transient.clone(),
                                state: aim_services::package::scan::ReplicaRuntime {
                                    usage: [0; 8],
                                    seinfo: None,
                                    override_seinfo: None,
                                    library_files: vec![],
                                    libraries: vec![],
                                },
                            },
                        )
                    })
                    .collect();
                owner.complete_runtime_at_boot(&usage, retained).unwrap();
                let store =
                    aim_services::package::scan_snapshot::Store::new(owner.clone(), usage).unwrap();
                let capture = store.capture();
                for (settings, factory) in [
                    (&owner.settings.packages, false),
                    (&owner.settings.disabled_system_packages, true),
                ] {
                    for setting in settings {
                        assert!(
                            aim_services::package::scan_snapshot::setting_record::captured(
                                &capture,
                                &setting.name,
                                factory
                            )
                            .unwrap()
                            .is_some()
                        );
                        assert!(
                            aim_services::package::scan_snapshot::runtime_record::captured(
                                &capture,
                                &setting.name,
                                factory
                            )
                            .unwrap()
                            .is_some()
                        );
                        for id in owner.scanned_user_states(&setting.name).unwrap().keys() {
                            assert!(
                                aim_services::package::scan_snapshot::user_record::captured(
                                    &capture,
                                    &setting.name,
                                    factory,
                                    *id
                                )
                                .unwrap()
                                .is_some()
                            );
                        }
                    }
                }
                assert_eq!(
                    aim_services::package::scan_snapshot::shared_record::captured(
                        &capture,
                        "fixture.incoming.group"
                    )
                    .unwrap()
                    .is_some(),
                    kept
                );
                let signing =
                    aim_services::package::scan_snapshot::endpoint::PackageSigningState::captured(
                        &capture,
                        "com.google.android.gsf",
                        false,
                    )
                    .unwrap()
                    .unwrap();
                let mut parcel = aim_binder_host::parcel::Parcel::new();
                aim_service_aidl::WriteParcelable::write_to(&signing, &mut parcel);
                let mut reader = aim_binder_host::parcel::Reader::new(parcel.data(), &[]);
                assert_eq!(reader.read_i64().unwrap(), capture.version() as i64);
                assert_eq!(
                    reader.read_string16().unwrap().as_deref(),
                    Some("com.google.android.gsf")
                );
                assert_eq!(reader.read_i32().unwrap(), 10002);
                assert_eq!(reader.read_i32().unwrap(), 0);
                assert_eq!(
                    reader.read_string16().unwrap().as_deref(),
                    kept.then_some("fixture.incoming.group")
                );
                assert_eq!(reader.read_i32().unwrap(), 10002);
                assert_eq!(
                    reader.read_string16().unwrap().as_deref(),
                    Some(old.name.as_str())
                );
                let restart_path = writable.join(format!("displaced-restart-{shared}-{keep}"));
                std::fs::create_dir_all(restart_path.join("system")).unwrap();
                // Rename publication is a separate global owner; scan writes
                // preserve its already committed record.
                std::fs::write(restart_path.join("system/packages.xml"), format!(
                    "<packages><renamed-package new='com.google.android.gsf' old='{}'/></packages>", old.name)).unwrap();
                let mut disk = aim_services::package::owner::Store::open(&restart_path, &[0])
                    .unwrap()
                    .unwrap();
                disk.commit_scan_settings(&capture).unwrap();
                let written = aim_android_xml::read(
                    &std::fs::read(restart_path.join("system/packages.xml")).unwrap(),
                )
                .unwrap();
                assert!(written.children().any(|e| e.name == "package"
                    && e.string("name").as_deref() == Some("com.google.android.gsf")));
                let reopened = aim_services::package::owner::Store::open(&restart_path, &[0])
                    .unwrap()
                    .unwrap();
                let restored = aim_services::package::scan::SigningScan::new(
                    &config,
                    &reopened.state().settings,
                    36,
                )
                .unwrap();
                assert_eq!(
                    restored
                        .settings
                        .packages
                        .iter()
                        .any(|p| p.name == "com.google.android.gsf"),
                    kept
                );
                assert_eq!(restored.identities.ids.get(10002).is_some(), kept);
                if kept {
                    assert_eq!(
                        restored.identities.shared_users["fixture.incoming.group"].member_count(),
                        if keep_member { 2 } else { 1 }
                    );
                }
                assert!(restored.identities.ids.get(10003).is_some());
                let mut foreign_uid = owner.clone();
                foreign_uid
                    .identities
                    .ids
                    .replace(
                        10002,
                        aim_services::package::owner::app_ids::Owner::Package(
                            "fixture.foreign".into(),
                        ),
                    )
                    .unwrap();
                let unchanged = foreign_uid.clone();
                assert!(
                    foreign_uid
                        .remove_package_setting("com.google.android.gsf")
                        .is_err()
                );
                assert_eq!(foreign_uid, unchanged);
                let mut removed = owner.clone();
                let result = removed
                    .remove_package_setting("com.google.android.gsf")
                    .unwrap()
                    .unwrap();
                assert_eq!(Some(&result.package), incoming_before.as_ref());
                assert_eq!(result.app_id_removed, !kept);
                assert_eq!(removed.identities.ids.get(10002).is_some(), kept);
                let removed_usage = aim_services::package::owner::usage::Usage::new(
                    removed.settings.packages.iter().map(|p| p.name.as_str()),
                );
                let removed_capture =
                    aim_services::package::scan_snapshot::Store::new(removed, removed_usage)
                        .unwrap()
                        .capture();
                assert!(
                    aim_services::package::scan_snapshot::endpoint::PackageSigningState::captured(
                        &removed_capture,
                        "com.google.android.gsf",
                        false
                    )
                    .unwrap()
                    .is_none()
                );
                assert!(
                    aim_services::package::scan_snapshot::endpoint::PackageSigningState::captured(
                        &capture,
                        "com.google.android.gsf",
                        false
                    )
                    .unwrap()
                    .is_some()
                );
                let mut foreign = owner.clone();
                foreign
                    .settings
                    .packages
                    .iter_mut()
                    .find(|p| p.name == "com.google.android.gsf")
                    .unwrap()
                    .version_code += 1;
                assert!(
                    store
                        .publish(&capture, foreign, capture.usage().clone())
                        .is_err()
                );
                assert_eq!(store.capture().version(), capture.version());
            }
            assert_ne!(
                candidate.record.settings.primary_cpu_abi.as_deref(),
                Some("armeabi-v7a")
            );
            assert_eq!(
                owner.transferred_packages().iter().collect::<Vec<_>>(),
                vec![&old.name]
            );
            if shared {
                let retained = retained_original(&owner, &old.name, shared);
                assert_eq!(retained.package, before);
                assert_eq!(retained.users, both_users[&old.name]);
                let (&user, state) = retained.users.iter().next().unwrap();
                assert!(retained.aliases_user(user));
                let mut changed = state.clone();
                changed.stopped = !changed.stopped;
                owner.fix_shared_seinfo_target_sdks_at_boot().unwrap();
                let usage = aim_services::package::owner::usage::Usage::new(
                    owner.settings.packages.iter().map(|p| p.name.as_str()),
                );
                let store =
                    aim_services::package::scan_snapshot::Store::new(owner.clone(), usage).unwrap();
                let capture = store.capture();
                let mut foreign = owner.clone();
                foreign
                    .identities
                    .ids
                    .replace(
                        10003,
                        aim_services::package::owner::app_ids::Owner::Package("foreign".into()),
                    )
                    .unwrap();
                assert!(
                    store
                        .publish(&capture, foreign, capture.usage().clone())
                        .is_err()
                );
                assert_eq!(store.capture().version(), capture.version());
                owner
                    .set_user_state(&old.name, user, changed.clone())
                    .unwrap();
                let retained = retained_original(&owner, &old.name, shared);
                assert_eq!(retained.users[&user], changed);
                assert_eq!(retained.package, before);
                assert_ne!(
                    retained_original(capture.owner(), &old.name, shared).users[&user],
                    changed
                );
                let new_user = both_users[&old.name].keys().max().unwrap() + 1;
                owner.set_user_state(&old.name, new_user, changed).unwrap();
                assert!(
                    !retained_original(&owner, &old.name, shared)
                        .users
                        .contains_key(&new_user)
                );
                if incoming_shared {
                    owner
                        .complete_library_dependencies(&|_, _| {
                            Ok(aim_services::package::libraries::Policy::pinned(false))
                        })
                        .unwrap();
                }
                store
                    .publish(&capture, owner, capture.usage().clone())
                    .unwrap();
                assert_eq!(
                    retained_original(capture.owner(), &old.name, shared).package,
                    before
                );
            } else {
                assert!(owner.identities.ids.detached_setting(10003).is_none());
                assert_eq!(
                    owner.identities.ids.get(10003),
                    Some(&aim_services::package::owner::app_ids::Owner::Package(
                        old.name.clone()
                    ))
                );
                let usage = aim_services::package::owner::usage::Usage::new(
                    owner.settings.packages.iter().map(|p| p.name.as_str()),
                );
                let store =
                    aim_services::package::scan_snapshot::Store::new(owner.clone(), usage).unwrap();
                let capture = store.capture();
                let (&user, previous) = owner
                    .scanned_user_states(&old.name)
                    .unwrap()
                    .iter()
                    .next()
                    .unwrap();
                let mut changed = previous.clone();
                changed.stopped = !changed.stopped;
                owner
                    .set_user_state(&old.name, user, changed.clone())
                    .unwrap();
                assert_ne!(
                    capture.owner().scanned_user_states(&old.name).unwrap()[&user],
                    changed
                );
                if incoming_shared {
                    owner
                        .complete_library_dependencies(&|_, _| {
                            Ok(aim_services::package::libraries::Policy::pinned(false))
                        })
                        .unwrap();
                }
                store
                    .publish(&capture, owner, capture.usage().clone())
                    .unwrap();
                assert!(
                    capture
                        .owner()
                        .identities
                        .ids
                        .detached_setting(10003)
                        .is_none()
                );
            }
        }
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
            .scan_saved_parsed_system_image(
                Image::parse(&apks, &[]).unwrap(),
                &apks,
                &config,
                inputs(&domain_ids),
                saved_inputs(),
            )
            .unwrap();
        if keep_data {
            // A fresh verifier cannot read this mapped certificate source.
            // The parser still reads the original APK through its code directory;
            // non-strict KeepData must refresh metadata without collecting it.
            let root = fixture.0.clone();
            let bad_signing = Apks {
                files: Box::new(move |path| {
                    Some(root.join(if path == "/product/priv-app/GSF/GSF.apk" {
                        "product/priv-app/GSF"
                    } else {
                        path.trim_start_matches('/')
                    }))
                }),
                platform: Platform::load(&original, Default::default()).unwrap(),
            };
            assert!(Image::load(&bad_signing, &[]).is_err());
            let mut retained =
                aim_services::package::scan::SigningScan::new(&config, &settings, 36).unwrap();
            let unchecked = retained
                .scan_saved_parsed_system_image(
                    Image::parse(&bad_signing, &[]).unwrap(),
                    &bad_signing,
                    &config,
                    inputs(&domain_ids),
                    saved_inputs(),
                )
                .unwrap();
            assert_eq!(unchecked.packages.len(), 1);
            assert_eq!(unchecked.retained_data.len(), 1);
            assert!(unchecked.retained_data[0].record.signing.unknown);
            assert!(unchecked.retained_code[0].signing.unknown);
            assert_eq!(retained.settings.packages[1], active);
            assert_eq!(
                unchecked.retained_data[0].record.settings.signatures,
                factory.signatures
            );
            assert!(data_code.exists());
            let mut strict = SystemConfig::default();
            strict
                .preinstall_packages_with_strict_signature_check
                .insert(factory.name.clone());
            let mut rejected =
                aim_services::package::scan::SigningScan::new(&strict, &settings, 36).unwrap();
            assert!(matches!(rejected.scan_saved_parsed_system_image(
                Image::parse(&bad_signing, &[]).unwrap(), &bad_signing, &strict,
                inputs(&domain_ids), saved_inputs(),
            ), Err(SigningError::Rejected(error)) if error.phase == "system-source" && error.message.ends_with("(-110)")));
            assert!(
                rejected.disabled_loaded_packages()[&factory.name]
                    .collected_signing
                    .unknown
            );
            assert_eq!(
                rejected.settings.disabled_system_packages[0].signatures,
                factory.signatures
            );
            assert_eq!(rejected.settings.packages[1], active);
            assert!(data_code.exists());
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
            let separate_compatibility =
                LibraryCompatibility::new(&config, &|_| None, false).unwrap();
            let data_inputs = || aim_services::package::scan::DataScanInputs {
                factory: Some(&batch.retained_data[0]),
                platform,
                vendor_sdk: 36,
                compatibility: &compatibility,
                remove_test_base: &|_, _| Err("system/BCP must not query compat".into()),
                expecting_better: &empty_packages,
                new_domain_id: &domain_ids,
                completion: ScanMetadataCompletion {
                    seinfo: common::seinfo::scan(),
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
            // The owner's decision sees policy-adjusted code, before library updates.
            for sdk in [29, 30] {
                let mut query_code = copy_code();
                query_code.parsed.target_sdk_version = sdk;
                query_code.parsed.booleans |= aim_services::package::pkg::booleans::PRIVILEGED
                    | aim_services::package::pkg::booleans::PERSISTENT;
                query_code.parsed.protected_broadcasts = vec!["fixture.protected".into()];
                query_code.parsed.uses_libraries.clear();
                query_code.parsed.uses_optional_libraries.clear();
                let calls = std::cell::Cell::new(0);
                let query = |pkg: &aim_services::package::pkg::AndroidPackage, system: bool| {
                    calls.set(calls.get() + 1);
                    assert!(!system);
                    assert!(!pkg.is(aim_services::package::pkg::booleans::SYSTEM));
                    assert!(!pkg.is(aim_services::package::pkg::booleans::PRIVILEGED));
                    assert!(!pkg.is(aim_services::package::pkg::booleans::PERSISTENT));
                    assert!(pkg.protected_broadcasts.is_empty());
                    assert!(pkg.uses_libraries.is_empty());
                    assert_eq!(pkg.target_sdk_version, sdk);
                    assert_eq!(pkg.path, raw.parsed.path);
                    assert_eq!(pkg.uid, raw.parsed.uid);
                    Ok(Some(sdk > 29))
                };
                let mut ordinary = before.clone();
                ordinary.settings.disabled_system_packages.clear();
                let accepted = ordinary
                    .scan_known_data(
                        &query_code,
                        &full_users,
                        None,
                        &apks,
                        aim_services::package::scan::DataScanInputs {
                            factory: None,
                            compatibility: &separate_compatibility,
                            remove_test_base: &query,
                            ..data_inputs()
                        },
                    )
                    .unwrap();
                assert_eq!(calls.get(), 1);
                assert_eq!(
                    accepted.candidate.record.parsed.uses_libraries,
                    if sdk == 29 {
                        vec!["android.test.base".to_string()]
                    } else {
                        Vec::new()
                    }
                );
            }
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
            assert!(
                accepted
                    .candidate
                    .record
                    .settings
                    .transient
                    .updated_system_app
            );
            assert_ne!(accepted.candidate.record.settings.private_flags & 8, 0);
            assert_eq!(accepted.candidate.record.signing, raw.signing);
            assert!(accepted.copies.is_empty());
            assert!(physical_data.join("base.apk").exists());
            use aim_services::package::scan::{DataImage, DataImageScanInputs};
            let destinations = std::collections::BTreeMap::new();
            let non_incremental = |_: &str| Ok(false);
            let remove_test_base = |_: &aim_services::package::pkg::AndroidPackage, _: bool| {
                Err("system/BCP must not query compat".into())
            };
            let loop_inputs = || DataImageScanInputs {
                certificates: Default::default(),
                seinfo: common::seinfo::scan(),
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
                .scan_parsed_data_image(
                    DataImage::parse(&apks, &[]).unwrap(),
                    &apks,
                    DataImageScanInputs {
                        certificates: Default::default(),
                        compatibility: &separate_compatibility,
                        ..loop_inputs()
                    },
                )
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
                        certificates: Default::default(),
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
            assert!(!ordinary.record.settings.transient.updated_system_app);
            assert_eq!(
                ordinary.record.settings.private_flags,
                ordinary_data.candidate.record.settings.private_flags
            );
            assert_eq!(ordinary.record.settings.app_id, active.app_id);
            assert_eq!(ordinary.record.settings.code_path, active.code_path);
            assert_eq!(ordinary.users, full_users[&active.name]);
            assert!(data_code.exists());
            let separate_compatibility =
                LibraryCompatibility::new(&config, &|_| None, false).unwrap();
            let calls = std::cell::Cell::new(0);
            let fail_rescan = |pkg: &aim_services::package::pkg::AndroidPackage, system: bool| {
                calls.set(calls.get() + 1);
                assert!(!system);
                assert!(!pkg.is(aim_services::package::pkg::booleans::SYSTEM));
                assert_eq!(
                    pkg.is(aim_services::package::pkg::booleans::PRIVILEGED),
                    ordinary_data
                        .candidate
                        .record
                        .parsed
                        .is(aim_services::package::pkg::booleans::PRIVILEGED)
                );
                Err("ex-system policy owner unavailable".into())
            };
            let mut failed_demotion = before.clone();
            assert!(
                matches!(failed_demotion.scan_data_image(DataImage::load(&apks, &[]).unwrap(), &apks,
                DataImageScanInputs { factories: &missing_factory, compatibility: &separate_compatibility, remove_test_base: &fail_rescan, ..loop_inputs() }),
                Err(SigningError::Fatal(e)) if e.phase == "policy")
            );
            assert_eq!(calls.get(), 1);
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
                .scan_data_image(
                    DataImage::default(),
                    &apks,
                    DataImageScanInputs {
                        certificates: Default::default(),
                        compatibility: &separate_compatibility,
                        ..loop_inputs()
                    },
                )
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
            // Two real image APKs, with GSF failing before the platform factory.
            let raw_factories = Image::load(&apks, &[]).unwrap();
            let mut fallback_settings = scan.owner.settings.clone();
            fallback_settings.disabled_system_packages = fallback_settings.packages.clone();
            for setting in &mut fallback_settings.packages {
                setting.code_path = format!("/data/app/absent/{}", setting.name);
                setting.version_code += 1;
                setting.flags |= 1 << 7;
            }
            let order = [1, 0];
            let fallback_factories = aim_services::package::scan::SystemImagePackages {
                packages: Vec::new(),
                retained_data: order
                    .iter()
                    .map(|&index| {
                        let record = &scan.packages[index].candidate.record;
                        aim_services::package::scan::DisabledSystemMetadata {
                            record: aim_services::package::scan::Record {
                                settings: record.settings.clone(),
                                parsed: record.parsed.clone(),
                                signing: record.signing.clone(),
                                identity: record.identity.clone(),
                                origin: record.origin.clone(),
                            },
                            users: full_users[&record.settings.name].clone(),
                            multi_arch_mismatch: false,
                            alignment_diagnostic: None,
                        }
                    })
                    .collect(),
                retained_code: order
                    .iter()
                    .map(|&index| raw_factories.packages[index].clone())
                    .collect(),
                rejected: Vec::new(),
            };
            let base = raw_factories.packages[1]
                .parsed
                .base_apk_path
                .clone()
                .unwrap();
            let root = fixture.0.clone();
            let bad_factory = Apks {
                files: Box::new(move |path| {
                    Some(if path == base {
                        root.clone()
                    } else {
                        root.join(path.trim_start_matches('/'))
                    })
                }),
                platform: Platform::load(&original, Default::default()).unwrap(),
            };
            let mut fallback_owner =
                aim_services::package::scan::SigningScan::new(&config, &fallback_settings, 36)
                    .unwrap();
            let fallback = fallback_owner
                .scan_data_image(
                    DataImage::default(),
                    &bad_factory,
                    DataImageScanInputs {
                        factories: &fallback_factories,
                        ..loop_inputs()
                    },
                )
                .unwrap();
            assert_eq!(fallback.factory_rejected.len(), 1);
            assert!(
                matches!(&fallback.factory_rejected[0].1, SigningError::Rejected(e) if e.phase == "certificates" && e.message.ends_with("(-103)"))
            );
            assert_eq!(fallback.recovered.len(), 1);
            assert_eq!(
                fallback.recovered[0].candidate.record.settings.name,
                "android"
            );
            assert!(fallback_owner.settings.disabled_system_packages.is_empty());
            assert!(!fallback_owner.loaded_packages().contains_key(&factory.name));
            assert!(fallback_owner.loaded_packages().contains_key("android"));
            assert!(app.join("GSF.apk").exists());
            let root = fixture.0.clone();
            let base = raw_factories.packages[1]
                .parsed
                .base_apk_path
                .clone()
                .unwrap();
            let unmapped_factory = Apks {
                files: Box::new(move |path| {
                    (path != base).then(|| root.join(path.trim_start_matches('/')))
                }),
                platform: Platform::load(&original, Default::default()).unwrap(),
            };
            let mut fatal_owner =
                aim_services::package::scan::SigningScan::new(&config, &fallback_settings, 36)
                    .unwrap();
            assert!(
                matches!(fatal_owner.scan_data_image(DataImage::default(), &unmapped_factory, DataImageScanInputs { factories: &fallback_factories, ..loop_inputs() }), Err(SigningError::Fatal(e)) if e.phase == "certificates")
            );
            assert_eq!(fatal_owner.settings.disabled_system_packages.len(), 1);
            assert!(!fatal_owner.loaded_packages().contains_key("android"));
            let empty_factory = fixture.0.join("empty-factory");
            std::fs::create_dir(&empty_factory).unwrap();
            let bad_path = raw_factories.packages[1].location.path.clone();
            let root = fixture.0.clone();
            let bad_parse = Apks {
                files: Box::new(move |path| {
                    Some(if path == bad_path {
                        empty_factory.clone()
                    } else {
                        root.join(path.trim_start_matches('/'))
                    })
                }),
                platform: Platform::load(&original, Default::default()).unwrap(),
            };
            let mut parse_owner =
                aim_services::package::scan::SigningScan::new(&config, &fallback_settings, 36)
                    .unwrap();
            let parsed = parse_owner
                .scan_data_image(
                    DataImage::default(),
                    &bad_parse,
                    DataImageScanInputs {
                        factories: &fallback_factories,
                        ..loop_inputs()
                    },
                )
                .unwrap();
            assert_eq!(parsed.factory_rejected.len(), 1);
            assert!(
                matches!(&parsed.factory_rejected[0].1, SigningError::Rejected(e) if e.phase == "parse")
            );
            assert_eq!(parsed.recovered.len(), 1);
            assert_eq!(
                parsed.recovered[0].candidate.record.settings.name,
                "android"
            );
            assert!(parse_owner.settings.disabled_system_packages.is_empty());
            let bad_path = raw_factories.packages[1].location.path.clone();
            let root = fixture.0.clone();
            let unmapped_parse = Apks {
                files: Box::new(move |path| {
                    (path != bad_path).then(|| root.join(path.trim_start_matches('/')))
                }),
                platform: Platform::load(&original, Default::default()).unwrap(),
            };
            let mut fatal_parse =
                aim_services::package::scan::SigningScan::new(&config, &fallback_settings, 36)
                    .unwrap();
            assert!(
                matches!(fatal_parse.scan_data_image(DataImage::default(), &unmapped_parse, DataImageScanInputs { factories: &fallback_factories, ..loop_inputs() }), Err(SigningError::Fatal(e)) if e.phase == "parse")
            );
            assert_eq!(fatal_parse.settings.disabled_system_packages.len(), 1);
            assert!(!fatal_parse.loaded_packages().contains_key("android"));
            std::fs::remove_dir(fixture.0.join("empty-factory")).unwrap();
            let fail_later = |_: &aim_services::package::pkg::AndroidPackage, _: bool| {
                attempt.set(attempt.get() + 1);
                Err("system must not query compat".into())
            };
            let repeated = DataImage {
                packages: vec![
                    DataCode {
                        scan_path: valid_candidate.scan_path.clone(),
                        code: raw.clone(),
                    },
                    DataCode {
                        scan_path: "/data/app/not-owner".into(),
                        code: raw.clone(),
                    },
                ],
                rejected: Vec::new(),
            };
            let mut partial = before.clone();
            assert!(
                matches!(partial.scan_data_image(repeated, &apks, DataImageScanInputs { remove_test_base: &fail_later, ..loop_inputs() }), Err(SigningError::Fatal(e)) if e.phase == "data-cleanup")
            );
            assert_eq!(attempt.get(), 0);
            assert!(partial.scanned_user_states(&active.name).is_some());
            assert_eq!(
                partial.scanned_user_states(&active.name),
                Some(&full_users[&active.name])
            );
            assert_ne!(partial, before);
            assert!(data_code.exists());
            // A signing source lost after parsing rejects the data APK, cleans
            // its outer scan path and recovers the retained factory.
            let missing_inventory = DataImage::parse(&apks, &[]).unwrap();
            assert_eq!(missing_inventory.packages.len(), 1);
            std::fs::remove_file(physical_data.join("base.apk")).unwrap();
            let mut missing_owner = before.clone();
            let missing = missing_owner
                .scan_parsed_data_image(missing_inventory, &apks, loop_inputs())
                .unwrap();
            assert!(missing.packages.is_empty());
            assert_eq!(missing.removed.len(), 1);
            assert!(
                matches!(&missing.removed[0].1, SigningError::Rejected(e) if e.phase == "certificates" && e.message.ends_with("(-103)"))
            );
            assert_eq!(missing.recovered.len(), 1);
            assert!(!data_code.exists());
            assert_eq!(
                missing.recovered[0].candidate.record.settings.code_path,
                factory.code_path
            );
            std::fs::write(&data_code, b"disposable retained code").unwrap();
            std::os::unix::fs::symlink(
                original.join(
                    "system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk",
                ),
                physical_data.join("base.apk"),
            )
            .unwrap();
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
        seinfo: common::seinfo::scan(),
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
    let mut independent_code = code.clone();
    independent_code.parsed.shared_user_id = None;
    let mut independent_settings = scan.owner.settings.clone();
    let independent_setting = independent_settings
        .packages
        .iter_mut()
        .find(|package| package.name == saved.name)
        .unwrap();
    independent_setting.shared_user = false;
    independent_setting.app_id = 19001;
    let mut independent =
        aim_services::package::scan::SigningScan::new(&config, &independent_settings, 36).unwrap();
    independent
        .scan_existing(
            &independent_code,
            update(),
            &saved_users,
            None,
            None,
            &apks,
            completion(),
        )
        .unwrap();
    let independent_before = independent.clone();
    let queried = std::cell::Cell::new(0);
    let unavailable = |package: &aim_services::package::pkg::AndroidPackage| {
        assert_eq!(package.package_name, saved.name);
        queried.set(queried.get() + 1);
        Err("original compatibility owner unavailable".into())
    };
    let mut denied = completion();
    denied.seinfo.compatibility = &unavailable;
    assert!(matches!(
        independent.scan_existing(&independent_code, update(), &saved_users, None, None, &apks, denied),
        Err(SigningError::Rejected(ref error)) if error.phase == "seinfo"
            && error.message == "original compatibility owner unavailable"
    ));
    assert_eq!(queried.get(), 1);
    assert_eq!(independent, independent_before);
    assert_eq!(scan.owner, before);
    for name in scan.owner.loaded_packages().keys() {
        assert!(
            scan.owner
                .seinfo_state(name)
                .unwrap()
                .unwrap()
                .base
                .is_some()
        );
    }
    let mut mismatched_owner = scan.owner.clone();
    let mut mismatched = mismatched_owner
        .apply_existing(&code, update(), &saved_users, None, None)
        .unwrap();
    let before_finalization = mismatched_owner.clone();
    assert!(
        matches!(captures.publish(&original_capture, mismatched_owner.clone(), original_capture.usage().clone()),
        Err(aim_services::package::scan_snapshot::Error::Invalid(ref message))
            if message == "scan metadata is not finalized")
    );
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
    let mut next_usage = original_capture.usage().clone();
    next_usage.notify("android", 0, 29);
    let next_capture = captures
        .publish(&original_capture, scan.owner.clone(), next_usage)
        .unwrap();
    assert_eq!(next_capture.version(), 2);
    assert_eq!(original_capture.usage().latest("android"), Some(17));
    assert_eq!(next_capture.usage().latest("android"), Some(29));
    assert_eq!(original_capture.owner(), &before);
    assert_eq!(next_capture.owner(), &scan.owner);
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
    let uncollected_factory = aim_services::package::scan::Code {
        location: factory_code.location.clone(),
        parsed: factory_code.parsed.clone(),
        signing: (),
    };
    let factory_before = factory_owner.clone();
    assert!(matches!(
        factory_owner.scan_disabled_system(&factory_code, update(), None, &unreadable, completion()),
        Err(SigningError::Rejected(ref error)) if error.phase == "code-time"
    ));
    assert_eq!(factory_owner, factory_before);
    let factory = factory_owner
        .scan_disabled_system(&uncollected_factory, update(), None, &apks, completion())
        .unwrap();
    assert_eq!(
        factory.record.settings.domain_set_id.as_deref(),
        Some("63636363-6363-6363-6363-636363636363")
    );
    assert_eq!(factory.record.settings.signatures, saved.signatures);
    assert!(factory.record.signing.unknown);
    assert!(factory.record.parsed.signing_details.is_none());
    let loaded_factory = &factory_owner.disabled_loaded_packages()[&saved.name];
    assert!(loaded_factory.collected_signing.unknown);
    assert!(
        loaded_factory
            .facade_entry()
            .unwrap()
            .past_signing_certificates
            .is_none()
    );
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

    let mut disabling = scan.owner.clone();
    let active_before = disabling.settings.packages.clone();
    let identities_before = disabling.identities.clone();
    let loaded_before = disabling.loaded_packages()[&saved.name].clone();
    assert!(!disabling.disable_system_package("missing-factory").unwrap());
    assert!(disabling.disable_system_package(&saved.name).unwrap());
    assert_eq!(
        disabling.settings.disabled_system_packages,
        vec![retained.candidate.record.settings.clone()]
    );
    assert!(
        !disabling.settings.disabled_system_packages[0]
            .transient
            .updated_system_app
    );
    let mut expected_active = active_before;
    expected_active
        .iter_mut()
        .find(|p| p.name == saved.name)
        .unwrap()
        .transient
        .updated_system_app = true;
    assert_eq!(disabling.settings.packages, expected_active);
    assert_eq!(disabling.identities, identities_before);
    assert!(std::sync::Arc::ptr_eq(
        &loaded_before,
        &disabling.loaded_packages()[&saved.name]
    ));
    assert!(std::sync::Arc::ptr_eq(
        &loaded_before,
        &disabling.disabled_loaded_packages()[&saved.name]
    ));
    assert_eq!(
        disabling.disabled_user_states(&saved.name),
        Some(&retained.candidate.users)
    );
    let disabled_before = disabling.clone();
    assert!(!disabling.disable_system_package(&saved.name).unwrap());
    assert_eq!(disabling, disabled_before);
    disabling
        .set_user_state(
            &saved.name,
            0,
            aim_services::package::restrictions::UserState {
                first_install_time: 314,
                ..retained.candidate.users[&0].clone()
            },
        )
        .unwrap();
    assert_eq!(
        disabling.disabled_user_states(&saved.name).unwrap()[&0].first_install_time,
        314
    );
    disabling
        .set_user_state(&saved.name, 20, Default::default())
        .unwrap();
    assert!(
        !disabling
            .disabled_user_states(&saved.name)
            .unwrap()
            .contains_key(&20)
    );

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
        certificates: Default::default(),
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
                    &uncollected_factory,
                    update(),
                    None,
                    &policy,
                    &apks,
                    selection_inputs(),
                )
                .unwrap();
            assert_eq!(selected.source, source);
            assert!(selected.factory.record.signing.unknown);
            assert!(selected.factory.record.parsed.signing_details.is_none());
            assert!(
                owner.disabled_loaded_packages()[&saved.name]
                    .collected_signing
                    .unknown
            );
            if source == KeepData {
                let mut cached_code = factory_code.clone();
                cached_code.signing.current_flags = vec![37; cached_code.signing.signatures.len()];
                let mut cached_owner = before.clone();
                let refreshed = cached_owner
                    .scan_updated_system(
                        &cached_code,
                        update(),
                        None,
                        &policy,
                        &apks,
                        selection_inputs(),
                    )
                    .unwrap();
                assert_eq!(
                    refreshed.factory.record.settings.signatures,
                    if strict {
                        saved.signatures.clone()
                    } else {
                        stale_factory.signatures.clone()
                    }
                );
                if strict {
                    // A directory still has a timestamp, so metadata refresh
                    // succeeds; full certificate collection must reject it.
                    cached_code.parsed.base_apk_path = cached_code.parsed.path.clone();
                    let mut metadata_only = before.clone();
                    metadata_only
                        .scan_updated_system(
                            &cached_code,
                            update(),
                            None,
                            &SystemConfig::default(),
                            &apks,
                            selection_inputs(),
                        )
                        .unwrap();
                    let mut failed = before.clone();
                    assert!(
                        matches!(failed.scan_updated_system(&cached_code, update(), None, &policy, &apks, selection_inputs()), Err(SigningError::Rejected(e)) if e.phase == "system-source" && e.message.ends_with("(-110)"))
                    );
                    assert_eq!(failed, metadata_only);
                    assert_ne!(failed, before);
                }
            }
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
                // Collection occurs after resource removal/enable, and ignores
                // a precollected input's flags when the old data path differs.
                let mut cached_code = factory_code.clone();
                cached_code.signing.current_flags = vec![37; cached_code.signing.signatures.len()];
                let mut cached_raw = raw_factory.clone();
                cached_raw.signing.current_flags = cached_code.signing.current_flags.clone();
                let mut cached_owner =
                    aim_services::package::scan::SigningScan::new(&config, &settings, 36).unwrap();
                let cached_selected = cached_owner
                    .scan_updated_system(
                        &cached_code,
                        update(),
                        None,
                        &policy,
                        &apks,
                        selection_inputs(),
                    )
                    .unwrap();
                std::fs::write(&disposable_code, b"disposable replaced code").unwrap();
                let aim_services::package::scan::UpdatedSystemBootOutcome::Factory(collected) =
                    cached_owner
                        .complete_updated_system_boot(
                            &cached_selected,
                            &cached_raw,
                            &saved_users,
                            None,
                            &apks,
                            restore_inputs(),
                        )
                        .unwrap()
                else {
                    panic!("factory restoration retained data");
                };
                assert_eq!(collected.candidate.record.signing, raw_factory.signing);
                assert!(!disposable_code.exists());
                let base = raw_factory.parsed.base_apk_path.clone().unwrap();
                let root = fixture.0.clone();
                let bad_source = Apks {
                    files: Box::new(move |path| {
                        Some(if path == base {
                            root.clone()
                        } else {
                            root.join(path.trim_start_matches('/'))
                        })
                    }),
                    platform: Platform::load(&original, Default::default()).unwrap(),
                };
                std::fs::write(&disposable_code, b"disposable replaced code").unwrap();
                let mut failed_collection = before.clone();
                assert!(
                    matches!(failed_collection.complete_updated_system_boot(&selected, &raw_factory, &saved_users, None, &bad_source, restore_inputs()), Err(SigningError::Rejected(e)) if e.phase == "certificates" && e.message.ends_with("(-103)"))
                );
                assert!(!disposable_code.exists());
                assert!(
                    failed_collection
                        .settings
                        .disabled_system_packages
                        .is_empty()
                );
                assert_eq!(failed_collection.identities, ids);
                assert_eq!(failed_collection.libraries, before.libraries);
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

    let apex_host = std::fs::read_dir(original.join("system/apex"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "apex"))
        .unwrap();
    let apex_path = format!(
        "/system/apex/{}",
        apex_host.file_name().unwrap().to_str().unwrap()
    );
    std::fs::create_dir_all(fixture.0.join("system/apex")).unwrap();
    std::os::unix::fs::symlink(
        &apex_host,
        fixture.0.join(apex_path.trim_start_matches('/')),
    )
    .unwrap();
    let inventory = aim_services::package::bootstrap::ApexInventory {
        packages: Some(vec![aim_services::package::bootstrap::ApexPackage {
            module_name: None,
            module_path: apex_path.clone(),
            preinstalled_path: apex_path,
            version_code: 0,
            factory: true,
            active: true,
            active_changed: false,
        }]),
        active: Vec::new(),
    };
    let verified_apex = aim_services::package::scan::ApexImage::load(
        &apks,
        &inventory,
        aim_services::package::parse::PARSE_IS_SYSTEM_DIR,
    )
    .unwrap();
    let parsed_apex = &verified_apex.packages[0].parsed;
    let mut with_apex = inputs(&domain_ids);
    with_apex.apex_image = &verified_apex;
    let image_reads = std::cell::Cell::new(0);
    let scan = SystemImageScan::first_boot(
        || {
            assert_eq!(notified_apex.get(), 1);
            image_reads.set(image_reads.get() + 1);
            Image::load(&apks, &[])
        },
        &apks,
        &config,
        with_apex,
    )
    .unwrap();
    assert_eq!(image_reads.get(), 1);
    assert_eq!(scan.packages[1].candidate.record.settings.app_id, 10000);
    let registered = &scan.owner.settings.packages[0];
    assert_eq!(registered.name, parsed_apex.package_name);
    assert_eq!(registered.app_id, -1);
    assert_eq!(
        registered.code_path,
        verified_apex.packages[0].info.module_path
    );
    assert!(
        scan.owner.loaded_packages()[&registered.name]
            .package
            .is2(aim_services::package::pkg::booleans2::APEX)
    );
    assert_eq!(
        scan.owner.loaded_packages()[&registered.name].package.uid,
        -1
    );
    assert_eq!(scan.apex.len(), 1);
    assert_eq!(notified_apex.get(), 1);
    assert!(registered.signatures.is_some());
    assert_eq!(registered.key_set_data, Default::default());
    assert_eq!(registered.primary_cpu_abi, None);
    assert_eq!(registered.legacy_native_library_path, None);

    let mut shared_apex = aim_services::package::scan::ApexImage {
        packages: verified_apex.packages.clone(),
    };
    shared_apex.packages[0].parsed.shared_user_id = Some("fixture.apex.shared".into());
    let mut shared_inputs = inputs(&domain_ids);
    shared_inputs.apex_image = &shared_apex;
    let shared_scan =
        SystemImageScan::first_boot(|| Image::load(&apks, &[]), &apks, &config, shared_inputs)
            .unwrap();
    assert_eq!(shared_scan.owner.settings.packages[0].app_id, 10000);
    assert_eq!(
        shared_scan.owner.settings.packages[0].shared_app_id(),
        Some(10000)
    );
    assert_eq!(
        shared_scan.packages[1].candidate.record.settings.app_id,
        10001
    );
    assert_eq!(shared_scan.apex[0].package.uid, -1);
    assert_eq!(
        shared_scan.owner.identities.shared_users["fixture.apex.shared"].seinfo_target_sdk(),
        shared_scan.apex[0].package.target_sdk_version
    );

    let reject_notification = |_: &[aim_services::package::scan::ApexScanResult]| {
        Err("original owner denied scan results".into())
    };
    let mut rejected_notification = inputs(&domain_ids);
    rejected_notification.apex_image = &verified_apex;
    rejected_notification.notify_apex_scan = &reject_notification;
    let domains_before = next_id.load(Ordering::SeqCst);
    image_reads.set(0);
    assert!(matches!(SystemImageScan::first_boot(|| {
            image_reads.set(image_reads.get() + 1);
            Image::load(&apks, &[])
        }, &apks, &config,
        rejected_notification), Err(SigningError::Rejected(ref error)) if error.phase == "apex-notification"));
    assert_eq!(image_reads.get(), 0);
    // One domain belongs to the completed container; no APK admission followed.
    assert_eq!(next_id.load(Ordering::SeqCst), domains_before + 1);

    let domains_before = next_id.load(Ordering::SeqCst);
    notified_apex.set(0);
    let mut failed_image = inputs(&domain_ids);
    failed_image.apex_image = &verified_apex;
    assert!(matches!(SystemImageScan::first_boot(|| {
        assert_eq!(notified_apex.get(), 1);
        Err(aim_services::package::scan::Error {
            package: String::new(), path: "/system/framework".into(),
            phase: "directory", message: "APK image read failed".into(),
        })
    }, &apks, &config, failed_image), Err(SigningError::Rejected(ref error))
        if error.phase == "directory" && error.path == "/system/framework"
            && error.message == "APK image read failed"));
    assert_eq!(next_id.load(Ordering::SeqCst), domains_before + 1);

    let mut early = Image::load(&apks, &[]).unwrap();
    let mut overlay = early.packages.remove(1);
    overlay.location.kind = aim_services::package::scan::Kind::Overlay;
    overlay.parsed.shared_user_id = Some("android.uid.system".into());
    early.packages.insert(0, overlay);
    assert!(
        matches!(SystemImageScan::first_boot(|| Ok(early), &apks, &config, inputs(&domain_ids)), Err(SigningError::Rejected(ref error)) if error.phase == "policy" && error.message.contains("scanned platform"))
    );
    assert!(
        apks.platform
            .framework_boolean("fixture_missing_boolean")
            .is_err()
    );

    let fail_domain = || Err("domain owner failure".into());
    assert!(
        matches!(SystemImageScan::first_boot(|| Image::load(&apks, &[]), &apks, &config, inputs(&fail_domain)), Err(SigningError::Rejected(ref error)) if error.phase == "domain")
    );
    let mut image = Image::load(&apks, &[]).unwrap();
    image
        .packages
        .retain(|code| code.parsed.package_name != "android");
    assert!(
        matches!(SystemImageScan::first_boot(|| Ok(image), &apks, &config, inputs(&domain_ids)), Err(SigningError::Rejected(ref error)) if error.phase == "framework")
    );
    assert_eq!(std::fs::read_dir(&framework).unwrap().count(), 1);
    assert_eq!(std::fs::read_dir(&app).unwrap().count(), 1);
}
