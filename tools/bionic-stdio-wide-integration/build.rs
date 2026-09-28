use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn output(command: &mut Command) -> String {
    let result = command.output().expect("run tool");
    assert!(result.status.success());
    String::from_utf8(result.stdout)
        .expect("UTF-8 tool output")
        .trim()
        .to_owned()
}

fn compile(compiler: &str, language: &str, source: &str, object: &Path, sdk: &str) {
    let mut command = Command::new(compiler);
    command.args([
        "-arch",
        "arm64",
        "-isysroot",
        sdk,
        language,
        "-O1",
        "-g",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-fno-builtin",
        "-fvisibility=hidden",
        "-I../bionic-wide-stdio-facade/include",
        "-I../bionic-locale-facade/include",
    ]);
    if let Ok(sanitizer) = env::var("BIONIC_STDIO_C_SANITIZER") {
        command
            .arg(format!("-fsanitize={sanitizer}"))
            .arg("-fno-omit-frame-pointer");
    }
    assert!(
        command
            .args(["-c", source, "-o"])
            .arg(object)
            .status()
            .expect("compile wide stdio integration")
            .success()
    );
}

fn main() {
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos"));
    assert_eq!(env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("aarch64"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest"));
    let root = manifest.join("../..");
    let sdk = output(Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]));
    let provider = out.join("wide-provider.o");
    let shims = out.join("wide-shims.o");
    let audit_dependencies = out.join("audit-dependency-shims.o");
    compile(
        "clang++",
        "-std=c++20",
        "../bionic-wide-stdio-facade/src/provider.cc",
        &provider,
        &sdk,
    );
    compile(
        "clang",
        "-std=c17",
        "../bionic-wide-stdio-facade/src/shims.c",
        &shims,
        &sdk,
    );
    compile(
        "clang",
        "-std=c17",
        "../bionic-stdio-facade/probes/audit_dependency_shims.c",
        &audit_dependencies,
        &sdk,
    );
    let archive = out.join("libbionic_stdio_wide_integration.a");
    let _ = fs::remove_file(&archive);
    assert!(
        Command::new("ar")
            .arg("rcs")
            .arg(&archive)
            .args([&provider, &shims, &audit_dependencies])
            .status()
            .expect("archive wide stdio integration")
            .success()
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=bionic_stdio_wide_integration");

    let locale = root.join("_build/bionic-locale-facade/libaim-bionic-locale.a");
    let icu = root.join("_build/icu-foundation");
    println!("cargo:rustc-link-arg=-Wl,-force_load,{}", locale.display());
    println!("cargo:rustc-link-search=native={}", icu.display());
    println!(
        "cargo:rustc-link-arg=-Wl,-force_load,{}",
        icu.join("libandroidicuinit-darwin.a").display()
    );
    println!("cargo:rustc-link-lib=static=icuuc-common-darwin");
    println!("cargo:rustc-link-lib=static=icuuc-stubdata-darwin");
    println!("cargo:rustc-link-lib=c++");

    if let Ok(sanitizer) = env::var("BIONIC_STDIO_C_SANITIZER") {
        let runtime_name = match sanitizer.as_str() {
            "address" => "clang_rt.asan_osx_dynamic",
            "undefined" => "clang_rt.ubsan_osx_dynamic",
            "thread" => "clang_rt.tsan_osx_dynamic",
            _ => panic!("unsupported C sanitizer"),
        };
        let runtime_file = format!("lib{runtime_name}.dylib");
        let runtime = output(Command::new("clang").arg(format!("-print-file-name={runtime_file}")));
        let runtime_dir = Path::new(&runtime)
            .parent()
            .expect("sanitizer runtime directory");
        println!("cargo:rustc-link-search=native={}", runtime_dir.display());
        println!("cargo:rustc-link-lib=dylib={runtime_name}");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", runtime_dir.display());
    }
    println!("cargo:rerun-if-env-changed=BIONIC_STDIO_C_SANITIZER");
    for source in [
        "../bionic-wide-stdio-facade/src/provider.cc",
        "../bionic-wide-stdio-facade/src/shims.c",
        "../bionic-wide-stdio-facade/include/aim_bionic_wide_stdio.h",
        "../bionic-locale-facade/include/aim_bionic_locale.h",
        "../bionic-stdio-facade/src/main.rs",
        "../bionic-stdio-facade/probes/audit_dependency_shims.c",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
}
