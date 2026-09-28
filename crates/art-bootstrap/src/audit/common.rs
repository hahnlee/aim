use super::*;

pub(super) fn build_runtime_native_owner(root: &Path) -> Result<PathBuf> {
    let manifest = root.join("crates/aim-runtime/Cargo.toml");
    let status = Command::new("cargo")
        .args(["build", "--manifest-path"])
        .arg(&manifest)
        .args(["--release", "-q"])
        .status()?;
    if !status.success() {
        return Err("failed to build Rust native runtime owner static library".into());
    }
    let archive = root.join("target/release/libaim_runtime.a");
    require_file(&archive, "Rust native runtime owner archive is missing")?;
    Ok(archive)
}

pub(super) fn require_file(path: &Path, description: &str) -> Result<()> {
    if path.is_file() {
        return Ok(());
    }
    Err(format!("{description}: {}", path.display()).into())
}
