//! `angle`: ANGLE's Metal build (docs/gles-driver.md), the host side of the
//! guest GLES driver, in the checkout `_build/angle-source` at the pin of
//! `upstream/angle.lock`. The checkout is fetched (git, then depot_tools'
//! `gclient sync`) when missing and never changed when present: other
//! checkouts may share it. gn writes `out/AimRelease` with
//! `upstream/angle-args.gn`, and ninja builds libEGL and libGLESv2 there.
//!
//! This is the one node that runs the system `ninja` rather than n2
//! (docs/build.md, "n2"): n2 expands a rule's `${rspfile}` inside that
//! rule's `command` to nothing, which breaks gn's actions with response
//! files, and it does not read ninja's logs, so it would rebuild an
//! existing ANGLE tree from scratch.

use super::repo;
use crate::cargo::write_if_changed;
use crate::graph::{Action, Node};
use crate::lockfile::Lock;
use crate::log::Log;
use crate::tools::Tool;
use std::ffi::OsString;
use std::fs;
use std::path::Path;
use std::process::Command;

const LOCK: &str = "upstream/angle.lock";
const ARGS: &str = "upstream/angle-args.gn";
const DEPOT_TOOLS: &str = "https://chromium.googlesource.com/chromium/tools/depot_tools.git";

pub fn node() -> Node {
    let out = aim_paths::angle();
    Node {
        name: "angle".into(),
        deps: Vec::new(),
        inputs: vec![repo(LOCK), repo(ARGS)],
        outputs: vec![out.join("libEGL.dylib"), out.join("libGLESv2.dylib")],
        tools: vec![Tool::Python, Tool::Ninja],
        recipe: 1,
        action: Action::Angle,
        boot: true,
    }
}

fn git(log: &mut Log, dir: &Path, args: &[&str]) -> Result<String, String> {
    log.output(Command::new("git").arg("-C").arg(dir).args(args))
}

/// depot_tools first on PATH (gclient, and the tools gn's actions run).
fn path_with(depot_tools: &Path) -> OsString {
    let mut path = depot_tools.as_os_str().to_owned();
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    path
}

fn fetch(src: &Path, revision: &str, log: &mut Log) -> Result<(), String> {
    let depot_tools = aim_paths::fetched().join("depot_tools");
    if !depot_tools.exists() {
        log.run(
            Command::new("git")
                .args(["clone", "-q", DEPOT_TOOLS])
                .arg(&depot_tools),
        )?;
    }
    fs::create_dir_all(src).map_err(|e| e.to_string())?;
    git(log, src, &["init", "-q"])?;
    git(
        log,
        src,
        &[
            "remote",
            "add",
            "origin",
            "https://chromium.googlesource.com/angle/angle",
        ],
    )?;
    git(log, src, &["fetch", "-q", "--depth=1", "origin", revision])?;
    git(
        log,
        src,
        &[
            "-c",
            "advice.detachedHead=false",
            "checkout",
            "-q",
            "--detach",
            revision,
        ],
    )?;
    let path = path_with(&depot_tools);
    log.run(
        Command::new("python3")
            .arg("scripts/bootstrap.py")
            .current_dir(src)
            .env("PATH", &path),
    )?;
    log.run(
        Command::new(depot_tools.join("gclient"))
            .arg("sync")
            .current_dir(src)
            .env("PATH", &path),
    )
}

pub fn run(log: &mut Log) -> Result<(), String> {
    let lock = Lock::read(&repo(LOCK))?;
    let revision = lock.get("ANGLE_REVISION")?;
    let src = aim_paths::angle_source();
    if !src.exists() {
        fetch(&src, revision, log)?;
    }
    let head = git(log, &src, &["rev-parse", "HEAD"])?;
    if head.trim() != revision {
        return Err(format!(
            "{} is at {}, not the pin {revision} of {LOCK}",
            src.display(),
            head.trim()
        ));
    }
    let out = aim_paths::angle();
    let args = fs::read(repo(ARGS)).map_err(|e| e.to_string())?;
    let path = path_with(&aim_paths::fetched().join("depot_tools"));
    // An existing tree with these args regenerates itself (build.ninja's
    // own gn rule) when a .gn file changes.
    if fs::read(out.join("args.gn")).ok() != Some(args.clone()) || !out.join("build.ninja").exists()
    {
        write_if_changed(&out.join("args.gn"), &args, 0o644)?;
        log.run(
            Command::new(src.join("buildtools/mac/gn"))
                .arg("gen")
                .arg("out/AimRelease")
                .current_dir(&src)
                .env("PATH", &path),
        )?;
    }
    log.run(
        Command::new("ninja")
            .arg("-C")
            .arg(&out)
            .args(["libEGL", "libGLESv2"])
            .env("PATH", &path),
    )
}
