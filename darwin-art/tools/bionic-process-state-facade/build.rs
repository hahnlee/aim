use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn output(command: &mut Command, description: &str) -> String {
    let result = command
        .output()
        .unwrap_or_else(|error| panic!("failed to launch {description}: {error}"));
    assert!(result.status.success(), "{description} failed");
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
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
        .unwrap();
    assert!(status.success(), "clang failed for {source}");
}

fn main() {
    let lock = std::fs::read_to_string("sources.lock").unwrap();
    let expected = lock
        .lines()
        .find_map(|line| line.strip_prefix("BIONIC_DEVICE_API_HEADER_SHA256="))
        .unwrap();
    let hash = output(
        Command::new("shasum").args(["-a", "256", "upstream/get_device_api_level_inlines.h"]),
        "bionic device API source hash",
    );
    assert_eq!(
        hash.split_whitespace().next(),
        Some(expected),
        "original bionic source drift"
    );
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos"));
    assert_eq!(env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("aarch64"));
    let output_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let sdk = output(
        Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]),
        "SDK lookup",
    );
    let shims = output_dir.join("shims.o");
    let process_exit = output_dir.join("process_exit.o");
    let device_api_level = output_dir.join("device_api_level.o");
    let setjmp = output_dir.join("setjmp.o");
    let errno = output_dir.join("errno.o");
    let archive = output_dir.join("libdarwin_art_bionic_process_state.a");
    let syscall = output_dir.join("syscall.o");
    let syscall_entry = output_dir.join("syscall_entry.o");
    let standalone = env::var_os("CARGO_FEATURE_STANDALONE_SYSCALL").is_some();
    if standalone {
        assert!(
            Command::new("clang++")
                .args([
                    "-std=c++20",
                    "-O2",
                    "-I../bionic-syscall-facade/include",
                    "-c",
                    "../bionic-syscall-facade/src/syscall.cc",
                    "-o"
                ])
                .arg(&syscall)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("clang")
                .args(["-c", "../bionic-syscall-facade/src/aapcs64_entry.S", "-o"])
                .arg(&syscall_entry)
                .status()
                .unwrap()
                .success()
        );
    }
    compile("src/shims.c", &shims, &sdk, &["include"]);
    compile("src/process_exit.c", &process_exit, &sdk, &["include"]);
    compile(
        "src/device_api_level.c",
        &device_api_level,
        &sdk,
        &["include"],
    );
    assert!(
        Command::new("clang")
            .args([
                "-arch",
                "arm64",
                "-isysroot",
                &sdk,
                "-c",
                "src/setjmp_arm64.S",
                "-o"
            ])
            .arg(&setjmp)
            .status()
            .unwrap()
            .success()
    );
    compile(
        "../bionic-errno-tls/src/errno_tls.c",
        &errno,
        &sdk,
        &[
            "../bionic-errno-tls/include",
            "../bionic-errno-tls/generated",
        ],
    );
    if archive.exists() {
        std::fs::remove_file(&archive).unwrap();
    }
    let mut archiver = Command::new("ar");
    archiver
        .arg("rcs")
        .arg(&archive)
        .arg(&shims)
        .arg(&process_exit)
        .arg(&device_api_level)
        .arg(&setjmp)
        .arg(&errno);
    if standalone {
        archiver.arg(&syscall).arg(&syscall_entry);
    }
    assert!(archiver.status().unwrap().success());
    println!("cargo:rustc-link-search=native={}", output_dir.display());
    println!("cargo:rustc-link-lib=static=darwin_art_bionic_process_state");
    if standalone {
        println!("cargo:rustc-link-lib=c++");
    }
    println!("cargo:rerun-if-changed=../bionic-syscall-facade/src/syscall.cc");
    println!("cargo:rerun-if-changed=../bionic-syscall-facade/src/aapcs64_entry.S");
    println!("cargo:rerun-if-changed=../bionic-syscall-facade/include/darwin_art_bionic_syscall.h");
    for source in [
        "src/shims.c",
        "src/process_exit.c",
        "src/device_api_level.c",
        "upstream/get_device_api_level_inlines.h",
        "sources.lock",
        "src/setjmp_arm64.S",
        "include/darwin_art_bionic_process_state.h",
        "../bionic-errno-tls/src/errno_tls.c",
        "../bionic-errno-tls/include/darwin_art_bionic_errno.h",
        "../bionic-errno-tls/generated/darwin_to_android.inc",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
}
