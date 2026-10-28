//! Host JVM checks of the production Java custom permission; no Android boot.
use std::{fs, process::Command};

mod common {
    pub mod java;
    pub mod runtime;
}

#[test]
#[ignore = "requires pinned JDK and aidl; run explicitly without a guest boot"]
fn system_exempted_custom_permission_grants_denies_and_preserves_owner_errors() {
    use common::runtime::{Data, run};
    let data = Data(std::env::temp_dir().join(format!("aim-fgs-policy-{}", std::process::id())));
    fs::create_dir(&data.0).unwrap();
    let jdk = aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let stubs = data.0.join("stubs");
    let classes = data.0.join("classes");
    fs::create_dir(&stubs).unwrap();
    fs::create_dir(&classes).unwrap();
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(common::java::sources(
            &aim_paths::root().join("java/device-services/stubs"),
        )));
    let aidls = common::java::private_aidl_sources(&data.0);
    let host = aidls
        .iter()
        .find(|p| p.ends_with("IForegroundServiceTypeHost.java"))
        .unwrap();
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(&stubs)
        .arg(host)
        .arg(
            aim_paths::root()
                .join("java/device-services/src/dev/aim/server/ForegroundServiceExemptions.java"),
        )
        .arg(
            aim_paths::root()
                .join("crates/aim-services/tests/fixtures/ForegroundServicePolicyTest.java"),
        ));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(&classes)
        .arg("ForegroundServicePolicyTest"));
}
