//! The cargo nodes, derived from `cargo metadata`, so a new crate needs no
//! edit here:
//!
//! - `host/<bin>`: every binary of the host crates (the workspace's
//!   default members), built with `--release` in a private Cargo directory
//!   and published as detached executable vnodes;
//! - `hal/<package>` and `daemon/<package>`: every guest package with a
//!   `[package.metadata.vendor-hal]` or `[package.metadata.daemon]` table,
//!   built for aarch64-linux-android with the `android` profile and
//!   installed where the table says.
//!
//! A cargo node's declared inputs are the manifests and build scripts of
//! the local packages it depends on, `Cargo.lock` and `.cargo/config.toml`;
//! its found inputs are the source files of cargo's dep-info for the
//! artifact, the sources aidl-gen generates or fetches among them: a node
//! whose dependencies include such a package runs after aidl-gen (order
//! only), and is stale only when the generated files it compiles changed,
//! not whenever another AIDL crate did.

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
const HOST_RECIPE: u32 = 2;

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
                    let mut deps = if self.needs_generated_sources(id) {
                        vec![Dep::order_only("aidl-gen")]
                    } else {
                        Vec::new()
                    };
                    if target.name == "guest-init" {
                        deps.push(Dep::order_only("host/aim-lock-holder"));
                    }
                    if target.name == "linux-run" { deps.push(Dep::order_only("host/aim-pty-holder")); }
                    nodes.push(Node {
                        name: format!("host/{}", target.name),
                        deps,
                        inputs: self.declared_inputs(id),
                        outputs: vec![artifact.clone()],
                        tools: vec![Tool::Rustc],
                        recipe: HOST_RECIPE,
                        action: Action::Cargo(Unit {
                            batch: "host".into(),
                            package: package.name.clone(),
                            bin: Some(target.name.clone()),
                            artifact: self
                                .target_dir
                                .join("aim-host-build/release")
                                .join(&target.name),
                            install: Some(artifact),
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
                deps.push(Dep::order_only("aidl-gen"));
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

/// Builds one batch of cargo units and publishes its artifacts. Returns
/// each unit's found inputs.
pub fn build(units: &[&Unit], ctx: &Ctx, log: &mut Log) -> Result<Vec<Vec<PathBuf>>, String> {
    let mut cmd = cargo();
    cmd.current_dir(aim_paths::root())
        .args(["build", "--locked"]);
    let guest = units[0].batch != "host";
    if guest {
        prepare_guest_links()?;
        cmd.args(["--target", ANDROID_TARGET, "--profile", ANDROID_PROFILE]);
    } else {
        // Cargo may replace or relink its own artifacts. They must never be
        // the executable vnodes currently running a guest (#1146).
        let staging = units[0].artifact.parent().unwrap().parent().unwrap();
        cmd.arg("--release").arg("--target-dir").arg(staging);
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
            if guest {
                install_file(&unit.artifact, install)?;
            } else {
                publish_executable(&unit.artifact, install)?;
            }
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

/// Publish a detached executable vnode, keeping every running old generation
/// immutable. Never hardlink a Cargo artifact or rewrite a public executable.
fn publish_executable(from: &Path, to: &Path) -> Result<(), String> {
    use std::io;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = to
        .parent()
        .ok_or_else(|| format!("{}: no executable directory", to.display()))?;
    fs::create_dir_all(parent).map_err(|error| format!("{}: {error}", parent.display()))?;
    let (partial, mut output) = loop {
        let mut name = to.as_os_str().to_owned();
        name.push(format!(
            ".publish-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let path = PathBuf::from(name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&path)
        {
            Ok(file) => break (path, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    };
    let result = (|| {
        let mut source =
            fs::File::open(from).map_err(|error| format!("{}: {error}", from.display()))?;
        io::copy(&mut source, &mut output)
            .map_err(|error| format!("{}: {error}", partial.display()))?;
        output
            .set_permissions(fs::Permissions::from_mode(0o755))
            .map_err(|error| error.to_string())?;
        output.sync_all().map_err(|error| error.to_string())?;
        drop(output);
        fs::rename(&partial, to).map_err(|error| format!("{}: {error}", to.display()))
    })();
    match result {
        Ok(()) => Ok(()),
        Err(error) => match fs::remove_file(&partial) {
            Ok(()) => Err(error),
            Err(cleanup) => Err(format!("{error}; remove {}: {cleanup}", partial.display())),
        },
    }
}

/// The repository sources a dep-info file lists: those outside `target/`
/// and `_build/`, and the ones aidl-gen generates or fetches.
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
    let generated = [aim_paths::generated(), aim_paths::aosp()];
    let (made, rest): (Vec<PathBuf>, Vec<PathBuf>) = paths
        .into_iter()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .partition(|p| generated.iter().any(|g| p.starts_with(g)));
    let mut sources = repository_sources(rest);
    sources.extend(made);
    sources.sort();
    sources.dedup();
    Ok(sources)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_nodes_separate_cargo_artifacts_from_public_commands() {
        let id = "fixture".to_owned();
        let workspace = Workspace {
            target_dir: PathBuf::from("fixture-target"),
            packages: HashMap::from([(
                id.clone(),
                Package {
                    name: "fixture-host".into(),
                    manifest: "Cargo.toml".into(),
                    local: true,
                    targets: vec![Target {
                        name: "fixture-bin".into(),
                        kind: vec!["bin".into()],
                        src_path: "src/main.rs".into(),
                    }],
                    metadata: Value::Null,
                    build_script: None,
                    deps: Vec::new(),
                },
            )]),
            members: vec![id.clone()],
            default_members: BTreeSet::from([id]),
        };
        let nodes = workspace.nodes().unwrap();
        assert_eq!(nodes.len(), 1);
        let public = workspace.host_bin("fixture-bin");
        assert_eq!(nodes[0].outputs, vec![public.clone()]);
        assert_eq!(nodes[0].recipe, HOST_RECIPE);
        let Action::Cargo(unit) = &nodes[0].action else {
            panic!("host Cargo node expected");
        };
        assert_eq!(unit.batch, "host");
        assert_eq!(unit.install.as_ref(), Some(&public));
        assert_eq!(
            unit.artifact,
            workspace
                .target_dir
                .join("aim-host-build/release/fixture-bin")
        );
        assert_ne!(unit.artifact, public);
    }

    #[test]
    fn executable_publication_detaches_compiler_and_running_generations() {
        use std::io::{Read, Seek};
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let directory =
            std::env::temp_dir().join(format!("aim-executable-publication-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let compiler = directory.join("compiler");
        let public = directory.join("public");
        fs::write(&compiler, b"original signed bytes").unwrap();
        publish_executable(&compiler, &public).unwrap();
        let mut running = fs::File::open(&public).unwrap();
        let original = running.metadata().unwrap();
        assert_ne!(original.ino(), fs::metadata(&compiler).unwrap().ino());
        // Even identical-byte publication creates a new vnode during migration.
        publish_executable(&compiler, &public).unwrap();
        assert_ne!(original.ino(), fs::metadata(&public).unwrap().ino());
        assert_eq!(fs::read(&public).unwrap(), b"original signed bytes");
        fs::write(&compiler, b"next signed bytes").unwrap();
        assert_eq!(fs::read(&public).unwrap(), b"original signed bytes");
        publish_executable(&compiler, &public).unwrap();
        running.rewind().unwrap();
        let mut retained = Vec::new();
        running.read_to_end(&mut retained).unwrap();
        assert_eq!(retained, b"original signed bytes");
        assert_eq!(fs::read(&public).unwrap(), b"next signed bytes");
        assert_eq!(
            fs::metadata(&public).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(publish_executable(&directory.join("missing"), &public).is_err());
        assert_eq!(fs::read(&public).unwrap(), b"next signed bytes");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 2);
        drop(running);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn published_macho_preserves_signature_and_executes() {
        use sha2::{Digest, Sha256};
        use std::os::unix::fs::MetadataExt;
        let source = std::env::current_exe().unwrap();
        let directory =
            std::env::temp_dir().join(format!("aim-signed-publication-{}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let public = directory.join("published-test");
        publish_executable(&source, &public).unwrap();
        assert_ne!(
            fs::metadata(&source).unwrap().ino(),
            fs::metadata(&public).unwrap().ino()
        );
        assert_eq!(
            Sha256::digest(fs::read(&source).unwrap()),
            Sha256::digest(fs::read(&public).unwrap())
        );
        let signature = Command::new("/usr/bin/codesign")
            .args(["--verify", "--strict", "--verbose=2"])
            .arg(&public)
            .output()
            .unwrap();
        assert!(
            signature.status.success(),
            "signed publication: {}",
            String::from_utf8_lossy(&signature.stderr)
        );
        let executed = Command::new(&public).arg("--list").output().unwrap();
        assert!(
            executed.status.success(),
            "published Mach-O exec: {:?} {}",
            executed.status,
            String::from_utf8_lossy(&executed.stderr)
        );
        assert!(
            String::from_utf8(executed.stdout)
                .unwrap()
                .contains("published_macho_preserves_signature_and_executes")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn dep_info_keeps_generated_sources() {
        let root = aim_paths::root();
        let [source, generated, fetched, built, staged] = [
            root.join("crates/x/src/lib.rs"),
            aim_paths::generated().join("hal-aidl/x/lib.rs"),
            aim_paths::aosp().join("binder/src/lib.rs"),
            root.join("target/release/build/x/out/y.rs"),
            root.join("target/aim-host-build/release/build/x/out/y.rs"),
        ];
        let file = std::env::temp_dir().join(format!("aim-dep-info-{}.d", std::process::id()));
        let listed = [
            &source,
            &generated,
            &fetched,
            &built,
            &staged,
            &PathBuf::from("/usr/x.h"),
        ]
        .map(|p| p.display().to_string().replace(' ', "\\ "));
        fs::write(
            &file,
            format!("{}: {}\n", built.display(), listed.join(" ")),
        )
        .unwrap();
        let mut want = vec![source, generated, fetched];
        want.sort();
        assert_eq!(dep_info(&file).unwrap(), want);
        fs::remove_file(&file).unwrap();
    }
}
