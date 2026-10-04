//! Java fixture source discovery and linkage against the original image.
use std::fs;
use std::path::{Path, PathBuf};

pub fn sources(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(sources(&path));
        } else if path.extension().is_some_and(|e| e == "java") {
            files.push(path);
        }
    }
    files
}

/// Generate the private snapshot interface into this fixture's owned directory.
pub fn snapshot_aidl(dir: &Path) -> PathBuf {
    private_aidl(dir, "IPackageScanSnapshot")
}

pub fn bootstrap_aidl(dir: &Path) -> PathBuf {
    private_aidl(dir, "IPackageBootstrapBridge")
}

fn private_aidl(dir: &Path, interface: &str) -> PathBuf {
    let out = dir.join("aidl");
    fs::create_dir_all(&out).unwrap();
    let input = aim_paths::root().join("java/device-services/aidl");
    let output = std::process::Command::new(
        aim_paths::fetched().join("java/build-tools-36.0.0/android-16/aidl"),
    )
    .args(["--lang=java", "--min_sdk_version", "36"])
    .arg(format!("-I{}", input.display()))
    .arg(format!("-o{}", out.display()))
    .arg(input.join(format!("dev/aim/server/{interface}.aidl")))
    .output()
    .unwrap();
    assert!(
        output.status.success(),
        "{interface} AIDL: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    out.join(format!("dev/aim/server/{interface}.java"))
}

/// Check the boot classpath plus the client's explicit Java classpath before boot.
pub fn check_linkage(dex: &Path, classpath: &[&str]) -> Result<(), String> {
    use aim_android_image::classpath::{self, BOOTCLASSPATH};
    use aim_android_image::linkage::ClassPath;

    let image = aim_paths::derived_image();
    let mut jars = classpath::jars(&image, "bootclasspath.pb", BOOTCLASSPATH)?;
    jars.extend(classpath.iter().map(|jar| (*jar).to_owned()));
    let bytes = fs::read(dex).map_err(|e| format!("{}: {e}", dex.display()))?;
    let missing = ClassPath::read(&image, &jars)?.unresolved(&bytes)?;
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} does not link against the image:\n  {}",
            dex.display(),
            missing.join("\n  ")
        ))
    }
}
