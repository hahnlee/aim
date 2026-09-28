use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn output(command: &mut Command, description: &str) -> String {
    let result = command
        .output()
        .unwrap_or_else(|error| panic!("failed to launch {description}: {error}"));
    assert!(
        result.status.success(),
        "{description} failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout)
        .expect("tool output is UTF-8")
        .trim()
        .to_owned()
}

fn compile(source: &str, object: &Path, sdk: &str, includes: &[&str]) {
    let mut command = Command::new("clang");
    command.args([
        "-arch",
        "arm64",
        "-isysroot",
        sdk,
        "-std=c17",
        "-O2",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-Wpedantic",
    ]);
    for include in includes {
        command.arg(format!("-I{include}"));
    }
    let status = command
        .args(["-c", source, "-o"])
        .arg(object)
        .status()
        .unwrap_or_else(|error| panic!("failed to launch clang: {error}"));
    assert!(status.success(), "clang failed for {source}");
}

fn main() {
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos"));
    assert_eq!(env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("aarch64"));
    let output_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let sdk = output(
        Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]),
        "xcrun SDK lookup",
    );
    let shims = output_dir.join("time_shims.o");
    let errno = output_dir.join("errno_tls.o");
    let archive = output_dir.join("libaim_bionic_time.a");
    compile("src/shims.c", &shims, &sdk, &["include"]);
    compile(
        "../bionic-errno-tls/src/errno_tls.c",
        &errno,
        &sdk,
        &[
            "../bionic-errno-tls/include",
            "../bionic-errno-tls/generated",
        ],
    );
    let status = Command::new("ar")
        .arg("rcs")
        .arg(&archive)
        .arg(&shims)
        .arg(&errno)
        .status()
        .expect("failed to launch ar");
    assert!(status.success(), "ar failed");
    println!("cargo:rustc-link-search=native={}", output_dir.display());
    println!("cargo:rustc-link-lib=static=aim_bionic_time");
    for source in [
        "src/shims.c",
        "include/aim_bionic_time.h",
        "../bionic-errno-tls/src/errno_tls.c",
        "../bionic-errno-tls/include/aim_bionic_errno.h",
        "../bionic-errno-tls/generated/darwin_to_android.inc",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
}
