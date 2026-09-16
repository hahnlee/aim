use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn output(command: &mut Command) -> String {
    let result = command.output().expect("run tool");
    assert!(result.status.success());
    String::from_utf8(result.stdout)
        .expect("UTF-8")
        .trim()
        .to_owned()
}
fn run(command: &mut Command, what: &str) {
    assert!(
        command.status().expect("launch tool").success(),
        "{what} failed"
    );
}

fn main() {
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos"));
    assert_eq!(env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("aarch64"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let here = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest"));
    let root = here.join("../..");
    let sdk = output(Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]));
    let sanitizer = env::var("BIONIC_SCANF_C_SANITIZER").ok();
    let mut flags = vec![
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
    let sanitizer_flag;
    if let Some(value) = sanitizer.as_deref() {
        sanitizer_flag = format!("-fsanitize={value}");
        flags.push(&sanitizer_flag);
        flags.push("-fno-omit-frame-pointer");
    }
    let scanf = out.join("scanf.o");
    run(
        Command::new("clang++")
            .args(&flags)
            .args(["-std=c++20", "-Iinclude", "-I"])
            .arg(root.join("tools/bionic-numeric-facade/include"))
            .arg("-I")
            .arg(root.join("tools/bionic-float-conversion-facade/include"))
            .arg("-I")
            .arg(root.join("tools/bionic-errno-tls/include"))
            .arg("-I")
            .arg(root.join("tools/bionic-stdio-facade/include"))
            .args(["-c", "src/scanf.cc", "-o"])
            .arg(&scanf),
        "scanf parser",
    );
    let entry = out.join("entry.o");
    run(
        Command::new("clang")
            .args([
                "-arch",
                "arm64",
                "-isysroot",
                &sdk,
                "-c",
                "src/aapcs64_entry.S",
                "-o",
            ])
            .arg(&entry),
        "AAPCS64 entry",
    );
    let numeric = out.join("numeric.o");
    run(
        Command::new("clang")
            .args(&flags)
            .arg("-std=c17")
            .arg("-I")
            .arg(root.join("tools/bionic-numeric-facade/include"))
            .args(["-c", "../bionic-numeric-facade/src/provider.c", "-o"])
            .arg(&numeric),
        "numeric provider",
    );
    let allocator = out.join("allocator.o");
    let allocator_options = out.join("allocator_options.o");
    run(
        Command::new("clang")
            .args(&flags)
            .arg("-std=c17")
            .arg("-I")
            .arg(root.join("tools/bionic-libc-allocator-facade/include"))
            .args([
                "-c",
                "../bionic-libc-allocator-facade/src/allocator.c",
                "-o",
            ])
            .arg(&allocator),
        "allocator dependency",
    );
    run(
        Command::new("clang")
            .args(&flags)
            .arg("-std=c17")
            .arg("-I")
            .arg(root.join("tools/bionic-libc-allocator-facade/include"))
            .args([
                "-c",
                "../bionic-libc-allocator-facade/src/allocator_options.c",
                "-o",
            ])
            .arg(&allocator_options),
        "allocator options dependency",
    );
    let stdio_audit_dependencies = out.join("stdio-audit-dependencies.o");
    run(
        Command::new("clang")
            .args(&flags)
            .arg("-std=c17")
            .args([
                "-c",
                "../bionic-stdio-facade/probes/audit_dependency_shims.c",
                "-o",
            ])
            .arg(&stdio_audit_dependencies),
        "stdio audit dependencies",
    );
    let archive = out.join("libdarwin_art_bionic_scanf.a");
    run(
        Command::new("ar")
            .arg("rcs")
            .arg(&archive)
            .args([&scanf, &entry]),
        "archive",
    );
    let support = out.join("libscanf_support.a");
    run(
        Command::new("ar").arg("rcs").arg(&support).args([
            &numeric,
            &allocator,
            &allocator_options,
            &stdio_audit_dependencies,
        ]),
        "support",
    );

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=darwin_art_bionic_scanf");
    println!("cargo:rustc-link-lib=static=scanf_support");
    println!(
        "cargo:rustc-link-search=native={}",
        root.join("_build/bionic-binary128-conversion-facade")
            .display()
    );
    println!("cargo:rustc-link-lib=static=darwin-art-bionic-binary128-conversion");
    println!(
        "cargo:rustc-link-search=native={}",
        root.join("_build/bionic-float-conversion-facade").display()
    );
    println!("cargo:rustc-link-lib=static=darwin-art-bionic-float-conversion");
    println!(
        "cargo:rustc-link-arg=-Wl,-force_load,{}",
        root.join("_build/icu-foundation/libandroidicuinit-darwin.a")
            .display()
    );
    println!(
        "cargo:rustc-link-arg={}",
        root.join("_build/icu-foundation/libicuuc-common-darwin.a")
            .display()
    );
    println!(
        "cargo:rustc-link-arg={}",
        root.join("_build/icu-foundation/libicuuc-stubdata-darwin.a")
            .display()
    );
    println!("cargo:rustc-link-lib=c++");
    if let Some(value) = sanitizer.as_deref() {
        let name = match value {
            "address" => "clang_rt.asan_osx_dynamic",
            "undefined" => "clang_rt.ubsan_osx_dynamic",
            _ => panic!("unsupported sanitizer"),
        };
        let runtime =
            output(Command::new("clang").arg(format!("-print-file-name=lib{name}.dylib")));
        println!(
            "cargo:rustc-link-search=native={}",
            Path::new(&runtime).parent().expect("runtime dir").display()
        );
        println!("cargo:rustc-link-lib=dylib={name}");
        println!(
            "cargo:rustc-link-arg=-Wl,-rpath,{}",
            Path::new(&runtime)
                .parent()
                .expect("runtime directory")
                .display()
        );
    }
    println!("cargo:rerun-if-env-changed=BIONIC_SCANF_C_SANITIZER");
    for source in [
        "build.rs",
        "src/scanf.cc",
        "src/aapcs64_entry.S",
        "include/darwin_art_bionic_scanf.h",
        "../bionic-libc-allocator-facade/src/allocator.c",
        "../bionic-libc-allocator-facade/src/allocator_options.c",
        "../bionic-stdio-facade/include/darwin_art_bionic_stdio.h",
        "../bionic-stdio-facade/probes/audit_dependency_shims.c",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
}
