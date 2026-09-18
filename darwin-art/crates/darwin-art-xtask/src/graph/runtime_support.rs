//! Independent production Java payload edge. These inputs deliberately never
//! participate in the native runtime's global cache digest.
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::atomic;

fn invalid(message: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_string())
}

fn collect_files(directory: &Path, inputs: &mut BTreeSet<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.file_name().is_some_and(|name| {
            matches!(name.to_str(), Some(".git" | ".DS_Store" | "test" | "tests"))
        }) {
            continue;
        }
        if path.is_dir() {
            collect_files(&path, inputs)?;
        } else if path.is_file() {
            inputs.insert(path);
        }
    }
    Ok(())
}
use crate::{ninja_path, shell_quote};
use darwin_art_build_contract::support_java::{SOURCE_MANIFEST, production_sources};

fn executable(name: &str) -> io::Result<PathBuf> {
    for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
        let path = directory.join(name);
        if path.is_file() {
            return fs::canonicalize(path);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("support tool missing: {name}"),
    ))
}

pub(crate) fn emit(graph: &mut String, root: &Path) -> io::Result<()> {
    let mut inputs = BTreeSet::new();
    inputs.insert(root.join(SOURCE_MANIFEST));
    for source in
        production_sources(&fs::read_to_string(root.join(SOURCE_MANIFEST))?).map_err(invalid)?
    {
        inputs.insert(root.join(source));
    }
    // Actual compile signatures, adapter implementations, verifier and its
    // pinned AOSP foundation closure; no generated output/fixture directory.
    for directory in [
        "runtime/framework/compile-stubs",
        "runtime/framework/pm",
        "_aosp/art/libartbase",
        "_aosp/art/libdexfile",
        "_aosp/system/libbase",
        "_aosp/system/libziparchive",
        "_aosp/external/tinyxml2",
        "compat/filesystem",
    ] {
        collect_files(&root.join(directory), &mut inputs)?;
    }
    for file in [
        "tools/dex-inspect.cc",
        "_aosp/art/tools/generate_operator_out.py",
        "tools/build-android16-package-dex-usage.sh",
        "compat/android_base_logging.cc",
        "sources.lock",
        "patches/art/0002-darwin-dynamic-page-size.patch",
        "patches/art/0020-darwin-low4g-mach-reservation.patch",
        "patches/art/0021-darwin-compressed-reference-window.patch",
        "patches/art/0094-darwin-thread-cpu-nanotime.patch",
        "patches/art/0102-darwin-logical-pthread-names.patch",
        "patches/art/0112-darwin-artbase-private-paths.patch",
        "patches/art/0039-darwin-memmap-exact-anonymous.patch",
    ] {
        inputs.insert(root.join(file));
    }
    let sdk = env::var_os("ANDROID_SDK_ROOT")
        .or_else(|| env::var_os("ANDROID_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env::var_os("HOME").unwrap_or_default()).join("Library/Android/sdk")
        });
    let mut d8_candidates: Vec<_> = fs::read_dir(sdk.join("build-tools"))?
        .map(|entry| entry.map(|entry| entry.path().join("d8")))
        .collect::<io::Result<_>>()?;
    d8_candidates.retain(|path| path.is_file());
    d8_candidates.sort();
    let d8 = d8_candidates
        .pop()
        .ok_or_else(|| invalid("no D8 build tool"))?;
    let javac = executable("javac")?;
    let java_home = javac
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| invalid("invalid JDK tool path"))?;
    let mut tools = vec![
        ("JAVAC", javac.clone()),
        ("JAR", java_home.join("bin/jar")),
        ("JAVAP", java_home.join("bin/javap")),
        ("D8", d8.clone()),
        ("PLATFORM", sdk.join("platforms/android-36/android.jar")),
        (
            "CORE",
            sdk.join("platforms/android-36/core-for-system-modules.jar"),
        ),
        ("CLANG", executable("clang++")?),
        ("PYTHON", executable("python3")?),
    ];
    inputs.insert(d8.parent().unwrap().join("lib/d8.jar"));
    inputs.insert(java_home.join("lib/modules"));
    inputs.insert(java_home.join("bin/java"));
    collect_files(
        Path::new("/opt/homebrew/opt/openjdk@17/include"),
        &mut inputs,
    )?;
    let sdk_output = Command::new("xcrun").args(["--show-sdk-path"]).output()?;
    if !sdk_output.status.success() {
        return Err(invalid("cannot resolve native SDK"));
    }
    let native_sdk = String::from_utf8(sdk_output.stdout)
        .map_err(invalid)?
        .trim()
        .to_owned();
    let mut identity = format!("JAVA_HOME={}\nSDKROOT={native_sdk}\n", java_home.display());
    let mut environment = format!(
        "JAVA_HOME={} SDKROOT={} PATH={} ",
        shell_quote(&java_home.to_string_lossy()),
        shell_quote(&native_sdk),
        shell_quote(&format!(
            "{}:{}",
            java_home.join("bin").display(),
            env::var("PATH").unwrap_or_default()
        ))
    );
    for (name, path) in tools.drain(..) {
        if !path.is_file() {
            return Err(invalid(format!(
                "missing support input: {}",
                path.display()
            )));
        }
        inputs.insert(path.clone());
        identity.push_str(&format!("{name}={}\n", path.display()));
        environment.push_str(&format!(
            "DARWIN_ART_SUPPORT_{name}={} ",
            shell_quote(&path.to_string_lossy())
        ));
    }
    for tool in [&javac, &executable("clang++")?, &executable("python3")?] {
        let output = Command::new(tool).arg("--version").output()?;
        if !output.status.success() {
            return Err(invalid("support tool version failed"));
        }
        identity.push_str(&String::from_utf8_lossy(&output.stdout));
        identity.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    let metadata = root.join("_build/native-graph");
    let sdk_settings = Path::new(&native_sdk).join("SDKSettings.json");
    identity.push_str(&format!(
        "SDK-settings={:x}\n",
        Sha256::digest(fs::read(&sdk_settings)?)
    ));
    inputs.insert(sdk_settings);
    environment.push_str(&format!(
        "DARWIN_ART_SUPPORT_SDK={} DARWIN_ART_SUPPORT_TOOLCHAIN_IDENTITY={:x} ",
        shell_quote(&native_sdk),
        Sha256::digest(identity.as_bytes())
    ));
    let identity_path = metadata.join("support-tools.identity");
    atomic::write(&identity_path, identity.as_bytes())?;
    inputs.insert(identity_path);
    for path in &inputs {
        if !path.is_file() {
            return Err(invalid(format!(
                "missing support input: {}",
                path.display()
            )));
        }
    }
    let snapshot = metadata.join("support-inputs.txt");
    atomic::write(
        &snapshot,
        inputs
            .iter()
            .map(|path| format!("{}\n", path.display()))
            .collect::<String>()
            .as_bytes(),
    )?;
    environment.push_str(&format!(
        "DARWIN_ART_SUPPORT_INPUTS={} ",
        shell_quote(&snapshot.to_string_lossy())
    ));
    let cli = root.join("target/debug/art-bootstrap");
    let output = ninja_path(&root.join("_build/runtime-support-dex/dex/classes.dex"));
    // Ninja escapes dollars even inside shell quotes.
    let command = format!(
        "{environment}{} build-runtime-support-dex",
        shell_quote(&cli.to_string_lossy())
    )
    .replace('$', "$$");
    graph.push_str(&format!("\nrule runtime_support_dex\n  command = {command}\n  depfile = $out.d\n  deps = gcc\n  description = Build production Android support DEX\nbuild {output}: runtime_support_dex {} {}", ninja_path(&cli), ninja_path(&snapshot)));
    for input in inputs {
        graph.push(' ');
        graph.push_str(&ninja_path(&input));
    }
    graph.push_str(&format!("\nbuild runtime-support-dex: phony {output}\n"));
    graph.push_str("build runtime-payload: phony graphics-audit runtime-support-dex\n");
    Ok(())
}
