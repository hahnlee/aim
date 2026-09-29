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

use super::repo;
use crate::graph::{Action, Dep, Node};
use crate::log::Log;
use aim_android_image::{assemble, identity};
use aim_storage::system;
use std::collections::BTreeSet;
use std::fs;

const OVERLAY: &str = "image/overlay.toml";

pub fn node(others: &[Node]) -> Result<Node, String> {
    let overlay = repo(OVERLAY);
    let text = fs::read_to_string(&overlay).map_err(|e| format!("{OVERLAY}: {e}"))?;
    let manifest = aim_android_image::manifest::parse(&text).map_err(|problems| {
        let lines: Vec<String> = problems.iter().map(|p| format!("{OVERLAY}: {p}")).collect();
        lines.join("\n")
    })?;
    let built = [aim_paths::root().join("target"), aim_paths::fetched()];
    let mut inputs = vec![overlay];
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
                    "{OVERLAY}:{line}: no node produces {source} (`cargo aim status` lists the nodes)"
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
        action: Action::DerivedImage,
        boot: true,
    })
}

pub fn run(log: &mut Log) -> Result<(), String> {
    let original = fs::canonicalize(aim_paths::original_image()).map_err(|e| e.to_string())?;
    let (_, plan) = aim_android_image::load(&repo(OVERLAY), &original, aim_paths::root()).map_err(
        |problems| {
            let lines: Vec<String> = problems.iter().map(|p| format!("{OVERLAY}: {p}")).collect();
            lines.join("\n")
        },
    )?;
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
