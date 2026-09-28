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

fn compile(
    compiler: &str,
    language: &str,
    source: &str,
    object: &Path,
    sdk: &str,
    includes: &[&str],
) {
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
    ]);
    for include in includes {
        command.arg(format!("-I{include}"));
    }
    if let Ok(sanitizer) = env::var("BIONIC_WIDE_STDIO_C_SANITIZER") {
        command
            .arg(format!("-fsanitize={sanitizer}"))
            .arg("-fno-omit-frame-pointer");
    }
    assert!(
        command
            .args(["-c", source, "-o"])
            .arg(object)
            .status()
            .unwrap_or_else(|error| panic!("failed to compile {source}: {error}"))
            .success(),
        "compile failed: {source}"
    );
}

fn main() {
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos"));
    assert_eq!(env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("aarch64"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest"));
    let root = manifest.join("../..");
    let locale_archive = root.join("_build/bionic-locale-facade/libaim-bionic-locale.a");
    let icu = root.join("_build/icu-foundation");
    let sdk = output(
        Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]),
        "macOS SDK lookup",
    );

    let provider = out.join("provider.o");
    let shims = out.join("shims.o");
    let errno = out.join("errno.o");
    let backend = out.join("elf_backend.o");
    compile(
        "clang++",
        "-std=c++20",
        "src/provider.cc",
        &provider,
        &sdk,
        &["include", "../bionic-locale-facade/include"],
    );
    compile(
        "clang",
        "-std=c17",
        "src/shims.c",
        &shims,
        &sdk,
        &["include"],
    );
    compile(
        "clang",
        "-std=c17",
        "../bionic-errno-tls/src/errno_tls.c",
        &errno,
        &sdk,
        &[
            "../bionic-errno-tls/include",
            "../bionic-errno-tls/generated",
        ],
    );
    compile(
        "clang++",
        "-std=c++20",
        "probes/elf_backend.cc",
        &backend,
        &sdk,
        &[
            "include",
            "probes",
            "../bionic-errno-tls/include",
            "../bionic-locale-facade/include",
        ],
    );
    let archive = out.join("libbionic_wide_stdio_test_bundle.a");
    assert!(
        Command::new("ar")
            .arg("rcs")
            .arg(&archive)
            .args([&provider, &shims, &errno, &backend])
            .status()
            .expect("archive test bundle")
            .success()
    );

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=bionic_wide_stdio_test_bundle");
    println!(
        "cargo:rustc-link-arg=-Wl,-force_load,{}",
        locale_archive.display()
    );
    println!("cargo:rustc-link-search=native={}", icu.display());
    println!(
        "cargo:rustc-link-arg=-Wl,-force_load,{}",
        icu.join("libandroidicuinit-darwin.a").display()
    );
    println!("cargo:rustc-link-lib=static=icuuc-common-darwin");
    println!("cargo:rustc-link-lib=static=icuuc-stubdata-darwin");
    println!("cargo:rustc-link-lib=c++");

    if let Ok(sanitizer) = env::var("BIONIC_WIDE_STDIO_C_SANITIZER") {
        let runtime_name = match sanitizer.as_str() {
            "address" => "clang_rt.asan_osx_dynamic",
            "undefined" => "clang_rt.ubsan_osx_dynamic",
            _ => panic!("unsupported C sanitizer"),
        };
        let runtime_file = format!("lib{runtime_name}.dylib");
        let runtime = output(
            Command::new("clang").arg(format!("-print-file-name={runtime_file}")),
            "sanitizer runtime lookup",
        );
        let runtime_dir = Path::new(&runtime)
            .parent()
            .expect("sanitizer runtime directory");
        println!("cargo:rustc-link-search=native={}", runtime_dir.display());
        println!("cargo:rustc-link-lib=dylib={runtime_name}");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", runtime_dir.display());
    }
    println!("cargo:rerun-if-env-changed=BIONIC_WIDE_STDIO_C_SANITIZER");
    for source in [
        "src/provider.cc",
        "src/shims.c",
        "include/aim_bionic_wide_stdio.h",
        "probes/backend.h",
        "probes/elf_backend.cc",
        "../bionic-locale-facade/include/aim_bionic_locale.h",
        "../bionic-errno-tls/src/errno_tls.c",
        "../bionic-errno-tls/include/aim_bionic_errno.h",
        "../bionic-errno-tls/generated/darwin_to_android.inc",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
    for source in [
        locale_archive,
        icu.join("libicuuc-common-darwin.a"),
        icu.join("libicuuc-stubdata-darwin.a"),
        icu.join("libandroidicuinit-darwin.a"),
    ] {
        println!("cargo:rerun-if-changed={}", source.display());
    }
}
