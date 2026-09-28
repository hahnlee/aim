//! `image`: the original image, extracted from the pinned archive of
//! `image/original.lock` and verified by its identity. An existing tree is
//! never rewritten: it may be shared with other checkouts.

use super::repo;
use crate::graph::{Action, Ctx, Dep, Node};
use crate::hash;
use crate::lockfile::Lock;
use crate::log::Log;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const LOCK: &str = "image/original.lock";

pub fn node() -> Node {
    Node {
        name: "image".into(),
        deps: vec![Dep::order_only("host/android-image-extract")],
        inputs: vec![repo(LOCK)],
        outputs: vec![aim_paths::original_image()],
        tools: Vec::new(),
        recipe: 1,
        action: Action::Image,
        boot: true,
    }
}

/// The identity android-image-extract recorded beside the (resolved) tree.
fn identity_file(tree: &Path) -> Result<PathBuf, String> {
    let real = fs::canonicalize(tree).map_err(|e| format!("{}: {e}", tree.display()))?;
    let mut name = real.file_name().unwrap().to_os_string();
    name.push(".identity");
    Ok(real.with_file_name(name))
}

fn check(tree: &Path, want: &str) -> Result<(), String> {
    let file = identity_file(tree)?;
    let got = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    if got.trim() == want {
        Ok(())
    } else {
        Err(format!(
            "{} holds the original {}, but {LOCK} pins {want}; move it away to extract the pin",
            tree.display(),
            got.trim()
        ))
    }
}

pub fn run(ctx: &Ctx, log: &mut Log) -> Result<(), String> {
    let lock = Lock::read(&repo(LOCK))?;
    let want = lock.get("SHA256")?;
    let tree = aim_paths::original_image();
    if tree.exists() {
        check(&tree, want)?;
        log.line(&format!("{} is the original {want}", tree.display()));
        return Ok(());
    }
    let archive = repo(lock.get("ARCHIVE")?);
    let got = hash::sha256_file(&archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    if got != want {
        return Err(format!(
            "{}: sha256 {got}, {LOCK} pins {want}",
            archive.display()
        ));
    }
    log.run(
        Command::new(ctx.workspace.host_bin("android-image-extract"))
            .arg(&archive)
            .arg(&tree),
    )?;
    check(&tree, want)
}
