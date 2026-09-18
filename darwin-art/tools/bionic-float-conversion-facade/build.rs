use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

const GDTOA_SOURCES: [&str; 13] = [
    "dmisc.c",
    "gethex.c",
    "gmisc.c",
    "hexnan.c",
    "hd_init.c",
    "misc.c",
    "smisc.c",
    "strtod.c",
    "strtodg.c",
    "strtof.c",
    "strtord.c",
    "sum.c",
    "ulp.c",
];

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
    let source_root = manifest.join("../../_aosp/bionic-float-conversion-facade");
    let gdtoa = source_root.join("libc/upstream-openbsd/lib/libc/gdtoa");
    let android_include = source_root.join("libc/upstream-openbsd/android/include");
    let sdk = output(
        Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]),
        "SDK lookup",
    );

    let mut objects = Vec::new();
    for source in GDTOA_SOURCES {
        let object = out.join(source.replace(".c", ".o"));
        let mut command = Command::new("clang");
        command.args([
            "-arch",
            "arm64",
            "-isysroot",
            &sdk,
            "-std=gnu17",
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-Wno-sign-compare",
            "-fvisibility=hidden",
            "-Iinclude",
            "-I",
        ]);
        command.arg(&android_include).arg("-I").arg(&gdtoa).args([
            "-include",
            "darwin_art_gdtoa_compat.h",
            "-Dstrtod=darwin_art_aosp_strtod",
            "-Dstrtof=darwin_art_aosp_strtof",
        ]);
        if source == "misc.c" {
            command.arg("-DDARWIN_ART_GDTOA_LOCAL_LOCKS");
        }
        command
            .arg("-c")
            .arg(gdtoa.join(source))
            .arg("-o")
            .arg(&object);
        run(&mut command, source);
        objects.push(object);
    }

    for (source, language) in [
        ("src/provider.cc", "c++"),
        ("src/lock_adapter.cc", "c++"),
        ("../bionic-errno-tls/src/errno_tls.c", "c"),
    ] {
        let object = out.join(
            Path::new(source)
                .file_name()
                .expect("source name")
                .to_string_lossy()
                .replace(['.', '-'], "_")
                + ".o",
        );
        let mut command = Command::new(if language == "c++" {
            "clang++"
        } else {
            "clang"
        });
        command.args([
            "-arch",
            "arm64",
            "-isysroot",
            &sdk,
            if language == "c++" {
                "-std=c++20"
            } else {
                "-std=c17"
            },
            "-O2",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-fvisibility=hidden",
            "-Iinclude",
            "-I../bionic-errno-tls/include",
            "-I../bionic-errno-tls/generated",
            "-c",
            source,
            "-o",
        ]);
        command.arg(&object);
        run(&mut command, source);
        objects.push(object);
    }

    let archive = out.join("libdarwin_art_bionic_float_conversion.a");
    let mut archive_command = Command::new("ar");
    archive_command.arg("rcs").arg(&archive).args(&objects);
    run(&mut archive_command, "archive");
    if env::var_os("CARGO_FEATURE_AUDIT").is_some() {
        let host_state = out.join("audit_host_state.o");
        run(
            Command::new("clang++")
                .args([
                    "-arch",
                    "arm64",
                    "-isysroot",
                    &sdk,
                    "-std=c++20",
                    "-Wall",
                    "-Wextra",
                    "-Werror",
                    "-c",
                    "probes/host_state.cc",
                    "-o",
                ])
                .arg(&host_state),
            "audit host state",
        );
        // Bin-only linkage: the separately published native archive stays fixture-free.
        println!(
            "cargo:rustc-link-arg-bin=bionic-float-conversion-facade={}",
            host_state.display()
        );
        println!(
            "cargo:rustc-link-arg-bin=bionic-float-conversion-facade=-L{}",
            out.display()
        );
        println!(
            "cargo:rustc-link-arg-bin=bionic-float-conversion-facade=-Wl,-force_load,{}",
            out.join("libdarwin_art_bionic_float_conversion.a")
                .display()
        );
        println!("cargo:rustc-link-arg-bin=bionic-float-conversion-facade=-lc++");
        println!("cargo:rerun-if-changed=probes/host_state.cc");
    }
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=darwin_art_bionic_float_conversion");
    println!("cargo:rustc-link-lib=c++");
    for source in GDTOA_SOURCES {
        println!("cargo:rerun-if-changed={}", gdtoa.join(source).display());
    }
    for source in [
        "build.rs",
        "src/provider.cc",
        "src/lock_adapter.cc",
        "include/darwin_art_gdtoa_compat.h",
        "include/thread_private.h",
        "include/darwin_art_bionic_float_conversion.h",
        "../bionic-errno-tls/src/errno_tls.c",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
}
