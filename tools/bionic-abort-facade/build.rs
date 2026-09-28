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
    assert!(
        command
            .args(["-c", source, "-o"])
            .arg(object)
            .status()
            .expect("compile C source")
            .success()
    );
}

fn main() {
    assert_eq!(env::var("CARGO_CFG_TARGET_OS").as_deref(), Ok("macos"));
    assert_eq!(env::var("CARGO_CFG_TARGET_ARCH").as_deref(), Ok("aarch64"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let sdk = output(Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]));
    let sources = [
        ("src/provider.c", vec!["include"]),
        (
            "../bionic-errno-tls/src/errno_tls.c",
            vec![
                "../bionic-errno-tls/include",
                "../bionic-errno-tls/generated",
            ],
        ),
        ("probes/death_support.c", vec![]),
    ];
    let mut objects = Vec::new();
    for (source, includes) in sources {
        let object = out.join(
            Path::new(source)
                .file_name()
                .expect("source name")
                .to_string_lossy()
                .replace('.', "_")
                + ".o",
        );
        compile(source, &object, &sdk, &includes);
        objects.push(object);
    }
    let archive = out.join("libbionic_abort.a");
    assert!(
        Command::new("ar")
            .arg("rcs")
            .arg(&archive)
            .args(&objects)
            .status()
            .expect("archive provider")
            .success()
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-lib=static=bionic_abort");
    for source in [
        "src/provider.c",
        "include/aim_bionic_abort.h",
        "probes/death_support.c",
        "../bionic-errno-tls/src/errno_tls.c",
        "../bionic-errno-tls/include/aim_bionic_errno.h",
        "../bionic-errno-tls/generated/darwin_to_android.inc",
    ] {
        println!("cargo:rerun-if-changed={source}");
    }
}
