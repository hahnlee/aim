//! Production typed bridge with JVM boundary doubles; original ART runs in the image.
use std::{fs, process::Command};

mod common {
    pub mod java;
    pub mod runtime;
}

#[test]
#[ignore = "requires pinned JDK; run explicitly without a guest boot"]
fn dex_use_bridge_keeps_original_identity_validation_and_errors() {
    use common::runtime::{Data, run};
    let data = Data(std::env::temp_dir().join(format!("aim-dex-use-{}", std::process::id())));
    fs::create_dir(&data.0).unwrap();
    let jdk = aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let production = aim_paths::root().join("java/device-services");
    let stubs = data.0.join("stubs");
    let classes = data.0.join("classes");
    fs::create_dir(&stubs).unwrap();
    fs::create_dir(&classes).unwrap();
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(common::java::sources(&production.join("stubs"))));
    let fixtures = aim_paths::root().join("crates/aim-services/tests/fixtures/dex_use");
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(&stubs)
        .args(common::java::sources(&fixtures))
        .arg(production.join("src/com/android/server/art/AppHostDexUse.java")));
    for mode in ["foreign", "normal"] {
        run(Command::new(jdk.join("bin/java"))
            .arg("-cp")
            .arg(format!("{}:{}", classes.display(), stubs.display()))
            .arg("com.android.server.art.DexUseTest")
            .arg(mode));
    }
}
