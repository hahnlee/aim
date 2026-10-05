//! Complete native capture exported to the original ART snapshot assembler.
use super::{
    java,
    runtime::{Boot, Data, run},
};
use aim_service_aidl::WriteParcelable;
use aim_services::package::scan_snapshot::{
    Snapshot, endpoint, runtime_record, setting_record, shared_record, user_record,
};
use std::{
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

pub fn export(directory: &Path, snapshot: &Snapshot) {
    fs::create_dir_all(directory.join("group")).unwrap();
    fs::write(directory.join("version"), snapshot.version().to_string()).unwrap();
    for (settings, factory, scope) in [
        (&snapshot.owner().settings.packages, false, "active"),
        (
            &snapshot.owner().settings.disabled_system_packages,
            true,
            "factory",
        ),
    ] {
        let mut names = Vec::new();
        for setting in settings {
            let name = &setting.name;
            names.push(name.clone());
            let root = directory.join(scope).join(name);
            fs::create_dir_all(&root).unwrap();
            fs::write(
                root.join("setting"),
                setting_record::captured(snapshot, name, factory)
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            fs::write(
                root.join("runtime"),
                runtime_record::captured(snapshot, name, factory)
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
            if let Some(code) = endpoint::PackageCode::captured(snapshot, name, factory).unwrap() {
                let mut parcel = aim_binder_host::parcel::Parcel::new();
                code.write_to(&mut parcel);
                fs::write(root.join("code"), parcel.data()).unwrap();
            }
            let mut parcel = aim_binder_host::parcel::Parcel::new();
            endpoint::PackageSigningState::captured(snapshot, name, factory)
                .unwrap()
                .unwrap()
                .write_to(&mut parcel);
            fs::write(root.join("signing"), parcel.data()).unwrap();
            let mut parcel = aim_binder_host::parcel::Parcel::new();
            endpoint::PackageTransientState::captured(snapshot, name, factory)
                .unwrap()
                .write_to(&mut parcel);
            fs::write(root.join("transient"), parcel.data()).unwrap();
            fs::write(
                root.join("hidden"),
                snapshot
                    .owner()
                    .hidden_api_enforcement_policy(name, factory)
                    .unwrap()
                    .unwrap()
                    .to_string(),
            )
            .unwrap();
            let ids = user_record::ids(snapshot, name, factory).unwrap().unwrap();
            fs::write(
                root.join("users"),
                ids.iter()
                    .map(i32::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .unwrap();
            for id in ids {
                fs::write(
                    root.join(format!("user-{id}")),
                    user_record::captured(snapshot, name, factory, id)
                        .unwrap()
                        .unwrap(),
                )
                .unwrap();
            }
        }
        names.sort();
        fs::write(directory.join(format!("{scope}-names")), names.join("\n")).unwrap();
    }
    let groups: Vec<_> = snapshot
        .owner()
        .identities
        .shared_users
        .keys()
        .cloned()
        .collect();
    fs::write(directory.join("groups"), groups.join("\n")).unwrap();
    for name in groups {
        fs::write(
            directory.join("group").join(&name),
            shared_record::captured(snapshot, &name).unwrap().unwrap(),
        )
        .unwrap();
    }
}

pub fn verify(directory: &Path) {
    let root = directory.join("oracle-build");
    fs::create_dir(&root).unwrap();
    let classes = root.join("classes");
    let stubs = root.join("stubs");
    let dex = root.join("dex");
    for path in [&classes, &stubs, &dex] {
        fs::create_dir(path).unwrap();
    }
    let java_root = aim_paths::fetched().join("java");
    let jdk = java_root.join("temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let repo = aim_paths::root();
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(java::sources(&repo.join("java/device-services/stubs")))
        .arg(repo.join("crates/aim-services/tests/api/PackageBootstrapBridge.java"))
        .arg(repo.join("crates/aim-services/tests/api/SELinuxMMAC.java")));
    let sources = [
        "dev/aim/server/PackageTransientState.java",
        "dev/aim/server/PackageLegacyPermissions.java",
        "dev/aim/server/PackageDomainIds.java",
        "dev/aim/server/PackageScanUsers.java",
        "com/android/server/pm/ApexBootFeed.java",
        "dev/aim/server/PackageObjects.java",
        "com/android/server/pm/CapturedPackageSetting.java",
        "dev/aim/server/PackageLibraryState.java",
        "dev/aim/server/PackageLibraryFeed.java",
        "dev/aim/server/PackageCode.java",
        "dev/aim/server/PackageSigningState.java",
        "dev/aim/server/PackageUserStateData.java",
        "dev/aim/server/PackageSettingData.java",
        "dev/aim/server/PackageMimeGroups.java",
        "com/android/server/pm/CapturedInstallSource.java",
        "com/android/server/pm/CapturedKeySetData.java",
        "dev/aim/server/PackageUserStateReplica.java",
        "dev/aim/server/PackageSeInfoState.java",
        "dev/aim/server/PackageUsageState.java",
        "dev/aim/server/PackageScanLease.java",
        "dev/aim/server/PackageStateReplica.java",
        "dev/aim/server/SharedUserData.java",
        "com/android/server/pm/SharedProcessFeed.java",
        "dev/aim/server/RetainedPackageData.java",
        "dev/aim/server/SharedUserReplica.java",
        "dev/aim/server/PackageRuntimeState.java",
        "dev/aim/server/PackageRuntimeFeed.java",
        "dev/aim/server/PackageUserScopeFeed.java",
        "dev/aim/server/PackageSnapshots.java",
    ];
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .args(
            sources
                .iter()
                .map(|name| repo.join("java/device-services/src").join(name)),
        )
        .arg(java::bootstrap_aidl(&root))
        .arg(java::snapshot_aidl(&root))
        .arg(repo.join("crates/aim-services/tests/fixtures/DisplacedSnapshotOracle.java"))
        .arg(repo.join("crates/aim-services/tests/fixtures/NativeDisplacedReadOracle.java")));
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
        .arg(java_root.join("build-tools-36.0.0/android-16/lib/d8.jar"))
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
    java::check_linkage(
        &dex.join("classes.dex"),
        &[
            "/system/framework/services.jar",
            "/system/framework/aim-services.jar",
        ],
    )
    .unwrap();
    let boot_data = Data(std::env::temp_dir().join(format!("aim-dsp-{}", std::process::id())));
    fs::create_dir(&boot_data.0).unwrap();
    let boot_dir = boot_data.0.join("g");
    let boot = Boot {
        ctl: repo.join("target/release/aimctl"),
        data: boot_dir.clone(),
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
            "disposable snapshot boot did not complete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    let guest = boot.data.join("data/local/tmp/displaced-snapshots");
    fs::create_dir(&guest).unwrap();
    let mut pending = vec![(directory.join("captures"), guest.clone())];
    while let Some((from, to)) = pending.pop() {
        for entry in fs::read_dir(from).unwrap() {
            let path = entry.unwrap().path();
            let target = to.join(path.file_name().unwrap());
            if path.is_dir() {
                fs::create_dir(&target).unwrap();
                pending.push((path, target));
            } else {
                fs::copy(path, target).unwrap();
            }
        }
    }
    fs::copy(dex.join("classes.dex"), guest.join("oracle.dex")).unwrap();
    let output = boot.command().args(["shell", "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/displaced-snapshots/oracle.dex:/system/framework/services.jar",
        "/system/bin", "DisplacedSnapshotOracle", "/data/local/tmp/displaced-snapshots"]).output().unwrap();
    assert!(
        output.status.success(),
        "displaced snapshot oracle: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "displaced full snapshot contracts: 6 cases\n"
    );
    let output = boot.command().args(["shell", "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/displaced-snapshots/oracle.dex:/system/framework/services.jar",
        "/system/bin", "com.android.server.pm.NativeDisplacedReadOracle", "/data/local/tmp/displaced-snapshots"]).output().unwrap();
    assert!(
        output.status.success(),
        "native displaced read oracle: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "native displaced read contracts: 6 cases\n"
    );
    drop(boot);
    for index in 0..6 {
        let mounted = aim_storage::data::DataImage::attach(&boot_dir, None).unwrap();
        let system = mounted.dir().join("data/system");
        for name in [
            "packages.xml",
            "packages-backup.xml",
            "packages.xml.reservecopy",
        ] {
            let path = system.join(name);
            if path.exists() {
                fs::remove_file(path).unwrap();
            }
        }
        let source = directory.join(format!(
            "captures/case-{index}/persisted/system/packages.xml"
        ));
        fs::copy(&source, system.join("packages.xml")).unwrap();
        assert_eq!(
            fs::read(&source).unwrap(),
            fs::read(system.join("packages.xml")).unwrap()
        );
        mounted.detach().unwrap();
        let reboot = Boot {
            ctl: repo.join("target/release/aimctl"),
            data: boot_dir.clone(),
        };
        run(reboot.command().args(["start", "--windows"]));
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let output = reboot
                .command()
                .args(["shell", "getprop", "sys.boot_completed"])
                .output()
                .unwrap();
            if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "1" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "native displaced restart did not boot: {index}"
            );
            std::thread::sleep(Duration::from_secs(1));
        }
        for name in ["android", "com.google.android.gsf"] {
            let output = run(reboot
                .command()
                .args(["shell", "cmd", "package", "path", name]));
            assert!(
                String::from_utf8_lossy(&output.stdout).starts_with("package:"),
                "restarted package missing: {index}/{name}"
            );
        }
        let restored = aim_services::package::State::read(&reboot.data.join("data"), &[0])
            .unwrap()
            .unwrap();
        assert!(
            restored
                .settings
                .packages
                .iter()
                .any(|p| p.name == "android")
        );
        assert!(
            restored
                .settings
                .packages
                .iter()
                .any(|p| p.name == "com.google.android.gsf")
        );
        drop(reboot);
    }
}
