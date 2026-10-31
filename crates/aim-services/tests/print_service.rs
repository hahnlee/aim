//! Verify the pinned original's print publication, policy and spooler route.
//! DEX checks complement the JVM lifecycle test; they do not execute a print job.
use aim_android_image::{
    dex::{Dex, instruction_units, units},
    system_server::dex_files,
};
use std::fs;

fn calls(files: &[&[u8]], class: &str, method: &str) -> Vec<(String, String, String)> {
    let bytes = files
        .iter()
        .find(|bytes| Dex::parse(bytes).unwrap().class(class).is_some())
        .unwrap_or_else(|| panic!("missing original {class}"));
    let dex = Dex::parse(bytes).unwrap();
    let methods = dex
        .methods_named(dex.class(class).unwrap(), method)
        .unwrap();
    assert_eq!(methods.len(), 1, "{class}->{method}");
    let insns = units(bytes, &methods[0]).unwrap();
    let mut result = Vec::new();
    let mut at = 0;
    while at < insns.len() {
        if matches!(insns[at] & 0xff, 0x6e..=0x72 | 0x74..=0x78) {
            result.push(dex.method(u32::from(insns[at + 1])).unwrap());
        }
        at += instruction_units(&insns, at).unwrap();
    }
    result
}

fn invokes(calls: &[(String, String, String)], class: &str, method: &str) {
    assert!(
        calls.iter().any(|(c, m, _)| c == class && m == method),
        "missing {class}->{method} in {calls:?}"
    );
}

#[test]
#[ignore = "requires the pinned original image and device-services build; no guest boot"]
fn original_print_service_keeps_settings_policy_and_real_spooler_binding() {
    const SERVICE: &str = "Lcom/android/server/print/PrintManagerService;";
    const IMPL: &str = "Lcom/android/server/print/PrintManagerService$PrintManagerImpl;";
    const USER: &str = "Lcom/android/server/print/UserState;";
    const SPOOLER: &str = "Lcom/android/server/print/RemotePrintSpooler;";
    let jar = fs::read(aim_paths::original_image().join("system/framework/services.jar"))
        .expect("NOT RUN: original services.jar missing");
    let original = dex_files(&jar).unwrap();
    invokes(&calls(&original, SERVICE, "<init>"), IMPL, "<init>");
    invokes(
        &calls(&original, SERVICE, "onStart"),
        "Lcom/android/server/SystemService;",
        "publishBinderService",
    );
    let init = calls(&original, IMPL, "<init>");
    for method in ["registerContentObservers", "registerBroadcastReceivers"] {
        invokes(&init, IMPL, method);
    }
    // All Settings calls retain the original user/profile/unlock checks and
    // dispatch into UserState, rather than answering at an added native endpoint.
    for method in [
        "addPrintJobStateChangeListener",
        "removePrintJobStateChangeListener",
        "getPrintJobInfos",
        "getPrintServices",
    ] {
        let path = calls(&original, IMPL, method);
        for check in [
            "resolveCallingUserEnforcingPermissions",
            "resolveCallingProfileParentLocked",
            "getOrCreateUserStateLocked",
        ] {
            invokes(&path, IMPL, check);
        }
        invokes(&path, USER, method);
    }
    invokes(
        &calls(&original, IMPL, "getPrintServices"),
        "Landroid/content/Context;",
        "enforceCallingOrSelfPermission",
    );
    invokes(
        &calls(&original, IMPL, "resolveCallingUserEnforcingPermissions"),
        "Landroid/app/IActivityManager;",
        "handleIncomingUser",
    );
    invokes(&calls(&original, USER, "<init>"), SPOOLER, "<init>");
    invokes(
        &calls(&original, USER, "getPrintJobInfos"),
        SPOOLER,
        "getPrintJobInfos",
    );
    invokes(
        &calls(&original, USER, "prunePrintServices"),
        SPOOLER,
        "pruneApprovedPrintServices",
    );
    invokes(
        &calls(&original, USER, "removeObsoletePrintJobs"),
        SPOOLER,
        "removeObsoletePrintJobs",
    );
    invokes(
        &calls(&original, SPOOLER, "bindLocked"),
        "Landroid/content/Context;",
        "bindServiceAsUser",
    );
    invokes(
        &calls(&original, SPOOLER, "setClientLocked"),
        "Landroid/print/IPrintSpooler;",
        "setClient",
    );
    // The vendor jar references the original and must never ship a print class
    // or a runtime copy of its compile-only ABI stub.
    let vendor = fs::read(aim_paths::out().join("device-services/aim-services.jar"))
        .expect("NOT RUN: device-services build missing");
    let vendor = dex_files(&vendor).unwrap();
    for bytes in &vendor {
        assert!(Dex::parse(bytes).unwrap().class(SERVICE).is_none());
    }
    invokes(
        &calls(&vendor, "Ldev/aim/server/SystemProcessHost$Server;", "main"),
        SERVICE,
        "<init>",
    );
}
