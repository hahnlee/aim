//! Threads, futexes and signals from guest code: `guest/threads.c`, built
//! with the NDK, runs under linux-run on the image's own linker64 and
//! bionic. Skipped when the NDK or the full image is not installed.

use std::os::unix::fs::symlink;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

const IMAGE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../_build/android16-image-full"
);
/// Under $HOME; the pinned NDK the repository's scripts use.
const NDK_CLANG: &str =
    "Library/Android/sdk/ndk/28.2.13676358/toolchains/llvm/prebuilt/darwin-x86_64/bin/clang";

/// A guest root with linker64, bionic and the test program, built once
/// per test process (rebuilt from scratch each run).
struct Root(PathBuf);

fn root() -> Option<&'static Root> {
    static ROOT: OnceLock<Option<Root>> = OnceLock::new();
    ROOT.get_or_init(build_root).as_ref()
}

fn build_root() -> Option<Root> {
    let clang = Path::new(&std::env::var_os("HOME")?).join(NDK_CLANG);
    let image = Path::new(IMAGE);
    if !clang.exists() || !image.join("system/lib64/ld-android.so").exists() {
        eprintln!(
            "skipped: needs the NDK ({}) and the full image ({IMAGE})",
            clang.display()
        );
        return None;
    }
    let r = Root(Path::new(env!("CARGO_TARGET_TMPDIR")).join("threads-root"));
    let _ = std::fs::remove_dir_all(&r.0);
    let copy = |guest: &str| {
        let to = r.0.join(guest);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(image.join(guest), &to).unwrap();
    };
    copy("apex/com.android.runtime/bin/linker64");
    copy("system/lib64/ld-android.so");
    std::fs::create_dir_all(r.0.join("system/bin")).unwrap();
    symlink(
        "/apex/com.android.runtime/bin/linker64",
        r.0.join("system/bin/linker64"),
    )
    .unwrap();
    for lib in ["libc.so", "libm.so", "libdl.so"] {
        copy(&format!("apex/com.android.runtime/lib64/bionic/{lib}"));
        symlink(
            format!("/apex/com.android.runtime/lib64/bionic/{lib}"),
            r.0.join("system/lib64").join(lib),
        )
        .unwrap();
    }
    let bin = r.0.join("data/local/tmp/threads");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guest/threads.c");
    let out = Command::new(clang)
        .args([
            "--target=aarch64-linux-android33",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-o",
        ])
        .arg(&bin)
        .arg(&src)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    Some(r)
}

fn run(root: &Root, args: &[&str]) -> (std::process::ExitStatus, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_linux-run"))
        .arg("--root")
        .arg(&root.0)
        .arg("/data/local/tmp/threads")
        .args(args)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    (out.status, text)
}

#[test]
fn threads_futexes_and_signals_on_original_bionic() {
    let Some(r) = root() else { return };
    let (status, out) = run(r, &[]);
    // The latencies, for the record.
    for line in out.lines().filter(|l| l.starts_with("lat ")) {
        eprintln!("{line}");
    }
    assert!(
        status.success() && out.contains("ALL PASSED"),
        "{status:?}\n{out}"
    );
}

#[test]
fn abort_kills_the_process_with_sigabrt() {
    let Some(r) = root() else { return };
    let (status, out) = run(r, &["abort"]);
    assert_eq!(status.signal(), Some(libc::SIGABRT), "{status:?}\n{out}");
}

#[test]
fn unhandled_fault_kills_the_process_with_the_signal() {
    let Some(r) = root() else { return };
    let (status, out) = run(r, &["segv_default"]);
    assert_eq!(status.signal(), Some(libc::SIGSEGV), "{status:?}\n{out}");
}

#[test]
fn process_outlives_its_main_thread() {
    let Some(r) = root() else { return };
    let (status, out) = run(r, &["main_exit"]);
    assert!(
        status.success() && out.contains("worker done"),
        "{status:?}\n{out}"
    );
}

/// A host signal from another process reaches a guest thread waiting in
/// sigwait with the signal blocked (ART's SignalCatcher and `kill -3`).
#[test]
fn signal_from_another_process_reaches_sigwait() {
    use std::io::{BufRead, BufReader};
    let Some(r) = root() else { return };
    let mut child = Command::new(env!("CARGO_BIN_EXE_linux-run"))
        .arg("--root")
        .arg(&r.0)
        .args(["/data/local/tmp/threads", "external_sigquit"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    assert_eq!(lines.next().unwrap().unwrap(), "ready");
    // SAFETY: signalling our own child.
    unsafe { libc::kill(child.id() as i32, libc::SIGQUIT) };
    assert_eq!(lines.next().unwrap().unwrap(), "got 3");
    assert!(child.wait().unwrap().success());
}
