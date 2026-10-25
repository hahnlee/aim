use super::*;
use crate::package::{info, model, pkg};

fn host() -> Arc<AppHost> {
    AppHost::new(LocalProcess::open(
        &aim_binder_driver::Driver::new(),
        aim_binder_driver::Device::Binder,
        aim_binder_driver::Credentials {
            pid: std::process::id() as i32,
            euid: SYSTEM_UID,
            security_context: None,
        },
    ))
}

/// `SystemServer.startBootstrapServices` calls `setSystemProcess` after PMS.
#[test]
fn system_process_attach_waits_for_package_boot_outcome() {
    for outcome in [
        Ok(()),
        Err(Exception::new(EX_ILLEGAL_STATE, "package boot failed")),
    ] {
        let host = host();
        let expected = outcome
            .clone()
            .and_then(|()| Err(Exception::new(EX_ILLEGAL_STATE, "app host not configured")));
        let (started, arrival) = std::sync::mpsc::channel();
        let (done, result) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let p = Parcel::new();
                let call = Call {
                    code: 0,
                    flags: 0,
                    sender_pid: 42,
                    sender_euid: SYSTEM_UID,
                    data: p.reader(),
                };
                started.send(()).unwrap();
                done.send(host.attach_system_process(&call, Some(Binder::Handle(1))))
                    .unwrap();
            });
            arrival.recv().unwrap();
            let blocked = result.recv_timeout(Duration::from_millis(50));
            host.package_boot.set(outcome).unwrap();
            assert_eq!(blocked, Err(std::sync::mpsc::RecvTimeoutError::Timeout));
            assert_eq!(result.recv().unwrap(), expected);
        });
    }
}

/// This internal generator keeps the old user-0/flags contract. It must
/// not acquire another model or add public-query visibility/caller policy.
#[test]
fn application_info_reuses_acquisition_and_preserves_trusted_flags() {
    let host = host();
    let pkg = Arc::new(pkg::AndroidPackage {
        package_name: "example.app".into(),
        uid: 10_123,
        target_sdk_version: 36,
        base_apk_path: Some("/data/app/example/base.apk".into()),
        primary_cpu_abi: Some("arm64-v8a".into()),
        ..Default::default()
    });
    let mut state = model::State::default();
    let mut ps = model::PackageState {
        name: pkg.package_name.clone(),
        app_id: pkg.uid,
        pkg: Some(pkg),
        primary_cpu_abi: Some("arm64-v8a".into()),
        seinfo: Some("default:targetSdkVersion=36".into()),
        ..Default::default()
    };
    ps.users.insert(
        0,
        model::PackageUserState {
            installed: true,
            data_exists: true,
            enabled: 2,
            ..Default::default()
        },
    );
    ps.users.insert(10, model::PackageUserState::default());
    state.packages.insert(ps.name.clone(), ps);
    for hidden in [false, true] {
        for installed in [false, true] {
            let ps = state.packages.get_mut("example.app").unwrap();
            ps.users.get_mut(&0).unwrap().hidden = hidden;
            ps.users.get_mut(&0).unwrap().installed = installed;
            for flags in [
                0,
                STOCK_PM_FLAGS,
                0x20000, // PackageManager.MATCH_ALL.
                info::flags::MATCH_UNINSTALLED_PACKAGES,
                info::flags::MATCH_SYSTEM_ONLY,
            ] {
                let ps = &state.packages["example.app"];
                let baseline = info::generate_application_info(
                    &info::Target {
                        sys: &state.system,
                        pkg: ps.pkg.as_deref().unwrap(),
                        ps,
                        state: &ps.users[&0],
                        user: 0,
                    },
                    flags,
                );
                let actual = host
                    .application_info("example.app", flags, Some(&state))
                    .unwrap();
                assert_eq!(
                    actual, baseline,
                    "hidden={hidden} installed={installed} flags={flags:#x}"
                );
                if let Some(info) = actual {
                    assert_eq!(info.uid, 10_123);
                    assert!(!info.enabled);
                    assert_eq!(info.primary_cpu_abi.as_deref(), Some("arm64-v8a"));
                    assert_eq!(info.se_info.as_deref(), Some("default:targetSdkVersion=36"));
                    assert_eq!(
                        info.source_dir.as_deref(),
                        Some("/data/app/example/base.apk")
                    );
                    assert_eq!(
                        info.credential_protected_data_dir.as_deref(),
                        Some("/data/user/0/example.app")
                    );
                    assert_eq!(
                        info.device_protected_data_dir.as_deref(),
                        Some("/data/user_de/0/example.app")
                    );
                }
            }
        }
    }
    assert!(
        host.application_info("missing", 0, Some(&state))
            .unwrap()
            .is_none()
    );
    state
        .packages
        .get_mut("example.app")
        .unwrap()
        .users
        .remove(&0);
    assert!(
        host.application_info("example.app", STOCK_PM_FLAGS, Some(&state))
            .unwrap()
            .is_none()
    );
    // No supplied capture means an ordinary acquisition, with its real error.
    assert_eq!(
        host.application_info("example.app", 0, None)
            .unwrap_err()
            .message,
        "native package system unavailable"
    );
}
