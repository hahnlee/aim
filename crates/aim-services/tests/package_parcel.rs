//! Original PackageImpl read/write oracle for native scan parcel output.
use aim_services::package::pkg::AndroidPackage;
use std::{
    fs,
    process::Command,
    time::{Duration, Instant},
};
mod common {
    pub mod java;
    pub mod runtime;
}
use common::java::sources;
use common::runtime::{Boot, Data, run};
#[test]
#[ignore = "requires pinned image, aimctl, JDK and d8; run explicitly"]
fn native_package_parcels_match_original_read_write() {
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
        .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/api/SELinuxMMAC.java")));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
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
                .join("java/device-services/src/dev/aim/server/SharedUserReplica.java"),
        )
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
    for (name, package, entry) in expected {
        if name.starts_with("scan-") {
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
        image,
        &apks,
        &config,
        FirstBootSystemInputs {
            // Controlled target for the original envelope/replica oracle.
            seinfo: aim_services::package::scan::SeInfoScan {
                policy: &policy,
                compatibility: &|_: &aim_services::package::pkg::AndroidPackage| Ok(36),
            },
            apex_settings: &Default::default(),
            first_api_level: 36,
            vendor_sdk: 36,
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
