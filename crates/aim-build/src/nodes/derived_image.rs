//! `derived-image`: the original plus `image/overlay.toml`
//! (crates/aim-android-image), at `target/aim/derived-image`.
//!
//! Its upstream is read from the overlay: a source under `target/` or
//! `_build/` must be an output of some node, which the derived image then
//! depends on (a source no node produces is an error); every other source
//! is a checked-in input.

use super::repo;
use crate::graph::{Action, Dep, Node};
use crate::log::Log;
use aim_android_image::{assemble, identity};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

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
    for entry in &manifest.entries {
        let Some(source) = &entry.source else {
            continue;
        };
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
                    "{OVERLAY}:{}: no node produces {source} (`cargo aim status` lists the nodes)",
                    entry.line
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
        recipe: 1,
        action: Action::DerivedImage,
        boot: true,
    })
}

pub fn run(log: &mut Log) -> Result<(), String> {
    // The real original: a derived image is cloned from it, never through a
    // link (which would apply the overlay to the original).
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
    let out: PathBuf = aim_paths::derived_image();
    if identity::read_tree_identity(&out)
        .ok()
        .flatten()
        .is_some_and(|found| found != derived.hex)
    {
        log.line(&format!("replacing {}", out.display()));
        assemble::force_remove(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    let outcome = assemble(&plan, &original, &derived, &out)?;
    log.line(&format!("{outcome:?} {} ({})", out.display(), derived.hex));
    Ok(())
}
