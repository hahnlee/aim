//! Compare setting construction and initial updates with the pinned original PMS.
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
    let original_update = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.NewSettingOracle",
        "update",
    ]));
    let root = aim_paths::original_image();
    let image = root.clone();
    let apks = aim_services::package::write::Apks {
        files: Box::new(move |p| Some(image.join(p.trim_start_matches('/')))),
        platform: aim_services::package::parse::Platform::load(&root, Default::default()).unwrap(),
    };
    let input = aim_services::package::State {
        settings: Settings {
            packages: vec![aim_services::package::settings::Package {
                name: "com.google.android.gsf".into(),
                code_path:
                    "/system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk"
                        .into(),
                ..Default::default()
            }],
            ..Default::default()
        },
        list: vec![],
        access: None,
        users: vec![],
    };
    let template = Inputs::load_verified_code(&input, &apks)
        .unwrap()
        .active
        .remove("com.google.android.gsf")
        .unwrap();
    let mut lines = Vec::new();
    for old_system in [0, 1] {
        for new_system in [0, 1] {
            for required in [0, 512] {
                for changed in [false, true] {
                    // Synthetic policy inputs isolate setting updates. The signer
                    // comes from the original fully verified GSF APK; no APK changes.
                    let package = aim_services::package::settings::Package {
                        name: "fixture".into(),
                        app_id: 10000,
                        code_path: "/system/nonexistent/old".into(),
                        legacy_native_library_path: Some("old.lib".into()),
                        primary_cpu_abi: Some("old.abi".into()),
                        flags: 64 | old_system,
                        private_flags: required,
                        version_code: 7,
                        mime_groups: vec![("keep".into(), vec![]), ("remove".into(), vec![])],
                        ..Default::default()
                    };
                    let settings = Settings {
                        packages: vec![package],
                        ..Default::default()
                    };
                    let mut scan = SigningScan::new(&Default::default(), &settings, 36).unwrap();
                    let mut parsed = template.parsed.clone();
                    parsed.package_name = "fixture".into();
                    parsed.manifest_package_name = Some("fixture".into());
                    parsed.shared_user_id = None;
                    let code = Code {
                        parsed,
                        signing: template.signing.clone(),
                        location: Location {
                            path: if changed {
                                "/system/nonexistent/new"
                            } else {
                                "/system/nonexistent/old"
                            }
                            .into(),
                            partition: Partition::System,
                            kind: Kind::App,
                            apex: None,
                        },
                    };
                    let saved_users = std::collections::BTreeMap::from([(
                        "fixture".into(),
                        std::collections::BTreeMap::from([(
                            0,
                            aim_services::package::restrictions::UserState {
                                installed: false,
                                uninstall_reason: 3,
                                ..Default::default()
                            },
                        )]),
                    )]);
                    let candidate = scan
                        .apply_existing(
                            &code,
                            SettingUpdate {
                                code_path: code.location.path.clone(),
                                legacy_native_library_path: Some("new.lib".into()),
                                primary_cpu_abi: Some("new.abi".into()),
                                secondary_cpu_abi: None,
                                flags: 128 | new_system,
                                private_flags: 8 | (required ^ 512),
                                uses_sdk_libraries: vec![],
                                uses_static_libraries: vec![],
                                mime_groups: vec!["keep".into(), "new".into()],
                                domain_set_id: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1],
                                target_sdk_version: 36,
                                restrict_update_hash: Some(vec![1]),
                            },
                            &saved_users,
                            None,
                            None,
                        )
                        .unwrap();
                    let p = candidate.record.settings;
                    let mut names: Vec<_> = p.mime_groups.iter().map(|(n, _)| n.as_str()).collect();
                    names.sort();
                    lines.push(format!(
                        "{} {} {} {} {} {} {} {} {} [{}]",
                        p.app_id,
                        p.shared_user,
                        p.flags,
                        p.private_flags,
                        p.legacy_native_library_path.unwrap(),
                        p.primary_cpu_abi.unwrap(),
                        p.version_code,
                        candidate.users[&0].installed,
                        candidate.users[&0].uninstall_reason,
                        names.join(", ")
                    ));
                }
            }
        }
    }
    assert_eq!(
        String::from_utf8(original_update.stdout).unwrap(),
        lines.join("\n") + "\n"
    );
    let started = run(boot.command().args([
        "shell",
        "am",
        "start",
        "-W",
        "-n",
        "com.android.settings/.Settings",
    ]));
    assert!(String::from_utf8_lossy(&started.stdout).contains("Status: ok"));
}
