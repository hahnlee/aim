//! `image`: the system image (docs/storage.md). The pinned archive of
//! `image/original.lock`, verified by its sha256, is extracted into a fresh
//! case-sensitive volume (`root/`, with `root.identity` beside it), its ELF
//! files are translated into the volume's `translated/`, and the volume
//! becomes the read-only `_build/android16-image.dmg` (in
//! [`system::FORMAT`]), mounted hidden at `_build/android16-image`. An
//! existing image is never rewritten, since other checkouts may share it;
//! this node then only checks its identity and attaches it. An image in
//! another format is built again (derived images over it are replaced).

use super::repo;
use crate::graph::{Action, Ctx, Dep, Node};
use crate::hash;
use crate::lockfile::Lock;
use crate::log::Log;
use aim_storage::system;
use std::fs;
use std::path::Path;
use std::process::Command;

const LOCK: &str = "image/original.lock";

pub fn node() -> Node {
    Node {
        name: "image".into(),
        deps: vec![
            Dep::order_only("host/android-image-extract"),
            Dep::order_only("host/linux-translate"),
        ],
        inputs: vec![repo(LOCK)],
        // Missing while the image is detached (after a restart), which
        // runs the node, and so attaches it, before anything reads it.
        outputs: vec![identity_file(&aim_paths::system_image_mount())],
        tools: Vec::new(),
        recipe: 3,
        action: Action::Image,
        boot: true,
    }
}

/// The identity android-image-extract recorded beside the tree.
fn identity_file(mount: &Path) -> std::path::PathBuf {
    mount.join("root.identity")
}

fn check(mount: &Path, want: &str) -> Result<(), String> {
    let file = identity_file(mount);
    let got = fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    if got.trim() == want {
        Ok(())
    } else {
        Err(format!(
            "{} holds the original {}, but {LOCK} pins {want}; move it away to build the pin",
            aim_paths::system_image().display(),
            got.trim()
        ))
    }
}

pub fn run(ctx: &Ctx, log: &mut Log) -> Result<(), String> {
    let lock = Lock::read(&repo(LOCK))?;
    let want = lock.get("SHA256")?;
    let image = aim_paths::system_image();
    if image.exists() && aim_storage::disk::format_of(&image)? != system::FORMAT {
        log.line(&format!(
            "{} is not {}; building it again",
            image.display(),
            system::FORMAT
        ));
        system::detach_all(&image)?;
        fs::remove_file(&image).map_err(|e| format!("{}: {e}", image.display()))?;
    }
    if !image.exists() {
        let archive = repo(lock.get("ARCHIVE")?);
        let got = hash::sha256_file(&archive).map_err(|e| format!("{}: {e}", archive.display()))?;
        if got != want {
            return Err(format!(
                "{}: sha256 {got}, {LOCK} pins {want}",
                archive.display()
            ));
        }
        system::build(&image, "aim-system", |mount| {
            // A name that differs from another only in case fails the
            // extraction; on this volume there is none to lose.
            log.run(
                Command::new(ctx.workspace.host_bin("android-image-extract"))
                    .arg(&archive)
                    .arg(system::root(mount)),
            )?;
            log.run(
                Command::new(ctx.workspace.host_bin("linux-translate"))
                    .arg("--image")
                    .arg(mount),
            )
        })?;
        log.line(&format!("built {}", image.display()));
    }
    let mount = aim_paths::system_image_mount();
    system::attach(&image, None, &mount, false)?;
    check(&mount, want)?;
    log.line(&format!("{} is the original {want}", mount.display()));
    Ok(())
}
