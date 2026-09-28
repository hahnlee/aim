//! `art`: the ART exception (docs/art-exception-patches.md): libart,
//! dex2oat64 and the libraries they bring, rebuilt from the pinned AOSP
//! sources with the base-relative reference series in
//! `patches/art-android/`, as android-arm64 ELF linked against the
//! original image's platform libraries.
//!
//! 1. Fetch the trees and files of `patches/art-android/sources.lock`.
//! 2. Stage ART in `target/aim/art/src/art` with the series applied;
//!    only files whose content changed are rewritten, so the build redoes
//!    only what a patch touches. The other fetched trees are linked beside
//!    it, the source root `gen_build.py` expects.
//! 3. `tools/art-android/gen_build.py` writes the ninja file from ART's own
//!    Android.bp files; n2 runs it.
//! 4. The derived image takes copies without debug info (`stripped/`).

use super::{files, repo};
use crate::fetch::Source;
use crate::graph::{Action, Dep, Node};
use crate::lockfile::Lock;
use crate::log::Log;
use crate::n2db;
use crate::tools::{self, Tool};
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const LOCK: &str = "patches/art-android/sources.lock";
const ART: &str = "platform/art";
/// What the derived image takes from the build.
const STRIPPED: [&str; 7] = [
    "lib64/libartbase.so",
    "lib64/libdexfile.so",
    "lib64/libprofile.so",
    "lib64/libart.so",
    "lib64/libopenjdkjvm.so",
    "lib64/libadbconnection.so",
    "bin/dex2oat64",
];

pub fn node() -> Node {
    let mut inputs = files("patches/art-android");
    inputs.extend(files("tools/art-android"));
    Node {
        name: "art".into(),
        deps: vec![Dep::on("xsdc"), Dep::on("image")],
        inputs,
        outputs: STRIPPED
            .iter()
            .map(|f| aim_paths::art().join("stripped").join(f))
            .collect(),
        tools: vec![Tool::Ndk, Tool::Python, Tool::Java, Tool::N2],
        recipe: 1,
        action: Action::Art,
        boot: true,
    }
}

pub fn run(log: &mut Log) -> Result<(), String> {
    let lock = Lock::read(&repo(LOCK))?;
    let tag = lock.get("AOSP_TAG")?;
    let mut problems = Vec::new();
    let mut art_paths = Vec::new();
    let mut tops = BTreeSet::new();
    for (entry, is_file) in lock
        .array("SOURCE_TREES")
        .iter()
        .map(|e| (e, false))
        .chain(lock.array("SOURCE_FILES").iter().map(|e| (e, true)))
    {
        let source = Source::parse(entry, tag)?;
        let fetched = if is_file {
            source.file(log)
        } else {
            source.tree(log)
        };
        if let Err(problem) = fetched {
            problems.push(problem);
        }
        if source.project == ART {
            art_paths.push(source.path.clone());
        } else {
            let rel = source.dest();
            let rel = rel.strip_prefix(aim_paths::aosp()).unwrap();
            tops.extend(rel.components().next().map(|c| c.as_os_str().to_owned()));
        }
    }
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }

    let out = aim_paths::art();
    let src = out.join("src");
    let stage = src.join("art");
    let fresh = src.join(".art.new");
    let _ = fs::remove_dir_all(&fresh);
    for path in &art_paths {
        copy_tree(&aim_paths::aosp().join("art").join(path), &fresh.join(path))?;
    }
    let series = repo("patches/art-android/series");
    let series = fs::read_to_string(&series).map_err(|e| format!("{}: {e}", series.display()))?;
    for patch in series.lines().map(str::trim) {
        if patch.is_empty() || patch.starts_with('#') {
            continue;
        }
        log.run(
            Command::new("patch")
                .args(["--batch", "--forward", "--quiet", "-p1", "-d"])
                .arg(&fresh)
                .arg("-i")
                .arg(repo("patches/art-android").join(patch)),
        )?;
    }
    sync_tree(&fresh, &stage)?;
    fs::remove_dir_all(&fresh).map_err(|e| e.to_string())?;
    for top in tops {
        let link = src.join(&top);
        let target = aim_paths::aosp().join(&top);
        if fs::read_link(&link).ok() != Some(target.clone()) {
            let _ = fs::remove_file(&link);
            std::os::unix::fs::symlink(&target, &link).map_err(|e| e.to_string())?;
        }
    }

    let image = fs::canonicalize(aim_paths::original_image()).map_err(|e| e.to_string())?;
    let ndk = tools::ndk()?;
    log.run(
        Command::new("python3")
            .arg(repo("tools/art-android/gen_build.py"))
            .arg("--art")
            .arg(&stage)
            .arg("--image")
            .arg(&image)
            .arg("--ndk")
            .arg(&ndk)
            .arg("--out")
            .arg(&out)
            .arg("--xsdc")
            .arg(super::xsdc::out())
            .arg("--apex-xsd")
            .arg(src.join("system/apex/apexd/ApexInfoList.xsd"))
            .arg("--java")
            .arg(tools::jdk()?.join("bin/java")),
    )?;
    n2db::build(&out, &[], log)?;

    let strip = tools::ndk_toolchain()?.join("bin/llvm-strip");
    for file in STRIPPED {
        let (built, stripped) = (out.join(file), out.join("stripped").join(file));
        if mtime(&stripped) > mtime(&built) {
            continue;
        }
        fs::create_dir_all(stripped.parent().unwrap()).map_err(|e| e.to_string())?;
        log.run(
            Command::new(&strip)
                .arg("--strip-debug")
                .arg("-o")
                .arg(&stripped)
                .arg(&built),
        )?;
    }
    Ok(())
}

fn mtime(path: &Path) -> Option<i128> {
    let m = fs::metadata(path).ok()?;
    Some(i128::from(m.mtime()) * 1_000_000_000 + i128::from(m.mtime_nsec()))
}

/// Copies a fetched tree (files, links and modes; no `.fetched` marker).
fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(from).map_err(|e| format!("{}: {e}", from.display()))?;
    if meta.is_dir() {
        fs::create_dir_all(to).map_err(|e| e.to_string())?;
        for entry in fs::read_dir(from).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_name() != ".fetched" {
                copy_tree(&entry.path(), &to.join(entry.file_name()))?;
            }
        }
    } else if meta.file_type().is_symlink() {
        let target = fs::read_link(from).map_err(|e| e.to_string())?;
        std::os::unix::fs::symlink(target, to).map_err(|e| e.to_string())?;
    } else {
        fs::create_dir_all(to.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::copy(from, to).map_err(|e| format!("{}: {e}", from.display()))?;
    }
    Ok(())
}

/// Makes `old` a copy of `new`, rewriting only what differs, so unchanged
/// files keep their modification times.
fn sync_tree(new: &Path, old: &Path) -> Result<(), String> {
    let mut wanted = BTreeSet::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(rel) = pending.pop() {
        for entry in fs::read_dir(new.join(&rel)).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let rel = rel.join(entry.file_name());
            let (from, to) = (new.join(&rel), old.join(&rel));
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            wanted.insert(rel.clone());
            if kind.is_dir() {
                if !fs::symlink_metadata(&to).is_ok_and(|m| m.is_dir()) {
                    let _ = fs::remove_file(&to);
                    fs::create_dir_all(&to).map_err(|e| e.to_string())?;
                }
                pending.push(rel);
            } else if kind.is_symlink() {
                let target = fs::read_link(&from).map_err(|e| e.to_string())?;
                if fs::read_link(&to).ok() != Some(target.clone()) {
                    let _ = fs::remove_file(&to);
                    std::os::unix::fs::symlink(target, &to).map_err(|e| e.to_string())?;
                }
            } else {
                let same = fs::read(&from).ok() == fs::read(&to).ok()
                    && fs::metadata(&from).map(|m| m.mode()).ok()
                        == fs::metadata(&to).map(|m| m.mode()).ok();
                if !same {
                    let _ = fs::remove_file(&to);
                    fs::copy(&from, &to).map_err(|e| format!("{}: {e}", to.display()))?;
                }
            }
        }
    }
    let mut stale = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(rel) = pending.pop() {
        let Ok(entries) = fs::read_dir(old.join(&rel)) else {
            continue;
        };
        for entry in entries.flatten() {
            let rel = rel.join(entry.file_name());
            if !wanted.contains(&rel) {
                stale.push(old.join(&rel));
            } else if entry.file_type().is_ok_and(|k| k.is_dir()) {
                pending.push(rel);
            }
        }
    }
    for path in stale {
        let removed = if fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        removed.map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}
