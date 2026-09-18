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

fn run(command: &mut Command, description: &str) {
    assert!(
        command.status().expect("launch tool").success(),
        "{description} failed"
    );
}

fn main() {
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos"));
    assert_eq!(env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("aarch64"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest"));
    let root = manifest.join("../..");
    let source_root = root.join("_aosp/bionic-binary128-conversion-facade");
    let gdtoa = source_root.join("libc/upstream-openbsd/lib/libc/gdtoa");
    let android_include =
        root.join("_aosp/bionic-float-conversion-facade/libc/upstream-openbsd/android/include");
    let shared_gdtoa =
        root.join("_aosp/bionic-float-conversion-facade/libc/upstream-openbsd/lib/libc/gdtoa");
    let icu_root = root.join("_aosp/external/icu-graphics");
    let sdk = output(
        Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]),
        "SDK lookup",
    );
    let sanitizer = env::var("BIONIC_BINARY128_C_SANITIZER").ok();
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
    let sanitizer_flag;
    if let Some(value) = sanitizer.as_deref() {
        sanitizer_flag = format!("-fsanitize={value}");
        common.push(&sanitizer_flag);
        common.push("-fno-omit-frame-pointer");
    }

    let strtorq = out.join("strtorQ.o");
    run(
        Command::new("clang")
            .args(&common)
            .args(["-std=gnu17", "-Wno-sign-compare", "-Iinclude", "-I"])
            .arg(root.join("tools/bionic-float-conversion-facade/include"))
            .arg("-I")
            .arg(&android_include)
            .arg("-I")
            .arg(&shared_gdtoa)
            .args([
                "-include",
                "darwin_art_gdtoa_compat.h",
                "-D__strtorQ=darwin_art_aosp_strtorQ",
                "-c",
            ])
            .arg(gdtoa.join("strtorQ.c"))
            .args(["-o"])
            .arg(&strtorq),
        "strtorQ",
    );

    let provider = out.join("provider.o");
    run(
        Command::new("clang++")
            .args(&common)
            .args(["-std=c++20", "-DANDROID", "-Iinclude", "-I"])
            .arg(root.join("tools/bionic-libc-allocator-facade/include"))
            .arg("-I")
            .arg(root.join("tools/bionic-errno-tls/include"))
            .arg("-I")
            .arg(icu_root.join("android_icu4c/include"))
            .arg("-I")
            .arg(icu_root.join("icu4c/source/common"))
            .arg("-I")
            .arg(icu_root.join("libandroidicuinit/include"))
            .args(["-c", "src/provider.cc", "-o"])
            .arg(&provider),
        "provider",
    );

    let audit = env::var_os("CARGO_FEATURE_AUDIT").is_some();
    let entry = out.join("entry.o");
    if audit {
        let host_state = out.join("audit_host_state.o");
        run(
            Command::new("clang++")
                .args(&common)
                .args(["-std=c++20", "-c", "probes/host_state.cc", "-o"])
                .arg(&host_state),
            "audit host state",
        );
        // This object belongs only to the audit binary, not the runtime archive.
        println!(
            "cargo:rustc-link-arg-bin=bionic-binary128-conversion-facade={}",
            host_state.display()
        );
        println!(
            "cargo:rustc-link-arg-bin=bionic-binary128-conversion-facade=-L{}",
            out.display()
        );
        println!(
            "cargo:rustc-link-arg-bin=bionic-binary128-conversion-facade=-Wl,-force_load,{}",
            out.join("libdarwin_art_bionic_binary128_conversion.a")
                .display()
        );
        println!(
            "cargo:rustc-link-arg-bin=bionic-binary128-conversion-facade=-Wl,-force_load,{}",
            out.join("libbinary128_allocator_test.a").display()
        );
        println!(
            "cargo:rustc-link-arg-bin=bionic-binary128-conversion-facade=-L{}",
            root.join("_build/bionic-float-conversion-facade").display()
        );
        println!(
            "cargo:rustc-link-arg-bin=bionic-binary128-conversion-facade=-Wl,-force_load,{}",
            root.join(
                "_build/bionic-float-conversion-facade/libdarwin-art-bionic-float-conversion.a"
            )
            .display()
        );
        println!(
            "cargo:rustc-link-arg-bin=bionic-binary128-conversion-facade=-Wl,-force_load,{}",
            root.join("_build/icu-foundation/libandroidicuinit-darwin.a")
                .display()
        );
        println!(
            "cargo:rustc-link-arg-bin=bionic-binary128-conversion-facade={}",
            root.join("_build/icu-foundation/libicuuc-common-darwin.a")
                .display()
        );
        println!(
            "cargo:rustc-link-arg-bin=bionic-binary128-conversion-facade={}",
            root.join("_build/icu-foundation/libicuuc-stubdata-darwin.a")
                .display()
        );
        println!("cargo:rustc-link-arg-bin=bionic-binary128-conversion-facade=-lc++");
        println!("cargo:rerun-if-changed=probes/host_state.cc");
    }
    run(
        Command::new("clang")
            .args([
                "-arch",
                "arm64",
                "-isysroot",
                &sdk,
                "-c",
                "src/entry.S",
                "-o",
            ])
            .arg(&entry),
        "AAPCS64 entry",
    );
    let archive = out.join("libdarwin_art_bionic_binary128_conversion.a");
    run(
        Command::new("ar")
            .arg("rcs")
            .arg(&archive)
            .args([&provider, &entry, &strtorq]),
        "archive",
    );

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=darwin_art_bionic_binary128_conversion");
    println!("cargo:rustc-link-lib=c++");

    if audit {
        let allocator = out.join("allocator.o");
        run(
            Command::new("clang")
                .args(&common)
                .arg("-std=c17")
                .arg("-I")
                .arg(root.join("tools/bionic-libc-allocator-facade/include"))
                .args([
                    "-c",
                    "../bionic-libc-allocator-facade/src/allocator.c",
                    "-o",
                ])
                .arg(&allocator),
            "allocator test dependency",
        );
        let allocator_options = out.join("allocator_options.o");
        run(
            Command::new("clang")
                .args(&common)
                .arg("-std=c17")
                .arg("-I")
                .arg(root.join("tools/bionic-libc-allocator-facade/include"))
                .args([
                    "-c",
                    "../bionic-libc-allocator-facade/src/allocator_options.c",
                    "-o",
                ])
                .arg(&allocator_options),
            "allocator options test dependency",
        );
        let allocator_archive = out.join("libbinary128_allocator_test.a");
        run(
            Command::new("ar")
                .arg("rcs")
                .arg(&allocator_archive)
                .arg(&allocator)
                .arg(&allocator_options),
            "allocator archive",
        );

        println!("cargo:rustc-link-lib=static=binary128_allocator_test");
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
        println!("cargo:rerun-if-changed=../bionic-libc-allocator-facade/src/allocator.c");
        println!("cargo:rerun-if-changed=../bionic-libc-allocator-facade/src/allocator_options.c");
    }

    if audit && let Some(value) = sanitizer.as_deref() {
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
        println!(
            "cargo:rustc-link-arg-bin=bionic-binary128-conversion-facade=-L{}",
            runtime_dir.display()
        );
        println!("cargo:rustc-link-arg-bin=bionic-binary128-conversion-facade=-l{runtime_name}");
        println!(
            "cargo:rustc-link-arg-bin=bionic-binary128-conversion-facade=-Wl,-rpath,{}",
            runtime_dir.display()
        );
    }
    println!("cargo:rerun-if-env-changed=BIONIC_BINARY128_C_SANITIZER");
    for source in [
        "build.rs",
        "src/provider.cc",
        "src/entry.S",
        "include/darwin_art_bionic_binary128_conversion.h",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
}
