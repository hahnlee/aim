//! Host JVM checks of the original TextUtils boundary; no Android boot.
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
fn safe_label_bridge_uses_the_original_parameters_and_checks_the_system_uid() {
    let data = Data(std::env::temp_dir().join(format!("aim-safe-labels-{}", std::process::id())));
    fs::create_dir(&data.0).unwrap();
    let jdk = aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let stubs = data.0.join("stubs");
    let classes = data.0.join("classes");
    fs::create_dir(&stubs).unwrap();
    fs::create_dir(&classes).unwrap();
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(sources(
            &aim_paths::root().join("java/device-services/stubs"),
        )));
    let aidl_input = aim_paths::root().join("java/device-services/aidl");
    let aidl_output = data.0.join("aidl");
    fs::create_dir(&aidl_output).unwrap();
    run(
        Command::new(aim_paths::fetched().join("java/build-tools-36.0.0/android-16/aidl"))
            .args(["--lang=java", "--min_sdk_version", "36"])
            .arg(format!("-I{}", aidl_input.display()))
            .arg(format!("-o{}", aidl_output.display()))
            .arg(aidl_input.join("dev/aim/server/ISafeLabels.aidl")),
    );
    let api = aidl_output.join("dev/aim/server/ISafeLabels.java");
    let source = aim_paths::root().join("java/device-services/src/dev/aim/server");
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(&stubs)
        .arg(api)
        .arg(source.join("SafeLabels.java"))
        .arg(source.join("Bridge.java")));
    // Shadows observe only the boundary; HTML and glyph layout stay in framework.jar.
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(format!("{}:{}", classes.display(), stubs.display()))
        .args(sources(
            &aim_paths::root().join("crates/aim-services/tests/fixtures/safe_labels"),
        )));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(format!("{}:{}", classes.display(), stubs.display()))
        .arg("SafeLabelsTest"));
}
