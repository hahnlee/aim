//! Run the production Java diagnostics registry and system-UID wrapper on a host JVM.
use std::{fs, path::Path, process::Command};
fn sources(dir: &Path) -> Vec<std::path::PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .flat_map(|entry| {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path)
            } else if path.extension().is_some_and(|e| e == "java") {
                vec![path]
            } else {
                vec![]
            }
        })
        .collect()
}
fn run(command: &mut Command) {
    let output = aim_storage::inherited::stdio_only(command)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
#[test]
fn original_diagnostics_policy_matches_permissions_requests_and_callback_lifetimes() {
    let dir = std::env::temp_dir().join(format!(
        "aim-connectivity-diagnostics-{}",
        std::process::id()
    ));
    fs::create_dir_all(&dir).unwrap();
    let jdk = aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let source = aim_paths::root().join("java/device-services");
    let stubs = dir.join("stubs");
    let classes = dir.join("classes");
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(sources(&source.join("stubs"))));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(&stubs)
        .args(sources(&aim_paths::root().join(
            "crates/aim-services/tests/fixtures/connectivity_diagnostics",
        )))
        .args(
            [
                "ConnectivityDiagnostics.java",
                "NetworkActivityBridge.java",
                "Bridge.java",
            ]
            .map(|name| source.join("src/dev/aim/server").join(name)),
        ));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(std::env::join_paths([&classes, &stubs]).unwrap())
        .arg("dev.aim.server.ConnectivityDiagnosticsTest"));
    fs::remove_dir_all(dir).unwrap();
}
