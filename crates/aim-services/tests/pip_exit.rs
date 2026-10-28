//! Host JVM checks of the shell's PiP exit WCT; no guest boot.
use std::{fs, process::Command};

mod common {
    pub mod java;
    pub mod runtime;
}

#[test]
#[ignore = "requires pinned JDK; run explicitly without a guest boot"]
fn pip_exit_restores_bounds_and_desktop_mode() {
    use common::runtime::{Data, run};
    let data = Data(std::env::temp_dir().join(format!("aim-pip-exit-{}", std::process::id())));
    fs::create_dir(&data.0).unwrap();
    let root = aim_paths::root();
    let jdk = aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let stubs = data.0.join("stubs");
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(common::java::sources(
            &root.join("java/device-services/stubs"),
        )));
    let production = common::java::production_classes(&data.0, &jdk, &stubs);
    let classes = data.0.join("tests");
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(format!("{}:{}", production.display(), stubs.display()))
        .args(common::java::sources(
            &root.join("crates/aim-services/tests/fixtures/pip_exit"),
        )));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(format!(
            "{}:{}:{}",
            classes.display(),
            production.display(),
            stubs.display()
        ))
        .arg("dev.aim.server.PipExitTest"));
}
