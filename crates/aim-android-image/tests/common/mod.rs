//! The pinned Java toolchain, to compile and dex fixtures.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The pinned Java toolchain of the `device-services` node
/// (upstream/java-toolchain.lock): the JDK's home and the build tools.
pub fn java_toolchain() -> Option<(PathBuf, PathBuf)> {
    let lock =
        std::fs::read_to_string(aim_paths::root().join("upstream/java-toolchain.lock")).ok()?;
    let get = |key: &str| {
        lock.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .unwrap()
    };
    let java = aim_paths::fetched().join("java");
    let jdk = java
        .join(format!("temurin-{}", get("JDK_VERSION")))
        .join(format!("jdk-{}/Contents/Home", get("JDK_VERSION")));
    let tools = java
        .join(format!("build-tools-{}", get("BUILD_TOOLS_VERSION")))
        .join(get("BUILD_TOOLS_DIR"));
    if !jdk.join("bin/javac").exists() || !tools.join("dexdump").exists() {
        aim_paths::skip("no pinned Java toolchain (cargo aim build device-services)");
        return None;
    }
    Some((jdk, tools))
}

/// The dex of `sources`, compiled and dexed in `work`.
pub fn dex(jdk: &Path, tools: &Path, sources: &[PathBuf], work: &Path) -> Vec<u8> {
    let classes = work.join("classes");
    let ok = Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .args(sources)
        .status()
        .unwrap();
    assert!(ok.success());
    let mut class_files = Vec::new();
    let mut pending = vec![classes];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                class_files.push(path);
            }
        }
    }
    let ok = Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(tools.join("lib/d8.jar"))
        .args([
            "com.android.tools.r8.D8",
            "--release",
            "--min-api",
            "36",
            "--lib",
        ])
        .arg(jdk)
        .arg("--output")
        .arg(work)
        .args(class_files)
        .status()
        .unwrap();
    assert!(ok.success());
    std::fs::read(work.join("classes.dex")).unwrap()
}
