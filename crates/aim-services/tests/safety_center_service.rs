//! Pinned original routes and vendor ownership, without booting Android.
//! DEX inspection and JVM lifecycle doubles do not execute aggregate/API semantics.
use aim_android_image::{
    dex::{Dex, instruction_units, units},
    system_server::dex_files,
};
use std::fs;

const SERVICE: &str = "Lcom/android/safetycenter/SafetyCenterService;";
const STUB: &str = "Lcom/android/safetycenter/SafetyCenterService$Stub;";

// Preserve instruction order, including guards/returns and strings, so startup
// checks distinguish disabled exits from unconditional initialization.
fn route(files: &[&[u8]], class: &str, method: &str) -> Vec<String> {
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
    let mut out = Vec::new();
    let mut at = 0;
    while at < insns.len() {
        let op = insns[at] & 0xff;
        match op {
            0x6e..=0x72 | 0x74..=0x78 => {
                let (owner, name, _) = dex.method(u32::from(insns[at + 1])).unwrap();
                out.push(format!("{owner}->{name}"));
            }
            0x13 => out.push(format!("int={}", insns[at + 1] as i16)),
            0x1a | 0x1b => {
                let mut id = u32::from(insns[at + 1]);
                if op == 0x1b {
                    id |= u32::from(insns[at + 2]) << 16;
                }
                out.push(dex.string(id).unwrap());
            }
            0x52..=0x58 => {
                let (_, name, _) = dex.field(u32::from(insns[at + 1])).unwrap();
                out.push(name);
            }
            0x32..=0x3d => out.push("branch".into()),
            0x0e..=0x11 => out.push("return".into()),
            _ => {}
        }
        at += instruction_units(&insns, at).unwrap();
    }
    out
}

fn ordered(actual: &[String], expected: &[&str]) {
    let mut rest = actual;
    for item in expected {
        let at = rest
            .iter()
            .position(|entry| entry == item)
            .unwrap_or_else(|| panic!("missing ordered {item} in {actual:?}"));
        rest = &rest[at + 1..];
    }
}

#[test]
#[ignore = "requires pinned image and device-services build; no guest boot"]
fn original_safety_center_owns_publication_state_aggregate_and_listeners() {
    let image = aim_paths::original_image();
    let jar = fs::read(image.join("apex/com.android.permission/javalib/service-permission.jar"))
        .expect("NOT RUN: original Permission jar missing");
    let files = dex_files(&jar).unwrap();
    let start = route(&files, SERVICE, "onStart");
    ordered(
        &start,
        &[
            "safety_center",
            "Lcom/android/server/SystemService;->publishBinderService",
            "mDeviceSupportsSafetyCenter",
            "branch",
            "return",
            "Lcom/android/permission/jarjar/com/android/safetycenter/resources/SafetyCenterResourcesApk;->init",
            "branch",
            "return",
            "Lcom/android/safetycenter/SafetyCenterConfigReader;->loadConfig",
            "branch",
            "return",
            "Lcom/android/safetycenter/data/SafetyCenterDataManager;->loadPersistableDataStateFromFile",
            "Lcom/android/safetycenter/SafetyCenterService$UserBroadcastReceiver;->register",
            "Lcom/android/safetycenter/notifications/SafetyCenterNotificationReceiver;->register",
            "Lcom/android/safetycenter/SafetyCenterService$LocaleBroadcastReceiver;->register",
        ],
    );
    ordered(
        &route(&files, SERVICE, "canUseSafetyCenter"),
        &[
            "mDeviceSupportsSafetyCenter",
            "branch",
            "mConfigAvailable",
            "branch",
        ],
    );
    ordered(
        &route(&files, SERVICE, "onBootPhase"),
        &[
            "int=1000",
            "branch",
            "Lcom/android/safetycenter/SafetyCenterService;->canUseSafetyCenter",
            "branch",
            "Lcom/android/safetycenter/SafetyCenterService;->registerSafetyCenterEnabledListenerLocked",
            "Lcom/android/safetycenter/SafetyCenterService;->newSafetyCenterPullAtomCallbackLocked",
            "Lcom/android/safetycenter/SafetyCenterService;->registerSafetyCenterPullAtomCallback",
        ],
    );
    ordered(
        &route(&files, SERVICE, "registerSafetyCenterPullAtomCallback"),
        &["Landroid/app/StatsManager;->setPullAtomCallback"],
    );
    ordered(
        &route(&files, SERVICE, "registerSafetyCenterEnabledListenerLocked"),
        &[
            // The read-only aconfig flag is folded true in this image.
            "Lcom/android/permission/jarjar/com/android/modules/utils/build/SdkLevel;->isAtLeastU",
            "branch",
            "return",
            "Landroid/provider/DeviceConfig;->addOnPropertiesChangedListener",
        ],
    );
    for method in [
        "getSafetyCenterData",
        "addOnSafetyCenterDataChangedListener",
    ] {
        let calls = route(&files, STUB, method);
        ordered(
            &calls,
            &[
                // R8 lowers requireNonNull to a throwing getClass call.
                "Ljava/lang/Object;->getClass",
                "android.permission.MANAGE_SAFETY_CENTER",
                "Landroid/content/Context;->enforceCallingOrSelfPermission",
                "Lcom/android/safetycenter/SafetyCenterService$Stub;->enforceCrossUserPermission",
                "Lcom/android/safetycenter/SafetyCenterService$Stub;->enforcePackage",
                "Lcom/android/safetycenter/SafetyCenterService$Stub;->checkApiEnabled",
                "Lcom/android/safetycenter/UserProfileGroup;->fromUser",
                "Lcom/android/safetycenter/SafetyCenterDataFactory;->assembleSafetyCenterData",
            ],
        );
        if method == "getSafetyCenterData" {
            ordered(
                &calls,
                &[
                    "Lcom/android/safetycenter/SafetyCenterService$Stub;->checkApiEnabled",
                    "branch",
                    "Lcom/android/safetycenter/SafetyCenterDataFactory;->assembleSafetyCenterData",
                    "return",
                    "Lcom/android/safetycenter/SafetyCenterDataFactory;->getDefaultSafetyCenterData",
                    "return",
                ],
            );
        } else {
            ordered(
                &calls,
                &[
                    "Lcom/android/safetycenter/SafetyCenterListeners;->addListener",
                    "Lcom/android/safetycenter/SafetyCenterListeners;->deliverDataForListener",
                ],
            );
        }
    }
    ordered(
        &route(&files, STUB, "removeOnSafetyCenterDataChangedListener"),
        &[
            "Lcom/android/safetycenter/SafetyCenterService$Stub;->enforceCrossUserPermission",
            "Lcom/android/safetycenter/SafetyCenterListeners;->removeListener",
        ],
    );
    for receiver in ["UserBroadcastReceiver", "LocaleBroadcastReceiver"] {
        let class = format!("Lcom/android/safetycenter/SafetyCenterService${receiver};");
        ordered(
            &route(&files, &class, "register"),
            &["Landroid/content/Context;->registerReceiverForAllUsers"],
        );
        ordered(
            &route(&files, &class, "onReceive"),
            &[
                "Lcom/android/safetycenter/SafetyCenterFlags;->getSafetyCenterEnabled",
                "Lcom/android/safetycenter/SafetyCenterService;->-$$Nest$mstartRefreshingSafetySourcesLocked",
                "Lcom/android/safetycenter/notifications/SafetyCenterNotificationChannels;->createAllChannelsForUser",
            ],
        );
    }
    for method in ["setSafetySourceData", "reportSafetySourceError"] {
        ordered(
            &route(&files, STUB, method),
            &[
                "android.permission.SEND_SAFETY_CENTER_UPDATE",
                "Landroid/content/Context;->enforceCallingOrSelfPermission",
                "Lcom/android/safetycenter/SafetyCenterService$Stub;->enforceCrossUserPermission",
                "Lcom/android/safetycenter/SafetyCenterService$Stub;->enforcePackage",
                "Lcom/android/safetycenter/SafetyCenterService$Stub;->checkApiEnabled",
                &format!("Lcom/android/safetycenter/data/SafetyCenterDataManager;->{method}"),
                "Lcom/android/safetycenter/SafetyCenterDataChangeNotifier;->updateDataConsumers",
            ],
        );
    }
    for (method, callback) in [
        ("addListener", "register"),
        ("removeListener", "unregister"),
    ] {
        ordered(
            &route(
                &files,
                "Lcom/android/safetycenter/SafetyCenterListeners;",
                method,
            ),
            &[&format!("Landroid/os/RemoteCallbackList;->{callback}")],
        );
    }
    let history = "Lcom/android/safetycenter/data/SafetyCenterIssueDismissalRepository;";
    ordered(
        &route(&files, history, "loadStateFromFile"),
        &[
            "Lcom/android/safetycenter/persistence/SafetyCenterIssuesPersistence;->read",
            "Lcom/android/safetycenter/data/SafetyCenterIssueDismissalRepository;->scheduleWriteStateToFile",
        ],
    );
    ordered(
        &route(&files, history, "writeStateToFile"),
        &["Lcom/android/safetycenter/persistence/SafetyCenterIssuesPersistence;->write"],
    );
    let services = fs::read(image.join("system/framework/services.jar")).unwrap();
    let services = dex_files(&services).unwrap();
    ordered(
        &route(
            &services,
            "Lcom/android/server/SystemServer;",
            "startOtherServices",
        ),
        &[
            "int=520",
            "Lcom/android/server/SystemServiceManager;->startBootPhase",
            "com.android.safetycenter.SafetyCenterService",
            "Lcom/android/server/SystemServiceManager;->startService",
            "com.android.server.appsearch.AppSearchModule$Lifecycle",
        ],
    );
    let vendor = fs::read(aim_paths::out().join("device-services/aim-services.jar"))
        .expect("NOT RUN: build device-services first");
    let vendor = dex_files(&vendor).unwrap();
    for bytes in &vendor {
        assert!(
            !Dex::parse(bytes)
                .unwrap()
                .classes
                .iter()
                .any(|c| c.descriptor.starts_with("Lcom/android/safetycenter/"))
        );
    }
    ordered(
        &route(&vendor, "Ldev/aim/server/SystemProcessHost$Server;", "main"),
        &[
            "Lcom/android/safetycenter/SafetyCenterService;-><init>",
            "Lcom/android/server/SystemService;->onStart",
            "Lcom/android/server/appsearch/AppSearchModule$Lifecycle;-><init>",
        ],
    );
}
