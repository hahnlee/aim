//! Execute the production local service against typed generated AIDL on a host JVM.
use std::{fs, process::Command};
fn sources(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    fs::read_dir(dir)
        .unwrap()
        .flat_map(|entry| {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path)
            } else if path.extension().is_some_and(|e| e == "java") {
                vec![path]
            } else {
                vec![]
            }
        })
        .collect()
}
fn run(command: &mut Command) {
    let output = aim_storage::inherited::stdio_only(command)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{:?}: {}",
        command.get_program(),
        String::from_utf8_lossy(&output.stderr)
    );
}
struct Data(std::path::PathBuf);
impl Drop for Data {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn activity_stats_publishes_original_local_shape_and_formats_owner_snapshots() {
    let data =
        Data(std::env::temp_dir().join(format!("aim-activity-stats-{}", std::process::id())));
    fs::create_dir_all(&data.0).unwrap();
    let jdk = aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let production = aim_paths::root().join("java/device-services");
    let stubs = data.0.join("stubs");
    let classes = data.0.join("classes");
    fs::create_dir(&stubs).unwrap();
    fs::create_dir(&classes).unwrap();
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(sources(&production.join("stubs"))));
    let generated = data.0.join("aidl");
    fs::create_dir(&generated).unwrap();
    let input = production.join("aidl");
    run(
        Command::new(aim_paths::fetched().join("java/build-tools-36.0.0/android-16/aidl"))
            .args(["--lang=java", "--min_sdk_version", "36"])
            .arg(format!("-I{}", input.display()))
            .arg(format!("-o{}", generated.display()))
            .args(
                [
                    "IActivityStatsHost",
                    "IActivityStatsSnapshot",
                    "ActivityStatsProcess",
                    "ActivityStatsPendingIntent",
                    "CachedAppsWatermark",
                ]
                .map(|name| input.join(format!("dev/aim/server/{name}.aidl"))),
            ),
    );
    let mut java_sources = sources(&generated);
    let mut shadow = |name: &str, body: String| {
        let path = data.0.join(name);
        fs::write(&path, body).unwrap();
        java_sources.push(path);
    };
    let binder = fs::read_to_string(production.join("stubs/android/os/Binder.java")).unwrap()
        .replace("public Binder() { throw new RuntimeException(\"stub\"); }", "public Binder() {}")
        .replace("public static final int getCallingUid() { throw new RuntimeException(\"stub\"); }",
                 "public static int uid = 1000; public static final int getCallingUid() { return uid; }")
        .replace("public void attachInterface(IInterface owner, String descriptor) { throw new RuntimeException(\"stub\"); }",
                 "public void attachInterface(IInterface owner, String descriptor) {}");
    shadow("Binder.java", binder);
    shadow("RemoteException.java", "package android.os; public class RemoteException extends Exception { public RuntimeException rethrowFromSystemServer(){return new IllegalStateException(this);} }".into());
    shadow("ActivityManager.java", "package android.app; public final class ActivityManager { public static IActivityManager service; public static IActivityManager getService(){return service;} }".into());
    shadow(
        "ActivityTaskManagerInternal.java",
        fs::read_to_string(
            production.join("stubs/com/android/server/wm/ActivityTaskManagerInternal.java"),
        )
        .unwrap()
        .replace("throw new RuntimeException(\"stub\");", ""),
    );
    shadow("Intent.java", "package android.content; public final class Intent { public static final String ACTION_MAIN=\"android.intent.action.MAIN\", CATEGORY_HOME=\"android.intent.category.HOME\"; public static final int FLAG_DEBUG_TRIAGED_MISSING=256; public final String action; public String category; public int flags; public Intent(String action){this.action=action;} public Intent addCategory(String category){this.category=category;return this;} public Intent addFlags(int flags){this.flags|=flags;return this;} }".into());
    shadow(
        "ActivityManagerInternal.java",
        fs::read_to_string(production.join("stubs/android/app/ActivityManagerInternal.java"))
            .unwrap()
            .replace(
                "public ActivityManagerInternal() { throw new RuntimeException(\"stub\"); }",
                "public ActivityManagerInternal() {}",
            ),
    );
    shadow("ProcessMemoryState.java", fs::read_to_string(production.join("stubs/android/app/ProcessMemoryState.java"))
        .unwrap().replace("throw new RuntimeException(\"stub\");", "this.uid=uid;this.pid=pid;processName=name;oomScore=oom;hasForegroundServices=foreground;mHostingComponentTypes=hosting;mHistoricalHostingComponentTypes=historical;"));
    shadow(
        "PendingIntentStats.java",
        fs::read_to_string(production.join("stubs/android/app/PendingIntentStats.java"))
            .unwrap()
            .replace(
                "throw new RuntimeException(\"stub\");",
                "this.uid=uid;this.count=count;this.sizeKb=sizeKb;",
            ),
    );
    shadow("LocalServices.java", "package com.android.server; public final class LocalServices { private static final java.util.Map<Class<?>,Object> services=new java.util.HashMap<>(); public static <T> void addService(Class<T> type,T value){services.put(type,value);} public static <T> T getService(Class<T> type){return type.cast(services.get(type));} }".into());
    shadow("StatsEvent.java", "package android.util; public final class StatsEvent { public int atom; public java.util.List<Integer> values=new java.util.ArrayList<>(); public static Builder newBuilder(){return new Builder();} public static class Builder { private final StatsEvent event=new StatsEvent(); public Builder setAtomId(int atom){event.atom=atom;return this;} public Builder writeInt(int value){event.values.add(value);return this;} public StatsEvent build(){return event;} } }".into());
    shadow("MemInfoReader.java", "package com.android.internal.util; public final class MemInfoReader { public void readMemInfo(){} public long getFreeSizeKb(){return 21;} public long getCachedSizeKb(){return 22;} public long getZramTotalSizeKb(){return 23;} public long getKernelUsedSizeKb(){return 24;} }".into());
    shadow("SparseIntArray.java", "package android.util; public class SparseIntArray { public int size(){return 2;} public int keyAt(int i){return i==0?10123:10124;} public int valueAt(int i){return i==0?5:11;} }".into());
    shadow("BinderInternal.java", "package com.android.internal.os; public class BinderInternal { public static android.util.SparseIntArray nGetBinderProxyPerUidCounts(){return new android.util.SparseIntArray();} }".into());
    java_sources.extend(sources(
        &aim_paths::root().join("crates/aim-services/tests/fixtures/activity_stats"),
    ));
    java_sources.push(production.join("src/dev/aim/server/ActivityStats.java"));
    java_sources.push(production.join("src/dev/aim/server/TaskManagerLocal.java"));
    java_sources.push(production.join("src/dev/aim/server/Bridge.java"));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(&stubs)
        .args(java_sources));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(format!("{}:{}", classes.display(), stubs.display()))
        .arg("dev.aim.server.ActivityStatsTest"));
}
