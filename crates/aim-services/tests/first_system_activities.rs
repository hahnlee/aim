//! Fresh native image scan and component queries, without booting Android.
//! android-16.0.0_r1 ComputerEngine.updateFlags/queryIntentActivitiesInternal
//! and ComponentResolver.ActivityIntentResolver.newResult: user unlock state
//! controls direct-boot matching; installed/enabled alone does not suffice.
mod common {
    pub mod query_projection;
    pub mod seinfo;
}
use aim_services::package::{
    parse::Platform,
    scan::{
        AbiPolicy, AbiScanContext, AbiScanMode, FirstBootSystemInputs, Image, LibraryCompatibility,
        NativeLibraryEnvironment, NativeLibraryInstallPolicy, ScanClock, ScanMetadataCompletion,
        SettingUpdate, SystemImageScan, UserPolicy,
    },
    system_config::SystemConfig,
    write::Apks,
};
use std::collections::BTreeMap;
struct Fixture(std::path::PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn fresh_image_scan_publishes_calendar_and_chrome_activities() {
    use aim_services::package::{
        intent::Intent,
        model,
        owner::usage::Usage,
        query::Query,
        scan_snapshot::{Store, query_state::Capture},
    };
    let original = aim_paths::original_image();
    let config = SystemConfig::read(&original, &|_| None);
    let data = std::env::temp_dir().join(format!("aim-fresh-activities-{}", std::process::id()));
    std::fs::create_dir_all(data.join("app")).unwrap();
    let fixture = Fixture(data.clone());
    let mut platform = Platform::load(
        &original,
        config
            .features
            .iter()
            .map(|(name, _)| name.clone())
            .collect(),
    )
    .unwrap();
    platform.density_dpi = Some(320);
    let apks = Apks {
        signing_overrides: None,
        files: Box::new(move |path| {
            Some(match path.strip_prefix("/data/") {
                Some(tail) => data.join(tail),
                None => original.join(path.trim_start_matches('/')),
            })
        }),
        platform,
    };
    let compatibility = LibraryCompatibility::new(&config, &|_| None, true).unwrap();
    let abi = AbiPolicy {
        all: vec!["arm64-v8a".into()],
        bit32: vec![],
        bit64: vec!["arm64-v8a".into()],
        native32: vec![],
        native64: vec!["arm64-v8a".into()],
        force_multi_arch_match: false,
    };
    let mut scan = SystemImageScan::first_boot_parsed(
        || Image::parse(&apks, &[]),
        &apks,
        &config,
        FirstBootSystemInputs {
            certificates: Default::default(),
            seinfo: common::seinfo::scan(),
            apex_image: &Default::default(),
            notify_apex_scan: &|_| Ok(()),
            first_api_level: 36,
            vendor_sdk: 36,
            shared_uid_migration: aim_services::package::scan::SharedUidMigration::NewInstallOnly,
            abi_policy: &abi,
            compatibility: &compatibility,
            preferred_abi: "arm64-v8a",
            app_lib32_install_dir: "/data/app-lib",
            platform_runtime_64bit: true,
            install: NativeLibraryInstallPolicy {
                page_size: 16384,
                extract: false,
                debuggable: false,
                compat_16kb_disabled: false,
                manifest_compat_disabled: false,
            },
            clock: ScanClock {
                current_time: 0,
                user_id: 0,
                update_time: false,
            },
            factory_test: false,
            users: UserPolicy {
                install_user: Some(0),
                users: None,
                allow_install: true,
                instant_app: false,
                virtual_preload: false,
                stopped_system_app: false,
            },
            new_domain_id: &|| Ok([1; 16]),
        },
    )
    .unwrap();
    for name in ["com.google.android.calendar", "com.android.chrome"] {
        let code = &scan.owner.loaded_packages()[name];
        assert_eq!(code.package.activities, code.runtime_package().activities);
        let user = &scan.owner.scanned_user_states(name).unwrap()[&0];
        assert!(user.installed && !user.hidden);
        assert_eq!(user.enabled, 0);
        assert!(
            !code.package.activities.is_empty(),
            "parsed activities: {name}"
        );
    }
    scan.owner.fix_shared_seinfo_target_sdks_at_boot().unwrap();
    scan.owner
        .complete_library_dependencies(&|_, _| {
            Ok(aim_services::package::libraries::Policy::pinned(false))
        })
        .unwrap();
    let usage = Usage::new(scan.owner.settings.packages.iter().map(|p| p.name.as_str()));
    scan.owner
        .complete_runtime_at_boot(&usage, Default::default())
        .unwrap();
    let migrations = scan
        .owner
        .settings
        .packages
        .iter()
        .map(|p| ((p.name.clone(), false), Default::default()))
        .collect();
    let groups = scan
        .owner
        .identities
        .shared_users
        .keys()
        .map(|name| (name.clone(), Default::default()))
        .collect();
    scan.owner
        .capture_legacy_permissions(&[0], migrations, groups)
        .unwrap();
    let fixed = scan
        .owner
        .settings
        .packages
        .iter()
        .map(|p| ((p.name.clone(), false), false))
        .collect();
    scan.owner.capture_install_permissions_fixed(fixed).unwrap();
    scan.owner
        .rebuild_shared_processes_from_native_members()
        .unwrap();
    // Complete the compressed system packages after the directory scan, as
    // installSystemStubPackages does, retaining the actual factory runtime.
    // SELinux labelling is an explicit external fixture input. Gzip, native
    // libraries, certificates, scan admission and dependency resolution are real.
    let factories = scan.owner.clone();
    let mut retained = BTreeMap::new();
    let decompression =
        aim_services::package::scan::boot_compressed::BootDecompression::new(fixture.0.join("app"))
            .unwrap();
    let names = scan
        .owner
        .settings
        .packages
        .iter()
        .filter(|p| {
            scan.owner.loaded_packages()[&p.name]
                .package
                .is2(aim_services::package::pkg::booleans2::STUB)
        })
        .map(|p| p.name.clone())
        .collect::<Vec<_>>();
    for name in names.into_iter().rev() {
        let saved = scan
            .owner
            .settings
            .packages
            .iter()
            .find(|p| p.name == name)
            .unwrap()
            .clone();
        let stub = scan.owner.loaded_packages()[&name].package.clone();
        let install = NativeLibraryInstallPolicy {
            page_size: 16384,
            extract: false,
            debuggable: false,
            compat_16kb_disabled: false,
            manifest_compat_disabled: false,
        };
        let zip_time = |_| Ok(std::time::SystemTime::UNIX_EPOCH);
        let expanded = decompression
            .decompress(&stub, &apks, &abi, install, &zip_time, &|_| Ok(()))
            .unwrap()
            .unwrap();
        let mut parsed = expanded.package;
        let signing = apks.signing_details(&parsed).unwrap();
        let platform = scan.owner.loaded_packages()["android"]
            .collected_signing
            .clone();
        let mut policy = aim_services::package::scan::ScanPolicy::default();
        policy.inherit_system_setting(&saved);
        policy.adjust_shared_uid_privilege(
            &parsed,
            &signing,
            &platform,
            &scan.owner.identities,
            36,
        );
        policy
            .apply_manifest(&mut parsed, &signing, Some(&platform), true, &apks)
            .unwrap();
        compatibility.apply(&mut parsed, true, true, None).unwrap();
        let (flags, private_flags) = aim_services::package::scan::application_flags(&parsed, true);
        let update = SettingUpdate {
            code_path: expanded.receipt.guest_path.clone(),
            legacy_native_library_path: saved.legacy_native_library_path.clone(),
            primary_cpu_abi: None,
            secondary_cpu_abi: None,
            flags,
            private_flags,
            uses_sdk_libraries: vec![],
            uses_static_libraries: parsed
                .uses_static_libraries
                .iter()
                .cloned()
                .zip(
                    parsed
                        .uses_static_libraries_versions
                        .iter()
                        .flatten()
                        .copied(),
                )
                .collect(),
            mime_groups: parsed.mime_groups.clone(),
            domain_set_id: [2; 16],
            target_sdk_version: parsed.target_sdk_version,
            restrict_update_hash: parsed.restrict_update_hash.clone(),
        };
        assert!(parsed.uses_sdk_libraries.is_empty());
        let factory = &scan
            .packages
            .iter()
            .find(|p| p.candidate.record.settings.name == name)
            .unwrap()
            .candidate
            .record;
        retained.insert(
            (name.clone(), true),
            aim_services::package::scan::OriginalRuntime {
                name: name.clone(),
                app_id: saved.app_id,
                path: saved.code_path.clone(),
                version: saved.version_code,
                has_code: true,
                state: factories
                    .replica_runtime(&name, false)
                    .unwrap()
                    .unwrap()
                    .clone(),
                transient: saved.transient.clone(),
            },
        );
        scan.owner.disable_system_package(&name).unwrap();
        let users = scan
            .owner
            .settings
            .packages
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    scan.owner.scanned_user_states(&p.name).unwrap().clone(),
                )
            })
            .collect();
        let environment = NativeLibraryEnvironment {
            preferred_abi: "arm64-v8a",
            app_lib32_install_dir: "/data/app-lib",
            code_is_directory: true,
            canonical_source: None,
        };
        let code = aim_services::package::scan::Code {
            location: aim_services::package::scan::Location {
                path: expanded.receipt.guest_path,
                partition: aim_services::package::scan::Partition::Data,
                kind: aim_services::package::scan::Kind::App,
                apex: None,
            },
            parsed,
            signing,
        };
        scan.owner
            .scan_existing(
                &code,
                update,
                &users,
                None,
                Some(factory),
                &apks,
                ScanMetadataCompletion {
                    scan_as_instant_app: false,
                    seinfo: common::seinfo::scan(),
                    abi_policy: &abi,
                    native_environment: &environment,
                    context: AbiScanContext {
                        mode: AbiScanMode::Existing {
                            first_boot_or_upgrade: true,
                            old_was_stub: true,
                            saved: Some(&saved),
                        },
                        system: true,
                        updated: true,
                        override_abi: None,
                        platform_runtime_64bit: None,
                    },
                    install,
                    destination: None,
                    clock: ScanClock {
                        current_time: 0,
                        user_id: 0,
                        update_time: false,
                    },
                    factory_test: false,
                },
            )
            .unwrap();
    }
    scan.owner.fix_shared_seinfo_target_sdks_at_boot().unwrap();
    scan.owner
        .complete_library_dependencies(&|_, _| {
            Ok(aim_services::package::libraries::Policy::pinned(false))
        })
        .unwrap();
    scan.owner
        .complete_runtime_at_boot(&usage, retained)
        .unwrap();
    let fixed = scan
        .owner
        .settings
        .packages
        .iter()
        .map(|p| ((p.name.clone(), false), false))
        .chain(
            scan.owner
                .settings
                .disabled_system_packages
                .iter()
                .map(|p| ((p.name.clone(), true), false)),
        )
        .collect();
    scan.owner.capture_install_permissions_fixed(fixed).unwrap();
    scan.owner
        .rebuild_shared_processes_from_native_members()
        .unwrap();
    let chrome = &scan.owner.loaded_packages()["com.android.chrome"].package;
    assert!(!chrome.is2(aim_services::package::pkg::booleans2::STUB));
    assert!(
        chrome
            .uses_static_libraries
            .iter()
            .any(|name| name.contains("trichrome"))
    );
    assert!(
        scan.owner
            .library_dependencies("com.android.chrome")
            .unwrap()
            .unwrap()
            .0
            .iter()
            .flatten()
            .any(|file| file.ends_with("/TrichromeLibrary.apk"))
    );
    let snapshot = Store::new(scan.owner, usage).unwrap().capture();
    let mut context = common::query_projection::context(&snapshot);
    context.users = [(
        0,
        model::User {
            id: 0,
            unlocking_or_unlocked: true,
            ..Default::default()
        },
    )]
    .into();
    for package in context.packages.values_mut() {
        package.users.retain(|id, _| *id == 0);
    }
    context
        .scan_users
        .users
        .as_mut()
        .unwrap()
        .retain(|user| user.id == 0);
    let mut locked_context = context.clone();
    locked_context
        .users
        .get_mut(&0)
        .unwrap()
        .unlocking_or_unlocked = false;
    let locked = Capture::new(snapshot.clone(), locked_context).unwrap();
    let launcher = Intent {
        action: Some("android.intent.action.MAIN".into()),
        categories: Some(vec!["android.intent.category.LAUNCHER".into()]),
        ..Default::default()
    };
    let locked_results = locked
        .resolution()
        .unwrap()
        .query_intent_activities(&launcher, None, 0, 0, 2000)
        .unwrap();
    assert_eq!(
        locked_results
            .iter()
            .map(|r| r.component().0)
            .collect::<Vec<_>>(),
        [
            "com.android.settings",
            "com.google.android.deskclock",
            "com.google.android.dialer"
        ]
    );
    let capture = Capture::new(snapshot, context).unwrap();
    let unlocked_results = capture
        .resolution()
        .unwrap()
        .query_intent_activities(&launcher, None, 0, 0, 2000)
        .unwrap();
    assert_eq!(unlocked_results.len(), 19);
    let resolution = capture.resolution().unwrap();
    let query = Query {
        state: &resolution.state,
        filter: &resolution.apps_filter,
        calling_uid: 2000,
    };
    assert!(
        resolution.state.packages["com.android.shell"]
            .pkg
            .as_ref()
            .unwrap()
            .requested_permissions
            .iter()
            .any(|permission| permission == "android.permission.QUERY_ALL_PACKAGES")
    );
    for name in ["com.google.android.calendar", "com.android.chrome"] {
        use aim_service_aidl::android_content_pm_ipackagemanager as pm;
        let mut request = aim_binder_host::parcel::Parcel::new();
        pm::IsPackageAvailable {
            package_name: Some(name.into()),
            user_id: 0,
        }
        .write(&mut request);
        let reply = query
            .answer(
                pm::DESCRIPTOR,
                pm::IS_PACKAGE_AVAILABLE,
                &mut request.reader(),
            )
            .unwrap();
        assert!(
            pm::read_is_package_available_reply(&mut reply.reader())
                .unwrap()
                .unwrap()
        );
        let intent = Intent {
            action: Some("android.intent.action.MAIN".into()),
            categories: Some(vec!["android.intent.category.LAUNCHER".into()]),
            package: Some(name.into()),
            ..Default::default()
        };
        let hidden = locked
            .resolution()
            .unwrap()
            .query_intent_activities(&intent, None, 0, 0, 2000)
            .unwrap();
        assert!(
            hidden.is_empty(),
            "encryption-unaware launcher while locked: {name}"
        );
        let found = resolution
            .query_intent_activities(&intent, None, 0, 0, query.calling_uid)
            .unwrap();
        assert!(!found.is_empty(), "fresh image launcher activities: {name}");
    }
}
