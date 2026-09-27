//! Dry-run boots: the upstream fixture image always, and the full pinned
//! Android 16 image when it has been extracted on this machine.

mod common;

use std::path::Path;

use darwin_android_init::rc::read_apex_info_list;
use darwin_guest_init::fsops::Effect;
use darwin_guest_init::{Boot, BootOptions, BootReport, RunMode};

const REAL_IMAGE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../_build/android16-image-full"
);

fn launched_pid(report: &BootReport, service: &str) -> Option<u32> {
    report
        .commands
        .iter()
        .flat_map(|c| &c.effects)
        .find_map(|e| match e {
            Effect::Launch { service: s, pid } if s == service => Some(*pid),
            _ => None,
        })
}

fn assert_in_order(report: &BootReport, chain: &[&str]) {
    let positions: Vec<usize> = chain
        .iter()
        .map(|t| {
            report
                .triggers
                .iter()
                .position(|x| x == t)
                .unwrap_or_else(|| panic!("{t} not run: {:?}", report.triggers))
        })
        .collect();
    assert!(
        positions.windows(2).all(|w| w[0] < w[1]),
        "{:?}",
        report.triggers
    );
}

#[test]
fn fixture_image_dry_run_boot() {
    let root = common::temp_dir("boot-fixture");
    let image = common::fixture_image(&root);
    let mut boot =
        Boot::prepare(BootOptions::new(image, root.join("data"), RunMode::DryRun)).unwrap();
    let report = boot.run().clone();
    eprintln!("{}", report.summary());
    assert!(report.fatal.is_none());
    assert_in_order(
        &report,
        &[
            "early-init",
            "init",
            "late-init",
            "fs",
            "post-fs",
            "post-fs-data",
            "boot",
        ],
    );
    for service in ["logd", "servicemanager"] {
        assert!(
            launched_pid(&report, service).is_some(),
            "{service} not launched"
        );
        assert_eq!(
            boot.property(&format!("init.svc.{service}")).as_deref(),
            Some("running")
        );
    }
    let logd = &report
        .launches
        .iter()
        .find(|(pid, _)| Some(*pid) == launched_pid(&report, "logd"))
        .unwrap()
        .1;
    assert!(logd.contains("uid=1036 gid=1036"), "{logd}");
    assert!(logd.contains("--path-map"), "{logd}");
    assert!(logd.contains("ANDROID_SOCKET_logdw=5"), "{logd}");
    assert_eq!(boot.property("ro.cold_boot_done").as_deref(), Some("true"));
    // A dry run only writes the runtime layout, never the image.
    assert!(!Path::new(&root.join("image/dev/socket")).exists());
    common::make_writable(&root);
    let _ = std::fs::remove_dir_all(&root);
}

/// The full boot of the pinned image as a dry run. Prints the statistics.
#[test]
fn real_image_dry_run_boot() {
    if !Path::new(REAL_IMAGE)
        .join("system/etc/init/hw/init.rc")
        .exists()
    {
        eprintln!("{REAL_IMAGE} not extracted on this machine; skipping");
        return;
    }
    let root = common::temp_dir("boot-real");
    let mut boot = Boot::prepare(BootOptions::new(
        REAL_IMAGE.into(),
        root.join("data"),
        RunMode::DryRun,
    ))
    .unwrap();
    let report = boot.run().clone();
    eprintln!("{}", report.summary());
    for (reason, count) in report.noop_reasons() {
        eprintln!("  no-op x{count}: {reason}");
    }
    assert!(report.fatal.is_none(), "{:?}", report.fatal);
    assert!(report.commands.len() > 1000, "{}", report.commands.len());
    assert_in_order(
        &report,
        &[
            "early-init",
            "init",
            "late-init",
            "queue_property_triggers",
            "early-fs",
            "fs",
            "post-fs",
            "late-fs",
            "post-fs-data",
            "zygote-start",
            "early-boot",
            "boot",
            "enable_property_trigger",
        ],
    );
    let launched = report.services_launched();
    for service in [
        "logd",
        "servicemanager",
        "hwservicemanager",
        "surfaceflinger",
        "zygote",
    ] {
        assert!(
            launched.contains(service),
            "{service} not launched: {launched:?}"
        );
    }
    // Roles: no ueventd or apexd process.
    assert!(!launched.contains("ueventd"));
    assert!(!launched.contains("apexd"));
    assert_eq!(boot.property("apexd.status").as_deref(), Some("ready"));
    assert_eq!(
        boot.property("ro.crypto.state").as_deref(),
        Some("unencrypted")
    );
    assert_eq!(
        boot.property("ro.persistent_properties.ready").as_deref(),
        Some("true")
    );
    // Every executed filesystem command left a description.
    let counts = report.effect_counts();
    assert!(
        counts["recorded"] > 100 && counts["applied"] > 100,
        "{counts:?}"
    );
    // The APEX list covers the flattened tree with partitions.
    let xml = std::fs::read_to_string(boot.layout.apex_info_list()).unwrap();
    let apexes = read_apex_info_list(&xml);
    let art = apexes
        .iter()
        .find(|a| a.module_name == "com.android.art")
        .unwrap();
    assert_eq!(art.partition, "SYSTEM");
    assert!(
        apexes
            .iter()
            .any(|a| a.module_name == "com.android.hardware.power" && a.partition == "VENDOR")
    );
    common::make_writable(&root);
    let _ = std::fs::remove_dir_all(&root);
}
