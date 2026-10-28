//! Inspect the pinned originals' registration route without a guest boot.
//! This is a DEX contract check, not execution of the original service or its JNI.
use aim_android_image::{
    dex::{Dex, instruction_units, units},
    system_server::dex_files,
};
use std::fs;

fn calls(dex: &Dex<'_>, bytes: &[u8], class: &str, method: &str) -> Vec<(String, String, String)> {
    let class = dex.class(class).unwrap();
    let code = dex.methods_named(class, method).unwrap();
    assert_eq!(code.len(), 1, "{method}");
    let insns = units(bytes, &code[0]).unwrap();
    let mut out = Vec::new();
    let mut at = 0;
    while at < insns.len() {
        if matches!(insns[at] & 0xff, 0x6e..=0x72 | 0x74..=0x78) {
            out.push(dex.method(u32::from(insns[at + 1])).unwrap());
        }
        at += instruction_units(&insns, at).unwrap();
    }
    out
}

#[test]
#[ignore = "requires the pinned original image; no guest boot"]
fn original_pullers_use_the_framework_statsmanager_registration_contract() {
    const SERVICE: &str = "Lcom/android/server/stats/pull/StatsPullAtomService;";
    const MANAGER: &str = "Landroid/app/StatsManager;";
    let image = aim_paths::original_image();
    let jar = fs::read(image.join("system/framework/services.jar"))
        .expect("NOT RUN: original services.jar missing");
    let files = dex_files(&jar).unwrap();
    let bytes = files
        .iter()
        .find(|bytes| Dex::parse(bytes).unwrap().class(SERVICE).is_some())
        .unwrap();
    let dex = Dex::parse(bytes).unwrap();
    let class = dex.class(SERVICE).unwrap();
    assert_eq!(
        class.superclass.as_deref(),
        Some("Lcom/android/server/SystemService;")
    );
    assert!(dex.members(class).unwrap().1.into_iter().any(|id| {
        let (_, name, signature) = dex.method(id).unwrap();
        name == "<init>" && signature == "(Landroid/content/Context;)V"
    }));
    let initialize = calls(&dex, bytes, SERVICE, "initializePullersState");
    assert!(initialize.iter().any(|(class, name, _)| {
        class == "Landroid/content/Context;" && name == "getSystemService"
    }));
    let registrations = calls(&dex, bytes, SERVICE, "registerPullers");
    let registration = (
        MANAGER.to_owned(),
        "setPullAtomCallback".to_owned(),
        "(ILandroid/app/StatsManager$PullAtomMetadata;Ljava/util/concurrent/Executor;Landroid/app/StatsManager$StatsPullAtomCallback;)V".to_owned(),
    );
    for name in [
        "registerSystemElapsedRealtime",
        "registerSystemUptime",
        "registerBuildInformation",
        "registerTemperature",
        "registerDirectoryUsage",
        "registerPendingIntentsPerPackagePuller",
    ] {
        assert!(
            registrations
                .iter()
                .any(|(class, method, _)| class == SERVICE && method == name)
        );
        assert!(
            calls(&dex, bytes, SERVICE, name).contains(&registration),
            "{name}"
        );
    }
    let clock = calls(&dex, bytes, SERVICE, "registerSystemElapsedRealtime");
    for name in ["setCoolDownMillis", "setTimeoutMillis"] {
        assert!(clock.iter().any(|(_, method, _)| method == name));
    }
    // The framework manager forwards the callback through the same AIDL the
    // native statsmanager implements; its forwarding is exercised in the Rust unit test.
    let jar = fs::read(image.join("apex/com.android.os.statsd/javalib/framework-statsd.jar"))
        .expect("NOT RUN: original framework-statsd.jar missing");
    let files = dex_files(&jar).unwrap();
    let bytes = files
        .iter()
        .find(|bytes| Dex::parse(bytes).unwrap().class(MANAGER).is_some())
        .unwrap();
    let dex = Dex::parse(bytes).unwrap();
    let calls = calls(&dex, bytes, MANAGER, "setPullAtomCallback");
    assert!(calls.iter().any(|(class, method, signature)| {
        class == "Landroid/os/IStatsManagerService;"
            && method == "registerPullAtomCallback"
            && signature == "(IJJ[ILandroid/os/IPullAtomCallback;)V"
    }));
}
