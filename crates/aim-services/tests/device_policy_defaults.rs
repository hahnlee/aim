//! Host JVM verification of the retained Java inputs; no guest boot.
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn sources(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(sources(&path));
        } else if path.extension().is_some_and(|e| e == "java") {
            files.push(path);
        }
    }
    files.sort();
    files
}

fn run(command: &mut Command) {
    let output = aim_storage::inherited::stdio_only(command)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{:?}: {}",
        command,
        String::from_utf8_lossy(&output.stderr)
    );
}

struct Data(PathBuf);
impl Drop for Data {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn defaults_use_original_helpers_only_after_the_system_uid_check() {
    let data =
        Data(std::env::temp_dir().join(format!("aim-policy-defaults-{}", std::process::id())));
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
        .args(sources(&production.join("stubs"))));
    let aidl = data.0.join("aidl");
    fs::create_dir(&aidl).unwrap();
    run(
        Command::new(aim_paths::fetched().join("java/build-tools-36.0.0/android-16/aidl"))
            .args(["--lang=java", "--min_sdk_version", "36"])
            .arg(format!("-I{}", production.join("aidl").display()))
            .arg(format!("-o{}", aidl.display()))
            .arg(production.join("aidl/dev/aim/server/IDevicePolicyDefaults.aidl")),
    );
    let api = aidl.join("dev/aim/server/IDevicePolicyDefaults.java");
    let fixtures = aim_paths::root().join("crates/aim-services/tests/fixtures");
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(&stubs)
        .arg(api)
        .arg(production.join("src/dev/aim/server/DevicePolicyDefaults.java"))
        .arg(production.join("src/dev/aim/server/Bridge.java")));
    // Compile the production Binder subclass against the complete API first;
    // the runtime doubles observe only helper calls and caller identity.
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(format!("{}:{}", classes.display(), stubs.display()))
        .arg(fixtures.join("safe_labels/Binder.java"))
        .args(sources(&fixtures.join("device_policy_defaults"))));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(format!("{}:{}", classes.display(), stubs.display()))
        .arg("dev.aim.server.DevicePolicyDefaultsTest"));
}
