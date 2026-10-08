//! The nodes that are not cargo packages. Each declares its inputs (lock,
//! patch and helper files), its upstream nodes and its outputs; the derived
//! image's upstream comes from `image/overlay.toml`.

mod aidl;
mod angle;
mod art;
mod boot_image;
mod derived_image;
mod device_services;
mod image;
mod java;
mod moltenvk;
mod oat;
mod service_aidl;
mod system_server;
mod translation_cache;
pub(crate) mod userdata;
mod xsdc;

use crate::graph::{Action, Ctx, Node};
use crate::log::Log;
use std::path::PathBuf;

/// The static nodes, then the derived image over `built` (every other
/// node).
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
        device_services::node(),
        oat::node(),
    ]);
    let derived = derived_image::node(&built)?;
    built.push(derived);
    built.push(translation_cache::node());
    built.push(userdata::empty());
    built.push(userdata::template());
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
        Action::SystemServer => system_server::run(log)?,
        Action::DeviceServices => device_services::run(log)?,
        Action::Oat => oat::run(ctx, log)?,
        Action::DerivedImage => derived_image::run(log)?,
        Action::TranslationCache => translation_cache::run(ctx, log)?,
        Action::EmptyUserdata => userdata::run_empty(log)?,
        Action::UserdataTemplate => userdata::run_template(ctx, log)?,
        Action::Cargo(_) => unreachable!(),
    }
    Ok(Vec::new())
}

/// A key for what `node` made, for early cutoff (docs/build.md, "Keys and
/// stamps"): a cargo artifact's content, or the derived image's identity
/// and shadow file. `None` where naming the output costs as much as a
/// rebuild; downstream nodes then see the node's own key.
pub fn output_key(node: &Node) -> Option<String> {
    match &node.action {
        Action::Cargo(unit) => {
            crate::hash::sha256_file(unit.install.as_ref().unwrap_or(&unit.artifact)).ok()
        }
        Action::DerivedImage => derived_image::output_key(),
        _ => None,
    }
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
        Action::DeviceServices => vec![device_services::out()],
        Action::Oat => vec![oat::out()],
        Action::EmptyUserdata => vec![userdata::out().join(aim_storage::data::EMPTY_TEMPLATE)],
        // Its templates go with the next run.
        Action::UserdataTemplate => Vec::new(),
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
    let _lease = aim_storage::system::ImageLease::write(&aim_paths::system_image())?;
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
