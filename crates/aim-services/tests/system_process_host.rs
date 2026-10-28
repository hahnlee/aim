//! Execute the production system-process startup on a host JVM with Android boundaries doubled.
use std::{fs, process::Command};

mod common {
    pub mod java;
    pub mod runtime;
}

#[test]
#[ignore = "requires pinned JDK; run explicitly without a guest boot"]
fn system_process_initializes_nonces_and_drives_original_service_lifecycles() {
    use common::runtime::{Data, run};
    let data =
        Data(std::env::temp_dir().join(format!("aim-system-process-{}", std::process::id())));
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
    let fixtures = aim_paths::root().join("crates/aim-services/tests/fixtures/system_process_host");
    // Binder's transport is outside this test; retain its API, allow construction.
    let binder = data.0.join("Binder.java");
    fs::write(
        &binder,
        fs::read_to_string(production.join("stubs/android/os/Binder.java"))
            .unwrap()
            .replace(
                "public Binder() { throw new RuntimeException(\"stub\"); }",
                "public Binder() {}",
            ),
    )
    .unwrap();
    // Only compile dependencies here; the class under test is always production source.
    let mut sources = common::java::sources(&fixtures);
    sources.push(binder);
    sources.push(production.join("src/dev/aim/server/ProviderQueries.java"));
    let generated = common::java::private_aidl_sources(&data.0);
    sources.extend(generated.into_iter().filter(|p| {
        p.ends_with("IForegroundServicePolicy.java")
            || p.ends_with("IForegroundServiceTypeHost.java")
    }));
    for (missing_memory, device_lock) in [(false, true), (false, false), (true, true)] {
        let server = data.0.join("SystemProcessHost.java");
        let mut source =
            fs::read_to_string(production.join("src/dev/aim/server/SystemProcessHost.java"))
                .unwrap();
        // JNI loading is an Android boundary; the image linkage gate checks the real call.
        source = source.replace(
            "System.loadLibrary(\"android_servers\");",
            "SystemProcessHostTest.loadLibrary(\"android_servers\");",
        );
        if missing_memory {
            source = source.replace(
                "ApplicationSharedMemory.setInstance(ApplicationSharedMemory.create());",
                "",
            );
        }
        fs::write(&server, source).unwrap();
        run(Command::new(jdk.join("bin/javac"))
            .args(["--release", "17", "-d"])
            .arg(&classes)
            .arg("-cp")
            .arg(&stubs)
            .args(&sources)
            .arg(&server));
        let output = Command::new(jdk.join("bin/java"))
            .arg("-cp")
            .arg(format!("{}:{}", classes.display(), stubs.display()))
            .arg("dev.aim.server.SystemProcessHostTest")
            .arg(device_lock.to_string())
            .output()
            .unwrap();
        if missing_memory {
            assert!(
                !output.status.success(),
                "control accepted missing nonce setup"
            );
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("unable to assign nonce handle")
            );
        } else {
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
