//! Host formatting gate. Android parcelable rendering is the boundary double;
//! production AIDL snapshot and formatter are compiled, with no guest process.
use std::{fs, process::Command};

#[test]
fn connectivity_dump_exact_output() {
    let root = aim_paths::root();
    let dir = std::env::temp_dir().join(format!("aim-connectivity-dump-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let jdk = aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let source = root.join("java/device-services");
    let mut inputs = vec![
        source.join("stubs/android/os/Parcelable.java"),
        source.join("stubs/android/os/Parcel.java"),
    ];
    // Compile-only dependencies of Parcel. Runtime never constructs a real Android Parcel.
    fn sources(path: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(path).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                sources(&p, out);
            } else if p.extension().is_some_and(|e| e == "java") {
                out.push(p);
            }
        }
    }
    inputs.clear();
    sources(&source.join("stubs"), &mut inputs);
    inputs.retain(|p| {
        ![
            "NetworkInfo.java",
            "LinkProperties.java",
            "NetworkCapabilities.java",
            "NetworkRequest.java",
        ]
        .iter()
        .any(|n| p.ends_with(n))
    });
    for name in [
        "NetworkInfo",
        "LinkProperties",
        "NetworkCapabilities",
        "NetworkRequest",
    ] {
        let mut text = fs::read_to_string(source.join(format!("stubs/android/net/{name}.java")))
            .unwrap()
            .lines()
            .map(|line| {
                if line.trim_start().starts_with(&format!("public {name}(")) {
                    line.replace("throw new RuntimeException(\"stub\");", "")
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            .replace("public final int requestId = 0", "public int requestId = 7")
            .replace(
                "public final Type type = null",
                "public Type type = Type.REQUEST",
            );
        // Only construction and rendering are doubled; unused methods keep throwing.
        text = text.replace(
            "String toShortString() { throw new RuntimeException(\"stub\"); }",
            "String toShortString() { return text; }",
        );
        let at = text.rfind('}').unwrap();
        text.insert_str(
            at,
            "public static String text; public String value; public String toString() { return value == null ? text : value; }\n",
        );
        let path = dir.join(format!("{name}.java"));
        fs::write(&path, text).unwrap();
        inputs.push(path);
    }
    let aidl = source.join("aidl");
    run(
        Command::new(aim_paths::fetched().join("java/build-tools-36.0.0/android-16/aidl"))
            .args(["--lang=java", "--min_sdk_version", "36"])
            .arg(format!("-I{}", aidl.display()))
            .arg("-o")
            .arg(&dir)
            .args(
                ["ConnectivityDumpSnapshot", "ConnectivityDumpRequest"]
                    .map(|n| aidl.join(format!("dev/aim/server/{n}.aidl"))),
            ),
    );
    for name in ["ConnectivityDumpSnapshot", "ConnectivityDumpRequest"] {
        inputs.push(dir.join(format!("dev/aim/server/{name}.java")));
    }
    inputs.push(source.join("src/dev/aim/server/ConnectivityDump.java"));
    let fixtures = root.join("crates/aim-services/tests/fixtures/connectivity_dump");
    inputs.push(fixtures.join("ConnectivityDumpTest.java"));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&dir)
        .args(inputs));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(&dir)
        .arg("dev.aim.server.ConnectivityDumpTest")
        .arg(&fixtures));
    fs::remove_dir_all(dir).unwrap();
}
fn run(command: &mut Command) {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
