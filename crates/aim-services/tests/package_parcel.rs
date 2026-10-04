//! Original PackageImpl read/write oracle for native scan parcel output.
use aim_services::package::pkg::AndroidPackage;
use std::{
    fs,
    process::Command,
    time::{Duration, Instant},
};
mod common {
    pub mod zip;
    pub mod java;
    pub mod runtime;
    pub mod seinfo;
}
use common::java::sources;
use common::runtime::{Boot, Data, run};
#[track_caller]
fn assert_guest_success(boot: &Boot, output: &std::process::Output, stage: &str) {
    if output.status.success() {
        return;
    }
    let logs = boot
        .command()
        .args(["shell", "logcat", "-d", "-s", "AndroidRuntime:V"])
        .output()
        .unwrap();
    panic!(
        "{stage}: {}; stderr: {}; AndroidRuntime: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&logs.stdout)
    );
}
#[test]
#[ignore = "requires pinned image, aimctl, JDK and d8; run explicitly"]
fn native_package_parcels_match_original_read_write() {
    use std::collections::BTreeMap;
    let dir = std::env::temp_dir().join(format!("aim-pm-parcels-{}", std::process::id()));
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
        .args(sources(
            &aim_paths::root().join("java/device-services/stubs"),
        ))
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/api/PackageBootstrapBridge.java"),
        )
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/api/SELinuxMMAC.java")));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/ScanSettingsWriteOracle.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/PackageTransientState.java"),
        )
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/CapturedTransientOracle.java"),
        )
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/CapturedPackageStateOracle.java"),
        )
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/PackageCacheValidationOracle.java"),
        )
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/CapturedSharedUserOracle.java"),
        )
        .arg("-classpath")
        .arg(&stubs)
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/PackageRoundTripOracle.java"),
        )
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/CapturedInstallSourceOracle.java"),
        )
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/CapturedKeySetOracle.java"),
        )
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/LegacyPermissionOracle.java"),
        )
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/LegacyRestoreOracle.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/PackageLegacyPermissions.java"),
        )
        .arg(
            aim_paths::root().join("java/device-services/src/dev/aim/server/PackageDomainIds.java"),
        )
        .arg(
            aim_paths::root().join("java/device-services/src/dev/aim/server/PackageScanUsers.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/com/android/server/pm/ApexBootFeed.java"),
        )
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/StaticLibraryIdentityOracle.java"),
        )
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/ApexParseOracle.java"),
        )
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/ApexNotifyOracle.java"),
        )
        .arg(aim_paths::root().join("java/device-services/src/dev/aim/server/PackageObjects.java"))
        .arg(
            aim_paths::root()
                .join("java/device-services/src/com/android/server/pm/CapturedPackageSetting.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/PackageLibraryState.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/PackageLibraryFeed.java"),
        )
        .arg(aim_paths::root().join("java/device-services/src/dev/aim/server/PackageCode.java"))
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/PackageSigningState.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/PackageUserStateData.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/PackageSettingData.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/PackageMimeGroups.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/com/android/server/pm/CapturedInstallSource.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/com/android/server/pm/CapturedKeySetData.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/PackageUserStateReplica.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/PackageSeInfoState.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/PackageUsageState.java"),
        )
        .arg(
            aim_paths::root().join("java/device-services/src/dev/aim/server/PackageScanLease.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/PackageStateReplica.java"),
        )
        .arg(aim_paths::root().join("java/device-services/src/dev/aim/server/SharedUserData.java"))
        .arg(
            aim_paths::root()
                .join("java/device-services/src/com/android/server/pm/SharedProcessFeed.java"),
        )
        .arg(
            aim_paths::root()
                .join("crates/aim-services/tests/fixtures/RetainedSharedUserOracle.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/RetainedPackageData.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/SharedUserReplica.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/PackageRuntimeState.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/PackageRuntimeFeed.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/PackageUserScopeFeed.java"),
        )
        .arg(
            aim_paths::root().join("java/device-services/src/dev/aim/server/PackageSnapshots.java"),
        )
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/PackageMigrationPolicyOracle.java"),
        )
        .arg(common::java::bootstrap_aidl(&data.0))
        .arg(common::java::snapshot_aidl(&data.0)));
    let mut pending = vec![classes.clone()];
    let mut class_files = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "class") {
                class_files.push(path);
            }
        }
    }
    class_files.sort();
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
        .args(class_files));
    common::java::check_linkage(
        &dex.join("classes.dex"),
        &[
            "/system/framework/services.jar",
            "/system/framework/aim-services.jar",
        ],
    )
    .unwrap();
    let boot = Boot {
        ctl: aim_paths::root().join("target/release/aimctl"),
        data: data.0.join("guest"),
    };
    run(boot.command().args(["start", "--windows"]));
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let output = boot
            .command()
            .args(["shell", "getprop", "sys.boot_completed"])
            .output()
            .unwrap();
        if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "1" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "disposable boot did not complete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }

    let directory = boot.data.join("data/local/tmp/package-parcels");
    fs::create_dir(&directory).unwrap();
    fs::copy(dex.join("classes.dex"), directory.join("oracle.dex")).unwrap();
    write_library_owner_fixture(&directory);
    let framework =
        aim_services::package::system_config::Framework::load(&aim_paths::derived_image()).unwrap();
    let system = aim_services::package::system_config::system(
        &aim_paths::derived_image(),
        &|_| None,
        &framework,
    )
    .unwrap();
    let mut fallback_cases = String::new();
    for (name, category) in &system.fallback_categories {
        fallback_cases.push_str(&format!("{name}\t{category}\n"));
    }
    for name in ["com.android.printspooler", "com.google.android.gms"] {
        let category = system
            .fallback_categories
            .iter()
            .find(|(p, _)| p == name)
            .map_or(-1, |(_, c)| *c);
        fallback_cases.push_str(&format!("{name}\t{category}\n"));
    }
    fs::write(directory.join("fallback-categories.txt"), fallback_cases).unwrap();
    let mut pending = vec![boot.data.join("data/system/package_cache")];
    let mut files = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    assert!(!files.is_empty(), "original parser cache is empty");
    let mut cached_names = std::collections::BTreeSet::new();
    let state = aim_services::package::State::read(&boot.data.join("data"), &[0])
        .unwrap()
        .unwrap();
    let saved = state
        .settings
        .packages
        .iter()
        .find(|p| p.name == "android")
        .unwrap()
        .signatures
        .as_ref()
        .unwrap();
    let signer = aim_services::package::sign::SigningDetails::from_saved(saved).unwrap();
    let mut expected = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let pkg = AndroidPackage::read_cache_entry(&fs::read(file).unwrap()).unwrap();
        cached_names.insert(pkg.package_name.clone());
        for enriched in [false, true] {
            let mut pkg = pkg.clone();
            if enriched {
                pkg.uid = 19001;
                pkg.primary_cpu_abi = Some("arm64-v8a".into());
                pkg.native_library_root_dir = Some("/data/app/fixture/lib".into());
                pkg.native_library_dir = Some("/data/app/fixture/lib/arm64".into());
                pkg.native_library_root_requires_isa = true;
                pkg.version_name = Some("native-owner".into());
                pkg.page_size_app_compat_flags = 8;
                pkg.signing_details = Some(signer.parcel_details().unwrap());
            }
            let entry = pkg.to_cache_entry().unwrap();
            let decoded = AndroidPackage::read_cache_entry(&entry.bytes).unwrap();
            assert_eq!(
                decoded == pkg,
                true,
                "native read/write changed {}",
                pkg.package_name
            );
            let name = format!("{index}-{enriched}.native");
            fs::write(directory.join(&name), &entry.bytes).unwrap();
            expected.push((name, pkg.package_name, entry));
        }
    }
    // Boot-generated cache counts vary; every discovered object remains part
    // of the exact original round-trip comparison (#839).
    for name in ["android", "com.google.android.gsf"] {
        assert!(
            cached_names.contains(name),
            "missing original cache: {name}"
        );
    }
    eprintln!("original cache entries: {}", files.len());
    let snapshot = native_scan_objects(&data.0);
    let runtime_snapshot = runtime_fixture_snapshot(&snapshot);
    // Separate valid writer inputs: the original XML writers reject nullable
    // MIME values and key aliases; existing oracle cases retain those failures.
    let mut writer_owner = snapshot.owner().clone();
    for setting in &mut writer_owner.settings.packages {
        setting.mime_groups.retain(|(name, _)| name.is_some());
        for (_, types) in &mut setting.mime_groups {
            types.retain(Option::is_some);
        }
        setting
            .key_set_data
            .defined_key_sets
            .retain(|(alias, _)| alias.is_some());
    }
    writer_owner.settings.key_sets.reference_counts = None;
    aim_services::package::owner::key_sets::restore(&mut writer_owner.settings).unwrap();
    let writer_snapshot =
        aim_services::package::scan_snapshot::Store::new(writer_owner, snapshot.usage().clone())
            .unwrap()
            .capture();
    fs::write(
        directory.join("settings-owner-order"),
        writer_snapshot
            .owner()
            .settings
            .packages
            .iter()
            .map(|setting| format!("scan-{}.native.writer-setting\n", setting.app_id))
            .collect::<String>(),
    )
    .unwrap();
    let writer_data = directory.join("native-settings-writer");
    let mut writer_store = aim_services::package::owner::Store::create(&writer_data, &[0]).unwrap();
    assert!(!writer_data.join("system/packages.xml").exists());
    writer_store.commit_scan_settings(&writer_snapshot).unwrap();
    let reindexed_data = directory.join("reindexed-settings-writer");
    fs::create_dir_all(reindexed_data.join("system")).unwrap();
    fs::write(
        reindexed_data.join("system/packages.xml"),
        fs::read(writer_data.join("system/packages.xml")).unwrap(),
    )
    .unwrap();
    let mut reindexed = aim_services::package::owner::Store::open(&reindexed_data, &[0])
        .unwrap()
        .unwrap();
    let mut desired = reindexed.state().settings.clone();
    let first = &desired.packages[0].signatures.as_ref().unwrap().signatures[0];
    let replacement = state
        .settings
        .packages
        .iter()
        .filter_map(|p| p.signatures.as_ref())
        .flat_map(|s| s.signatures.iter())
        .find(|cert| *cert != first)
        .expect("original image has no distinct certificate fixture")
        .clone();
    desired.packages[0].signatures.as_mut().unwrap().signatures = vec![replacement];
    reindexed.commit_signatures(&desired).unwrap();
    fn describe(signatures: &aim_services::package::settings::Signatures) -> String {
        fn hex(bytes: &[u8]) -> String {
            bytes.iter().map(|b| format!("{b:02x}")).collect()
        }
        let current = signatures
            .signatures
            .iter()
            .map(|c| hex(c))
            .collect::<Vec<_>>()
            .join(",");
        let past = signatures
            .past_signatures
            .as_ref()
            .map_or("null".into(), |past| {
                past.iter()
                    .map(|(cert, flags)| format!("{}:{flags}", hex(cert)))
                    .collect::<Vec<_>>()
                    .join(",")
            });
        format!("{}|{current}|{past}", signatures.scheme_version)
    }
    let mut expected_certificates = String::new();
    for package in &desired.packages {
        for (tag, signatures) in [
            ("sigs", &package.signatures),
            (
                "install-initiator-sigs",
                &package.install_source.initiating_package_signatures,
            ),
        ] {
            if let Some(signatures) = signatures {
                expected_certificates.push_str(&format!(
                    "package/{}/{tag}={}\n",
                    package.name,
                    describe(signatures)
                ));
            }
        }
    }
    for group in &desired.shared_users {
        if let Some(signatures) = &group.signatures {
            expected_certificates.push_str(&format!(
                "shared-user/{}/sigs={}\n",
                group.name,
                describe(signatures)
            ));
        }
    }
    fs::write(
        directory.join("reindexed-certificates.properties"),
        expected_certificates,
    )
    .unwrap();

    let mut writer_document =
        aim_android_xml::read(&fs::read(writer_data.join("system/packages.xml")).unwrap()).unwrap();
    // Global keyset ownership is checked by its separate original oracle.
    writer_document
        .content
        .retain(|n| !matches!(n, aim_android_xml::Node::Element(e) if e.name == "keyset-settings"));
    let writer_expected =
        aim_services::package::settings::Settings::parse(&writer_document).unwrap();
    let mut static_identity = snapshot.owner().loaded_packages()["android"]
        .package
        .clone();
    static_identity.package_name = "fixture.provider".into();
    static_identity.manifest_package_name = Some("fixture.provider".into());
    aim_services::package::scan::Identity {
        manifest_name: "fixture.provider".into(),
        internal_name: "fixture.provider".into(),
        real_name: None,
    }
    .apply(&mut static_identity);

    static_identity.static_shared_library_name = Some("fixture.library".into());
    static_identity.static_shared_lib_version = 0x100000007;
    static_identity.booleans2 |= aim_services::package::pkg::booleans2::APEX;
    fs::write(
        directory.join("static-identity.input"),
        static_identity.to_cache_entry().unwrap().bytes,
    )
    .unwrap();

    scoped_runtime_objects(&directory);
    distinct_shared_id_objects(&directory);
    cache_validation_objects(
        &directory,
        &snapshot.owner().loaded_packages()["android"].package,
    );
    for group in snapshot.owner().identities.shared_users.keys() {
        fs::write(
            directory.join(format!("shared-{group}.record")),
            aim_services::package::scan_snapshot::shared_record::captured(&snapshot, group)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
    }
    for loaded in snapshot.owner().loaded_packages().values() {
        let pkg = &loaded.package;
        let name = format!("scan-{}.native", pkg.uid);
        fs::write(
            directory.join(format!("{name}.captured-runtime")),
            aim_services::package::scan_snapshot::runtime_record::captured(
                &runtime_snapshot,
                &pkg.package_name,
                false,
            )
            .unwrap()
            .unwrap(),
        )
        .unwrap();

        let facade = loaded.facade_entry().unwrap();
        fs::write(
            directory.join(format!("{name}.hidden-policy")),
            snapshot
                .owner()
                .hidden_api_enforcement_policy(&pkg.package_name, false)
                .unwrap()
                .unwrap()
                .to_string(),
        )
        .unwrap();
        let code = aim_services::package::scan_snapshot::endpoint::PackageCode::captured(
            &snapshot,
            &pkg.package_name,
            false,
        )
        .unwrap()
        .unwrap();
        let mut parcel = aim_binder_host::parcel::Parcel::new();
        aim_service_aidl::WriteParcelable::write_to(&code, &mut parcel);
        fs::write(directory.join(format!("{name}.snapshot")), parcel.data()).unwrap();
        let user_state = aim_services::package::scan_snapshot::user_record::captured(
            &snapshot,
            &pkg.package_name,
            false,
            0,
        )
        .unwrap()
        .unwrap();
        fs::write(directory.join(format!("{name}.user")), user_state).unwrap();
        fs::write(
            directory.join(format!("{name}.keysets-native")),
            keysets_xml_expected(&directory, &name),
        )
        .unwrap();

        fs::write(
            directory.join(format!("{name}.mime-native")),
            mime_xml_expected(&directory, &name),
        )
        .unwrap();
        let mut mime_feed = aim_binder_host::parcel::Parcel::new();
        let captured_setting = snapshot
            .owner()
            .settings
            .packages
            .iter()
            .find(|p| p.name == pkg.package_name)
            .unwrap();
        mime_feed.write_i32(captured_setting.mime_groups.len() as i32);
        for (name, types) in &captured_setting.mime_groups {
            mime_feed.write_string16(name.as_deref());
            mime_feed.write_i32(types.len() as i32);
            for value in types {
                mime_feed.write_string16(value.as_deref());
            }
        }
        fs::write(
            directory.join(format!("{name}.mime-feed-native")),
            mime_feed.data(),
        )
        .unwrap();
        let setting_bytes = aim_services::package::scan_snapshot::setting_record::captured(
            &snapshot,
            &pkg.package_name,
            false,
        )
        .unwrap()
        .unwrap();
        fs::write(directory.join(format!("{name}.setting")), setting_bytes).unwrap();
        fs::write(
            directory.join(format!("{name}.writer-setting")),
            aim_services::package::scan_snapshot::setting_record::captured(
                &writer_snapshot,
                &pkg.package_name,
                false,
            )
            .unwrap()
            .unwrap(),
        )
        .unwrap();
        for variant in 0..49 {
            let factory = variant >= 24 && variant < 48;
            let mut owner = snapshot.owner().clone();
            let state = if variant == 48 {
                captured_setting.transient.clone()
            } else {
                aim_services::package::owner::transient::State {
                    hidden_until_installed: variant % 8 & 1 != 0,
                    updated_system_app: variant % 8 & 2 != 0,
                    apk_in_updated_apex: variant % 8 & 4 != 0,
                    apex_module_name: match (variant % 24) / 8 {
                        0 => None,
                        1 => Some(String::new()),
                        _ => Some("com.example.apex".into()),
                    },
                }
            };
            if factory {
                let mut setting = captured_setting.clone();
                setting.transient = state;
                owner
                    .settings
                    .disabled_system_packages
                    .retain(|p| p.name != pkg.package_name);
                owner.settings.disabled_system_packages.push(setting);
                // This fixture captures only fresh active/factory settings;
                // it does not supply unrelated scan/permission assignments.
                owner = aim_services::package::scan::SigningScan::new(
                    &Default::default(),
                    &owner.settings,
                    36,
                )
                .unwrap();
            } else {
                owner
                    .settings
                    .packages
                    .iter_mut()
                    .find(|p| p.name == pkg.package_name)
                    .unwrap()
                    .transient = state;
            }
            let capture =
                aim_services::package::scan_snapshot::Store::new(owner, snapshot.usage().clone())
                    .unwrap()
                    .capture();
            let state =
                aim_services::package::scan_snapshot::endpoint::PackageTransientState::captured(
                    &capture,
                    &pkg.package_name,
                    factory,
                )
                .unwrap();
            let mut p = aim_binder_host::parcel::Parcel::new();
            aim_service_aidl::WriteParcelable::write_to(&state, &mut p);
            let suffix = if variant == 48 {
                "transient".into()
            } else {
                format!("transient-{variant}")
            };
            fs::write(directory.join(format!("{name}.{suffix}")), p.data()).unwrap();
        }

        fs::write(
            directory.join(format!("{name}.libraries")),
            aim_services::package::scan_snapshot::library_record::captured(
                &snapshot,
                &pkg.package_name,
            )
            .unwrap()
            .unwrap(),
        )
        .unwrap();
        for (suffix, leaving) in [
            ("true", Some(true)),
            ("false", Some(false)),
            ("unknown", None),
        ] {
            let mut owner = snapshot.owner().clone();
            owner
                .settings
                .packages
                .iter_mut()
                .find(|p| p.name == pkg.package_name)
                .unwrap()
                .leaving_shared_user = leaving;
            let changed =
                aim_services::package::scan_snapshot::Store::new(owner, snapshot.usage().clone())
                    .unwrap()
                    .capture();
            let bytes = aim_services::package::scan_snapshot::setting_record::captured(
                &changed,
                &pkg.package_name,
                false,
            )
            .unwrap()
            .unwrap();
            fs::write(
                directory.join(format!("{name}.setting-leaving-{suffix}")),
                bytes,
            )
            .unwrap();
        }
        for (suffix, paths) in [("null", None), ("empty", Some(vec![]))] {
            let mut changed = snapshot.owner().clone();
            changed
                .settings
                .packages
                .iter_mut()
                .find(|p| p.name == pkg.package_name)
                .unwrap()
                .old_paths = paths;
            let changed =
                aim_services::package::scan_snapshot::Store::new(changed, snapshot.usage().clone())
                    .unwrap()
                    .capture();
            fs::write(
                directory.join(format!("{name}.setting-{suffix}")),
                aim_services::package::scan_snapshot::setting_record::captured(
                    &changed,
                    &pkg.package_name,
                    false,
                )
                .unwrap()
                .unwrap(),
            )
            .unwrap();
        }

        fs::write(
            directory.join(format!("{name}.loading-native")),
            loading_xml_expected(&directory, &name),
        )
        .unwrap();

        for user in [10, 11, 12, 13, 14] {
            let bytes = aim_services::package::scan_snapshot::user_record::captured(
                &snapshot,
                &pkg.package_name,
                false,
                user,
            )
            .unwrap()
            .unwrap();
            fs::write(directory.join(format!("{name}.user-{user}")), bytes).unwrap();
        }
        fs::write(
            directory.join(format!("{name}.runtime")),
            runtime_expected(),
        )
        .unwrap();
        let saved_signing =
            aim_services::package::scan_snapshot::endpoint::PackageSigningState::captured(
                &snapshot,
                &pkg.package_name,
                false,
            )
            .unwrap()
            .unwrap();
        let mut signing_parcel = aim_binder_host::parcel::Parcel::new();
        aim_service_aidl::WriteParcelable::write_to(&saved_signing, &mut signing_parcel);
        fs::write(
            directory.join(format!("{name}.saved-signing")),
            signing_parcel.data(),
        )
        .unwrap();
        let signing_store = aim_services::package::scan_snapshot::Store::new(
            snapshot.owner().clone(),
            snapshot.usage().clone(),
        )
        .unwrap();
        let signing_base = signing_store.capture();
        let mut changed_signing = snapshot.owner().clone();
        let setting = changed_signing
            .settings
            .packages
            .iter_mut()
            .find(|setting| setting.name == pkg.package_name)
            .unwrap();
        if let Some(past) = setting
            .signatures
            .as_mut()
            .and_then(|s| s.past_signatures.as_mut())
        {
            if let Some((_, flags)) = past.first_mut() {
                *flags &= !1;
            }
        }
        if setting.shared_user {
            let aim_services::package::owner::app_ids::Owner::SharedUser(group_name) =
                changed_signing.identities.ids.get(setting.app_id).unwrap()
            else {
                panic!("shared owner")
            };
            let group = changed_signing
                .identities
                .shared_users
                .get_mut(group_name)
                .unwrap();
            if let Some(past) = group
                .signatures
                .as_mut()
                .and_then(|s| s.past_signatures.as_mut())
            {
                if let Some((_, flags)) = past.first_mut() {
                    *flags &= !4;
                }
            }
            if let Some(saved) = changed_signing
                .settings
                .shared_users
                .iter_mut()
                .find(|g| &g.name == group_name)
            {
                saved.signatures = group.signatures.clone();
            }
        }
        changed_signing
            .complete_library_dependencies(&|_, _| {
                Ok(aim_services::package::libraries::Policy::pinned(false))
            })
            .unwrap();
        let changed = signing_store
            .publish(&signing_base, changed_signing, snapshot.usage().clone())
            .unwrap();
        let changed_state =
            aim_services::package::scan_snapshot::endpoint::PackageSigningState::captured(
                &changed,
                &pkg.package_name,
                false,
            )
            .unwrap()
            .unwrap();
        let mut changed_parcel = aim_binder_host::parcel::Parcel::new();
        aim_service_aidl::WriteParcelable::write_to(&changed_state, &mut changed_parcel);
        fs::write(
            directory.join(format!("{name}.saved-signing.changed")),
            changed_parcel.data(),
        )
        .unwrap();
        let usage = aim_services::package::scan_snapshot::endpoint::PackageUsage::captured(
            &snapshot,
            &pkg.package_name,
        )
        .unwrap();
        let mut parcel = aim_binder_host::parcel::Parcel::new();
        aim_service_aidl::WriteParcelable::write_to(&usage, &mut parcel);
        fs::write(directory.join(format!("{name}.usage")), parcel.data()).unwrap();
        let seinfo = aim_services::package::scan_snapshot::endpoint::PackageSeInfo::captured(
            &snapshot,
            &pkg.package_name,
        )
        .unwrap()
        .unwrap();
        let mut parcel = aim_binder_host::parcel::Parcel::new();
        aim_service_aidl::WriteParcelable::write_to(&seinfo, &mut parcel);
        fs::write(directory.join(format!("{name}.boot-seinfo")), parcel.data()).unwrap();
        let (policy, policy_read) = match aim_services::package::owner::seinfo::Policy::load(
            &aim_paths::original_image(),
        ) {
            Ok(policy) => (policy, true),
            Err(error) => {
                assert_eq!(error, "duplicate mac-permissions policy");
                (
                    aim_services::package::owner::seinfo::Policy::unread(),
                    false,
                )
            }
        };
        fs::write(
            directory.join(format!("{name}.seinfo-read")),
            if policy_read { "true" } else { "false" },
        )
        .unwrap();
        let label = policy.label(
            &pkg.package_name,
            aim_services::package::owner::seinfo::Signing::Known(&loaded.collected_signing),
            true,
            36,
            aim_services::package::owner::seinfo::Partition::System,
        );
        fs::write(directory.join(format!("{name}.seinfo")), label).unwrap();
        let mut metadata = Vec::new();
        match facade.past_signing_certificates {
            None => metadata.extend_from_slice(&(-1_i32).to_be_bytes()),
            Some(past) => {
                metadata.extend_from_slice(&(past.len() as i32).to_be_bytes());
                for (cert, flags) in past {
                    metadata.extend_from_slice(&(cert.len() as i32).to_be_bytes());
                    metadata.extend_from_slice(&cert);
                    metadata.extend_from_slice(&flags.to_be_bytes());
                }
            }
        }
        fs::write(directory.join(format!("{name}.signing")), metadata).unwrap();
        let entry = facade.cache;
        fs::write(directory.join(&name), &entry.bytes).unwrap();
        expected.push((name, pkg.package_name.clone(), entry));
    }
    let restore_cases = [
        "<packages><version sdkVersion='36' databaseVersion='3'/><package name='early' codePath='/data/early' sharedUserId='10050'><perms><item name='dropped'/></perms></package><shared-user name='group' userId='10050'><perms><item name='base' flags='17'/></perms></shared-user><package name='late' codePath='/data/late' sharedUserId='10050'><perms><item name='late'/></perms></package><updated-package name='late' codePath='/system/late' userId='10050'><perms><item name='factory'/></perms></updated-package><updated-package name='early' codePath='/system/early' sharedUserId='10050'><perms><item name='factory-shared'/></perms></updated-package><package name='standalone' codePath='/data/standalone' userId='10070'><signing-keyset><perms><item name='nested'/></perms></signing-keyset><unknown><perms><item name='ignored'/></perms></unknown></package></packages>",
        "<packages><preferred-packages><package name='system' codePath='/system/app' sharedUserId='1000'><perms><item name='seed'/></perms></package></preferred-packages><package name='oem' codePath='/vendor/app' sharedUserId='2901'><perms><item name='oem'/></perms></package><updated-package name='system' codePath='/system/old' userId='1000'><perms><item name='factory'/></perms></updated-package><unknown><package name='ignored' codePath='/data/app' userId='10090'/></unknown></packages>",
    ];
    for (index, text) in restore_cases.iter().cycle().take(4).enumerate() {
        let root = aim_android_xml::read_next(text.as_bytes()).unwrap();
        let bytes = if index >= 2 {
            aim_android_xml::abx::write(&root).unwrap()
        } else {
            text.as_bytes().to_vec()
        };
        let input = directory.join(format!("legacy-restore-{index}"));
        fs::create_dir_all(input.join("system")).unwrap();
        fs::write(input.join("system/packages.xml"), bytes).unwrap();
        let mut config = aim_services::package::system_config::SystemConfig::default();
        config.oem_defined_uids = vec![("android.uid.vendor.fixture".into(), 2901)];
        let state = aim_services::package::State::read_with_config(&input, &[10, 0], &config)
            .unwrap()
            .unwrap();
        let mut scan =
            aim_services::package::scan::SigningScan::new(&config, &state.settings, 36).unwrap();
        scan.restore_legacy_permissions_from_data(&input, &state, &config)
            .unwrap();
        for (settings, factory) in [
            (&state.settings.packages, false),
            (&state.settings.disabled_system_packages, true),
        ] {
            for package in settings {
                let stem = format!(
                    "{}-{}",
                    if factory { "factory" } else { "active" },
                    package.name
                );
                fs::write(
                    input.join(format!("{stem}.input")),
                    scan.legacy_permissions(&package.name, factory)
                        .unwrap()
                        .unwrap()
                        .bytes(),
                )
                .unwrap();
                fs::write(
                    input.join(format!("{stem}.fixed")),
                    if !factory
                        && scan
                            .legacy_restoration_metadata()
                            .unwrap()
                            .unwrap()
                            .install_permissions_fixed
                            .contains(&package.name)
                    {
                        "true"
                    } else {
                        "false"
                    },
                )
                .unwrap();
            }
        }
        for name in scan.identities.shared_users.keys() {
            fs::write(
                input.join(format!("shared-{name}.input")),
                scan.shared_legacy_permissions(name)
                    .unwrap()
                    .unwrap()
                    .bytes(),
            )
            .unwrap();
        }
        scan.assign_seinfo_at_boot(
            &aim_services::package::owner::seinfo::Policy::unread(),
            &mut |_| Ok(36),
        )
        .unwrap();
        let usage = aim_services::package::owner::usage::Usage::new(
            scan.settings.packages.iter().map(|p| p.name.as_str()),
        );
        let snapshot = aim_services::package::scan_snapshot::Store::new(scan, usage)
            .unwrap()
            .capture();
        for package in &state.settings.disabled_system_packages {
            fs::write(
                input.join(format!("factory-{}.setting", package.name)),
                aim_services::package::scan_snapshot::setting_record::captured(
                    &snapshot,
                    &package.name,
                    true,
                )
                .unwrap()
                .unwrap(),
            )
            .unwrap();
        }
    }
    let migration_cases = [
        "<perms/>",
        "<perms><item name='BB'/><item/><item name=''/><item name='Aa' flags='-1'/><item name='BB' granted='false' flags='17'/></perms>",
        "<perms><item name='outer'><item name='inner'/></item><unknown><item name='ignored'/></unknown></perms>",
        "<perms><item name='bad' granted='nonsense' flags='overflow'/><item name='true' granted='TRUE' flags='+7f'/><item name='negative' flags='-80000000'/><item name='overflow' flags='80000000'/></perms>",
        "<permissions/>",
        "<permissions><item name='BB'/><item/><item name=''/><item name='Aa' flags='-1'/><item name='BB' granted='false' flags='17'/></permissions>",
        "<permissions><unknown><item name='nested'/></unknown><item name='outer'><item name='inner'/></item></permissions>",
        "<permissions><item name='bad' granted='nonsense' flags='overflow'/><item name='true' granted='TRUE' flags='+7f'/><item name='negative' flags='-80000000'/><item name='overflow' flags='80000000'/></permissions>",
    ];
    for binary in [false, true] {
        for (case, text) in migration_cases.iter().enumerate() {
            let root = aim_android_xml::read_next(text.as_bytes()).unwrap();
            let index = case + if binary { 8 } else { 0 };
            fs::write(
                directory.join(format!("legacy-migration-{index}.xml")),
                if binary {
                    aim_android_xml::abx::write(&root).unwrap()
                } else {
                    text.as_bytes().to_vec()
                },
            )
            .unwrap();
            use aim_services::package::{
                owner::legacy_permissions::{Migration, Permission},
                permissions::RuntimePermission,
            };
            let mut migration = Migration::default();
            migration
                .put(
                    0,
                    Permission {
                        name: Some("seed".into()),
                        runtime: false,
                        granted: false,
                        flags: i32::MAX,
                    },
                )
                .unwrap();
            migration.set_missing(10, true).unwrap();
            if case >= 4 {
                migration.read_legacy_runtime(&root, 10).unwrap();
            } else {
                migration.read_install(&root, &[10, 0]).unwrap();
            }
            migration
                .read_runtime(
                    0,
                    &[
                        RuntimePermission {
                            name: "modern".into(),
                            granted: false,
                            flags: i32::MIN,
                        },
                        RuntimePermission {
                            name: "seed".into(),
                            granted: true,
                            flags: 0x408030,
                        },
                    ],
                )
                .unwrap();
            fs::write(
                directory.join(format!("legacy-migration-{index}.input")),
                migration.project(10042, &[10, 0, 11]).unwrap().bytes(),
            )
            .unwrap();
        }
    }
    let mut legacy = aim_binder_host::parcel::Parcel::new();
    legacy.write_i32(10042);
    legacy.write_i32(3);
    legacy.write_i32(10);
    legacy.write_bool(true);
    legacy.write_i32(4);
    for (name, runtime, granted, flags) in [
        (None, false, false, i32::MIN),
        (Some(""), true, true, -1),
        (Some("BB"), true, false, 17),
        (Some("Aa"), false, true, i32::MAX),
    ] {
        legacy.write_string16(name);
        legacy.write_bool(runtime);
        legacy.write_bool(granted);
        legacy.write_i32(flags);
    }
    legacy.write_i32(0);
    legacy.write_bool(false);
    legacy.write_i32(1);
    legacy.write_string16(Some("android.permission.CAMERA"));
    legacy.write_bool(true);
    legacy.write_bool(true);
    legacy.write_i32(0x408030);
    legacy.write_i32(11);
    legacy.write_bool(false);
    legacy.write_i32(0);
    let permissions = aim_services::package::owner::legacy_permissions::State::read(
        legacy.data(),
        10042,
        &[10, 0, 11],
    )
    .unwrap();
    fs::write(
        directory.join("legacy-permissions.input"),
        permissions.bytes(),
    )
    .unwrap();
    let mut apex_legacy = aim_services::package::owner::legacy_permissions::Migration::default();
    for user in permissions.users() {
        apex_legacy.set_missing(user.id, user.missing).unwrap();
        for permission in &user.permissions {
            apex_legacy.put(user.id, permission.clone()).unwrap();
        }
    }
    let detached_apex = apex_legacy.project(-1, &[10, 0, 11]).unwrap();
    fs::write(
        directory.join("legacy-permissions-apex.input"),
        detached_apex.bytes(),
    )
    .unwrap();
    let mut apex_features = aim_services::package::parse::Platform::load(
        &aim_paths::original_image(),
        Default::default(),
    )
    .unwrap()
    .features
    .into_iter()
    .collect::<Vec<_>>();
    apex_features.sort();
    fs::write(
        directory.join("apex-parser-features.input"),
        apex_features.join("\n"),
    )
    .unwrap();
    let original = boot.command().args([
        "shell", "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/package-parcels/oracle.dex:/system/framework/services.jar",
        "/system/bin", "PackageRoundTripOracle", "/data/local/tmp/package-parcels",
    ]).output().unwrap();
    assert!(
        original.status.success(),
        "original package oracle: {} {}",
        String::from_utf8_lossy(&original.stdout),
        String::from_utf8_lossy(&original.stderr)
    );
    assert_eq!(
        String::from_utf8(original.stdout).unwrap(),
        format!("PARCELS {}\n", expected.len())
    );
    for active in [false, true] {
        let location = aim_services::package::scan::Location {
            path: if active {
                "/apex/mount/app/provider"
            } else {
                "/system/app/provider"
            }
            .into(),
            partition: aim_services::package::scan::Partition::System,
            kind: aim_services::package::scan::Kind::App,
            apex: active.then(|| aim_services::package::scan::Apex {
                module_name: None,
                mount_path: "/apex/mount".into(),
                partition: aim_services::package::scan::Partition::System,
                factory: true,
                active_changed: false,
            }),
        };
        let mut native = static_identity.clone();
        aim_services::package::scan::Identity::select_for_location(
            &native,
            &Default::default(),
            true,
            &location,
        )
        .apply(&mut native);
        let mut original = AndroidPackage::read_cache_entry(
            &fs::read(directory.join(format!("static-identity-{active}.original"))).unwrap(),
        )
        .unwrap();
        normalize_maps(&mut native);
        normalize_maps(&mut original);
        assert!(
            native == original,
            "original static-library rename differs for active APEX {active}"
        );
    }
    let apex = aim_services::package::bootstrap::ApexInventory::read_original_record(
        &fs::read(directory.join("apex-inventory.original")).unwrap(),
    )
    .unwrap();
    let all = apex.packages.as_ref().unwrap();
    assert_eq!(all.len(), 4);
    assert_eq!(all[0].module_name.as_deref(), Some("same.module"));
    assert_eq!(all[1].module_name, all[0].module_name);
    assert_eq!(all[1].module_path, "/data/apex/active/updated.apex");
    assert_eq!(all[1].version_code, 0x100000007);
    assert!(all[0].factory && !all[0].active && !all[0].active_changed);
    assert!(!all[1].factory && all[1].active && all[1].active_changed);
    assert_eq!(all[2].module_name, None);
    let scan = apex.scan_apexes();
    assert_eq!(scan.len(), 2);
    assert_eq!(scan[0].module_name, None);
    assert_eq!(scan[0].mount_path, "/apex/null");
    assert_eq!(
        scan[0].partition,
        aim_services::package::scan::Partition::SystemExt
    );
    assert_eq!(
        scan[1].partition,
        aim_services::package::scan::Partition::Vendor
    );
    assert_eq!(scan[1].module_name.as_deref(), Some("same.module"));
    assert!(scan[1].active_changed && !scan[1].factory);
    for (phase, expected) in [("null", None), ("empty", Some(Vec::new()))] {
        assert_eq!(
            aim_services::package::bootstrap::ApexInventory::read_original_record(
                &fs::read(directory.join(format!("apex-inventory-{phase}.original"))).unwrap()
            )
            .unwrap()
            .packages,
            expected
        );
    }
    let live_apex = aim_services::package::bootstrap::ApexInventory::read_original_record(
        &fs::read(directory.join("apex-inventory-live.original")).unwrap(),
    )
    .unwrap();
    assert!(!live_apex.packages.as_ref().unwrap().is_empty());
    assert!(!live_apex.scan_apexes().is_empty());
    let scan_users = aim_services::package::bootstrap::ScanUsers::read_original_record(
        &fs::read(directory.join("scan-users.original")).unwrap(),
    )
    .unwrap()
    .users
    .unwrap();
    assert_eq!(
        scan_users
            .iter()
            .map(|u| (u.id, u.pre_created, u.adb_install_disallowed))
            .collect::<Vec<_>>(),
        [
            (0, false, false),
            (10, true, false),
            (11, false, true),
            (12, false, false)
        ]
    );
    for (phase, expected) in [("empty", Some(Vec::new())), ("uninitialized", None)] {
        assert_eq!(
            aim_services::package::bootstrap::ScanUsers::read_original_record(
                &fs::read(directory.join(format!("scan-users-{phase}.original"))).unwrap()
            )
            .unwrap()
            .users,
            expected
        );
    }
    let apex_inventory = aim_services::package::bootstrap::ApexInventory::read_original_record(
        &fs::read(directory.join("apex-inventory-parse.original")).unwrap(),
    )
    .unwrap();
    let original_root = aim_paths::original_image();
    let apex_apks = aim_services::package::write::Apks {
        files: Box::new(move |path| Some(original_root.join(path.trim_start_matches('/')))),
        platform: aim_services::package::parse::Platform::load(
            &aim_paths::original_image(),
            Default::default(),
        )
        .unwrap(),
    };
    let apex_image = aim_services::package::scan::ApexImage::load(
        &apex_apks,
        &apex_inventory,
        aim_services::package::parse::PARSE_IS_SYSTEM_DIR,
    )
    .unwrap();
    assert_eq!(
        apex_image.packages.len(),
        apex_inventory.packages.as_ref().unwrap().len()
    );
    for (index, info) in apex_inventory.packages.as_ref().unwrap().iter().enumerate() {
        let native = apex_image
            .packages
            .iter()
            .find(|p| p.info.module_path == info.module_path)
            .unwrap();
        let mut parsed = native.parsed.clone();
        let mut original = AndroidPackage::read_cache_entry(
            &fs::read(directory.join(format!("apex-parse-{index}.original"))).unwrap(),
        )
        .unwrap();
        normalize_maps(&mut parsed);
        normalize_maps(&mut original);
        assert!(
            parsed == original,
            "original APEX parse differs: {}",
            info.module_path
        );
        assert_eq!(
            native.scan_parse_flags,
            aim_services::package::parse::PARSE_IS_SYSTEM_DIR
        );
        assert!(!native.signing.signatures.is_empty());
        let mut signed = native.parsed.clone();
        signed.signing_details = Some(native.signing.parcel_details().unwrap());
        let mut signing = aim_binder_host::parcel::Parcel::new();
        aim_services::package::info::write_signing_details(
            &mut signing,
            aim_services::package::info::signing_info(&signed).as_ref(),
        );
        assert_eq!(
            signing.data(),
            fs::read(directory.join(format!("apex-signing-{index}.original"))).unwrap()
        );
        let mut capabilities = aim_binder_host::parcel::Parcel::new();
        capabilities.write_i32(
            native
                .signing
                .past_signing_certificates
                .as_ref()
                .map_or(-1, |p| p.len() as i32),
        );
        if let Some(past) = &native.signing.past_signing_certificates {
            for (_, flags) in past {
                capabilities.write_i32(*flags);
            }
        }
        assert_eq!(
            capabilities.data(),
            fs::read(directory.join(format!("apex-capabilities-{index}.original"))).unwrap()
        );
        assert_eq!(
            (i64::from(native.parsed.version_code_major) << 32)
                | i64::from(native.parsed.version_code as u32),
            info.version_code
        );
    }
    let apex_settings = aim_services::package::settings::Settings {
        packages: apex_image
            .packages
            .iter()
            .map(|code| aim_services::package::settings::Package {
                name: code.parsed.package_name.clone(),
                code_path: code.info.module_path.clone(),
                app_id: -1,
                version_code: code.info.version_code,
                flags: 1,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    for (index, _) in apex_inventory.packages.as_ref().unwrap().iter().enumerate() {
        assert_eq!(
            fs::read(directory.join(format!("apex-uid-{index}.original"))).unwrap(),
            (-1i32).to_le_bytes()
        );
    }
    let config = aim_services::package::system_config::SystemConfig::default();
    let owner = aim_services::package::scan::SigningScan::new_after_apex(
        &config,
        &apex_settings,
        36,
        &apex_image,
    )
    .unwrap();
    assert_eq!(owner.settings.packages, apex_settings.packages);
    assert_eq!(
        owner.identities,
        aim_services::package::owner::shared_users::Bootstrap::new(&config)
    );
    let mut ids = owner.identities.ids.clone();
    assert_eq!(
        ids.acquire(aim_services::package::owner::app_ids::Owner::Package(
            "first.apk".into()
        ))
        .unwrap(),
        10000
    );
    assert!(aim_services::package::scan::SigningScan::new(&config, &apex_settings, 36).is_err());
    for changed in 0..4 {
        let mut invalid = apex_settings.clone();
        match changed {
            0 => invalid.packages[0].app_id = 10000,
            1 => invalid.packages[0].code_path = "/data/app/foreign.apex".into(),
            2 => invalid.packages[0].version_code += 1,
            _ => invalid.packages.push(invalid.packages[0].clone()),
        }
        assert!(
            aim_services::package::scan::SigningScan::new_after_apex(
                &config,
                &invalid,
                36,
                &apex_image
            )
            .is_err()
        );
    }
    let mut with_disabled = apex_settings.clone();
    with_disabled
        .disabled_system_packages
        .push(apex_settings.packages[0].clone());
    let disabled_owner = aim_services::package::scan::SigningScan::new_after_apex(
        &config,
        &with_disabled,
        36,
        &apex_image,
    )
    .unwrap();
    let mut expected_disabled = with_disabled.clone();
    aim_services::package::owner::key_sets::restore(&mut expected_disabled).unwrap();
    assert!(
        disabled_owner.settings == expected_disabled,
        "disabled APEX settings changed outside keyset restoration"
    );
    assert_eq!(disabled_owner.identities, owner.identities);
    with_disabled.disabled_system_packages[0].app_id = 10000;
    assert!(
        aim_services::package::scan::SigningScan::new_after_apex(
            &config,
            &with_disabled,
            36,
            &apex_image
        )
        .is_err()
    );
    let mut mixed = apex_settings.clone();
    mixed
        .packages
        .push(aim_services::package::settings::Package {
            name: "installed.apk".into(),
            code_path: "/data/app/installed.apk".into(),
            app_id: 10000,
            ..Default::default()
        });
    let mixed_owner =
        aim_services::package::scan::SigningScan::new_after_apex(&config, &mixed, 36, &apex_image)
            .unwrap();
    let mut expected_mixed = mixed.clone();
    aim_services::package::owner::key_sets::restore(&mut expected_mixed).unwrap();
    assert!(
        mixed_owner.settings == expected_mixed,
        "mixed settings changed outside keyset restoration"
    );
    assert_eq!(
        mixed_owner.identities.ids.get(10000),
        Some(&aim_services::package::owner::app_ids::Owner::Package(
            "installed.apk".into()
        ))
    );
    let mut next = mixed_owner.identities.ids.clone();
    assert_eq!(
        next.acquire(aim_services::package::owner::app_ids::Owner::Package(
            "next.apk".into()
        ))
        .unwrap(),
        10001
    );
    let compatibility =
        aim_services::package::scan::LibraryCompatibility::new(&config, &|_| None, true).unwrap();
    let abi = aim_services::package::scan::AbiPolicy {
        all: vec!["arm64-v8a".into()],
        bit32: Vec::new(),
        bit64: vec!["arm64-v8a".into()],
        native32: Vec::new(),
        native64: vec!["arm64-v8a".into()],
        force_multi_arch_match: false,
    };
    let domain_counter = std::cell::Cell::new(1u8);
    let domain = || {
        let value = domain_counter.get();
        domain_counter.set(value + 1);
        Ok([value; 16])
    };
    let mut test_base_pkg = snapshot.owner().loaded_packages()["android"]
        .package
        .clone();
    test_base_pkg.booleans &= !aim_services::package::pkg::booleans::SYSTEM;
    test_base_pkg.uid = -1;
    fs::write(
        directory.join("test-base.cache"),
        test_base_pkg.to_cache_entry().unwrap().bytes,
    )
    .unwrap();
    let policy_classpath = "-Djava.class.path=/data/local/tmp/package-parcels/oracle.dex:/system/framework/aim-services.jar:/system/framework/services.jar";
    let policy_output = boot
        .client(1000)
        .args([
            "/system/bin/app_process",
            policy_classpath,
            "/system/bin",
            "dev.aim.server.PackageMigrationPolicyOracle",
            "/data/local/tmp/package-parcels",
        ])
        .output()
        .unwrap();
    assert_guest_success(&boot, &policy_output, "migration policy");
    let bytes = fs::read(directory.join("migration-policy.original")).unwrap();
    let mut reader = aim_binder_host::parcel::Reader::new(&bytes, &[]);
    let best_effort = aim_service_aidl::dev_aim_server_ipackagebootstrapbridge::read_is_shared_uid_migration_best_effort_reply(&mut reader).unwrap().unwrap();
    assert_eq!(reader.remaining(), 0);
    assert_eq!(
        String::from_utf8(policy_output.stdout).unwrap(),
        format!("MIGRATION_POLICY {}\n", i32::from(best_effort))
    );
    for sdk in [29, 30] {
        let bytes = fs::read(directory.join(format!("test-base-{sdk}.original"))).unwrap();
        let mut reader = aim_binder_host::parcel::Reader::new(&bytes, &[]);
        aim_service_aidl::dev_aim_server_ipackagebootstrapbridge::read_is_test_base_library_change_enabled_reply(&mut reader).unwrap().unwrap();
        assert_eq!(reader.remaining(), 0);
    }
    let migration_policy = if best_effort {
        aim_services::package::scan::SharedUidMigration::BestEffort
    } else {
        aim_services::package::scan::SharedUidMigration::NewInstallOnly
    };
    let denied = boot
        .client(2000)
        .args([
            "/system/bin/app_process",
            policy_classpath,
            "/system/bin",
            "dev.aim.server.PackageMigrationPolicyOracle",
            "/data/local/tmp/package-parcels",
        ])
        .output()
        .unwrap();
    assert_guest_success(&boot, &denied, "denied migration policy");
    assert_eq!(
        String::from_utf8(denied.stdout).unwrap(),
        "MIGRATION_POLICY_DENIED\n"
    );
    let scan_inputs = |image| aim_services::package::scan::FirstBootSystemInputs {
        seinfo: common::seinfo::scan(),
        apex_image: image,
        notify_apex_scan: &|_| Err("direct APEX phase does not notify".into()),
        first_api_level: 36,
        vendor_sdk: 36,
        shared_uid_migration: migration_policy,
        abi_policy: &abi,
        compatibility: &compatibility,
        preferred_abi: "arm64-v8a",
        app_lib32_install_dir: "/data/app-lib",
        platform_runtime_64bit: true,
        install: aim_services::package::scan::NativeLibraryInstallPolicy {
            page_size: 16384,
            extract: false,
            debuggable: false,
            compat_16kb_disabled: false,
            manifest_compat_disabled: false,
        },
        clock: aim_services::package::scan::ScanClock {
            current_time: 0,
            user_id: 0,
            update_time: false,
        },
        factory_test: false,
        users: aim_services::package::scan::UserPolicy {
            install_user: Some(0),
            users: None,
            allow_install: true,
            instant_app: false,
            virtual_preload: false,
            stopped_system_app: false,
        },
        new_domain_id: &domain,
    };
    let mut registered =
        aim_services::package::scan::SigningScan::new(&config, &Default::default(), 36).unwrap();
    let results = registered
        .scan_initial_apex(&apex_apks, &config, &scan_inputs(&apex_image))
        .unwrap();
    let notification =
        aim_services::package::scan::ApexScanResult::notification_payload(&results).unwrap();
    fs::write(directory.join("apex-notify.input"), &notification).unwrap();
    let mut invalid_result = results[0].clone();
    invalid_result.package.uid = 10000;
    assert!(
        aim_services::package::scan::ApexScanResult::notification_payload(&[invalid_result])
            .is_err()
    );
    assert_eq!(results.len(), apex_image.packages.len());
    assert_eq!(registered.loaded_packages().len(), results.len());
    assert_eq!(
        registered.identities,
        aim_services::package::owner::shared_users::Bootstrap::new(&config)
    );
    assert!(registered.settings.key_sets.public_keys.is_empty());
    assert!(registered.settings.key_sets.key_sets.is_empty());
    for result in &results {
        let setting = registered
            .settings
            .packages
            .iter()
            .find(|p| p.name == result.package.package_name)
            .unwrap();
        assert_eq!(setting.app_id, -1);
        assert_eq!(result.package.uid, -1);
        assert!(
            result
                .package
                .is2(aim_services::package::pkg::booleans2::APEX)
        );
        assert_eq!(setting.transient.apex_module_name, result.info.module_name);
        assert!(!setting.transient.apk_in_updated_apex);
        assert_eq!(setting.key_set_data, Default::default());
        assert!(setting.signatures.is_some());
        assert_eq!(setting.primary_cpu_abi, None);
        assert_eq!(setting.legacy_native_library_path, None);
        assert!(
            registered
                .seinfo_state(&setting.name)
                .unwrap()
                .unwrap()
                .base
                .is_some()
        );
    }
    let mut shared_image = aim_services::package::scan::ApexImage {
        packages: vec![apex_image.packages[0].clone()],
    };
    shared_image.packages[0].parsed.shared_user_id = Some("aim.fixture.apex".into());
    let unread = aim_services::package::owner::seinfo::Policy::unread();
    let shared_compatibility_calls = std::cell::Cell::new(0);
    let shared_compatibility = |pkg: &aim_services::package::pkg::AndroidPackage| {
        shared_compatibility_calls.set(shared_compatibility_calls.get() + 1);
        Ok(pkg.target_sdk_version)
    };
    let mut shared_inputs = scan_inputs(&shared_image);
    shared_inputs.seinfo.policy = &unread;
    shared_inputs.seinfo.compatibility = &shared_compatibility;
    let mut shared =
        aim_services::package::scan::SigningScan::new(&config, &Default::default(), 36).unwrap();
    let shared_results = shared
        .scan_initial_apex(&apex_apks, &config, &shared_inputs)
        .unwrap();
    assert_eq!(shared_compatibility_calls.get(), 1);
    shared
        .scan_initial_apex(&apex_apks, &config, &shared_inputs)
        .unwrap();
    assert_eq!(shared_compatibility_calls.get(), 1);
    let reject_shared_compatibility = |_: &aim_services::package::pkg::AndroidPackage| {
        Err("shared compatibility owner denied scan".into())
    };
    let mut rejected_inputs = scan_inputs(&shared_image);
    rejected_inputs.seinfo.compatibility = &reject_shared_compatibility;
    let mut rejected =
        aim_services::package::scan::SigningScan::new(&config, &Default::default(), 36).unwrap();
    assert!(
        matches!(rejected.scan_initial_apex(&apex_apks, &config, &rejected_inputs),
        Err(aim_services::package::scan::SigningError::Rejected(ref error))
            if error.phase == "seinfo" && error.message == "shared compatibility owner denied scan")
    );
    assert!(rejected.settings.packages.is_empty());
    assert!(rejected.loaded_packages().is_empty());
    assert_eq!(
        rejected.identities.shared_users["aim.fixture.apex"].app_id,
        10000
    );
    let setting = &shared.settings.packages[0];
    assert_eq!(setting.app_id, 10000);
    assert_eq!(setting.shared_app_id(), Some(10000));
    assert_eq!(shared_results[0].package.uid, -1);
    let group = &shared.identities.shared_users["aim.fixture.apex"];
    assert_eq!(group.signatures, setting.signatures);
    assert_eq!(
        group.seinfo_target_sdk(),
        shared_results[0].package.target_sdk_version
    );
    assert_eq!(shared.identities.ids.get(-1), None);
    let restored = aim_services::package::scan::SigningScan::new_after_apex(
        &config,
        &shared.settings,
        36,
        &shared_image,
    )
    .unwrap();
    let mut ids = restored.identities.ids.clone();
    assert_eq!(
        ids.acquire(aim_services::package::owner::app_ids::Owner::Package(
            "next.apk".into()
        ))
        .unwrap(),
        10001
    );
    let mut native = aim_binder_host::parcel::Parcel::new();
    aim_service_aidl::write_byte_array(
        &mut native,
        Some(&shared_results[0].package.to_cache_entry().unwrap().bytes),
    );
    native.write_i32(setting.flags);
    native.write_i32(setting.private_flags);
    native.write_string16(shared.seinfo(&setting.name).unwrap());
    native.write_i32(group.seinfo_target_sdk());
    native.write_i32(setting.app_id);
    fs::write(directory.join("shared-apex.input"), native.data()).unwrap();
    let shared_without_factory = shared.clone();
    let mut leaving_image = aim_services::package::scan::ApexImage {
        packages: shared_image.packages.clone(),
    };
    leaving_image.packages[0].parsed.booleans |=
        aim_services::package::pkg::booleans::LEAVING_SHARED_UID;
    let mut leaving_inputs = scan_inputs(&leaving_image);
    leaving_inputs.shared_uid_migration =
        aim_services::package::scan::SharedUidMigration::NewInstallOnly;
    leaving_inputs.seinfo.policy = &unread;
    leaving_inputs.seinfo.compatibility = &shared_compatibility;
    let mut not_migrated = shared_without_factory.clone();
    not_migrated
        .scan_initial_apex(&apex_apks, &config, &leaving_inputs)
        .unwrap();
    assert_eq!(
        not_migrated.settings.packages[0].shared_app_id(),
        Some(10000)
    );
    leaving_inputs.shared_uid_migration =
        aim_services::package::scan::SharedUidMigration::BestEffort;
    let mut new_leaving =
        aim_services::package::scan::SigningScan::new(&config, &Default::default(), 36).unwrap();
    let new_result = new_leaving
        .scan_initial_apex(&apex_apks, &config, &leaving_inputs)
        .unwrap();
    assert_eq!(new_leaving.settings.packages[0].shared_app_id(), None);
    assert_eq!(new_leaving.settings.packages[0].app_id, -1);
    assert_eq!(new_result[0].package.uid, -1);
    assert_eq!(
        new_leaving.identities,
        aim_services::package::owner::shared_users::Bootstrap::new(&config)
    );
    let mut converted = shared_without_factory.clone();
    let name = converted.settings.packages[0].name.clone();
    converted
        .capture_legacy_permissions(
            &[10, 0, 11],
            std::collections::BTreeMap::from([((name.clone(), false), apex_legacy.clone())]),
            converted
                .identities
                .shared_users
                .keys()
                .map(|name| (name.clone(), Default::default()))
                .collect(),
        )
        .unwrap();
    converted
        .capture_install_permissions_fixed(std::collections::BTreeMap::from([(
            (name.clone(), false),
            true,
        )]))
        .unwrap();
    let mut failed_conversion = converted.clone();
    let mut denied_conversion = scan_inputs(&leaving_image);
    denied_conversion.shared_uid_migration =
        aim_services::package::scan::SharedUidMigration::BestEffort;
    denied_conversion.seinfo.compatibility = &reject_shared_compatibility;
    // A retained group supplies its frozen SDK, so use a domain failure to
    // reject this candidate before the conversion owner is reached.
    let denied_domain = || Err("conversion domain owner denied scan".into());
    denied_conversion.new_domain_id = &denied_domain;
    assert!(
        matches!(failed_conversion.scan_initial_apex(&apex_apks, &config, &denied_conversion),
        Err(aim_services::package::scan::SigningError::Rejected(ref error)) if error.phase == "apex-domain")
    );
    assert_eq!(failed_conversion, converted);
    let result = converted
        .scan_initial_apex(&apex_apks, &config, &leaving_inputs)
        .unwrap();
    assert_eq!(converted.settings.packages[0].app_id, -1);
    assert_eq!(converted.settings.packages[0].shared_app_id(), None);
    assert_eq!(result[0].package.uid, -1);
    assert_eq!(
        converted.identities.ids.get(10000),
        Some(&aim_services::package::owner::app_ids::Owner::Package(
            name.clone()
        ))
    );
    assert!(
        !converted
            .identities
            .shared_users
            .contains_key("aim.fixture.apex")
    );
    assert_eq!(
        converted.legacy_permissions(&name, false).unwrap().unwrap(),
        detached_apex
    );
    assert_eq!(
        converted.install_permissions_fixed(&name, false).unwrap(),
        Some(true)
    );
    assert_eq!(
        converted.seinfo(&name).unwrap(),
        shared_without_factory.seinfo(&name).unwrap()
    );
    let mut ids = converted.identities.ids.clone();
    assert_eq!(
        ids.acquire(aim_services::package::owner::app_ids::Owner::Package(
            "next.apk".into()
        ))
        .unwrap(),
        10001
    );
    fs::write(
        directory.join("apex-conversion-legacy.input"),
        converted
            .legacy_permissions(&name, false)
            .unwrap()
            .unwrap()
            .bytes(),
    )
    .unwrap();
    let mut conversion = aim_binder_host::parcel::Parcel::new();
    conversion.write_i32(converted.settings.packages[0].app_id);
    conversion.write_i32(converted.settings.packages[0].shared_app_id().unwrap_or(-1));
    conversion.write_bool(converted.identities.ids.get(10000).is_some());
    fs::write(directory.join("apex-conversion.input"), conversion.data()).unwrap();
    converted
        .scan_initial_apex(&apex_apks, &config, &leaving_inputs)
        .unwrap();
    assert_eq!(converted.settings.packages[0].shared_app_id(), None);
    assert_eq!(
        converted.identities.ids.get(10000),
        Some(&aim_services::package::owner::app_ids::Owner::Package(
            name.clone()
        ))
    );
    let mut blocks_conversion = shared_without_factory.clone();
    blocks_conversion.disable_system_package(&name).unwrap();
    blocks_conversion
        .scan_initial_apex(&apex_apks, &config, &leaving_inputs)
        .unwrap();
    assert_eq!(
        blocks_conversion.settings.packages[0].shared_app_id(),
        Some(10000)
    );
    let mut factory_conversion = not_migrated.clone();
    factory_conversion.disable_system_package(&name).unwrap();
    factory_conversion
        .capture_legacy_permissions(
            &[10, 0, 11],
            std::collections::BTreeMap::from([
                ((name.clone(), false), Default::default()),
                ((name.clone(), true), apex_legacy.clone()),
            ]),
            factory_conversion
                .identities
                .shared_users
                .keys()
                .map(|name| (name.clone(), Default::default()))
                .collect(),
        )
        .unwrap();
    factory_conversion
        .capture_install_permissions_fixed(std::collections::BTreeMap::from([
            ((name.clone(), false), false),
            ((name.clone(), true), true),
        ]))
        .unwrap();
    let factory_legacy = factory_conversion
        .legacy_permissions(&name, true)
        .unwrap()
        .unwrap();
    factory_conversion
        .scan_initial_apex(&apex_apks, &config, &leaving_inputs)
        .unwrap();
    assert_eq!(factory_conversion.settings.packages[0].app_id, -1);
    assert_eq!(
        factory_conversion.settings.packages[0].shared_app_id(),
        None
    );
    assert_eq!(
        factory_conversion.settings.disabled_system_packages[0].app_id,
        10000
    );
    assert_eq!(
        factory_conversion.settings.disabled_system_packages[0].shared_app_id(),
        None
    );

    assert_eq!(
        factory_conversion
            .legacy_permissions(&name, true)
            .unwrap()
            .unwrap(),
        factory_legacy
    );
    assert_eq!(
        factory_conversion
            .legacy_permissions(&name, false)
            .unwrap()
            .unwrap(),
        aim_services::package::owner::legacy_permissions::Migration::default()
            .project(-1, &[10, 0, 11])
            .unwrap()
    );
    assert_eq!(
        factory_conversion
            .install_permissions_fixed(&name, false)
            .unwrap(),
        Some(false)
    );
    assert_eq!(
        factory_conversion
            .install_permissions_fixed(&name, true)
            .unwrap(),
        Some(true)
    );
    aim_services::package::scan::SigningScan::new_after_apex(
        &config,
        &factory_conversion.settings,
        36,
        &leaving_image,
    )
    .unwrap();
    let mut multiple = shared_without_factory.clone();
    multiple
        .settings
        .packages
        .push(aim_services::package::settings::Package {
            name: "fixture.other".into(),
            app_id: 10000,
            shared_user: true,
            shared_user_app_id: Some(10000),
            ..Default::default()
        });
    multiple
        .identities
        .shared_users
        .get_mut("aim.fixture.apex")
        .unwrap()
        .add_package("fixture.other", 0, 0);
    multiple
        .scan_initial_apex(&apex_apks, &config, &leaving_inputs)
        .unwrap();
    assert_eq!(multiple.settings.packages[0].shared_app_id(), Some(10000));
    assert_eq!(multiple.settings.packages[1].shared_app_id(), Some(10000));
    let mut cases = aim_binder_host::parcel::Parcel::new();
    aim_service_aidl::write_byte_array(
        &mut cases,
        Some(&result[0].package.to_cache_entry().unwrap().bytes),
    );
    cases.write_i32(4);
    for (mode, state) in [
        (0, &converted),
        (1, &blocks_conversion),
        (2, &factory_conversion),
        (3, &multiple),
    ] {
        cases.write_i32(mode);
        cases.write_bool(state.settings.packages[0].shared_app_id().is_none());
        cases.write_i32(state.settings.packages[0].app_id);
        cases.write_i32(state.settings.packages[0].shared_app_id().unwrap_or(-1));
        if mode == 1 || mode == 2 {
            cases.write_i32(state.settings.disabled_system_packages[0].app_id);
            cases.write_i32(
                state.settings.disabled_system_packages[0]
                    .shared_app_id()
                    .unwrap_or(-1),
            );
        }
    }
    fs::write(directory.join("apex-conversion-cases.input"), cases.data()).unwrap();

    shared
        .disable_system_package(&setting.name.clone())
        .unwrap();
    assert_eq!(
        shared.settings.disabled_system_packages[0].shared_app_id(),
        Some(10000)
    );
    assert_eq!(shared.settings.disabled_system_packages[0].app_id, 10000);
    aim_services::package::scan::SigningScan::new_after_apex(
        &config,
        &shared.settings,
        36,
        &shared_image,
    )
    .unwrap();
    let replacement_guest = "/data/apex/active/shared-replacement.apex";
    let replacement_file = directory.join("shared-replacement.apex");
    std::os::unix::fs::symlink(
        aim_paths::original_image().join(
            shared_image.packages[0]
                .info
                .module_path
                .trim_start_matches('/'),
        ),
        &replacement_file,
    )
    .unwrap();
    let original_root = aim_paths::original_image();
    let replacement_apks = aim_services::package::write::Apks {
        files: Box::new(move |path| {
            Some(if path == replacement_guest {
                replacement_file.clone()
            } else {
                original_root.join(path.trim_start_matches('/'))
            })
        }),
        platform: aim_services::package::parse::Platform::load(
            &aim_paths::original_image(),
            Default::default(),
        )
        .unwrap(),
    };
    let mut replacement_info = shared_image.packages[0].info.clone();
    replacement_info.module_path = replacement_guest.into();
    replacement_info.factory = false;
    let mut replacement_image = aim_services::package::scan::ApexImage::load(
        &replacement_apks,
        &aim_services::package::bootstrap::ApexInventory {
            packages: Some(vec![replacement_info]),
            active: Vec::new(),
        },
        aim_services::package::parse::PARSE_IS_SYSTEM_DIR,
    )
    .unwrap();
    replacement_image.packages[0].parsed.shared_user_id = Some("aim.fixture.replacement".into());
    let mut replacement_inputs = scan_inputs(&replacement_image);
    replacement_inputs.seinfo.policy = &unread;
    replacement_inputs.seinfo.compatibility = &shared_compatibility;
    let capture_apex_legacy = |scan: &mut aim_services::package::scan::SigningScan| {
        let packages = scan
            .settings
            .packages
            .iter()
            .map(|p| ((p.name.clone(), false), apex_legacy.clone()))
            .chain(
                scan.settings
                    .disabled_system_packages
                    .iter()
                    .map(|p| ((p.name.clone(), true), apex_legacy.clone())),
            )
            .collect();
        let fixed = scan
            .settings
            .packages
            .iter()
            .map(|p| ((p.name.clone(), false), true))
            .chain(
                scan.settings
                    .disabled_system_packages
                    .iter()
                    .map(|p| ((p.name.clone(), true), true)),
            )
            .collect();
        let groups = scan
            .identities
            .shared_users
            .keys()
            .map(|name| {
                (
                    name.clone(),
                    if name == "aim.fixture.apex" {
                        apex_legacy.clone()
                    } else {
                        Default::default()
                    },
                )
            })
            .collect();
        scan.capture_legacy_permissions(&[10, 0, 11], packages, groups)
            .unwrap();
        scan.capture_install_permissions_fixed(fixed).unwrap();
    };
    let mut adoption_image = aim_services::package::scan::ApexImage {
        packages: vec![apex_image.packages[0].clone()],
    };
    adoption_image.packages[0].parsed.original_packages =
        Some(vec![Some("original.fixture".into())]);
    let mut original_setting = registered.settings.packages[0].clone();
    original_setting.name = "original.fixture".into();
    original_setting.app_id = 10000;
    original_setting.code_path = "/system/apex/original.fixture.apex".into();
    original_setting.version_code = 1;
    let original_settings = aim_services::package::settings::Settings {
        packages: vec![original_setting.clone()],
        ..Default::default()
    };
    let original_users = BTreeMap::from([(
        0,
        aim_services::package::restrictions::UserState {
            enabled: 2,
            hidden: true,
            first_install_time: 123,
            ..aim_services::package::restrictions::UserState::initialized()
        },
    )]);
    let mut adoption =
        aim_services::package::scan::SigningScan::new(&config, &original_settings, 36).unwrap();
    adoption
        .capture_user_states(BTreeMap::from([(
            ("original.fixture".into(), false),
            aim_services::package::scan::CapturedUsers {
                states: original_users.clone(),
                active_aliases: Default::default(),
            },
        )]))
        .unwrap();
    capture_apex_legacy(&mut adoption);
    let prior = adoption.clone();
    let reject_adoption_domain = || Err("adoption domain owner rejected".into());
    let mut rejected_adoption_inputs = scan_inputs(&adoption_image);
    rejected_adoption_inputs.new_domain_id = &reject_adoption_domain;
    assert!(
        adoption
            .scan_initial_apex(&apex_apks, &config, &rejected_adoption_inputs)
            .is_err()
    );
    assert!(
        adoption == prior,
        "rejected adoption changed prior UID/settings owners"
    );
    let adopted_results = adoption
        .scan_initial_apex(&apex_apks, &config, &scan_inputs(&adoption_image))
        .unwrap();
    assert_eq!(adopted_results[0].package.package_name, "original.fixture");
    assert_eq!(adopted_results[0].package.uid, -1);
    assert_eq!(adoption.settings.packages[0].app_id, -1);
    assert_eq!(
        adoption.settings.packages[0].real_name.as_deref(),
        Some(adoption_image.packages[0].parsed.package_name.as_str())
    );
    assert_eq!(
        adoption.scanned_user_states("original.fixture"),
        Some(&original_users)
    );
    assert_eq!(
        adoption.identities.ids.get(10000),
        Some(
            &aim_services::package::owner::app_ids::Owner::DetachedPackage(
                "original.fixture".into()
            )
        )
    );
    let slot = adoption.identities.ids.detached_setting(10000).unwrap();
    assert_eq!(slot.package, original_setting);
    assert_eq!(slot.users, original_users);
    assert_eq!(slot.install_fixed, Some(true));
    assert_eq!(
        slot.legacy.as_ref().unwrap(),
        &apex_legacy.project(10000, &[10, 0, 11]).unwrap()
    );
    assert_eq!(
        prior.identities.ids.get(10000),
        Some(&aim_services::package::owner::app_ids::Owner::Package(
            "original.fixture".into()
        ))
    );
    assert_eq!(prior.identities.ids.detached_setting(10000), None);
    let mut next_adoption_ids = adoption.identities.ids.clone();
    assert_eq!(
        next_adoption_ids
            .acquire(aim_services::package::owner::app_ids::Owner::Package(
                "next.apk".into()
            ))
            .unwrap(),
        10001
    );
    let mut native_adoption = aim_binder_host::parcel::Parcel::new();
    native_adoption.write_i32(slot.package.app_id);
    native_adoption.write_i32(adoption.settings.packages[0].app_id);
    aim_service_aidl::write_byte_array(
        &mut native_adoption,
        Some(&slot.legacy.as_ref().unwrap().bytes()),
    );
    aim_service_aidl::write_byte_array(
        &mut native_adoption,
        Some(
            &adoption
                .legacy_permissions("original.fixture", false)
                .unwrap()
                .unwrap()
                .bytes(),
        ),
    );
    fs::write(
        directory.join("apex-original-adoption.input"),
        native_adoption.data(),
    )
    .unwrap();
    let captured_slot = slot.clone();
    adoption
        .scan_initial_apex(&apex_apks, &config, &scan_inputs(&adoption_image))
        .unwrap();
    adoption
        .set_install_permissions_fixed("original.fixture", false, false)
        .unwrap();
    let mut changed_user = original_users[&0].clone();
    changed_user.hidden = false;
    changed_user.installed = false;
    adoption
        .set_user_state("original.fixture", 0, changed_user)
        .unwrap();
    let mut absent_user = aim_services::package::restrictions::UserState::initialized();
    absent_user.installed = false;
    adoption
        .set_user_state("original.fixture", 10, absent_user.clone())
        .unwrap();
    let aliased_slot = adoption.identities.ids.detached_setting(10000).unwrap();
    assert!(!aliased_slot.users[&0].hidden && !aliased_slot.users[&0].installed);
    assert!(!aliased_slot.users.contains_key(&10));
    assert!(captured_slot.users[&0].hidden && captured_slot.users[&0].installed);
    assert_eq!(aliased_slot.package, captured_slot.package);
    assert_eq!(aliased_slot.legacy, captured_slot.legacy);
    assert_eq!(aliased_slot.install_fixed, Some(true));
    let mut alias_record = aim_binder_host::parcel::Parcel::new();
    alias_record.write_bool(aliased_slot.aliases_user(0));
    alias_record.write_bool(aliased_slot.users[&0].installed);
    alias_record
        .write_bool(adoption.scanned_user_states("original.fixture").unwrap()[&0].installed);
    alias_record.write_bool(!aliased_slot.users.contains_key(&10));
    alias_record
        .write_bool(adoption.scanned_user_states("original.fixture").unwrap()[&10].installed);
    alias_record.write_bool(captured_slot.users[&0].installed);
    fs::write(
        directory.join("apex-original-user-alias.input"),
        alias_record.data(),
    )
    .unwrap();
    assert_eq!(
        adoption
            .install_permissions_fixed("original.fixture", false)
            .unwrap(),
        Some(false)
    );
    let mut shared_adoption_image = aim_services::package::scan::ApexImage {
        packages: adoption_image.packages.clone(),
    };
    shared_adoption_image.packages[0].parsed.shared_user_id = Some("shared.fixture".into());
    shared_adoption_image.packages[0].parsed.booleans |=
        aim_services::package::pkg::booleans::LEAVING_SHARED_UID;
    let mut shared_original = original_setting.clone();
    shared_original.shared_user = true;
    let shared_original_settings = aim_services::package::settings::Settings {
        packages: vec![shared_original.clone()],
        shared_users: vec![aim_services::package::settings::SharedUser {
            name: "shared.fixture".into(),
            app_id: 10000,
            ..Default::default()
        }],
        ..Default::default()
    };
    let mut shared_adoption =
        aim_services::package::scan::SigningScan::new(&config, &shared_original_settings, 36)
            .unwrap();
    shared_adoption
        .capture_user_states(BTreeMap::from([(
            ("original.fixture".into(), false),
            aim_services::package::scan::CapturedUsers {
                states: original_users.clone(),
                active_aliases: Default::default(),
            },
        )]))
        .unwrap();
    capture_apex_legacy(&mut shared_adoption);
    shared_adoption
        .capture_replica_runtime(BTreeMap::from([(
            ("original.fixture".into(), false),
            aim_services::package::scan::ReplicaRuntime {
                usage: [0; 8],
                seinfo: None,
                override_seinfo: None,
                library_files: vec![],
                libraries: vec![],
            },
        )]))
        .unwrap();
    let prior_shared_adoption = shared_adoption.clone();
    let mut shared_adoption_inputs = scan_inputs(&shared_adoption_image);
    shared_adoption_inputs.shared_uid_migration =
        aim_services::package::scan::SharedUidMigration::BestEffort;
    let shared_adoption_results = shared_adoption
        .scan_initial_apex(&apex_apks, &config, &shared_adoption_inputs)
        .unwrap();
    assert_eq!(shared_adoption_results[0].package.uid, -1);
    let current = &shared_adoption.settings.packages[0];
    assert!(current.shared_user);
    assert_eq!(current.app_id, 10000);
    let group = &shared_adoption.identities.shared_users["shared.fixture"];
    assert_eq!(group.member_count(), 2);
    assert_eq!(group.seinfo_target_sdk(), 10000);
    let retained = group.retained_setting("original.fixture").unwrap();
    assert_eq!(retained.package, shared_original);
    assert_eq!(retained.users, original_users);
    assert_eq!(retained.install_fixed, Some(true));
    assert_eq!(
        retained.legacy.as_ref().unwrap(),
        &apex_legacy.project(10000, &[10, 0, 11]).unwrap()
    );
    assert_eq!(
        prior_shared_adoption.identities.shared_users["shared.fixture"].member_count(),
        1
    );
    assert_eq!(
        prior_shared_adoption.identities.shared_users["shared.fixture"]
            .retained_setting("original.fixture"),
        None
    );
    assert_eq!(
        shared_adoption.identities.ids.get(10000),
        Some(&aim_services::package::owner::app_ids::Owner::SharedUser(
            "shared.fixture".into()
        ))
    );
    let mut shared_flags = aim_binder_host::parcel::Parcel::new();
    shared_flags.write_i32(shared_original.flags);
    shared_flags.write_i32(shared_original.private_flags);
    shared_flags.write_i32(current.flags);
    shared_flags.write_i32(current.private_flags);
    fs::write(
        directory.join("apex-shared-original-adoption.flags"),
        shared_flags.data(),
    )
    .unwrap();
    let mut shared_adoption_record = aim_binder_host::parcel::Parcel::new();
    shared_adoption_record.write_i32(group.member_count() as i32);
    shared_adoption_record.write_bool(!current.shared_user);
    shared_adoption_record.write_i32(group.seinfo_target_sdk());
    shared_adoption_record.write_i32(retained.package.app_id);
    shared_adoption_record.write_i32(current.app_id);
    fs::write(
        directory.join("apex-shared-original-adoption.input"),
        shared_adoption_record.data(),
    )
    .unwrap();
    let prior_shared_record = retained.clone();
    let mut publishable = shared_adoption.clone();
    publishable
        .complete_library_dependencies(&|_, _| {
            Ok(aim_services::package::libraries::Policy::pinned(false))
        })
        .unwrap();
    let captured_usage = aim_services::package::owner::usage::Usage::new(["original.fixture"]);
    publishable
        .complete_runtime_at_boot(&captured_usage, BTreeMap::new())
        .unwrap();
    let process_orders = publishable
        .identities
        .shared_users
        .iter()
        .map(|(name, group)| {
            (
                name.clone(),
                if name == "shared.fixture" {
                    vec![
                        ("original.fixture".into(), false),
                        ("original.fixture".into(), true),
                    ]
                } else {
                    assert_eq!(group.member_count(), 0);
                    vec![]
                },
            )
        })
        .collect();
    publishable
        .complete_shared_process_instances(process_orders)
        .unwrap();
    let captured_apex = aim_services::package::scan_snapshot::Store::new(
        publishable.clone(),
        captured_usage.clone(),
    )
    .unwrap()
    .capture();
    let mut wrong_setting_uid = publishable.clone();
    wrong_setting_uid.settings.packages[0].app_id = 10001;
    assert!(
        aim_services::package::scan_snapshot::Store::new(wrong_setting_uid, captured_usage)
            .is_err()
    );
    std::fs::write(
        directory.join("apex-shared-captured.setting"),
        aim_services::package::scan_snapshot::setting_record::captured(
            &captured_apex,
            "original.fixture",
            false,
        )
        .unwrap()
        .unwrap(),
    )
    .unwrap();
    let code = aim_services::package::scan_snapshot::endpoint::PackageCode::captured(
        &captured_apex,
        "original.fixture",
        false,
    )
    .unwrap()
    .unwrap();
    let mut code_frame = aim_binder_host::parcel::Parcel::new();
    aim_service_aidl::WriteParcelable::write_to(&code, &mut code_frame);
    std::fs::write(
        directory.join("apex-shared-captured.code"),
        code_frame.data(),
    )
    .unwrap();

    std::fs::write(
        directory.join("apex-shared-instance.record"),
        aim_services::package::scan_snapshot::shared_record::captured_owner(
            17,
            &shared_adoption,
            "shared.fixture",
        )
        .unwrap()
        .unwrap(),
    )
    .unwrap();
    let prior_shared_identities = shared_adoption.identities.clone();
    assert!(
        !shared_adoption
            .migrate_single_shared_user(
                "shared.fixture",
                aim_services::package::scan::SharedUidMigration::BestEffort,
                &Default::default(),
            )
            .unwrap()
    );
    assert_eq!(shared_adoption.identities, prior_shared_identities);
    shared_adoption
        .scan_initial_apex(&apex_apks, &config, &shared_adoption_inputs)
        .unwrap();
    shared_adoption
        .set_install_permissions_fixed("original.fixture", false, false)
        .unwrap();
    assert_eq!(
        shared_adoption.identities.shared_users["shared.fixture"].member_count(),
        2
    );
    assert_eq!(
        shared_adoption.identities.shared_users["shared.fixture"]
            .retained_setting("original.fixture"),
        Some(&prior_shared_record)
    );
    // The removal owner has already withdrawn collected code; preserve the
    // exact runtime group objects while constructing its unloaded setting phase.
    let mut shared_changed_user = original_users[&0].clone();
    shared_changed_user.installed = false;
    shared_adoption
        .set_user_state("original.fixture", 0, shared_changed_user)
        .unwrap();
    shared_adoption
        .set_user_state("original.fixture", 10, absent_user)
        .unwrap();
    let shared_alias = shared_adoption.identities.shared_users["shared.fixture"]
        .retained_setting("original.fixture")
        .unwrap();
    let mut shared_alias_record = aim_binder_host::parcel::Parcel::new();
    shared_alias_record.write_bool(shared_alias.aliases_user(0));
    shared_alias_record.write_bool(shared_alias.users[&0].installed);
    shared_alias_record.write_bool(
        shared_adoption
            .scanned_user_states("original.fixture")
            .unwrap()[&0]
            .installed,
    );
    shared_alias_record.write_bool(!shared_alias.users.contains_key(&10));
    shared_alias_record.write_bool(
        shared_adoption
            .scanned_user_states("original.fixture")
            .unwrap()[&10]
            .installed,
    );
    shared_alias_record.write_bool(prior_shared_record.users[&0].installed);
    fs::write(
        directory.join("apex-shared-user-alias.input"),
        shared_alias_record.data(),
    )
    .unwrap();
    let retained_after_alias = shared_alias.clone();
    let mut shared_removal =
        aim_services::package::scan::SigningScan::new(&config, &shared_adoption.settings, 36)
            .unwrap();
    shared_removal.identities = shared_adoption.identities.clone();
    let removed = shared_removal
        .remove_package_setting("original.fixture")
        .unwrap()
        .unwrap();
    assert!(!removed.app_id_removed);
    let remaining = &shared_removal.identities.shared_users["shared.fixture"];
    assert_eq!(remaining.member_count(), 1);
    assert_eq!(
        remaining
            .retained_setting("original.fixture")
            .unwrap()
            .users,
        retained_after_alias.users
    );
    assert!(
        !remaining
            .retained_setting("original.fixture")
            .unwrap()
            .aliases_user(0)
    );
    let mut removal_record = aim_binder_host::parcel::Parcel::new();
    removal_record.write_i32(remaining.member_count() as i32);
    removal_record.write_bool(removed.app_id_removed);
    removal_record.write_i32(remaining.flags);
    removal_record.write_i32(remaining.private_flags);
    removal_record.write_i32(remaining.seinfo_target_sdk());
    removal_record.write_bool(
        shared_removal.identities.ids.get(10000)
            == Some(&aim_services::package::owner::app_ids::Owner::SharedUser(
                "shared.fixture".into(),
            )),
    );
    fs::write(
        directory.join("apex-shared-original-removal.input"),
        removal_record.data(),
    )
    .unwrap();
    let mut renamed_image = aim_services::package::scan::ApexImage {
        packages: vec![apex_image.packages[0].clone()],
    };
    let manifest_name = renamed_image.packages[0].parsed.package_name.clone();
    let old_name = "aim.fixture.original.apex";
    let mut old_image = aim_services::package::scan::ApexImage {
        packages: renamed_image.packages.clone(),
    };
    old_image.packages[0].parsed.package_name = old_name.into();
    old_image.packages[0].parsed.manifest_package_name = Some(old_name.into());
    let mut renamed =
        aim_services::package::scan::SigningScan::new(&config, &Default::default(), 36).unwrap();
    renamed
        .scan_initial_apex(&apex_apks, &config, &scan_inputs(&old_image))
        .unwrap();
    renamed.settings.packages[0].real_name = Some(manifest_name.clone());
    renamed
        .settings
        .renamed_packages
        .push((manifest_name.clone(), old_name.into()));
    renamed_image.packages[0].parsed.original_packages = Some(vec![Some(old_name.into())]);
    capture_apex_legacy(&mut renamed);
    let legacy = renamed
        .legacy_permissions(old_name, false)
        .unwrap()
        .unwrap();
    let restored = aim_services::package::scan::SigningScan::new_after_apex(
        &config,
        &renamed.settings,
        36,
        &renamed_image,
    )
    .unwrap();
    assert_eq!(restored.settings, renamed.settings);
    let mut no_mapping = renamed.settings.clone();
    no_mapping.renamed_packages.clear();
    assert!(
        aim_services::package::scan::SigningScan::new_after_apex(
            &config,
            &no_mapping,
            36,
            &renamed_image,
        )
        .is_err()
    );
    let renamed_results = renamed
        .scan_initial_apex(&apex_apks, &config, &scan_inputs(&renamed_image))
        .unwrap();
    assert_eq!(renamed_results[0].package.package_name, old_name);
    assert_eq!(renamed_results[0].package.uid, -1);
    assert_eq!(renamed.settings.packages.len(), 1);
    let retained = &renamed.settings.packages[0];
    assert_eq!(retained.name, old_name);
    assert_eq!(retained.real_name.as_deref(), Some(manifest_name.as_str()));
    assert_eq!(
        renamed
            .legacy_permissions(old_name, false)
            .unwrap()
            .unwrap(),
        legacy
    );
    assert_eq!(
        renamed.install_permissions_fixed(old_name, false).unwrap(),
        Some(true)
    );
    let mut renamed_record = aim_binder_host::parcel::Parcel::new();
    aim_service_aidl::write_byte_array(
        &mut renamed_record,
        Some(&renamed_results[0].package.to_cache_entry().unwrap().bytes),
    );
    renamed_record.write_string16(Some(old_name));
    renamed_record.write_string16(Some(&manifest_name));
    renamed_record.write_string16(Some(&retained.code_path));
    renamed_record.write_i32(retained.flags);
    renamed_record.write_i32(retained.private_flags);
    renamed_record.write_i32(retained.target_sdk_version);
    aim_service_aidl::write_byte_array(&mut renamed_record, Some(&legacy.bytes()));
    renamed_record.write_bool(true);
    fs::write(
        directory.join("apex-retained-rename.input"),
        renamed_record.data(),
    )
    .unwrap();
    let mut fresh_captured =
        aim_services::package::scan::SigningScan::new(&config, &Default::default(), 36).unwrap();
    capture_apex_legacy(&mut fresh_captured);
    fresh_captured
        .scan_initial_apex(&apex_apks, &config, &shared_inputs)
        .unwrap();
    let fresh_name = &shared_image.packages[0].parsed.package_name;
    let empty_group = aim_services::package::owner::legacy_permissions::Migration::default()
        .project(10000, &[10, 0, 11])
        .unwrap();
    assert_eq!(
        fresh_captured
            .legacy_permissions(fresh_name, false)
            .unwrap()
            .unwrap(),
        empty_group
    );
    assert_eq!(
        fresh_captured
            .shared_legacy_permissions("aim.fixture.apex")
            .unwrap()
            .unwrap(),
        empty_group
    );
    assert_eq!(
        fresh_captured
            .install_permissions_fixed(fresh_name, false)
            .unwrap(),
        Some(false)
    );
    let mut constructors = aim_binder_host::parcel::Parcel::new();
    constructors.write_i32(2);
    let mut transitions = aim_binder_host::parcel::Parcel::new();
    transitions.write_i32(3);
    for disabled in [false, true] {
        let mut replacement = if disabled {
            shared.clone()
        } else {
            shared_without_factory.clone()
        };
        replacement.settings.packages[0].pending_restore = true;
        capture_apex_legacy(&mut replacement);
        let calls = shared_compatibility_calls.get();
        let result = replacement
            .scan_initial_apex(&replacement_apks, &config, &replacement_inputs)
            .unwrap();
        assert_eq!(shared_compatibility_calls.get(), calls + 1);
        let setting = &replacement.settings.packages[0];
        assert_eq!(
            (setting.app_id, setting.shared_app_id()),
            (10001, Some(10001))
        );
        assert!(setting.pending_restore);
        let active_legacy = replacement
            .legacy_permissions(&setting.name, false)
            .unwrap()
            .unwrap();
        assert_eq!(
            active_legacy,
            aim_services::package::owner::legacy_permissions::Migration::default()
                .project(10001, &[10, 0, 11])
                .unwrap()
        );
        assert_eq!(
            replacement
                .install_permissions_fixed(&setting.name, false)
                .unwrap(),
            Some(false)
        );
        let group_legacy = replacement
            .shared_legacy_permissions("aim.fixture.replacement")
            .unwrap()
            .unwrap();
        assert_eq!(group_legacy, active_legacy);
        constructors.write_bool(disabled);
        aim_service_aidl::write_byte_array(&mut constructors, Some(&active_legacy.bytes()));
        constructors.write_bool(
            replacement
                .install_permissions_fixed(&setting.name, false)
                .unwrap()
                .unwrap(),
        );
        aim_service_aidl::write_byte_array(&mut constructors, Some(&group_legacy.bytes()));
        if disabled {
            let factory = replacement
                .legacy_permissions(&setting.name, true)
                .unwrap()
                .unwrap();
            assert_eq!(factory, apex_legacy.project(10000, &[10, 0, 11]).unwrap());
            constructors.write_bool(
                replacement
                    .install_permissions_fixed(&setting.name, true)
                    .unwrap()
                    .unwrap(),
            );
            aim_service_aidl::write_byte_array(&mut constructors, Some(&factory.bytes()));
            let old = replacement
                .shared_legacy_permissions("aim.fixture.apex")
                .unwrap()
                .unwrap();
            assert_eq!(old, factory);
            aim_service_aidl::write_byte_array(&mut constructors, Some(&old.bytes()));
        } else {
            assert_eq!(
                replacement
                    .shared_legacy_permissions("aim.fixture.apex")
                    .unwrap(),
                None
            );
        }

        assert_eq!(result[0].package.uid, -1);
        assert_eq!(replacement.identities.ids.get(10000).is_some(), disabled);
        if disabled {
            assert!(
                !replacement.identities.shared_users["aim.fixture.apex"]
                    .clone()
                    .remove_package(&setting.name)
            );
        }
        assert!(
            replacement.identities.shared_users["aim.fixture.replacement"]
                .clone()
                .remove_package(&setting.name)
        );
        assert!(replacement.seinfo(&setting.name).unwrap().is_some());
        aim_services::package::scan::SigningScan::new_after_apex(
            &config,
            &replacement.settings,
            36,
            &aim_services::package::scan::ApexImage {
                packages: if disabled {
                    vec![
                        shared_image.packages[0].clone(),
                        replacement_image.packages[0].clone(),
                    ]
                } else {
                    replacement_image.packages.clone()
                },
            },
        )
        .unwrap();
        transitions.write_bool(disabled);
        transitions.write_i32(setting.app_id);
        transitions.write_bool(replacement.identities.ids.get(10000).is_some());
    }
    fs::write(
        directory.join("apex-constructor-legacy.input"),
        constructors.data(),
    )
    .unwrap();
    let mut unshared_image = aim_services::package::scan::ApexImage {
        packages: replacement_image.packages.clone(),
    };
    unshared_image.packages[0].parsed.shared_user_id = None;
    let unshared_inputs = scan_inputs(&unshared_image);
    let mut unshared = shared_without_factory.clone();
    let result = unshared
        .scan_initial_apex(&replacement_apks, &config, &unshared_inputs)
        .unwrap();
    assert_eq!(unshared.settings.packages[0].app_id, -1);
    assert_eq!(unshared.settings.packages[0].shared_app_id(), None);
    assert_eq!(result[0].package.uid, -1);
    assert!(unshared.identities.ids.get(10000).is_none());
    let name = unshared.settings.packages[0].name.clone();
    let group_permissions = unshared
        .identities
        .shared_users
        .keys()
        .map(|name| (name.clone(), apex_legacy.clone()))
        .collect();
    unshared
        .capture_legacy_permissions(
            &[10, 0, 11],
            std::collections::BTreeMap::from([((name.clone(), false), apex_legacy.clone())]),
            group_permissions,
        )
        .unwrap();
    assert_eq!(
        unshared.legacy_permissions(&name, false).unwrap().unwrap(),
        detached_apex
    );
    aim_services::package::scan::SigningScan::new_after_apex(
        &config,
        &unshared.settings,
        36,
        &unshared_image,
    )
    .unwrap();
    transitions.write_bool(false);
    transitions.write_i32(-1);
    transitions.write_bool(false);
    let before_disabled = shared.settings.packages.clone();
    assert!(
        shared
            .scan_initial_apex(&replacement_apks, &config, &unshared_inputs)
            .is_err()
    );
    assert_eq!(shared.settings.packages, before_disabled);
    fs::write(
        directory.join("apex-group-change.input"),
        transitions.data(),
    )
    .unwrap();
    let mut inherited = shared_without_factory.clone();
    let name = inherited.settings.packages[0].name.clone();
    inherited
        .set_user_state(
            &name,
            0,
            aim_services::package::restrictions::UserState {
                hidden: true,
                installed: false,
                enabled_components: Some(vec!["enabled.fixture".into()]),
                disabled_components: Some(vec!["disabled.fixture".into()]),
                ..Default::default()
            },
        )
        .unwrap();
    inherited.disable_system_package(&name).unwrap();
    inherited
        .capture_legacy_permissions(
            &[10, 0, 11],
            std::collections::BTreeMap::from([
                ((name.clone(), false), Default::default()),
                ((name.clone(), true), apex_legacy.clone()),
            ]),
            inherited
                .identities
                .shared_users
                .keys()
                .map(|name| (name.clone(), Default::default()))
                .collect(),
        )
        .unwrap();
    let inheritance_users = [aim_services::package::scan::User {
        id: 0,
        pre_created: false,
        adb_install_disallowed: false,
    }];
    let mut inheritance_inputs = scan_inputs(&unshared_image);
    inheritance_inputs.users.users = Some(&inheritance_users);
    let mut partial_bits = inherited.clone();
    partial_bits
        .scan_initial_apex(&replacement_apks, &config, &inheritance_inputs)
        .unwrap();
    assert_eq!(
        partial_bits
            .install_permissions_fixed(&name, false)
            .unwrap(),
        Some(false)
    );
    assert_eq!(
        partial_bits.install_permissions_fixed(&name, true).unwrap(),
        None
    );
    inherited
        .capture_install_permissions_fixed(std::collections::BTreeMap::from([
            ((name.clone(), false), true),
            ((name.clone(), true), true),
        ]))
        .unwrap();
    let mut failed_inheritance = inherited.clone();
    let mut denied = scan_inputs(&unshared_image);
    denied.users.users = Some(&inheritance_users);
    denied.seinfo.compatibility = &reject_shared_compatibility;
    assert!(
        matches!(failed_inheritance.scan_initial_apex(&replacement_apks, &config, &denied),
        Err(aim_services::package::scan::SigningError::Rejected(ref error)) if error.phase == "seinfo")
    );
    assert_eq!(failed_inheritance, inherited);
    inherited
        .scan_initial_apex(&replacement_apks, &config, &inheritance_inputs)
        .unwrap();
    assert_eq!(
        inherited.legacy_permissions(&name, false).unwrap().unwrap(),
        detached_apex
    );
    assert_eq!(
        inherited.install_permissions_fixed(&name, false).unwrap(),
        Some(false)
    );
    assert_eq!(
        inherited.install_permissions_fixed(&name, true).unwrap(),
        Some(true)
    );
    assert_eq!(
        inherited.settings.packages[0].signatures,
        inherited.settings.disabled_system_packages[0].signatures
    );
    let user = &inherited.scanned_user_states(&name).unwrap()[&0];
    assert!(!user.hidden && user.installed);
    assert_eq!(
        user.enabled_components.as_deref(),
        Some(["enabled.fixture".into()].as_slice())
    );
    assert_eq!(
        user.disabled_components.as_deref(),
        Some(["disabled.fixture".into()].as_slice())
    );
    fs::write(
        directory.join("disabled-apex-legacy.input"),
        inherited
            .legacy_permissions(&name, false)
            .unwrap()
            .unwrap()
            .bytes(),
    )
    .unwrap();
    let factory_user = inherited.disabled_user_states(&name).unwrap()[&0].clone();
    let mut changed = user.clone();
    changed
        .enabled_components
        .as_mut()
        .unwrap()
        .push("late.fixture".into());
    inherited.set_user_state(&name, 0, changed).unwrap();
    assert_eq!(
        inherited.disabled_user_states(&name).unwrap()[&0],
        factory_user
    );
    let mut failed_replacement = shared_without_factory.clone();
    capture_apex_legacy(&mut failed_replacement);
    let prior_legacy = failed_replacement
        .legacy_permissions(&name, false)
        .unwrap()
        .unwrap();
    let previous_packages = failed_replacement.settings.packages.clone();
    replacement_inputs.seinfo.compatibility = &reject_shared_compatibility;
    assert!(
        matches!(failed_replacement.scan_initial_apex(&replacement_apks, &config, &replacement_inputs),
        Err(aim_services::package::scan::SigningError::Rejected(ref error))
            if error.phase == "seinfo" && error.message == "shared compatibility owner denied scan")
    );
    assert_eq!(failed_replacement.settings.packages, previous_packages);
    assert_eq!(
        failed_replacement.loaded_packages(),
        shared_without_factory.loaded_packages()
    );
    assert!(
        failed_replacement.identities.shared_users["aim.fixture.apex"]
            .clone()
            .remove_package(&previous_packages[0].name)
    );
    assert_eq!(
        failed_replacement.identities.shared_users["aim.fixture.replacement"].app_id,
        10001
    );
    assert_eq!(
        failed_replacement
            .legacy_permissions(&name, false)
            .unwrap()
            .unwrap(),
        prior_legacy
    );
    assert_eq!(
        failed_replacement
            .install_permissions_fixed(&name, false)
            .unwrap(),
        Some(true)
    );
    let allocated_legacy = failed_replacement
        .shared_legacy_permissions("aim.fixture.replacement")
        .unwrap()
        .unwrap();
    assert_eq!(
        allocated_legacy,
        aim_services::package::owner::legacy_permissions::Migration::default()
            .project(10001, &[10, 0, 11])
            .unwrap()
    );
    fs::write(
        directory.join("apex-rejected-group-legacy.input"),
        allocated_legacy.bytes(),
    )
    .unwrap();
    let original_notification = boot.client(2000).args([
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/package-parcels/oracle.dex:/system/framework/services.jar",
        "/system/bin", "com.android.server.pm.ApexNotifyOracle", "/data/local/tmp/package-parcels",
    ]).output().unwrap();
    assert_guest_success(&boot, &original_notification, "original APEX notification");
    assert_eq!(
        String::from_utf8(original_notification.stdout).unwrap(),
        format!("APEX_NOTIFY {}\n", results.len())
    );
    let adopted = fs::read(directory.join("apex-original-adoption.original")).unwrap();
    let mut adopted = aim_binder_host::parcel::Reader::new(&adopted, &[]);
    assert_eq!(adopted.read_i32().unwrap(), 10000);
    assert_eq!(adopted.read_i32().unwrap(), -1);
    assert_eq!(
        aim_service_aidl::read_byte_array(&mut adopted)
            .unwrap()
            .unwrap(),
        apex_legacy.project(10000, &[10, 0, 11]).unwrap().bytes()
    );
    assert_eq!(
        aim_service_aidl::read_byte_array(&mut adopted)
            .unwrap()
            .unwrap(),
        apex_legacy.project(-1, &[10, 0, 11]).unwrap().bytes()
    );
    assert_eq!(adopted.remaining(), 0);
    let shared_adopted =
        fs::read(directory.join("apex-shared-original-adoption.original")).unwrap();
    let mut shared_adopted = aim_binder_host::parcel::Reader::new(&shared_adopted, &[]);
    assert_eq!(shared_adopted.read_i32().unwrap(), 2);
    assert_eq!(shared_adopted.read_i32().unwrap(), 0);
    assert_eq!(shared_adopted.read_i32().unwrap(), 10000);
    assert_eq!(shared_adopted.read_i32().unwrap(), 10000);
    assert_eq!(shared_adopted.read_i32().unwrap(), 10000);
    assert_eq!(shared_adopted.remaining(), 0);
    let shared_process_members =
        aim_services::package::scan::OriginalSharedProcesses::read_original_record(
            &fs::read(directory.join("apex-shared-process-members.original")).unwrap(),
        )
        .unwrap();
    assert_eq!(
        shared_process_members.members,
        ["original.fixture", "original.fixture"]
    );
    assert_eq!(
        shared_process_members
            .member_settings
            .iter()
            .filter(|p| p.retained)
            .count(),
        1
    );
    assert_eq!(
        shared_process_members
            .member_settings
            .iter()
            .filter(|p| !p.retained)
            .count(),
        1
    );
    for member in &shared_process_members.member_settings {
        assert_eq!(member.app_id, 10000);
        assert_eq!(member.has_code, !member.retained);
        assert_eq!(
            member.path,
            if member.retained {
                "/system/apex/original.fixture.apex"
            } else {
                "/system/apex/incoming.fixture.apex"
            }
        );
    }

    let removed_process_members =
        aim_services::package::scan::OriginalSharedProcesses::read_original_record(
            &fs::read(directory.join("apex-shared-process-removed.original")).unwrap(),
        )
        .unwrap();
    assert_eq!(removed_process_members.member_settings.len(), 1);
    assert!(removed_process_members.member_settings[0].retained);
    assert!(!removed_process_members.member_settings[0].has_code);
    assert!(removed_process_members.records.is_empty());

    let mut inactive = aim_services::package::scan::ApexImage {
        packages: vec![apex_image.packages[0].clone()],
    };
    inactive.packages[0].info.active = false;
    let mut disabled =
        aim_services::package::scan::SigningScan::new(&config, &Default::default(), 36).unwrap();
    capture_apex_legacy(&mut disabled);
    disabled
        .scan_initial_apex(&apex_apks, &config, &scan_inputs(&inactive))
        .unwrap();
    let inactive_name = &inactive.packages[0].parsed.package_name;
    assert_eq!(
        disabled.legacy_permissions(inactive_name, false).unwrap(),
        disabled.legacy_permissions(inactive_name, true).unwrap()
    );
    assert_eq!(
        disabled
            .install_permissions_fixed(inactive_name, false)
            .unwrap(),
        Some(false)
    );
    assert_eq!(
        disabled
            .install_permissions_fixed(inactive_name, true)
            .unwrap(),
        Some(false)
    );

    assert_eq!(disabled.settings.disabled_system_packages.len(), 1);
    assert!(
        !disabled.settings.disabled_system_packages[0]
            .transient
            .updated_system_app
    );
    assert!(disabled.settings.packages[0].transient.updated_system_app);
    assert!(std::sync::Arc::ptr_eq(
        &disabled.loaded_packages()[&disabled.settings.packages[0].name],
        &disabled.disabled_loaded_packages()[&disabled.settings.packages[0].name]
    ));
    let mut rejected_source = aim_services::package::scan::ApexImage {
        packages: inactive.packages.clone(),
    };
    rejected_source.packages[0].info.module_name = None;
    let reject_domain = || Err("domain owner denied APEX registration".into());
    let before = disabled.clone();
    let mut rejected_inputs = scan_inputs(&rejected_source);
    rejected_inputs.new_domain_id = &reject_domain;
    assert!(
        matches!(disabled.scan_initial_apex(&apex_apks, &config, &rejected_inputs),
        Err(aim_services::package::scan::SigningError::Rejected(ref error)) if error.phase == "apex-domain")
    );
    let mut refreshed = before;
    refreshed.settings.disabled_system_packages[0]
        .transient
        .apex_module_name = None;
    assert!(
        disabled == refreshed,
        "rejected container changed owners beyond original disabled-module refresh"
    );
    let update_guest = "/data/apex/active/native-fixture.apex";
    let original_file = aim_paths::original_image().join(
        inactive.packages[0]
            .info
            .module_path
            .trim_start_matches('/'),
    );
    let updated_file = directory.join("updated-container.apex");
    std::os::unix::fs::symlink(original_file, &updated_file).unwrap();
    let original_root = aim_paths::original_image();
    let update_apks = aim_services::package::write::Apks {
        files: Box::new(move |path| {
            Some(if path == update_guest {
                updated_file.clone()
            } else {
                original_root.join(path.trim_start_matches('/'))
            })
        }),
        platform: aim_services::package::parse::Platform::load(
            &aim_paths::original_image(),
            Default::default(),
        )
        .unwrap(),
    };
    let mut update_info = inactive.packages[0].info.clone();
    update_info.module_path = update_guest.into();
    update_info.factory = false;
    update_info.active = true;
    let update_inventory = aim_services::package::bootstrap::ApexInventory {
        packages: Some(vec![update_info]),
        active: Vec::new(),
    };
    let update_image = aim_services::package::scan::ApexImage::load(
        &update_apks,
        &update_inventory,
        aim_services::package::parse::PARSE_IS_SYSTEM_DIR,
    )
    .unwrap();
    disabled
        .scan_initial_apex(&update_apks, &config, &scan_inputs(&update_image))
        .unwrap();
    let active = &disabled.settings.packages[0];
    assert_eq!(active.code_path, update_guest);
    assert_eq!(active.app_id, -1);
    assert!(active.transient.updated_system_app);
    assert!(!active.transient.apk_in_updated_apex);
    assert_ne!(
        active.code_path,
        disabled.settings.disabled_system_packages[0].code_path
    );
    assert_eq!(
        disabled.disabled_loaded_packages()[&active.name]
            .package
            .path,
        Some(inactive.packages[0].info.module_path.clone())
    );
    assert_eq!(
        disabled.loaded_packages()[&active.name].package.path,
        Some(update_guest.into())
    );
    assert!(disabled.settings.key_sets.public_keys.is_empty());
    let mut updated_apex = apex_inventory.clone();
    let mut info = updated_apex.packages.as_ref().unwrap()[0].clone();
    info.factory = false;
    updated_apex.packages = Some(vec![info]);
    let updated = aim_services::package::scan::ApexImage::load(
        &apex_apks,
        &updated_apex,
        aim_services::package::parse::PARSE_IS_SYSTEM_DIR,
    )
    .unwrap();
    assert_eq!(updated.packages[0].scan_parse_flags, 0);
    assert!(
        updated.packages[0].parsed
            == apex_image
                .packages
                .iter()
                .find(|p| p.info.module_path == updated.packages[0].info.module_path)
                .unwrap()
                .parsed
    );
    let mut missing_apex = updated_apex.clone();
    missing_apex.packages.as_mut().unwrap()[0].module_path =
        "/system/apex/fixture-missing.apex".into();
    assert_eq!(
        aim_services::package::scan::ApexImage::load(
            &apex_apks,
            &missing_apex,
            aim_services::package::parse::PARSE_IS_SYSTEM_DIR
        )
        .unwrap_err()
        .phase,
        "apex-parse"
    );
    let manifest = aim_apps::apk::Apk::open(
        &(apex_apks.files)(&apex_image.packages[0].info.module_path).unwrap(),
    )
    .unwrap()
    .file("AndroidManifest.xml")
    .unwrap();
    let unsigned_archive = directory.join("unsigned.apex");
    fs::write(
        &unsigned_archive,
        common::zip::resource_apk(&manifest, None),
    )
    .unwrap();
    let mut unsigned_inventory = updated_apex.clone();
    unsigned_inventory.packages.as_mut().unwrap()[0].module_path =
        "/data/local/tmp/package-parcels/unsigned.apex".into();
    let unsigned_apks = aim_services::package::write::Apks {
        files: Box::new(move |_| Some(unsigned_archive.clone())),
        platform: aim_services::package::parse::Platform::load(
            &aim_paths::original_image(),
            Default::default(),
        )
        .unwrap(),
    };
    assert_eq!(
        aim_services::package::scan::ApexImage::load(
            &unsigned_apks,
            &unsigned_inventory,
            aim_services::package::parse::PARSE_IS_SYSTEM_DIR
        )
        .unwrap_err()
        .phase,
        "apex-signatures"
    );
    let mut null_apex = apex_inventory.clone();
    null_apex.packages = None;
    assert!(
        aim_services::package::scan::ApexImage::load(
            &apex_apks,
            &null_apex,
            aim_services::package::parse::PARSE_IS_SYSTEM_DIR
        )
        .unwrap()
        .packages
        .is_empty()
    );
    let library_feed = fs::read(directory.join("library-feed-original.parcel")).unwrap();
    let mut reader = aim_binder_host::parcel::Reader::new(&library_feed, &[]);
    assert_eq!(reader.read_i32().unwrap(), 7);
    for index in 0..7 {
        let bytes = aim_service_aidl::read_byte_array(&mut reader)
            .unwrap()
            .unwrap();
        let library = aim_services::package::model::SharedLibrary::read_parcel(&bytes).unwrap();
        assert_eq!(
            library.write_parcel(),
            bytes,
            "original library owner {index}"
        );
        if index == 0 {
            assert!(!library.dependents_initialized && !library.dependencies_initialized);
            assert_eq!(library.optional_dependents, None);
            assert_eq!(library.cert_digests, None);
        } else if index == 1 {
            assert!(library.dependents_initialized && library.dependencies_initialized);
            assert_eq!(
                library.code_paths,
                Some(vec![Some("/explicit/code.jar".into())])
            );
        } else if index == 4 {
            assert_eq!(
                library.dependents,
                vec![Some(("dependent.consumer".into(), 41))]
            );
            assert_eq!(
                library.optional_dependents,
                Some(vec![None, Some(("consumer".into(), i64::MAX))])
            );
            assert_eq!(
                library.cert_digests,
                Some(vec![None, Some("digest".into())])
            );
            assert_eq!(
                library.dependencies[0].as_ref().unwrap().cert_digests,
                Some(vec![Some("nested.digest".into())])
            );
        }
        if index == 5 {
            assert!(library.declaring_absent);
            assert_eq!(
                library.cert_digests,
                Some(vec![Some("sdk.certificate".into())])
            );
        }
        if index == 6 {
            assert_eq!(
                library.code_paths,
                Some(vec![None, Some("/system/nullable.apk".into())])
            );
            assert_eq!(
                library.dependents,
                vec![None, Some(("nullable.consumer".into(), 59))]
            );
            assert_eq!(library.dependencies[0], None);
            assert_eq!(
                library.dependencies[1].as_ref().unwrap().name.as_deref(),
                Some("nullable.nested")
            );
        }
        for end in 0..bytes.len() {
            assert!(
                aim_services::package::model::SharedLibrary::read_parcel(&bytes[..end]).is_err(),
                "truncated library {index} at {end}"
            );
        }
        let mut trailing = bytes.clone();
        trailing.extend_from_slice(&[0; 4]);
        assert!(aim_services::package::model::SharedLibrary::read_parcel(&trailing).is_err());
    }
    assert_eq!(reader.remaining(), 0);
    for index in 0..16 {
        let original =
            fs::read(directory.join(format!("legacy-migration-{index}.original"))).unwrap();
        let native = fs::read(directory.join(format!("legacy-migration-{index}.input"))).unwrap();
        assert_eq!(original, native, "migration case {index}");
        assert_eq!(
            aim_services::package::owner::legacy_permissions::State::read(
                &original,
                10042,
                &[10, 0, 11]
            )
            .unwrap()
            .bytes(),
            original
        );
    }
    let original_permissions = fs::read(directory.join("legacy-permissions.original")).unwrap();
    assert_eq!(
        aim_services::package::owner::legacy_permissions::State::read(
            &original_permissions,
            10042,
            &[10, 0, 11]
        )
        .unwrap(),
        permissions
    );
    assert_eq!(original_permissions, permissions.bytes());
    for factory in [false, true] {
        let bytes = fs::read(directory.join(format!("scoped-{factory}.original-runtime"))).unwrap();
        let exported =
            aim_services::package::scan::OriginalRuntime::read_original_record(&bytes).unwrap();
        assert_eq!(exported.name, "aim.unloaded.fixture");
        assert!(!exported.has_code);
        assert_eq!(
            exported.state.usage[if factory { 0 } else { 2 }],
            if factory { 17 } else { 71 }
        );
        assert_eq!(
            exported.state.seinfo.as_deref(),
            if factory { None } else { Some("active-label") }
        );
        assert_eq!(
            exported.state.library_files,
            if factory {
                vec![None, Some("/system/null-slot.jar".into())]
            } else {
                vec![]
            }
        );
    }
    for (phase, factory, users, aliases) in [
        ("active", false, vec![0], vec![]),
        ("factory", true, vec![0, 10], vec![0]),
        ("independent", true, vec![0], vec![]),
    ] {
        let scope = aim_services::package::scan::OriginalUserScope::read_original_record(
            &fs::read(directory.join(format!("scope-{phase}.original"))).unwrap(),
        )
        .unwrap();
        assert_eq!(scope.name, "scope.fixture");
        assert_eq!(scope.app_id, 10123);
        assert_eq!(scope.version, 7);
        assert_eq!(scope.factory, factory);
        assert_eq!(scope.users.into_iter().collect::<Vec<_>>(), users);
        assert_eq!(
            scope.active_aliases.into_iter().collect::<Vec<_>>(),
            aliases
        );
    }
    let mut original_inputs = aim_services::package::model::State::default();
    for loaded in runtime_snapshot.owner().loaded_packages().values() {
        let bytes = fs::read(directory.join(format!(
            "scan-{}.native.original-runtime",
            loaded.package.uid
        )))
        .unwrap();
        let exported =
            aim_services::package::scan::OriginalRuntime::read_original_record(&bytes).unwrap();
        assert_eq!(
            &exported.state,
            runtime_snapshot
                .owner()
                .replica_runtime(&exported.name, false)
                .unwrap()
                .unwrap()
        );
        original_inputs
            .runtime_inputs
            .insert((exported.name.clone(), false), exported);
    }
    let mut imported = snapshot.owner().clone();
    imported.capture_original_runtime(&original_inputs).unwrap();
    aim_services::package::scan_snapshot::Store::new(imported, snapshot.usage().clone()).unwrap();
    let full_original = aim_services::package::settings::Settings::parse(
        &aim_android_xml::read(&fs::read(directory.join("settings-inventory-original")).unwrap())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        full_original, writer_expected,
        "complete original settings inventory differs"
    );
    for (name, package, entry) in expected {
        if name.starts_with("scan-") {
            let original_settings = aim_services::package::settings::Settings::parse(
                &aim_android_xml::read(
                    &fs::read(directory.join(format!("{name}.settings-original"))).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
            assert_eq!(original_settings.packages.len(), 1);
            let native_setting = writer_expected
                .packages
                .iter()
                .find(|p| p.name == package)
                .unwrap();
            assert_eq!(
                &original_settings.packages[0], native_setting,
                "{name}: native setting persistence differs from original Settings writer"
            );
            assert_eq!(original_settings.shared_users, writer_expected.shared_users);
            for (suffix, apex) in [("apex-package-policy", true), ("apk-in-apex-policy", false)] {
                let policy = AndroidPackage::read_cache_entry(
                    &fs::read(directory.join(format!("{name}.{suffix}"))).unwrap(),
                )
                .unwrap();
                assert_eq!(
                    policy.is2(aim_services::package::pkg::booleans2::APEX),
                    apex
                );
                assert!(policy.is(aim_services::package::pkg::booleans::SYSTEM));
                assert!(policy.is(aim_services::package::pkg::booleans::VENDOR));
            }

            assert_eq!(
                fs::read(directory.join(format!("{name}.mime-feed-original"))).unwrap(),
                fs::read(directory.join(format!("{name}.mime-feed-native"))).unwrap()
            );
            assert_eq!(
                fs::read(directory.join(format!("{name}.mime-original"))).unwrap(),
                fs::read(directory.join(format!("{name}.mime-native"))).unwrap()
            );
            assert_eq!(
                fs::read(directory.join(format!("{name}.keysets-original"))).unwrap(),
                fs::read(directory.join(format!("{name}.keysets-native"))).unwrap()
            );

            assert_eq!(
                fs::read(directory.join(format!("{name}.loading-original"))).unwrap(),
                fs::read(directory.join(format!("{name}.loading-native"))).unwrap()
            );

            assert_eq!(
                fs::read(directory.join(format!("{name}.setting-runtime.original"))).unwrap(),
                setting_runtime_expected()
            );

            let xml = fs::read(directory.join(format!("{name}.null-icon.xml"))).unwrap();
            let restored = aim_services::package::restrictions::Restrictions::parse(
                &aim_android_xml::read(&xml).unwrap(),
            )
            .unwrap();
            let archive = restored.packages[0].1.archive_state.as_ref().unwrap();
            assert_eq!(archive.archive_time, 123);
            assert_eq!(archive.activities.len(), 1);
            assert_eq!(archive.activities[0].title, "icon");
            assert_eq!(
                archive.activities[0].icon_path.as_deref(),
                Some("/data/icon")
            );
            assert_eq!(archive.activities[0].monochrome_icon_path, None);

            let root = aim_android_xml::read(
                &fs::read(directory.join(format!("{name}.null-params.xml"))).unwrap(),
            )
            .unwrap();
            let restored = aim_services::package::restrictions::Restrictions::parse(&root).unwrap();
            let state = &restored.packages[0].1;
            assert_eq!(
                state.suspensions.as_ref().unwrap()[0].params,
                Some(aim_services::package::restrictions::SuspendParams::default())
            );
            assert_eq!(state.is_quarantined(10, true), Ok(false));
            assert_eq!(state.resolved_suspensions(10, true)[0].0, 0);
        }
        let original = fs::read(directory.join(format!("{name}.original"))).unwrap();
        let mut native = AndroidPackage::read_cache_entry(&entry.bytes).unwrap();
        let mut original = AndroidPackage::read_cache_entry(&original).unwrap();
        normalize_maps(&mut native);
        normalize_maps(&mut original);
        assert!(
            native == original,
            "{name} {package}: original PackageImpl changed a decoded field"
        );
    }
}

// Map iteration order is not part of the Parcel map's contract: readHashMap
// constructs a HashMap even when the producer used an ArrayMap. Compare all
// entries and their values while retaining order for lists and arrays.
fn normalize_maps(pkg: &mut AndroidPackage) {
    use aim_services::package::pkg::{Component, Property};
    let props = |values: &mut Option<Vec<(String, Property)>>| {
        if let Some(values) = values {
            values.sort_by(|a, b| a.0.cmp(&b.0));
        }
    };
    let component = |value: &mut Component| {
        props(&mut value.properties);
    };
    props(&mut pkg.properties);
    if let Some(values) = &mut pkg.overlayables {
        values.sort_by(|a, b| a.0.cmp(&b.0));
    }
    if let Some(values) = &mut pkg.key_set_mapping {
        values.sort_by(|a, b| a.0.cmp(&b.0));
    }
    if let Some(values) = &mut pkg.processes {
        values.sort_by(|a, b| a.map_key.cmp(&b.map_key));
        for value in values {
            value
                .app_class_names_by_package
                .sort_by(|a, b| a.0.cmp(&b.0));
        }
    }
    for value in pkg.activities.iter_mut().chain(&mut pkg.receivers) {
        component(&mut value.main.component);
    }
    for value in &mut pkg.services {
        component(&mut value.main.component);
    }
    for value in &mut pkg.providers {
        component(&mut value.main.component);
    }
    for value in &mut pkg.permissions {
        component(&mut value.component);
        if let Some(group) = &mut value.parsed_permission_group {
            component(&mut group.component);
        }
    }
    for value in &mut pkg.permission_groups {
        component(&mut value.component);
    }
    for value in &mut pkg.instrumentations {
        component(&mut value.component);
    }
}

// Scan real original framework and rotated GSF code, without saved settings.
fn native_scan_objects(
    dir: &std::path::Path,
) -> std::sync::Arc<aim_services::package::scan_snapshot::Snapshot> {
    use aim_services::package::{
        parse::Platform,
        scan::{
            AbiPolicy, FirstBootSystemInputs, Image, LibraryCompatibility,
            NativeLibraryInstallPolicy, ScanClock, SystemImageScan, UserPolicy,
        },
        system_config::SystemConfig,
        write::Apks,
    };
    let original = aim_paths::original_image();
    let root = dir.join("scan-inputs");
    let framework = root.join("system/framework");
    let app = root.join("product/priv-app/GSF");
    fs::create_dir_all(&framework).unwrap();
    fs::create_dir_all(&app).unwrap();
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
    let apks = Apks {
        files: Box::new(move |path| Some(root.join(path.trim_start_matches('/')))),
        platform: Platform::load(&original, Default::default()).unwrap(),
    };
    let mut config = SystemConfig::default();
    config.library_order.push("aim.fixture".into());
    // Controlled declaration/path inputs for the original object transport oracle.
    config.libraries.insert(
        "aim.fixture".into(),
        aim_services::package::system_config::Library {
            name: "aim.fixture".into(),
            filename: format!("/system/framework/{}.jar", "x".repeat(150_000)),
            native: false,
            dependencies: vec![],
            on_bootclasspath_since: None,
            on_bootclasspath_before: None,
            can_be_safely_ignored: false,
        },
    );
    let compatibility = LibraryCompatibility::new(&config, &|_| None, true).unwrap();
    let abi = AbiPolicy {
        all: vec!["arm64-v8a".into()],
        bit32: vec![],
        bit64: vec!["arm64-v8a".into()],
        native32: vec![],
        native64: vec!["arm64-v8a".into()],
        force_multi_arch_match: false,
    };
    let next = std::cell::Cell::new(1_u8);
    let domain = || {
        let id = next.get();
        next.set(id + 1);
        Ok([id; 16])
    };
    let mut image = Image::load(&apks, &[]).unwrap();
    for code in &mut image.packages {
        code.parsed.uses_libraries.push("aim.fixture".into());
    }
    let collected: std::collections::BTreeMap<_, _> = image
        .packages
        .iter()
        .map(|code| (code.parsed.package_name.clone(), code.signing.clone()))
        .collect();
    let policy = aim_services::package::owner::seinfo::Policy::load(&original).unwrap();
    let scan = SystemImageScan::first_boot(
        || Ok(image),
        &apks,
        &config,
        FirstBootSystemInputs {
            // Controlled target for the original envelope/replica oracle.
            seinfo: aim_services::package::scan::SeInfoScan {
                policy: &policy,
                compatibility: &|_: &aim_services::package::pkg::AndroidPackage| Ok(36),
            },
            apex_image: &Default::default(),
            notify_apex_scan: &|results| {
                assert!(results.is_empty());
                Ok(())
            },
            first_api_level: 36,
            vendor_sdk: 36,
            shared_uid_migration: aim_services::package::scan::SharedUidMigration::NewInstallOnly,
            abi_policy: &abi,
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
            new_domain_id: &domain,
        },
    )
    .unwrap();
    assert!(scan.rejected.is_empty());
    let objects: Vec<_> = scan
        .packages
        .iter()
        .map(|p| {
            let record = &p.candidate.record;
            assert_eq!(record.parsed.uid, record.settings.app_id);
            assert!(
                record.parsed.signing_details == Some(record.signing.parcel_details().unwrap())
            );
            let loaded = &scan.owner.loaded_packages()[&record.settings.name];
            assert_eq!(
                &loaded.collected_signing,
                &collected[&record.parsed.package_name]
            );
            std::sync::Arc::clone(loaded)
        })
        .collect();
    assert_eq!(
        objects.iter().map(|p| p.package.uid).collect::<Vec<_>>(),
        [1000, 10000]
    );
    let mut usage = aim_services::package::owner::usage::Usage::new(
        scan.owner.settings.packages.iter().map(|p| p.name.as_str()),
    );
    for name in ["android", "com.google.android.gsf"] {
        for (reason, time) in [-1, 17, 29, 0, 0, 0, 0, 55].into_iter().enumerate() {
            usage.notify(name, reason as i32, time);
        }
    }
    let mut owner = scan.owner;
    for name in ["android", "com.google.android.gsf"] {
        owner.set_user_runtime(name, 0, runtime_capture()).unwrap();
        let setting = owner
            .settings
            .packages
            .iter_mut()
            .find(|p| p.name == name)
            .unwrap();
        setting.set_loading_progress(0.5);
        setting.loading_completed_time = 17;
        setting.add_old_path(Some("/data/retained"));
        setting.add_old_path(None);
        setting.add_old_path(Some(&"x".repeat(100_000)));
        setting.restrict_update_hash = Some(vec![1, 2, 3]);
        if name == "com.google.android.gsf" {
            setting.uses_sdk_libraries = vec![aim_services::package::settings::UsesSdkLibrary {
                name: "sdk".into(),
                version_major: i64::MAX,
                optional: false,
            }];
            setting.uses_static_libraries = vec![("static".into(), 17)];
            setting.add_mime_types(
                "BB".into(),
                ["text/plain".into(), "image/png".into(), "text/plain".into()],
            );
            setting.add_mime_types("Aa".into(), ["".into()]);
            setting.add_nullable_mime_types(
                Some("nullable".into()),
                [
                    Some("BB".into()),
                    None,
                    Some(String::new()),
                    Some("Aa".into()),
                    None,
                ],
            );
            setting.add_nullable_mime_types(None, [None, Some(String::new())]);
        }
        if name == "com.google.android.gsf" {
            let keys = &mut setting.key_set_data;
            let id = keys.proper_signing_key_set;
            assert!(id > 0, "native scan did not register its signing keyset");
            keys.upgrade_key_sets.clear();
            keys.defined_key_sets.clear();
            keys.add_upgrade_key_set(id);
            keys.add_upgrade_key_set(id);
            for alias in [Some("BB"), Some("Aa"), None, Some(""), Some("BB")] {
                keys.add_defined_key_set(id, alias.map(str::to_owned));
            }
        }

        if name == "com.google.android.gsf" {
            setting.install_source = aim_services::package::settings::InstallSource {
                initiating_package: Some(name.into()),
                originating_package: Some("origin".into()),
                installer: Some("".into()),
                installer_uid: 123,
                update_owner: Some("owner".into()),
                installer_attribution_tag: Some("tag".into()),
                package_source: 3,
                is_orphaned: true,
                initiating_package_uninstalled: true,
                initiating_package_signatures: setting.signatures.clone(),
            };
        }

        let xml = b"<package-restrictions><pkg name='fixture' ceDataInode='17' deDataInode='19' inst='false' stopped='true' nl='true' hidden='true' distraction_flags='3' instant-app='true' virtual-preload='true' enabled='3' enabledCaller='caller' install-reason='4' uninstall-reason='5' harmful-app-warning='warning' splash-screen-theme='theme' first-install-time='2b' min-aspect-ratio='2'><enabled-components><item name='fixture.Enabled'/></enabled-components><disabled-components><item name='fixture.Disabled'/></disabled-components><suspend-params suspending-package='android' suspending-user='0' quarantined='true'><dialog-info title='title' dialogMessage='message' buttonText='button' buttonAction='1'/><app-extras><int-array name='values' num='2'><item value='7'/><item value='9'/></int-array></app-extras></suspend-params><suspend-params suspending-package='android' suspending-user='10' quarantined='false'/><archive-state installer-title='installer' archive-time='55'><archive-activity-info activity-title='archived' original-component-name='fixture/.Archive' icon-path='/data/icon' monochrome-icon-path='/data/mono'/></archive-state></pkg></package-restrictions>";
        let mut state = aim_services::package::restrictions::Restrictions::parse(
            &aim_android_xml::read(xml).unwrap(),
        )
        .unwrap()
        .packages
        .remove(0)
        .1;
        state.runtime = runtime_capture();
        owner.set_user_state(name, 10, state).unwrap();
        let mut empty = aim_services::package::restrictions::UserState::initialized();
        empty.suspensions = Some(vec![]);
        empty
            .runtime
            .set_library_overlay_paths("absent".into(), None);
        owner.set_user_state(name, 11, empty).unwrap();
        use aim_services::package::restrictions::{
            ArchiveActivity, ArchiveState, SuspendParams, UserState,
        };
        let archive = ArchiveState {
            installer_title: "runtime installer".into(),
            archive_time: 123,
            activities: vec![
                ArchiveActivity {
                    title: "no icon".into(),
                    original_component_name: "fixture/fixture.NoIcon".into(),
                    icon_path: None,
                    monochrome_icon_path: Some("/data/mono".into()),
                },
                ArchiveActivity {
                    title: "icon".into(),
                    original_component_name: "fixture/fixture.Icon".into(),
                    icon_path: Some("/data/icon".into()),
                    monochrome_icon_path: None,
                },
            ],
        };
        owner
            .set_user_state(
                name,
                14,
                UserState {
                    archive_state: Some(archive),
                    ..UserState::default()
                },
            )
            .unwrap();
        for (user, null_name, positive_name) in [(12, "android", "B"), (13, "B", "android")] {
            let mut state = UserState::default();
            for package in ["B", "android"] {
                let params = (package == positive_name).then(|| SuspendParams {
                    quarantined: true,
                    ..Default::default()
                });
                state.put_suspension(user, false, 0, package.into(), params);
            }
            assert_eq!(
                state
                    .suspensions
                    .as_ref()
                    .unwrap()
                    .iter()
                    .find(|s| s.package == null_name)
                    .unwrap()
                    .params,
                None
            );
            owner.set_user_state(name, user, state).unwrap();
        }
    }
    owner
        .assign_seinfo_at_boot(&policy, &mut |_| Ok(36))
        .unwrap();
    use aim_services::package::owner::legacy_permissions::{Migration, Permission};
    let legacy_packages = owner
        .settings
        .packages
        .iter()
        .map(|setting| {
            let mut migration = Migration::default();
            if setting.name == "com.google.android.gsf" {
                migration
                    .put(
                        10,
                        Permission {
                            name: None,
                            runtime: false,
                            granted: true,
                            flags: -1,
                        },
                    )
                    .unwrap();
                migration
                    .put(
                        10,
                        Permission {
                            name: Some("BB".into()),
                            runtime: true,
                            granted: false,
                            flags: 17,
                        },
                    )
                    .unwrap();
                migration.set_missing(10, true).unwrap();
            }
            ((setting.name.clone(), false), migration)
        })
        .collect();
    let legacy_groups = owner
        .identities
        .shared_users
        .keys()
        .map(|name| (name.clone(), Migration::default()))
        .collect();
    owner
        .capture_legacy_permissions(&[10, 0, 11], legacy_packages, legacy_groups)
        .unwrap();
    let fixed = owner
        .settings
        .packages
        .iter()
        .map(|setting| {
            (
                (setting.name.clone(), false),
                setting.name == "com.google.android.gsf",
            )
        })
        .collect();
    owner.capture_install_permissions_fixed(fixed).unwrap();
    owner
        .complete_library_dependencies(&|_, _| {
            Ok(aim_services::package::libraries::Policy::pinned(false))
        })
        .unwrap();
    aim_services::package::scan_snapshot::Store::new(owner, usage)
        .unwrap()
        .capture()
}

fn distinct_shared_id_objects(directory: &std::path::Path) {
    use aim_service_aidl::WriteParcelable;
    use aim_services::package::{
        owner::usage::Usage,
        scan::SigningScan,
        scan_snapshot::{Store, endpoint::PackageSigningState},
        settings,
        system_config::SystemConfig,
    };
    let name = "aim.shared.identity.fixture";
    let settings = settings::Settings {
        packages: vec![settings::Package {
            name: name.into(),
            code_path: "/data/app/shared-identity".into(),
            app_id: 10123,
            shared_user: true,
            shared_user_app_id: Some(1000),
            ..Default::default()
        }],
        shared_users: vec![settings::SharedUser {
            name: "android.uid.system".into(),
            app_id: 1000,
            ..Default::default()
        }],
        ..Default::default()
    };
    let owner = SigningScan::new(&SystemConfig::default(), &settings, 36).unwrap();
    assert_eq!(owner.identities.ids.get(10123), None);
    let snapshot = Store::new(owner, Usage::new([name])).unwrap().capture();
    std::fs::write(
        directory.join("shared-id.setting"),
        aim_services::package::scan_snapshot::setting_record::captured(&snapshot, name, false)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    let state = PackageSigningState::captured(&snapshot, name, false)
        .unwrap()
        .unwrap();
    let mut out = aim_binder_host::parcel::Parcel::new();
    state.write_to(&mut out);
    std::fs::write(directory.join("shared-id.signing"), out.data()).unwrap();
}

fn scoped_runtime_objects(directory: &std::path::Path) {
    use aim_service_aidl::WriteParcelable;
    use aim_services::package::{
        owner::{legacy_permissions::Migration, usage::Usage},
        scan::{CapturedUsers, ReplicaRuntime, SigningScan},
        scan_snapshot::{Store, endpoint::PackageSigningState},
        settings,
        system_config::SystemConfig,
    };
    use std::collections::BTreeMap;
    let name = "aim.unloaded.fixture";
    let active = settings::Package {
        name: name.into(),
        code_path: "/data/app/unloaded".into(),
        app_id: 10123,
        domain_set_id: Some("00000000-0000-0000-0000-000000000001".into()),
        leaving_shared_user: Some(false),
        target_sdk_version: 28,
        flags: 1,
        version_code: 19,
        ..Default::default()
    };
    let factory = settings::Package {
        code_path: "/system/app/unloaded".into(),
        version_code: 7,
        target_sdk_version: 24,
        domain_set_id: None,
        ..active.clone()
    };
    let mut settings = settings::Settings::default();
    settings.packages.push(active);
    settings.disabled_system_packages.push(factory);
    let mut owner = SigningScan::new(&SystemConfig::default(), &settings, 29).unwrap();
    let mut states = BTreeMap::new();
    for factory in [false, true] {
        let mut users = CapturedUsers::default();
        let mut user = aim_services::package::restrictions::UserState::default();
        user.enabled = if factory { 3 } else { 2 };
        users.states.insert(if factory { 10 } else { 0 }, user);
        states.insert((name.into(), factory), users);
    }
    owner.capture_user_states(states).unwrap();
    owner
        .capture_legacy_permissions(
            &[0, 10],
            BTreeMap::from([
                ((name.into(), false), Migration::default()),
                ((name.into(), true), Migration::default()),
            ]),
            owner
                .identities
                .shared_users
                .keys()
                .map(|name| (name.clone(), Migration::default()))
                .collect(),
        )
        .unwrap();
    owner
        .capture_install_permissions_fixed(BTreeMap::from([
            ((name.into(), false), true),
            ((name.into(), true), false),
        ]))
        .unwrap();
    let mut usage = Usage::new([name]);
    usage.notify(name, 2, 71);
    let mut runtimes = BTreeMap::new();
    for factory in [false, true] {
        let mut times = if factory {
            [0; 8]
        } else {
            *usage.times(name).unwrap()
        };
        if factory {
            times[0] = 17;
        }
        runtimes.insert(
            (name.into(), factory),
            ReplicaRuntime {
                usage: times,
                seinfo: if factory {
                    None
                } else {
                    Some("active-label".into())
                },
                override_seinfo: if factory {
                    Some("".into())
                } else {
                    Some("active-override".into())
                },
                library_files: if factory {
                    vec![None, Some("/system/null-slot.jar".into())]
                } else {
                    vec![]
                },
                libraries: vec![],
            },
        );
    }
    owner.capture_replica_runtime(runtimes).unwrap();
    let snapshot = Store::new(owner, usage).unwrap().capture();
    for factory in [false, true] {
        let prefix = format!("scoped-{factory}");
        fs::write(
            directory.join(format!("{prefix}.setting")),
            aim_services::package::scan_snapshot::setting_record::captured(
                &snapshot, name, factory,
            )
            .unwrap()
            .unwrap(),
        )
        .unwrap();
        fs::write(
            directory.join(format!("{prefix}.runtime")),
            aim_services::package::scan_snapshot::runtime_record::captured(
                &snapshot, name, factory,
            )
            .unwrap()
            .unwrap(),
        )
        .unwrap();
        let mut signing = aim_binder_host::parcel::Parcel::new();
        PackageSigningState::captured(&snapshot, name, factory)
            .unwrap()
            .unwrap()
            .write_to(&mut signing);
        fs::write(directory.join(format!("{prefix}.signing")), signing.data()).unwrap();
        let mut transient = aim_binder_host::parcel::Parcel::new();
        aim_services::package::scan_snapshot::endpoint::PackageTransientState::captured(
            &snapshot, name, factory,
        )
        .unwrap()
        .write_to(&mut transient);
        fs::write(
            directory.join(format!("{prefix}.transient")),
            transient.data(),
        )
        .unwrap();
        let user = if factory { 10 } else { 0 };
        fs::write(
            directory.join(format!("{prefix}.user")),
            aim_services::package::scan_snapshot::user_record::captured(
                &snapshot, name, factory, user,
            )
            .unwrap()
            .unwrap(),
        )
        .unwrap();
    }
}

fn runtime_fixture_snapshot(
    snapshot: &aim_services::package::scan_snapshot::Snapshot,
) -> std::sync::Arc<aim_services::package::scan_snapshot::Snapshot> {
    let mut owner = snapshot.owner().clone();
    let usage = snapshot.usage().clone();
    owner
        .complete_runtime_at_boot(&usage, std::collections::BTreeMap::new())
        .unwrap();
    aim_services::package::scan_snapshot::Store::new(owner, usage)
        .unwrap()
        .capture()
}

fn runtime_expected() -> Vec<u8> {
    use aim_services::package::{
        model::OverlayPaths,
        owner::user_runtime::{Component, LabelIcon, State},
    };
    fn text(out: &mut Vec<u8>, value: Option<&str>) {
        out.extend_from_slice(&value.map_or(-1, |s| s.len() as i32).to_be_bytes());
        if let Some(value) = value {
            out.extend_from_slice(value.as_bytes());
        }
    }
    fn paths(out: &mut Vec<u8>, value: Option<&OverlayPaths>) {
        out.push(u8::from(value.is_some()));
        if let Some(value) = value {
            for strings in [&value.resource_dirs, &value.overlay_paths] {
                out.extend_from_slice(&(strings.len() as i32).to_be_bytes());
                for s in strings {
                    text(out, Some(s));
                }
            }
        }
    }
    fn record(out: &mut Vec<u8>, state: &State, changed: bool) {
        out.push(u8::from(changed));
        paths(out, state.overlays());
        paths(out, state.all_overlay_paths().as_ref());
        let libraries = state.libraries().unwrap_or(&[]);
        out.extend_from_slice(&(libraries.len() as i32).to_be_bytes());
        for (name, value) in libraries {
            text(out, Some(name));
            paths(out, Some(value));
        }
        let value = state
            .overrides()
            .and_then(|entries| entries.iter().find(|(c, _)| c.class == "Activity"))
            .map(|(_, v)| v);
        out.push(u8::from(value.is_some()));
        if let Some(value) = value {
            text(out, value.label.as_deref());
            out.push(u8::from(value.icon.is_some()));
            if let Some(icon) = value.icon {
                out.extend_from_slice(&icon.to_be_bytes());
            }
        }
    }
    fn apk(names: &[&str]) -> OverlayPaths {
        OverlayPaths {
            resource_dirs: names.iter().map(|s| s.to_string()).collect(),
            overlay_paths: names.iter().map(|s| s.to_string()).collect(),
        }
    }
    let mut state = State::default();
    let mut out = Vec::new();
    macro_rules! step {
        ($change:expr) => {{
            let changed = $change;
            record(&mut out, &state, changed);
        }};
    }
    record(&mut out, &state, false);
    step!(state.set_overlay_paths(Some(OverlayPaths::default())));
    step!(state.set_library_overlay_paths("missing".into(), None));
    step!(state.set_overlay_paths(Some(OverlayPaths {
        resource_dirs: vec!["base-apk".into()],
        overlay_paths: vec!["base".into(), "base".into(), "base-apk".into()]
    })));
    step!(state.set_library_overlay_paths("Aa".into(), Some(apk(&["one", "base-apk"]))));
    step!(state.set_library_overlay_paths("B".into(), Some(apk(&["two"]))));
    step!(state.set_library_overlay_paths("BB".into(), Some(apk(&["three"]))));
    step!(state.set_library_overlay_paths("Aa".into(), Some(apk(&["one", "base-apk"]))));
    let c = Component {
        package: "fixture".into(),
        class: "Activity".into(),
    };
    let value = LabelIcon {
        label: Some(String::new()),
        icon: Some(0),
    };
    step!(state.override_label_icon(c.clone(), value.clone()));
    step!(state.override_label_icon(c.clone(), value));
    step!(state.set_library_overlay_paths("B".into(), None));
    step!(state.set_overlay_paths(None));
    step!(state.set_library_overlay_paths("Aa".into(), None));
    step!(state.set_library_overlay_paths("BB".into(), None));
    step!(state.override_label_icon(c, LabelIcon::default()));
    out
}

fn runtime_capture() -> aim_services::package::owner::user_runtime::State {
    use aim_services::package::{
        model::OverlayPaths,
        owner::user_runtime::{Component, LabelIcon, State},
    };
    let mut state = State::default();
    state.set_overlay_paths(Some(OverlayPaths {
        resource_dirs: vec!["base-apk".into()],
        overlay_paths: vec!["base".into(), "base".into(), "base-apk".into()],
    }));
    for (library, names) in [
        ("Aa", vec!["one", "base-apk"]),
        ("B", vec!["two"]),
        ("BB", vec!["three"]),
    ] {
        state.set_library_overlay_paths(
            library.into(),
            Some(OverlayPaths {
                resource_dirs: names.iter().map(|s| s.to_string()).collect(),
                overlay_paths: names.iter().map(|s| s.to_string()).collect(),
            }),
        );
    }
    state.override_label_icon(
        Component {
            package: "fixture".into(),
            class: "Activity".into(),
        },
        LabelIcon {
            label: Some(String::new()),
            icon: Some(0),
        },
    );
    state
}

fn setting_runtime_expected() -> Vec<u8> {
    use aim_services::package::settings::Package;
    fn state(out: &mut Vec<u8>, p: &Package) {
        out.extend_from_slice(&p.loading_progress.to_bits().to_be_bytes());
        out.push(u8::from(p.is_loading()));
        out.extend_from_slice(&p.loading_completed_time.to_be_bytes());
        out.extend_from_slice(
            &p.old_paths
                .as_ref()
                .map_or(-1, |v| v.len() as i32)
                .to_be_bytes(),
        );
        if let Some(paths) = &p.old_paths {
            for path in paths {
                out.extend_from_slice(&path.as_ref().map_or(-1, |v| v.len() as i32).to_be_bytes());
                if let Some(path) = path {
                    out.extend_from_slice(path.as_bytes());
                }
            }
        }
    }
    let mut p = Package::default();
    let mut out = Vec::new();
    state(&mut out, &p);
    for value in [
        -1.0,
        -0.0,
        0.0,
        f32::NAN,
        0.25,
        0.1,
        f32::from_bits(1.0f32.to_bits() - 1),
        1.0,
        f32::NAN,
        f32::from_bits(1.0f32.to_bits() + 1),
        2.0,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ] {
        p.set_loading_progress(value);
        state(&mut out, &p);
    }
    for time in [-1, i64::MIN, 123, i64::MAX] {
        p.loading_completed_time = time;
        state(&mut out, &p);
    }
    p.remove_old_path(None);
    state(&mut out, &p);
    p.add_old_path(Some("/data/sole"));
    state(&mut out, &p);
    p.remove_old_path(Some("/data/sole"));
    state(&mut out, &p);
    for path in [
        Some("//data//B/"),
        Some("/data/B"),
        Some("a/../b"),
        Some(""),
        None,
        Some("Aa"),
        Some("BB"),
    ] {
        p.add_old_path(path);
        state(&mut out, &p);
    }
    let copy = p.clone();
    for path in [
        None,
        Some("missing"),
        Some("/data/B/"),
        Some("a/../b"),
        Some(""),
        Some("Aa"),
        Some("BB"),
    ] {
        p.remove_old_path(path);
        state(&mut out, &p);
    }
    state(&mut out, &copy);
    out
}

fn loading_xml_expected(directory: &std::path::Path, name: &str) -> Vec<u8> {
    let mut expected = Vec::new();
    let mut index = 0;
    for disabled in [false, true] {
        for attrs in [
            "",
            "loadingProgress='-1'",
            "loadingProgress='NaN'",
            "loadingProgress='Infinity'",
            "loadingProgress='0.5' loadingCompletedTime='-1'",
            "loadingProgress='1' isLoading='true'",
            "loadingProgress='bad' loadingCompletedTime='bad-value'",
            "loadingProgress='0.25' isLoading='false' loadingCompletedTime='1234'",
        ] {
            let tag = if disabled {
                "updated-package"
            } else {
                "package"
            };
            let xml = format!(
                "<{tag} name='fixture' codePath='/data/app/fixture' userId='10001' {attrs}/>"
            );
            let root = aim_android_xml::read(xml.as_bytes()).unwrap();
            for bytes in [
                xml.into_bytes(),
                aim_android_xml::abx::write(&root).unwrap(),
            ] {
                let e = aim_android_xml::read(&bytes).unwrap();
                let document = aim_android_xml::Element {
                    name: "packages".into(),
                    attrs: vec![],
                    content: vec![aim_android_xml::Node::Element(e)],
                };
                let parsed = aim_services::package::settings::Settings::parse(&document).unwrap();
                let p = if disabled {
                    &parsed.disabled_system_packages[0]
                } else {
                    &parsed.packages[0]
                };
                expected.extend_from_slice(&p.loading_progress.to_bits().to_be_bytes());
                expected.push(u8::from(p.is_loading()));
                expected.extend_from_slice(&p.loading_completed_time.to_be_bytes());
                fs::write(directory.join(format!("{name}.loading-{index}.xml")), bytes).unwrap();
                index += 1;
            }
        }
    }
    expected
}

fn mime_xml_expected(directory: &std::path::Path, name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let cases = [
        "",
        "<mime-group name='empty'/>",
        "<mime-group name='same'><mime-type value='text/plain'/><mime-type value='text/plain'/></mime-group><mime-group name='same'><mime-type value='image/png'/></mime-group>",
        "<mime-group name='BB'><mime-type value='BB'/><mime-type value='Aa'/></mime-group><mime-group name='Aa'><mime-type value=''/></mime-group>",
        "<mime-group name='zzzzzz'><mime-type value='😀'/><mime-type/></mime-group><mime-group name='😀'/>",
        "<mime-group><mime-type value='ignored'/></mime-group><mime-group name='nested'><mime-type value='outer'><mime-type value='inner'/></mime-type><unknown><mime-type value='ignored'/></unknown></mime-group>",
    ];
    for (i, content) in cases.iter().enumerate() {
        let xml =
            format!("<package name='p' codePath='/data/p' userId='10001'>{content}</package>");
        let root = aim_android_xml::read(xml.as_bytes()).unwrap();
        for (j, bytes) in [
            xml.into_bytes(),
            aim_android_xml::abx::write(&root).unwrap(),
        ]
        .into_iter()
        .enumerate()
        {
            let document = aim_android_xml::Element {
                name: "packages".into(),
                attrs: vec![],
                content: vec![aim_android_xml::Node::Element(
                    aim_android_xml::read(&bytes).unwrap(),
                )],
            };
            let parsed = aim_services::package::settings::Settings::parse(&document).unwrap();
            let groups = &parsed.packages[0].mime_groups;
            out.extend_from_slice(&(groups.len() as i32).to_be_bytes());
            for (name, types) in groups {
                let name = name.as_ref().expect("XML does not import null MIME groups");
                out.extend_from_slice(&(name.len() as i32).to_be_bytes());
                out.extend_from_slice(name.as_bytes());
                out.extend_from_slice(&(types.len() as i32).to_be_bytes());
                for value in types {
                    let value = value.as_ref().expect("XML does not import null MIME types");
                    out.extend_from_slice(&(value.len() as i32).to_be_bytes());
                    out.extend_from_slice(value.as_bytes());
                }
            }
            fs::write(
                directory.join(format!("{name}.mime-{}.xml", i * 2 + j)),
                bytes,
            )
            .unwrap();
        }
    }
    out
}

fn keysets_xml_expected(directory: &std::path::Path, name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, content) in ["", "<upgrade-keyset identifier='2'/><upgrade-keyset identifier='1'/><upgrade-keyset identifier='2'/>", "<defined-keyset identifier='1'/><defined-keyset alias='' identifier='2'/><defined-keyset identifier='3'/>", "<defined-keyset alias='BB' identifier='1'/><defined-keyset alias='Aa' identifier='2'/><defined-keyset alias='BB' identifier='3'/>", "<defined-keyset alias='zzzzzz' identifier='1'/><defined-keyset alias='😀' identifier='2'/><proper-signing-keyset identifier='17'/><proper-signing-keyset identifier='19'/>", "<defined-keyset alias='same' identifier='1'/><defined-keyset alias='only' identifier='2'/><defined-keyset alias='same' identifier='3'/><upgrade-keyset identifier='-1'/><upgrade-keyset identifier='-1'/>"] .iter().enumerate() {
        let xml = format!("<package name='p' codePath='/data/p' userId='10001'>{content}</package>");
        let root = aim_android_xml::read(xml.as_bytes()).unwrap();
        for (j, bytes) in [xml.into_bytes(), aim_android_xml::abx::write(&root).unwrap()].into_iter().enumerate() {
            let e = aim_android_xml::read(&bytes).unwrap();
            let document = aim_android_xml::Element { name: "packages".into(), attrs: vec![], content: vec![aim_android_xml::Node::Element(e)] };
            let parsed = aim_services::package::settings::Settings::parse(&document).unwrap();
            let data = &parsed.packages[0].key_set_data;
            out.extend_from_slice(&data.proper_signing_key_set.to_be_bytes());
            out.extend_from_slice(&if data.upgrade_key_sets.is_empty() { -1i32 } else { data.upgrade_key_sets.len() as i32 }.to_be_bytes());
            for id in &data.upgrade_key_sets { out.extend_from_slice(&id.to_be_bytes()); }
            out.extend_from_slice(&(data.defined_key_sets.len() as i32).to_be_bytes());
            for (alias, id) in &data.defined_key_sets {
                out.extend_from_slice(&alias.as_ref().map_or(-1, |s| s.len() as i32).to_be_bytes());
                if let Some(alias) = alias { out.extend_from_slice(alias.as_bytes()); }
                out.extend_from_slice(&id.to_be_bytes());
            }
            fs::write(directory.join(format!("{name}.keysets-{}.xml", i*2+j)), bytes).unwrap();
        }
    }
    out
}

/// Raw optional owners are independent of declaration constructors and query feeds.
fn write_library_owner_fixture(directory: &std::path::Path) {
    use aim_services::package::{info::ApplicationInfo, model::SharedLibrary};
    let library = |name: &str, optional_dependents, cert_digests| SharedLibrary {
        name: Some(name.into()),
        declaring: ("owner".into(), 7),
        kind: 3,
        optional_dependents,
        cert_digests,
        ..Default::default()
    };
    let nested = library(
        "nested",
        Some(vec![Some(("nested.consumer".into(), 23))]),
        Some(vec![Some("nested.digest".into())]),
    );
    let mut populated = library(
        "populated",
        Some(vec![None, Some(("consumer".into(), i64::MAX))]),
        Some(vec![None, Some("digest".into())]),
    );
    populated
        .dependents
        .push(Some(("dependent.consumer".into(), 41)));
    populated.dependencies.push(Some(nested));
    let info = ApplicationInfo {
        shared_library_files: Some(vec![
            None,
            Some(String::new()),
            Some("/system/framework/lib.jar".into()),
            None,
        ]),
        shared_library_infos: Some(vec![
            library("null", None, None),
            library("empty", Some(vec![]), Some(vec![])),
            populated.clone(),
        ]),
        optional_shared_library_infos: Some(vec![populated]),
        ..Default::default()
    };
    let mut parcel = aim_binder_host::parcel::Parcel::new();
    info.write(&mut parcel, None);
    fs::write(directory.join("library-owners.parcel"), parcel.data()).unwrap();
}

fn cache_validation_objects(directory: &std::path::Path, original: &AndroidPackage) {
    let mut empty = original.clone();
    empty.feature_flag_state = Some(Vec::new());
    empty.processes = None;
    let entry = empty.to_cache_entry().unwrap();
    fs::write(directory.join("cache-validation-empty"), &entry.bytes).unwrap();
    assert_eq!(
        AndroidPackage::read_cache_entry(&entry.bytes).unwrap(),
        empty
    );
    let mut null = entry.bytes.clone();
    null[4..8].copy_from_slice(&(-1i32).to_le_bytes());
    assert!(AndroidPackage::read_cache_entry(&null).is_err());
    fs::write(directory.join("cache-validation-null-array"), null).unwrap();
    let mut invalid = empty.clone();
    invalid.feature_flag_state = None;
    assert!(invalid.to_cache_entry().is_err());
    let mut populated = empty.clone();
    let mut flags = vec![
        Some("aim.test.true=1".into()),
        Some("aim.test.false=0".into()),
        Some("aim.test.unknown=?".into()),
    ];
    flags.sort_by_key(|flag: &Option<String>| {
        aim_services::package::info::java_hash(flag.as_ref().unwrap().rsplit_once('=').unwrap().0)
    });
    populated.feature_flag_state = Some(flags);
    let entry = populated.to_cache_entry().unwrap();
    assert!(
        AndroidPackage::read_cache_entry(&entry.bytes).unwrap() == populated,
        "populated feature owner changed"
    );
    fs::write(directory.join("cache-validation-populated"), &entry.bytes).unwrap();
    let mut null = entry.bytes.clone();
    let null_index = entry.pool.iter().position(Option::is_none).unwrap() as i32;
    null[8..12].copy_from_slice(&null_index.to_le_bytes());
    assert!(AndroidPackage::read_cache_entry(&null).is_err());
    fs::write(directory.join("cache-validation-null-string"), null).unwrap();
    invalid.feature_flag_state = Some(vec![None]);
    assert!(invalid.to_cache_entry().is_err());
    let mut process = empty.clone();
    process.processes = Some(vec![aim_services::package::pkg::Process {
        map_key: None,
        name: Some(String::new()),
        ..Default::default()
    }]);
    let entry = process.to_cache_entry().unwrap();
    assert!(
        AndroidPackage::read_cache_entry(&entry.bytes).unwrap() == process,
        "empty process name owner changed"
    );
    fs::write(
        directory.join("cache-validation-empty-process-name"),
        &entry.bytes,
    )
    .unwrap();
    process.processes.as_mut().unwrap()[0].name =
        Some("aim.native.cache.validation.process".into());
    let entry = process.to_cache_entry().unwrap();
    let index = entry
        .pool
        .iter()
        .position(|value| value.as_deref() == Some("aim.native.cache.validation.process"))
        .unwrap() as i32;
    let positions: Vec<_> = entry
        .strings
        .iter()
        .filter(|(_, value)| *value == index)
        .collect();
    assert_eq!(positions.len(), 1);
    let null_index = entry.pool.iter().position(Option::is_none).unwrap() as i32;
    let mut null = entry.bytes.clone();
    null[positions[0].0..positions[0].0 + 4].copy_from_slice(&null_index.to_le_bytes());
    assert!(AndroidPackage::read_cache_entry(&null).is_err());
    fs::write(directory.join("cache-validation-null-process-name"), null).unwrap();
    process.processes.as_mut().unwrap()[0].name = None;
    assert!(process.to_cache_entry().is_err());
}
