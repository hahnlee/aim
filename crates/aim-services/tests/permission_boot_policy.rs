//! Execute the production cold policy on a host JVM; no Android boot.
use std::{fs, process::Command};
mod common {
    pub mod java;
    pub mod runtime;
}

#[test]
fn update_restricted_allowlists_and_role_grants() {
    use common::runtime::{Data, run};
    let data =
        Data(std::env::temp_dir().join(format!("aim-permission-boot-{}", std::process::id())));
    fs::create_dir(&data.0).unwrap();
    let jdk = aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let production = aim_paths::root().join("java/device-services");
    let stubs = data.0.join("stubs");
    let classes = data.0.join("classes");
    fs::create_dir(&stubs).unwrap();
    fs::create_dir(&classes).unwrap();
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(common::java::sources(&production.join("stubs"))));
    let binder = data.0.join("Binder.java");
    fs::write(&binder, fs::read_to_string(production.join("stubs/android/os/Binder.java"))
        .unwrap().replace("public Binder() { throw new RuntimeException(\"stub\"); }", "public Binder() {}")
        .replace("public void attachInterface(IInterface owner, String descriptor) { throw new RuntimeException(\"stub\"); }",
            "public void attachInterface(IInterface owner, String descriptor) {}" )).unwrap();
    let bridge = data.0.join("Bridge.java");
    fs::write(&bridge, "package dev.aim.server; final class Bridge { static int uid = 1000; static void enforceSystemUid() { if (uid != 1000) throw new SecurityException(); } }").unwrap();
    let aidl = common::java::private_aidl_sources(&data.0)
        .into_iter()
        .find(|p| p.ends_with("IPermissionBootPolicy.java"))
        .unwrap();
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(&stubs)
        .args([
            binder,
            bridge,
            aidl,
            production.join("src/dev/aim/server/PermissionBootPolicy.java"),
            aim_paths::root()
                .join("crates/aim-services/tests/fixtures/PermissionBootPolicyTest.java"),
        ]));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(format!("{}:{}", classes.display(), stubs.display()))
        .arg("dev.aim.server.PermissionBootPolicyTest"));
    let startup =
        fs::read_to_string(production.join("src/dev/aim/server/SystemProcessHost.java")).unwrap();
    assert!(
        startup.find("host.attachPermissionBootPolicy(").unwrap()
            < startup.find("ActivityThread.systemMain()").unwrap()
    );
}
