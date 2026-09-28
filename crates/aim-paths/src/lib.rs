//! Where `cargo aim` (crates/aim-build, docs/build.md) keeps what it fetches
//! and what it builds. The build, the guest crates' build scripts and the
//! tests all find their inputs here, so there is one layout:
//!
//! - `_build/`: large fetched inputs, each verified against a pin: the
//!   extracted original image, the AOSP trees (`_build/aosp`), the ANGLE
//!   checkout and its build. A worktree may link them from another
//!   checkout.
//! - `target/aim/`: every build output, per node; `target/aim-cache/`: the
//!   node stamps.
//!
//! The paths are fixed relative to the repository root (the checked-in
//! manifests name some of them), whatever cargo's own target directory is.

use std::path::{Path, PathBuf};

/// The pinned NDK (r28c): the guest crates' linker and bindgen sysroot, the
/// ART exception's compiler and the tests' NDK programs.
pub const NDK_VERSION: &str = "28.2.13676358";

/// The repository root.
pub fn root() -> &'static Path {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .expect("crates/aim-paths")
}

/// Large fetched inputs.
pub fn fetched() -> PathBuf {
    root().join("_build")
}

/// AOSP trees fetched at the image's tag: `<project without platform/>/<subtree>`.
pub fn aosp() -> PathBuf {
    fetched().join("aosp")
}

/// Downloaded archives, kept so a refetch needs no network.
pub fn downloads() -> PathBuf {
    fetched().join("downloads")
}

/// The extracted original image (read-only; `image` node). In a worktree it
/// may be a link: resolve it before handing it to a tool that writes beside
/// or into its argument.
pub fn original_image() -> PathBuf {
    fetched().join("android16-image-full")
}

/// The ANGLE checkout (`angle` node).
pub fn angle_source() -> PathBuf {
    fetched().join("angle-source")
}

/// Every build output, one directory per node.
pub fn out() -> PathBuf {
    root().join("target/aim")
}

/// Node stamps.
pub fn cache() -> PathBuf {
    root().join("target/aim-cache")
}

/// Generated sources (the AIDL crates the checked-in manifests point at).
pub fn generated() -> PathBuf {
    out().join("gen")
}

/// The image libraries the guest crates link (`libbinder_ndk.so`, ...).
pub fn android_link_dir() -> PathBuf {
    out().join("link")
}

/// Wrappers around the pinned NDK's tools, which `.cargo/config.toml` names.
pub fn ndk_wrappers() -> PathBuf {
    out().join("ndk")
}

/// A vendor HAL service (`/vendor/bin/hw`).
pub fn hal_bin(name: &str) -> PathBuf {
    out().join("hal/bin").join(name)
}

/// A vendor HAL driver library.
pub fn hal_lib(name: &str) -> PathBuf {
    out().join("hal/lib").join(name)
}

/// A HAL's test client, run by the crate tests from `/data/local/tmp`.
pub fn hal_test(name: &str) -> PathBuf {
    out().join("hal/test").join(name)
}

/// A replaced native daemon.
pub fn daemon_bin(name: &str) -> PathBuf {
    out().join("daemons/bin").join(name)
}

/// The ART exception build (`art` node): `stripped/{lib64,bin}` go into the
/// image.
pub fn art() -> PathBuf {
    out().join("art")
}

/// The regenerated boot image (`boot-image` node): `framework/`.
pub fn boot_image() -> PathBuf {
    out().join("boot-image")
}

/// ANGLE's Metal build (`angle` node): `libEGL.dylib`, `libGLESv2.dylib`.
/// It stays in the checkout's own `out/` (see docs/build.md, "ANGLE").
pub fn angle() -> PathBuf {
    angle_source().join("out/AimRelease")
}

/// The derived image of `image/overlay.toml` (`derived-image` node).
pub fn derived_image() -> PathBuf {
    out().join("derived-image")
}

/// Markers of tests that skipped for a missing input (see [`skip`]).
pub fn test_skips() -> PathBuf {
    out().join("test-skips")
}

/// The Android SDK: `ANDROID_SDK_ROOT`, else `~/Library/Android/sdk`.
pub fn sdk() -> Option<PathBuf> {
    std::env::var_os("ANDROID_SDK_ROOT")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join("Library/Android/sdk")))
}

/// The pinned NDK, if installed.
pub fn ndk() -> Option<PathBuf> {
    let ndk = sdk()?.join("ndk").join(NDK_VERSION);
    ndk.join("source.properties").exists().then_some(ndk)
}

/// The pinned NDK's LLVM toolchain.
pub fn ndk_toolchain() -> Option<PathBuf> {
    Some(ndk()?.join("toolchains/llvm/prebuilt/darwin-x86_64"))
}

/// The NDK's clang driver for aarch64-linux-android at `api`.
pub fn ndk_clang(api: u32) -> Option<PathBuf> {
    let clang = ndk_toolchain()?.join(format!("bin/aarch64-linux-android{api}-clang"));
    clang.exists().then_some(clang)
}

/// `path` with every link resolved (a tool gets the real path of its
/// input, never a worktree's link), if it exists; otherwise the test
/// skips, naming the `cargo aim` node that builds it.
pub fn input(path: PathBuf, node: &str) -> Option<PathBuf> {
    if let Ok(real) = std::fs::canonicalize(&path) {
        return Some(real);
    }
    skip(&format!(
        "no {} (cargo aim build {node})",
        path.strip_prefix(root()).unwrap_or(&path).display()
    ));
    None
}

/// The extracted original image, resolved, if it holds `file`; otherwise
/// the test skips.
pub fn original_image_with(file: &str) -> Option<PathBuf> {
    input(original_image().join(file), "image")?;
    input(original_image(), "image")
}

/// Records that the running test skipped for a missing input. Plain
/// `cargo test` passes such a test; `cargo aim test`, which builds every
/// input first, fails it.
pub fn skip(reason: &str) {
    eprintln!("skipped: {reason}");
    let exe = std::env::current_exe().ok();
    let exe = exe
        .as_deref()
        .and_then(Path::file_name)
        .map_or("test".into(), |name| name.to_string_lossy());
    let thread = std::thread::current();
    let test = thread.name().unwrap_or("main").replace("::", "-");
    let dir = test_skips();
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::write(
            dir.join(format!("{exe}.{test}.{}", std::process::id())),
            format!("{exe} {test}: {reason}\n"),
        );
    }
}
