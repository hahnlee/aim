//! Inactive typed sync seam: delegation, account/user identity and failure propagation.
use std::{fs, process::Command};
mod common {
    pub mod java;
    pub mod runtime;
}

#[test]
#[ignore = "requires pinned JDK; run explicitly without a guest boot"]
fn sync_accounts_adapter() {
    use common::runtime::run;
    let work = aim_paths::scratch_dir("sync-accounts-java");
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
    common::java::sync_accounts_aidl(&work);
    run(Command::new(jdk.join("javac"))
        .arg("-d")
        .arg(&classes)
        .arg("-cp")
        .arg(&stubs)
        .args(common::java::sources(&work.join("aidl")))
        .arg(production.join("src/dev/aim/server/NativeAccountManagerInternal.java"))
        .arg(production.join("src/dev/aim/server/NativeSyncAccounts.java"))
        .arg(production.join("src/dev/aim/server/AccountConsentNotifications.java"))
        .args(common::java::sources(
            &root.join("crates/aim-services/tests/fixtures/sync_accounts"),
        )));
    run(Command::new(jdk.join("java"))
        .arg("-cp")
        .arg(format!("{}:{}", classes.display(), stubs.display()))
        .arg("dev.aim.server.SyncAccountsTest"));
    let declarations = work.join("JobSchedulerInternal.java");
    fs::write(
        &declarations,
        fs::read_to_string(
            production.join("stubs/com/android/server/job/JobSchedulerInternal.java"),
        )
        .unwrap()
        .replace("throw new RuntimeException(\"stub\");", ""),
    )
    .unwrap();
    run(Command::new(jdk.join("javac"))
        .arg("-d")
        .arg(&classes)
        .arg("-cp")
        .arg(&stubs)
        .arg(declarations)
        .arg(root.join(
            "crates/aim-services/tests/fixtures/sync_accounts/android/os/RemoteException.java",
        ))
        .arg(production.join("src/dev/aim/server/NativeJobScheduler.java"))
        .args(common::java::sources(
            &root.join("crates/aim-services/tests/fixtures/sync_jobs"),
        )));
    run(Command::new(jdk.join("java"))
        .arg("-cp")
        .arg(format!("{}:{}", classes.display(), stubs.display()))
        .arg("dev.aim.server.SyncJobsTest"));
    fs::remove_dir_all(work).unwrap();
}
