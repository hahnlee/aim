use std::env;
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
        "-fno-builtin",
    ]);
    for include in includes {
        command.arg(format!("-I{include}"));
    }
    if let Ok(sanitizer) = env::var("BIONIC_WIDE_INTEGER_C_SANITIZER") {
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
            .success()
    );
}

fn main() {
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos"));
    assert_eq!(env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("aarch64"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let sdk = output(Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]));
    let provider = out.join("provider.o");
    let errno = out.join("errno.o");
    compile("src/provider.c", &provider, &sdk, &["include"]);
    compile(
        "../bionic-errno-tls/src/errno_tls.c",
        &errno,
        &sdk,
        &[
            "../bionic-errno-tls/include",
            "../bionic-errno-tls/generated",
        ],
    );
    let archive = out.join("libbionic_wide_integer.a");
    assert!(
        Command::new("ar")
            .arg("rcs")
            .arg(&archive)
            .args([&provider, &errno])
            .status()
            .expect("archive provider")
            .success()
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=bionic_wide_integer");
    if let Ok(sanitizer) = env::var("BIONIC_WIDE_INTEGER_C_SANITIZER") {
        let runtime_name = match sanitizer.as_str() {
            "address" => "clang_rt.asan_osx_dynamic",
            "undefined" => "clang_rt.ubsan_osx_dynamic",
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
    println!("cargo:rerun-if-env-changed=BIONIC_WIDE_INTEGER_C_SANITIZER");
    for source in [
        "src/provider.c",
        "include/aim_bionic_wide_integer.h",
        "../bionic-errno-tls/src/errno_tls.c",
        "../bionic-errno-tls/include/aim_bionic_errno.h",
        "../bionic-errno-tls/generated/darwin_to_android.inc",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
}
