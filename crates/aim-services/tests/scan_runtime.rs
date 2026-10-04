//! Reconcile the saved image package set against the running original PMS.
//! This diagnostic uses persisted order, not the complete boot scan selector.
use aim_services::package::{
    State,
    libraries::TYPE_STATIC,
    parse::Platform,
    scan::{
        AbiPolicy, AbiScanContext, AbiScanMode, Apex, Code, FirstBootSystemInputs, Image, Inputs,
        Kind, LibraryCompatibility, Location, NativeLibraryEnvironment, NativeLibraryInstallPolicy,
        Partition, ScanClock, ScanMetadataCompletion, ScanPolicy, SettingUpdate, SigningScan,
        SupportedAbis, SystemImageScan, UpdatedSystemSource, UserPolicy, application_flags,
    },
    system_config::SystemConfig,
    write::Apks,
};
use std::collections::BTreeMap;
use std::fs;
use std::process::Command;
use std::time::{Duration, Instant};

mod common {
    pub mod java;
    pub mod runtime;
    pub mod seinfo;
}
use common::runtime::{Boot, Data, run};

#[test]
#[ignore = "requires aimctl and the pinned derived image; run explicitly"]
fn saved_scan_libraries_match_original_pms() {
    let dir = std::env::temp_dir().join(format!("aim-scan-runtime-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let data = Data(dir);
    let java = aim_paths::fetched().join("java");
    let jdk = java.join("temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let classes = data.0.join("classes");
    let stubs = data.0.join("stubs");
    let dex = data.0.join("dex");
    for path in [&classes, &stubs, &dex] {
        fs::create_dir(path).unwrap();
    }
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(common::java::sources(
            &aim_paths::root().join("java/device-services/stubs"),
        )));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/BootScanOracle.java"),
        ));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(java.join("build-tools-36.0.0/android-16/lib/d8.jar"))
        .args([
            "com.android.tools.r8.D8",
            "--release",
            "--min-api",
            "36",
            "--lib",
        ])
        .arg(&jdk)
        .arg("--classpath")
        .arg(&stubs)
        .arg("--output")
        .arg(&dex)
        .arg(classes.join("com/android/server/BootScanOracle.class")));
    common::java::check_linkage(
        &dex.join("classes.dex"),
        &["/system/framework/services.jar"],
    )
    .unwrap();
    let boot = Boot {
        ctl: aim_paths::root().join("target/release/aimctl"),
        data: data.0.join("guest"),
    };
    run(boot.command().args(["start", "--windows"]));
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let result = boot
            .command()
            .args(["shell", "getprop", "sys.boot_completed"])
            .output()
            .unwrap();
        if result.status.success() && String::from_utf8_lossy(&result.stdout).trim() == "1" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "original PMS boot did not complete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    let properties =
        String::from_utf8(run(boot.command().args(["shell", "getprop"])).stdout).unwrap();
    let properties: BTreeMap<_, _> = properties
        .lines()
        .filter_map(|line| {
            let (name, value) = line
                .strip_prefix('[')?
                .strip_suffix(']')?
                .split_once("]: [")?;
            Some((name.to_owned(), value.to_owned()))
        })
        .collect();
    fs::copy(
        dex.join("classes.dex"),
        boot.data.join("data/local/tmp/boot-scan.dex"),
    )
    .unwrap();
    let boot_policy = String::from_utf8(
        run(boot.command().args([
            "shell",
            "/system/bin/app_process",
            "-Djava.class.path=/data/local/tmp/boot-scan.dex:/system/framework/services.jar",
            "/system/bin",
            "com.android.server.BootScanOracle",
        ]))
        .stdout,
    )
    .unwrap();
    let mut boot_policy = boot_policy.lines();
    let test_base_on_bcp: bool = boot_policy.next().unwrap().parse().unwrap();
    let stop_system_packages: bool = boot_policy.next().unwrap().parse().unwrap();
    let initial_non_stopped: std::collections::BTreeSet<_> =
        boot_policy.map(str::to_owned).collect();
    let image = aim_paths::derived_image();
    let config = SystemConfig::read(&image, &|name| properties.get(name).cloned());
    assert_eq!(
        config.initial_non_stopped_system_packages,
        initial_non_stopped
    );
    let strict_signatures = String::from_utf8(
        run(boot.command().args([
            "shell",
            "/system/bin/app_process",
            "-Djava.class.path=/data/local/tmp/boot-scan.dex:/system/framework/services.jar",
            "/system/bin",
            "com.android.server.BootScanOracle",
            "strict-signatures",
        ]))
        .stdout,
    )
    .unwrap();
    assert_eq!(
        config.preinstall_packages_with_strict_signature_check,
        strict_signatures.lines().map(str::to_owned).collect()
    );
    let factory_users = String::from_utf8(
        run(boot.command().args([
            "shell",
            "/system/bin/app_process",
            "-Djava.class.path=/data/local/tmp/boot-scan.dex:/system/framework/services.jar",
            "/system/bin",
            "com.android.server.BootScanOracle",
            "factory-users",
        ]))
        .stdout,
    )
    .unwrap();
    assert_eq!(
        factory_users.lines().collect::<Vec<_>>(),
        ["0", "456", "456"]
    );
    let code_parent = boot.data.join("data/app/~~aim-cleanup-proof");
    let code_child = code_parent.join("com.aim.cleanup-test");
    fs::create_dir_all(code_child.join("lib/arm64")).unwrap();
    fs::write(code_child.join("base.apk"), b"disposable replaced code").unwrap();
    fs::write(
        code_child.join("lib/arm64/libfixture.so"),
        b"disposable library",
    )
    .unwrap();
    let protected = boot.data.join("data/local/tmp/aim-cleanup-invalid");
    fs::create_dir(&protected).unwrap();
    fs::write(protected.join("keep"), b"unrelated data").unwrap();
    let installer = String::from_utf8(
        run(boot.client(1000).args([
            "/system/bin/app_process",
            "-Djava.class.path=/data/local/tmp/boot-scan.dex:/system/framework/services.jar",
            "/system/bin",
            "com.android.server.BootScanOracle",
            "installer-cleanup",
        ]))
        .stdout,
    )
    .unwrap();
    assert_eq!(
        installer.lines().collect::<Vec<_>>(),
        ["child-removed", "parent-removed", "invalid-root-rejected"]
    );
    assert!(!code_parent.exists());
    assert!(protected.join("keep").exists());
    let app_data = String::from_utf8(
        run(boot.client(1000).args([
            "/system/bin/app_process",
            "-Djava.class.path=/data/local/tmp/boot-scan.dex:/system/framework/services.jar",
            "/system/bin",
            "com.android.server.BootScanOracle",
            "installer-app-data",
        ]))
        .stdout,
    )
    .unwrap();
    assert_eq!(
        app_data.lines().collect::<Vec<_>>(),
        [
            "app-data-removed",
            "app-data-retry",
            "invalid-user-rejected"
        ]
    );
    assert!(protected.join("keep").exists());
    let mut platform = Platform::load(&image, Default::default()).unwrap();
    let density =
        String::from_utf8(run(boot.command().args(["shell", "wm", "density"])).stdout).unwrap();
    platform.density_dpi = Some(
        density
            .lines()
            .filter_map(|line| {
                line.strip_prefix("Physical density: ")
                    .or_else(|| line.strip_prefix("Override density: "))
                    .map(|n| n.parse().unwrap())
            })
            .last()
            .expect("original display density"),
    );
    let output = String::from_utf8(
        run(boot
            .command()
            .args(["shell", "dumpsys", "package", "libraries"]))
        .stdout,
    )
    .unwrap();
    let apex_xml = run(boot
        .command()
        .args(["shell", "cat", "/apex/apex-info-list.xml"]));
    let apex_xml = aim_android_xml::read(&apex_xml.stdout).unwrap();
    let apexes: Vec<_> = apex_xml
        .children()
        .filter(|e| e.name == "apex-info")
        .filter(|e| e.attr("isActive").unwrap().string().unwrap() == "true")
        .map(|e| {
            let attr = |name| e.attr(name).unwrap().string().unwrap().into_owned();
            let original = attr("preinstalledModulePath");
            let (partition, _) = partition_path(&original).unwrap();
            Apex {
                module_name: Some("module".into()),
                mount_path: format!("/apex/{}", attr("moduleName")),
                partition,
                factory: attr("isFactory") == "true",
                // This diagnostic does not select changed APEX versions.
                active_changed: false,
            }
        })
        .collect();
    // Freeze only this owned data image before comparing disk state.
    run(boot.command().arg("stop"));
    let volume = aim_storage::data::DataImage::attach(&boot.data, None).unwrap();
    let original = State::read_with_config(&boot.data.join("data"), &[0], &config)
        .unwrap()
        .unwrap();
    assert_eq!(original.settings.packages.len(), 243);
    assert_eq!(original.settings.shared_users.len(), 16);
    let data_files = boot.data.join("data");
    let apks = Apks {
        files: Box::new(move |path| {
            Some(if let Some(relative) = path.strip_prefix("/data/") {
                data_files.join(relative)
            } else {
                image.join(path.trim_start_matches('/'))
            })
        }),
        platform,
    };
    let inputs = Inputs::load(&original, &apks).unwrap();
    assert_eq!(inputs.active.len(), original.settings.packages.len());
    assert_eq!(
        inputs.disabled.len(),
        original.settings.disabled_system_packages.len()
    );
    let first_api = properties
        .get("ro.product.first_api_level")
        .map(|n| n.parse().unwrap())
        .unwrap_or(0);
    let mut scan = SigningScan::new(&config, &original.settings, first_api).unwrap();
    scan.restore_legacy_permissions_from_data(&boot.data.join("data"), &original, &config)
        .unwrap();
    let platform_signing = &inputs.active["android"].signing;
    let vendor_sdk = properties
        .get("ro.vndk.version")
        .map(|v| {
            v.parse().unwrap_or_else(|_| {
                if apks.platform.codenames.contains(v) {
                    10000
                } else {
                    28
                }
            })
        })
        .unwrap_or(28);
    assert_eq!(
        apks.platform
            .framework_boolean("config_stopSystemPackagesByDefault")
            .unwrap(),
        stop_system_packages
    );
    let abi_list = |key| {
        properties
            .get(key)
            .filter(|v| !v.is_empty())
            .map(|v| v.split(',').map(str::to_owned).collect::<Vec<_>>())
            .unwrap_or_default()
    };
    let bit32 = abi_list("ro.product.cpu.abilist32");
    let bit64 = abi_list("ro.product.cpu.abilist64");
    let all_abis = abi_list("ro.product.cpu.abilist");
    let abi_policy = AbiPolicy::from_platform(
        &apks.platform,
        &all_abis,
        &SupportedAbis {
            bit32: &bit32,
            bit64: &bit64,
        },
        &|name| properties.get(name).cloned(),
    )
    .unwrap();
    let compatibility = LibraryCompatibility::new(
        &config,
        &|name| properties.get(name).cloned(),
        test_base_on_bcp,
    )
    .unwrap();
    let system_image = Image::load(&apks, &apexes).unwrap();
    let domain_sequence = std::sync::atomic::AtomicU32::new(1);
    let domain_ids = || {
        let mut id = [0; 16];
        id[..4].copy_from_slice(
            &domain_sequence
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                .to_be_bytes(),
        );
        Ok(id)
    };
    let mut updated_scan = SigningScan::new(&config, &original.settings, first_api).unwrap();
    updated_scan
        .restore_legacy_permissions_from_data(&boot.data.join("data"), &original, &config)
        .unwrap();
    for saved in &original.settings.disabled_system_packages {
        let image_code = system_image
            .packages
            .iter()
            .find(|code| code.location.path == saved.code_path)
            .expect("disabled factory code remains in the physical image");
        let mut code = Code {
            location: image_code.location.clone(),
            parsed: image_code.parsed.clone(),
            signing: image_code.signing.clone(),
        };
        let mut policy = ScanPolicy::for_location(&code.location);
        policy.adjust_shared_uid_privilege(
            &code.parsed,
            &code.signing,
            platform_signing,
            &updated_scan.identities,
            vendor_sdk,
        );
        policy
            .apply(
                &mut code.parsed,
                &code.signing,
                Some(platform_signing),
                true,
                &apks,
                &compatibility,
                None,
            )
            .unwrap();
        let updated = saved.transient.updated_system_app;
        let (flags, private_flags) = application_flags(&code.parsed, updated);
        let environment = NativeLibraryEnvironment {
            preferred_abi: all_abis.first().unwrap(),
            app_lib32_install_dir: "/data/app-lib",
            code_is_directory: fs::metadata((apks.files)(&saved.code_path).unwrap())
                .unwrap()
                .is_dir(),
            canonical_source: None,
        };
        let before = updated_scan.clone();
        let selected = updated_scan
            .scan_updated_system(
                &code,
                SettingUpdate {
                    code_path: saved.code_path.clone(),
                    legacy_native_library_path: None,
                    primary_cpu_abi: saved.primary_cpu_abi.clone(),
                    secondary_cpu_abi: saved.secondary_cpu_abi.clone(),
                    flags,
                    private_flags,
                    uses_sdk_libraries: saved.uses_sdk_libraries.clone(),
                    uses_static_libraries: saved.uses_static_libraries.clone(),
                    mime_groups: code.parsed.mime_groups.clone(),
                    domain_set_id: domain_ids().unwrap(),
                    target_sdk_version: code.parsed.target_sdk_version,
                    restrict_update_hash: code.parsed.restrict_update_hash.clone(),
                },
                None,
                &config,
                &apks,
                ScanMetadataCompletion {
                    seinfo: common::seinfo::scan(),
                    abi_policy: &abi_policy,
                    native_environment: &environment,
                    context: AbiScanContext {
                        mode: AbiScanMode::Existing {
                            first_boot_or_upgrade: true,
                            old_was_stub: false,
                            saved: Some(saved),
                        },
                        system: true,
                        updated,
                        override_abi: None,
                        platform_runtime_64bit: None,
                    },
                    install: NativeLibraryInstallPolicy {
                        page_size: 16384,
                        extract: false,
                        debuggable: false,
                        compat_16kb_disabled: false,
                        manifest_compat_disabled: false,
                    },
                    destination: None,
                    clock: ScanClock {
                        current_time: 0,
                        user_id: 0,
                        update_time: false,
                    },
                    factory_test: false,
                },
            )
            .unwrap();
        assert_eq!(
            selected.source,
            UpdatedSystemSource::KeepData,
            "{}",
            saved.name
        );
        assert_eq!(selected.factory.record.settings.app_id, saved.app_id);
        assert_eq!(selected.factory.users[&0].first_install_time, -1);
        assert_eq!(selected.factory.record.settings.last_update_time, -1);
        assert_eq!(
            updated_scan.disabled_user_states(&saved.name),
            Some(&selected.factory.users)
        );
        assert_eq!(updated_scan.settings.packages, before.settings.packages);
        assert_eq!(updated_scan.identities, before.identities);
        assert_eq!(updated_scan.libraries, before.libraries);
        eprintln!(
            "native factory refresh keeps original data source: {}",
            saved.name
        );
    }
    let mut saved_users = BTreeMap::new();
    for (id, user) in &original.users {
        for (name, state) in &user.restrictions.packages {
            saved_users
                .entry(name.clone())
                .or_insert_with(BTreeMap::new)
                .insert(*id as i32, state.clone());
        }
    }
    let empty_packages = std::collections::BTreeSet::new();
    let native_driver = aim_binder_driver::Driver::new();
    let native_process = aim_binder_host::local::LocalProcess::open(
        &native_driver,
        aim_binder_driver::Device::Binder,
        aim_binder_driver::Credentials {
            pid: std::process::id() as i32,
            euid: 1000,
            security_context: None,
        },
    );
    let resource_root = data.0.join("recovery-data");
    fs::create_dir(&resource_root).unwrap();
    let resources = aim_services::package::owner::resources::CodeResources::new(
        native_process,
        resource_root,
        None,
    );
    let mut resumed = SigningScan::new(&config, &original.settings, first_api).unwrap();
    resumed
        .restore_legacy_permissions_from_data(&boot.data.join("data"), &original, &config)
        .unwrap();
    let resumed_packages = resumed
        .scan_saved_system_image(
            Image::load(&apks, &apexes).unwrap(),
            &apks,
            &config,
            FirstBootSystemInputs {
                certificates: Default::default(),
                seinfo: common::seinfo::scan(),
                apex_image: &Default::default(),
                notify_apex_scan: &|results| {
                    assert!(results.is_empty());
                    Ok(())
                },
                shared_uid_migration:
                    aim_services::package::scan::SharedUidMigration::NewInstallOnly,
                first_api_level: first_api,
                vendor_sdk,
                abi_policy: &abi_policy,
                compatibility: &compatibility,
                preferred_abi: all_abis.first().unwrap(),
                app_lib32_install_dir: "/data/app-lib",
                platform_runtime_64bit: true,
                install: NativeLibraryInstallPolicy {
                    page_size: 16384,
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
                new_domain_id: &domain_ids,
            },
            aim_services::package::scan::SavedSystemScanInputs {
                users: &saved_users,
                first_boot_or_upgrade: false,
                old_stub_packages: &empty_packages,
                incremental_packages: &empty_packages,
                resources: &resources,
            },
        )
        .unwrap();
    assert_eq!(resumed_packages.packages.len(), 240);
    assert_eq!(resumed_packages.retained_data.len(), 3);
    for factory in &resumed_packages.retained_data {
        let name = &factory.record.settings.name;
        assert_eq!(
            resumed.settings.packages.iter().find(|p| &p.name == name),
            original.settings.packages.iter().find(|p| &p.name == name)
        );
    }
    for completed in &resumed_packages.packages {
        let candidate = &completed.candidate;
        let saved = original
            .settings
            .packages
            .iter()
            .find(|p| p.name == candidate.record.settings.name)
            .unwrap();
        assert_eq!(candidate.record.settings.app_id, saved.app_id);
        assert_eq!(candidate.record.settings.code_path, saved.code_path);
        assert_eq!(candidate.record.settings.version_code, saved.version_code);
        assert_eq!(candidate.record.settings.key_set_data, saved.key_set_data);
        assert_eq!(candidate.users, saved_users[&saved.name]);
    }
    let data_image = aim_services::package::scan::DataImage::load(&apks, &[]).unwrap();
    assert!(data_image.rejected.is_empty());
    assert_eq!(data_image.packages.len(), 3);
    let destinations = BTreeMap::new();
    let resumed_data = resumed
        .scan_data_image(
            data_image,
            &apks,
            aim_services::package::scan::DataImageScanInputs {
                certificates: Default::default(),
                seinfo: common::seinfo::scan(),
                factories: &resumed_packages,
                platform: platform_signing,
                vendor_sdk,
                abi_policy: &abi_policy,
                compatibility: &compatibility,
                preferred_abi: all_abis.first().unwrap(),
                app_lib32_install_dir: "/data/app-lib",
                install: NativeLibraryInstallPolicy {
                    page_size: 16384,
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
                users: &saved_users,
                all_users: None,
                first_boot_or_upgrade: false,
                old_stub_packages: &empty_packages,
                expecting_better: &empty_packages,
                is_incremental: &|_| Ok(false),
                remove_test_base: &|_, _| Err("system/BCP must not query compat".into()),
                destinations: &destinations,
                resources: &resources,
                new_domain_id: &domain_ids,
            },
        )
        .unwrap();
    assert_eq!(resumed_data.packages.len(), 3);
    let mut usage = aim_services::package::owner::usage::Usage::new(
        resumed.settings.packages.iter().map(|p| p.name.as_str()),
    );
    usage
        .read(&boot.data.join("data/system/package-usage.list"))
        .unwrap();
    let captures =
        aim_services::package::scan_snapshot::Store::new(resumed.clone(), usage.clone()).unwrap();
    let capture = captures.capture();
    assert_eq!(capture.owner(), &resumed);
    assert_eq!(capture.usage(), &usage);
    assert_eq!(resumed.loaded_packages().len(), 243);
    assert_eq!(resumed.disabled_loaded_packages().len(), 3);
    for name in resumed.loaded_packages().keys() {
        let state = resumed.seinfo_state(name).unwrap().unwrap();
        assert!(state.base.is_some(), "unfinished scan seInfo: {name}");
        assert!(
            state.override_label.is_none(),
            "boot override ran during scan: {name}"
        );
        assert_eq!(capture.owner().seinfo_state(name).unwrap(), Some(state));
    }
    for completed in resumed_packages
        .packages
        .iter()
        .chain(&resumed_data.packages)
        .chain(&resumed_data.recovered)
    {
        let record = &completed.candidate.record;
        let loaded = &resumed.loaded_packages()[&record.settings.name];
        assert!(std::sync::Arc::ptr_eq(
            loaded,
            &capture.owner().loaded_packages()[&record.settings.name]
        ));
        assert_eq!(loaded.collected_signing, record.signing);
        assert_eq!(
            loaded.facade_entry().unwrap().past_signing_certificates,
            record.signing.past_signing_certificates
        );
        assert_eq!(record.parsed.uid, record.settings.app_id);
        assert!(record.parsed.signing_details == Some(record.signing.parcel_details().unwrap()));
        assert_eq!(
            resumed.loaded_packages()[&record.settings.name].package,
            record.parsed
        );
    }
    for factory in &resumed_packages.retained_data {
        let record = &factory.record;
        let loaded = &resumed.disabled_loaded_packages()[&record.settings.name];
        assert!(std::sync::Arc::ptr_eq(
            loaded,
            &capture.owner().disabled_loaded_packages()[&record.settings.name]
        ));
        assert_eq!(loaded.collected_signing, record.signing);
        assert_eq!(
            loaded.facade_entry().unwrap().past_signing_certificates,
            record.signing.past_signing_certificates
        );
        assert!(record.parsed.signing_details == Some(record.signing.parcel_details().unwrap()));
        assert_eq!(
            resumed.disabled_loaded_packages()[&record.settings.name].package,
            record.parsed
        );
        assert_ne!(
            resumed.loaded_packages()[&record.settings.name]
                .package
                .path,
            record.parsed.path
        );
    }
    assert!(resumed_data.recovered.is_empty());
    assert!(resumed_data.rejected.is_empty());
    assert!(resumed_data.removed.is_empty());
    for completed in &resumed_data.packages {
        let record = &completed.candidate.record;
        let saved = original
            .settings
            .packages
            .iter()
            .find(|p| p.name == record.settings.name)
            .unwrap();
        assert_eq!(record.settings.app_id, saved.app_id);
        assert_eq!(record.settings.code_path, saved.code_path);
        assert_eq!(record.settings.version_code, saved.version_code);
        assert_eq!(record.settings.flags, saved.flags);
        assert_eq!(record.settings.private_flags, saved.private_flags);
        assert_eq!(record.settings.last_modified_time, saved.last_modified_time);
        assert_eq!(record.settings.key_set_data, saved.key_set_data);
        assert_eq!(completed.candidate.users, saved_users[&saved.name]);
        assert!(completed.copies.is_empty());
    }
    assert_eq!(resumed.settings.key_sets, original.settings.key_sets);
    eprintln!(
        "saved native system/data loops completed 240 system APKs and all 3 selected data APKs"
    );
    let system_count = system_image.packages.len();
    let first_system = SystemImageScan::first_boot(
        || Ok(system_image),
        &apks,
        &config,
        FirstBootSystemInputs {
            certificates: Default::default(),
            seinfo: common::seinfo::scan(),
            apex_image: &Default::default(),
            notify_apex_scan: &|results| {
                assert!(results.is_empty());
                Ok(())
            },
            shared_uid_migration: aim_services::package::scan::SharedUidMigration::NewInstallOnly,
            first_api_level: first_api,
            vendor_sdk,
            abi_policy: &abi_policy,
            compatibility: &compatibility,
            preferred_abi: all_abis.first().unwrap(),
            app_lib32_install_dir: "/data/app-lib",
            platform_runtime_64bit: true,
            install: NativeLibraryInstallPolicy {
                page_size: 16384,
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
            // Explicit diagnostic IDs; actual domain state is not persisted.
            new_domain_id: &domain_ids,
        },
    )
    .unwrap();
    assert_eq!(first_system.packages.len(), system_count);
    for rejected in &first_system.rejected {
        eprintln!(
            "system parse rejection: {}: {}",
            rejected.location.path, rejected.reason
        );
    }

    let mut matched_source = 0;
    for completed in &first_system.packages {
        let keys = &first_system.owner.settings.key_sets;
        let proper = completed
            .candidate
            .record
            .settings
            .key_set_data
            .proper_signing_key_set;
        assert!(proper > 0);
        let ids = &keys
            .key_sets
            .iter()
            .find(|(id, _)| *id == proper)
            .unwrap()
            .1;
        let registered: Vec<_> = ids
            .iter()
            .map(|id| {
                keys.public_keys
                    .iter()
                    .find(|(key, _)| key == id)
                    .unwrap()
                    .1
                    .clone()
            })
            .collect();
        assert_eq!(
            aim_services::package::sign::serialize_public_keys(&registered).unwrap(),
            aim_services::package::sign::serialize_public_keys(
                &completed.candidate.record.signing.public_keys
            )
            .unwrap()
        );
        let candidate = &completed.candidate.record.settings;
        if let Some(saved) =
            original.settings.packages.iter().find(|saved| {
                saved.name == candidate.name && saved.code_path == candidate.code_path
            })
        {
            assert_eq!(
                (candidate.flags, candidate.private_flags),
                (saved.flags, saved.private_flags),
                "first system flags: {}",
                candidate.name
            );
            assert_eq!(
                candidate.version_code, saved.version_code,
                "first system version: {}",
                candidate.name
            );
            assert_eq!(
                candidate.target_sdk_version, saved.target_sdk_version,
                "first system target SDK: {}",
                candidate.name
            );
            assert_eq!(
                candidate.last_modified_time, saved.last_modified_time,
                "first system file time: {}",
                candidate.name
            );
            assert_eq!(
                candidate.restrict_update_hash, saved.restrict_update_hash,
                "first system update hash: {}",
                candidate.name
            );
            assert_eq!(
                candidate.signatures.as_ref().unwrap().signatures,
                saved.signatures.as_ref().unwrap().signatures,
                "first system certificates: {}",
                candidate.name
            );
            matched_source += 1;
        }
    }
    assert!(
        matched_source >= 230,
        "only {matched_source} unchanged system sources matched"
    );
    eprintln!(
        "first native scan final flags/version/SDK/time/hash/certificates match {matched_source} original packages at unchanged code paths"
    );
    eprintln!(
        "first native image scan completed {system_count} system APKs in directory order; {} parse rejections, without persisted package settings",
        first_system.rejected.len()
    );
    let mut checked_flags = 0;
    let mut flag_mismatches = Vec::new();
    for saved in &original.settings.packages {
        assert_eq!(
            apks.scan_file_time(&inputs.active[&saved.name].parsed)
                .unwrap(),
            saved.last_modified_time,
            "original scan file time: {}",
            saved.name
        );

        let record = &inputs.active[&saved.name];
        let location = scan_location(&saved.code_path, &apexes);
        // InitAppsHelper scans overlay directories before framework-res.
        let before_platform = location.as_ref().is_some_and(|l| l.kind == Kind::Overlay);
        let mut policy = location
            .as_ref()
            .map(ScanPolicy::for_location)
            .unwrap_or_default();
        let disabled = inputs.disabled.get(&saved.name);
        if let Some(original) = disabled {
            // Disabled XML restores only system/priv-app flags. The original
            // factory scan refreshes partition flags before scanning its update.
            let location = scan_location(&original.settings.code_path, &apexes).unwrap();
            let factory_policy = ScanPolicy::for_location(&location);
            let mut parsed = original.parsed.clone();
            factory_policy
                .apply_manifest(
                    &mut parsed,
                    &original.signing,
                    if location.kind == Kind::Overlay {
                        None
                    } else {
                        Some(platform_signing)
                    },
                    false,
                    &apks,
                )
                .unwrap();
            let mut factory_setting = original.settings.clone();
            (factory_setting.flags, factory_setting.private_flags) =
                application_flags(&parsed, false);
            policy.inherit_system_setting(&factory_setting);
        }
        policy.adjust_shared_uid_privilege(
            &record.parsed,
            &record.signing,
            platform_signing,
            &scan.identities,
            vendor_sdk,
        );
        let mut parsed = record.parsed.clone();
        policy
            .apply_manifest(
                &mut parsed,
                &record.signing,
                if before_platform {
                    None
                } else {
                    Some(platform_signing)
                },
                disabled.is_some(),
                &apks,
            )
            .unwrap();
        let actual = application_flags(&parsed, disabled.is_some());
        if actual != (saved.flags, saved.private_flags) {
            flag_mismatches.push(format!(
                "{}: native {actual:?}, original {:?}",
                saved.name,
                (saved.flags, saved.private_flags)
            ));
        }
        checked_flags += 1;

        // This subowner consumes collected signing, not a fresh verification
        // result. Preserve cache-owned current Signature flags as init does.
        let collected = scan
            .collect_initial_code(
                &Code {
                    location: scan_location(&saved.code_path, &apexes).unwrap_or(Location {
                        path: saved.code_path.clone(),
                        partition: Partition::Data,
                        kind: Kind::App,
                        apex: None,
                    }),
                    parsed: record.parsed.clone(),
                    signing: record.signing.clone(),
                },
                &apks,
                Default::default(),
            )
            .unwrap();
        let record = aim_services::package::scan::Record {
            settings: record.settings.clone(),
            parsed: collected.parsed,
            signing: collected.signing,
            identity: record.identity.clone(),
            origin: record.origin.clone(),
        };
        let result = scan
            .apply_with_disabled(&record, inputs.disabled.get(&saved.name))
            .unwrap_or_else(|error| panic!("{}: {error:?}", saved.name));
        assert!(result.system_signature_mismatch.is_none(), "{}", saved.name);
    }
    assert_eq!(checked_flags, 243);
    assert!(
        flag_mismatches.is_empty(),
        "original scan application flags: {flag_mismatches:#?}"
    );
    eprintln!(
        "native scan manifest policy/application flags match all {checked_flags} original packages"
    );
    for (saved, candidate) in original
        .settings
        .packages
        .iter()
        .zip(&scan.settings.packages)
    {
        let mut expected = saved.clone();
        let keys = candidate.signatures.as_ref().unwrap().public_keys.clone();
        assert!(
            keys.as_ref().is_some_and(|k| !k.is_empty()),
            "{}",
            saved.name
        );
        expected.signatures.as_mut().unwrap().public_keys = keys;
        assert_eq!(&expected, candidate, "{}", saved.name);
    }
    assert_eq!(original.settings.shared_users, scan.settings.shared_users);
    let mut expected: Vec<_> = output
        .lines()
        .filter_map(|line| {
            line.strip_prefix("  ")
                .filter(|line| line.contains(" -> "))
                .map(str::to_owned)
        })
        .collect();
    assert!(!expected.is_empty(), "original library dump: {output}");
    let mut actual: Vec<_> = scan
        .libraries
        .entries()
        .map(|library| {
            let mut line = library.name.clone().unwrap();
            if library.kind == TYPE_STATIC {
                line.push_str(&format!(" version={}", library.version));
            }
            line.push_str(" -> ");
            if let Some(path) = &library.path {
                line.push_str(if library.native { " (so) " } else { " (jar) " });
                line.push_str(path);
            } else {
                line.push_str(" (apk) ");
                line.push_str(library.package_name.as_ref().unwrap());
            }
            line
        })
        .collect();
    expected.sort();
    actual.sort();
    assert_eq!(actual, expected);
    eprintln!(
        "reconciled {} active / {} disabled packages, {} shared UID groups, {} libraries",
        inputs.active.len(),
        inputs.disabled.len(),
        scan.settings.shared_users.len(),
        actual.len()
    );
    assert_eq!(
        State::read(&boot.data.join("data"), &[0]).unwrap().unwrap(),
        original
    );
    volume.detach().unwrap();
}

fn partition_path(path: &str) -> Option<(Partition, &str)> {
    [
        ("/system/", Partition::System),
        ("/vendor/", Partition::Vendor),
        ("/odm/", Partition::Odm),
        ("/oem/", Partition::Oem),
        ("/product/", Partition::Product),
        ("/system_ext/", Partition::SystemExt),
    ]
    .iter()
    .find_map(|(prefix, p)| path.strip_prefix(prefix).map(|r| (*p, r)))
}

fn scan_location(path: &str, apexes: &[Apex]) -> Option<Location> {
    if path.starts_with("/data/app/") {
        return None;
    }
    let apex = apexes
        .iter()
        .find(|a| path.starts_with(&format!("{}/", a.mount_path)));
    let (partition, relative) = partition_path(path)
        .or_else(|| {
            apex.map(|a| {
                (
                    a.partition,
                    path.strip_prefix(&format!("{}/", a.mount_path)).unwrap(),
                )
            })
        })
        .unwrap_or_else(|| panic!("scan flag diagnostic needs explicit location: {path}"));
    Some(Location {
        path: path.into(),
        partition,
        kind: if relative.starts_with("overlay/") {
            Kind::Overlay
        } else if relative.starts_with("priv-app/") {
            Kind::PrivApp
        } else if relative.starts_with("framework/") {
            Kind::Framework
        } else {
            Kind::App
        },
        apex: apex.cloned(),
    })
}
