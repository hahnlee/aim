//! A system image's own translation cache (docs/storage.md): `<volume>/root`
//! is the guest root and `<volume>/translated` its cache, indexed by path
//! relative to the root. linux-run looks there before the user's cache, so
//! with an empty user cache the image's files run with no load-time
//! rewriting. The volume here is a small copy of the pinned image's files
//! linkerconfig needs; skipped when the extracted image is absent.

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

const LINKER: &str = "/apex/com.android.runtime/bin/linker64";
const LINKERCONFIG: &str = "/apex/com.android.runtime/bin/linkerconfig";

/// Regular files copied from the image.
const FILES: &[&str] = &[
    "apex/com.android.runtime/bin/linker64",
    "apex/com.android.runtime/bin/linkerconfig",
    "apex/com.android.runtime/lib64/bionic/libc.so",
    "apex/com.android.runtime/lib64/bionic/libm.so",
    "apex/com.android.runtime/lib64/bionic/libdl.so",
    "system/lib64/libbase.so",
    "system/lib64/libc++.so",
    "system/lib64/liblog.so",
    "system/lib64/libnetd_client.so",
];
/// Links as in the image.
const LINKS: &[(&str, &str)] = &[
    (
        "system/lib64/libc.so",
        "/apex/com.android.runtime/lib64/bionic/libc.so",
    ),
    (
        "system/lib64/libm.so",
        "/apex/com.android.runtime/lib64/bionic/libm.so",
    ),
    (
        "system/lib64/libdl.so",
        "/apex/com.android.runtime/lib64/bionic/libdl.so",
    ),
];

fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
    let _ = aim_linux_abi::cache::remove_tree(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A volume whose `root` holds [`FILES`] and [`LINKS`] of `image`.
fn volume(image: &Path) -> PathBuf {
    let vol = scratch("image-volume");
    let root = vol.join("root");
    for file in FILES {
        let to = root.join(file);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(image.join(file), &to).unwrap();
    }
    for (link, target) in LINKS {
        symlink(target, root.join(link)).unwrap();
    }
    std::fs::create_dir_all(root.join("vendor/lib64")).unwrap();
    vol
}

/// Files the run translated at load time (the loader's program and
/// interpreter, and every executable mapping no cache had), and the trace.
fn load_time_rewrites(root: &Path, user_cache: &Path) -> (usize, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_linux-run"))
        .arg("--cache")
        .arg(user_cache)
        .arg("--root")
        .arg(root)
        .args(["--trace", LINKER, LINKERCONFIG, "--help"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{:?}\n{err}", out.status);
    assert!(err.contains("Usage : linkerconfig"), "{err}");
    let count = err
        .lines()
        .filter(|l| l.contains("from load-time rewrite") || l.contains("load-time rewrite of "))
        .count();
    (count, err)
}

#[test]
fn image_cache_serves_the_image_with_an_empty_user_cache() {
    let Some(image) = aim_paths::original_image_with(LINKER.trim_start_matches('/')) else {
        return;
    };
    let vol = volume(&image);
    let root = vol.join("root");
    let user = scratch("empty-user-cache");

    // No image cache: everything is rewritten at load time.
    let (count, trace) = load_time_rewrites(&root, &user);
    assert!(
        count >= FILES.len() - 1,
        "{count} load-time rewrites:\n{trace}"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_linux-translate"))
        .arg("--image")
        .arg(&vol)
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "linux-translate --image:\n{report}");
    assert!(vol.join("translated/paths").is_dir(), "{report}");

    // The first run recorded its load-time sites in the user cache.
    let user = scratch("empty-user-cache-2");
    let (count, trace) = load_time_rewrites(&root, &user);
    assert_eq!(
        count, 0,
        "load-time rewrites with the image cache:\n{trace}"
    );
    assert!(trace.contains("from translation cache"), "{trace}");
    assert!(
        std::fs::read_dir(&user).unwrap().next().is_none(),
        "the user cache stays empty"
    );

    // The same volume elsewhere (another mount point) still hits.
    let moved = scratch("image-volume-moved");
    std::fs::remove_dir(&moved).unwrap();
    std::fs::rename(&vol, &moved).unwrap();
    let (count, trace) = load_time_rewrites(&moved.join("root"), &user);
    assert_eq!(count, 0, "after a move:\n{trace}");

    aim_linux_abi::cache::remove_tree(&moved).unwrap();
    aim_linux_abi::cache::remove_tree(&user).unwrap();
}

/// The same on the real images: the system image and the derived image
/// over it carry their own caches, so an empty user cache is enough.
#[test]
fn the_built_images_need_no_runtime_translation() {
    for (root, node) in [
        (aim_paths::original_image(), "image"),
        (aim_paths::derived_image(), "derived-image"),
    ] {
        let Some(root) = aim_paths::input(root, node) else {
            return;
        };
        let user = scratch(&format!("empty-user-cache-{node}"));
        let (count, trace) = load_time_rewrites(&root, &user);
        assert_eq!(count, 0, "{}:\n{trace}", root.display());
        assert!(
            std::fs::read_dir(&user).unwrap().next().is_none(),
            "the user cache stays empty"
        );
        aim_linux_abi::cache::remove_tree(&user).unwrap();
    }
}
