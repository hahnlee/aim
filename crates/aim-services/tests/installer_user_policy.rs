//! Original Parcel/LocalServices policy record with explicit test owners; not live SystemServer.
use std::{
    fs,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
mod common {
    pub mod java;
    pub mod runtime;
}
use common::runtime::{Boot, Data, run};

struct TransferGuard {
    transfer: std::sync::Arc<aim_services::package::installer::file_bridge::Transfer>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for TransferGuard {
    fn drop(&mut self) {
        self.transfer.stop();
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap();
        }
    }
}

#[test]
#[ignore = "requires original pinned image, aimctl, JDK and d8; explicit test owners only"]
fn installer_user_policy_record_matches_original_parcel() {
    let directory = std::env::temp_dir().join(format!("aim-iup-{}", std::process::id()));
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
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(common::java::sources(
            &repo.join("java/device-services/stubs"),
        )));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .arg(repo.join("java/device-services/src/dev/aim/server/InstallerUserPolicy.java"))
        .arg(repo.join("crates/aim-services/tests/fixtures/InstallerUserPolicyOracle.java")));
    let mut class_files = Vec::new();
    let mut pending = vec![classes.clone()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "class") {
                class_files.push(path);
            }
        }
    }
    class_files.sort();
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
        .args(class_files));
    common::java::check_linkage(
        &dex.join("classes.dex"),
        &["/system/framework/services.jar"],
    )
    .unwrap();
    let boot = Boot {
        ctl: repo.join("target/release/aimctl"),
        data: data.0.join("g"),
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
            "user policy oracle boot incomplete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    let guest = boot.data.join("data/local/tmp/installer-user-policy");
    fs::create_dir(&guest).unwrap();
    aim_storage::guest_inode::record(
        &guest,
        aim_storage::guest_inode::GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o700),
            ..Default::default()
        },
    )
    .unwrap();
    fs::copy(dex.join("classes.dex"), guest.join("oracle.dex")).unwrap();
    let target_path = data.0.join("filebridge-target");
    let target = fs::File::create(&target_path).unwrap();
    let transfer =
        std::sync::Arc::new(aim_services::package::installer::file_bridge::Transfer::default());
    let (socket, worker) = transfer.start(target).unwrap();
    let guard = TransferGuard {
        transfer: transfer.clone(),
        worker: Some(worker),
    };
    let input: std::os::fd::OwnedFd = socket.into();
    let output = boot.client(1000).args([
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/installer-user-policy/oracle.dex:/system/framework/services.jar",
        "/system/bin", "dev.aim.server.InstallerUserPolicyOracle", "/data/local/tmp/installer-user-policy",
    ]).stdin(Stdio::from(input)).output().unwrap();
    assert!(
        output.status.success(),
        "policy record/test-owner oracle: {}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains(
        "original installer user-policy record/test-owner checks passed (not live SystemServer)"
    ));
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("original FileBridgeOutputStream write/fsync/close checks passed")
    );
    assert!(transfer.is_closed());
    drop(guard);
    assert!(transfer.error().is_none());
    let expected = (0..17003).map(|i| (i * 31 + 7) as u8).collect::<Vec<_>>();
    assert_eq!(fs::read(&target_path).unwrap(), expected);
    let mode = fs::read(guest.join("installer-revocable-fd.original")).unwrap();
    assert!(
        matches!(mode.as_slice(), [0] | [1]),
        "invalid original file mode selector"
    );
    println!("original installer revocable FD selector: {}", mode[0] == 1);
    for flags in 0..16 {
        let bytes = fs::read(guest.join(format!("policy-{flags}.original"))).unwrap();
        let mut expected = aim_binder_host::parcel::Parcel::new();
        expected.write_i32(43);
        for bit in 0..4 {
            expected.write_i32((flags >> bit) & 1);
        }
        assert_eq!(
            bytes,
            expected.data(),
            "original policy record field/ABI mismatch: {flags}"
        );
    }
    drop(boot);
}
