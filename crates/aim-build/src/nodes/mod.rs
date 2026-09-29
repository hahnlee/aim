//! The nodes that are not cargo packages. Each declares its inputs (lock,
//! patch and helper files), its upstream nodes and its outputs; the derived
//! image's upstream comes from `image/overlay.toml`.

mod aidl;
mod angle;
mod art;
mod boot_image;
mod derived_image;
mod image;
mod moltenvk;
mod service_aidl;
mod system_server;
mod translation_cache;
mod xsdc;

use crate::graph::{Action, Ctx, Node};
use crate::log::Log;
use std::path::PathBuf;

/// The static nodes, then the derived image over `built` (every other node).
pub fn declare(mut built: Vec<Node>) -> Result<Vec<Node>, String> {
    built.extend([
        image::node(),
        aidl::node(),
        xsdc::node(),
        art::node(),
        boot_image::node(),
        angle::node(),
        moltenvk::node(),
        system_server::node(),
    ]);
    let derived = derived_image::node(&built)?;
    built.push(derived);
    built.push(translation_cache::node());
    Ok(built)
}

/// Runs a non-cargo node; returns its found inputs (n2's deps log for the
/// ninja nodes).
pub fn run(node: &Node, ctx: &Ctx, log: &mut Log) -> Result<Vec<PathBuf>, String> {
    match node.action {
        Action::Image => image::run(ctx, log)?,
        Action::AidlGen => aidl::run(log)?,
        Action::Xsdc => xsdc::run(log)?,
        Action::Art => {
            art::run(log)?;
            return crate::n2db::repository_deps(&aim_paths::art());
        }
        Action::BootImage => boot_image::run(ctx, log)?,
        Action::Angle => angle::run(log)?,
        Action::MoltenVk => moltenvk::run(log)?,
        Action::SystemServer => system_server::run(ctx, log)?,
        Action::DerivedImage => derived_image::run(log)?,
        Action::TranslationCache => translation_cache::run(ctx, log)?,
        Action::Cargo(_) => unreachable!(),
    }
    Ok(Vec::new())
}

/// Every file under `dir` (relative to the repository root).
fn files(dir: &str) -> Vec<PathBuf> {
    crate::hash::files_under(&aim_paths::root().join(dir))
}

fn repo(path: &str) -> PathBuf {
    aim_paths::root().join(path)
}

/// Forgets `node` and removes its outputs. Cargo's own artifacts (`cargo
/// clean`), the shared original image and the fetched sources stay.
pub fn clean(node: &Node) -> Result<(), String> {
    crate::stamp::Stamp::remove(&node.name);
    let paths: Vec<PathBuf> = match &node.action {
        Action::Cargo(unit) => unit.install.iter().cloned().collect(),
        Action::Image | Action::TranslationCache => Vec::new(),
        Action::AidlGen => node.outputs.clone(),
        Action::Xsdc => vec![xsdc::out()],
        Action::Art => vec![aim_paths::art()],
        Action::BootImage => vec![aim_paths::boot_image()],
        // The build tree of a checkout other checkouts may share.
        Action::Angle => Vec::new(),
        Action::MoltenVk => vec![aim_paths::moltenvk()],
        Action::SystemServer => vec![system_server::out()],
        Action::DerivedImage => {
            detach_derived()?;
            vec![aim_paths::derived_image_shadow()]
        }
    };
    for path in paths {
        aim_android_image::assemble::force_remove(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

/// Detaches the derived image (its shadow stays).
pub fn detach_derived() -> Result<(), String> {
    aim_storage::system::detach(
        &aim_paths::system_image(),
        Some(&aim_paths::derived_image_shadow()),
    )
}

/// `dir`, created if needed, with every link resolved: the Python helpers
/// compute relative paths between their arguments, which a link (a
/// worktree's `target` or `_build`) would break.
fn real(dir: &std::path::Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))
}
