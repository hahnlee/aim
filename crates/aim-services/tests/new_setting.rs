//! Compare new-setting defaults against the pinned original constructor.
use aim_services::package::{
    pkg::AndroidPackage,
    scan::*,
    settings::{Settings, UsesSdkLibrary},
    sign::SigningDetails,
    system_config::SystemConfig,
};
use std::fs;
use std::process::Command;
use std::time::{Duration, Instant};

mod common {
    pub mod java;
    pub mod runtime;
}
use common::java::sources;
use common::runtime::{Boot, Data, run};

#[test]
#[ignore = "requires aimctl, the pinned derived image, JDK and d8; run explicitly"]
fn new_settings_match_the_original_runtime() {
    let dir = std::env::temp_dir().join(format!("aim-setting-{}", std::process::id()));
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
        )));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/NewSettingOracle.java"),
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
        .arg(classes.join("com/android/server/pm/NewSettingOracle.class")));
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
    fs::copy(
        dex.join("classes.dex"),
        boot.data.join("data/local/tmp/new-setting.dex"),
    )
    .unwrap();
    let original = boot
        .command()
        .args([
            "shell",
            "/system/bin/app_process",
            "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
            "/system/bin",
            "com.android.server.pm.NewSettingOracle",
        ])
        .output()
        .unwrap();
    if !original.status.success() {
        let logs = boot
            .command()
            .args(["shell", "logcat", "-d", "-t", "300"])
            .output()
            .unwrap();
        panic!(
            "original constructor: {}\n{}",
            String::from_utf8_lossy(&original.stderr),
            String::from_utf8_lossy(&logs.stdout)
        );
    }

    let mut lines = Vec::new();
    for flags in [0, 1, 0x40000000, 0x40000001] {
        for shared in [false, true] {
            for stopped in [false, true] {
                for user in [None, Some(0), Some(10), Some(-1)] {
                    let mut scan =
                        UidScan::new(&SystemConfig::default(), &Settings::default()).unwrap();
                    // Synthetic metadata isolates constructor policy from APK verification.
                    let code = Code {
                        location: Location {
                            path: "/system/nonexistent/fixture".into(),
                            partition: Partition::System,
                            kind: Kind::App,
                            apex: None,
                        },
                        parsed: AndroidPackage {
                            package_name: "fixture".into(),
                            shared_user_id: shared.then(|| "group".into()),
                            ..Default::default()
                        },
                        signing: SigningDetails::from_saved(&Default::default()).unwrap(),
                    };
                    let (identity, _) = scan.apply(&code).unwrap();
                    let metadata = SettingMetadata {
                        code_path: code.location.path.clone(),
                        legacy_native_library_path: Some("/system/lib64".into()),
                        primary_cpu_abi: Some("arm64-v8a".into()),
                        secondary_cpu_abi: None,
                        version_code: 0x100000002,
                        flags,
                        private_flags: 8,
                        last_modified_time: 0,
                        uses_sdk_libraries: vec![UsesSdkLibrary {
                            name: "sdk".into(),
                            version_major: 9,
                            optional: true,
                        }],
                        uses_static_libraries: vec![("static".into(), 7)],
                        mime_groups: vec!["mime".into()],
                        domain_set_id: std::array::from_fn(|i| i as u8),
                        target_sdk_version: 36,
                        restrict_update_hash: Some(vec![1, 2]),
                    };
                    let policy = UserPolicy {
                        install_user: user,
                        users: None,
                        allow_install: true,
                        instant_app: true,
                        virtual_preload: true,
                        stopped_system_app: stopped,
                    };
                    let candidate = scan
                        .new_setting(&identity, metadata.clone(), policy)
                        .unwrap();
                    use aim_services::package::owner::app_ids::Owner;
                    let id = candidate.package.app_id;
                    scan.identities
                        .ids
                        .replace(id, Owner::Package("other".into()))
                        .unwrap();
                    assert!(
                        scan.new_setting(&identity, metadata.clone(), policy)
                            .is_err()
                    );
                    let owner = if shared {
                        Owner::SharedUser("group".into())
                    } else {
                        Owner::Package("fixture".into())
                    };
                    scan.identities.ids.replace(id, owner).unwrap();
                    let p = &candidate.package;
                    assert_eq!(p.uses_sdk_libraries, metadata.uses_sdk_libraries);
                    assert_eq!(p.uses_static_libraries, metadata.uses_static_libraries);
                    assert_eq!(p.mime_groups, vec![("mime".into(), Vec::new())]);
                    assert_eq!(p.flags, flags);
                    assert_eq!(p.private_flags, 8);
                    assert_eq!(p.version_code, 0x100000002);
                    assert_eq!(p.install_source.installer_uid, -1);
                    assert!(p.signatures.is_none());
                    let mut line = format!(
                        "{} {} {} {} {} {} {:.1} {} {}",
                        p.app_id,
                        p.shared_user,
                        p.category_hint,
                        p.page_size_compat,
                        p.key_set_data.proper_signing_key_set,
                        p.loading,
                        p.loading_progress,
                        p.domain_set_id.as_ref().unwrap(),
                        p.scanned_as_stopped_system_app
                    );
                    for id in [0, 10, -1] {
                        let state = candidate.users.get(&id).cloned().unwrap_or_default();
                        line.push_str(&format!(
                            " {} {} {} {} {}",
                            state.installed,
                            state.stopped,
                            state.not_launched,
                            state.instant_app,
                            state.virtual_preload
                        ));
                    }
                    lines.push(line);
                    assert!(scan.prune_unused_groups().is_err());
                    assert!(scan.accept_uid("fixture").unwrap());
                    assert!(scan.new_setting(&identity, metadata, policy).is_err());
                }
            }
        }
    }
    assert_eq!(
        String::from_utf8(original.stdout).unwrap(),
        lines.join("\n") + "\n"
    );
    run(boot.command().args([
        "shell",
        "am",
        "start",
        "-W",
        "-n",
        "com.android.settings/.Settings",
    ]));
}
