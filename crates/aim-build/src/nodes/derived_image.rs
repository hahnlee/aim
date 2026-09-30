//! `derived-image`: the original plus `image/overlay.toml`
//! (crates/aim-android-image). The overlay is applied to the system image
//! attached with a shadow file (`target/aim/derived.shadow`), which takes
//! every write, so the derived image costs only what the overlay changes;
//! it is then mounted read-only at `target/aim/derived` (docs/storage.md).
//!
//! Its upstream is read from the overlay: a source (or included manifest,
//! whose own sources that node produces too) under `target/` or
//! `_build/` must be an output of some node, which the derived image then
//! depends on (a source no node produces is an error); every other source
//! is a checked-in input.
//!
//! A variant (`cargo aim build --variant NAME`) is the same image with the
//! entries of `image/variants/NAME.toml` too, a check-only image of a
//! change not yet made to every boot (docs/build.md, "Image variants"). It
//! replaces the derived image until a build without the variant.

use super::repo;
use crate::graph::{Action, Dep, Node};
use crate::log::Log;
use aim_android_image::{Manifest, Problem, assemble, identity, manifest};
use aim_storage::system;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const OVERLAY: &str = "image/overlay.toml";
const VARIANTS: &str = "image/variants";

/// The manifest `variant` (a name) stands for.
pub fn variant(name: &str) -> Result<PathBuf, String> {
    let path = repo(VARIANTS).join(format!("{name}.toml"));
    if !path.is_file() {
        return Err(format!(
            "no image variant `{name}` ({VARIANTS}/{name}.toml)"
        ));
    }
    Ok(path)
}

/// `problems` of the manifest `name`, one per line.
fn problems(name: &str, problems: Vec<Problem>) -> String {
    let lines: Vec<String> = problems.iter().map(|p| format!("{name}: {p}")).collect();
    lines.join("\n")
}

/// The name of the overlay `variant` adds to, for messages.
fn name(variant: Option<&Path>) -> String {
    match variant {
        Some(v) => format!("{OVERLAY} with {}", v.display()),
        None => OVERLAY.into(),
    }
}

/// The overlay, with the entries of `variant` (which includes nothing).
fn manifest(variant: Option<&Path>) -> Result<Manifest, String> {
    let read = |path: &Path| {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        manifest::parse(&text).map_err(|p| problems(&path.display().to_string(), p))
    };
    let mut overlay = read(&repo(OVERLAY))?;
    if let Some(path) = variant {
        let added = read(path)?;
        if !added.includes.is_empty() {
            return Err(format!("{}: a variant includes nothing", path.display()));
        }
        overlay.entries.extend(added.entries);
    }
    Ok(overlay)
}

pub fn node(others: &[Node], variant: Option<PathBuf>) -> Result<Node, String> {
    let manifest = manifest(variant.as_deref())?;
    let built = [aim_paths::root().join("target"), aim_paths::fetched()];
    let mut inputs = vec![repo(OVERLAY)];
    inputs.extend(variant.clone());
    let mut deps = BTreeSet::from(["image".to_string()]);
    let sources = manifest
        .entries
        .iter()
        .filter_map(|e| Some((e.source.as_ref()?, e.line)))
        .chain(manifest.includes.iter().map(|i| (&i.source, i.line)));
    for (source, line) in sources {
        let path = aim_paths::root().join(source);
        if !built.iter().any(|b| path.starts_with(b)) {
            inputs.push(path);
            continue;
        }
        let producer = others
            .iter()
            .find(|n| n.outputs.iter().any(|o| path.starts_with(o)))
            .ok_or_else(|| {
                format!(
                    "{}:{line}: no node produces {source} (`cargo aim status` lists the nodes)",
                    name(variant.as_deref())
                )
            })?;
        deps.insert(producer.name.clone());
    }
    Ok(Node {
        name: "derived-image".into(),
        deps: deps.iter().map(|d| Dep::on(d)).collect(),
        inputs,
        outputs: vec![aim_paths::derived_image().join(".identity")],
        tools: Vec::new(),
        recipe: 2,
        action: Action::DerivedImage(variant),
        boot: true,
    })
}

pub fn run(variant: Option<&Path>, log: &mut Log) -> Result<(), String> {
    let original = fs::canonicalize(aim_paths::original_image()).map_err(|e| e.to_string())?;
    let mut manifest = manifest(variant)?;
    manifest::expand(&mut manifest, aim_paths::root()).map_err(|p| problems(&name(variant), p))?;
    let plan = aim_android_image::validate(&manifest, &original, aim_paths::root())
        .map_err(|p| problems(&name(variant), p))?;
    let original_identity = identity::original_identity(&original, None)?
        .ok_or_else(|| format!("{}: no identity", original.display()))?;
    let derived = identity::compute(&original_identity, &plan);
    let (image, shadow) = (aim_paths::system_image(), aim_paths::derived_image_shadow());
    let mount = aim_paths::derived_image_mount();
    let modified = |p: &std::path::Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    if shadow.exists() && modified(&shadow) < modified(&image) {
        // Written over a system image that has been built again since.
        log.line(&format!(
            "replacing {} (a new system image)",
            mount.display()
        ));
        system::detach(&image, Some(&shadow))?;
        fs::remove_file(&shadow).map_err(|e| format!("{}: {e}", shadow.display()))?;
    }
    if shadow.exists() {
        // A shadow of another system image does not attach, or shows
        // another identity.
        let found = system::attach(&image, Some(&shadow), &mount, false)
            .ok()
            .and_then(|()| identity::read_tree_identity(&aim_paths::derived_image()).ok())
            .flatten();
        if found.as_deref() == Some(derived.hex.as_str()) {
            log.line(&format!("reused {} ({})", mount.display(), derived.hex));
            return Ok(());
        }
        log.line(&format!("replacing {}", mount.display()));
        system::detach(&image, Some(&shadow))?;
        fs::remove_file(&shadow).map_err(|e| format!("{}: {e}", shadow.display()))?;
    }
    // Written through the shadow; its identity last, so an interrupted
    // build is replaced on the next run.
    system::attach(&image, Some(&shadow), &mount, true)?;
    let applied = assemble::apply(&plan, &derived, &system::root(&mount));
    system::detach(&image, Some(&shadow))?;
    applied?;
    system::attach(&image, Some(&shadow), &mount, false)?;
    log.line(&format!("built {} ({})", mount.display(), derived.hex));
    Ok(())
}

/// The mounted derived image's identity and its shadow file's creation
/// time: a shadow made again, even with the same identity, has lost what
/// the translation cache wrote into it.
pub fn output_key() -> Option<String> {
    let identity = identity::read_tree_identity(&aim_paths::derived_image()).ok()??;
    let created = fs::metadata(aim_paths::derived_image_shadow())
        .and_then(|m| m.created())
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(crate::hash::sha256(
        format!("{identity} {}", created.as_nanos()).as_bytes(),
    ))
}
