//! `xsdc`: AOSP's XSD code generator at the revision of
//! `upstream/android16-xsdc.lock`, which the ART build runs for
//! ApexInfoList.xsd. The checkout is `_build/xsdc`; the jar and its one
//! dependency go to `target/aim/xsdc`.

use super::repo;
use crate::fetch;
use crate::graph::{Action, Node};
use crate::hash;
use crate::lockfile::Lock;
use crate::log::Log;
use crate::tools::{self, Tool};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

const LOCK: &str = "upstream/android16-xsdc.lock";
const COMMONS_CLI: &str = "commons-cli-1.2.jar";

pub fn out() -> PathBuf {
    aim_paths::out().join("xsdc")
}

pub fn node() -> Node {
    Node {
        name: "xsdc".into(),
        deps: Vec::new(),
        inputs: vec![repo(LOCK)],
        outputs: vec![out().join("xsdc.jar"), out().join(COMMONS_CLI)],
        tools: vec![Tool::Java],
        recipe: 1,
        action: Action::Xsdc,
        boot: true,
    }
}

pub fn run(log: &mut Log) -> Result<(), String> {
    let lock = Lock::read(&repo(LOCK))?;
    let revision = lock.get("XSDC_REVISION")?;
    let src = aim_paths::fetched().join("xsdc");
    let git = |log: &mut Log, args: &[&str]| {
        log.output(Command::new("git").arg("-C").arg(&src).args(args))
    };
    if !src.join(".git").exists() {
        fs::create_dir_all(&src).map_err(|e| e.to_string())?;
        git(log, &["init", "-q"])?;
        git(
            log,
            &[
                "remote",
                "add",
                "origin",
                "https://android.googlesource.com/platform/system/tools/xsdc",
            ],
        )?;
    }
    if git(log, &["rev-parse", "HEAD"])
        .map(|h| h.trim().to_string())
        .as_deref()
        != Ok(revision)
    {
        git(log, &["fetch", "-q", "--depth=1", "origin", revision])?;
        git(
            log,
            &[
                "-c",
                "advice.detachedHead=false",
                "checkout",
                "-q",
                "--detach",
                revision,
            ],
        )?;
    }
    if git(log, &["rev-parse", "HEAD"])?.trim() != revision
        || !git(log, &["status", "--porcelain"])?.is_empty()
    {
        return Err(format!(
            "{} is not a clean checkout of {revision}",
            src.display()
        ));
    }

    let dependency = aim_paths::downloads().join(COMMONS_CLI);
    let want = lock.get("COMMONS_CLI_SHA256")?;
    if !dependency.exists() {
        fetch::download(
            "https://repo.maven.apache.org/maven2/commons-cli/commons-cli/1.2/commons-cli-1.2.jar",
            &dependency,
            log,
            |_, _| true,
        )?;
    }
    if hash::sha256_file(&dependency).map_err(|e| e.to_string())? != want {
        return Err(format!(
            "{}: not the pinned sha256 {want}",
            dependency.display()
        ));
    }

    let out = out();
    let classes = out.join("classes");
    let _ = fs::remove_dir_all(&classes);
    fs::create_dir_all(&classes).map_err(|e| e.to_string())?;
    let sources: Vec<PathBuf> = hash::files_under(&src.join("src/main/java"))
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "java"))
        .collect();
    let jdk = tools::jdk()?;
    log.run(
        Command::new(jdk.join("bin/javac"))
            .args(["--release", "17", "-cp"])
            .arg(&dependency)
            .arg("-d")
            .arg(&classes)
            .args(&sources),
    )?;
    log.run(
        Command::new(jdk.join("bin/jar"))
            .args(["--create", "--file"])
            .arg(out.join("xsdc.jar"))
            .args(["--main-class", "com.android.xsdc.Main", "-C"])
            .arg(&classes)
            .arg("."),
    )?;
    fs::copy(&dependency, out.join(COMMONS_CLI)).map_err(|e| e.to_string())?;
    Ok(())
}
