use super::*;

pub(crate) fn build_runtime_host(root: &Path) -> Result<()> {
    let status = Command::new("cargo")
        .args(["build", "-p", "darwin-art-host", "--bin", "darwin-art-host"])
        .current_dir(root)
        .status()?;
    if !status.success() {
        return Err("failed to build the Rust darwin-art-host launcher".into());
    }
    let executable = root.join("target/debug/darwin-art-host");
    if !executable.is_file() {
        return Err(format!("Rust runtime host is missing: {}", executable.display()).into());
    }
    Ok(())
}

pub(crate) fn probe_runtime_dex(root: &Path, show_window: bool) -> Result<()> {
    probe_runtime_dex_flavor(root, show_window, false, false, false, false, false)
}

pub(crate) fn probe_runtime_elf_jni(root: &Path) -> Result<()> {
    build_elf_jni_dex_probe(root)?;
    probe_runtime_dex_flavor(root, false, false, false, true, false, false)
}

pub(crate) fn probe_runtime_network(root: &Path) -> Result<()> {
    build_network_dex_probe(root)?;
    // The host is intentionally GPU-only. Network acceptance still drives
    // the same Activity/DecorView path as the other Android flavors, so use
    // the real graphics runtime rather than a CPU/minimal flavor that cannot
    // create an active Metal surface.
    probe_runtime_dex_flavor(root, false, true, false, false, true, false)
}
