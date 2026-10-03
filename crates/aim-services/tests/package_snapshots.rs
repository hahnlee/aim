//! Facade snapshot scopes using real original PackageSetting objects/interfaces.
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
fn facade_package_snapshots_use_original_interfaces_and_preserve_capture_scope() {
    let dir = std::env::temp_dir().join(format!("aim-pm-scopes-{}", std::process::id()));
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
