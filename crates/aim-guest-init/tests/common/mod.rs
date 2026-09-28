//! Test images and helpers.
//!
//! The image is `aim-android-init`'s upstream fixture (android-16.0.0_r1
//! init.rc, logd.rc, servicemanager.rc, ... and property contexts) copied
//! into a temp directory, plus empty files for the service programs so
//! init's program check passes.

#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const PROGRAMS: &[&str] = &[
    "/system/bin/adbd",
    "/system/bin/app_process64",
    "/system/bin/audioserver",
    "/system/bin/auditctl",
    "/system/bin/boringssl_self_test64",
    "/system/bin/logd",
    "/system/bin/servicemanager",
    "/system/bin/sh",
    "/system/bin/surfaceflinger",
    "/system/bin/ueventd",
    "/system/bin/testsvc",
];

/// A fresh short temp directory (socket paths stay readable in logs).
pub fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dgi-{tag}-{}", std::process::id()));
    if dir.exists() {
        make_writable(&dir);
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn make_writable(path: &Path) {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return;
    };
    if meta.file_type().is_symlink() {
        return;
    }
    let mode = meta.permissions().mode();
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o700));
    if meta.is_dir() {
        for entry in fs::read_dir(path).unwrap().flatten() {
            make_writable(&entry.path());
        }
    }
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap().flatten() {
        let source = entry.path();
        let target = to.join(entry.file_name());
        if fs::metadata(&source).unwrap().is_dir() {
            copy_tree(&source, &target);
        } else {
            fs::write(&target, fs::read(&source).unwrap()).unwrap();
        }
    }
}

/// The fixture image with program stubs, under `root/image`.
pub fn fixture_image(root: &Path) -> PathBuf {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../aim-android-init/tests/golden/image");
    let image = root.join("image");
    copy_tree(&fixture, &image);
    for program in PROGRAMS {
        let path = image.join(program.trim_start_matches('/'));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    image
}

/// A stand-in for `linux-run` that accepts the contract's options, records
/// what the service process received next to its identity file
/// (`<identity>.probe`), and then sleeps.
pub fn fake_linux_run(root: &Path) -> PathBuf {
    let path = root.join("fake-linux-run");
    fs::write(
        &path,
        r#"#!/bin/sh
if [ "$1" = "--help" ]; then
  echo "usage: linux-run [--root DIR] [--path-map FILE] [--identity FILE] [--inherit-env] [--trace] PROGRAM [ARGS...]" >&2
  exit 2
fi
id=""
while [ $# -gt 0 ]; do
  case "$1" in
    --identity) id="$2"; shift 2 ;;
    --root|--path-map) shift 2 ;;
    --inherit-env|--trace) shift ;;
    *) break ;;
  esac
done
{
  echo "program=$1"
  /usr/bin/env | /usr/bin/grep "^ANDROID_" | /usr/bin/sort
  for fd in 3 4 5 6 7 8; do
    if [ -S /dev/fd/$fd ]; then echo "fd$fd=socket"; elif [ -e /dev/fd/$fd ]; then echo "fd$fd=open"; fi
  done
} > "$id.probe.tmp"
/bin/mv "$id.probe.tmp" "$id.probe"
exec /bin/sleep 20
"#,
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Waits for a file to appear.
pub fn wait_for_file(path: &Path) -> String {
    for _ in 0..500 {
        if let Ok(text) = fs::read_to_string(path) {
            return text;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("{} did not appear", path.display());
}
