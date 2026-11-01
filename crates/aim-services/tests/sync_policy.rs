//! Local sync policy adapters over typed native owners; no guest boot.
use std::process::Command;
mod common {
    pub mod java;
    pub mod runtime;
}
#[test]
#[ignore = "requires pinned JDK; run explicitly without a guest boot"]
fn sync_policy_adapters() {
    use common::runtime::run;
    let work = aim_paths::scratch_dir("sync-policy-java");
    let root = aim_paths::root();
    let production = root.join("java/device-services");
    let jdk =
        aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home/bin");
    let stubs = work.join("stubs");
    let classes = work.join("classes");
    run(Command::new(jdk.join("javac"))
        .arg("-d")
        .arg(&stubs)
        .args(common::java::sources(&production.join("stubs"))));
    run(Command::new(jdk.join("javac"))
        .arg("-d")
        .arg(&classes)
        .arg("-cp")
        .arg(&stubs)
        .arg(root.join(
            "crates/aim-services/tests/fixtures/sync_accounts/android/os/RemoteException.java",
        ))
        .arg(production.join("src/dev/aim/server/SyncPolicyLocals.java"))
        .args(common::java::sources(
            &root.join("crates/aim-services/tests/fixtures/sync_policy"),
        )));
    run(Command::new(jdk.join("java"))
        .arg("-cp")
        .arg(format!("{}:{}", classes.display(), stubs.display()))
        .arg("dev.aim.server.SyncPolicyTest"));
    std::fs::remove_dir_all(work).unwrap();
}
