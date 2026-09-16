use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn output(command: &mut Command) -> String {
    let result = command.output().expect("run tool");
    assert!(result.status.success(), "tool failed: {command:?}");
    String::from_utf8(result.stdout)
        .expect("UTF-8 tool output")
        .trim()
        .to_owned()
}

fn compile(
    compiler: &str,
    language: Option<&str>,
    source: &str,
    object: &Path,
    sdk: &str,
    includes: &[&str],
    sanitizer: Option<&str>,
) {
    let mut command = Command::new(compiler);
    command.args(["-arch", "arm64", "-isysroot", sdk, "-O2"]);
    if let Some(language) = language {
        command.arg(format!("-std={language}"));
        command.args(["-Wall", "-Wextra", "-Werror", "-fno-builtin"]);
    }
    if let Some(sanitizer) = sanitizer {
        command.args([
            &format!("-fsanitize={sanitizer}"),
            "-fno-omit-frame-pointer",
        ]);
    }
    for include in includes {
        command.arg(format!("-I{include}"));
    }
    assert!(
        command
            .args(["-c", source, "-o"])
            .arg(object)
            .status()
            .expect("compile")
            .success(),
        "compile failed: {source}"
    );
}

fn main() {
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos"));
    assert_eq!(env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("aarch64"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let sdk = output(Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]));
    let sanitizer = env::var("BIONIC_FORMATTED_STDIO_C_SANITIZER").ok();
    if let Some(value) = sanitizer.as_deref() {
        assert!(matches!(value, "address" | "undefined"));
    }

    let provider = out.join("provider.o");
    let fprintf_entry = out.join("fprintf-entry.o");
    let format = out.join("format.o");
    let format_entry = out.join("format-entry.o");
    let allocator = out.join("allocator.o");
    let allocator_options = out.join("allocator_options.o");
    let audit_dependencies = out.join("audit-dependency-shims.o");
    compile(
        "clang++",
        Some("c++20"),
        "src/provider.cc",
        &provider,
        &sdk,
        &[
            "include",
            "../bionic-format-facade/include",
            "../bionic-stdio-facade/include",
            "../bionic-libc-allocator-facade/include",
            "../bionic-errno-tls/include",
        ],
        sanitizer.as_deref(),
    );
    compile(
        "clang",
        None,
        "src/aapcs64_entry.S",
        &fprintf_entry,
        &sdk,
        &[],
        None,
    );
    compile(
        "clang++",
        Some("c++20"),
        "../bionic-format-facade/src/format.cc",
        &format,
        &sdk,
        &[
            "../bionic-format-facade/include",
            "../bionic-libc-allocator-facade/include",
            "../bionic-errno-tls/include",
        ],
        sanitizer.as_deref(),
    );
    compile(
        "clang",
        None,
        "../bionic-format-facade/src/aapcs64_entry.S",
        &format_entry,
        &sdk,
        &[],
        None,
    );
    compile(
        "clang",
        Some("c17"),
        "../bionic-libc-allocator-facade/src/allocator.c",
        &allocator,
        &sdk,
        &["../bionic-libc-allocator-facade/include"],
        sanitizer.as_deref(),
    );
    compile(
        "clang",
        Some("c17"),
        "../bionic-libc-allocator-facade/src/allocator_options.c",
        &allocator_options,
        &sdk,
        &["../bionic-libc-allocator-facade/include"],
        sanitizer.as_deref(),
    );
    compile(
        "clang",
        Some("c17"),
        "../bionic-stdio-facade/probes/audit_dependency_shims.c",
        &audit_dependencies,
        &sdk,
        &[],
        sanitizer.as_deref(),
    );

    let archive = out.join("libbionic_formatted_stdio.a");
    assert!(
        Command::new("ar")
            .arg("rcs")
            .arg(&archive)
            .args([
                &provider,
                &fprintf_entry,
                &format,
                &format_entry,
                &allocator,
                &allocator_options,
                &audit_dependencies,
            ])
            .status()
            .expect("archive")
            .success()
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=bionic_formatted_stdio");
    println!("cargo:rustc-link-lib=c++");

    if let Some(sanitizer) = sanitizer {
        let runtime_name = match sanitizer.as_str() {
            "address" => "clang_rt.asan_osx_dynamic",
            "undefined" => "clang_rt.ubsan_osx_dynamic",
            _ => unreachable!(),
        };
        let runtime_file = format!("lib{runtime_name}.dylib");
        let runtime = output(Command::new("clang").arg(format!("-print-file-name={runtime_file}")));
        let runtime_dir = Path::new(&runtime).parent().expect("sanitizer directory");
        println!("cargo:rustc-link-search=native={}", runtime_dir.display());
        println!("cargo:rustc-link-lib=dylib={runtime_name}");
        println!("cargo:rustc-link-arg=-Wl,-rpath,{}", runtime_dir.display());
    }
    println!("cargo:rerun-if-env-changed=BIONIC_FORMATTED_STDIO_C_SANITIZER");
    for source in [
        "src/provider.cc",
        "src/aapcs64_entry.S",
        "include/darwin_art_bionic_formatted_stdio.h",
        "../bionic-format-facade/src/format.cc",
        "../bionic-format-facade/src/aapcs64_entry.S",
        "../bionic-libc-allocator-facade/src/allocator.c",
        "../bionic-libc-allocator-facade/src/allocator_options.c",
        "../bionic-stdio-facade/probes/audit_dependency_shims.c",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
}
