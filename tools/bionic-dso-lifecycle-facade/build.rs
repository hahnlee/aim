use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn output(command: &mut Command, description: &str) -> String {
    let result = command
        .output()
        .unwrap_or_else(|error| panic!("failed to launch {description}: {error}"));
    assert!(result.status.success(), "{description} failed");
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

fn compile(source: &str, object: &Path, sdk: &str) {
    assert!(
        Command::new("clang")
            .args([
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
                "-Iinclude",
                "-c",
                source,
                "-o",
            ])
            .arg(object)
            .status()
            .unwrap()
            .success(),
        "clang failed for {source}"
    );
}

fn main() {
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos"));
    assert_eq!(env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("aarch64"));
    let output_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let standalone_test_stubs = env::var_os("CARGO_FEATURE_STANDALONE_TEST_STUBS").is_some();
    let sdk = output(
        Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]),
        "SDK lookup",
    );
    let shims = output_dir.join("shims.o");
    let archive = output_dir.join("libaim_bionic_dso_lifecycle.a");
    compile("src/shims.c", &shims, &sdk);
    if archive.exists() {
        fs::remove_file(&archive).expect("failed to replace lifecycle archive");
    }
    let mut archive_command = Command::new("ar");
    archive_command.arg("rcs").arg(&archive).arg(&shims);
    if standalone_test_stubs {
        let standalone_stubs = output_dir.join("standalone_libdl_stubs.o");
        compile("src/standalone_libdl_stubs.c", &standalone_stubs, &sdk);
        archive_command.arg(&standalone_stubs);
    }
    assert!(archive_command.status().unwrap().success());
    println!("cargo:rustc-link-search=native={}", output_dir.display());
    println!("cargo:rustc-link-lib=static=aim_bionic_dso_lifecycle");
    for source in [
        "src/shims.c",
        "src/standalone_libdl_stubs.c",
        "include/aim_bionic_dso_lifecycle.h",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
}
