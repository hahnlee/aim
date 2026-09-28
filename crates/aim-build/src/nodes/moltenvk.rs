//! `moltenvk`: the official MoltenVK release of `upstream/moltenvk.lock`
//! (docs/vulkan-driver.md), the host side of the guest Vulkan driver. The
//! release archive is downloaded into `_build/downloads` and checked against
//! its sha256; its macOS dylib and license go to `target/aim/moltenvk`
//! unchanged, beside the Khronos registry (`vk.xml`) of the Vulkan-Headers
//! revision the release is built with, which `tools/gen-vulkan-thunks.py`
//! reads.

use super::repo;
use crate::fetch;
use crate::graph::{Action, Node};
use crate::hash;
use crate::lockfile::Lock;
use crate::log::Log;
use std::fs;
use std::path::Path;
use std::process::Command;

const LOCK: &str = "upstream/moltenvk.lock";
/// The dylib and license inside the release archive.
const DYLIB: &str = "MoltenVK/MoltenVK/dynamic/dylib/macOS/libMoltenVK.dylib";
const LICENSE: &str = "MoltenVK/LICENSE";

pub fn node() -> Node {
    let out = aim_paths::moltenvk();
    Node {
        name: "moltenvk".into(),
        deps: Vec::new(),
        inputs: vec![repo(LOCK)],
        outputs: vec![
            out.join("libMoltenVK.dylib"),
            out.join("LICENSE"),
            out.join("vk.xml"),
        ],
        tools: Vec::new(),
        recipe: 1,
        action: Action::MoltenVk,
        boot: true,
    }
}

/// Downloads `url` into `_build/downloads/<name>` once and checks its sha256.
fn pinned(
    url: &str,
    name: &str,
    sha256: &str,
    log: &mut Log,
) -> Result<std::path::PathBuf, String> {
    let file = aim_paths::downloads().join(name);
    if !file.exists() {
        fetch::download(url, &file, log, |_, _| true)?;
    }
    let got = hash::sha256_file(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    if got != sha256 {
        return Err(format!(
            "{}: sha256 {got}, not the pin {sha256} of {LOCK}",
            file.display()
        ));
    }
    Ok(file)
}

fn copy(from: &Path, to: &Path) -> Result<(), String> {
    fs::copy(from, to)
        .map(drop)
        .map_err(|e| format!("{} -> {}: {e}", from.display(), to.display()))
}

pub fn run(log: &mut Log) -> Result<(), String> {
    let lock = Lock::read(&repo(LOCK))?;
    let version = lock.get("MOLTENVK_VERSION")?;
    let archive = pinned(
        lock.get("MOLTENVK_URL")?,
        &format!("MoltenVK-macos-{version}.tar"),
        lock.get("MOLTENVK_SHA256")?,
        log,
    )?;
    let registry = pinned(
        lock.get("VK_XML_URL")?,
        &format!("vk-{}.xml", lock.get("VULKAN_HEADERS_REVISION")?),
        lock.get("VK_XML_SHA256")?,
        log,
    )?;

    let out = aim_paths::moltenvk();
    let unpacked = out.join("unpacked");
    let _ = fs::remove_dir_all(&unpacked);
    fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
    log.run(
        Command::new("tar")
            .arg("-xf")
            .arg(&archive)
            .arg("-C")
            .arg(&unpacked)
            .args([DYLIB, LICENSE]),
    )?;
    // Replaced, not overwritten in place: a process may have the old one
    // mapped, and a signed binary changed in place is killed on next use.
    let dylib = out.join("libMoltenVK.dylib");
    let _ = fs::remove_file(&dylib);
    copy(&unpacked.join(DYLIB), &dylib)?;
    copy(&unpacked.join(LICENSE), &out.join("LICENSE"))?;
    copy(&registry, &out.join("vk.xml"))?;
    fs::remove_dir_all(&unpacked).map_err(|e| e.to_string())
}
