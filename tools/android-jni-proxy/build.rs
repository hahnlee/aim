use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn output(command: &mut Command, description: &str) -> String {
    let result = command.output().unwrap_or_else(|error| {
        panic!("failed to launch {description}: {error}");
    });
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

fn compile(source: &str, object: &Path, sdk: &str) {
    let status = Command::new("clang")
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
            "-Igenerated",
            "-c",
            source,
            "-o",
        ])
        .arg(object)
        .status()
        .unwrap_or_else(|error| panic!("failed to launch clang: {error}"));
    assert!(status.success(), "clang failed for {source}");
}

fn compile_assembly(source: &str, object: &Path, sdk: &str) {
    let status = Command::new("clang")
        .args(["-arch", "arm64", "-isysroot", sdk, "-c", source, "-o"])
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
    let proxy = output_dir.join("proxy.o");
    let call_entry = output_dir.join("aapcs64_call.o");
    let fake = output_dir.join("fake_backend.o");
    let archive = output_dir.join("libaim_jni_proxy.a");
    compile("src/proxy.c", &proxy, &sdk);
    compile_assembly("src/aapcs64_call.S", &call_entry, &sdk);
    compile("probes/fake_backend.c", &fake, &sdk);
    let status = Command::new("ar")
        .arg("rcs")
        .arg(&archive)
        .arg(&proxy)
        .arg(&call_entry)
        .arg(&fake)
        .status()
        .expect("failed to launch ar");
    assert!(status.success(), "ar failed");
    println!("cargo:rustc-link-search=native={}", output_dir.display());
    println!("cargo:rustc-link-lib=static=aim_jni_proxy");
    for source in [
        "src/proxy.c",
        "src/aapcs64_call.S",
        "probes/fake_backend.c",
        "include/aim_jni_proxy.h",
        "generated/jni_slots.h",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
}
