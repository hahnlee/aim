//! Original ART pure queries through retained native captured query leases.
use super::*;
#[allow(dead_code)]
#[path = "../../tests/common/java.rs"] mod java;
#[path = "../../tests/common/runtime.rs"] mod runtime;
use runtime::{Boot,Data,run};
use std::{fs,process::Command};
#[test]
#[ignore = "requires pinned image, built host/image, JDK and d8; run explicitly"]
fn original_art_pure_queries_match_full_captured_computer() {
    run_original_queries();
}
#[test]
#[ignore = "requires pinned image, JDK and d8; compile-only export, not an ART pass"]
fn compile_original_user_delta_oracle() {
    let dir = std::env::temp_dir().join(format!("aim-query-compile-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let data = Data(dir);
    let dex = compile_original_query_oracle(&data);
    let retained = aim_paths::root().join("target/aim/metadata-oracles");
    fs::create_dir_all(&retained).unwrap();
    fs::copy(dex, retained.join("user-delta-oracle.dex")).unwrap();
}
fn compile_original_query_oracle(data: &Data) -> std::path::PathBuf {
    let classes = data.0.join("classes");
    let stubs = data.0.join("stubs");
    let dex = data.0.join("dex");
    for path in [&classes, &stubs, &dex] {
        fs::create_dir(path).unwrap();
    }
    let jdk = aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(java::sources(
            &aim_paths::root().join("java/device-services/stubs"),
        )));
    let production = java::production_classes(&data.0, &jdk, &stubs);
    let classpath = std::env::join_paths([production.as_path(), stubs.as_path()]).unwrap();
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"]).arg(&classes).arg("-classpath").arg(&classpath)
        .args(["NativePackageQueryOracle.java", "PackageServiceQueryOracle.java", "PackageReceiverQueryOracle.java", "PackagePureQueryOracle.java", "PackageSnapshotUsageOracle.java", "PackageSnapshotUserDeltaOracle.java", "PackageSnapshotConcurrencyOracle.java"]
            .map(|name| aim_paths::root().join("crates/aim-services/tests/fixtures").join(name))));
    let mut programs = Vec::new();
    let mut directories = vec![classes.clone()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() { directories.push(path); }
            else if path.extension().is_some_and(|extension| extension == "class") { programs.push(path); }
        }
    }
    programs.sort();
    run(
        Command::new(aim_paths::fetched().join("java/build-tools-36.0.0/android-16/d8"))
            .args(["--min-api", "36", "--classpath"])
            .arg(&production).arg("--classpath").arg(&stubs)
            .arg("--output")
            .arg(&dex)
            .args(programs),
    );
    java::check_linkage(&dex.join("classes.dex"), &[
        "/system/framework/services.jar",
        "/system/framework/aim-services.jar",
        "/apex/com.android.permission/javalib/service-permission.jar",
    ]).unwrap();
    dex.join("classes.dex")
}
fn run_original_queries() {
    let dir = std::env::temp_dir().join(format!("aim-pure-query-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let data = Data(dir);
    let dex = compile_original_query_oracle(&data);
    let name = format!("dev.aim.test.pure.{}", std::process::id());
    let server = aim_binder_host::server::Server::start(&name).unwrap();
    let boot = Boot::new(aim_paths::root().join("target/release/aimctl"), data.0.join("guest"));
    run(boot.start_command().args(["start", "--windows"]));
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
            "disposable pure query boot did not complete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    let directory = boot.data.join("data/local/tmp/pure-query");
    fs::create_dir(&directory).unwrap();
    fs::copy(&dex, directory.join("oracle.dex")).unwrap();
    let oracle = |system: &Arc<System>, _bridge: &Arc<crate::package::bootstrap::Bridge>,
        _config: &SystemConfig, _store: &Arc<Mutex<crate::package::owner::Store>>| {
        let output = boot.client(1000).args(["--binder", &name, "/system/bin/app_process",
            "-Djava.class.path=/data/local/tmp/pure-query/oracle.dex:/system/framework/aim-services.jar:/system/framework/services.jar",
            "/system/bin", "dev.aim.server.NativePackageQueryOracle",
            "queries"]).output().unwrap();
        if !output.status.success() {
            let logs = boot.command().args(["shell", "logcat", "-d"]).output().unwrap();
            eprintln!("original query failure logcat: {}", String::from_utf8_lossy(&logs.stdout));
        }
        assert!(output.status.success(), "original pure-query oracle: stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8(output.stdout).unwrap().contains(
            "ORIGINAL_NATIVE_QUERY service receiver visibility application parity usage publication"));
        assert!(system.capture_package_queries().is_ok());
    };
    super::exercise_bootstrap_on(server.driver().clone(), true, false, Some(&oracle));
}
