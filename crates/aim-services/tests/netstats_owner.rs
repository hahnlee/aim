//! Cold production Java netstats calls and session close on the host JVM.
use std::{fs, process::Command};

mod common {
    pub mod java;
    pub mod runtime;
}

#[test]
#[ignore = "requires the pinned JDK; run explicitly without a guest boot"]
fn usage_session_close_and_operation_counts_keep_original_checks() {
    use common::runtime::{Data, run};
    let data = Data(std::env::temp_dir().join(format!("aim-netstats-{}", std::process::id())));
    fs::create_dir(&data.0).unwrap();
    let root = aim_paths::root();
    let jdk = aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    assert!(
        jdk.join("bin/javac").is_file(),
        "NOT RUN: pinned JDK missing"
    );
    let stubs = data.0.join("stubs");
    let classes = data.0.join("classes");
    fs::create_dir(&stubs).unwrap();
    fs::create_dir(&classes).unwrap();
    let mut api = common::java::sources(&root.join("java/device-services/stubs"));
    api.extend(common::java::sources(
        &root.join("java/nearby-initializer/stubs"),
    ));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(api));
    let mut runtime = common::java::sources(&root.join("crates/aim-services/tests/api/netstats"));
    runtime.push(
        root.join("java/nearby-initializer/src/dev/aim/server/nearby/NetworkStatsInitializer.java"),
    );
    runtime.push(root.join("crates/aim-services/tests/fixtures/NetworkStatsOwnerTest.java"));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(&stubs)
        .args(runtime));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(format!("{}:{}", classes.display(), stubs.display()))
        .arg("dev.aim.server.nearby.NetworkStatsOwnerTest"));
}
