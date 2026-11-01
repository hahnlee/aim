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
    let binder_source = fs::read_to_string(&binder).unwrap().replace(
        "public final boolean transact(int code, Parcel data, Parcel reply, int flags) throws RemoteException { throw new RuntimeException(\"stub\"); }",
        "public final boolean transact(int code, Parcel data, Parcel reply, int flags) throws RemoteException { return onTransact(code, data, reply, flags); }",
    );
    fs::write(&binder, binder_source).unwrap();
    // Only compile dependencies here; the class under test is always production source.
    let mut sources = common::java::sources(&fixtures);
    let original = fs::read_to_string(
        aim_paths::aosp()
            .join("frameworks/base/services/core/java/com/android/server/content/SyncManager.java"),
    )
    .unwrap();
    use sha2::{Digest, Sha256};
    assert_eq!(
        format!("{:x}", Sha256::digest(original.as_bytes())),
        "ac8deedeeca97dadea2d9da8b7e2956305fd63e44d9fdb8d80222ddf76f1ed51"
    );
    let mut replay =
        fs::read_to_string(fixtures.join("dev/aim/server/SyncReplayOriginal.java.in")).unwrap();
    for (key, start, end) in [
        (
            "@CONTAINS@",
            "    private boolean containsAccountAndUser(",
            "    /** target indicates",
        ),
        (
            "@REFRESH@",
            "        private void updateRunningAccountsH(",
            "        /**",
        ),
        (
            "@UNLOCK_FLAG@",
            "    public void onUnlockUser(",
            "    public void onStopUser(",
        ),
        (
            "@UNLOCK_ACCOUNTS@",
            "    private void onUserUnlocked(",
            "    private void onUserStopped(",
        ),
        (
            "@ACCOUNTS_MESSAGE@",
            "    private void updateRunningAccounts(",
            "    private void removeStaleAccounts(",
        ),
        (
            "@USER_RECEIVER@",
            "    private BroadcastReceiver mUserIntentReceiver =",
            "    private static class PackageMonitorImpl",
        ),
    ] {
        let body = &original[original.find(start).unwrap()..];
        let body = body[..body.find(end).unwrap()]
            .replace("AccountManagerService.getSingleton()", "mSyncAccounts");
        replay = replay.replace(key, &body);
    }
    let replay_source = data.0.join("SyncReplayOriginal.java");
    fs::write(&replay_source, replay).unwrap();
    sources.push(replay_source);
    sources.push(binder);
    sources.push(production.join("src/dev/aim/server/SyncOwnerStartup.java"));
    sources.push(production.join("src/com/android/server/content/PrivateContentService.java"));
    sources.push(production.join("src/dev/aim/server/ProviderQueries.java"));
    sources.push(aim_paths::root().join("crates/aim-services/tests/fixtures/dex_use/com/android/server/art/ArtModuleServiceInitializer.java"));
    let generated = common::java::private_aidl_sources(&data.0);
    sources.extend(generated.into_iter().filter(|p| {
        p.ends_with("IForegroundServicePolicy.java")
            || p.ends_with("IForegroundServiceTypeHost.java")
    }));
    for (missing_memory, missing_art, device_lock, printing, fail_sync) in [
        (false, false, true, true, false),
        (false, false, false, true, false),
        (false, false, true, false, false),
        (false, false, false, false, false),
        (true, false, true, true, false),
        (false, true, true, true, false),
        (false, false, true, true, true),
    ] {
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
        if missing_art {
            source = source.replace(
                "com.android.server.art.ArtModuleServiceInitializer.setArtModuleServiceManager(\n                    new android.os.ArtModuleServiceManager());",
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
            .arg(printing.to_string())
            .arg(fail_sync.to_string())
            .output()
            .unwrap();
        if fail_sync {
            assert!(
                !output.status.success(),
                "accepted failed sync construction"
            );
            assert!(String::from_utf8_lossy(&output.stderr).contains("sync construction failed"));
        } else if missing_art {
            assert!(
                !output.status.success(),
                "control accepted missing ART initialization"
            );
            assert!(String::from_utf8_lossy(&output.stderr).contains("ArtModuleServiceManager"));
        } else if missing_memory {
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
