//! Run the retained Java rule oracle offline; package boot has no Java dependency.
use std::{fs, process::Command};
mod common {
    pub mod runtime;
}

#[test]
fn update_restricted_allowlists_and_role_grants() {
    use common::runtime::{Data, run};
    let data =
        Data(std::env::temp_dir().join(format!("aim-permission-boot-{}", std::process::id())));
    fs::create_dir(&data.0).unwrap();
    let jdk = aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let fixtures = aim_paths::root().join("crates/aim-services/tests/fixtures");
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&data.0)
        .arg(fixtures.join("PermissionBootPolicy.java"))
        .arg(fixtures.join("PermissionBootPolicyTest.java")));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(&data.0)
        .arg("dev.aim.server.PermissionBootPolicyTest"));
}
