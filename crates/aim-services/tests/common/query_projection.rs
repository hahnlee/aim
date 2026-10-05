//! Explicit controlled external owners; package fields come from the real native scan.
use aim_binder_host::parcel::Parcel;
use aim_services::package::{
    bootstrap::{ApexInventory, ScanUsers},
    model, scan,
    scan_snapshot::{
        Snapshot,
        query_state::{Capture, Context, PackageInputs, UserInputs},
    },
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    sync::Arc,
};

pub fn export(directory: &Path, snapshot: &Arc<Snapshot>) {
    let mut packages = BTreeMap::new();
    for (settings, factory) in [
        (&snapshot.owner().settings.packages, false),
        (&snapshot.owner().settings.disabled_system_packages, true),
    ] {
        for s in settings {
            let stored = if factory {
                snapshot.owner().disabled_user_states(&s.name)
            } else {
                snapshot.owner().scanned_user_states(&s.name)
            }
            .unwrap();
            let ids: BTreeSet<_> = stored.keys().copied().chain([0, 10]).collect();
            packages.insert(
                (s.name.clone(), factory),
                PackageInputs {
                    app_id: s.app_id,
                    path: s.code_path.clone(),
                    version: s.version_code,
                    installed_permissions: vec![],
                    domain_verification: None,
                    uri_relative_filter_groups: vec![],
                    filter_application_query: true,
                    syncable_authorities: vec![],
                    users: ids
                        .into_iter()
                        .map(|id| {
                            (
                                id,
                                UserInputs {
                                    gids: vec![],
                                    granted_permissions: vec![],
                                    domain_selection: None,
                                },
                            )
                        })
                        .collect(),
                },
            );
        }
    }
    let mut retained_packages = BTreeMap::new();
    let identities = &snapshot.owner().identities;
    let detached = identities.ids.owners().filter_map(|(id, _)| {
        identities
            .ids
            .detached_setting(id)
            .map(|setting| (id, setting))
    });
    let shared = identities.shared_users.values().flat_map(|group| {
        group
            .retained_settings()
            .map(|(_, setting)| (group.app_id, setting))
    });
    for (id, old) in detached.chain(shared) {
        let setting = &old.package;
        retained_packages.insert(
            (id, setting.name.clone()),
            PackageInputs {
                app_id: setting.app_id,
                path: setting.code_path.clone(),
                version: setting.version_code,
                installed_permissions: vec![],
                domain_verification: None,
                uri_relative_filter_groups: vec![],
                filter_application_query: true,
                syncable_authorities: vec![],
                users: old
                    .users
                    .keys()
                    .copied()
                    .chain([0, 10])
                    .map(|id| {
                        (
                            id,
                            UserInputs {
                                gids: vec![],
                                granted_permissions: vec![],
                                domain_selection: None,
                            },
                        )
                    })
                    .collect(),
            },
        );
    }
    let capture = Capture::new(
        snapshot.clone(),
        Context {
            scan_version: snapshot.version(),
            nonce: None,
            system: model::System {
                sdk_sandbox_package: Some(None),
                ..Default::default()
            },
            platform: model::Platform::default(),
            users: [0, 10]
                .into_iter()
                .map(|id| {
                    (
                        id,
                        model::User {
                            id,
                            ..Default::default()
                        },
                    )
                })
                .collect(),
            apex_inventory: ApexInventory {
                packages: Some(vec![]),
                active: vec![],
            },
            scan_users: ScanUsers {
                users: Some(
                    [0, 10]
                        .into_iter()
                        .map(|id| scan::User {
                            id,
                            pre_created: false,
                            adb_install_disallowed: false,
                        })
                        .collect(),
                ),
            },
            cross_user_suspensions: true,
            packages,
            retained_packages,
        },
    )
    .unwrap();
    for (packages, scope) in [
        (&capture.state().packages, "active"),
        (&capture.state().disabled_system_packages, "factory"),
    ] {
        for s in packages.values() {
            let root = directory.join(scope).join(&s.name);
            let mut p = Parcel::new();
            p.write_string16(Some(&s.name));
            p.write_i32(s.app_id);
            p.write_string16(Some(&s.path));
            p.write_string16(s.volume_uuid.as_deref());
            p.write_string16(s.primary_cpu_abi.as_deref());
            p.write_string16(s.secondary_cpu_abi.as_deref());
            p.write_string16(s.cpu_abi_override.as_deref());
            p.write_string16(s.seinfo.as_deref());
            p.write_string16(s.apex_module_name.as_deref());
            p.write_i64(s.version_code);
            p.write_i32(s.target_sdk_version);
            p.write_i32(s.category_override);
            p.write_i32(s.hidden_api_enforcement_policy);
            p.write_i64(s.last_modified_time);
            p.write_i64(s.last_update_time);
            aim_service_aidl::write_byte_array(&mut p, s.restrict_update_hash.as_deref());
            let f = s.is;
            for value in [
                f.system,
                f.privileged,
                f.oem,
                f.vendor,
                f.product,
                f.system_ext,
                f.odm,
                f.updated_system_app,
                f.apex,
                f.apk_in_updated_apex,
                f.hidden_until_installed,
                f.default_to_device_protected_storage,
                f.force_queryable_override,
                f.scanned_as_stopped_system_app,
                f.update_available,
                f.install_permissions_fixed,
                f.pending_restore,
                f.debuggable,
                f.loading,
            ] {
                p.write_bool(value);
            }
            let signing = s
                .signatures
                .as_ref()
                .map(aim_services::package::sign::SigningDetails::from_saved)
                .transpose()
                .unwrap()
                .map(|signing| signing.package_details().unwrap())
                .flatten()
                .map(|dto| aim_services::package::info::SigningInfo {
                    scheme_version: dto.scheme_version,
                    signatures: dto.signatures.unwrap(),
                    public_keys: dto.public_keys,
                    past_signing_certificates: dto.past_signing_certificates,
                });
            aim_services::package::info::write_signing_details(&mut p, signing.as_ref());
            fs::write(root.join("query-package"), p.data()).unwrap();
            fs::write(
                root.join("query-users"),
                s.users
                    .keys()
                    .map(i32::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .unwrap();
            for (id, u) in &s.users {
                let mut p = Parcel::new();
                p.write_i64(u.ce_data_inode);
                p.write_i64(u.de_data_inode);
                for value in [
                    u.installed,
                    u.stopped,
                    u.not_launched,
                    u.hidden,
                    u.instant_app,
                    u.virtual_preload,
                    u.quarantined,
                    u.data_exists,
                    !u.suspended_by.is_empty(),
                ] {
                    p.write_bool(value);
                }
                p.write_i32(u.distraction_flags);
                p.write_i32(u.enabled);
                p.write_string16(u.last_disable_app_caller.as_deref());
                for values in [&u.enabled_components, &u.disabled_components] {
                    let mut values = values.clone();
                    values.sort();
                    p.write_i32(values.len() as i32);
                    for v in values {
                        p.write_string16(Some(&v));
                    }
                }
                p.write_i32(u.install_reason);
                p.write_i32(u.uninstall_reason);
                p.write_string16(u.harmful_app_warning.as_deref());
                p.write_string16(u.splash_screen_theme.as_deref());
                p.write_i64(u.first_install_time);
                p.write_i32(u.min_aspect_ratio);
                p.write_bool(u.overlay_paths.is_some());
                if let Some(paths) = &u.overlay_paths {
                    for values in [&paths.resource_dirs, &paths.overlay_paths] {
                        p.write_i32(values.len() as i32);
                        for v in values {
                            p.write_string16(Some(v));
                        }
                    }
                }
                fs::write(root.join(format!("query-user-{id}")), p.data()).unwrap();
            }
        }
    }
}
