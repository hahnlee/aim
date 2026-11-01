//! CTS SharedUserIdTest.testSharedUidDifferentCerts, android-16.0.0_r1 (C111).
use super::*;
use crate::package::{owner::usage::Usage, scan::*, scan_snapshot::Store};
use std::collections::BTreeMap;

const FIRST: &str = "com.android.cts.shareuidinstall";
const FOREIGN: &str = "com.android.cts.shareuidinstalldiffcert";
const GROUP: &str = "cts.shared";
fn certificate(second: bool) -> Vec<u8> {
    if second {
        include_bytes!("../../../../../tests/fixtures/settings-certificate-2.der").to_vec()
    } else {
        include_bytes!("../../../../../tests/fixtures/settings-certificate-1.der").to_vec()
    }
}
fn shared(name: &str, second: bool) -> Request {
    let mut r = request(name, 1);
    r.code.shared_user_id = Some(GROUP.into());
    r.code.feature_flag_state = Some(vec![]);
    r.signing = SigningDetails::from_saved(&settings::Signatures {
        signatures: vec![certificate(second)],
        scheme_version: 2,
        ..Default::default()
    })
    .unwrap();
    r
}
fn install(owner: &SigningScan, r: Request) -> SigningScan {
    let users = [User {
        id: 0,
        pre_created: false,
        adb_install_disallowed: false,
    }];
    let files: crate::package::write::Files = Box::new(|_| panic!("no update reads"));
    let admission = owner
        .prepare_live_installs(vec![r], &users, false, &files)
        .unwrap();
    let policy = crate::package::owner::seinfo::Policy::parse(&[]).unwrap();
    let abi = AbiPolicy {
        all: vec!["arm64-v8a".into()],
        bit32: vec![],
        bit64: vec!["arm64-v8a".into()],
        native32: vec![],
        native64: vec!["arm64-v8a".into()],
        force_multi_arch_match: false,
    };
    let env = NativeLibraryEnvironment {
        preferred_abi: "arm64-v8a",
        app_lib32_install_dir: "/data/app-lib",
        code_is_directory: true,
        canonical_source: None,
    };
    let mut table = vec![
        2, 0, 12, 0, 40, 0, 0, 0, 0, 0, 0, 0, 1, 0, 28, 0, 28, 0, 0, 0,
    ];
    table.extend([0; 12]);
    table.extend(28u32.to_le_bytes());
    table.extend([0; 4]);
    let mut zip = vec![0; 22];
    zip[..4].copy_from_slice(b"PK\x05\x06");
    let code_file =
        std::env::temp_dir().join(format!("aim-1173-code-{}-{}", std::process::id(), {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        }));
    std::fs::write(&code_file, zip).unwrap();
    let apk_file = code_file.clone();
    let apks = crate::package::write::Apks {
        signing_overrides: None,
        files: Box::new(move |_| Some(apk_file.clone())),
        platform: crate::package::parse::Platform {
            sdk: 36,
            codenames: vec![],
            sdk_extensions: None,
            features: Default::default(),
            flags: [("android.content.pm.app_compat_option_16kb".into(), false)].into(),
            flag_packages: Default::default(),
            split_permissions: vec![],
            locale: (*b"en", *b"US"),
            use_round_icon: false,
            recents_limit: 16,
            framework: crate::package::parse::resources::Table::parse(&table).unwrap(),
            framework_overlays: vec![],
            framework_overlay_apks: vec![],
            framework_attrs: Default::default(),
            density_dpi: Some(160),
        },
    };
    let mut completed = admission
        .complete_metadata(
            &apks,
            vec![ScanMetadataCompletion {
                scan_as_instant_app: false,
                seinfo: SeInfoScan {
                    policy: &policy,
                    compatibility: &|p: &AndroidPackage| Ok(p.target_sdk_version),
                },
                abi_policy: &abi,
                native_environment: &env,
                context: AbiScanContext {
                    mode: AbiScanMode::Install { moved: None },
                    system: false,
                    updated: false,
                    override_abi: None,
                    platform_runtime_64bit: None,
                },
                install: NativeLibraryInstallPolicy {
                    page_size: 4096,
                    extract: false,
                    debuggable: false,
                    compat_16kb_disabled: false,
                    manifest_compat_disabled: false,
                },
                destination: None,
                clock: ScanClock {
                    current_time: 20,
                    user_id: 0,
                    update_time: false,
                },
                factory_test: false,
            }],
        )
        .unwrap_or_else(|e| panic!("metadata: {:?}", e.error));
    std::fs::remove_file(code_file).unwrap();
    completed
        .owner
        .complete_library_dependencies(&|_, _| {
            Ok(crate::package::libraries::Policy {
                enforce_native_dependencies: false,
                sdk_library_independence: false,
            })
        })
        .unwrap();
    let usage = Usage::new(
        completed
            .owner
            .settings
            .packages
            .iter()
            .map(|p| p.name.as_str()),
    );
    let base = Store::new(
        owner.clone(),
        Usage::new(owner.settings.packages.iter().map(|p| p.name.as_str())),
    )
    .unwrap()
    .capture();
    crate::package::installer::permission_prepare::complete_candidate_runtime(
        &mut completed.owner,
        &base,
        &usage,
    )
    .unwrap();
    Store::new(completed.owner, usage)
        .unwrap()
        .capture()
        .owner()
        .clone()
}
fn remove(mut owner: SigningScan, name: &str) -> SigningScan {
    let mut user = owner.scanned_user_states(name).unwrap()[&0].clone();
    user.installed = false;
    owner.set_user_state(name, 0, user).unwrap();
    let mut next = owner.prepare_live_package_removal(name).unwrap();
    let usage = Usage::new(next.settings.packages.iter().map(|p| p.name.as_str()));
    next.complete_runtime_at_boot(&usage, BTreeMap::new())
        .unwrap();
    Store::new(next, usage).unwrap().capture().owner().clone()
}
#[test]
fn c111_shared_user_sequence_and_matching_member_lifecycle() {
    let empty = SigningScan::new(&SystemConfig::default(), &Default::default(), 36).unwrap();
    let first = install(&empty, shared(FIRST, false));
    let id = first.settings.packages[0].app_id;
    assert_eq!(
        first.identities.shared_users[GROUP]
            .package_names()
            .collect::<Vec<_>>(),
        [FIRST]
    );
    assert!(
        first
            .shared_processes(GROUP)
            .unwrap()
            .unwrap()
            .records()
            .is_empty()
    );
    let users = [User {
        id: 0,
        pre_created: false,
        adb_install_disallowed: false,
    }];
    let files: crate::package::write::Files = Box::new(|_| panic!("no update reads"));
    let before = first.clone();
    let error = first
        .prepare_live_installs(vec![shared(FOREIGN, true)], &users, false, &files)
        .err()
        .unwrap();
    assert_eq!(error.status, -8); // INSTALL_FAILED_SHARED_USER_INCOMPATIBLE
    assert_eq!(first, before);
    assert_eq!(
        first.identities.shared_users[GROUP]
            .package_names()
            .collect::<Vec<_>>(),
        [FIRST]
    );
    let cleaned = remove(first.clone(), FIRST);
    assert!(!cleaned.identities.shared_users.contains_key(GROUP));
    assert!(
        !cleaned
            .settings
            .shared_users
            .iter()
            .any(|g| g.name == GROUP)
    );
    let two = install(&first, shared("matching", false));
    assert!(two.settings.packages.iter().all(|p| p.app_id == id));
    assert_eq!(two.identities.shared_users[GROUP].member_count(), 2);
    let one = remove(two, FIRST);
    assert_eq!(one.identities.shared_users[GROUP].app_id, id);
    assert_eq!(
        one.identities.shared_users[GROUP]
            .package_names()
            .collect::<Vec<_>>(),
        ["matching"]
    );
    assert!(one.shared_processes(GROUP).unwrap().is_some());
    let none = remove(one, "matching");
    assert!(!none.identities.shared_users.contains_key(GROUP));
    assert!(none.identities.ids.get(id).is_none());
    assert_eq!(first, before); // old capture remains coherent
}
#[test]
fn shared_user_lineage_requires_shared_uid_capability() {
    let empty = SigningScan::new(&SystemConfig::default(), &Default::default(), 36).unwrap();
    let first = install(&empty, shared(FIRST, false));
    for allowed in [false, true] {
        let mut rotated = shared("rotated", true);
        rotated.signing.scheme_version = 3;
        rotated.signing.past_signing_certificates = Some(vec![
            (certificate(false), if allowed { 2 } else { 0 }),
            (certificate(true), 0),
        ]);
        let users = [User {
            id: 0,
            pre_created: false,
            adb_install_disallowed: false,
        }];
        let files: crate::package::write::Files = Box::new(|_| panic!("no update reads"));
        let result = first.prepare_live_installs(vec![rotated], &users, false, &files);
        if allowed {
            assert!(result.is_ok());
        } else {
            assert_eq!(result.err().unwrap().status, -8);
        }
    }
}
#[test]
fn shared_user_settings_persist_and_reload_through_last_removal() {
    let data = std::env::temp_dir().join(format!("aim-1173-{}", std::process::id()));
    std::fs::create_dir_all(data.join("system")).unwrap();
    std::fs::write(data.join("system/packages.xml"), b"<packages/>").unwrap();
    let mut disk = crate::package::owner::Store::open(&data, &[0])
        .unwrap()
        .unwrap();
    let empty = SigningScan::new(&SystemConfig::default(), &Default::default(), 36).unwrap();
    let first = install(&empty, shared(FIRST, false));
    let two = install(&first, shared("matching", false));
    for owner in [
        first,
        two.clone(),
        remove(two.clone(), FIRST),
        remove(remove(two, FIRST), "matching"),
    ] {
        let expected = owner.settings.packages.len();
        let usage = Usage::new(owner.settings.packages.iter().map(|p| p.name.as_str()));
        let snapshot = Store::new(owner.clone(), usage).unwrap().capture();
        disk.commit_scan_settings(&snapshot).unwrap();
        let saved = crate::package::owner::Store::open(&data, &[0])
            .unwrap()
            .unwrap();
        let restored =
            SigningScan::new(&SystemConfig::default(), &saved.state().settings, 36).unwrap();
        assert_eq!(restored.settings.packages.len(), expected);
        assert_eq!(
            restored.identities.shared_users.get(GROUP).map(|g| (
                g.app_id,
                g.member_count(),
                g.signatures.clone()
            )),
            owner.identities.shared_users.get(GROUP).map(|g| (
                g.app_id,
                g.member_count(),
                g.signatures.clone()
            ))
        );
    }
    std::fs::remove_dir_all(data).unwrap();
}
