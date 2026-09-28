//! Locked AOSP source and generated-source materialization.
//!
//! This module owns the filesystem/network boundary used by the bootstrap
//! command graph.  The public-in-module functions intentionally retain the
//! existing command API so callers do not need to know how sources are
//! materialized.

use std::fs;
use std::path::Path;
use std::process::Command;

use super::verify_sha256;
use crate::Result;
use crate::support::run_command;

pub(crate) fn materialize_file(
    root: &Path,
    project: &str,
    revision: &str,
    remote_file: &str,
    local_file: &str,
    expected_sha: &str,
) -> Result<()> {
    let destination = root.join(local_file);
    if destination.exists() {
        verify_sha256(&destination, expected_sha)?;
        println!(
            "sync: locked file already materialized at {}",
            destination.display()
        );
        return Ok(());
    }
    let downloads = root.join("_downloads");
    fs::create_dir_all(&downloads)?;
    let encoded = downloads.join(format!(
        "{}-{}.base64",
        remote_file.replace('/', "-"),
        revision
    ));
    let url = format!(
        "https://android.googlesource.com/{project}/+/{revision}/{remote_file}?format=TEXT"
    );
    run_command(
        Command::new("curl")
            .args(["-fL", "--retry", "3", "-o"])
            .arg(&encoded)
            .arg(url),
    )?;
    let decoded = Command::new("base64")
        .args(["-D", "-i"])
        .arg(&encoded)
        .output()?;
    if !decoded.status.success() {
        return Err(format!(
            "base64 decode failed for {remote_file}: {}",
            String::from_utf8_lossy(&decoded.stderr)
        )
        .into());
    }
    let parent = destination
        .parent()
        .ok_or_else(|| format!("file has no parent: {}", destination.display()))?;
    fs::create_dir_all(parent)?;
    fs::write(&destination, decoded.stdout)?;
    verify_sha256(&destination, expected_sha)?;
    fs::remove_file(&encoded)?;
    println!(
        "sync: materialized locked file at {}",
        destination.display()
    );
    Ok(())
}

#[path = "source/generated_operator.rs"]
mod generated_operator;
pub(crate) use generated_operator::generate_operator_source;

#[allow(clippy::too_many_arguments)]
pub(crate) fn materialize_archive(
    root: &Path,
    project: &str,
    revision: &str,
    archive_name: &str,
    remote_subtree: &str,
    local_subtree: &str,
    verification_file: &str,
    expected_source_sha: &str,
) -> Result<()> {
    let source_dir = root.join(local_subtree);
    let marker = source_dir.join(".source-revision");

    if marker.exists() && fs::read_to_string(&marker)?.trim() == revision {
        println!(
            "sync: source already materialized at {}",
            source_dir.display()
        );
        return Ok(());
    }
    if source_dir.exists() && !marker.exists() {
        // Older local gates materialized a few source subtrees before the
        // provenance marker became mandatory. Adopt only a tree whose pinned
        // verification file already matches; arbitrary or modified source is
        // still rejected by the checksum.
        verify_sha256(&source_dir.join(verification_file), expected_source_sha)?;
        fs::write(&marker, format!("{revision}\n"))?;
        println!(
            "sync: adopted checksum-matched locked source at {}",
            source_dir.display()
        );
        return Ok(());
    }
    if source_dir.exists() {
        return Err(format!(
            "{} exists but does not match sources.lock; move it aside explicitly",
            source_dir.display()
        )
        .into());
    }

    let downloads = root.join("_downloads");
    fs::create_dir_all(&downloads)?;
    let archive = downloads.join(format!("{archive_name}-{revision}.tar.gz"));
    let archive_suffix = if remote_subtree.is_empty() {
        format!("{revision}.tar.gz")
    } else {
        format!("{revision}/{remote_subtree}.tar.gz")
    };
    let url = format!("https://android.googlesource.com/{project}/+archive/{archive_suffix}");

    if !archive.exists() {
        let partial = downloads.join(format!("{archive_name}-{revision}.tar.gz.partial"));
        run_command(
            Command::new("curl")
                .args(["-fL", "--retry", "3", "--output"])
                .arg(&partial)
                .arg(&url),
        )?;
        fs::rename(partial, &archive)?;
    }

    let staging_dir = source_dir.with_extension("partial");
    if staging_dir.exists() {
        fs::remove_dir_all(&staging_dir)?;
    }
    fs::create_dir_all(&staging_dir)?;
    run_command(
        Command::new("tar")
            .arg("-xzf")
            .arg(&archive)
            .arg("-C")
            .arg(&staging_dir),
    )?;
    verify_sha256(&staging_dir.join(verification_file), expected_source_sha)?;
    fs::write(
        staging_dir.join(".source-revision"),
        format!("{revision}\n"),
    )?;
    fs::rename(&staging_dir, &source_dir)?;

    println!(
        "sync: materialized locked source without Git metadata at {}",
        source_dir.display()
    );
    Ok(())
}
