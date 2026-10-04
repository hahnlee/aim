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
    pub mod seinfo;
}
use common::java::sources;
use common::runtime::{Boot, Data, run};

#[test]
#[ignore = "requires aimctl, the pinned derived image, JDK and d8; run explicitly"]
fn new_settings_match_the_original_runtime() {
    compare_new_settings(false);
}

#[test]
#[ignore = "requires aimctl, the pinned derived image, JDK and d8; run explicitly"]
fn original_setting_adoption_matches_the_original_runtime() {
    compare_new_settings(true);
}

fn compare_new_settings(original_only: bool) {
    let dir = std::env::temp_dir().join(format!(
        "aim-setting-{}-{}",
        original_only,
        std::process::id()
    ));
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
    fs::copy(
        dex.join("classes.dex"),
        boot.data.join("data/local/tmp/new-setting.dex"),
    )
    .unwrap();
    let adoption = boot
        .command()
        .args([
            "shell",
            "/system/bin/app_process",
            "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
            "/system/bin",
            "com.android.server.pm.NewSettingOracle",
            "original-setting",
        ])
        .output()
        .unwrap();
    if !adoption.status.success() {
        let logs = run(boot.command().args(["shell", "logcat", "-d", "-t", "200"]));
        panic!(
            "original adoption: {}\n{}\n{}",
            adoption.status,
            String::from_utf8_lossy(&adoption.stderr),
            String::from_utf8_lossy(&logs.stdout)
        );
    }
    assert_eq!(
        String::from_utf8(adoption.stdout).unwrap(),
        "original setting adoption contracts: 4 cases\n"
    );
    eprintln!(
        "original setting creation preserves source identity across four shared-UID arguments"
    );
    if original_only {
        return;
    }
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
                    assert_eq!(p.mime_groups, vec![(Some("mime".into()), Vec::new())]);
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
                        p.is_loading(),
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
                        mime_groups: vec![
                            (Some("keep".into()), vec![]),
                            (Some("remove".into()), vec![]),
                        ],
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
                    let mut names: Vec<_> = p
                        .mime_groups
                        .iter()
                        .map(|(n, _)| n.as_deref().unwrap())
                        .collect();
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
    let original_abis = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.NewSettingOracle",
        "shared-abis",
        "/data/local/tmp/code-time.cache",
    ]));
    let output = String::from_utf8(original_abis.stdout).unwrap();
    let mut original = output.lines();
    let mut cases = 0;
    for layout in [
        [None, None, None],
        [None, Some("armeabi-v7a"), Some("arm64-v8a")],
        [Some("armeabi-v7a"), Some("armeabi"), None],
        [Some("arm64-v8a"), None, Some("x86")],
    ] {
        for scan in 0..5 {
            for loaded in 0..3 {
                let header = original.next().unwrap();
                let order = header.strip_prefix(&format!("case {cases} ")).unwrap();
                // The original exposes its actual ArraySet membership order.
                let mut members: Vec<_> = order
                    .split(',')
                    .map(|name| {
                        let at = ["a", "b", "c"].iter().position(|n| *n == name).unwrap();
                        aim_services::package::settings::Package {
                            name: name.into(),
                            code_path: format!("/system/nonexistent/{name}"),
                            primary_cpu_abi: layout[at].map(str::to_owned),
                            secondary_cpu_abi: Some("x86_64".into()),
                            ..Default::default()
                        }
                    })
                    .collect();
                let mut scanned = (scan != 0).then(|| AndroidPackage {
                    package_name: if scan < 3 {
                        "new"
                    } else if scan == 3 {
                        "a"
                    } else {
                        "b"
                    }
                    .into(),
                    primary_cpu_abi: match scan {
                        2 => Some("arm64-v8a".into()),
                        4 => Some("armeabi-v7a".into()),
                        _ => None,
                    },
                    ..Default::default()
                });
                let parsed: std::collections::BTreeMap<_, _> = members
                    .iter()
                    .filter(|_| loaded != 0)
                    .map(|p| {
                        (
                            p.name.clone(),
                            AndroidPackage {
                                primary_cpu_abi: (loaded == 2).then(|| "arm64-v8a".into()),
                                ..Default::default()
                            },
                        )
                    })
                    .collect();
                let choice = SharedUserAbi::derive(&members, scanned.as_ref()).unwrap();
                assert_eq!(
                    original.next().unwrap(),
                    choice.primary.as_deref().unwrap_or("null"),
                    "{header}"
                );
                let changed = choice.apply(&mut members, &parsed, scanned.as_mut());
                assert_eq!(
                    original.next().unwrap(),
                    scanned
                        .as_ref()
                        .map(|p| p.primary_cpu_abi.as_deref().unwrap_or("null"))
                        .unwrap_or("absent"),
                    "{header}"
                );
                let settings = members
                    .iter()
                    .map(|p| {
                        format!(
                            "{}={};",
                            p.name,
                            p.primary_cpu_abi.as_deref().unwrap_or("null")
                        )
                    })
                    .collect::<String>();
                assert_eq!(original.next().unwrap(), settings, "{header}");
                let paths = if changed.is_empty() {
                    "null".into()
                } else {
                    format!("[{}]", changed.join(", "))
                };
                assert_eq!(original.next().unwrap(), paths, "{header}");
                let parsed = members
                    .iter()
                    .map(|p| {
                        format!(
                            "{}={};",
                            p.name,
                            parsed
                                .get(&p.name)
                                .map(|p| p.primary_cpu_abi.as_deref().unwrap_or("null"))
                                .unwrap_or("absent")
                        )
                    })
                    .collect::<String>();
                assert_eq!(original.next().unwrap(), parsed, "{header}");
                assert!(
                    members
                        .iter()
                        .all(|p| p.secondary_cpu_abi.as_deref() == Some("x86_64"))
                );
                cases += 1;
            }
        }
    }
    assert_eq!(cases, 60);
    assert_eq!(original.next(), None);
    eprintln!("native/original shared UID ABI selection and application match {cases} cases");
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
    let preferred_abi = properties
        .get("ro.product.cpu.abilist")
        .unwrap()
        .split(',')
        .next()
        .unwrap();
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
    let supported_abis = SupportedAbis {
        bit32: &bit32,
        bit64: &bit64,
    };
    let zip_original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.NewSettingOracle",
        "zip-abis",
    ]));
    let zip_output = String::from_utf8(zip_original.stdout).unwrap();
    let mut original = zip_output.lines().peekable();
    let data_root = boot.data.clone();
    let zip_apks = aim_services::package::write::Apks {
        platform: aim_services::package::parse::Platform::load(&root, Default::default()).unwrap(),
        files: Box::new(move |p| Some(data_root.join(p.trim_start_matches('/')))),
    };
    for case in 0..17 {
        let header = original.next().unwrap();
        let paths: Vec<_> = header
            .strip_prefix(&format!("case {case} "))
            .unwrap()
            .split(',')
            .collect();
        let pkg = AndroidPackage {
            base_apk_path: Some(paths[0].into()),
            split_code_paths: Some(paths[1..].iter().map(|p| Some((*p).into())).collect()),
            ..Default::default()
        };
        let inventory = zip_apks.zip_native_libraries(&pkg);
        if original.peek() == Some(&"error") {
            original.next();
            assert!(inventory.is_err(), "{header}: original rejected ZIP");
            continue;
        }
        let inventory = inventory.unwrap_or_else(|e| panic!("{header}: {e}"));
        for supported in [
            vec!["arm64-v8a", "x86"],
            vec!["x86", "arm64-v8a"],
            vec![],
            vec!["unknown"],
        ] {
            let supported: Vec<_> = supported.into_iter().map(str::to_owned).collect();
            let expected = match inventory.find_supported_abi(&supported) {
                SupportedAbi::None => -114,
                SupportedAbi::NoMatch => -113,
                SupportedAbi::Index(at) => at as i32,
            };
            assert_eq!(
                original.next().unwrap().parse::<i32>().unwrap(),
                expected,
                "{header}"
            );
        }
        assert_eq!(
            original.next().unwrap().parse::<bool>().unwrap(),
            inventory.renderscript_bitcode,
            "{header}"
        );
    }
    assert_eq!(original.next(), None);
    eprintln!("native/original ZIP ABI and RenderScript inventory match 17 archive cases");
    let page_size: u64 =
        String::from_utf8(run(boot.command().args(["shell", "getconf", "PAGESIZE"])).stdout)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
    let copied = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.NewSettingOracle",
        "native-copy",
    ]));
    let output = String::from_utf8(copied.stdout).unwrap();
    let mut original = output.lines();
    let zip_seconds: u64 = original
        .next()
        .unwrap()
        .strip_prefix("clock ")
        .unwrap()
        .parse()
        .unwrap();
    let mut cases = 0;
    for archive in 0..6 {
        for extract in [false, true] {
            for debuggable in [false, true] {
                for manifest_compat_disabled in [false, true] {
                    let line = original.next().unwrap();
                    let columns: Vec<_> = line.split_whitespace().collect();
                    assert_eq!(columns[0], "case");
                    assert_eq!(columns[1].parse::<usize>().unwrap(), cases);
                    let path = columns[2];
                    let policy = NativeLibraryInstallPolicy {
                        page_size,
                        extract,
                        debuggable,
                        manifest_compat_disabled,
                        compat_16kb_disabled: properties
                            .get("pm.16kb.app_compat.disabled")
                            .is_some_and(|v| {
                                matches!(v.as_str(), "1" | "true" | "y" | "yes" | "on")
                            }),
                    };
                    let package = AndroidPackage {
                        base_apk_path: Some(path.into()),
                        ..Default::default()
                    };
                    let plan = zip_apks.native_library_install_plan(&package, "arm64-v8a", policy);
                    assert_eq!(
                        plan.as_ref().map(|_| 1).unwrap_or_else(|e| e.code),
                        columns[3].parse::<i32>().unwrap(),
                        "{line}"
                    );
                    let source = android_image_extract::source::FileSource::open(
                        &(zip_apks.files)(path).unwrap(),
                    )
                    .unwrap();
                    let directory = boot
                        .data
                        .join(format!("data/local/tmp/native-copy-native-{cases}"));
                    fs::create_dir(&directory).unwrap();
                    let clock = |dos| {
                        assert_eq!(
                            dos,
                            (((2025 - 1980) << 9 | 1 << 5 | 2) << 16) | (3 << 11) | (4 << 5) | 3
                        );
                        // The fixture's clock converts DOS local time in the guest's timezone.
                        Ok(std::time::UNIX_EPOCH + Duration::from_secs(zip_seconds))
                    };
                    let owner = aim_storage::guest_inode::GuestInode {
                        uid: Some(0),
                        gid: Some(0),
                        mode: None,
                    };
                    let first = policy.copy_to(&source, "arm64-v8a", &directory, owner, &clock);
                    assert_eq!(
                        first.as_ref().map(|_| 1).unwrap_or_else(|e| e.code),
                        columns[3].parse::<i32>().unwrap(),
                        "{line}"
                    );
                    let second = policy.copy_to(&source, "arm64-v8a", &directory, owner, &clock);
                    assert_eq!(
                        second.as_ref().map(|_| 1).unwrap_or_else(|e| e.code),
                        columns[4].parse::<i32>().unwrap(),
                        "{line}"
                    );
                    let file = if archive == 4 { "wrap.sh" } else { "libx.so" };
                    let native = directory.join(file);
                    let exists = columns[5].parse::<bool>().unwrap();
                    assert_eq!(native.exists(), exists, "{line}");
                    if exists {
                        assert_eq!(first.unwrap().extracted.len(), 1, "{line}");
                        assert_eq!(second.unwrap().reused.len(), 1, "{line}");
                        assert!(
                            columns[8].parse::<bool>().unwrap(),
                            "{line}: original replaced matching file"
                        );
                        let expected = boot.data.join(format!(
                            "data/local/tmp/native-copy-original-{cases}/{file}"
                        ));
                        assert_eq!(
                            fs::read(&native).unwrap(),
                            fs::read(&expected).unwrap(),
                            "{line}"
                        );
                        assert_eq!(
                            fs::metadata(&native)
                                .unwrap()
                                .modified()
                                .unwrap()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap()
                                .as_millis(),
                            columns[6].parse::<u128>().unwrap(),
                            "{line}"
                        );
                        assert_eq!(
                            fs::metadata(&native).unwrap().len(),
                            columns[7].parse::<u64>().unwrap(),
                            "{line}"
                        );
                        use std::os::unix::fs::PermissionsExt;
                        assert_eq!(
                            fs::metadata(&native).unwrap().permissions().mode() & 0o777,
                            0o755
                        );
                    }
                    assert_eq!(
                        fs::read_dir(&directory).unwrap().count(),
                        usize::from(exists),
                        "{line}"
                    );
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 48);
    assert_eq!(original.next(), None);
    eprintln!(
        "native/original ZIP admission, extraction and reuse match {cases} cases at {page_size}-byte guest pages"
    );
    let alignment = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.NewSettingOracle",
        "native-alignment",
    ]));
    let output = String::from_utf8(alignment.stdout).unwrap();
    let mut cases = 0;
    for line in output.lines() {
        let columns: Vec<_> = line.split_whitespace().collect();
        assert_eq!(columns[0], "case");
        assert_eq!(columns[1].parse::<usize>().unwrap(), cases);
        let paths: Vec<_> = columns[2].split(',').collect();
        let package = AndroidPackage {
            base_apk_path: Some(paths[0].into()),
            split_code_paths: Some(paths[1..].iter().map(|p| Some((*p).into())).collect()),
            ..Default::default()
        };
        let native_paths = NativeLibraryPaths {
            root: columns[3].into(),
            requires_isa: columns[4].parse().unwrap(),
            primary: String::new(),
            secondary: None,
        };
        let policy = NativeLibraryInstallPolicy {
            page_size,
            extract: columns[5].parse().unwrap(),
            debuggable: true,
            compat_16kb_disabled: false,
            manifest_compat_disabled: false,
        };
        let flags = zip_apks.native_library_alignment(&package, &bit64, &native_paths, policy);
        assert_eq!(
            flags.map(|f| f as i32).unwrap_or(-1),
            columns[6].parse::<i32>().unwrap(),
            "{line}"
        );
        cases += 1;
    }
    assert_eq!(cases, 60);
    eprintln!(
        "native/original ZIP and ELF alignment flags match {cases} base/split and ISA-path cases"
    );
    let page_settings = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.NewSettingOracle",
        "page-size-setting",
    ]));
    let output = String::from_utf8(page_settings.stdout).unwrap();
    let mut original = output.lines();
    let mut cases = 0;
    for seed in [0, 6, 8, 16, 32, 64, 127] {
        for mode in -1..=128 {
            let mut setting = aim_services::package::settings::Package::default();
            setting.set_page_size_compat(seed).unwrap();
            let value = match setting.set_page_size_compat(mode) {
                Ok(()) => setting.page_size_compat.to_string(),
                Err(error) => {
                    assert_eq!(setting.page_size_compat, seed);
                    format!("error={error}")
                }
            };
            assert_eq!(original.next().unwrap(), format!("{seed} {mode} {value}"));
            cases += 1;
        }
    }
    assert_eq!(original.next(), None);
    eprintln!("native/original page-size setting setter matches {cases} seed/mode combinations");
    let abi_policy = AbiPolicy::from_platform(&apks.platform, &all_abis, &supported_abis, &|key| {
        properties.get(key).cloned()
    })
    .unwrap();
    let packages = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.NewSettingOracle",
        "native-package-copy",
    ]));
    fn native_tree(
        root: &std::path::Path,
    ) -> std::collections::BTreeMap<String, (Option<Vec<u8>>, u32, u128)> {
        fn visit(
            root: &std::path::Path,
            path: &std::path::Path,
            result: &mut std::collections::BTreeMap<String, (Option<Vec<u8>>, u32, u128)>,
        ) {
            use std::os::unix::fs::PermissionsExt;
            if !path.exists() {
                return;
            }
            let metadata = fs::metadata(path).unwrap();
            let mode = aim_storage::guest_inode::read(path)
                .unwrap()
                .and_then(|i| i.mode)
                .unwrap_or(metadata.permissions().mode())
                & 0o7777;
            let key = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned();
            let (bytes, modified) = if metadata.is_file() {
                (
                    Some(fs::read(path).unwrap()),
                    metadata
                        .modified()
                        .unwrap()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis(),
                )
            } else {
                (None, 0)
            };
            result.insert(key, (bytes, mode, modified));
            if metadata.is_dir() {
                for entry in fs::read_dir(path).unwrap() {
                    visit(root, &entry.unwrap().path(), result);
                }
            }
        }
        let mut result = std::collections::BTreeMap::new();
        visit(root, root, &mut result);
        result
    }
    let output = String::from_utf8(packages.stdout).unwrap();
    let mut cases = 0;
    for line in output.lines() {
        let columns: Vec<_> = line.split_whitespace().collect();
        assert_eq!(columns[0], "case");
        assert_eq!(columns[1].parse::<usize>().unwrap(), cases);
        let paths: Vec<_> = columns[2].split(',').collect();
        let multi: bool = columns[4].parse().unwrap();
        let override_abi = (columns[5] != "null").then_some(columns[5]);
        let package = AndroidPackage {
            base_apk_path: Some(paths[0].into()),
            split_code_paths: Some(paths[1..].iter().map(|p| Some((*p).into())).collect()),
            booleans: if multi {
                aim_services::package::pkg::booleans::MULTI_ARCH
            } else {
                0
            },
            ..Default::default()
        };
        let native = boot
            .data
            .join(format!("data/local/tmp/native-package-native-{cases}"));
        let clock = |_| Ok(std::time::UNIX_EPOCH + Duration::from_secs(zip_seconds));
        let restorecon = |path: &std::path::Path| {
            let guest = format!(
                "/{}",
                path.strip_prefix(&boot.data)
                    .map_err(|e| e.to_string())?
                    .display()
            );
            let restored = boot.command().args([
                "shell", "/system/bin/app_process",
                "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
                "/system/bin", "com.android.server.pm.NewSettingOracle", "restore-native-directory", &guest,
            ]).output().map_err(|e| e.to_string())?;
            if restored.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&restored.stderr).into_owned())
            }
        };
        let destination = NativeLibraryDestination {
            guest_root: &format!("/data/local/tmp/native-package-native-{cases}"),
            root: &native,
            owner: aim_storage::guest_inode::GuestInode {
                uid: Some(0),
                gid: Some(0),
                mode: None,
            },
            zip_time: &clock,
            restorecon: &restorecon,
        };
        let copied = zip_apks.copy_native_libraries_with_override(
            &package,
            &abi_policy,
            override_abi,
            NativeLibraryInstallPolicy {
                page_size,
                extract: true,
                debuggable: false,
                compat_16kb_disabled: false,
                manifest_compat_disabled: false,
            },
            &destination,
        );
        assert_eq!(
            copied.as_ref().map(|_| 1).unwrap_or_else(|e| e.code),
            columns[6].parse::<i32>().unwrap(),
            "{line}"
        );
        if let Ok(copied) = copied {
            assert_eq!(
                copied.ignored_override,
                multi && override_abi.is_some_and(|v| v != "-")
            );
        }
        let original = boot.data.join(columns[3].trim_start_matches('/'));
        assert_eq!(native_tree(&native), native_tree(&original), "{line}");
        cases += 1;
    }
    assert_eq!(cases, 64);
    eprintln!(
        "native/original split and multiarch package copy matches {cases} layout/override cases"
    );
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.NewSettingOracle",
        "abi-selection",
        "/data/local/tmp/code-time.cache",
    ]));
    let output = String::from_utf8(original.stdout).unwrap();
    let mut original = output.lines();
    let mut cases = 0;
    for archive in [0, 1, 2, 3, 9] {
        let path = format!("/data/local/tmp/abi-inventory-{archive}-0.zip");
        let mut pkg = AndroidPackage {
            base_apk_path: Some(path),
            ..Default::default()
        };
        let inventory = zip_apks.zip_native_libraries(&pkg).unwrap();
        for multi in [false, true] {
            for prefer32 in [false, true] {
                for sdk in [34, 35] {
                    for override_abi in [None, Some("x86"), Some("arm64-v8a")] {
                        for library in [false, true] {
                            pkg.booleans = if multi {
                                aim_services::package::pkg::booleans::MULTI_ARCH
                            } else {
                                0
                            } | if prefer32 {
                                aim_services::package::pkg::booleans::USE_32_BIT_ABI
                            } else {
                                0
                            };
                            pkg.target_sdk_version = sdk;
                            pkg.library_names = if library {
                                vec!["fixture.library".into()]
                            } else {
                                vec![]
                            };
                            let result = match PackageAbis::select(
                                &pkg,
                                &inventory,
                                &abi_policy,
                                override_abi,
                            ) {
                                Ok(selected) => format!(
                                    "{},{}",
                                    selected.primary.as_deref().unwrap_or("null"),
                                    selected.secondary.as_deref().unwrap_or("null")
                                ),
                                Err(error) => format!("error={},{}", error.code, error.message),
                            };
                            assert_eq!(
                                original.next().unwrap(),
                                format!("case {cases} {result}"),
                                "archive={archive}, multi={multi}, prefer32={prefer32}, sdk={sdk}, override={override_abi:?}, library={library}"
                            );
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 240);
    assert_eq!(original.next(), None);
    eprintln!("native/original PackageAbiHelper ABI policy matches {cases} cases");
    let result = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.NewSettingOracle",
        "abi-lifecycle",
        "/data/local/tmp/code-time.cache",
    ]));
    let result = String::from_utf8(result.stdout).unwrap();
    let mut original = result.lines();
    let saved = aim_services::package::settings::Package {
        name: "fixture".into(),
        primary_cpu_abi: Some("armeabi-v7a".into()),
        secondary_cpu_abi: Some("arm64-v8a".into()),
        ..Default::default()
    };
    let no_inventory = aim_services::package::write::Apks {
        files: Box::new(|_| None),
        platform: aim_services::package::parse::Platform::load(&root, Default::default()).unwrap(),
    };
    let mut cases = 0;
    for mode in 0..8 {
        for system in [false, true] {
            for updated in [false, true] {
                for requested in [None, Some("-"), Some("arm64-v8a")] {
                    let mut pkg = template.parsed.clone();
                    pkg.package_name = "fixture".into();
                    pkg.primary_cpu_abi = Some("x86".into());
                    pkg.secondary_cpu_abi = Some("x86_64".into());
                    let context = AbiScanContext {
                        mode: match mode {
                            0 | 7 => AbiScanMode::Existing {
                                first_boot_or_upgrade: false,
                                old_was_stub: false,
                                saved: Some(&saved),
                            },
                            1 => AbiScanMode::Existing {
                                first_boot_or_upgrade: true,
                                old_was_stub: false,
                                saved: Some(&saved),
                            },
                            2 => AbiScanMode::Existing {
                                first_boot_or_upgrade: false,
                                old_was_stub: true,
                                saved: Some(&saved),
                            },
                            3 => AbiScanMode::Existing {
                                first_boot_or_upgrade: false,
                                old_was_stub: false,
                                saved: None,
                            },
                            4 => AbiScanMode::Install {
                                moved: Some(&saved),
                            },
                            5 => AbiScanMode::Install { moved: None },
                            _ => AbiScanMode::Apex,
                        },
                        system,
                        updated,
                        override_abi: requested,
                        platform_runtime_64bit: (mode == 7).then_some(true),
                    };
                    let inputs = if (1..=3).contains(&mode) {
                        &apks
                    } else {
                        &no_inventory
                    };
                    if let Some(planned) = inputs
                        .scan_native_libraries(
                            &pkg,
                            &abi_policy,
                            &NativeLibraryEnvironment {
                                preferred_abi,
                                app_lib32_install_dir: "/data/app-lib",
                                code_is_directory: false,
                                canonical_source: None,
                            },
                            context,
                        )
                        .unwrap()
                    {
                        assert!(!planned.requires_extraction);
                        planned.apply_metadata(&mut pkg);
                    }
                    let mut setting = saved.clone();
                    context.apply_setting(&pkg, &mut setting).unwrap();
                    assert_eq!(original.next().unwrap(), format!("case {cases}"));
                    let fields = [
                        setting
                            .primary_cpu_abi
                            .as_deref()
                            .unwrap_or("null")
                            .to_owned(),
                        pkg.secondary_cpu_abi
                            .as_deref()
                            .unwrap_or("null")
                            .to_owned(),
                        pkg.native_library_root_dir
                            .as_deref()
                            .unwrap_or("null")
                            .to_owned(),
                        pkg.native_library_root_requires_isa.to_string(),
                        pkg.native_library_dir
                            .as_deref()
                            .unwrap_or("null")
                            .to_owned(),
                        pkg.secondary_native_library_dir
                            .as_deref()
                            .unwrap_or("null")
                            .to_owned(),
                        setting
                            .cpu_abi_override
                            .as_deref()
                            .unwrap_or("null")
                            .to_owned(),
                        setting
                            .legacy_native_library_path
                            .as_deref()
                            .unwrap_or("null")
                            .to_owned(),
                    ];
                    for field in fields {
                        assert_eq!(
                            original.next().unwrap(),
                            field,
                            "ABI lifecycle: mode={mode}, system={system}, updated={updated}, requested={requested:?}"
                        );
                    }
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 96);
    assert_eq!(original.next(), None);
    eprintln!("native ABI lifecycle matches original helper/setter routes in {cases} cases");
    let inventory = aim_paths::derived_image();
    let bundled_apks = aim_services::package::write::Apks {
        platform: aim_services::package::parse::Platform::load(&inventory, Default::default())
            .unwrap(),
        files: Box::new(move |p| Some(inventory.join(p.trim_start_matches('/')))),
    };
    let mut bundled_cases = 0;
    for (code_path, present) in [
        ("/system/app/PrintSpooler", true),
        ("/system/priv-app/BuiltInPrintService", true),
        ("/system/priv-app/DeviceAsWebcam", true),
        // The overlay removes this original app and its unpacked libraries.
        ("/system_ext/priv-app/MultiDisplayProvider", false),
        ("/product/app/Camera2", true),
    ] {
        let parsed = aim_services::package::parse::parse(
            &root.join(code_path.trim_start_matches('/')),
            code_path,
            aim_services::package::parse::PARSE_IS_SYSTEM_DIR,
            &apks.platform,
        )
        .unwrap()
        .to_cache_entry();
        let pkg = AndroidPackage::read_cache_entry(&parsed.bytes).unwrap();
        fs::write(
            boot.data.join("data/local/tmp/bundled-abi.cache"),
            parsed.bytes,
        )
        .unwrap();
        let actual = bundled_apks
            .bundled_abis(
                &pkg,
                &NativeLibraryEnvironment {
                    preferred_abi,
                    app_lib32_install_dir: "/data/app-lib",
                    code_is_directory: true,
                    canonical_source: None,
                },
                &supported_abis,
            )
            .unwrap();
        let original = run(boot.command().args([
            "shell",
            "/system/bin/app_process",
            "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
            "/system/bin",
            "com.android.server.pm.NewSettingOracle",
            "bundled-abis",
            "/data/local/tmp/bundled-abi.cache",
        ]));
        assert_eq!(
            String::from_utf8(original.stdout).unwrap(),
            format!(
                "{}\n{}\n",
                actual.primary.as_deref().unwrap_or("null"),
                actual.secondary.as_deref().unwrap_or("null")
            ),
            "bundled ABI: {code_path}"
        );
        assert_eq!(
            actual.primary.as_ref(),
            if present { bit64.first() } else { None },
            "derived 64-bit inventory: {code_path}"
        );
        assert_eq!(actual.secondary, None);
        assert!(!actual.multi_arch_mismatch);
        if present {
            let planned = bundled_apks
                .native_library_scan(
                    &pkg,
                    &abi_policy,
                    &NativeLibraryEnvironment {
                        preferred_abi,
                        app_lib32_install_dir: "/data/app-lib",
                        code_is_directory: true,
                        canonical_source: None,
                    },
                    true,
                    false,
                    None,
                )
                .unwrap();
            assert!(!planned.requires_extraction);
            assert!(!planned.multi_arch_mismatch);
            let original = run(boot.command().args([
                "shell",
                "/system/bin/app_process",
                "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
                "/system/bin",
                "com.android.server.pm.NewSettingOracle",
                "scan-abi-package",
                "/data/local/tmp/bundled-abi.cache",
            ]));
            assert_eq!(
                String::from_utf8(original.stdout).unwrap(),
                format!(
                    "{}\n{}\n{}\n{}\n{}\n{}\n",
                    planned.abis.primary.as_deref().unwrap_or("null"),
                    planned.abis.secondary.as_deref().unwrap_or("null"),
                    planned.paths.root,
                    planned.paths.requires_isa,
                    planned.paths.primary,
                    planned.paths.secondary.as_deref().unwrap_or("null")
                ),
                "combined ABI phase: {code_path}"
            );
            let mut parsed = pkg.clone();
            planned.apply_metadata(&mut parsed);
            assert_eq!(parsed.primary_cpu_abi, actual.primary);
            let mut vanished = pkg.clone();
            vanished.base_apk_path = Some("/system/nonexistent.apk".into());
            let before = vanished.clone();
            assert!(
                bundled_apks
                    .native_library_scan(
                        &vanished,
                        &abi_policy,
                        &NativeLibraryEnvironment {
                            preferred_abi,
                            app_lib32_install_dir: "/data/app-lib",
                            code_is_directory: true,
                            canonical_source: None
                        },
                        true,
                        false,
                        None
                    )
                    .is_err()
            );
            assert_eq!(vanished, before);
            let zip = bundled_apks.zip_native_libraries(&pkg).unwrap();
            let original = run(boot.command().args([
                "shell",
                "/system/bin/app_process",
                "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
                "/system/bin",
                "com.android.server.pm.NewSettingOracle",
                "zip-package",
                "/data/local/tmp/bundled-abi.cache",
                &all_abis.join(","),
            ]));
            let selected = match zip.find_supported_abi(&all_abis) {
                SupportedAbi::None => -114,
                SupportedAbi::NoMatch => -113,
                SupportedAbi::Index(at) => at as i32,
            };
            assert_eq!(
                String::from_utf8(original.stdout).unwrap(),
                format!("{selected}\n{}\n", zip.renderscript_bitcode),
                "APK ZIP inventory: {code_path}"
            );
        }
        bundled_cases += 1;
    }
    assert_eq!(bundled_cases, 5);
    eprintln!(
        "native/original bundled ABI inventory matches {bundled_cases} image apps (4 present, 1 removed)"
    );
    let mut path_cases = 0;
    for code in [
        "/system/app/Fixture",
        "/system/app/fixture.apk",
        "/apex/com.android.fixture/app/fixture.apk",
    ] {
        for primary in [None, Some("arm64-v8a"), Some("x86")] {
            for secondary in [None, Some("armeabi-v7a")] {
                for system in [false, true] {
                    for updated in [false, true] {
                        let mut pkg = template.parsed.clone();
                        pkg.path = Some(code.into());
                        pkg.base_apk_path = Some(if code.ends_with(".apk") {
                            code.into()
                        } else {
                            format!("{code}/base.apk")
                        });
                        pkg.primary_cpu_abi = primary.map(Into::into);
                        pkg.secondary_cpu_abi = secondary.map(Into::into);
                        let original = run(boot.command().args([
                            "shell", "/system/bin/app_process",
                            "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
                            "/system/bin", "com.android.server.pm.NewSettingOracle", "native-paths",
                            "/data/local/tmp/code-time.cache",
                            if system { "true" } else { "false" },
                            if updated { "true" } else { "false" },
                            code, pkg.base_apk_path.as_deref().unwrap(),
                            primary.unwrap_or("-"), secondary.unwrap_or("-"),
                        ]));
                        let paths = NativeLibraryPaths::derive(
                            &pkg,
                            &NativeLibraryEnvironment {
                                preferred_abi,
                                app_lib32_install_dir: "/data/app-lib",
                                code_is_directory: false,
                                canonical_source: None,
                            },
                            system,
                            updated,
                        )
                        .unwrap();
                        let expected = format!(
                            "{}\n{}\n{}\n{}\n",
                            paths.root,
                            paths.requires_isa,
                            paths.primary,
                            paths.secondary.as_deref().unwrap_or("null")
                        );
                        assert_eq!(
                            String::from_utf8(original.stdout).unwrap(),
                            expected,
                            "native library paths: {code}, {primary:?}, {secondary:?}, {system}, {updated}"
                        );
                        path_cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(path_cases, 72);
    eprintln!("native/original library paths match {path_cases} selected-ABI cases");
    let config = SystemConfig::read(&aim_paths::derived_image(), &|p| properties.get(p).cloned());
    let framework =
        aim_services::package::system_config::Framework::load(&aim_paths::derived_image()).unwrap();
    let app_system = aim_services::package::system_config::system(
        &aim_paths::derived_image(),
        &|p| properties.get(p).cloned(),
        &framework,
    )
    .unwrap();
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
    let abi_environment = NativeLibraryEnvironment {
        preferred_abi,
        app_lib32_install_dir: "/data/app-lib",
        code_is_directory: root.join(path.trim_start_matches('/')).is_dir(),
        canonical_source: None,
    };
    let abis = bundled_apks
        .bundled_abis(&code.parsed, &abi_environment, &supported_abis)
        .unwrap();
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.NewSettingOracle",
        "bundled-abis",
        "/data/local/tmp/code-time.cache",
    ]));
    assert_eq!(
        String::from_utf8(original.stdout).unwrap(),
        format!(
            "{}\n{}\n",
            abis.primary.as_deref().unwrap_or("null"),
            abis.secondary.as_deref().unwrap_or("null")
        )
    );
    assert!(!abis.multi_arch_mismatch);
    let selected = PackageAbis::select(
        &code.parsed,
        &bundled_apks.zip_native_libraries(&code.parsed).unwrap(),
        &abi_policy,
        None,
    )
    .unwrap();
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.NewSettingOracle",
        "abi-package",
        "/data/local/tmp/code-time.cache",
    ]));
    assert_eq!(
        String::from_utf8(original.stdout).unwrap(),
        format!(
            "{}\n{}\n",
            selected.primary.as_deref().unwrap_or("null"),
            selected.secondary.as_deref().unwrap_or("null")
        )
    );
    let native_libraries = bundled_apks
        .native_library_scan(
            &code.parsed,
            &abi_policy,
            &abi_environment,
            true,
            false,
            None,
        )
        .unwrap();
    assert_eq!(native_libraries.abis.primary, abis.primary);
    assert_eq!(native_libraries.abis.secondary, abis.secondary);
    assert!(!native_libraries.requires_extraction);
    native_libraries.apply_metadata(&mut code.parsed);
    eprintln!("native/original bundled GSF ABI inventory matches");
    let paths = NativeLibraryPaths::derive(
        &code.parsed,
        &NativeLibraryEnvironment {
            preferred_abi,
            app_lib32_install_dir: "/data/app-lib",
            code_is_directory: root.join(path.trim_start_matches('/')).is_dir(),
            canonical_source: None,
        },
        true,
        false,
    )
    .unwrap();
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.NewSettingOracle",
        "native-paths",
        "/data/local/tmp/code-time.cache",
        "true",
        "false",
        path,
        code.parsed.base_apk_path.as_deref().unwrap(),
        code.parsed.primary_cpu_abi.as_deref().unwrap_or("-"),
        code.parsed.secondary_cpu_abi.as_deref().unwrap_or("-"),
    ]));
    assert_eq!(
        String::from_utf8(original.stdout).unwrap(),
        format!(
            "{}\n{}\n{}\n{}\n",
            paths.root,
            paths.requires_isa,
            paths.primary,
            paths.secondary.as_deref().unwrap_or("null")
        )
    );
    paths.apply(&mut code.parsed);
    let (flags, private_flags) = application_flags(&code.parsed, false);
    let mut scan = SigningScan::new(&Default::default(), &Default::default(), 36).unwrap();
    let candidate = scan
        .apply_new_system(
            &code,
            SettingMetadata {
                code_path: path.clone(),
                legacy_native_library_path: code.parsed.native_library_root_dir.clone(),
                primary_cpu_abi: code.parsed.primary_cpu_abi.clone(),
                secondary_cpu_abi: code.parsed.secondary_cpu_abi.clone(),
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
    let duplicate = |candidate: &NewPackageOutcome| NewPackageOutcome {
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
    let no_inventory = aim_services::package::write::Apks {
        files: Box::new(|_| None),
        platform: aim_services::package::parse::Platform::load(&root, Default::default()).unwrap(),
    };
    // Isolate the accepted scan/copy seam with generated library ZIPs. This
    // does not represent signature verification or a complete APK install.
    let mut install_scan = scan.clone();
    let mut install_candidate = duplicate(&candidate);
    install_candidate.record.parsed.base_apk_path =
        Some("/data/local/tmp/native-package-copy-0-0.zip".into());
    install_candidate.record.parsed.split_code_paths = None;
    install_candidate.record.parsed.booleans |=
        aim_services::package::pkg::booleans::EXTRACT_NATIVE_LIBS;
    let install_context = AbiScanContext {
        mode: AbiScanMode::Existing {
            first_boot_or_upgrade: true,
            old_was_stub: false,
            saved: None,
        },
        system: true,
        updated: true,
        override_abi: Some("arm64-v8a"),
        platform_runtime_64bit: None,
    };
    let install_env = NativeLibraryEnvironment {
        app_lib32_install_dir: "/data/local/tmp/scan-native-install",
        code_is_directory: false,
        ..abi_environment
    };
    let staged = zip_apks
        .scan_native_libraries(
            &install_candidate.record.parsed,
            &abi_policy,
            &install_env,
            install_context,
        )
        .unwrap()
        .unwrap();
    assert!(staged.requires_extraction);
    let native_root = boot.data.join(staged.paths.root.trim_start_matches('/'));
    fs::create_dir_all(native_root.parent().unwrap()).unwrap();
    let clock = |_| Ok(std::time::UNIX_EPOCH + Duration::from_secs(zip_seconds));
    let restorecon = |path: &std::path::Path| {
        let guest = format!("/{}", path.strip_prefix(&boot.data).unwrap().display());
        let restored = boot
            .command()
            .args([
                "shell",
                "/system/bin/app_process",
                "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
                "/system/bin",
                "com.android.server.pm.NewSettingOracle",
                "restore-native-directory",
                &guest,
            ])
            .output()
            .map_err(|e| e.to_string())?;
        if restored.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&restored.stderr).into_owned())
        }
    };
    let mut destination = NativeLibraryDestination {
        guest_root: "/data/local/tmp/wrong-native-root",
        root: &native_root,
        owner: aim_storage::guest_inode::GuestInode {
            uid: Some(0),
            gid: Some(0),
            mode: None,
        },
        zip_time: &clock,
        restorecon: &restorecon,
    };
    let install_policy = NativeLibraryInstallPolicy {
        page_size,
        extract: false,
        debuggable: false,
        compat_16kb_disabled: false,
        manifest_compat_disabled: false,
    };
    let before_install = install_scan.clone();
    assert!(matches!(
        install_scan.finish_native_library_metadata(
            duplicate(&install_candidate),
            &zip_apks,
            &abi_policy,
            &install_env,
            install_context,
        ),
        Err(SigningError::Rejected(Error { message, .. })) if message == "native library extraction has not completed (#810)"
    ));
    assert!(matches!(
        install_scan.finish_native_library_install(
            duplicate(&install_candidate),
            &zip_apks,
            &abi_policy,
            &install_env,
            install_context,
            install_policy,
            &destination,
        ),
        Err(SigningError::Rejected(Error { message, .. })) if message == "writable native root disagrees with derived guest path"
    ));
    assert_eq!(install_scan, before_install);
    assert!(!native_root.exists());
    destination.guest_root = &staged.paths.root;
    let mut stale = duplicate(&install_candidate);
    stale.record.settings.version_code += 1;
    assert!(matches!(
        install_scan.finish_native_library_install(
            stale,
            &zip_apks,
            &abi_policy,
            &install_env,
            install_context,
            install_policy,
            &destination,
        ),
        Err(SigningError::Rejected(_))
    ));
    assert!(!native_root.exists());
    let corrupt_guest = "/data/local/tmp/scan-native-corrupt.zip";
    let corrupt_host = boot.data.join(corrupt_guest.trim_start_matches('/'));
    let source = (zip_apks.files)(
        install_candidate
            .record
            .parsed
            .base_apk_path
            .as_deref()
            .unwrap(),
    )
    .unwrap();
    let mut bytes = fs::read(source).unwrap();
    bytes[100] = 255;
    fs::write(&corrupt_host, bytes).unwrap();
    let mut corrupt = duplicate(&install_candidate);
    corrupt.record.parsed.base_apk_path = Some(corrupt_guest.into());
    assert!(matches!(
        install_scan.finish_native_library_install(
            corrupt,
            &zip_apks,
            &abi_policy,
            &install_env,
            install_context,
            install_policy,
            &destination,
        ),
        Err(SigningError::NativeLibrary {
            error: NativeLibraryError::Copy {
                code: -110,
                cause: NativeLibraryInstallError { code: -18, .. },
                ..
            },
            ..
        })
    ));
    assert_eq!(install_scan, before_install);
    assert!(
        native_tree(&native_root)
            .values()
            .all(|(bytes, _, _)| bytes.is_none())
    );
    let (installed, mismatch, copies) = install_scan
        .finish_native_library_install(
            install_candidate,
            &zip_apks,
            &abi_policy,
            &install_env,
            install_context,
            install_policy,
            &destination,
        )
        .unwrap();
    assert!(!mismatch);
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].abi, "arm64-v8a");
    assert_eq!(
        fs::read(
            boot.data
                .join(staged.paths.primary.trim_start_matches('/'))
                .join("libx.so")
        )
        .unwrap(),
        b"payload-0-0"
    );
    assert_eq!(
        installed.record.settings.primary_cpu_abi.as_deref(),
        Some("arm64-v8a")
    );
    assert_eq!(
        installed
            .record
            .settings
            .legacy_native_library_path
            .as_deref(),
        Some(staged.paths.root.as_str())
    );
    assert_eq!(
        installed.record.parsed.native_library_dir.as_deref(),
        Some(staged.paths.primary.as_str())
    );
    assert_ne!(install_scan, before_install);
    eprintln!(
        "accepted native scan commits only after extraction; stale/path/corrupt failures preserve settings"
    );
    let context = AbiScanContext {
        mode: AbiScanMode::Existing {
            first_boot_or_upgrade: true,
            old_was_stub: false,
            saved: None,
        },
        system: true,
        updated: false,
        override_abi: Some("arm64-v8a"),
        platform_runtime_64bit: None,
    };
    let unchanged = scan.clone();
    assert!(matches!(
        scan.finish_native_library_metadata(
            duplicate(&candidate),
            &no_inventory,
            &abi_policy,
            &abi_environment,
            context
        ),
        Err(SigningError::NativeLibrary { .. })
    ));
    assert_eq!(scan, unchanged);
    let (candidate, mismatch) = scan
        .finish_native_library_metadata(candidate, &apks, &abi_policy, &abi_environment, context)
        .unwrap();
    assert!(!mismatch);
    let page_policy = PageSizeCompatPolicy::from_platform(&apks.platform).unwrap();
    if page_policy.enabled && page_size == 16384 {
        // The generated compressed ELF fixture's original extracted alignment
        // result is 4. A caller's contradictory direct-map flags must not select
        // the APK path instead of the parsed package's extracted library.
        let mut alignment_candidate = duplicate(&installed);
        alignment_candidate.record.parsed.base_apk_path =
            Some("/data/local/tmp/native-alignment-38-0.zip".into());
        alignment_candidate.record.parsed.page_size_app_compat_flags = 0;
        alignment_candidate.record.parsed.native_library_root_dir =
            Some("/data/local/tmp/unaccepted-native-root".into());
        let library = native_root.join("lib0.so");
        let payload =
            fs::read(boot.data.join("data/local/tmp/native-alignment-38/lib0.so")).unwrap();
        fs::write(&library, &payload).unwrap();
        let before = alignment_candidate.record.settings.page_size_compat;
        let alignment_context = AbiScanContext {
            system: false,
            ..install_context
        };
        let (aligned, diagnostic) = install_scan
            .finish_page_size_metadata(
                alignment_candidate,
                &zip_apks,
                &page_policy,
                &bit64,
                install_policy,
                alignment_context,
            )
            .unwrap();
        assert_eq!(diagnostic, None);
        assert_eq!(aligned.record.settings.page_size_compat, before | 4);
        let mut direct = duplicate(&aligned);
        direct.record.parsed.booleans &= !aim_services::package::pkg::booleans::EXTRACT_NATIVE_LIBS;
        let unchanged = install_scan.clone();
        let (_, diagnostic) = install_scan
            .finish_page_size_metadata(
                direct,
                &zip_apks,
                &page_policy,
                &bit64,
                NativeLibraryInstallPolicy {
                    extract: true,
                    ..install_policy
                },
                alignment_context,
            )
            .unwrap();
        assert_eq!(
            diagnostic.as_deref(),
            Some("native library is compressed with extractNativeLibs=false")
        );
        assert_eq!(install_scan, unchanged);
        fs::remove_file(&library).unwrap();
        let (_, diagnostic) = install_scan
            .finish_page_size_metadata(
                duplicate(&aligned),
                &zip_apks,
                &page_policy,
                &bit64,
                install_policy,
                alignment_context,
            )
            .unwrap();
        assert!(diagnostic.unwrap().contains("No such file"));
        assert_eq!(install_scan, unchanged);
        // Copy succeeds, then the separate code-time mapping fails. The
        // completion transaction retains all accepted metadata; copied files
        // remain the installation cleanup owner's responsibility.
        let inputs = || ScanMetadataCompletion {
            seinfo: common::seinfo::scan(),
            factory_test: false,
            abi_policy: &abi_policy,
            native_environment: &install_env,
            context: install_context,
            install: install_policy,
            destination: Some(&destination),
            clock: ScanClock {
                current_time: 0,
                user_id: 0,
                update_time: false,
            },
        };
        assert!(
            matches!(install_scan.finish_scan_metadata(duplicate(&aligned), &zip_apks, inputs()),
            Err(SigningError::Rejected(ref e)) if e.phase == "code-time")
        );
        assert_eq!(install_scan, unchanged);
        assert_eq!(fs::read(&library).unwrap(), payload);
        let image = root.clone();
        let data_root = boot.data.clone();
        let mapped = aim_services::package::write::Apks {
            platform: aim_services::package::parse::Platform::load(&root, Default::default())
                .unwrap(),
            files: Box::new(move |path| {
                Some(if path.starts_with("/data/") {
                    data_root.join(path.trim_start_matches('/'))
                } else {
                    image.join(path.trim_start_matches('/'))
                })
            }),
        };
        let finished = install_scan
            .finish_scan_metadata(duplicate(&aligned), &mapped, inputs())
            .unwrap();
        assert_eq!(finished.copies.len(), 1);
        assert!(!finished.multi_arch_mismatch);
        assert_eq!(finished.alignment_diagnostic, None);
        assert_eq!(
            finished
                .candidate
                .record
                .settings
                .primary_cpu_abi
                .as_deref(),
            Some("arm64-v8a")
        );
        assert_eq!(
            finished.candidate.record.settings.last_modified_time,
            expected_time
        );
        assert_eq!(
            finished.candidate.users[&0].first_install_time,
            expected_time
        );
        eprintln!(
            "accepted scan binds extraction flags for alignment and stages copy/page/time completion with late-failure rollback"
        );
    }
    let install = NativeLibraryInstallPolicy {
        page_size,
        extract: false,
        debuggable: false,
        compat_16kb_disabled: false,
        manifest_compat_disabled: false,
    };
    if page_policy.enabled && page_size == 16384 {
        let unchanged = scan.clone();
        let mut no_manifest = duplicate(&candidate);
        no_manifest.record.parsed.page_size_app_compat_flags = 0;
        let (_, diagnostic) = scan
            .finish_page_size_metadata(
                no_manifest,
                &no_inventory,
                &page_policy,
                &bit64,
                install,
                AbiScanContext {
                    system: false,
                    ..context
                },
            )
            .unwrap();
        assert!(diagnostic.unwrap().contains("unmapped APK"));
        assert_eq!(scan, unchanged);
    }
    let mut candidate = candidate;
    let before_page = candidate.record.settings.page_size_compat;
    candidate.record.parsed.page_size_app_compat_flags = 32;
    let stale_page = duplicate(&candidate);
    let (candidate, diagnostic) = scan
        .finish_page_size_metadata(
            candidate,
            &no_inventory,
            &page_policy,
            &bit64,
            install,
            context,
        )
        .unwrap();
    assert_eq!(diagnostic, None);
    let expected_page = if page_policy.enabled && page_size == 16384 {
        before_page | 32
    } else {
        before_page
    };
    assert_eq!(candidate.record.settings.page_size_compat, expected_page);
    let mut invalid_page = duplicate(&candidate);
    invalid_page.record.parsed.page_size_app_compat_flags = 128;
    let unchanged = scan.clone();
    if page_policy.enabled && page_size == 16384 {
        assert!(matches!(
            scan.finish_page_size_metadata(
                invalid_page,
                &apks,
                &page_policy,
                &bit64,
                install,
                context
            ),
            Err(SigningError::NativeLibrary { .. })
        ));
        if expected_page != before_page {
            assert!(matches!(
                scan.finish_page_size_metadata(
                    stale_page,
                    &apks,
                    &page_policy,
                    &bit64,
                    install,
                    context
                ),
                Err(SigningError::Rejected(_))
            ));
        }
        assert_eq!(scan, unchanged);
    }
    assert_eq!(
        candidate.record.settings.primary_cpu_abi.as_deref(),
        Some("arm64-v8a")
    );
    assert_eq!(
        candidate.record.settings.cpu_abi_override.as_deref(),
        Some("arm64-v8a")
    );
    assert_eq!(
        candidate.record.settings.legacy_native_library_path,
        candidate.record.parsed.native_library_root_dir
    );
    assert_eq!(
        scan.settings
            .packages
            .iter()
            .find(|p| p.name == candidate.record.settings.name)
            .unwrap(),
        &candidate.record.settings
    );
    let mut stale = duplicate(&candidate);
    stale.record.settings.version_code += 1;
    let unchanged = scan.clone();
    assert!(matches!(
        scan.finish_native_library_metadata(stale, &apks, &abi_policy, &abi_environment, context),
        Err(SigningError::Rejected(_))
    ));
    assert_eq!(scan, unchanged);
    let failed_candidate = duplicate(&candidate);
    assert_eq!(
        candidate.record.settings.legacy_native_library_path,
        code.parsed.native_library_root_dir
    );
    assert_eq!(
        candidate.record.parsed.native_library_dir,
        code.parsed.native_library_dir
    );
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
    let combined_context = AbiScanContext {
        mode: AbiScanMode::Existing {
            first_boot_or_upgrade: false,
            old_was_stub: false,
            saved: Some(&completed.record.settings),
        },
        override_abi: None,
        ..context
    };
    let inputs = || ScanMetadataCompletion {
        seinfo: common::seinfo::scan(),
        factory_test: false,
        abi_policy: &abi_policy,
        native_environment: &abi_environment,
        context: combined_context,
        install,
        destination: None,
        clock,
    };
    let before = scan.clone();
    assert!(
        matches!(scan.finish_scan_metadata(duplicate(&completed), &unreadable, inputs()),
        Err(SigningError::Rejected(ref e)) if e.phase == "code-time")
    );
    assert_eq!(scan, before);
    let mut stale = duplicate(&completed);
    stale.record.settings.app_id += 1;
    assert!(matches!(
        scan.finish_scan_metadata(stale, &unreadable, inputs()),
        Err(SigningError::Rejected(_))
    ));
    assert_eq!(scan, before);
    let combined = scan
        .finish_scan_metadata(duplicate(&completed), &apks, inputs())
        .unwrap();
    assert!(combined.copies.is_empty());
    assert!(!combined.multi_arch_mismatch);
    assert_eq!(combined.alignment_diagnostic, None);
    assert_eq!(
        combined.candidate.record.settings.last_modified_time,
        expected_time
    );
    assert_eq!(combined.candidate.record.settings.cpu_abi_override, None);
    assert_eq!(combined.candidate.users, completed.users);
    assert_eq!(scan.identities.ids, before.identities.ids);
    eprintln!(
        "accepted scan reuse completion clears install-only override and preserves all stages on code-time failure"
    );
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/new-setting.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.NewSettingOracle",
        "final-flags",
        "/data/local/tmp/code-time.cache",
    ]));
    let mut expected_flags = String::new();
    for factory in [false, true] {
        for permission in [false, true] {
            for old_factory in [false, true] {
                for updated in [false, true] {
                    let mut owned = scan.clone();
                    let mut candidate = duplicate(&combined.candidate);
                    candidate.record.parsed = template.parsed.clone();
                    candidate
                        .record
                        .parsed
                        .requested_permissions
                        .retain(|p| p != "android.permission.FACTORY_TEST");
                    if permission {
                        candidate
                            .record
                            .parsed
                            .requested_permissions
                            .push("android.permission.FACTORY_TEST".into());
                    }
                    if old_factory {
                        candidate.record.parsed.booleans |=
                            aim_services::package::pkg::booleans::FACTORY_TEST;
                    } else {
                        candidate.record.parsed.booleans &=
                            !aim_services::package::pkg::booleans::FACTORY_TEST;
                    }
                    let stale = duplicate(&candidate);
                    let final_candidate = owned
                        .finish_application_metadata(candidate, factory, updated)
                        .unwrap();
                    use std::fmt::Write;
                    writeln!(
                        &mut expected_flags,
                        "{} {} {}",
                        final_candidate
                            .record
                            .parsed
                            .is(aim_services::package::pkg::booleans::FACTORY_TEST),
                        final_candidate.record.settings.flags,
                        final_candidate.record.settings.private_flags,
                    )
                    .unwrap();
                    if final_candidate.record.settings != stale.record.settings {
                        let unchanged = owned.clone();
                        assert!(matches!(
                            owned.finish_application_metadata(stale, factory, updated),
                            Err(SigningError::Rejected(_))
                        ));
                        assert_eq!(owned, unchanged);
                    }
                }
            }
        }
    }
    assert_eq!(String::from_utf8(original.stdout).unwrap(), expected_flags);
    eprintln!(
        "native/original final factory and ApplicationInfo flags match 16 cases; stale final candidates reject"
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
