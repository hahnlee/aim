//! Differential against the pinned original PackageInstaller DTO constructors.
use std::{
    fs,
    process::Command,
    time::{Duration, Instant},
};
mod common {
    pub mod installer_codec;
    pub mod java;
    pub mod runtime;
}
use common::runtime::{Boot, Data, run};

#[test]
#[ignore = "requires original pinned image, aimctl, JDK and d8; run explicitly"]
fn installer_parcels_match_original_framework() {
    let directory =
        std::env::temp_dir().join(format!("aim-installer-codec-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let data = Data(directory);
    let repo = aim_paths::root();
    let java = aim_paths::fetched().join("java");
    let jdk = java.join("temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let classes = data.0.join("classes");
    let stubs = data.0.join("stubs");
    let dex = data.0.join("dex");
    for path in [&classes, &stubs, &dex] {
        fs::create_dir(path).unwrap();
    }
    let api = repo.join("crates/aim-services/tests/api/installer");
    let replacement = [
        "android/content/pm/PackageInstaller.java",
        "android/graphics/Bitmap.java",
        "android/net/Uri.java",
    ];
    let original_stubs = repo.join("java/device-services/stubs");
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(
            common::java::sources(&original_stubs)
                .into_iter()
                .filter(|path| {
                    !replacement
                        .iter()
                        .any(|relative| path == &original_stubs.join(relative))
                }),
        )
        .args(common::java::sources(&api)));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .arg(repo.join("crates/aim-services/tests/fixtures/InstallerCodecOracle.java")));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(java.join("build-tools-36.0.0/android-16/lib/d8.jar"))
        .args([
            "com.android.tools.r8.D8",
            "--release",
            "--min-api",
            "36",
            "--lib",
        ])
        .arg(&jdk)
        .arg("--classpath")
        .arg(&stubs)
        .arg("--output")
        .arg(&dex)
        .arg(classes.join("InstallerCodecOracle.class")));
    common::java::check_linkage(&dex.join("classes.dex"), &[]).unwrap();
    let boot = Boot::new(repo.join("target/release/aimctl"), data.0.join("guest"));
    run(boot.start_command().args(["start", "--windows"]));
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let output = boot
            .command()
            .args(["shell", "getprop", "sys.boot_completed"])
            .output()
            .unwrap();
        if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "1" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "installer oracle boot incomplete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    let guest = boot.data.join("data/local/tmp/installer-codec");
    fs::create_dir(&guest).unwrap();
    common::installer_codec::export(&guest);
    fs::copy(dex.join("classes.dex"), guest.join("oracle.dex")).unwrap();
    let output = boot
        .command()
        .args([
            "shell",
            "/system/bin/app_process",
            "-Djava.class.path=/data/local/tmp/installer-codec/oracle.dex",
            "/system/bin",
            "InstallerCodecOracle",
            "/data/local/tmp/installer-codec",
        ])
        .output()
        .unwrap();
    if !output.status.success() {
        let logs = boot
            .command()
            .args(["shell", "logcat", "-d", "-s", "AndroidRuntime:V"])
            .output()
            .unwrap();
        panic!(
            "installer codec oracle: {}; stdout: {}; stderr: {}; logcat: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&logs.stdout)
        );
    }
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("original installer codec differential passed")
    );
    common::installer_codec::verify(&guest);
    drop(boot);
}
