//! The cargo nodes, derived from `cargo metadata`, so a new crate needs no
//! edit here:
//!
//! - `host/<bin>`: every binary of the host crates (the workspace's
//!   default members), built with `--release`;
//! - `hal/<package>` and `daemon/<package>`: every guest package with a
//!   `[package.metadata.vendor-hal]` or `[package.metadata.daemon]` table,
//!   built for aarch64-linux-android with the `android` profile and
//!   installed where the table says.
//!
//! A cargo node's declared inputs are the manifests and build scripts of
//! the local packages it depends on, `Cargo.lock` and `.cargo/config.toml`;
//! its found inputs are the source files of cargo's dep-info for the
//! artifact. Sources under `target/` and `_build/` are covered by the
//! upstream nodes that produce them (aidl-gen), which a guest node depends
//! on when its dependencies include such a package.

use crate::graph::{Action, Ctx, Dep, Node};
use crate::log::Log;
use crate::tools::Tool;
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

pub const ANDROID_TARGET: &str = "aarch64-linux-android";
pub const ANDROID_PROFILE: &str = "android";
/// The API level of the NDK linker wrapper `.cargo/config.toml` names.
const ANDROID_API: u32 = 35;
const RECIPE: u32 = 1;

pub struct Package {
    pub name: String,
    pub manifest: PathBuf,
    pub local: bool,
    targets: Vec<Target>,
    metadata: Value,
    build_script: Option<PathBuf>,
    /// Normal and build dependencies (package ids).
    deps: Vec<String>,
}

struct Target {
    name: String,
    kind: Vec<String>,
    src_path: PathBuf,
}

pub struct Workspace {
    pub target_dir: PathBuf,
    packages: HashMap<String, Package>,
    members: Vec<String>,
    default_members: BTreeSet<String>,
}

/// One cargo node's artifact.
pub struct Unit {
    /// Nodes of one batch are built by one cargo invocation.
    pub batch: String,
    pub package: String,
    /// `--bin` for a host binary; guest packages are built whole.
    pub bin: Option<String>,
    pub artifact: PathBuf,
    pub install: Option<PathBuf>,
}

pub fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

impl Workspace {
    pub fn load() -> Result<Workspace, String> {
        let out = cargo()
            .args(["metadata", "--format-version", "1", "--locked"])
            .current_dir(aim_paths::root())
            .output()
            .map_err(|e| format!("cargo metadata: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "cargo metadata: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let json: Value =
            serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
        let strings = |v: &Value| -> Vec<String> {
            v.as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| s.as_str().map(String::from))
                .collect()
        };
        let mut deps: HashMap<String, Vec<String>> = HashMap::new();
        for node in json["resolve"]["nodes"].as_array().into_iter().flatten() {
            let id = node["id"].as_str().unwrap_or_default().to_string();
            let wanted = node["deps"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|d| {
                    d["dep_kinds"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|k| matches!(k["kind"].as_str(), None | Some("build")))
                })
                .filter_map(|d| d["pkg"].as_str().map(String::from))
                .collect();
            deps.insert(id, wanted);
        }
        let mut packages = HashMap::new();
        for p in json["packages"].as_array().into_iter().flatten() {
            let id = p["id"].as_str().unwrap_or_default().to_string();
            let targets: Vec<Target> = p["targets"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|t| Target {
                    name: t["name"].as_str().unwrap_or_default().into(),
                    kind: strings(&t["kind"]),
                    src_path: normalize(Path::new(t["src_path"].as_str().unwrap_or_default())),
                })
                .collect();
            let build_script = targets
                .iter()
                .find(|t| t.kind.iter().any(|k| k == "custom-build"))
                .map(|t| t.src_path.clone());
            packages.insert(
                id.clone(),
                Package {
                    name: p["name"].as_str().unwrap_or_default().into(),
                    manifest: normalize(Path::new(p["manifest_path"].as_str().unwrap_or_default())),
                    local: p["source"].is_null(),
                    targets,
                    metadata: p["metadata"].clone(),
                    build_script,
                    deps: deps.remove(&id).unwrap_or_default(),
                },
            );
        }
        Ok(Workspace {
            target_dir: json["target_directory"].as_str().unwrap_or("target").into(),
            packages,
            members: strings(&json["workspace_members"]),
            default_members: strings(&json["workspace_default_members"])
                .into_iter()
                .collect(),
        })
    }

    /// `id` and every package it depends on (normal and build).
    fn closure(&self, id: &str) -> Vec<&Package> {
        let mut seen = BTreeSet::new();
        let mut pending = vec![id.to_string()];
        while let Some(id) = pending.pop() {
            if seen.insert(id.clone()) {
                pending.extend(self.packages[&id].deps.iter().cloned());
            }
        }
        seen.iter().map(|id| &self.packages[id]).collect()
    }

    /// The name of the package with cargo id `id`.
    pub fn package_name(&self, id: &str) -> String {
        self.packages.get(id).map_or(id.into(), |p| p.name.clone())
    }

    /// The host binary `bin`'s path.
    pub fn host_bin(&self, bin: &str) -> PathBuf {
        self.target_dir.join("release").join(bin)
    }

    fn declared_inputs(&self, id: &str) -> Vec<PathBuf> {
        let root = aim_paths::root();
        let mut inputs = vec![
            root.join("Cargo.toml"),
            root.join("Cargo.lock"),
            root.join(".cargo/config.toml"),
        ];
        for package in self.closure(id).into_iter().filter(|p| p.local) {
            inputs.push(package.manifest.clone());
            inputs.extend(package.build_script.clone());
        }
        inputs.sort();
        inputs.dedup();
        inputs
    }

    /// Whether `id` depends on sources that a node generates or fetches.
    fn needs_generated_sources(&self, id: &str) -> bool {
        let generated = [aim_paths::generated(), aim_paths::aosp()];
        self.closure(id).iter().any(|p| {
            p.targets
                .iter()
                .any(|t| generated.iter().any(|g| t.src_path.starts_with(g)))
        })
    }

    pub fn nodes(&self) -> Result<Vec<Node>, String> {
        let mut nodes = Vec::new();
        for id in &self.members {
            let package = &self.packages[id];
            if self.default_members.contains(id) {
                if package.name == env!("CARGO_PKG_NAME") {
                    continue; // the running tool
                }
                for target in package
                    .targets
                    .iter()
                    .filter(|t| t.kind.iter().any(|k| k == "bin"))
                {
                    let artifact = self.host_bin(&target.name);
                    nodes.push(Node {
                        name: format!("host/{}", target.name),
                        deps: Vec::new(),
                        inputs: self.declared_inputs(id),
                        outputs: vec![artifact.clone()],
                        tools: vec![Tool::Rustc],
                        recipe: RECIPE,
                        action: Action::Cargo(Unit {
                            batch: "host".into(),
                            package: package.name.clone(),
                            bin: Some(target.name.clone()),
                            artifact,
                            install: None,
                        }),
                        boot: true,
                    });
                }
                continue;
            }
            let (prefix, table) = if package.metadata.get("vendor-hal").is_some() {
                ("hal", &package.metadata["vendor-hal"])
            } else if package.metadata.get("daemon").is_some() {
                ("daemon", &package.metadata["daemon"])
            } else {
                continue; // a library of the guest crates
            };
            let (install, boot) = guest_install(prefix, table)
                .ok_or_else(|| format!("{}: unknown [package.metadata.{prefix}]", package.name))?;
            let target = package
                .targets
                .iter()
                .find(|t| t.kind.iter().any(|k| k == "bin" || k == "cdylib"))
                .ok_or_else(|| format!("{}: no bin or cdylib target", package.name))?;
            let dir = self.target_dir.join(ANDROID_TARGET).join(ANDROID_PROFILE);
            let artifact = if target.kind.iter().any(|k| k == "cdylib") {
                dir.join(format!("lib{}.so", target.name.replace('-', "_")))
            } else {
                dir.join(&target.name)
            };
            let mut deps = vec![Dep::on("image")];
            if self.needs_generated_sources(id) {
                deps.push(Dep::on("aidl-gen"));
            }
            nodes.push(Node {
                name: format!("{prefix}/{}", package.name),
                deps,
                inputs: self.declared_inputs(id),
                outputs: vec![install.clone()],
                tools: vec![Tool::Rustc, Tool::Ndk],
                recipe: RECIPE,
                action: Action::Cargo(Unit {
                    batch: prefix.into(),
                    package: package.name.clone(),
                    bin: None,
                    artifact,
                    install: Some(install),
                }),
                boot,
            });
        }
        Ok(nodes)
    }
}

/// Where a guest package's metadata table installs it, and whether the image
/// needs it (test clients are not in the image).
fn guest_install(prefix: &str, table: &Value) -> Option<(PathBuf, bool)> {
    let field = |key: &str| table.get(key).and_then(Value::as_str);
    match prefix {
        "hal" => {
            if let Some(name) = field("binary") {
                Some((aim_paths::hal_bin(name), true))
            } else if let Some(name) = field("library") {
                Some((aim_paths::hal_lib(name), true))
            } else {
                field("test").map(|name| (aim_paths::hal_test(name), false))
            }
        }
        _ => field("program").map(|name| (aim_paths::daemon_bin(name), true)),
    }
}

/// Builds one batch of cargo units and installs guest artifacts. Returns
/// each unit's found inputs.
pub fn build(units: &[&Unit], ctx: &Ctx, log: &mut Log) -> Result<Vec<Vec<PathBuf>>, String> {
    let mut cmd = cargo();
    cmd.current_dir(aim_paths::root())
        .args(["build", "--locked"]);
    let guest = units[0].install.is_some();
    if guest {
        prepare_guest_links()?;
        cmd.args(["--target", ANDROID_TARGET, "--profile", ANDROID_PROFILE]);
    } else {
        cmd.arg("--release");
    }
    let mut packages = BTreeSet::new();
    for unit in units {
        packages.insert(unit.package.as_str());
    }
    for package in packages {
        cmd.args(["-p", package]);
    }
    for unit in units {
        if let Some(bin) = &unit.bin {
            cmd.args(["--bin", bin]);
        }
    }
    if !ctx.verbose {
        cmd.arg("--quiet");
    }
    log.run(&mut cmd)?;
    let mut found = Vec::new();
    for unit in units {
        if let Some(install) = &unit.install {
            install_file(&unit.artifact, install)?;
            log.line(&format!("installed {}", install.display()));
        }
        found.push(dep_info(&unit.artifact.with_extension("d"))?);
    }
    Ok(found)
}

/// The NDK linker wrapper `.cargo/config.toml` names and the image
/// libraries the guest crates link. Batches run at once share them.
fn prepare_guest_links() -> Result<(), String> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = LOCK.lock().unwrap();
    let clang = aim_paths::ndk_clang(ANDROID_API).ok_or_else(|| {
        format!(
            "the Android NDK {} is not installed",
            aim_paths::NDK_VERSION
        )
    })?;
    let wrapper =
        aim_paths::ndk_wrappers().join(format!("aarch64-linux-android{ANDROID_API}-clang"));
    let script = format!("#!/bin/sh\nexec '{}' \"$@\"\n", clang.display());
    write_if_changed(&wrapper, script.as_bytes(), 0o755)?;
    let image = fs::canonicalize(aim_paths::original_image())
        .map_err(|e| format!("{}: {e}", aim_paths::original_image().display()))?;
    let links = aim_paths::android_link_dir();
    fs::create_dir_all(&links).map_err(|e| e.to_string())?;
    for lib in ["libbinder_ndk.so", "libnativewindow.so"] {
        let link = links.join(lib);
        let target = image.join("system/lib64").join(lib);
        if fs::read_link(&link).ok().as_deref() != Some(target.as_path()) {
            let _ = fs::remove_file(&link);
            std::os::unix::fs::symlink(&target, &link).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub fn write_if_changed(path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    if fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let mut partial = path.as_os_str().to_owned();
    partial.push(format!(".partial-{}", std::process::id()));
    let partial = PathBuf::from(partial);
    fs::write(&partial, bytes).map_err(|e| format!("{}: {e}", partial.display()))?;
    fs::set_permissions(&partial, fs::Permissions::from_mode(mode)).map_err(|e| e.to_string())?;
    fs::rename(&partial, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Copies a built file into place (a new inode, never rewritten in place).
pub fn install_file(from: &Path, to: &Path) -> Result<(), String> {
    let bytes = fs::read(from).map_err(|e| format!("{}: {e}", from.display()))?;
    write_if_changed(to, &bytes, 0o755)
}

/// The repository sources a dep-info file lists, other than under `target/`
/// and `_build/`.
pub fn dep_info(path: &Path) -> Result<Vec<PathBuf>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let first = text.lines().next().unwrap_or_default();
    let Some((_, deps)) = first.split_once(": ") else {
        return Ok(Vec::new());
    };
    let mut paths = Vec::new();
    let mut current = String::new();
    let mut chars = deps.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => current.extend(chars.next()),
            ' ' => paths.push(std::mem::take(&mut current)),
            c => current.push(c),
        }
    }
    paths.push(current);
    Ok(repository_sources(
        paths
            .into_iter()
            .filter(|p| !p.is_empty())
            .map(PathBuf::from),
    ))
}

/// `paths` inside the repository but outside `target/` and `_build/`,
/// sorted and deduplicated.
pub fn repository_sources(paths: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let root = aim_paths::root();
    let excluded = [root.join("target"), aim_paths::fetched()];
    let mut out: Vec<PathBuf> = paths
        .into_iter()
        .filter(|p| p.starts_with(root) && !excluded.iter().any(|e| p.starts_with(e)))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// `path` without `.` and `..` components (lexically).
pub fn normalize(path: &Path) -> PathBuf {
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
