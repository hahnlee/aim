//! Compare UID-slot operations with the image's original AppIdSettingMap.
use aim_services::package::owner::app_ids::{AppIds, Error, Owner};
use std::fs;
use std::process::Command;
use std::time::{Duration, Instant};

mod common {
    pub mod runtime;
}
use common::runtime::{Boot, Data, run, sources};

#[test]
#[ignore = "requires aimctl, the pinned derived image, JDK and d8; run explicitly"]
fn allocation_matches_the_original_runtime() {
    let dir = std::env::temp_dir().join(format!("aim-ids-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let data = Data(dir);
    let java = aim_paths::fetched().join("java");
    let jdk = java.join("temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let classes = data.0.join("classes");
    let stubs = data.0.join("stubs");
    let dex = data.0.join("dex");
    for path in [&classes, &stubs, &dex] {
        fs::create_dir(path).unwrap();
    }
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(sources(
            &aim_paths::root().join("java/device-services/stubs"),
        )));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/AppIdsOracle.java"),
        ));
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
        .arg(classes.join("com/android/server/pm/AppIdsOracle.class")));
    let boot = Boot {
        ctl: aim_paths::root().join("target/release/aimctl"),
        data: data.0.join("guest"),
    };
    run(boot.command().args(["start", "--windows"]));
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
            "disposable boot did not complete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    fs::copy(
        dex.join("classes.dex"),
        boot.data.join("data/local/tmp/app-ids.dex"),
    )
    .unwrap();
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/app-ids.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.AppIdsOracle",
    ]));

    let a = Owner::SharedUser("a".into());
    let b = Owner::SharedUser("b".into());
    let mut ids = AppIds::default();
    let mut lines = vec![ids.register_existing(10002, a.clone()).is_ok().to_string()];
    for _ in 0..3 {
        lines.push(ids.acquire(b.clone()).unwrap().to_string());
    }
    ids.remove(10001);
    lines.push(ids.acquire(b.clone()).unwrap().to_string());
    ids.replace(10001, a.clone()).unwrap();
    lines.push((ids.get(10001) == Some(&a)).to_string());
    ids.replace(1000, a.clone()).unwrap();
    lines.push((ids.get(1000) == Some(&a)).to_string());
    ids.remove(10010);
    lines.push(ids.acquire(b.clone()).unwrap().to_string());
    let mut restart = AppIds::default();
    restart.register_existing(10002, a.clone()).unwrap();
    lines.push(restart.acquire(b.clone()).unwrap().to_string());
    let mut full = AppIds::default();
    let mut last = -1;
    for _ in 0..10000 {
        last = full.acquire(a.clone()).unwrap();
    }
    lines.push(last.to_string());
    assert_eq!(full.acquire(b.clone()), Err(Error::Exhausted));
    lines.push("-1".into());
    full.remove(19999);
    assert_eq!(full.acquire(b), Err(Error::Exhausted));
    lines.push("-1".into());
    assert_eq!(
        String::from_utf8(original.stdout).unwrap(),
        lines.join("\n") + "\n"
    );

    let state = aim_services::package::State::read(&boot.data.join("data"), &[0])
        .unwrap()
        .unwrap();
    let before = state.clone();
    assert!(!state.settings.packages.is_empty());
    assert!(!state.settings.shared_users.is_empty());
    let restored = AppIds::restore(&state.settings).unwrap();
    for group in &state.settings.shared_users {
        assert_eq!(
            restored.get(group.app_id),
            Some(&Owner::SharedUser(group.name.clone()))
        );
    }
    for package in &state.settings.packages {
        if package.app_id == -1 && package.is_sdk_library {
            assert_eq!(restored.get(-1), None);
        } else if package.shared_user {
            assert!(matches!(
                restored.get(package.app_id),
                Some(Owner::SharedUser(_))
            ));
        } else {
            assert_eq!(
                restored.get(package.app_id),
                Some(&Owner::Package(package.name.clone()))
            );
        }
    }
    assert_eq!(state, before);
    println!(
        "restored {} active packages and {} shared UID groups",
        state.settings.packages.len(),
        state.settings.shared_users.len()
    );
}
