//! The pinned image's libcrypto (a BoringSSL FIPS module) passes its
//! integrity self-test after translation and after load-time rewriting, so
//! the original toybox can use it. Skipped when the extracted image is
//! absent.

use std::path::{Path, PathBuf};
use std::process::Command;

use aim_linux_abi::cache::{Cache, remove_tree};
use aim_linux_abi::xlate;

const INPUT: &str = "/system/etc/hosts";

/// The extracted pinned image (the `image` node of `cargo aim`).
fn image() -> Option<PathBuf> {
    aim_paths::original_image_with("system/lib64/libcrypto.so")
}

fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}-{}", std::process::id()));
    let _ = remove_tree(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `toybox sha256sum` of the input, which links libcrypto.
fn check_sha256sum(image: &Path, cache: &Path) {
    let out = Command::new(env!("CARGO_BIN_EXE_linux-run"))
        .arg("--cache")
        .arg(cache)
        .arg("--root")
        .arg(image)
        .args(["/system/bin/toybox", "sha256sum", INPUT])
        .output()
        .unwrap();
    let (so, se) = (
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(!se.contains("FIPS integrity test failed"), "{se}");
    assert!(out.status.success(), "{:?}\n{so}\n{se}", out.status);
    let bytes = std::fs::read(image.join(INPUT.trim_start_matches('/'))).unwrap();
    assert_eq!(so, format!("{}  {INPUT}\n", xlate::sha256_hex(&bytes)));
}

#[test]
fn libcrypto_passes_its_fips_integrity_test() {
    let Some(ref image) = image() else { return };

    // No cache: libcrypto is rewritten, and its hash recomputed, at load time.
    let empty = scratch("fips-empty-cache");
    check_sha256sum(image, &empty);

    // libcrypto from the cache, its hash re-injected by the translator.
    let dir = scratch("fips-cache");
    let cache = Cache::new(&dir);
    let lib = image
        .join("system/lib64/libcrypto.so")
        .canonicalize()
        .unwrap();
    let r = cache
        .translate_file(&lib, &xlate::Options::default())
        .unwrap()
        .unwrap();
    assert_eq!(r.kind, "translated");
    assert!(r.report.unwrap().fips_rehashed);
    check_sha256sum(image, &dir);

    remove_tree(&empty).unwrap();
    remove_tree(&dir).unwrap();
}
