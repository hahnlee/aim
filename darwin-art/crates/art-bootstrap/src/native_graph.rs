//! Native graph driver.
//!
//! This module owns only the xtask/Ninja handoff. The large bootstrap flavor
//! builder remains in `runtime_commands` until its archive phases are split;
//! changing graph policy therefore does not touch ART source/flag setup.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::SystemTime;

use crate::Result;
use crate::support::run_command;

pub(crate) fn build_native_graph(root: &Path, target: &str) -> Result<()> {
    if !matches!(
        target,
        "graphics-audit"
            | "graphics-bootstrap"
            | "runtime-bootstrap"
            | "headless-runtime-audit"
            | "graphics-foundation"
            | "foundation"
            | "runtime-support-dex"
            | "runtime-payload"
    ) {
        return Err(format!("unsupported native graph target: {target}").into());
    }
    let graph_dir = root.join("_build/native-graph");
    fs::create_dir_all(&graph_dir)?;
    let with_support = matches!(target, "runtime-support-dex" | "runtime-payload");
    let graph = graph_dir.join(if with_support {
        "support.ninja"
    } else {
        "build.ninja"
    });
    run_xtask_native_graph(root, &graph, with_support)?;
    let ninja = root.join("_aosp/external/skia/third_party/ninja/ninja");
    if !ninja.is_file() {
        return Err(format!("pinned Ninja is missing: {}", ninja.display()).into());
    }
    run_command(
        Command::new(&ninja)
            .arg("-f")
            .arg(&graph)
            .arg(target)
            .current_dir(root),
    )
}

fn run_xtask_native_graph(root: &Path, graph: &Path, with_support: bool) -> Result<()> {
    let binary = root.join("target/debug/darwin-art-xtask");
    if xtask_needs_rebuild(root, &binary)? {
        run_command(
            Command::new("cargo")
                .args(["build", "-q", "-p", "darwin-art-xtask"])
                .current_dir(root),
        )?;
    }
    if !binary.is_file() {
        return Err(format!("darwin-art-xtask binary is missing: {}", binary.display()).into());
    }
    let mut command = Command::new(&binary);
    command
        .args(["native-graph", "--out"])
        .arg(graph)
        .current_dir(root);
    if with_support {
        command.arg("--with-support");
    }
    run_command(&mut command)
}

fn xtask_needs_rebuild(root: &Path, binary: &Path) -> Result<bool> {
    let Ok(binary_mtime) = binary.metadata().and_then(|metadata| metadata.modified()) else {
        return Ok(true);
    };
    for path in [
        root.join("Cargo.toml"),
        root.join("Cargo.lock"),
        root.join("crates/darwin-art-xtask"),
        root.join("crates/darwin-art-build-contract"),
    ] {
        if newest_mtime(&path)?.is_some_and(|mtime| mtime > binary_mtime) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn newest_mtime(path: &Path) -> Result<Option<SystemTime>> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut newest = metadata.modified().ok();
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if let Some(mtime) = newest_mtime(&entry.path())?
                && newest.is_none_or(|current| mtime > current)
            {
                newest = Some(mtime);
            }
        }
    }
    Ok(newest)
}
