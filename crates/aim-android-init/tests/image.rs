//! End-to-end over an image root: property init from `.prop` files and
//! `*_property_contexts`, script discovery and parsing, and a dry-run boot.
//!
//! `tests/golden/image` is a miniature image made of upstream android-16.0.0_r1
//! scripts (system/core rootdir init.rc, init.usb.rc, init.usb.configfs.rc,
//! init.zygote64.rc; logd.rc, servicemanager.rc, surfaceflinger.rc,
//! audioserver.rc), system/sepolicy's property_contexts, and hand-written
//! `.prop` fixtures that exercise init's override rules.
//!
//! `real_image_statistics` parses the extracted Android 16 image when one is
//! present on this machine and prints the statistics.

use std::collections::BTreeMap;
use std::path::PathBuf;

use aim_android_init::engine::{ActionManager, DryRunExecutor, Step};
use aim_android_init::props::load::{
    KernelBootProperties, PropertyInitOptions, create_serialized_property_info, property_init,
    start_property_service,
};
use aim_android_init::props::{PropertyService, SetEffect};
use aim_android_init::rc::{IdResolver, InitScripts, ScriptLoader};
use aim_android_init::{ImageRoot, PropertyLookup, Severity};

fn fixture_image() -> ImageRoot {
    ImageRoot::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/image"))
}

fn boot_properties(image: &ImageRoot) -> PropertyService {
    let mut diagnostics = Vec::new();
    let info = create_serialized_property_info(image, 36, &mut diagnostics).unwrap();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    let mut service = PropertyService::new(info).unwrap();
    let options = PropertyInitOptions {
        kernel: KernelBootProperties {
            cmdline: vec![("androidboot.hardware".to_string(), "ranchu".to_string())],
            ..Default::default()
        },
        vendor_api_level: 36,
        ..Default::default()
    };
    let log = property_init(&mut service, image, &options);
    for line in &log {
        eprintln!("{line}");
    }
    start_property_service(&mut service);
    service
}

#[test]
fn property_init_follows_init_rules() {
    let image = fixture_image();
    let service = boot_properties(&image);
    let get = |name: &str| service.property(name);

    assert_eq!(get("ro.boot.hardware").as_deref(), Some("ranchu"));
    // ExportKernelBootProps sets ro.hardware first; build.prop cannot
    // override a set ro.* property.
    assert_eq!(get("ro.hardware").as_deref(), Some("ranchu"));
    assert_eq!(get("ro.bootmode").as_deref(), Some("unknown"));
    assert_eq!(get("spaced.key").as_deref(), Some("spaced value"));
    assert_eq!(get("ro.imported.a").as_deref(), Some("1"));
    assert_eq!(get("ro.not.imported"), None);
    // Later files override earlier ones in the map, ro.* included.
    assert_eq!(get("ro.override.me").as_deref(), Some("vendor"));
    assert_eq!(get("dalvik.vm.heapsize").as_deref(), Some("512m"));
    // "yes" fails the bool type check and is not loaded.
    assert_eq!(get("ro.debuggable").as_deref(), Some("1"));
    assert_eq!(get("ctl.start"), None);
    assert_eq!(get("ro.long.fixture").map(|v| v.len()), Some(100));
    assert_eq!(get("debug.too.long.for.a.mutable.property"), None);
    // Derived defaults.
    assert_eq!(get("ro.product.brand").as_deref(), Some("Android"));
    assert_eq!(
        get("ro.build.fingerprint").as_deref(),
        Some("Android/sdk_phone64_arm64/emu64a:16/BP2A.250605.031/13580296:userdebug/test-keys")
    );
    assert_eq!(get("ro.product.cpu.abilist").as_deref(), Some("arm64-v8a"));
    assert_eq!(get("ro.product.cpu.abilist32").as_deref(), Some(""));
    assert_eq!(get("ro.vendor.api_level").as_deref(), Some("202504"));
    assert_eq!(get("persist.sys.usb.config").as_deref(), Some("adb"));
    assert_eq!(
        get("ro.boot.hardware.cpu.pagesize").as_deref(),
        Some("16384")
    );
    assert_eq!(get("ro.property_service.version").as_deref(), Some("2"));
}

#[test]
fn setprop_rules_and_serials() {
    let image = fixture_image();
    let mut service = boot_properties(&image);
    let serial_before = service.areas().area_serial();

    let ok = service.init_set("debug.test", "1");
    assert!(ok.is_success());
    assert!(ok.effects.contains(&SetEffect::Changed {
        name: "debug.test".to_string(),
        value: "1".to_string()
    }));
    let first = service.areas().property_serial("debug.test").unwrap();
    assert_eq!(first >> 24, 1);
    assert_eq!(service.areas().area_serial(), serial_before + 1);

    service.init_set("debug.test", "three");
    let second = service.areas().property_serial("debug.test").unwrap();
    assert_eq!(second >> 24, 5);
    // Update sets the dirty bit, then publishes (serial|1)+1: +2 per update,
    // clean.
    assert_eq!(second & 0xffffff, (first & 0xffffff) + 2);
    assert_eq!(second & 1, 0);
    assert_eq!(service.areas().area_serial(), serial_before + 2);

    let read_only = service.init_set("ro.build.id", "other");
    assert_eq!(read_only.reply, Some(0x0B));
    let bad_type = service.init_set("sys.boot_completed", "maybe");
    assert_eq!(bad_type.reply, Some(0x14));
    let bad_name = service.init_set("a..b", "1");
    assert_eq!(bad_name.reply, Some(0x10));
    let control = service.handle_set(
        "ctl.start",
        b"logd",
        "u:r:shell:s0",
        &aim_android_init::props::Ucred {
            pid: 42,
            uid: 2000,
            gid: 2000,
        },
        true,
    );
    assert_eq!(control.reply, None);
    assert!(
        matches!(&control.effects[0], SetEffect::Control(m) if m.action == "start" && m.target == "logd")
    );
}

fn load_scripts(
    image: &ImageRoot,
    properties: &dyn PropertyLookup,
    vendor_api_level: u32,
    vendor_apexes: Option<Vec<String>>,
) -> InitScripts {
    let ids = IdResolver::from_image(image, properties);
    ScriptLoader {
        image,
        properties,
        ids: &ids,
        vendor_api_level,
        vendor_apexes,
        bootstrap_apexes: Vec::new(),
    }
    .load()
}

#[test]
fn upstream_init_rc_parses_and_boots() {
    let image = fixture_image();
    let service = boot_properties(&image);
    let scripts = load_scripts(&image, &service, 36, None);
    let boot = &scripts.boot;
    for diagnostic in &boot.diagnostics {
        eprintln!("{diagnostic}");
    }
    assert_eq!(
        boot.files,
        vec![
            "/system/etc/init/hw/init.rc",
            "/system/etc/init/hw/init.usb.rc",
            "/system/etc/init/hw/init.usb.configfs.rc",
            "/system/etc/init/hw/init.zygote64.rc",
            "/system/etc/init/audioserver.rc",
            "/system/etc/init/logd.rc",
            "/system/etc/init/servicemanager.rc",
            "/system/etc/init/surfaceflinger.rc",
        ]
    );
    assert_eq!(boot.parse_error_count, 0, "{:#?}", boot.diagnostics);
    let zygote = boot.service("zygote").unwrap();
    assert_eq!(zygote.args[0], "/system/bin/app_process64");
    assert!(
        zygote
            .sockets
            .iter()
            .any(|s| s.name == "zygote" && s.perm == 0o660)
    );
    let logd = boot.service("logd").unwrap();
    assert!(logd.sockets.iter().any(|s| s.name == "logdw"));
    let servicemanager = boot.service("servicemanager").unwrap();
    assert!(servicemanager.critical.is_some());
    assert!(servicemanager.onrestart.len() >= 5);

    let mut props: BTreeMap<String, String> = service.areas().foreach().into_iter().collect();
    props.insert("ro.cold_boot_done".to_string(), "true".to_string());
    let mut manager = ActionManager::new(scripts, 36);
    manager.queue_boot(&props);
    let mut executor = DryRunExecutor::default();
    let mut ran = Vec::new();
    for _ in 0..50 {
        let (commands, step) = manager.run_until_blocked(&mut props, &mut executor, 10_000);
        ran.extend(commands);
        match step {
            Step::Idle => break,
            Step::WaitingForProperty { name, value } => {
                // What the daemon (ueventd, persistent props, ...) would do.
                props.insert(name.clone(), value.clone());
                manager.property_changed(&name, &value);
            }
            Step::WaitingForExec => manager.exec_finished(),
            Step::Ran(_) => unreachable!(),
        }
    }
    let mut triggers: Vec<String> = Vec::new();
    for command in &ran {
        let event = command.action.rsplit(" && ").next().unwrap().to_string();
        if !event.contains('=') && !triggers.contains(&event) {
            triggers.push(event);
        }
    }
    let chain: Vec<&str> = [
        "early-init",
        "init",
        "late-init",
        "queue_property_triggers",
        "early-fs",
        "fs",
        "post-fs",
        "late-fs",
        "post-fs-data",
        "load-bpf-programs",
        "bpf-progs-loaded",
        "zygote-start",
        "firmware_mounts_complete",
        "boot",
        "enable_property_trigger",
    ]
    .to_vec();
    let positions: Vec<usize> = chain
        .iter()
        .map(|t| {
            triggers
                .iter()
                .position(|x| x == t)
                .unwrap_or_else(|| panic!("{t} not run: {triggers:?}"))
        })
        .collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{triggers:?}");
    assert!(manager.property_triggers_enabled());
    eprintln!(
        "dry-run boot: {} commands; triggers in order: {triggers:?}",
        ran.len()
    );
}

/// Stand-in for apexd's apex-info-list.xml partitions on an extracted image
/// that has none: an activated `/apex/<name>` whose name prefixes a
/// `/vendor/apex` or `/odm/apex` package file is a vendor APEX.
fn vendor_apexes_from_packages(image: &ImageRoot) -> Vec<String> {
    let mut packages = Vec::new();
    for dir in ["/vendor/apex", "/odm/apex"] {
        if let Ok(files) = image.regular_files(dir) {
            packages.extend(files);
        }
    }
    let mut apexes = image.subdirectories("/apex").unwrap_or_default();
    apexes.retain(|name| {
        packages.iter().any(|package| {
            let file = package.rsplit('/').next().unwrap_or("");
            file.starts_with(&format!("{name}.")) || file.starts_with(&format!("{name}-"))
        })
    });
    apexes.sort();
    apexes
}

/// Parses the real extracted image when available and reports statistics.
#[test]
fn real_image_statistics() {
    let Some(root) = aim_paths::input(aim_paths::original_image(), "image") else {
        return;
    };
    let image = ImageRoot::new(&root);
    let vendor_version = aim_android_init::rc::vendor_android_version(&image).unwrap_or(36);
    // The emulator image's bootconfig carries androidboot.hardware=ranchu.
    let options = PropertyInitOptions {
        kernel: KernelBootProperties {
            bootconfig: vec![("androidboot.hardware".to_string(), "ranchu".to_string())],
            ..Default::default()
        },
        vendor_api_level: vendor_version,
        ..Default::default()
    };
    let mut diagnostics = Vec::new();
    let (properties, summary): (Box<dyn PropertyLookup>, String) =
        match create_serialized_property_info(&image, vendor_version, &mut diagnostics) {
            Ok(info) => {
                let size = info.len();
                let mut service = PropertyService::new(info).unwrap();
                diagnostics.extend(property_init(&mut service, &image, &options));
                start_property_service(&mut service);
                let summary = format!(
                    "property_info {size} bytes, {} contexts, {} properties set",
                    service.areas().contexts().len(),
                    service.areas().foreach().len()
                );
                (Box::new(service), summary)
            }
            Err(error) => {
                let mut map = BTreeMap::new();
                for file in [
                    "/system/build.prop",
                    "/system_ext/etc/build.prop",
                    "/vendor/build.prop",
                    "/product/etc/build.prop",
                ] {
                    if let Ok(bytes) = image.read(file) {
                        for (k, v) in aim_android_init::props::load::read_prop_file(
                            &String::from_utf8_lossy(&bytes),
                        ) {
                            map.insert(k, v);
                        }
                    }
                }
                (Box::new(map), format!("no property contexts ({error})"))
            }
        };
    let vendor_apexes = if image.exists("/apex/apex-info-list.xml") {
        None
    } else {
        Some(vendor_apexes_from_packages(&image))
    };
    let scripts = load_scripts(
        &image,
        properties.as_ref(),
        vendor_version,
        vendor_apexes.clone(),
    );
    let count =
        |d: &[aim_android_init::Diagnostic], s| d.iter().filter(|x| x.severity == s).count();
    eprintln!("image: {}", root.display());
    eprintln!("vendor android version: {vendor_version}; vendor apexes: {vendor_apexes:?}");
    eprintln!(
        "properties: {summary}; property-load diagnostics: {}",
        diagnostics.len()
    );
    for diagnostic in &diagnostics {
        eprintln!("  {diagnostic}");
    }
    let boot_services = scripts.boot.services.len();
    for (name, parsed) in [("boot", &scripts.boot), ("apex", &scripts.apex)] {
        let services = if name == "apex" {
            format!(
                "{} (+{} from APEXes)",
                parsed.services.len(),
                parsed.services.len() - boot_services
            )
        } else {
            parsed.services.len().to_string()
        };
        eprintln!(
            "{name}: files={} services={services} actions={} warnings={} errors={} parse_errors={} verbatim_options={}",
            parsed.files.len(),
            parsed.actions.len(),
            count(&parsed.diagnostics, Severity::Warning),
            count(&parsed.diagnostics, Severity::Error),
            parsed.parse_error_count,
            parsed
                .services
                .iter()
                .map(|s| s.other_options.len())
                .sum::<usize>(),
        );
        for diagnostic in &parsed.diagnostics {
            eprintln!("  {diagnostic}");
        }
    }
    assert_eq!(scripts.boot.parse_error_count, 0);
}
