//! `cargo aim fmt`: rustfmt over the workspace's own crates. A crate whose
//! sources are fetched or generated (`hal/binder` builds the AOSP sources
//! in `_build`, the AIDL crates are generated into `target`) is left out,
//! which `cargo fmt` alone cannot do: it formats every member's sources
//! wherever they are.

use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use serde_json::Value;

use crate::cargo::cargo;

/// `path` with its `..` components folded, as cargo metadata reports a
/// target whose path leaves its package's directory.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Whether `src` is the repository's own source: under `root`, and not
/// under the fetched trees or the build output.
fn own_source(root: &Path, src: &Path) -> bool {
    let src = normalize(src);
    src.starts_with(root)
        && !["_build", "_prebuilt", "target"]
            .iter()
            .any(|dir| src.starts_with(root.join(dir)))
}

/// The workspace members all of whose targets are the repository's own
/// sources.
fn own_packages(root: &Path, metadata: &Value) -> Vec<String> {
    let members: Vec<&str> = metadata["workspace_members"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["id"].as_str().is_some_and(|id| members.contains(&id)))
        .filter(|p| {
            p["targets"].as_array().into_iter().flatten().all(|t| {
                t["src_path"]
                    .as_str()
                    .is_some_and(|src| own_source(root, Path::new(src)))
            })
        })
        .filter_map(|p| p["name"].as_str().map(String::from))
        .collect()
}

pub fn run(check: bool) -> Result<ExitCode, String> {
    let root = aim_paths::root();
    let out = cargo()
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .current_dir(&root)
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let metadata: Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let mut fmt = cargo();
    fmt.arg("fmt").current_dir(&root);
    for package in own_packages(&root, &metadata) {
        fmt.args(["-p", &package]);
    }
    if check {
        fmt.arg("--check");
    }
    let status = fmt.status().map_err(|e| format!("cargo fmt: {e}"))?;
    Ok(if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetched_and_generated_crates_are_left_out() {
        let root = Path::new("/r");
        let metadata = serde_json::json!({
            "workspace_members": ["own", "binder", "aidl"],
            "packages": [
                {"id": "own", "name": "own", "targets": [{"src_path": "/r/crates/own/src/lib.rs"}]},
                {"id": "binder", "name": "binder", "targets": [{"src_path": "/r/hal/binder/../../_build/aosp/binder/src/lib.rs"}, {"src_path": "/r/hal/binder/build.rs"}]},
                {"id": "aidl", "name": "aidl", "targets": [{"src_path": "/r/target/aim/gen/aidl/lib.rs"}]},
                {"id": "dep", "name": "dep", "targets": [{"src_path": "/r/crates/dep/src/lib.rs"}]},
            ],
        });
        assert_eq!(own_packages(root, &metadata), ["own"]);
    }
}
