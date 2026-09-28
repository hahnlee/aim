//! Dry-run boots: the upstream fixture image always, and the derived image
//! of the full pinned Android 16 image when that has been extracted and the
//! overlay's sources are built (tools/build-vendor-hals.sh) on this machine.

mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use aim_android_init::rc::read_apex_info_list;
use aim_guest_init::fsops::Effect;
use aim_guest_init::{Boot, BootOptions, BootReport, RunMode};

const REAL_IMAGE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../_build/android16-image-full"
);

/// The derived image of `image/overlay.toml`, assembled (APFS clones) once
/// per overlay identity, or why it cannot be.
fn derived_image() -> Result<PathBuf, String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let original = Path::new(REAL_IMAGE)
        .canonicalize()
        .map_err(|e| format!("{REAL_IMAGE}: {e}"))?;
    let (_, plan) = aim_android_image::load(&root.join("image/overlay.toml"), &original, &root)
        .map_err(|problems| format!("{problems:?}"))?;
    let identity = aim_android_image::identity::compute("boot-test-original", &plan);
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("boot-derived-image");
    aim_android_image::assemble(&plan, &original, &identity, &out)?;
    Ok(out)
}

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

#[test]
fn excluded_services_are_not_started() {
    let root = common::temp_dir("boot-exclude");
    let image = common::fixture_image(&root);
    let mut options = BootOptions::new(image, root.join("data"), RunMode::DryRun);
    options.exclude = BTreeSet::from(["logd".to_string()]);
    let mut boot = Boot::prepare(options).unwrap();
    let report = boot.run().clone();
    assert!(launched_pid(&report, "logd").is_none());
    assert!(launched_pid(&report, "servicemanager").is_some());
    assert!(
        report
            .commands
            .iter()
            .flat_map(|c| &c.effects)
            .any(|e| matches!(e, Effect::Skipped(reason) if reason == "'logd' excluded"))
    );
    assert_eq!(boot.property("init.svc.logd"), None);
    common::make_writable(&root);
    let _ = std::fs::remove_dir_all(&root);
}

/// The full boot of the derived image as a dry run. Prints the statistics.
#[test]
fn real_image_dry_run_boot() {
    if !Path::new(REAL_IMAGE)
        .join("system/etc/init/hw/init.rc")
        .exists()
    {
        eprintln!("{REAL_IMAGE} not extracted on this machine; skipping");
        return;
    }
    let image = match derived_image() {
        Ok(image) => image,
        Err(error) => {
            eprintln!("derived image unavailable ({error}); skipping");
            return;
        }
    };
    let root = common::temp_dir("boot-real");
    let mut boot =
        Boot::prepare(BootOptions::new(image, root.join("data"), RunMode::DryRun)).unwrap();
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
        "apexd",
        "logd",
        "servicemanager",
        "hwservicemanager",
        "surfaceflinger",
        "zygote",
        // class early_hal from a vendorBootstrap APEX: parsed at
        // `perform_apex_config --bootstrap`, before `class_start early_hal`.
        "vendor.gatekeeper_nonsecure",
    ] {
        assert!(
            launched.contains(service),
            "{service} not launched: {launched:?}"
        );
    }
    // Roles: no ueventd process; apexd only serves apexservice.
    assert!(!launched.contains("ueventd"));
    assert!(!launched.contains("apexd-bootstrap"));
    assert_eq!(boot.property("apexd.status").as_deref(), Some("ready"));
    // apex.all.ready is the replaced apexd's to set once it serves; a dry
    // run starts no process.
    // A lazy `aidl/apexservice` start does not take the status back.
    boot.executor.start_service("apexd").unwrap();
    assert_eq!(boot.property("apexd.status").as_deref(), Some("ready"));
    // The device: init.aim.rc, not the emulator's init.ranchu.rc.
    assert_eq!(boot.property("ro.hardware").as_deref(), Some("aim"));
    assert_eq!(boot.property("ro.hardware.egl").as_deref(), Some("aim"));
    assert_eq!(boot.property("ro.sf.lcd_density").as_deref(), Some("320"));
    assert!(!launched.contains("qemu-props"));
    assert!(!launched.contains("goldfish-logcat"));
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
