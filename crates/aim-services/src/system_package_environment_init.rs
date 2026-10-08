//! Complete owned live-install environment assembly from actual boot inputs.
//! Include as System child module; mandatory callbacks come from real owners.
use super::*;
use crate::package::{
    bootstrap::Bridge,
    installer::{
        app_data, environment, environment_image, environment_producers, permission_prepare,
    },
    system_config::Properties,
    write::Apks,
};
use std::path::Path;
pub struct EnvironmentBootInputs {
    pub factory_test: bool,
    pub parser_cache: Option<std::path::PathBuf>,
}
impl System {
    pub fn construct_package_install_environment(
        self: &Arc<Self>,
        bridge: &Arc<Bridge>,
        image: &Path,
        data: &Path,
        apks: Arc<Apks>,
        properties: Properties,
        boot: EnvironmentBootInputs,
    ) -> Result<environment::Config> {
        self.check_package_bootstrap(bridge)?;
        let (snapshots, installer, capture) = {
            let state = self.package_bootstrap.lock().unwrap();
            let current = state
                .current
                .as_ref()
                .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
                .ok_or_else(|| environment_error("environment bootstrap unavailable"))?;
            (
                current
                    .snapshots
                    .clone()
                    .ok_or_else(|| environment_error("environment native scan unavailable"))?,
                current
                    .installer
                    .as_ref()
                    .map(|(owner, _)| owner.clone())
                    .ok_or_else(|| environment_error("environment native installer unavailable"))?,
                current
                    .queries
                    .clone()
                    .ok_or_else(|| environment_error("environment native capture unavailable"))?,
            )
        };
        let users = capture
            .state()
            .scan_users
            .as_ref()
            .and_then(|owner| owner.users.clone())
            .ok_or_else(|| environment_error("environment actual scan users unavailable"))?;
        let cross_user_suspensions = capture.context().cross_user_suspensions;
        let weak = Arc::downgrade(self);
        let retained = bridge.clone();
        let attachment: environment_image::Attachment = Arc::new(move || {
            weak.upgrade()
                .ok_or("environment system stopped")?
                .check_package_bootstrap(&retained)
                .map_err(|error| error.message)
        });
        let image_inputs = environment_image::ImageInputs::load(
            image,
            data,
            apks.clone(),
            &properties,
            bridge.clone(),
            attachment.clone(),
        )
        .map_err(environment_error)?;
        let external = self.package_installer_external(bridge)?;
        let guest = external.native_install_environment()?;
        let compatibility = Arc::new(
            bridge
                .library_compatibility(installer.system_config(), properties.as_ref())
                .map_err(|error| {
                    environment_error(format!("environment library compatibility {error:?}"))
                })?,
        );
        let vendor_sdk = properties("ro.vendor.api_level")
            .ok_or_else(|| environment_error("vendor API level property unavailable"))?
            .parse()
            .map_err(|_| environment_error("vendor API level property invalid"))?;
        let build_debuggable = properties("ro.debuggable")
            .ok_or_else(|| environment_error("debuggable image property unavailable"))?
            == "1";
        let timezone_external = external.clone();
        let timezone = Arc::new(move |year, month, day, hour, minute, second| {
            timezone_external
                .zip_local_utc_offset(year, month, day, hour, minute, second)
                .map_err(|error| error.message)
        });
        let label_bridge = bridge.clone();
        let label_guard = attachment.clone();
        let label_data = std::fs::canonicalize(data)
            .map_err(|error| environment_error(format!("installer label data owner: {error}")))?;
        let labeler = Arc::new(move |path: &Path| {
            label_guard()?;
            let path = installer_guest_context_path(&label_data,path)?;
            label_bridge
                .restore_installer_context(&path)
                .map_err(|error| format!("actual install label {error:?}"))?;
            label_guard()
        });
        let app = app_data::Owner::new(
            self.environment_bridge_capability(bridge, EnvironmentLeaf::AppData)?,
        );
        let (create, flags, rollback, commit) = app.callbacks();
        let weak = Arc::downgrade(self);
        let retained = bridge.clone();
        let current = Arc::new(move || {
            let system = weak.upgrade().ok_or("install context system stopped")?;
            system
                .check_package_bootstrap(&retained)
                .map_err(|error| error.message)?;
            system
                .capture_package_queries()
                .map_err(|error| error.message)
        });
        let contexts = crate::package::scan_snapshot::install_context::Builder::new(
            current,
            bridge.clone(),
            Arc::new(installer.system_config().clone()),
        );
        let permission = permission_prepare::Owner::new(
            self.environment_bridge_capability(bridge, EnvironmentLeaf::Permissions)?,
            self.process.clone(),
            snapshots.clone(),
            cross_user_suspensions,
            contexts.permission_source(),
        );
        let library_bridge = bridge.clone();
        permission.configure_library_policy(Arc::new(move |name, package| {
            library_bridge.library_policy(name, package.target_sdk_version)
                .map_err(|error| format!("installer shared-library policy owner: {error:?}"))
        }))?;
        let metadata = environment_producers::metadata(apks.clone());
        let publish = self.package_install_query_publication(
            bridge,
            snapshots.clone(),
            contexts.publication_source(),
        )?;
        let completion = self.package_install_completion_owner(bridge)?;
        let code_resources = environment_producers::code_resources(
            self.clone(),
            data.to_path_buf(),
            boot.parser_cache,
        )
        .map_err(environment_error)?;
        let mut config = image_inputs.configuration(environment_image::Services {
            snapshots,
            users,
            build_debuggable,
            cross_user_suspensions,
            factory_test: boot.factory_test,
            library_compatibility: compatibility,
            vendor_sdk,
            remove_test_base: {
                let bridge = bridge.clone();
                Arc::new(move |package, system| {
                    bridge
                        .remove_test_base(package, system)
                        .map_err(|error| format!("test.base owner {error:?}"))
                })
            },
            app_data_flags: flags,
            labeler,
            metadata,
            install_source: environment_producers::install_source(guest.headless_system_user),
            library_policy: environment_producers::library_policy(guest.library)
                .map_err(environment_error)?,
            zip_clock: environment_producers::zip_clock(timezone),
            app_data: create,
            rollback_app_data: rollback,
            commit_app_data: commit,
            permissions: permission.runtime_prepare(),
            release_permissions: permission.release(),
            effects: self.package_effects_owner(bridge)?,
            code_resources,
            publish,
            completion,
        });
        // This binds actual data/image producers; it does not install placeholder
        // app-data/permission/publication callbacks before a later overwrite.
        config = environment_producers::configure_guest_sources(
            config,
            external,
            bridge.clone(),
            installer.system_config(),
            properties.as_ref(),
        )
        .map_err(environment_error)?;
        config.metadata = permission.metadata(config.metadata.clone());
        self.check_package_bootstrap(bridge)?;
        Ok(config)
    }
    fn environment_bridge_capability(
        &self,
        bridge: &Arc<Bridge>,
        leaf: EnvironmentLeaf,
    ) -> Result<Strong> {
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as api;
        self.check_package_bootstrap(bridge)?;
        let mut request = Parcel::new();
        let code = match leaf {
            EnvironmentLeaf::AppData => {
                api::GetPackageAppDataBridge {}.write(&mut request);
                api::GET_PACKAGE_APP_DATA_BRIDGE
            }
            EnvironmentLeaf::Permissions => {
                api::GetInstallerPermissionBridge {}.write(&mut request);
                api::GET_INSTALLER_PERMISSION_BRIDGE
            }
        };
        let reply = bridge
            .owner
            .transact(code, &request, false)
            .map_err(|code| environment_error(format!("environment leaf transport {code}")))?;
        let mut reader = reply.reader();
        reader
            .read_exception()
            .map_err(|code| environment_error(format!("environment leaf exception {code}")))??;
        let binder = reader
            .read_binder()
            .map_err(|code| environment_error(format!("environment leaf binder {code}")))?
            .ok_or_else(|| environment_error("environment leaf missing"))?;
        if reader.remaining() != 0 {
            return Err(environment_error("environment leaf trailing bytes"));
        }
        let strong = reply
            .retain_remote_binder(binder)
            .map_err(|code| environment_error(format!("environment leaf lifetime {code}")))?;
        self.check_package_bootstrap(bridge)?;
        Ok(strong)
    }
}

/// Native copy/extraction callbacks hold host paths; the original SELinux owner
/// operates in the guest VFS. Preserve the path below this exact writable root.
fn installer_guest_context_path(data:&Path,path:&Path)->std::result::Result<String,String> {
    if !data.is_absolute()||!path.is_absolute(){return Err("installer label path owner is not absolute".into());}
    let text=path.to_str().ok_or("installer context path is not UTF-8")?;
    if text.contains('\0')||text.contains("/./")||text.ends_with("/.")
        ||path.components().any(|component|matches!(component,std::path::Component::ParentDir)) {
        return Err("installer label path has invalid components".into());
    }
    let suffix=path.strip_prefix(data).map_err(|_|"installer label path is outside writable data owner")?;
    let suffix=suffix.to_str().ok_or("installer context path is not UTF-8")?;
    if suffix.is_empty(){return Err("installer label path is the data root".into());}
    Ok(format!("/data/{suffix}"))
}

#[cfg(test)]
mod label_tests {
    #[test]
    fn installer_context_maps_reserved_host_paths_to_guest_namespace() {
        let root=std::env::temp_dir().join(format!("aim-installer-context-{}-{}",std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(root.join("app/pkg-reserved/lib/arm64")).unwrap();
        std::fs::write(root.join("app/pkg-reserved/base.apk"),b"owned install code").unwrap();
        let data=std::fs::canonicalize(&root).unwrap();
        for relative in ["app/pkg-reserved","app/pkg-reserved/base.apk","app/pkg-reserved/lib/arm64"] {
            assert_eq!(super::installer_guest_context_path(&data,&data.join(relative)).unwrap(),format!("/data/{relative}"));
        }
        assert!(super::installer_guest_context_path(&data,&data.with_extension("sibling").join("app/pkg/base.apk")).is_err());
        assert!(super::installer_guest_context_path(&data,&data.join("app/../system/install_sessions.xml")).is_err());
        assert!(super::installer_guest_context_path(&data,&data.join("app/./pkg/base.apk")).is_err());
        assert!(super::installer_guest_context_path(&data,&data).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
enum EnvironmentLeaf {
    AppData,
    Permissions,
}
fn environment_error(message: impl Into<String>) -> Exception {
    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message)
}

impl System {
    pub fn package_install_completion_owner(
        self: &Arc<Self>,
        bridge: &Arc<Bridge>,
    ) -> Result<environment::Completion> {
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bootstrap;
        self.check_package_bootstrap(bridge)?;
        let mut request = Parcel::new();
        bootstrap::GetInstallerCompletionBridge {}.write(&mut request);
        let reply = bridge
            .owner
            .transact(bootstrap::GET_INSTALLER_COMPLETION_BRIDGE, &request, false)
            .map_err(|code| environment_error(format!("completion bridge transport {code}")))?;
        let mut reader = reply.reader();
        let binder = bootstrap::read_get_installer_completion_bridge_reply(&mut reader)
            .map_err(|code| environment_error(format!("completion bridge reply {code}")))??
            .ok_or_else(|| environment_error("actual ART completion owner unavailable"))?;
        if reader.remaining() != 0 {
            return Err(environment_error("completion bridge trailing data"));
        }
        let art =
            Arc::new(reply.retain_remote_binder(binder).map_err(|code| {
                environment_error(format!("completion bridge retention {code}"))
            })?);
        let weak = Arc::downgrade(self);
        let retained = bridge.clone();
        Ok(Arc::new(move |receipt| {
            let system = weak
                .upgrade()
                .ok_or_else(|| environment_error("install completion system stopped"))?;
            system.check_package_bootstrap(&retained)?;
            let capture = system.capture_package_queries()?;
            if capture.scan().version() < receipt.generation {
                return Err(environment_error(
                    "install completion generation unpublished",
                ));
            }
            system.reconcile_original_package_domains(&retained)?;
            for code in receipt.verified_sessions {
                use crate::package::pkg::{booleans, booleans2};
                let name = if code.package.static_shared_library_name.is_some() {
                    format!(
                        "{}_{}",
                        code.package.package_name, code.package.static_shared_lib_version
                    )
                } else {
                    code.package.package_name.clone()
                };
                if !receipt.packages.iter().any(|package| {
                    package.name == name
                        && package.version_code
                            == ((i64::from(code.package.version_code_major) << 32)
                                | (i64::from(code.package.version_code) & 0xffff_ffff))
                }) {
                    return Err(environment_error(
                        "verified session has no matching durable package receipt",
                    ));
                }
                let rollback = code.record.params.install_flags & 0x40000 != 0
                    && code.record.initiating_package.as_deref() == Some("android");
                let mut request = Parcel::new();
                use aim_service_aidl::dev_aim_server_iinstallercompletionbridge as api;
                api::DexoptInstalled {
                    package_name: Some(name.clone()),
                    install_scenario: code.record.params.install_scenario,
                    install_reason: code.record.params.install_reason,
                    install_flags: code.record.params.install_flags,
                    compiler_filter: code.record.params.dexopt_compiler_filter.clone(),
                    debuggable: code.package.is(booleans::DEBUGGABLE),
                    instant_app: code.record.params.install_flags & 0x800 != 0,
                    apex: code.package.is2(booleans2::APEX),
                    rollback_from_platform: rollback,
                }
                .write(&mut request);
                let reply = art
                    .transact(api::DEXOPT_INSTALLED, &request, false)
                    .map_err(|code| {
                        environment_error(format!("ART completion transport {code}"))
                    })?;
                let mut reader = reply.reader();
                let status = api::read_dexopt_installed_reply(&mut reader)
                    .map_err(|code| environment_error(format!("ART completion reply {code}")))??;
                if reader.remaining() != 0 {
                    return Err(environment_error("ART completion trailing reply"));
                }
                // Dexopt failure does not invalidate a committed install. Preserve
                // the original ART result diagnostically without inventing success.
                if status == 30 {
                    eprintln!("native install ART dexopt failed for {name}");
                }
                for installed in receipt
                    .packages
                    .iter()
                    .filter(|package| package.name == name)
                {
                    let user = installed.user as i32;
                    let owner = system.existing_package_owner()?;
                    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
                    let complete_bridge = retained.clone();
                    let complete_name = name.clone();
                    let complete_system = Arc::downgrade(&system);
                    let token = owner.restores.register(Box::new(move |_| {
                        let result = (|| {
                            let system = complete_system.upgrade().ok_or_else(|| {
                                environment_error("restore completion system stopped")
                            })?;
                            system.check_package_bootstrap(&complete_bridge)?;
                            complete_bridge
                                .complete_existing_install(
                                    Some(&complete_name),
                                    user,
                                    None,
                                    1,
                                    true,
                                )
                                .map_err(|error| {
                                    environment_error(format!("post-restore permissions {error:?}"))
                                })?;
                            system.restore_existing_install_preferences(&complete_name, user)?;
                            Ok(())
                        })();
                        let _ = sender.send(result);
                        Ok(())
                    }))?;
                    let needs_restore = receipt
                        .new_installations
                        .contains(&(name.clone(), installed.user))
                        && code.package.is(booleans::ALLOW_BACKUP)
                        && code.record.params.install_flags
                            & (0x800 | crate::package::installer::archiver::ARCHIVED)
                            == 0;
                    let accepted = if needs_restore {
                        match retained.restore_existing_install(&name, user, token) {
                            Ok(accepted) => accepted,
                            Err(error) => {
                                owner.restores.cancel(token);
                                return Err(environment_error(format!("original backup restore {error:?}")));
                            }
                        }
                    } else {
                        false
                    };
                    if !accepted {
                        owner.restores.finish(token, false)?;
                    }
                    receiver.recv().map_err(|_| {
                        environment_error("backup restore owner closed before completion")
                    })??;
                }
            }
            system.check_package_bootstrap(&retained)
        }))
    }
}
