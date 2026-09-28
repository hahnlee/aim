use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn output(command: &mut Command, label: &str) -> String {
    let result = command
        .output()
        .unwrap_or_else(|error| panic!("launch {label}: {error}"));
    assert!(
        result.status.success(),
        "{label} failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout)
        .expect("tool output UTF-8")
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
        "-fno-builtin",
        "-fno-stack-protector",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-Wpedantic",
    ]);
    for include in includes {
        command.arg(format!("-I{include}"));
    }
    if let Ok(sanitizer) = env::var("BIONIC_STRERROR_C_SANITIZER") {
        command.args([
            format!("-fsanitize={sanitizer}"),
            "-fno-omit-frame-pointer".to_owned(),
        ]);
    }
    assert!(
        command
            .args(["-c", source, "-o"])
            .arg(object)
            .status()
            .unwrap_or_else(|error| panic!("launch clang for {source}: {error}"))
            .success(),
        "clang failed for {source}"
    );
}

fn main() {
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos"));
    assert_eq!(env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("aarch64"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let sdk = output(
        Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]),
        "SDK lookup",
    );
    let provider = out.join("strerror.o");
    let errno = out.join("errno.o");
    compile("src/strerror.c", &provider, &sdk, &["include", "generated"]);
    compile(
        "../bionic-errno-tls/src/errno_tls.c",
        &errno,
        &sdk,
        &[
            "../bionic-errno-tls/include",
            "../bionic-errno-tls/generated",
        ],
    );
    let archive = out.join("libbionic_strerror.a");
    assert!(
        Command::new("ar")
            .arg("rcs")
            .arg(&archive)
            .args([&provider, &errno])
            .status()
            .expect("launch ar")
            .success()
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=bionic_strerror");
    if let Ok(sanitizer) = env::var("BIONIC_STRERROR_C_SANITIZER") {
        let runtime_name = match sanitizer.as_str() {
            "address" => "clang_rt.asan_osx_dynamic",
            "undefined" => "clang_rt.ubsan_osx_dynamic",
            _ => panic!("unsupported C sanitizer"),
        };
        let filename = format!("lib{runtime_name}.dylib");
        let runtime = output(
            Command::new("clang").arg(format!("-print-file-name={filename}")),
            "sanitizer runtime lookup",
        );
        let runtime_dir = Path::new(&runtime).parent().expect("sanitizer directory");
        println!("cargo:rustc-link-search=native={}", runtime_dir.display());
        println!("cargo:rustc-link-lib=dylib={runtime_name}");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", runtime_dir.display());
    }
    println!("cargo:rerun-if-env-changed=BIONIC_STRERROR_C_SANITIZER");
    for source in [
        "src/strerror.c",
        "include/aim_bionic_strerror.h",
        "generated/android_errno_messages.inc",
        "../bionic-errno-tls/src/errno_tls.c",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
}
