use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn output(command: &mut Command, description: &str) -> String {
    let result = command.output().expect("run tool");
    assert!(result.status.success(), "{description} failed");
    String::from_utf8(result.stdout)
        .expect("UTF-8 tool output")
        .trim()
        .to_owned()
}

fn compile(
    compiler: &str,
    standard: &str,
    source: &str,
    object: &Path,
    sdk: &str,
    includes: &[PathBuf],
) {
    let mut command = Command::new(compiler);
    command.args([
        "-arch",
        "arm64",
        "-isysroot",
        sdk,
        standard,
        "-O2",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-fno-builtin",
        "-fvisibility=hidden",
    ]);
    if standard == "-std=c++20" {
        command.arg("-DANDROID");
    }
    for include in includes {
        command.arg("-I").arg(include);
    }
    if let Ok(sanitizer) = env::var("BIONIC_WIDE_FLOAT_C_SANITIZER") {
        command
            .arg(format!("-fsanitize={sanitizer}"))
            .arg("-fno-omit-frame-pointer");
    }
    assert!(
        command
            .args(["-c", source, "-o"])
            .arg(object)
            .status()
            .expect("compile provider")
            .success(),
        "compile {source} failed"
    );
}

fn main() {
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos"));
    assert_eq!(env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("aarch64"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest"));
    let root = manifest.join("../..");
    let sdk = output(
        Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]),
        "SDK lookup",
    );
    let icu_root = root.join("_aosp/external/icu-graphics");
    let provider = out.join("provider.o");
    let allocator = out.join("allocator.o");
    let allocator_options = out.join("allocator_options.o");
    compile(
        "clang++",
        "-std=c++20",
        "src/provider.cc",
        &provider,
        &sdk,
        &[
            manifest.join("include"),
            root.join("tools/bionic-float-conversion-facade/include"),
            root.join("tools/bionic-libc-allocator-facade/include"),
            root.join("tools/bionic-errno-tls/include"),
            icu_root.join("android_icu4c/include"),
            icu_root.join("icu4c/source/common"),
            icu_root.join("libandroidicuinit/include"),
        ],
    );
    compile(
        "clang",
        "-std=c17",
        "../bionic-libc-allocator-facade/src/allocator.c",
        &allocator,
        &sdk,
        &[root.join("tools/bionic-libc-allocator-facade/include")],
    );
    compile(
        "clang",
        "-std=c17",
        "../bionic-libc-allocator-facade/src/allocator_options.c",
        &allocator_options,
        &sdk,
        &[root.join("tools/bionic-libc-allocator-facade/include")],
    );
    let archive = out.join("libdarwin_art_bionic_wide_float.a");
    assert!(
        Command::new("ar")
            .arg("rcs")
            .arg(&archive)
            .arg(&provider)
            .status()
            .expect("archive provider")
            .success()
    );
    let allocator_archive = out.join("libdarwin_art_bionic_wide_float_allocator_test.a");
    assert!(
        Command::new("ar")
            .arg("rcs")
            .arg(&allocator_archive)
            .arg(&allocator)
            .arg(&allocator_options)
            .status()
            .expect("archive allocator test dependency")
            .success()
    );

    let float_build = root.join("_build/bionic-float-conversion-facade");
    let icu_build = root.join("_build/icu-foundation");
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=darwin_art_bionic_wide_float");
    println!("cargo:rustc-link-lib=static=darwin_art_bionic_wide_float_allocator_test");
    println!("cargo:rustc-link-search=native={}", float_build.display());
    println!("cargo:rustc-link-lib=static=darwin-art-bionic-float-conversion");
    println!(
        "cargo:rustc-link-arg=-Wl,-force_load,{}",
        icu_build.join("libandroidicuinit-darwin.a").display()
    );
    println!(
        "cargo:rustc-link-arg={}",
        icu_build.join("libicuuc-common-darwin.a").display()
    );
    println!(
        "cargo:rustc-link-arg={}",
        icu_build.join("libicuuc-stubdata-darwin.a").display()
    );
    println!("cargo:rustc-link-lib=c++");

    if let Ok(sanitizer) = env::var("BIONIC_WIDE_FLOAT_C_SANITIZER") {
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
    println!("cargo:rerun-if-env-changed=BIONIC_WIDE_FLOAT_C_SANITIZER");
    for source in [
        "src/provider.cc",
        "include/darwin_art_bionic_wide_float.h",
        "../bionic-libc-allocator-facade/src/allocator.c",
        "../bionic-libc-allocator-facade/src/allocator_options.c",
        "../bionic-libc-allocator-facade/include/darwin_art_bionic_allocator.h",
        "../bionic-errno-tls/include/darwin_art_bionic_errno.h",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
}
