//! Static bionic executables: no interpreter, so bionic's static `__libc_init`
//! sets up TLS from AT_PHDR and reads AT_PAGESZ itself. Skipped when the
//! pinned NDK is not installed.
//!
//! bionic's static startup applies no relocations, so a static executable
//! must be ET_EXEC at its link address; `-static-pie` output other than
//! linker64 (which relocates itself) is not runnable on Android either. The
//! image's static executables link at 0x200000, which Darwin reserves, so
//! the fixture links above Darwin's reserved low range.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The pinned NDK's clang for the guest (arm64 Android), if installed.
fn ndk_clang() -> Option<PathBuf> {
    aim_paths::ndk_clang(35)
}

#[test]
fn static_executable_sets_up_tls_and_page_size() {
    let Some(clang) = ndk_clang() else {
        aim_paths::skip("the pinned NDK is not installed");
        return;
    };
    let root =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("static-{}", std::process::id()));
    std::fs::create_dir_all(root.join("bin")).unwrap();
    let built = Command::new(clang)
        .args(["-static", "-O1", "-Wl,--image-base=0x7100000000", "-o"])
        .arg(root.join("bin/static_tls"))
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/static_tls.c"))
        .status()
        .unwrap();
    assert!(built.success());
    let out = Command::new(env!("CARGO_BIN_EXE_linux-run"))
        .arg("--cache")
        .arg(root.join("cache"))
        .arg("--root")
        .arg(&root)
        .arg("/bin/static_tls")
        .output()
        .unwrap();
    let so = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{:?}\n{so}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(so, "page 16384 tls 42 7\n");
    std::fs::remove_dir_all(&root).unwrap();
}
