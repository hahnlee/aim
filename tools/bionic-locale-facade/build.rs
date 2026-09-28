use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(command: &mut Command, description: &str) {
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("failed to launch {description}: {error}"));
    assert!(status.success(), "{description} failed");
}

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

fn main() {
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos"));
    assert_eq!(env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("aarch64"));
    let output_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    let project_root = manifest_dir.join("../..");
    let icu_root = project_root.join("_aosp/external/icu-graphics");
    let icu_foundation = project_root.join("_build/icu-foundation");
    let sdk = output(
        Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]),
        "macOS SDK lookup",
    );
    let provider = output_dir.join("provider.o");
    let errno = output_dir.join("errno.o");
    let archive = output_dir.join("libaim_bionic_locale.a");
    run(
        Command::new("clang++")
            .args([
                "-arch",
                "arm64",
                "-isysroot",
                &sdk,
                "-std=c++20",
                "-O2",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-fno-builtin",
                "-fvisibility=hidden",
                "-DANDROID",
                "-Iinclude",
                "-I",
            ])
            .arg(icu_root.join("android_icu4c/include"))
            .arg("-I")
            .arg(icu_root.join("icu4c/source/common"))
            .arg("-I")
            .arg(icu_root.join("libandroidicuinit/include"))
            .args(["-c", "src/provider.cc", "-o"])
            .arg(&provider),
        "locale provider compile",
    );
    run(
        Command::new("clang")
            .args([
                "-arch",
                "arm64",
                "-isysroot",
                &sdk,
                "-std=c17",
                "-O2",
                "-Wall",
                "-Wextra",
                "-Werror",
                "-I../bionic-errno-tls/include",
                "-I../bionic-errno-tls/generated",
                "-c",
                "../bionic-errno-tls/src/errno_tls.c",
                "-o",
            ])
            .arg(&errno),
        "Bionic errno compile",
    );
    run(
        Command::new("ar")
            .arg("rcs")
            .arg(&archive)
            .arg(&provider)
            .arg(&errno),
        "locale provider archive",
    );
    println!("cargo:rustc-link-search=native={}", output_dir.display());
    println!("cargo:rustc-link-lib=static=aim_bionic_locale");
    println!(
        "cargo:rustc-link-search=native={}",
        icu_foundation.display()
    );
    println!(
        "cargo:rustc-link-arg=-Wl,-force_load,{}",
        icu_foundation.join("libandroidicuinit-darwin.a").display()
    );
    println!("cargo:rustc-link-lib=static=icuuc-common-darwin");
    println!("cargo:rustc-link-lib=static=icuuc-stubdata-darwin");
    println!("cargo:rustc-link-lib=c++");
    for source in [
        "src/provider.cc",
        "include/aim_bionic_locale.h",
        "../bionic-errno-tls/src/errno_tls.c",
        "../bionic-errno-tls/include/aim_bionic_errno.h",
        "../bionic-errno-tls/generated/darwin_to_android.inc",
        "../../upstream/android16-icu-foundation.lock",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
    for source in [
        icu_foundation.join("libicuuc-common-darwin.a"),
        icu_foundation.join("libicuuc-stubdata-darwin.a"),
        icu_foundation.join("libandroidicuinit-darwin.a"),
        icu_foundation.join("runtime/i18n/etc/icu/icudt76l.dat"),
    ] {
        println!("cargo:rerun-if-changed={}", source.display());
    }
}

#[allow(dead_code)]
fn _path_type(_: &Path) {}
