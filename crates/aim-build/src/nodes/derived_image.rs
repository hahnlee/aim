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
//! A device app ([`device_services::APPS`]) is added at the guest path it
//! is built for, with the `oat` node's manifest of its compiled code
//! ([`oat::app_manifest`]) included beside it, and that manifest only with
//! its app.

use super::{device_services, oat, repo};
use crate::graph::{Action, Dep, Node};
use crate::log::Log;
use aim_android_image::{Manifest, Problem, assemble, identity, manifest};
use aim_storage::system;
use std::collections::BTreeSet;
use std::fs;

const OVERLAY: &str = "image/overlay.toml";

/// `problems` of the manifest `name`, one per line.
fn problems(name: &str, problems: Vec<Problem>) -> String {
    let lines: Vec<String> = problems.iter().map(|p| format!("{name}: {p}")).collect();
    lines.join("\n")
}

/// The overlay.
fn manifest() -> Result<Manifest, String> {
    let path = repo(OVERLAY);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    manifest::parse(&text).map_err(|p| problems(OVERLAY, p))
}

/// Each device app `manifest` adds is at its guest path, with its compiled
/// code included; no app's code is included without the app.
fn check_apps(manifest: &Manifest) -> Result<(), String> {
    let built = device_services::out();
    let built = built.strip_prefix(aim_paths::root()).unwrap();
    for app in &device_services::APPS {
        let source = built.join(app.apk).display().to_string();
        let code = oat::app_manifest(app.guest);
        let added = manifest
            .entries
            .iter()
            .find(|e| e.source.as_deref() == Some(source.as_str()));
        let included = manifest.includes.iter().find(|i| i.source == code);
        match (added, included) {
            (Some(e), _) if e.path != app.guest => {
                return Err(format!(
                    "{OVERLAY}:{}: {source} is built for {}, not {}",
                    e.line, app.guest, e.path
                ));
            }
            (Some(e), None) => {
                return Err(format!(
                    "{OVERLAY}:{}: {} needs its compiled code beside it: [[include]] source = \"{code}\"",
                    e.line, app.guest
                ));
            }
            (None, Some(i)) => {
                return Err(format!(
                    "{OVERLAY}:{}: {code} without its app {}",
                    i.line, app.guest
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn node(others: &[Node]) -> Result<Node, String> {
    let manifest = manifest()?;
    check_apps(&manifest)?;
    let built = [aim_paths::root().join("target"), aim_paths::fetched()];
    let mut inputs = vec![repo(OVERLAY)];
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
    let mut manifest = manifest()?;
    manifest::expand(&mut manifest, aim_paths::root()).map_err(|p| problems(OVERLAY, p))?;
    let plan = aim_android_image::validate(&manifest, &original, aim_paths::root())
        .map_err(|p| problems(OVERLAY, p))?;
    let original_identity = identity::original_identity(&original, None)?
        .ok_or_else(|| format!("{}: no identity", original.display()))?;
    let derived = identity::compute(&original_identity, &plan);
    let (image, shadow) = (aim_paths::system_image(), aim_paths::derived_image_shadow());
    let mount = aim_paths::derived_image_mount();
    let _lease = system::ImageLease::write(&image)?;
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
        system::attach(&image, Some(&shadow), &mount, false)?;
        let found = identity::read_tree_identity(&aim_paths::derived_image())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn check(text: &str) -> Result<(), String> {
        check_apps(&manifest::parse(text).unwrap())
    }

    #[test]
    fn a_device_app_comes_with_its_compiled_code() {
        let add = |path: &str| {
            format!(
                "[[add]]\npath = \"{path}\"\nsource = \"target/aim/device-services/lightweight-home.apk\"\n"
            )
        };
        let include =
            "[[include]]\nsource = \"target/aim/oat/apps/AimHome.toml\"\nreason = \"r\"\n";
        let home = "/system_ext/app/AimHome/AimHome.apk";
        assert_eq!(check("schema = 1\n"), Ok(()));
        assert_eq!(
            check(&format!("schema = 1\n{}{include}", add(home))),
            Ok(())
        );
        let without = check(&format!("schema = 1\n{}", add(home))).unwrap_err();
        assert!(
            without.contains("target/aim/oat/apps/AimHome.toml"),
            "{without}"
        );
        let moved = check(&format!(
            "schema = 1\n{}{include}",
            add("/system/app/H/H.apk")
        ));
        assert!(moved.unwrap_err().contains("is built for"));
        let orphan = check(&format!("schema = 1\n{include}")).unwrap_err();
        assert!(orphan.contains("without its app"), "{orphan}");
    }
}
