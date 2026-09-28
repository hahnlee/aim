use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(command: &mut Command, description: &str) {
    assert!(
        command.status().expect("launch tool").success(),
        "{description} failed"
    );
}

fn output(command: &mut Command, description: &str) -> String {
    let result = command.output().expect("launch tool");
    assert!(result.status.success(), "{description} failed");
    String::from_utf8(result.stdout)
        .expect("UTF-8 output")
        .trim()
        .to_owned()
}

fn main() {
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos"));
    assert_eq!(env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("aarch64"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest"));
    let root = manifest.join("../..");
    let source_root = root.join("_aosp/bionic-swprintf-facade");
    let gdtoa_source = source_root.join("libc/upstream-openbsd/lib/libc/gdtoa/gdtoa.c");
    let shared_gdtoa =
        root.join("_aosp/bionic-float-conversion-facade/libc/upstream-openbsd/lib/libc/gdtoa");
    let android_include =
        root.join("_aosp/bionic-float-conversion-facade/libc/upstream-openbsd/android/include");
    let sdk = output(
        Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]),
        "SDK lookup",
    );
    let sanitizer = env::var("BIONIC_SWPRINTF_C_SANITIZER").ok();
    let sanitizer_flag = sanitizer
        .as_ref()
        .map(|value| format!("-fsanitize={value}"));

    let mut common = vec![
        "-arch",
        "arm64",
        "-isysroot",
        &sdk,
        "-O2",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-fno-builtin",
        "-fvisibility=hidden",
    ];
    if let Some(flag) = sanitizer_flag.as_deref() {
        common.push(flag);
        common.push("-fno-omit-frame-pointer");
    }

    let gdtoa_object = out.join("gdtoa.o");
    let mut gdtoa_command = Command::new("clang");
    gdtoa_command
        .args(&common)
        .args(["-std=gnu17", "-Wno-sign-compare", "-I"])
        .arg(root.join("tools/bionic-float-conversion-facade/include"))
        .arg("-I")
        .arg(&android_include)
        .arg("-I")
        .arg(&shared_gdtoa)
        .args(["-include", "aim_gdtoa_compat.h", "-D__gdtoa=aim_aosp_gdtoa"]);
    if sanitizer.as_deref() == Some("undefined") {
        // Pinned gdtoa uses a signed normalization shift; the arithmetic is
        // source-authoritative and independently differential-tested.
        gdtoa_command.arg("-fno-sanitize=shift");
    }
    gdtoa_command
        .arg("-c")
        .arg(&gdtoa_source)
        .args(["-o"])
        .arg(&gdtoa_object);
    run(&mut gdtoa_command, "gdtoa");

    let provider = out.join("provider.o");
    run(
        Command::new("clang++")
            .args(&common)
            .args([
                "-std=c++20",
                "-Iinclude",
                "-I../bionic-errno-tls/include",
                "-I../bionic-format-facade/include",
                "-c",
                "src/provider.cc",
                "-o",
            ])
            .arg(&provider),
        "provider",
    );

    let entry = out.join("entry.o");
    run(
        Command::new("clang")
            .args(&common)
            .args(["-c", "src/aapcs64_entry.S", "-o"])
            .arg(&entry),
        "AAPCS64 entry",
    );
    let archive = out.join("libaim_bionic_swprintf.a");
    run(
        Command::new("ar")
            .arg("rcs")
            .arg(&archive)
            .args([&provider, &entry, &gdtoa_object]),
        "archive",
    );

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=aim_bionic_swprintf");
    println!(
        "cargo:rustc-link-search=native={}",
        root.join("_build/bionic-format-facade").display()
    );
    println!("cargo:rustc-link-lib=static=aim-bionic-format");
    println!(
        "cargo:rustc-link-search=native={}",
        root.join("_build/bionic-float-conversion-facade").display()
    );
    println!("cargo:rustc-link-lib=static=aim-bionic-float-conversion");
    println!("cargo:rustc-link-lib=c++");

    if let Some(value) = sanitizer.as_deref() {
        let runtime_name = match value {
            "address" => "clang_rt.asan_osx_dynamic",
            "undefined" => "clang_rt.ubsan_osx_dynamic",
            _ => panic!("unsupported C sanitizer"),
        };
        let runtime_file = format!("lib{runtime_name}.dylib");
        let runtime = output(
            Command::new("clang").arg(format!("-print-file-name={runtime_file}")),
            "sanitizer runtime lookup",
        );
        let runtime_dir = Path::new(&runtime).parent().expect("sanitizer directory");
        println!("cargo:rustc-link-search=native={}", runtime_dir.display());
        println!("cargo:rustc-link-lib=dylib={runtime_name}");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", runtime_dir.display());
    }
    println!("cargo:rerun-if-env-changed=BIONIC_SWPRINTF_C_SANITIZER");
    for source in [
        "build.rs",
        "src/provider.cc",
        "src/aapcs64_entry.S",
        "include/aim_bionic_swprintf.h",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
}
