//! Host JVM policy/dispatch checks with Android runtime boundaries doubled.
use std::{fs, process::Command};

mod common {
    pub mod java;
    pub mod runtime;
}

#[test]
#[ignore = "requires pinned JDK and aidl; run explicitly without a guest boot"]
fn uri_groups_java_policy_and_paired_reads() {
    use common::runtime::{Data, run};
    let data = Data(std::env::temp_dir().join(format!("aim-uri-groups-{}", std::process::id())));
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
    // Compile every production class against generated AIDL, independently of
    // the executable Android doubles used for the policy test below.
    common::java::production_classes(&data.0, &jdk, &stubs);
    let fixture = aim_paths::root().join("crates/aim-services/tests/fixtures/uri_groups");
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(&stubs)
        .args(common::java::sources(&fixture))
        .arg(
            production.join(
                "src/com/android/server/pm/verify/domain/AppHostUriRelativeFilterGroups.java",
            ),
        ));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(format!("{}:{}", classes.display(), stubs.display()))
        .arg("UriGroupsTest"));
}
