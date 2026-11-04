//! Host checks for process-lifetime ownership; actual original-store boot acceptance is separate.
use std::{fs, process::Command};
mod common {
    pub mod runtime;
}
#[test]
#[ignore = "requires pinned JDK; run explicitly without a guest boot"]
fn one_storage_cache_writer_per_process_and_data_cohort() {
    let work = aim_paths::scratch_dir("sync-storage-owner");
    fs::create_dir_all(&work).unwrap();
    let jdk =
        aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home/bin");
    let java = work.join("Owner.java");
    fs::write(&java, r#"
import com.android.server.content.SyncStorageOwner;
public class Owner {
    public static void main(String[] args) throws Exception {
        var data = new java.io.File(args[0]);
        if (args.length == 2) {
            try { new SyncStorageOwner(data); }
            catch (IllegalStateException expected) { return; }
            throw new AssertionError("second process acquired writer");
        }
        var owner = new SyncStorageOwner(data);
        try { new SyncStorageOwner(data); throw new AssertionError("second cache writer"); }
        catch (IllegalStateException expected) {}
        var child = new ProcessBuilder(System.getProperty("java.home")+"/bin/java", "-cp",
                System.getProperty("java.class.path"), "Owner", args[0], "locked").inheritIO().start();
        if (!child.waitFor(10, java.util.concurrent.TimeUnit.SECONDS)) {
            child.destroyForcibly().waitFor(); throw new AssertionError("writer child timed out");
        }
        if (child.exitValue() != 0) throw new AssertionError("lost writer lock");
        java.lang.ref.Reference.reachabilityFence(owner);
    }
}
"#).unwrap();
    common::runtime::run(
        Command::new(jdk.join("javac"))
            .arg("-d")
            .arg(&work)
            .arg(
                aim_paths::root().join(
                    "java/device-services/src/com/android/server/content/SyncStorageOwner.java",
                ),
            )
            .arg(java),
    );
    // Reopening after process exit must succeed without changing any stored records.
    let data = work.join("data");
    fs::create_dir_all(data.join("system/sync")).unwrap();
    let accounts = data.join("system/sync/accounts.xml");
    let bytes = b"<accounts><authority user=\"10\" /></accounts>";
    fs::write(&accounts, bytes).unwrap();
    for _ in 0..2 {
        common::runtime::run(
            Command::new(jdk.join("java"))
                .arg("-cp")
                .arg(&work)
                .arg("Owner")
                .arg(&data),
        );
        assert_eq!(fs::read(&accounts).unwrap(), bytes);
    }
    fs::remove_dir_all(work).unwrap();
}

#[test]
#[ignore = "requires pinned AOSP recovery sources and JDK; run explicitly"]
fn original_atomic_recovery_and_legacy_status_statistics_migration() {
    use sha2::{Digest, Sha256};
    let base = aim_paths::aosp().join("frameworks/base");
    let load = |path: &str, pin: &str| {
        let source = fs::read_to_string(base.join(path)).unwrap();
        assert_eq!(format!("{:x}", Sha256::digest(source.as_bytes())), pin);
        source
    };
    let store = load(
        "services/core/java/com/android/server/content/SyncStorageEngine.java",
        "a16b1277ff717caee2e1cebec06d46017464c2f7945df5d540d074764fa464d0",
    );
    let atomic = load(
        "core/java/android/util/AtomicFile.java",
        "90e1cc85e92e53e6c257e49156677de354a79d2b6caf65d826cb77d0cffab70e",
    );
    let status = load(
        "core/java/android/content/SyncStatusInfo.java",
        "0f5a06d8e33e7c787436af4c54be3df614b4cd1bdf7e230f19b4fa814d5192b7",
    );
    fn body<'a>(source: &'a str, signature: &str) -> &'a str {
        let start = source.find(signature).unwrap();
        let open = start + source[start..].find('{').unwrap();
        let mut depth = 0;
        for (offset, ch) in source[open..].char_indices() {
            if ch == '{' {
                depth += 1;
            }
            if ch == '}' {
                depth -= 1;
                if depth == 0 {
                    return &source[start..open + offset + 1];
                }
            }
        }
        panic!("unclosed original method");
    }
    let atomic_methods = [
        "public File getBaseFile()",
        "public FileOutputStream startWrite()",
        "public FileOutputStream startWrite(long",
        "public void finishWrite(",
        "public void failWrite(",
        "public FileInputStream openRead()",
        "public boolean exists()",
        "public byte[] readFully()",
        "private static void rename(",
    ]
    .map(|s| body(&atomic, s))
    .join("\n");
    let store_methods = [
        "private void readStatusParcelLocked(",
        "private void upgradeStatusIfNeededLocked()",
        "private void readStatsParcelLocked(",
        "private void upgradeStatisticsIfNeededLocked()",
        "public void writeAllState()",
    ]
    .map(|s| body(&store, s))
    .join("\n");
    let fields = &status[status.find("static final int VERSION").unwrap()
        ..status
            .find("public SyncStatusInfo(int authorityId)")
            .unwrap()];
    let source = include_str!("fixtures/sync_storage/Recovery.java.in")
        .replace("@ATOMIC@", &atomic_methods)
        .replace("@STORE_METHODS@", &store_methods)
        .replace(
            "@STATUS_FIELDS@",
            &fields.replace("@UnsupportedAppUsage", ""),
        )
        .replace(
            "@STATUS_CONSTRUCT@",
            body(&status, "public SyncStatusInfo(Parcel parcel)"),
        )
        .replace(
            "@STATUS_WRITE@",
            body(
                &status,
                "public void writeToParcel(Parcel parcel, int flags)",
            ),
        )
        .replace("@DAY_STATS@", body(&store, "public static class DayStats"));
    let work = aim_paths::scratch_dir("sync-recovery-java");
    fs::create_dir_all(&work).unwrap();
    let file = work.join("Recovery.java");
    fs::write(&file, source).unwrap();
    let jdk =
        aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home/bin");
    common::runtime::run(
        Command::new(jdk.join("javac"))
            .arg("-d")
            .arg(&work)
            .arg(file),
    );
    common::runtime::run(
        Command::new(jdk.join("java"))
            .arg("-cp")
            .arg(&work)
            .arg("Recovery")
            .arg(work.join("data")),
    );
    fs::remove_dir_all(work).unwrap();
}
