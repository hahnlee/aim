//! Host JVM checks of original-policy bridge boundaries; no guest boot.
use std::{fs, process::Command};
mod common {
    pub mod java;
    pub mod runtime;
}

#[test]
#[ignore = "requires pinned JDK; run explicitly without a guest boot"]
fn audio_configuration_and_sync_adapter_visibility_use_retained_java() {
    use common::runtime::{Data, run};
    let data =
        Data(std::env::temp_dir().join(format!("aim-start-read-queries-{}", std::process::id())));
    fs::create_dir(&data.0).unwrap();
    let root = aim_paths::root();
    let production = root.join("java/device-services");
    let jdk = aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let stubs = data.0.join("stubs");
    let classes = data.0.join("classes");
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(common::java::sources(&production.join("stubs"))));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(&stubs)
        .args(common::java::sources(
            &root.join("crates/aim-services/tests/fixtures/start_reads"),
        ))
        .arg(production.join("src/dev/aim/server/Bridge.java"))
        .arg(production.join("src/dev/aim/server/AudioPolicyQueries.java"))
        .arg(production.join("src/dev/aim/server/SyncAdapterQueries.java")));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(format!("{}:{}", classes.display(), stubs.display()))
        .arg("dev.aim.server.ReadQueriesTest"));
}
