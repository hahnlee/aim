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
    // Read-only power-attribution flag is folded into the pinned DEX:
    // no registration, and the callback is const/4 v0, #1 (PULL_SKIP); return v0.
    let cpu = calls(
        &dex,
        bytes,
        SERVICE,
        "registerCpuCyclesPerThreadGroupCluster",
    );
    assert!(!cpu.iter().any(|(_, name, _)| name == "setPullAtomCallback"));
    let code = dex
        .methods_named(class, "pullCpuCyclesPerThreadGroupCluster")
        .unwrap();
    assert_eq!(units(bytes, &code[0]).unwrap(), [0x1012, 0x000f]);
    assert!(
        calls(&dex, bytes, SERVICE, "registerPinnerServiceStats")
            .iter()
            .any(|(_, name, _)| name == "setPullAtomCallback")
    );
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

#[test]
#[ignore = "requires pinned original image; static call-site audit, no guest boot"]
fn activity_internal_calls_in_system_process_modules() {
    let image = aim_paths::original_image();
    let mut jars = vec![
        (
            "system/framework/services.jar",
            Some("Lcom/android/server/stats/pull/"),
        ),
        (
            "apex/com.android.appsearch/javalib/service-appsearch.jar",
            None,
        ),
        (
            "apex/com.android.adservices/javalib/service-adservices.jar",
            None,
        ),
        (
            "apex/com.android.tethering/javalib/service-connectivity.jar",
            None,
        ),
        (
            "apex/com.android.devicelock/javalib/service-devicelock.jar",
            None,
        ),
    ];
    jars.sort();
    for (jar, prefix) in jars {
        let bytes = fs::read(image.join(jar)).expect("NOT RUN: required original module missing");
        for bytes in dex_files(&bytes).unwrap() {
            let dex = Dex::parse(&bytes).unwrap();
            for class in &dex.classes {
                if prefix.is_some_and(|p| !class.descriptor.starts_with(p)) {
                    continue;
                }
                let mut methods = std::collections::BTreeSet::new();
                for id in dex.members(class).unwrap().1 {
                    methods.insert(dex.method(id).unwrap().1);
                }
                for method in methods {
                    for code in dex.methods_named(class, &method).unwrap() {
                        let insns = units(&bytes, &code).unwrap();
                        let mut at = 0;
                        while at < insns.len() {
                            if matches!(insns[at] & 0xff, 0x6e..=0x72 | 0x74..=0x78) {
                                let (owner, name, _) =
                                    dex.method(u32::from(insns[at + 1])).unwrap();
                                if owner == "Landroid/app/ActivityManagerInternal;" {
                                    assert!(
                                        matches!(
                                            name.as_str(),
                                            "getMemoryStateForProcesses"
                                                | "getPendingIntentStats"
                                                | "getCachedAppsHighWatermarkStats"
                                        ),
                                        "unserved ActivityManagerInternal: {}->{method}: {name}",
                                        class.descriptor
                                    );
                                    println!("{jar}: {}->{method}: {name}", class.descriptor);
                                }
                            }
                            at += instruction_units(&insns, at).unwrap();
                        }
                    }
                }
            }
        }
    }
}
