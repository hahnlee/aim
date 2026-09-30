//! `aidl-gen`: the AOSP sources of the guest crates and their AIDL crates.
//!
//! 1. Every tree of `hal/sources.lock` and `daemons/sources.lock` (AOSP's
//!    Rust binder crate, the libbinder_ndk headers, the daemons' AIDL
//!    sources) is fetched into `_build/aosp` and checked.
//! 2. Every frozen AIDL API of `hal/sources.lock` is fetched and checked
//!    against its own `.hash`, recomputed as Soong does.
//! 3. `tools/lib/vendor_hal_aidl.py` and `tools/lib/daemon_aidl.py` compile
//!    them with the pinned SDK `aidl --lang=rust` into the crates that
//!    `hal/aidl/*` and `daemons/aidl/*` point at (`target/aim/gen`).
//! 4. The framework AIDL of the native system services is generated into
//!    the crate `crates/aim-services/aidl` points at, and checked against
//!    the image ([`super::service_aidl`]).

use super::{files, real, repo, service_aidl};
use crate::fetch::Source;
use crate::graph::{Action, Dep, Node};
use crate::lockfile::Lock;
use crate::log::Log;
use crate::tools::{self, Tool};
use sha1::{Digest, Sha1};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn node() -> Node {
    let mut inputs = vec![
        repo("hal/sources.lock"),
        repo("daemons/sources.lock"),
        repo("tools/lib/vendor_hal_aidl.py"),
        repo("tools/lib/daemon_aidl.py"),
        repo(service_aidl::LOCK),
        // The generated codes are checked against the image's stubs.
        repo("image/original.lock"),
    ];
    // The generators check each crate's manifest against its imports.
    for dir in ["hal/aidl", "daemons/aidl"] {
        inputs.extend(files(dir).into_iter().filter(|p| p.ends_with("Cargo.toml")));
    }
    Node {
        name: "aidl-gen".into(),
        deps: vec![Dep::order_only("image")],
        inputs,
        outputs: vec![hal_out(), daemon_out(), service_aidl::out()],
        tools: vec![Tool::Aidl, Tool::Python],
        recipe: 3,
        action: Action::AidlGen,
        boot: true,
    }
}

fn hal_out() -> PathBuf {
    aim_paths::generated().join("hal-aidl")
}

fn daemon_out() -> PathBuf {
    aim_paths::generated().join("daemon-aidl")
}

/// Soong's check of a frozen API (aidlVerifyHashRule): sha1 over the
/// sorted `sha1  ./path` lines of its `.aidl` files and the previous
/// version (`latest-version` for V1), against the last line of `.hash`.
fn check_frozen(api: &Path, package: &str, version: u32) -> Result<(), String> {
    let hash_file = api.join(".hash");
    let text =
        fs::read_to_string(&hash_file).map_err(|e| format!("{}: {e}", hash_file.display()))?;
    let want = text.split_whitespace().last().unwrap_or_default();
    let mut names: Vec<String> = crate::hash::files_under(api)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "aidl"))
        .map(|p| format!("./{}", p.strip_prefix(api).unwrap().display()))
        .collect();
    names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    let mut lines = String::new();
    for name in names {
        let bytes = fs::read(api.join(&name[2..])).map_err(|e| e.to_string())?;
        lines.push_str(&format!(
            "{}  {name}\n",
            crate::hash::hex(&Sha1::digest(&bytes))
        ));
    }
    let previous = if version == 1 {
        "latest-version".to_string()
    } else {
        (version - 1).to_string()
    };
    lines.push_str(&format!("{previous}\n"));
    let got = crate::hash::hex(&Sha1::digest(lines.as_bytes()));
    if got != want {
        return Err(format!("{package} V{version}: API hash {got} != {want}"));
    }
    Ok(())
}

pub fn run(log: &mut Log) -> Result<(), String> {
    let (aidl, _) = tools::aidl()?;

    let hal = Lock::read(&repo("hal/sources.lock"))?;
    let tag = hal.get("AOSP_TAG")?;
    for entry in hal.array("SOURCE_TREES") {
        Source::parse(entry, tag)?.tree(log)?;
    }
    // An interface is compiled against the frozen APIs of its imports, so
    // every API is fetched and checked first.
    let apis = aim_paths::aosp().join("aidl_api");
    for entry in hal.array("AIDL_INTERFACES") {
        let fields: Vec<&str> = entry.split('|').collect();
        let [package, version, project, api, _imports] = fields[..] else {
            return Err(format!("bad AIDL_INTERFACES entry `{entry}`"));
        };
        let version: u32 = version
            .parse()
            .map_err(|_| format!("bad version in `{entry}`"))?;
        let dest = apis.join(package).join(version.to_string());
        let source = Source::parse(&format!("{project}|{api}|"), tag)?;
        source.extract(&dest, log)?;
        check_frozen(&dest, package, version)?;
    }
    log.run(
        Command::new("python3")
            .arg(repo("tools/lib/vendor_hal_aidl.py"))
            .arg(&aidl)
            .arg(real(&apis)?)
            .arg(real(&hal_out())?)
            .arg(repo("hal/aidl"))
            .args(hal.array("AIDL_INTERFACES")),
    )?;

    let daemons = Lock::read(&repo("daemons/sources.lock"))?;
    let tag = daemons.get("AOSP_TAG")?;
    for entry in daemons.array("SOURCE_TREES") {
        Source::parse(entry, tag)?.tree(log)?;
    }
    log.run(
        Command::new("python3")
            .arg(repo("tools/lib/daemon_aidl.py"))
            .arg(&aidl)
            .arg(real(&aim_paths::aosp())?)
            .arg(real(&daemon_out())?)
            .arg(repo("daemons/aidl"))
            .args(daemons.array("AIDL_CRATES")),
    )?;
    service_aidl::run(log)
}
