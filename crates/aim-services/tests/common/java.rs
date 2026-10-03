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
