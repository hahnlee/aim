use aim_binder_host::parcel::{Parcel, Reader};
use aim_service_aidl::{ReadParcelable, WriteParcelable};
use aim_services::package::installer::codec::{Object, SessionInfo, SessionParams};
use std::{fs, path::Path};
fn uri(mode: i32) -> Option<Object> {
    if mode < 2 {
        return None;
    }
    let mut p = Parcel::new();
    p.write_string16(Some(if mode == 2 {
        "android.net.Uri$StringUri"
    } else {
        if mode == 4 {
            "android.net.Uri$HierarchicalUri"
        } else {
            "android.net.Uri$OpaqueUri"
        }
    }));
    p.write_i32(if mode == 2 {
        1
    } else if mode == 4 {
        3
    } else {
        2
    });
    p.write_string8(Some(if mode == 2 {
        "https://example.test/a?b=c#d"
    } else {
        if mode == 4 {
            "https://hierarchy.test/a%20b?x%3Dy#z"
        } else {
            "scheme:body#fragment"
        }
    }));
    Some(Object {
        bytes: p.data().to_vec(),
        objects: vec![],
    })
}
fn bitmap(mode: i32) -> Option<Object> {
    if mode != 2 && mode != 4 {
        return None;
    }
    let mut p = Parcel::new();
    p.write_string16(Some("android.graphics.Bitmap"));
    for value in [1, 4, 2, -1, 1, 1, 4, 160] {
        p.write_i32(value);
    }
    p.write_i64(0);
    p.write_i32(0);
    p.write_i32(4);
    p.write_i32(0xff563412u32 as i32);
    p.write_bool(mode == 4);
    if mode == 4 {
        p.write_i32(1);
        p.write_i32(1);
        let nested = bitmap(2).unwrap();
        let mut r = Reader::new(&nested.bytes, &[]);
        r.read_string16().unwrap();
        let at = r.position();
        p.write_raw(&nested.bytes[at..], &[]);
        for value in [
            0.5f32, 0.75, 1.0, 2.0, 3.0, 4.0, 0.8, 1.0, 1.2, 0.001, 0.002, 0.003, 0.004, 0.005,
            0.006, 1.2, 4.0,
        ] {
            p.write_f32(value);
        }
        p.write_i32(0);
    }
    Some(Object {
        bytes: p.data().to_vec(),
        objects: vec![],
    })
}
fn loader(mode: i32) -> Option<Object> {
    if mode != 3 {
        return None;
    }
    let mut p = Parcel::new();
    p.write_string16(Some("android.content.pm.DataLoaderParamsParcel"));
    let at = p.position();
    p.write_i32(0);
    p.write_i32(1);
    p.write_string16(Some("loader.package"));
    p.write_string16(Some("Loader"));
    p.write_string16(Some("--arg 😀"));
    p.set_i32_at(at, (p.position() - at) as i32);
    Some(Object {
        bytes: p.data().to_vec(),
        objects: vec![],
    })
}
fn params(mode: i32) -> SessionParams {
    SessionParams {
        mode: if mode == 0 { 0 } else { 11 },
        install_flags: if mode == 0 { 0 } else { 12 },
        install_location: if mode == 0 { 0 } else { 13 },
        install_reason: if mode == 0 { 0 } else { 14 },
        install_scenario: if mode == 0 { 0 } else { 15 },
        size_bytes: if mode == 0 { 0 } else { 0x100000000 + 16 },
        app_package_name: (mode != 0).then(|| "app_package_name 😀".into()),
        app_icon: bitmap(mode),
        app_label: (mode != 0).then(|| "app_label 😀".into()),
        originating_uri: uri(mode),
        originating_uid: if mode == 0 { 0 } else { 21 },
        referrer_uri: uri(mode),
        abi_override: (mode != 0).then(|| "abi_override 😀".into()),
        volume_uuid: (mode != 0).then(|| "volume_uuid 😀".into()),
        permission_states: if mode == 0 {
            vec![]
        } else {
            vec![(Some("BB".into()), Some(2)), (Some("Aa".into()), Some(2))]
        },
        whitelisted_restricted_permissions: (mode != 0)
            .then(|| vec![Some("BB".into()), None, Some("Aa".into())]),
        auto_revoke_permissions_mode: if mode == 0 { 0 } else { 27 },
        installer_package_name: (mode != 0).then(|| "installer_package_name 😀".into()),
        multi_package: mode != 0 && true,
        staged: mode != 0 && false,
        force_queryable_override: mode != 0 && true,
        required_installed_version_code: if mode == 0 { 0 } else { 0x100000000 + 32 },
        data_loader_params: loader(mode),
        rollback_data_policy: if mode == 0 { 0 } else { 34 },
        rollback_lifetime_millis: if mode == 0 { 0 } else { 0x100000000 + 35 },
        rollback_impact_level: if mode == 0 { 0 } else { 36 },
        require_user_action: if mode == 0 { 0 } else { 37 },
        package_source: if mode == 0 { 0 } else { 38 },
        application_enabled_setting_persistent: mode != 0 && true,
        development_install_flags: if mode == 0 { 0 } else { 40 },
        unarchive_id: if mode == 0 { 0 } else { 41 },
        dexopt_compiler_filter: (mode != 0).then(|| "dexopt_compiler_filter 😀".into()),
        auto_install_dependencies_enabled: mode != 0 && true,
    }
}
fn info(mode: i32) -> SessionInfo {
    SessionInfo {
        session_id: if mode == 0 { 0 } else { 11 },
        user_id: if mode == 0 { 0 } else { 12 },
        installer_package_name: (mode != 0).then(|| "installer_package_name 😀".into()),
        installer_attribution_tag: (mode != 0).then(|| "installer_attribution_tag 😀".into()),
        resolved_base_code_path: (mode != 0).then(|| "resolved_base_code_path 😀".into()),
        progress: if mode == 0 { 0.0 } else { 0.375 },
        sealed: mode != 0 && true,
        active: mode != 0 && false,
        mode: if mode == 0 { 0 } else { 19 },
        install_reason: if mode == 0 { 0 } else { 20 },
        install_scenario: if mode == 0 { 0 } else { 21 },
        size_bytes: if mode == 0 { 0 } else { 0x100000000 + 22 },
        app_package_name: (mode != 0).then(|| "app_package_name 😀".into()),
        app_icon: bitmap(mode),
        app_label: (mode != 0).then(|| "app_label 😀".into()),
        install_location: if mode == 0 { 0 } else { 26 },
        originating_uri: uri(mode),
        originating_uid: if mode == 0 { 0 } else { 28 },
        referrer_uri: uri(mode),
        granted_runtime_permissions: (mode != 0)
            .then(|| vec![Some("BB".into()), None, Some("Aa".into())]),
        whitelisted_restricted_permissions: (mode != 0)
            .then(|| vec![Some("BB".into()), None, Some("Aa".into())]),
        auto_revoke_permissions_mode: if mode == 0 { 0 } else { 32 },
        install_flags: if mode == 0 { 0 } else { 33 },
        multi_package: mode != 0 && false,
        staged: mode != 0 && true,
        force_queryable: mode != 0 && false,
        parent_session_id: if mode == 0 { 0 } else { 37 },
        child_session_ids: Some(if mode == 0 { vec![] } else { vec![37, 99] }),
        session_applied: mode != 0 && true,
        session_ready: mode != 0 && false,
        session_failed: mode != 0 && true,
        session_error_code: if mode == 0 { 0 } else { 42 },
        session_error_message: (mode != 0).then(|| "session_error_message 😀".into()),
        committed: mode != 0 && false,
        preapproval_requested: mode != 0 && true,
        rollback_data_policy: if mode == 0 { 0 } else { 46 },
        rollback_lifetime_millis: if mode == 0 { 0 } else { 0x100000000 + 47 },
        rollback_impact_level: if mode == 0 { 0 } else { 48 },
        created_millis: if mode == 0 { 0 } else { 0x100000000 + 49 },
        require_user_action: if mode == 0 { 0 } else { 50 },
        installer_uid: if mode == 0 { 0 } else { 51 },
        package_source: if mode == 0 { 0 } else { 52 },
        application_enabled_setting_persistent: mode != 0 && true,
        pending_user_action_reason: if mode == 0 { 0 } else { 54 },
        auto_installing_dependencies_enabled: mode != 0 && true,
    }
}
pub fn export(dir: &Path) {
    for mode in 0..5 {
        let mut p = Parcel::new();
        params(mode).write_to(&mut p);
        fs::write(dir.join(format!("params-{mode}.native")), p.data()).unwrap();
        let mut p = Parcel::new();
        info(mode).write_to(&mut p);
        fs::write(dir.join(format!("info-{mode}.native")), p.data()).unwrap();
    }
}
pub fn verify(dir: &Path) {
    for mode in 0..5 {
        for kind in ["params", "info"] {
            let bytes = fs::read(dir.join(format!("{kind}-{mode}.original"))).unwrap();
            let mut r = Reader::new(&bytes, &[]);
            let mut p = Parcel::new();
            if kind == "params" {
                SessionParams::read_from(&mut r).unwrap().write_to(&mut p);
            } else {
                SessionInfo::read_from(&mut r).unwrap().write_to(&mut p);
            }
            assert_eq!(r.remaining(), 0, "{kind}:{mode}");
            assert_eq!(p.data(), bytes, "{kind}:{mode}");
        }
    }
}
