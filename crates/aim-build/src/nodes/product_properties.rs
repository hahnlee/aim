//! The derived product's GC configuration, matching the ART exception
//! (ADR 0012 decision 4). Every other byte stays from the pinned image.

use super::repo;
use crate::graph::{Action, Dep, Node};
use crate::hash;
use crate::log::Log;
use std::fs;
use std::path::PathBuf;

const INPUT: &str = "product/etc/build.prop";
const SHA256: &str = "0319328546c01e1eddad657c0cd5e608001e1ee3d216a5ad6b4d6feef21faaf1";
const PROPERTY: &str = "ro.dalvik.vm.enable_uffd_gc=true\n";

pub fn out() -> PathBuf {
    aim_paths::out().join("product-properties/build.prop")
}

pub fn node() -> Node {
    Node {
        name: "product-properties".into(),
        deps: vec![Dep::on("image")],
        inputs: vec![
            repo("image/original.lock"),
            aim_paths::original_image().join(INPUT),
        ],
        outputs: vec![out()],
        tools: Vec::new(),
        recipe: 1,
        action: Action::ProductProperties,
        boot: true,
    }
}

fn generate(original: &str) -> Result<String, String> {
    if hash::sha256(original.as_bytes()) != SHA256 {
        return Err(format!(
            "{INPUT}: pinned original changed (expected {SHA256})"
        ));
    }
    if original
        .lines()
        .filter(|line| *line == PROPERTY.trim_end())
        .count()
        != 1
    {
        return Err(format!("{INPUT}: expected exactly one {PROPERTY:?}"));
    }
    Ok(original.replacen(PROPERTY, "ro.dalvik.vm.enable_uffd_gc=false\n", 1))
}

pub fn run(log: &mut Log) -> Result<(), String> {
    let input = aim_paths::original_image().join(INPUT);
    let original = fs::read_to_string(&input).map_err(|e| format!("{}: {e}", input.display()))?;
    let generated = generate(&original)?;
    let out = out();
    fs::create_dir_all(out.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(&out, generated).map_err(|e| format!("{}: {e}", out.display()))?;
    log.line("product: ro.dalvik.vm.enable_uffd_gc=false (concurrent copying, no userfaultfd)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_product_changes_only_the_gc_property() {
        let original = fs::read_to_string(aim_paths::original_image().join(INPUT)).unwrap();
        let generated = generate(&original).unwrap();
        assert_eq!(
            generated.replace("ro.dalvik.vm.enable_uffd_gc=false\n", PROPERTY),
            original
        );
        assert_eq!(
            generated
                .matches("ro.dalvik.vm.enable_uffd_gc=false\n")
                .count(),
            1
        );
        for changed in [
            original.replace(PROPERTY, ""),
            original.replace(PROPERTY, "ro.dalvik.vm.enable_uffd_gc=false\n"),
            format!("{original}{PROPERTY}"),
            format!("{original}# changed\n"),
        ] {
            assert!(generate(&changed).is_err());
        }
    }
}
