//! Execute the production package helpers on the host JVM, without a guest boot.
use std::{fs, process::Command};

mod common {
    pub mod java;
    pub mod runtime;
}

#[test]
fn retained_package_helpers_use_java_owners_and_system_uid_guards() {
    use common::runtime::{Data, run};
    let data = Data(std::env::temp_dir().join(format!("aim-package-java-{}", std::process::id())));
    fs::create_dir(&data.0).unwrap();
    let jdk = aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let stubs = data.0.join("stubs");
    fs::create_dir(&stubs).unwrap();
    let production = aim_paths::root().join("java/device-services");
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(common::java::sources(&production.join("stubs"))));
    let classes = common::java::production_classes(&data.0, &jdk, &stubs);
    // Binder and Android scheduling/context boundaries are doubled; the
    // bridge, PreferredPolicyBridge and NativePackageHandlers stay production.
    let binder = data.0.join("Binder.java");
    let source = fs::read_to_string(production.join("stubs/android/os/Binder.java")).unwrap()
        .replace("public Binder() { throw new RuntimeException(\"stub\"); }", "public Binder() {}")
        .replace("public static final int getCallingUid() { throw new RuntimeException(\"stub\"); }",
            "public static int uid = 1000; public static final int getCallingUid() { return uid; }")
        .replace("public void attachInterface(IInterface owner, String descriptor) { throw new RuntimeException(\"stub\"); }",
            "public void attachInterface(IInterface owner, String descriptor) {}");
    fs::write(&binder, source).unwrap();
    let classpath = format!("{}:{}", classes.display(), stubs.display());
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(&classpath)
        .arg(&binder)
        .args(common::java::sources(&aim_paths::root().join(
            "crates/aim-services/tests/fixtures/package_bootstrap_java",
        ))));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(&classpath)
        .arg("dev.aim.server.PackageBootstrapJavaTest"));
}
