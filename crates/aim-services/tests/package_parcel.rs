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
        .arg("-classpath")
        .arg(&stubs)
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/PackageRoundTripOracle.java"),
        )
        .arg(aim_paths::root().join("java/device-services/src/dev/aim/server/PackageObjects.java"))
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
                .join("java/device-services/src/dev/aim/server/PackageSeInfoState.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/PackageUsageState.java"),
        )
        .arg(
            aim_paths::root().join("java/device-services/src/dev/aim/server/PackageScanLease.java"),
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
    for loaded in snapshot.owner().loaded_packages().values() {
        let pkg = &loaded.package;
        let name = format!("scan-{}.native", pkg.uid);
        let facade = loaded.facade_entry().unwrap();
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
    for (name, package, entry) in expected {
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
    let config = SystemConfig::default();
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
    let image = Image::load(&apks, &[]).unwrap();
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
    owner
        .assign_seinfo_at_boot(&policy, &mut |_| Ok(36))
        .unwrap();
    aim_services::package::scan_snapshot::Store::new(owner, usage)
        .unwrap()
        .capture()
}
