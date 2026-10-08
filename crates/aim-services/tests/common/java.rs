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

pub fn computer_aidl(dir: &Path) -> PathBuf {
    private_aidl(dir, "IPackageComputer")
}

pub fn resolver_identity_aidl(dir: &Path) -> PathBuf {
    private_aidl(dir, "IPackageResolverIdentity")
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

/// Compile the complete checked-in device service sources and generated own
/// interfaces for oracle compilation and its vendor implementation dex.
/// Original API stubs remain compile-only and are never returned as inputs.
pub fn production_classes(dir:&Path,jdk:&Path,stubs:&Path)->PathBuf {
    let input=aim_paths::root().join("java/device-services");
    let generated=dir.join("production-aidl");
    let classes=dir.join("production-classes");
    fs::create_dir_all(&generated).unwrap();fs::create_dir_all(&classes).unwrap();
    fn aidls(dir:&Path,files:&mut Vec<PathBuf>){
        for entry in fs::read_dir(dir).unwrap(){let path=entry.unwrap().path();
            if path.is_dir(){aidls(&path,files)}else if path.extension().is_some_and(|extension|extension=="aidl"){files.push(path);}}
    }
    let mut interfaces=Vec::new();aidls(&input.join("aidl"),&mut interfaces);interfaces.sort();
    super::runtime::run(std::process::Command::new(aim_paths::fetched().join("java/build-tools-36.0.0/android-16/aidl"))
        .args(["--lang=java","--min_sdk_version","36"])
        .arg(format!("-I{}",input.join("aidl").display())).arg("-o").arg(&generated).args(interfaces));
    let mut files=sources(&input.join("src"));files.extend(sources(&generated));files.sort();
    super::runtime::run(std::process::Command::new(jdk.join("bin/javac"))
        .args(["--release","17","-d"]).arg(&classes).arg("-classpath").arg(stubs).args(files));
    classes
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
