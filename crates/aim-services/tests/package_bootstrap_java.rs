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
    let users = data.0.join("UserManagerInternal.java");
    fs::write(
        &users,
        fs::read_to_string(production.join("stubs/com/android/server/pm/UserManagerInternal.java"))
            .unwrap()
            .replace(
                "public UserManagerInternal() { throw new RuntimeException(\"stub\"); }",
                "public UserManagerInternal() {}",
            ),
    )
    .unwrap();
    let permissions = data.0.join("PermissionManagerServiceInternal.java");
    fs::write(&permissions, fs::read_to_string(production.join(
        "stubs/com/android/server/pm/permission/PermissionManagerServiceInternal.java")).unwrap()
        .replace("final class PackageInstalledParams {", "final class PackageInstalledParams { private java.util.List<String> names;")
        .replace("public java.util.List<String> getAllowlistedRestrictedPermissions(){throw new RuntimeException(\"stub\");}",
            "public java.util.List<String> getAllowlistedRestrictedPermissions(){return names;}")
        .replace("java.util.List<String> permissions,int mode){throw new RuntimeException(\"stub\");}", "java.util.List<String> permissions,int mode){names=permissions;}")
        .replace("public Builder(){throw new RuntimeException(\"stub\");}", "private java.util.List<String> names=java.util.List.of(); public Builder(){}")
        .replace("setAllowlistedRestrictedPermissions(java.util.List<String> permissions){throw new RuntimeException(\"stub\");}",
            "setAllowlistedRestrictedPermissions(java.util.List<String> permissions){names=permissions;}")
        .replace("public PackageInstalledParams build(){throw new RuntimeException(\"stub\");}",
            "public PackageInstalledParams build(){return new PackageInstalledParams(null,names,0);}")).unwrap();
    let mut discovery = Vec::new();
    for (name, package) in [
        ("IUserManager", "android/os"),
        ("IPackageManager", "android/content/pm"),
    ] {
        let file = data.0.join(format!("{name}.java"));
        let source =
            fs::read_to_string(production.join(format!("stubs/{package}/{name}.java"))).unwrap();
        let declaration = if name == "IUserManager" {
            "abstract class Stub extends Binder"
        } else {
            "abstract class Stub extends android.os.Binder"
        };
        let source = source.replace(
            &format!("{declaration} implements {name} {{"),
            &format!("{declaration} implements {name} {{ public static {name} service;"),
        );
        let source = source
            .lines()
            .map(|line| {
                if line.contains(" asInterface(") {
                    line.replace("throw new RuntimeException(\"stub\");", "return service;")
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&file, source).unwrap();
        discovery.push(file);
    }
    // The decoded package boundary retains only the getters this helper uses.
    let package = data.0.join("AndroidPackage.java");
    let api =
        fs::read_to_string(production.join("stubs/com/android/server/pm/pkg/AndroidPackage.java"))
            .unwrap();
    fs::write(
        &package,
        api.lines()
            .map(|line| {
                if line.trim_end().ends_with(";") && line.contains('(') {
                    format!(
                        "default {} {{ throw new AssertionError(); }}",
                        line.trim().trim_end_matches(';')
                    )
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .unwrap();
    let classpath = format!("{}:{}", classes.display(), stubs.display());
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(&classpath)
        .arg(&binder)
        .arg(&users)
        .arg(&permissions)
        .arg(&package)
        .args(&discovery)
        .args(common::java::sources(&aim_paths::root().join(
            "crates/aim-services/tests/fixtures/package_bootstrap_java",
        ))));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(&classpath)
        .arg("dev.aim.server.PackageBootstrapJavaTest")
        .arg(&data.0));
}
