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
    let original_groups = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.NewSettingOracle",
        "group-flags",
    ]));
    let mut expected_groups = String::new();
    for seed in [0, 64] {
        for private_seed in [0, 8] {
            for update in [0, 2, 128] {
                for private_update in [0, 16] {
                    use aim_services::package::owner::shared_users::SharedUser;
                    use std::fmt::Write;
                    let mut group = SharedUser::new(10001, seed, private_seed);
                    let mut state = |g: &SharedUser| {
                        writeln!(&mut expected_groups, "{} {}", g.flags, g.private_flags).unwrap();
                    };
                    state(&group);
                    group.add_package("a", 1, 32);
                    state(&group);
                    group.add_package("a", 1, 32);
                    state(&group);
                    group.add_package("a", update, private_update);
                    state(&group);
                    group.add_package("b", 128, 16);
                    state(&group);
                    for name in ["a", "a", "b"] {
                        writeln!(&mut expected_groups, "{}", group.remove_package(name)).unwrap();
                        writeln!(
                            &mut expected_groups,
                            "{} {}",
                            group.flags, group.private_flags
                        )
                        .unwrap();
                    }
                    group.add_package("a", update, private_update);
                    writeln!(
                        &mut expected_groups,
                        "{} {}",
                        group.flags, group.private_flags
                    )
                    .unwrap();
                    writeln!(&mut expected_groups, "{}", group.remove_package("a")).unwrap();
                    writeln!(
                        &mut expected_groups,
                        "{} {}",
                        group.flags, group.private_flags
                    )
                    .unwrap();
                }
            }
        }
    }
    assert_eq!(
        String::from_utf8(original_groups.stdout).unwrap(),
        expected_groups
    );
    eprintln!("native/original shared UID flags match 24 add/update/remove sequences");
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
            packages: vec![
                aim_services::package::settings::Package {
                    name: "com.google.android.gsf".into(),
                    code_path: concat!(
                        "/system_ext/priv-app/GoogleServicesFramework/",
                        "GoogleServicesFramework.apk"
                    )
                    .into(),
                    ..Default::default()
                },
                aim_services::package::settings::Package {
                    name: "android".into(),
                    code_path: "/system/framework/framework-res.apk".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        },
        list: vec![],
        access: None,
        users: vec![],
    };
    let mut inputs = Inputs::load_verified_code(&input, &apks).unwrap();
    let platform_signing = inputs.active.remove("android").unwrap().signing;
    let template = inputs.active.remove("com.google.android.gsf").unwrap();
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
    // The original method reads the native parser's actual unchanged GSF
    // path through its own PackageCacher and filesystem; no timestamps are fed.
    let path = &template.settings.code_path;
    let cache = aim_services::package::parse::parse(
        &root.join(path.trim_start_matches('/')),
        path,
        aim_services::package::parse::PARSE_IS_SYSTEM_DIR,
        &apks.platform,
    )
    .unwrap()
    .to_cache_entry();
    fs::write(
        boot.data.join("data/local/tmp/code-time.cache"),
        cache.bytes,
    )
    .unwrap();
    let original_time = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.NewSettingOracle",
        "time",
        "/data/local/tmp/code-time.cache",
    ]));
    let expected_time: i64 = String::from_utf8(original_time.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(
        apks.scan_file_time(&template.parsed).unwrap(),
        expected_time
    );
    let properties =
        String::from_utf8(run(boot.command().args(["shell", "getprop"])).stdout).unwrap();
    let properties: std::collections::BTreeMap<_, _> = properties
        .lines()
        .filter_map(|line| {
            let (name, value) = line
                .strip_prefix('[')?
                .strip_suffix(']')?
                .split_once("]: [")?;
            Some((name.to_owned(), value.to_owned()))
        })
        .collect();
    let config = SystemConfig::read(&aim_paths::derived_image(), &|p| properties.get(p).cloned());
    let framework =
        aim_services::package::system_config::Framework::load(&aim_paths::derived_image()).unwrap();
    let app_system = aim_services::package::system_config::system(
        &aim_paths::derived_image(),
        &|p| properties.get(p).cloned(),
        &framework,
    );
    let original_bcp = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.NewSettingOracle",
        "library-policy",
    ]));
    let on_bcp: bool = String::from_utf8(original_bcp.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let compatibility =
        LibraryCompatibility::new(&config, &|p| properties.get(p).cloned(), on_bcp).unwrap();
    if !on_bcp {
        let mut unresolved = template.parsed.clone();
        unresolved.booleans |= aim_services::package::pkg::booleans::CORE_APP;
        let unchanged = unresolved.clone();
        assert!(
            ScanPolicy::default()
                .apply(
                    &mut unresolved,
                    &template.signing,
                    Some(&platform_signing),
                    false,
                    &apks,
                    &compatibility,
                    None
                )
                .is_err()
        );
        assert_eq!(unresolved, unchanged);
    }
    let mut checked = 0;
    for sdk in [27, 28, 29, 30] {
        for system in [false, true] {
            for updated in [false, true] {
                for (required, optional) in [
                    ("-", "-"),
                    (
                        "android.test.runner,wear-sdk,android.net.ipsec.ike,com.google.android.maps",
                        "android.hidl.base-V1.0-java",
                    ),
                    (
                        "android.hidl.manager-V1.0-java",
                        "android.test.runner,org.apache.http.legacy,wear-sdk",
                    ),
                ] {
                    let result = run(boot.command().args([
                        "shell", "/system/bin/app_process",
                        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
                        "/system/bin", "com.android.server.pm.NewSettingOracle", "libraries",
                        "/data/local/tmp/code-time.cache", &sdk.to_string(), &system.to_string(), &updated.to_string(),
                        "/data/local/tmp/library-policy.cache", required, optional,
                    ]));
                    let change = if !system && !on_bcp {
                        Some(
                            String::from_utf8(result.stdout)
                                .unwrap()
                                .trim()
                                .parse::<bool>()
                                .unwrap(),
                        )
                    } else {
                        None
                    };
                    let expected = AndroidPackage::read_cache_entry(
                        &fs::read(boot.data.join("data/local/tmp/library-policy.cache")).unwrap(),
                    )
                    .unwrap();
                    let mut actual = template.parsed.clone();
                    actual.target_sdk_version = sdk;
                    for (names, list) in [
                        (required, &mut actual.uses_libraries),
                        (optional, &mut actual.uses_optional_libraries),
                    ] {
                        for name in names.split(',').filter(|n| *n != "-") {
                            if !list.iter().any(|n| n == name) {
                                list.push(name.into());
                            }
                        }
                    }
                    if let Some(expected_change) =
                        change.filter(|_| required == "-" && optional == "-" && !updated)
                    {
                        let mut encoded = aim_binder_host::parcel::Parcel::new();
                        use aim_service_aidl::com_android_internal_compat_iplatformcompat as compat;
                        compat::IsChangeEnabled {
                            change_id: 133396946,
                            app_info: Some(aim_services::package::info::app_info_without_state(
                                &actual,
                                &app_system,
                            )),
                        }
                        .write(&mut encoded);
                        fs::write(
                            boot.data.join("data/local/tmp/compat-info.parcel"),
                            encoded.data(),
                        )
                        .unwrap();
                        run(boot.command().args([
                            "shell", "/system/bin/app_process",
                            "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
                            "/system/bin", "com.android.server.pm.NewSettingOracle", "compat-call",
                            "/data/local/tmp/compat-info.parcel",
                        ]).arg(compat::IS_CHANGE_ENABLED.to_string())
                          .arg("/data/local/tmp/compat-reply.parcel"));
                        let bytes =
                            fs::read(boot.data.join("data/local/tmp/compat-reply.parcel")).unwrap();
                        let mut reply = aim_binder_host::parcel::Reader::new(&bytes, &[]);
                        assert_eq!(
                            compat::read_is_change_enabled_reply(&mut reply)
                                .unwrap()
                                .unwrap(),
                            expected_change,
                            "native AIDL compatibility call SDK {sdk}"
                        );
                    }
                    compatibility
                        .apply(&mut actual, system, updated, change)
                        .unwrap();
                    assert_eq!(
                        actual.uses_libraries, expected.uses_libraries,
                        "required libraries sdk={sdk} system={system} updated={updated}"
                    );
                    assert_eq!(
                        actual.uses_optional_libraries, expected.uses_optional_libraries,
                        "optional libraries sdk={sdk} system={system} updated={updated}"
                    );
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 48);
    eprintln!(
        "native/original library compatibility matches {checked} cases; test.base on BCP={on_bcp}"
    );
    // Run the original applyPolicy on the same native-parsed original APK.
    // Shared library compatibility belongs to another owner; compare the
    // manifest/component restrictions without claiming full parcel equality.
    for (policy, updated, original_flags) in [
        (ScanPolicy::default(), false, 0),
        (ScanPolicy::default(), true, 0),
        (
            ScanPolicy {
                system: true,
                privileged: true,
                system_ext: true,
                ..Default::default()
            },
            false,
            (1 << 16) | (1 << 17) | (1 << 21),
        ),
        (
            ScanPolicy {
                system: true,
                vendor: true,
                apex: true,
                ..Default::default()
            },
            false,
            (1 << 16) | (1 << 19) | (1 << 26),
        ),
    ] {
        run(boot.command().args([
            "shell",
            "/system/bin/app_process",
            "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
            "/system/bin",
            "com.android.server.pm.NewSettingOracle",
            "policy",
            "/data/local/tmp/code-time.cache",
            &original_flags.to_string(),
            &updated.to_string(),
            "/data/local/tmp/scan-policy.cache",
        ]));
        let expected = AndroidPackage::read_cache_entry(
            &fs::read(boot.data.join("data/local/tmp/scan-policy.cache")).unwrap(),
        )
        .unwrap();
        let mut actual = template.parsed.clone();
        policy
            .apply_manifest(&mut actual, &template.signing, None, updated, &apks)
            .unwrap();
        assert_eq!(actual.booleans, expected.booleans);
        assert_eq!(actual.booleans2, expected.booleans2);
        assert_eq!(
            actual
                .activities
                .iter()
                .map(|c| &c.main)
                .collect::<Vec<_>>(),
            expected
                .activities
                .iter()
                .map(|c| &c.main)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            actual.receivers.iter().map(|c| &c.main).collect::<Vec<_>>(),
            expected
                .receivers
                .iter()
                .map(|c| &c.main)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            actual.services.iter().map(|c| &c.main).collect::<Vec<_>>(),
            expected
                .services
                .iter()
                .map(|c| &c.main)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            actual.providers.iter().map(|c| &c.main).collect::<Vec<_>>(),
            expected
                .providers
                .iter()
                .map(|c| &c.main)
                .collect::<Vec<_>>()
        );
        assert_eq!(actual.permission_groups, expected.permission_groups);
        assert_eq!(actual.protected_broadcasts, expected.protected_broadcasts);
        assert_eq!(actual.original_packages, expected.original_packages);
        assert_eq!(actual.adopt_permissions, expected.adopt_permissions);
        assert_eq!(
            application_flags(&actual, updated),
            application_flags(&expected, updated)
        );
    }
    eprintln!("native manifest policy matches original applyPolicy for four GSF scan policies");
    let mut code = Code {
        location: Location {
            path: path.clone(),
            partition: Partition::SystemExt,
            kind: Kind::PrivApp,
            apex: None,
        },
        parsed: template.parsed.clone(),
        signing: template.signing.clone(),
    };
    ScanPolicy::for_location(&code.location)
        .apply(
            &mut code.parsed,
            &code.signing,
            Some(&platform_signing),
            false,
            &apks,
            &compatibility,
            None,
        )
        .unwrap();
    let (flags, private_flags) = application_flags(&code.parsed, false);
    let mut scan = SigningScan::new(&Default::default(), &Default::default(), 36).unwrap();
    let candidate = scan
        .apply_new_system(
            &code,
            SettingMetadata {
                code_path: path.clone(),
                legacy_native_library_path: None,
                primary_cpu_abi: None,
                secondary_cpu_abi: None,
                version_code: 0,
                flags,
                private_flags,
                last_modified_time: 0,
                uses_sdk_libraries: vec![],
                uses_static_libraries: vec![],
                mime_groups: code.parsed.mime_groups.clone(),
                domain_set_id: [0; 16],
                target_sdk_version: code.parsed.target_sdk_version,
                restrict_update_hash: code.parsed.restrict_update_hash.clone(),
            },
            UserPolicy {
                install_user: None,
                users: None,
                allow_install: true,
                instant_app: false,
                virtual_preload: false,
                stopped_system_app: false,
            },
        )
        .unwrap();
    let failed_candidate = NewPackageOutcome {
        record: Record {
            settings: candidate.record.settings.clone(),
            parsed: candidate.record.parsed.clone(),
            signing: candidate.record.signing.clone(),
            identity: candidate.record.identity.clone(),
            origin: candidate.record.origin,
        },
        users: candidate.users.clone(),
        signing: SigningOutcome {
            system_signature_mismatch: None,
        },
    };
    let clock = ScanClock {
        current_time: 0,
        user_id: 0,
        update_time: false,
    };
    let unreadable = aim_services::package::write::Apks {
        files: Box::new(|_| None),
        platform: aim_services::package::parse::Platform::load(&root, Default::default()).unwrap(),
    };
    let mut inaccessible_stub = code.parsed.clone();
    inaccessible_stub.path = Some("/system_ext/priv-app/Fixture-Stub".into());
    let unchanged = inaccessible_stub.clone();
    assert!(
        ScanPolicy::for_location(&code.location)
            .apply_manifest(
                &mut inaccessible_stub,
                &code.signing,
                Some(&platform_signing),
                false,
                &unreadable
            )
            .is_err()
    );
    assert_eq!(inaccessible_stub, unchanged);
    let snapshot = scan.clone();
    assert!(
        matches!(scan.finish_code_metadata(failed_candidate, &unreadable, clock),
        Err(SigningError::Rejected(ref e)) if e.phase == "code-time")
    );
    assert_eq!(scan, snapshot);
    let completed = scan.finish_code_metadata(candidate, &apks, clock).unwrap();
    assert_eq!(
        (
            completed.record.settings.flags,
            completed.record.settings.private_flags
        ),
        (flags, private_flags)
    );
    assert_eq!(completed.record.settings.last_modified_time, expected_time);
    assert_eq!(completed.record.settings.last_update_time, expected_time);
    assert_eq!(completed.users[&0].first_install_time, expected_time);
    assert_eq!(scan.identities.ids, snapshot.identities.ids);
    eprintln!("native/original GSF file time and accepted metadata match: {expected_time}");
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
