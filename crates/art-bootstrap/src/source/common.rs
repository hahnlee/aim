use super::*;
use sha2::{Digest, Sha256};
use std::time::UNIX_EPOCH;

pub(crate) fn workspace_root() -> Result<PathBuf> {
    Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?)
}

pub(crate) fn build_shell_gate(root: &Path, script: &str) -> Result<()> {
    build_shell_gate_with_args(root, script, &[])
}

pub(crate) fn build_shell_gate_with_args(root: &Path, script: &str, args: &[&str]) -> Result<()> {
    let script = root.join("tools").join(script);
    if !script.is_file() {
        return Err(format!("build gate script is missing: {}", script.display()).into());
    }
    run_command(
        Command::new("bash")
            .arg(script)
            .args(args)
            .current_dir(root),
    )
}

/// Run a source-pinned build script only when its declared inputs or outputs
/// changed. This is deliberately separate from `build_shell_gate`: the full
/// acceptance path must still execute every upstream audit, while the
/// incremental graphics path may reuse a previously successful foundation
/// build and then run the closure/link/symbol checks again.
pub(crate) fn build_shell_gate_cached(
    root: &Path,
    script: &str,
    args: &[&str],
    inputs: &[PathBuf],
    outputs: &[PathBuf],
) -> Result<()> {
    let script_path = root.join("tools").join(script);
    if !script_path.is_file() {
        return Err(format!("build gate script is missing: {}", script_path.display()).into());
    }
    let stamp_dir = root.join("_build/gate-cache");
    fs::create_dir_all(&stamp_dir)?;
    let stamp_name = script.replace(['/', '\\'], "_");
    let stamp = stamp_dir.join(format!("{stamp_name}.stamp"));
    let mut digest = Sha256::new();
    digest.update(b"aim-cached-gate-v1\0");
    digest.update(script.as_bytes());
    for arg in args {
        digest.update([0]);
        digest.update(arg.as_bytes());
    }
    let mut all_inputs = Vec::with_capacity(inputs.len() + 1);
    all_inputs.push(script_path.clone());
    all_inputs.extend(inputs.iter().cloned());
    all_inputs.sort();
    all_inputs.dedup();
    let mut complete = true;
    for path in all_inputs {
        if !path.is_file() {
            complete = false;
            continue;
        }
        digest.update([0]);
        digest.update(path.to_string_lossy().as_bytes());
        let metadata = fs::metadata(&path)?;
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_nanos() as u64);
        // Lock/scripts are small and content changes must invalidate even if
        // a checkout preserves their timestamp. Static archives are treated
        // as immutable products of their source-pinned gates; use size only
        // so copying an unchanged archive does not invalidate every consumer.
        let is_large_archive = matches!(
            path.extension().and_then(|ext| ext.to_str()),
            Some("a" | "o" | "dylib" | "bundle")
        );
        digest.update(metadata.len().to_le_bytes());
        if is_large_archive {
            continue;
        } else {
            digest.update(modified.to_le_bytes());
            digest.update(fs::read(&path)?);
        }
    }
    let identity = format!("{:x}", digest.finalize());
    let outputs_ready = outputs.iter().all(|path| path.is_file());
    if complete
        && outputs_ready
        && fs::read_to_string(&stamp).is_ok_and(|value| value.trim() == identity)
    {
        println!("{script}: cached");
        return Ok(());
    }

    run_command(
        Command::new("bash")
            .arg(&script_path)
            .args(args)
            .current_dir(root),
    )?;
    if !outputs.iter().all(|path| path.is_file()) {
        return Err(
            format!("cached build gate completed without all declared outputs: {script}").into(),
        );
    }
    let temporary = stamp.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, identity)?;
    fs::rename(temporary, stamp)?;
    Ok(())
}

pub(crate) fn doctor() -> Result<()> {
    if env::consts::OS != "macos" || env::consts::ARCH != "aarch64" {
        return Err(format!(
            "requires arm64 macOS, found {}-{}",
            env::consts::ARCH,
            env::consts::OS
        )
        .into());
    }

    #[cfg(target_os = "macos")]
    let page_size = unsafe { getpagesize() };

    #[cfg(not(target_os = "macos"))]
    let page_size = 0;

    if page_size != 16 * 1024 {
        return Err(format!("expected a 16 KiB page size, found {page_size}").into());
    }
    for dependency in [
        "/opt/homebrew/opt/icu4c@78/include/unicode/ucnv.h",
        "/opt/homebrew/opt/icu4c@78/lib/libicui18n.dylib",
        "/opt/homebrew/opt/icu4c@78/lib/libicuuc.dylib",
    ] {
        if !Path::new(dependency).is_file() {
            return Err(format!("Homebrew icu4c@78 dependency is missing: {dependency}").into());
        }
    }

    let clang = command_output(Command::new("clang").arg("--version"))?;
    let first_line = clang.lines().next().unwrap_or("unknown clang");
    println!("doctor: arm64 macOS, page_size={page_size}, {first_line}");
    Ok(())
}
