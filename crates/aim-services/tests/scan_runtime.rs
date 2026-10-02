//! Reconcile the saved image package set against the running original PMS.
//! This diagnostic uses persisted order, not the complete boot scan selector.
use aim_services::package::{
    State,
    libraries::TYPE_STATIC,
    parse::Platform,
    scan::{Apex, Inputs, Kind, Location, Partition, ScanPolicy, SigningScan, application_flags},
    system_config::SystemConfig,
    write::Apks,
};
use std::collections::BTreeMap;
use std::fs;
use std::time::{Duration, Instant};

mod common {
    pub mod runtime;
}
use common::runtime::{Boot, Data, run};

#[test]
#[ignore = "requires aimctl and the pinned derived image; run explicitly"]
fn saved_scan_libraries_match_original_pms() {
    let dir = std::env::temp_dir().join(format!("aim-scan-runtime-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let data = Data(dir);
    let boot = Boot {
        ctl: aim_paths::root().join("target/release/aimctl"),
        data: data.0.join("guest"),
    };
    run(boot.command().args(["start", "--windows"]));
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let result = boot
            .command()
            .args(["shell", "getprop", "sys.boot_completed"])
            .output()
            .unwrap();
        if result.status.success() && String::from_utf8_lossy(&result.stdout).trim() == "1" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "original PMS boot did not complete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    let properties =
        String::from_utf8(run(boot.command().args(["shell", "getprop"])).stdout).unwrap();
    let properties: BTreeMap<_, _> = properties
        .lines()
        .filter_map(|line| {
            let (name, value) = line
                .strip_prefix('[')?
                .strip_suffix(']')?
                .split_once("]: [")?;
            Some((name.to_owned(), value.to_owned()))
        })
        .collect();
    let image = aim_paths::derived_image();
    let config = SystemConfig::read(&image, &|name| properties.get(name).cloned());
    let mut platform = Platform::load(&image, Default::default()).unwrap();
    let density =
        String::from_utf8(run(boot.command().args(["shell", "wm", "density"])).stdout).unwrap();
    platform.density_dpi = Some(
        density
            .lines()
            .filter_map(|line| {
                line.strip_prefix("Physical density: ")
                    .or_else(|| line.strip_prefix("Override density: "))
                    .map(|n| n.parse().unwrap())
            })
            .last()
            .expect("original display density"),
    );
    let output = String::from_utf8(
        run(boot
            .command()
            .args(["shell", "dumpsys", "package", "libraries"]))
        .stdout,
    )
    .unwrap();
    let apex_xml = run(boot
        .command()
        .args(["shell", "cat", "/apex/apex-info-list.xml"]));
    let apex_xml = aim_android_xml::read(&apex_xml.stdout).unwrap();
    let apexes: Vec<_> = apex_xml
        .children()
        .filter(|e| e.name == "apex-info")
        .filter(|e| e.attr("isActive").unwrap().string().unwrap() == "true")
        .map(|e| {
            let attr = |name| e.attr(name).unwrap().string().unwrap().into_owned();
            let original = attr("preinstalledModulePath");
            let (partition, _) = partition_path(&original).unwrap();
            Apex {
                mount_path: format!("/apex/{}", attr("moduleName")),
                partition,
                factory: attr("isFactory") == "true",
                // This diagnostic does not select changed APEX versions.
                active_changed: false,
            }
        })
        .collect();
    // Freeze only this owned data image before comparing disk state.
    run(boot.command().arg("stop"));
    let volume = aim_storage::data::DataImage::attach(&boot.data, None).unwrap();
    let original = State::read(&boot.data.join("data"), &[0]).unwrap().unwrap();
    assert_eq!(original.settings.packages.len(), 243);
    assert_eq!(original.settings.shared_users.len(), 16);
    let data_files = boot.data.join("data");
    let apks = Apks {
        files: Box::new(move |path| {
            Some(if let Some(relative) = path.strip_prefix("/data/") {
                data_files.join(relative)
            } else {
                image.join(path.trim_start_matches('/'))
            })
        }),
        platform,
    };
    let inputs = Inputs::load(&original, &apks).unwrap();
    assert_eq!(inputs.active.len(), original.settings.packages.len());
    assert_eq!(
        inputs.disabled.len(),
        original.settings.disabled_system_packages.len()
    );
    let first_api = properties
        .get("ro.product.first_api_level")
        .map(|n| n.parse().unwrap())
        .unwrap_or(0);
    let mut scan = SigningScan::new(&config, &original.settings, first_api).unwrap();
    let platform_signing = &inputs.active["android"].signing;
    let vendor_sdk = properties
        .get("ro.vndk.version")
        .map(|v| {
            v.parse().unwrap_or_else(|_| {
                if apks.platform.codenames.contains(v) {
                    10000
                } else {
                    28
                }
            })
        })
        .unwrap_or(28);
    let mut checked_flags = 0;
    let mut flag_mismatches = Vec::new();
    for saved in &original.settings.packages {
        assert_eq!(
            apks.scan_file_time(&inputs.active[&saved.name].parsed)
                .unwrap(),
            saved.last_modified_time,
            "original scan file time: {}",
            saved.name
        );

        let record = &inputs.active[&saved.name];
        let location = scan_location(&saved.code_path, &apexes);
        // InitAppsHelper scans overlay directories before framework-res.
        let before_platform = location.as_ref().is_some_and(|l| l.kind == Kind::Overlay);
        let mut policy = location
            .as_ref()
            .map(ScanPolicy::for_location)
            .unwrap_or_default();
        let disabled = inputs.disabled.get(&saved.name);
        if let Some(original) = disabled {
            // Disabled XML restores only system/priv-app flags. The original
            // factory scan refreshes partition flags before scanning its update.
            let location = scan_location(&original.settings.code_path, &apexes).unwrap();
            let factory_policy = ScanPolicy::for_location(&location);
            let mut parsed = original.parsed.clone();
            factory_policy
                .apply_manifest(
                    &mut parsed,
                    &original.signing,
                    if location.kind == Kind::Overlay {
                        None
                    } else {
                        Some(platform_signing)
                    },
                    false,
                    &apks,
                )
                .unwrap();
            let mut factory_setting = original.settings.clone();
            (factory_setting.flags, factory_setting.private_flags) =
                application_flags(&parsed, false);
            policy.inherit_system_setting(&factory_setting);
        }
        policy.adjust_shared_uid_privilege(
            &record.parsed,
            &record.signing,
            platform_signing,
            &scan.identities,
            vendor_sdk,
        );
        let mut parsed = record.parsed.clone();
        policy
            .apply_manifest(
                &mut parsed,
                &record.signing,
                if before_platform {
                    None
                } else {
                    Some(platform_signing)
                },
                disabled.is_some(),
                &apks,
            )
            .unwrap();
        let actual = application_flags(&parsed, disabled.is_some());
        if actual != (saved.flags, saved.private_flags) {
            flag_mismatches.push(format!(
                "{}: native {actual:?}, original {:?}",
                saved.name,
                (saved.flags, saved.private_flags)
            ));
        }
        checked_flags += 1;

        let result = scan
            .apply_with_disabled(
                &inputs.active[&saved.name],
                inputs.disabled.get(&saved.name),
            )
            .unwrap_or_else(|error| panic!("{}: {error:?}", saved.name));
        assert!(result.system_signature_mismatch.is_none(), "{}", saved.name);
    }
    assert_eq!(checked_flags, 243);
    assert!(
        flag_mismatches.is_empty(),
        "original scan application flags: {flag_mismatches:#?}"
    );
    eprintln!(
        "native scan manifest policy/application flags match all {checked_flags} original packages"
    );
    for (saved, candidate) in original
        .settings
        .packages
        .iter()
        .zip(&scan.settings.packages)
    {
        let mut expected = saved.clone();
        let keys = candidate.signatures.as_ref().unwrap().public_keys.clone();
        assert!(
            keys.as_ref().is_some_and(|k| !k.is_empty()),
            "{}",
            saved.name
        );
        expected.signatures.as_mut().unwrap().public_keys = keys;
        assert_eq!(&expected, candidate, "{}", saved.name);
    }
    assert_eq!(original.settings.shared_users, scan.settings.shared_users);
    let mut expected: Vec<_> = output
        .lines()
        .filter_map(|line| {
            line.strip_prefix("  ")
                .filter(|line| line.contains(" -> "))
                .map(str::to_owned)
        })
        .collect();
    assert!(!expected.is_empty(), "original library dump: {output}");
    let mut actual: Vec<_> = scan
        .libraries
        .entries()
        .map(|library| {
            let mut line = library.name.clone().unwrap();
            if library.kind == TYPE_STATIC {
                line.push_str(&format!(" version={}", library.version));
            }
            line.push_str(" -> ");
            if let Some(path) = &library.path {
                line.push_str(if library.native { " (so) " } else { " (jar) " });
                line.push_str(path);
            } else {
                line.push_str(" (apk) ");
                line.push_str(library.package_name.as_ref().unwrap());
            }
            line
        })
        .collect();
    expected.sort();
    actual.sort();
    assert_eq!(actual, expected);
    eprintln!(
        "reconciled {} active / {} disabled packages, {} shared UID groups, {} libraries",
        inputs.active.len(),
        inputs.disabled.len(),
        scan.settings.shared_users.len(),
        actual.len()
    );
    assert_eq!(
        State::read(&boot.data.join("data"), &[0]).unwrap().unwrap(),
        original
    );
    volume.detach().unwrap();
}

fn partition_path(path: &str) -> Option<(Partition, &str)> {
    [
        ("/system/", Partition::System),
        ("/vendor/", Partition::Vendor),
        ("/odm/", Partition::Odm),
        ("/oem/", Partition::Oem),
        ("/product/", Partition::Product),
        ("/system_ext/", Partition::SystemExt),
    ]
    .iter()
    .find_map(|(prefix, p)| path.strip_prefix(prefix).map(|r| (*p, r)))
}

fn scan_location(path: &str, apexes: &[Apex]) -> Option<Location> {
    if path.starts_with("/data/app/") {
        return None;
    }
    let apex = apexes
        .iter()
        .find(|a| path.starts_with(&format!("{}/", a.mount_path)));
    let (partition, relative) = partition_path(path)
        .or_else(|| {
            apex.map(|a| {
                (
                    a.partition,
                    path.strip_prefix(&format!("{}/", a.mount_path)).unwrap(),
                )
            })
        })
        .unwrap_or_else(|| panic!("scan flag diagnostic needs explicit location: {path}"));
    Some(Location {
        path: path.into(),
        partition,
        kind: if relative.starts_with("overlay/") {
            Kind::Overlay
        } else if relative.starts_with("priv-app/") {
            Kind::PrivApp
        } else if relative.starts_with("framework/") {
            Kind::Framework
        } else {
            Kind::App
        },
        apex: apex.cloned(),
    })
}
