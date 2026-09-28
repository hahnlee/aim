//! The external tools nodes run, and their versions, which are part of the
//! keys of the nodes that use them.

use crate::hash;
use crate::lockfile::Lock;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// A tool whose version a node's key includes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tool {
    Rustc,
    Ndk,
    Aidl,
    Python,
    Java,
    N2,
    /// The system ninja, for ANGLE only (docs/build.md, "n2").
    Ninja,
}

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Tool::Rustc => "rustc",
            Tool::Ndk => "ndk",
            Tool::Aidl => "aidl",
            Tool::Python => "python3",
            Tool::Java => "jdk",
            Tool::N2 => "n2",
            Tool::Ninja => "ninja",
        }
    }
}

/// The n2 revision `cargo aim` runs ninja files with (crates/aim-build/Cargo.toml).
const N2_REVISION: &str = "b1fead52ccda0c497d816696f23f4099c3e8ec1f";

#[derive(Default)]
pub struct Tools {
    versions: Mutex<HashMap<Tool, Result<String, String>>>,
}

impl Tools {
    /// The tool's version string (hashed into keys), found once per run.
    pub fn version(&self, tool: Tool) -> Result<String, String> {
        if let Some(found) = self.versions.lock().unwrap().get(&tool) {
            return found.clone();
        }
        let found = probe(tool);
        self.versions.lock().unwrap().insert(tool, found.clone());
        found
    }
}

/// Standard output and error (`java -version` writes to stderr).
fn output(cmd: &mut Command) -> Result<(String, String), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    if !out.status.success() {
        return Err(format!("{:?}: {}", cmd.get_program(), out.status));
    }
    Ok((
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

fn stdout(cmd: &mut Command) -> Result<String, String> {
    output(cmd).map(|(out, _)| out)
}

fn probe(tool: Tool) -> Result<String, String> {
    match tool {
        Tool::Rustc => stdout(Command::new("rustc").arg("-vV")),
        Tool::Ndk => {
            let ndk = ndk()?;
            std::fs::read_to_string(ndk.join("source.properties")).map_err(|e| e.to_string())
        }
        Tool::Aidl => aidl().map(|(_, sha)| sha),
        Tool::Python => stdout(Command::new("python3").arg("--version")),
        Tool::Java => output(Command::new(jdk()?.join("bin/java")).arg("-version"))
            .map(|(out, err)| out + &err),
        Tool::N2 => Ok(N2_REVISION.into()),
        Tool::Ninja => stdout(Command::new("ninja").arg("--version")),
    }
}

pub fn ndk() -> Result<PathBuf, String> {
    aim_paths::ndk().ok_or_else(|| {
        format!(
            "the Android NDK {} is not installed (sdkmanager \"ndk;{}\")",
            aim_paths::NDK_VERSION,
            aim_paths::NDK_VERSION
        )
    })
}

pub fn ndk_toolchain() -> Result<PathBuf, String> {
    Ok(ndk()?.join("toolchains/llvm/prebuilt/darwin-x86_64"))
}

/// The SDK's `aidl`, checked against the pin of hal/sources.lock (the
/// generated code is the compiler's output, so the compiler is pinned too).
pub fn aidl() -> Result<(PathBuf, String), String> {
    let lock = Lock::read(&aim_paths::root().join("hal/sources.lock"))?;
    let sdk = aim_paths::sdk().ok_or("no Android SDK")?;
    let aidl = sdk.join(lock.get("AIDL_COMPILER")?);
    let want = lock.get("AIDL_COMPILER_SHA256")?;
    let got = hash::sha256_file(&aidl).map_err(|e| format!("{}: {e}", aidl.display()))?;
    if got != want {
        return Err(format!(
            "{} is not the pinned compiler (sha256 {want})",
            aidl.display()
        ));
    }
    Ok((aidl, got))
}

/// JDK 17 (Homebrew `openjdk@17`, else `java_home -v 17`).
pub fn jdk() -> Result<PathBuf, String> {
    let brew = Path::new("/opt/homebrew/opt/openjdk@17");
    if brew.join("bin/javac").exists() {
        return Ok(brew.to_path_buf());
    }
    let home = stdout(Command::new("/usr/libexec/java_home").args(["-v", "17"]))
        .map_err(|_| "JDK 17 not found (brew install openjdk@17)".to_string())?;
    Ok(PathBuf::from(home.trim()))
}
