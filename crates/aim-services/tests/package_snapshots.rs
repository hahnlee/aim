//! Facade snapshot scopes using real original PackageSetting objects/interfaces.
use std::{
    fs,
    process::Command,
    time::Duration,
};
mod common {
    pub mod java;
    pub mod runtime;
}
use common::java::sources;
use common::runtime::{Boot, Data, run};
#[test]
#[ignore = "requires pinned image, aimctl, JDK and d8; run explicitly"]
fn facade_package_snapshots_use_original_interfaces_and_preserve_capture_scope() {
    let dir = std::env::temp_dir().join(format!("ps-{}", std::process::id()));
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
                .join("tests/fixtures/PackageSnapshotsOracle.java"),
        )
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/PackageUsageOracle.java"),
        )
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/SharedSeInfoOracle.java"),
        )
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/SharedProcessesOracle.java"),
        )
        .arg(
            aim_paths::root()
                .join("java/device-services/src/com/android/server/pm/SharedProcessFeed.java"),
        )
        .arg(aim_paths::root().join("java/device-services/src/dev/aim/server/NativePackageCapabilities.java"))
        .arg(
            aim_paths::root().join("java/device-services/src/dev/aim/server/PackageSnapshots.java"),
        ));
    let mut pending = vec![classes.clone()];
    let mut class_files = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "class") {
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
    let boot = Boot::new(aim_paths::root().join("target/release/aimctl"), data.0.join("g"));
    run(boot.start_command().args(["start", "--windows"]));
    boot.wait_ready(Duration::from_secs(300)).expect("package_snapshots oracle boot readiness");

    fs::copy(
        dex.join("classes.dex"),
        boot.data.join("data/local/tmp/package-snapshots.dex"),
    )
    .unwrap();
    let result = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/package-snapshots.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.PackageSnapshotsOracle",
    ]));
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "SNAPSHOTS scopes identity immutability version visibility uncommitted errors\n"
    );
    let directory = boot.data.join("data/local/tmp/usage-oracle");
    fs::create_dir(&directory).unwrap();
    let mut usage = aim_services::package::owner::usage::Usage::new(["a", "b"]);
    usage.apply(b"a +7\nb -4\nunknown invalid\n").unwrap();
    assert!(
        usage
            .apply(b"PACKAGE_USAGE__VERSION_1\na 9 10 invalid 4 5 6 7 8\n")
            .is_err()
    );
    usage.notify("a", 2, 44);
    // Seed the remaining slots to distinguish each usage reason in the original runtime.
    for reason in 3..8 {
        usage.notify("a", reason, i64::from(reason) + 1);
    }
    fs::write(directory.join("native.list"), usage.encode()).unwrap();
    fs::write(
        directory.join("legacy.list"),
        b"a +7\nb -4\nunknown invalid\n",
    )
    .unwrap();
    fs::write(
        directory.join("partial.list"),
        b"PACKAGE_USAGE__VERSION_1\na 9 10 invalid 4 5 6 7 8\n",
    )
    .unwrap();
    let result = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/package-snapshots.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.PackageUsageOracle",
        "/data/local/tmp/usage-oracle",
    ]));
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "USAGE native original versions reasons prefix historical\n"
    );
    let mut original = aim_services::package::owner::usage::Usage::new(["a", "b"]);
    original.read(&directory.join("usage.list")).unwrap();
    assert_eq!(original.times("a"), Some(&[9, 10, 44, 4, 5, 6, 7, 55]));
    assert_eq!(original.times("b"), Some(&[0; 8]));
    verify_shared_processes(&boot);
    let result = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/package-snapshots.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.SharedSeInfoOracle",
    ]));
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "SHARED_SEINFO first parsed boot runtime removal empty reboot\n"
    );
}

fn encode_processes(owner: &aim_services::package::owner::shared_processes::Processes) -> Vec<u8> {
    fn string(out: &mut Vec<u8>, value: Option<&str>) {
        out.extend_from_slice(&value.map_or(-1, |s| s.len() as i32).to_be_bytes());
        if let Some(value) = value {
            out.extend_from_slice(value.as_bytes());
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(&(owner.records().len() as i32).to_be_bytes());
    for process in owner.records() {
        string(&mut out, process.map_key.as_deref());
        string(&mut out, process.name.as_deref());
        out.extend_from_slice(&(process.app_class_names_by_package.len() as i32).to_be_bytes());
        for (name, value) in &process.app_class_names_by_package {
            string(&mut out, Some(name));
            string(&mut out, value.as_deref());
        }
        out.extend_from_slice(&(process.denied_permissions.len() as i32).to_be_bytes());
        for name in &process.denied_permissions {
            string(&mut out, Some(name));
        }
        for mode in [
            process.gwp_asan_mode,
            process.memtag_mode,
            process.native_heap_zero_initialized,
        ] {
            out.extend_from_slice(&mode.to_be_bytes());
        }
        out.push(u8::from(process.use_embedded_dex));
    }
    out
}

fn verify_shared_processes(boot: &Boot) {
    use aim_services::package::{
        owner::shared_processes::Processes,
        pkg::{AndroidPackage, Process},
    };
    let directory = boot.data.join("data/local/tmp/shared-processes");
    fs::create_dir(&directory).unwrap();
    let a = vec![
        Process {
            map_key: Some("worker".into()),
            name: Some("worker".into()),
            app_class_names_by_package: vec![
                ("BB".into(), Some("first".into())),
                ("Aa".into(), None),
            ],
            denied_permissions: vec!["BB".into(), "internet".into()],
            gwp_asan_mode: 1,
            memtag_mode: 2,
            native_heap_zero_initialized: 1,
            use_embedded_dex: true,
        },
        Process {
            map_key: Some("BB".into()),
            name: Some("BB".into()),
            ..Default::default()
        },
        Process {
            map_key: Some("Aa".into()),
            name: Some("Aa".into()),
            ..Default::default()
        },
        Process {
            map_key: None,
            name: Some("null-map".into()),
            ..Default::default()
        },
        Process {
            map_key: Some("".into()),
            name: Some("".into()),
            ..Default::default()
        },
    ];
    let b = vec![Process {
        map_key: Some("different".into()),
        name: Some("worker".into()),
        app_class_names_by_package: vec![
            ("BB".into(), None),
            ("new".into(), Some("second".into())),
        ],
        denied_permissions: vec!["Aa".into(), "BB".into()],
        gwp_asan_mode: -1,
        memtag_mode: -1,
        native_heap_zero_initialized: -1,
        use_embedded_dex: false,
    }];
    let c = vec![Process {
        map_key: Some("worker".into()),
        name: Some("worker".into()),
        app_class_names_by_package: vec![("new".into(), None)],
        denied_permissions: vec!["camera".into()],
        ..Default::default()
    }];
    let d = vec![Process {
        map_key: Some("worker".into()),
        name: Some("worker".into()),
        ..Default::default()
    }];
    let members = std::collections::BTreeMap::from([("a", a), ("b", b), ("c", c), ("d", d)]);
    for (name, processes) in &members {
        let package = AndroidPackage {
            package_name: (*name).into(),
            feature_flag_state: Some(Vec::new()),
            processes: Some(processes.clone()),
            ..Default::default()
        };
        fs::write(
            directory.join(format!("{name}.cache")),
            package.to_cache_entry().unwrap().bytes,
        )
        .unwrap();
    }
    let result = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/package-snapshots.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.SharedProcessesOracle",
        "/data/local/tmp/shared-processes",
    ]));
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        "SHARED_PROCESSES copy union overwrite modes embedded collisions rebuild removal empty\n"
    );
    let check = |phase: &str, owner: &Processes| {
        let aggregate = aim_services::package::scan::OriginalSharedProcesses::read_original_record(
            &fs::read(directory.join(format!("{phase}.aggregate"))).unwrap(),
        )
        .unwrap();
        assert_eq!(aggregate.name, "group");
        assert_eq!(
            aggregate.records,
            owner.records(),
            "actual original aggregate {phase}"
        );
        assert_eq!(
            encode_processes(owner),
            fs::read(directory.join(format!("{phase}.original"))).unwrap(),
            "process owner phase {phase}"
        );
    };
    let mut owner = Processes::default();
    check("empty", &owner);
    owner.add(Some(&members["a"])).unwrap();
    check("a", &owner);
    let retained = owner.clone();
    owner.add(Some(&members["b"])).unwrap();
    check("ab", &owner);
    owner.add(None).unwrap();
    owner.add(Some(&[])).unwrap();
    check("noop", &owner);
    owner.add(Some(&members["c"])).unwrap();
    check("abc", &owner);
    check("a", &retained);
    for phase in ["rebuilt", "removed"] {
        let bytes = fs::read(directory.join(format!("{phase}.order"))).unwrap();
        let mut input = bytes.as_slice();
        let integer = |input: &mut &[u8]| {
            let (head, rest) = input.split_at(4);
            *input = rest;
            i32::from_be_bytes(head.try_into().unwrap())
        };
        let count = integer(&mut input);
        assert!((0..=3).contains(&count));
        let mut order = Vec::new();
        for _ in 0..count {
            let length = integer(&mut input);
            assert!(length > 0);
            let (head, rest) = input.split_at(length as usize);
            input = rest;
            order.push(std::str::from_utf8(head).unwrap().to_owned());
        }
        assert!(input.is_empty());
        owner
            .rebuild(
                order
                    .iter()
                    .map(|name| Some(members[name.as_str()].as_slice())),
            )
            .unwrap();
        check(phase, &owner);
    }
    check("absent", &owner);
    owner.rebuild([]).unwrap();
    check("cleared", &owner);
    owner.add(Some(&members["d"])).unwrap();
    let retained = owner.clone();
    assert!(owner.add(Some(&members["a"])).is_err());
    assert_eq!(owner, retained);
}
