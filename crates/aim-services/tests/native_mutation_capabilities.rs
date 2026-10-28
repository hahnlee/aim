//! Production mutation capabilities on original ART, including Binder diagnostics.
use std::{fs, process::Command, time::Duration};
mod common {
    pub mod java;
    pub mod runtime;
}
use common::runtime::{Boot, Data, run};

#[test]
#[ignore = "requires pinned image, aimctl, JDK and d8; run explicitly"]
fn original_art_mutation_capabilities_have_no_blocking_warnings() {
    let dir = std::env::temp_dir().join(format!("mutation-capabilities-{}", std::process::id()));
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
        .args(common::java::sources(
            &aim_paths::root().join("java/device-services/stubs"),
        )));
    let production = common::java::production_classes(&data.0, &jdk, &stubs);
    let classpath = std::env::join_paths([stubs.as_path(), production.as_path()]).unwrap();
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(classpath)
        .arg(
            aim_paths::root()
                .join("crates/aim-services/tests/fixtures/NativeMutationCapabilitiesOracle.java"),
        ));
    // Only the fixture and current vendor implementation enter the guest dex.
    let mut pending = vec![classes, production];
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
    let system_jars = aim_android_image::classpath::jars(
        &common::runtime::cohort::original_image(),
        "systemserverclasspath.pb",
        aim_android_image::classpath::SYSTEMSERVERCLASSPATH,
    )
    .unwrap();
    let jars = system_jars.iter().map(String::as_str).collect::<Vec<_>>();
    common::java::check_linkage(&dex.join("classes.dex"), &jars).unwrap();
    let guest_classpath = format!(
        "-Djava.class.path=/data/local/tmp/mutation-capabilities.dex:{}",
        system_jars.join(":")
    );
    let boot = Boot::new(
        aim_paths::root().join("target/release/aimctl"),
        data.0.join("g"),
    );
    run(boot.start_command().args(["start", "--windows"]));
    boot.wait_ready(Duration::from_secs(300))
        .expect("mutation capabilities oracle boot readiness");
    fs::copy(
        dex.join("classes.dex"),
        boot.data.join("data/local/tmp/mutation-capabilities.dex"),
    )
    .unwrap();
    let token = format!("mutation-capabilities-{}", std::process::id());
    let result = boot
        .client(1000)
        .args([
            "/system/bin/app_process",
            &guest_classpath,
            "/system/bin",
            "dev.aim.server.NativeMutationCapabilitiesOracle",
            &token,
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "mutation capabilities oracle: {}\n{}\n{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    let stdout = String::from_utf8(result.stdout).unwrap();
    let pid: u32 = stdout
        .strip_prefix("NATIVE_MUTATION_CAPABILITIES reservation snapshot sequence scopes installed close pid=")
        .and_then(|tail| tail.strip_suffix('\n'))
        .expect("mutation capabilities oracle success record")
        .parse()
        .expect("oracle PID");
    assert!(pid > 0, "invalid oracle PID");
    // Pinned BinderProxy emits the diagnostic under Binder.TAG ("Binder").
    let result = run(boot.command().args([
        "logs",
        "-b",
        "all",
        "--pid",
        &pid.to_string(),
        "-s",
        "NativeMutationOracle:I",
        "Binder:W",
        "BinderProxy:W",
        "*:S",
    ]));
    let logs = String::from_utf8(result.stdout).unwrap();
    let begin = format!("BEGIN {token}");
    let end = format!("END {token}");
    let (_, interval) = logs.split_once(&begin).expect("oracle begin log marker");
    let (interval, _) = interval.split_once(&end).expect("oracle end log marker");
    assert!(
        !interval.lines().any(|line| line.contains("FLAG_ONEWAY")
            || line.to_ascii_lowercase().contains("outgoing transaction")),
        "mutation capability synchronous Binder warning:\n{interval}"
    );
}
