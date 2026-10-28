//! Execute production cold Wi-Fi Java with Android transport/handler boundaries doubled.
use std::{fs, process::Command};
mod common {
    pub mod java;
    pub mod runtime;
}

#[test]
#[ignore = "requires pinned JDK; run explicitly without a guest boot"]
fn wifi_state_listener_registry_pair_and_empty_passpoint_query() {
    use common::runtime::{Data, run};
    let data = Data(std::env::temp_dir().join(format!("aim-wifi-bridge-{}", std::process::id())));
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
            &root.join("crates/aim-services/tests/fixtures/wifi"),
        ))
        .arg(production.join("src/dev/aim/server/Bridge.java"))
        .arg(production.join("src/dev/aim/server/WifiBridge.java")));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(format!("{}:{}", classes.display(), stubs.display()))
        .arg("dev.aim.server.WifiBridgeTest"));
}
