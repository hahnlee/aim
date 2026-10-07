//! Additional snapshot queries from the pinned ComputerEngine and PMS owner.
use super::*;

struct VersionedPackage {
    name: Option<String>,
    version: i64,
}
impl ReadParcelable for VersionedPackage {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Self {
            name: r.read_string8()?,
            version: r.read_i64()?,
        })
    }
}

impl Query<'_> {
    pub(super) fn extra_package(&self, code: u32, r: &mut Reader<'_>) -> Answered {
        match code {
            pm::GET_INSTRUMENTATION_INFO_AS_USER => {
                let a = args(pm::GetInstrumentationInfoAsUser::<ComponentName>::read(r))?;
                thrown(
                    self.instrumentation_info(a.class_name.as_ref(), i64::from(a.flags), a.user_id),
                    |p, v| pm::write_get_instrumentation_info_as_user_reply(p, v.as_ref()),
                )
            }
            pm::QUERY_INSTRUMENTATION_AS_USER => {
                let a = args(pm::QueryInstrumentationAsUser::read(r))?;
                thrown(
                    self.instrumentations(
                        a.target_package.as_deref(),
                        i64::from(a.flags),
                        a.user_id,
                    ),
                    |p, v| {
                        pm::write_query_instrumentation_as_user_reply(
                            p,
                            Some(&ListSlice {
                                creator: "android.content.pm.InstrumentationInfo".into(),
                                items: v,
                            }),
                        )
                    },
                )
            }
            pm::GET_INSTALLED_MODULES => {
                let a = args(pm::GetInstalledModules::read(r))?;
                thrown(self.installed_modules(a.flags), |p, v| {
                    pm::write_get_installed_modules_reply(p, Some(&v))
                })
            }
            pm::GET_MODULE_INFO => {
                let a = args(pm::GetModuleInfo::read(r))?;
                thrown(
                    self.module_info(a.package_name.as_deref(), a.flags),
                    |p, v| pm::write_get_module_info_reply(p, v.as_ref()),
                )
            }
            pm::GET_APP_METADATA_SOURCE => {
                let a = args(pm::GetAppMetadataSource::read(r))?;
                thrown(
                    self.app_metadata_source(a.package_name.as_deref(), a.user_id),
                    pm::write_get_app_metadata_source_reply,
                )
            }
            pm::GET_PERMISSION_CONTROLLER_PACKAGE_NAME => {
                args(pm::GetPermissionControllerPackageName::read(r))?;
                thrown(self.permission_controller(), |p, v| {
                    pm::write_get_permission_controller_package_name_reply(p, &Some(v))
                })
            }
            pm::GET_SHARED_LIBRARIES => {
                let a = args(pm::GetSharedLibraries::read(r))?;
                thrown(
                    self.shared_libraries(a.package_name.as_deref(), a.flags, a.user_id),
                    |p, v| pm::write_get_shared_libraries_reply(p, v.as_ref()),
                )
            }
            pm::GET_SYSTEM_SHARED_LIBRARY_NAMES_AND_PATHS => {
                args(pm::GetSystemSharedLibraryNamesAndPaths::read(r))?;
                let entries = self.system_library_paths()?;
                Ok(reply(|p| {
                    pm::write_get_system_shared_library_names_and_paths_reply(p, &Some(entries))
                }))
            }
            pm::GET_SDK_SANDBOX_PACKAGE_NAME => {
                args(pm::GetSdkSandboxPackageName::read(r))?;
                let name = self
                    .state
                    .system
                    .sdk_sandbox_package
                    .as_ref()
                    .ok_or(NotModelled("the SDK sandbox package selection"))?;
                Ok(reply(|p| {
                    pm::write_get_sdk_sandbox_package_name_reply(p, name)
                }))
            }
            pm::GET_ALL_APEX_DIRECTORIES => {
                args(pm::GetAllApexDirectories::read(r))?;
                if !matches!(self.calling_uid, 0 | 1000) {
                    return thrown(
                        Ok(Err(Exception::security(
                            "getAllApexDirectories can only be called by system or root",
                        ))),
                        |_, _: ()| {},
                    );
                }
                let apex = self
                    .state
                    .apex_inventory
                    .as_ref()
                    .ok_or(NotModelled("the active APEX inventory"))?;
                let paths = Some(
                    apex.active
                        .iter()
                        .map(|a| Some(a.mount_path.clone()))
                        .collect(),
                );
                Ok(reply(|p| {
                    pm::write_get_all_apex_directories_reply(p, &paths)
                }))
            }
            pm::GET_INITIAL_NON_STOPPED_SYSTEM_PACKAGES => {
                args(pm::GetInitialNonStoppedSystemPackages::read(r))?;
                let names = self
                    .state
                    .system
                    .initial_non_stopped_system_packages
                    .as_ref()
                    .ok_or(NotModelled("the initial non-stopped system package owner"))?;
                Ok(reply(|p| {
                    pm::write_get_initial_non_stopped_system_packages_reply(
                        p,
                        &Some(names.iter().cloned().map(Some).collect()),
                    )
                }))
            }
            pm::IS_PROTECTED_BROADCAST => {
                let a = args(pm::IsProtectedBroadcast::read(r))?;
                let prefix = a.action_name.as_ref().is_some_and(|name| {
                    [
                        "android.net.netmon.lingerExpired",
                        "com.android.server.sip.SipWakeupTimer",
                        "com.android.internal.telephony.data-reconnect",
                        "android.net.netmon.launchCaptivePortalApp",
                    ]
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
                });
                let protected = if prefix {
                    true
                } else {
                    let known = self
                        .state
                        .protected_broadcasts
                        .as_ref()
                        .ok_or(NotModelled("the registered protected-broadcast owner"))?;
                    a.action_name
                        .as_ref()
                        .is_some_and(|name| known.contains(name))
                };
                Ok(reply(|p| {
                    pm::write_is_protected_broadcast_reply(p, protected)
                }))
            }
            pm::GET_DECLARED_SHARED_LIBRARIES => {
                let a = args(pm::GetDeclaredSharedLibraries::read(r))?;
                thrown(
                    self.declared_libraries(a.package_name.as_deref(), a.flags, a.user_id),
                    |p, v| pm::write_get_declared_shared_libraries_reply(p, v.as_ref()),
                )
            }
            pm::CHECK_PERMISSION => {
                let a = args(pm::CheckPermission::read(r))?;
                let value = self.check_package_permission(
                    a.pkg_name.as_deref(),
                    a.perm_name.as_deref(),
                    a.user_id,
                )?;
                Ok(reply(|p| pm::write_check_permission_reply(p, value)))
            }
            pm::CHECK_UID_PERMISSION => {
                let a = args(pm::CheckUidPermission::read(r))?;
                let value = self.check_uid_permission(a.uid, a.perm_name.as_deref())?;
                Ok(reply(|p| pm::write_check_uid_permission_reply(p, value)))
            }
            pm::GET_APP_OP_PERMISSION_PACKAGES => {
                let a = args(pm::GetAppOpPermissionPackages::read(r))?;
                thrown(
                    self.appop_packages(a.permission_name.as_deref(), a.user_id),
                    |p, v| pm::write_get_app_op_permission_packages_reply(p, &Some(v)),
                )
            }
            pm::GET_SYSTEM_SHARED_LIBRARY_NAMES => {
                args(pm::GetSystemSharedLibraryNames::read(r))?;
                let names = self.system_library_names()?;
                Ok(reply(|p| {
                    pm::write_get_system_shared_library_names_reply(p, &names)
                }))
            }
            pm::IS_INSTANT_APP => {
                let a = args(pm::IsInstantApp::read(r))?;
                thrown(
                    self.is_instant(a.package_name.as_deref().unwrap_or_default(), a.user_id),
                    pm::write_is_instant_app_reply,
                )
            }
            pm::CURRENT_TO_CANONICAL_PACKAGE_NAMES => {
                let a = args(pm::CurrentToCanonicalPackageNames::read(r))?;
                let names = self.translate_names(a.names.as_deref(), false)?;
                Ok(reply(|p| {
                    pm::write_current_to_canonical_package_names_reply(p, &names)
                }))
            }
            pm::CANONICAL_TO_CURRENT_PACKAGE_NAMES => {
                let a = args(pm::CanonicalToCurrentPackageNames::read(r))?;
                let names = self.translate_names(a.names.as_deref(), true)?;
                Ok(reply(|p| {
                    pm::write_canonical_to_current_package_names_reply(p, &names)
                }))
            }
            pm::GET_FLAGS_FOR_UID => {
                let a = args(pm::GetFlagsForUid::read(r))?;
                let value = self.uid_flags(a.uid, false)?;
                Ok(reply(|p| pm::write_get_flags_for_uid_reply(p, value)))
            }
            pm::GET_PRIVATE_FLAGS_FOR_UID => {
                let a = args(pm::GetPrivateFlagsForUid::read(r))?;
                let value = self.uid_flags(a.uid, true)?;
                Ok(reply(|p| {
                    pm::write_get_private_flags_for_uid_reply(p, value)
                }))
            }
            pm::GET_SPLASH_SCREEN_THEME => {
                let a = args(pm::GetSplashScreenTheme::read(r))?;
                let value = match self.enforce_cross_user(
                    a.user_id,
                    false,
                    false,
                    "getSplashScreenTheme",
                )? {
                    Err(e) => {
                        return thrown(Ok(Err(e)), |p, v: Option<String>| {
                            pm::write_get_splash_screen_theme_reply(p, &v)
                        });
                    }
                    Ok(()) => self
                        .visible_user_state(
                            a.package_name.as_deref().unwrap_or_default(),
                            a.user_id,
                        )?
                        .and_then(|u| u.splash_screen_theme),
                };
                Ok(reply(|p| {
                    pm::write_get_splash_screen_theme_reply(p, &value)
                }))
            }
            pm::GET_USER_MIN_ASPECT_RATIO => {
                let a = args(pm::GetUserMinAspectRatio::read(r))?;
                let value = self
                    .visible_user_state(a.package_name.as_deref().unwrap_or_default(), a.user_id)?
                    .map_or(0, |u| u.min_aspect_ratio);
                Ok(reply(|p| {
                    pm::write_get_user_min_aspect_ratio_reply(p, value)
                }))
            }
            pm::GET_APPLICATION_HIDDEN_SETTING_AS_USER => {
                let a = args(pm::GetApplicationHiddenSettingAsUser::read(r))?;
                thrown(
                    self.hidden_setting(a.package_name.as_deref().unwrap_or_default(), a.user_id),
                    pm::write_get_application_hidden_setting_as_user_reply,
                )
            }
            pm::GET_ALL_PACKAGES => {
                args(pm::GetAllPackages::read(r))?;
                thrown(self.all_packages(), |p, v| {
                    pm::write_get_all_packages_reply(p, &Some(v))
                })
            }
            pm::GET_PACKAGE_INFO_VERSIONED => {
                let a = args(pm::GetPackageInfoVersioned::<VersionedPackage>::read(r))?;
                let versioned = a
                    .versioned_package
                    .ok_or(NotModelled("a null versioned package"))?;
                thrown(
                    self.package_info(
                        versioned.name.as_deref().unwrap_or_default(),
                        versioned.version,
                        a.flags,
                        a.user_id,
                    ),
                    |p, v| pm::write_get_package_info_versioned_reply(p, v.as_ref()),
                )
            }
            pm::GET_PACKAGES_HOLDING_PERMISSIONS => {
                let a = args(pm::GetPackagesHoldingPermissions::read(r))?;
                let permissions = a
                    .permissions
                    .ok_or(NotModelled("a null permission array"))?;
                thrown(
                    self.packages_holding_permissions(&permissions, a.flags, a.user_id),
                    |p, v| {
                        pm::write_get_packages_holding_permissions_reply(
                            p,
                            Some(&ListSlice {
                                creator: "android.content.pm.PackageInfo".into(),
                                items: v,
                            }),
                        )
                    },
                )
            }
            pm::IS_PACKAGE_AVAILABLE => {
                let a = args(pm::IsPackageAvailable::read(r))?;
                thrown(
                    self.package_available(
                        a.package_name.as_deref().unwrap_or_default(),
                        a.user_id,
                    ),
                    pm::write_is_package_available_reply,
                )
            }
            pm::GET_PACKAGE_GIDS => {
                let a = args(pm::GetPackageGids::read(r))?;
                thrown(
                    self.package_gids(
                        a.package_name.as_deref().unwrap_or_default(),
                        a.flags,
                        a.user_id,
                    ),
                    |p, v| pm::write_get_package_gids_reply(p, &v),
                )
            }
            pm::GET_TARGET_SDK_VERSION => {
                let a = args(pm::GetTargetSdkVersion::read(r))?;
                let value =
                    self.target_sdk_version(a.package_name.as_deref().unwrap_or_default())?;
                Ok(reply(|p| pm::write_get_target_sdk_version_reply(p, value)))
            }
            pm::GET_UID_FOR_SHARED_USER => {
                let a = args(pm::GetUidForSharedUser::read(r))?;
                let value = self.shared_uid(a.shared_user_name.as_deref())?;
                Ok(reply(|p| pm::write_get_uid_for_shared_user_reply(p, value)))
            }
            pm::IS_UID_PRIVILEGED => {
                let a = args(pm::IsUidPrivileged::read(r))?;
                let value = self.uid_privileged(a.uid)?;
                Ok(reply(|p| pm::write_is_uid_privileged_reply(p, value)))
            }
            pm::GET_INSTALLER_PACKAGE_NAME => {
                let a = args(pm::GetInstallerPackageName::read(r))?;
                thrown(
                    self.installer_package(a.package_name.as_deref().unwrap_or_default()),
                    |p, v| pm::write_get_installer_package_name_reply(p, &v),
                )
            }
            pm::GET_INSTALL_REASON => {
                let a = args(pm::GetInstallReason::read(r))?;
                thrown(
                    self.install_reason(a.package_name.as_deref().unwrap_or_default(), a.user_id),
                    pm::write_get_install_reason_reply,
                )
            }
            pm::GET_MIME_GROUP => {
                let a = args(pm::GetMimeGroup::read(r))?;
                thrown(
                    self.mime_group(
                        a.package_name.as_deref().unwrap_or_default(),
                        a.group.as_deref(),
                    ),
                    |p, v| pm::write_get_mime_group_reply(p, &Some(v)),
                )
            }
            pm::HAS_SIGNING_CERTIFICATE => {
                let a = args(pm::HasSigningCertificate::read(r))?;
                let value = self.package_certificate(
                    a.package_name.as_deref().unwrap_or_default(),
                    a.signing_certificate.as_deref(),
                    a.flags,
                )?;
                Ok(reply(|p| pm::write_has_signing_certificate_reply(p, value)))
            }
            pm::HAS_UID_SIGNING_CERTIFICATE => {
                let a = args(pm::HasUidSigningCertificate::read(r))?;
                let details = self.uid_signatures(a.uid)?;
                let value = details
                    .as_ref()
                    .is_some_and(|s| certificate(s, a.signing_certificate.as_deref(), a.flags));
                Ok(reply(|p| {
                    pm::write_has_uid_signing_certificate_reply(p, value)
                }))
            }
            pm::CHECK_UID_SIGNATURES => {
                let a = args(pm::CheckUidSignatures::read(r))?;
                let (s1, s2) = (self.uid_signatures(a.uid1)?, self.uid_signatures(a.uid2)?);
                let value = match (s1, s2) {
                    (Some(s1), Some(s2)) => check_signatures(
                        &AndroidPackage {
                            signing_details: Some(s1),
                            ..Default::default()
                        },
                        &AndroidPackage {
                            signing_details: Some(s2),
                            ..Default::default()
                        },
                    ),
                    _ => SIGNATURE_UNKNOWN_PACKAGE,
                };
                Ok(reply(|p| pm::write_check_uid_signatures_reply(p, value)))
            }
            _ => Err(NotModelled("a method not modelled")),
        }
    }

    pub(super) fn instrumentation_info(
        &self,
        component: Option<&ComponentName>,
        flags: i64,
        user: i32,
    ) -> Thrown<Option<Instrumentation>> {
        if let Err(e) =
            self.enforce_cross_user(user, false, false, "getInstrumentationInfoAsUser")?
        {
            return Ok(Err(e));
        }
        if self.user(user).is_none() {
            return Ok(Ok(None));
        }
        let Some(component) = component else {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "null instrumentation component",
            )));
        };
        let Some((ps, pkg)) = self.package_of(&component.package) else {
            return Ok(Ok(None));
        };
        let instrument = pkg.instrumentations.iter().rev().find(|i| {
            i.component.name == component.class && i.component.package_name == component.package
        });
        if let Some(instrument) = instrument {
            let caller = if apps_filter::is_isolated(self.calling_uid) {
                self.state
                    .system
                    .isolated_owners
                    .iter()
                    .find(|(uid, _)| *uid == self.calling_uid)
                    .map(|(_, owner)| *owner)
                    .ok_or(NotModelled("an isolated instrumentation caller's owner"))?
            } else {
                self.calling_uid
            };
            let instant = apps_filter::instant_app_package_name(self.state, caller)?.is_some();
            let target_visible = instant
                && !user_state(ps, user).instant_app
                && apps_filter::is_caller_same_app(
                    self.state,
                    instrument.target_package.as_deref(),
                    caller,
                )?;
            if !target_visible && self.filtered(Some(ps), self.calling_uid, user)? {
                return Ok(Ok(None));
            }
            let state = user_state(ps, user);
            return Ok(Ok(info::generate_instrumentation_info(
                &self.target(ps, pkg, &state, user),
                instrument,
                flags,
            )
            .map(Instrumentation)));
        }
        if self.filtered(Some(ps), self.calling_uid, user)? {
            return Ok(Ok(None));
        }
        Ok(Ok(None))
    }

    pub(super) fn instrumentations(
        &self,
        target: Option<&str>,
        flags: i64,
        user: i32,
    ) -> Thrown<Vec<Instrumentation>> {
        if let Err(e) = self.enforce_cross_user(user, false, false, "queryInstrumentationAsUser")? {
            return Ok(Err(e));
        }
        if self.user(user).is_none() {
            return Ok(Ok(Vec::new()));
        }
        let mut registered: Vec<(
            &PackageState,
            &AndroidPackage,
            &super::super::pkg::Instrumentation,
        )> = Vec::new();
        for ps in self.state.packages.values() {
            let Some(pkg) = ps.pkg.as_deref() else {
                continue;
            };
            for instrument in &pkg.instrumentations {
                if let Some(index) = registered.iter().position(|(_, _, i)| {
                    i.component.package_name == instrument.component.package_name
                        && i.component.name == instrument.component.name
                }) {
                    registered[index] = (ps, pkg, instrument);
                } else {
                    registered.push((ps, pkg, instrument));
                }
            }
        }
        let hash = |i: &super::super::pkg::Instrumentation| {
            info::java_hash(&i.component.package_name)
                .wrapping_add(info::java_hash(&i.component.name))
        };
        registered.sort_by_key(|(_, _, i)| hash(i));
        if registered
            .windows(2)
            .any(|pair| hash(pair[0].2) == hash(pair[1].2))
        {
            return Err(NotModelled(
                "instrumentation registry collision insertion order",
            ));
        }
        let mut result = Vec::new();
        for (ps, pkg, instrument) in registered {
            if target.is_some() && target != instrument.target_package.as_deref() {
                continue;
            }
            if self.filtered(Some(ps), self.calling_uid, user)? {
                continue;
            }
            let state = user_state(ps, user);
            if let Some(info) = info::generate_instrumentation_info(
                &self.target(ps, pkg, &state, user),
                instrument,
                flags,
            ) {
                result.push(Instrumentation(info));
            }
        }
        Ok(Ok(result))
    }

    pub(super) fn module_metadata_package(&self) -> Thrown<Option<String>> {
        let owner = self
            .state
            .system
            .module_metadata
            .as_ref()
            .ok_or(NotModelled("native module metadata owner unavailable"))?;
        if !owner.loaded() {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "Call to getVersion before metadata loaded",
            )));
        }
        Ok(Ok(owner.provider().map(str::to_owned)))
    }

    fn installed_modules(
        &self,
        flags: i32,
    ) -> Thrown<Vec<Option<super::super::module_metadata::ModuleInfo>>> {
        let owner = self
            .state
            .system
            .module_metadata
            .as_ref()
            .ok_or(NotModelled("native module metadata owner unavailable"))?;
        if !owner.loaded() {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "Call to getInstalledModules before metadata loaded",
            )));
        }
        if flags & 0x20000 != 0 {
            return Ok(Ok(owner.modules().iter().cloned().map(Some).collect()));
        }
        let packages = match self
            .installed_packages(i64::from(flags) | MATCH_APEX, user_id(self.calling_uid))?
        {
            Ok(p) => p,
            Err(e) => return Ok(Err(e)),
        };
        Ok(Ok(packages
            .iter()
            .filter_map(|p| {
                owner
                    .modules()
                    .iter()
                    .find(|m| m.package == p.package_name)
                    .cloned()
                    .map(Some)
            })
            .collect()))
    }

    fn module_info(
        &self,
        name: Option<&str>,
        flags: i32,
    ) -> Thrown<Option<super::super::module_metadata::ModuleInfo>> {
        let owner = self
            .state
            .system
            .module_metadata
            .as_ref()
            .ok_or(NotModelled("native module metadata owner unavailable"))?;
        if !owner.loaded() {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "Call to getModuleInfo before metadata loaded",
            )));
        }
        if flags & 1 != 0 {
            if name.is_none() && !owner.modules().is_empty() {
                return Ok(Err(Exception::new(
                    aim_binder_host::parcel::EX_NULL_POINTER,
                    "null APEX module name",
                )));
            }
            Ok(Ok(owner
                .modules()
                .iter()
                .find(|m| m.apex.as_deref() == name)
                .cloned()))
        } else {
            Ok(Ok(owner
                .modules()
                .iter()
                .find(|m| m.package.as_deref() == name)
                .cloned()))
        }
    }

    fn app_metadata_source(&self, name: Option<&str>, user: i32) -> Thrown<i32> {
        if !matches!(self.calling_uid, 0 | 1000)
            && !self.uid_has_permission(self.calling_uid, "android.permission.GET_APP_METADATA")?
        {
            return Ok(Err(Exception::security(
                "GET_APP_METADATA permission required",
            )));
        }
        let ps = name.and_then(|name| self.state.packages.get(name));
        if ps.is_none() || self.filtered_including_uninstalled(ps, user)? {
            let mut payload = Parcel::new();
            payload.write_string16(Some("android.os.ParcelableException"));
            payload.write_string16(Some(
                "android.content.pm.PackageManager$NameNotFoundException",
            ));
            payload.write_string16(name);
            let message = match name {
                Some(name) => {
                    format!("android.content.pm.PackageManager$NameNotFoundException: {name}")
                }
                None => "android.content.pm.PackageManager$NameNotFoundException".into(),
            };
            return Ok(Err(Exception::parcelable(Some(&message), &payload)
                .map_err(|_| {
                    NotModelled("metadata exception payload requires capability ownership")
                })?));
        }
        Ok(Ok(ps.unwrap().app_metadata_source.ok_or(NotModelled(
            "the PackageSetting app metadata source",
        ))?))
    }

    fn permission_controller(&self) -> Thrown<String> {
        let selected = self
            .state
            .system
            .permission_controller_package
            .as_ref()
            .ok_or(NotModelled("the native permission controller selection"))?;
        if let Some(package) = selected {
            if self
                .visible_user_state(package, user_id(self.calling_uid))?
                .is_some()
            {
                return Ok(Ok(package.clone()));
            }
        }
        Ok(Err(Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE,
            "PermissionController is not found",
        )))
    }

    fn shared_libraries(
        &self,
        package: Option<&str>,
        flags: i64,
        user: i32,
    ) -> Thrown<Option<ListSlice<Library>>> {
        if self.user(user).is_none()
            || apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some()
        {
            return Ok(Ok(None));
        }
        let flags = match self.update_flags_for_package(flags, user)? {
            Ok(flags) => flags,
            Err(e) => return Ok(Err(e)),
        };
        let privileged = matches!(self.calling_uid, 0 | 1000)
            || self.check_uid_permission(self.calling_uid, Some(INSTALL_PACKAGES))? == 0
            || self.check_uid_permission(
                self.calling_uid,
                Some("android.permission.DELETE_PACKAGES"),
            )? == 0;
        let can_see = if privileged {
            true
        } else {
            match self.request_installs(package, user)? {
                Err(e) => return Ok(Err(e)),
                Ok(requests) => {
                    requests
                        || self.check_uid_permission(
                            self.calling_uid,
                            Some("android.permission.REQUEST_DELETE_PACKAGES"),
                        )? == 0
                        || self.check_uid_permission(
                            self.calling_uid,
                            Some("android.permission.ACCESS_SHARED_LIBRARIES"),
                        )? == 0
                }
            }
        };
        let libraries = self
            .state
            .shared_libraries
            .as_ref()
            .ok_or(NotModelled("the finalized shared library registry"))?;
        let cleared = Query {
            state: self.state,
            filter: self.filter,
            calling_uid: 1000,
        };
        let mut values = Vec::new();
        let mut blocked = HashSet::new();
        for library in libraries {
            if blocked.contains(&library.name) {
                continue;
            }
            if matches!(library.kind, 2 | 3) && !can_see {
                blocked.insert(library.name.clone());
                continue;
            }
            match cleared.package_info(
                &library.declaring.0,
                library.declaring.1,
                flags | MATCH_STATIC_SHARED_AND_SDK_LIBRARIES,
                user,
            )? {
                Err(e) => return Ok(Err(e)),
                Ok(None) => continue,
                Ok(Some(_)) => {}
            }
            let mut value = library.clone();
            value.dependents = self.library_consumers(library, flags, user)?;
            value.dependents_initialized = !value.dependents.is_empty();
            value.optional_dependents = None;
            value.cert_digests = None;
            if library.kind == 3 {
                let independence = self
                    .state
                    .system
                    .flags
                    .iter()
                    .find(|(n, _)| n == "android.content.pm.sdk_lib_independence")
                    .map(|(_, on)| *on)
                    .ok_or(NotModelled("the sdk_lib_independence flag owner"))?;
                if independence {
                    let optional: Vec<_> = value
                        .dependents
                        .iter()
                        .filter(|dependent| {
                            dependent.as_ref().is_some_and(|(name, _)| {
                                self.state.packages.values().any(|ps| {
                                    let external = ps.pkg.as_ref().map_or_else(
                                        || ps.name.clone(),
                                        |p| Self::external_name(p),
                                    );
                                    external == *name
                                        && ps.version_code == dependent.as_ref().unwrap().1
                                        && ps.uses_sdk_libraries.iter().any(|l| {
                                            Some(&l.name) == library.name.as_ref()
                                                && l.version_major == library.version
                                                && l.optional
                                        })
                                })
                            })
                        })
                        .cloned()
                        .collect();
                    value.optional_dependents = (!optional.is_empty()).then_some(optional);
                }
            }
            values.push(Library(value));
        }
        Ok(Ok((!values.is_empty()).then_some(ListSlice {
            creator: "android.content.pm.SharedLibraryInfo".into(),
            items: values,
        })))
    }

    fn request_installs(&self, name: Option<&str>, user: i32) -> Thrown<bool> {
        let name = name.unwrap_or_default();
        let uid = match self.package_uid(name, 0, user)? {
            Ok(uid) => uid,
            Err(e) => return Ok(Err(e)),
        };
        if uid != self.calling_uid && !matches!(self.calling_uid, 0 | 1000) {
            return Ok(Err(Exception::security(format!(
                "Caller uid {} does not own package {name}",
                self.calling_uid
            ))));
        }
        let Some((ps, pkg)) = self.package_of(name) else {
            return Ok(Ok(false));
        };
        if user_state(ps, user).instant_app
            || pkg.target_sdk_version < 26
            || !pkg
                .requested_permissions
                .iter()
                .any(|p| p == "android.permission.REQUEST_INSTALL_PACKAGES")
        {
            return Ok(Ok(false));
        }
        Err(NotModelled(
            "unknown-sources user restrictions and external source policy",
        ))
    }

    fn declared_libraries(
        &self,
        name: Option<&str>,
        flags: i64,
        user: i32,
    ) -> Thrown<Option<ListSlice<Library>>> {
        if !matches!(self.calling_uid, 0 | 1000)
            && self.check_uid_permission(
                self.calling_uid,
                Some("android.permission.ACCESS_SHARED_LIBRARIES"),
            )? != 0
        {
            return Ok(Err(Exception::security("ACCESS_SHARED_LIBRARIES required")));
        }
        if let Err(e) = self.enforce_full_cross_user(user, "getDeclaredSharedLibraries")? {
            return Ok(Err(e));
        }
        let Some(name) = name else {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "packageName cannot be null",
            )));
        };
        if self.user(user).is_none()
            || apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some()
        {
            return Ok(Ok(None));
        }
        let libraries = self
            .state
            .shared_libraries
            .as_ref()
            .ok_or(NotModelled("the finalized shared library registry"))?;
        let cleared = Query {
            state: self.state,
            filter: self.filter,
            calling_uid: 1000,
        };
        let mut values = Vec::new();
        for library in libraries {
            if library.declaring.0 != name {
                continue;
            }
            match cleared.package_info(
                name,
                library.declaring.1,
                flags | MATCH_STATIC_SHARED_AND_SDK_LIBRARIES,
                user,
            )? {
                Err(error) => return Ok(Err(error)),
                Ok(None) => continue,
                Ok(Some(_)) => {}
            }
            let mut value = library.clone();
            value.dependents = self.library_consumers(library, flags, user)?;
            value.dependents_initialized = !value.dependents.is_empty();
            value.optional_dependents = None;
            value.cert_digests = None;
            values.push(Library(value));
        }
        Ok(Ok((!values.is_empty()).then_some(ListSlice {
            creator: "android.content.pm.SharedLibraryInfo".into(),
            items: values,
        })))
    }

    fn library_consumers(
        &self,
        library: &super::super::model::SharedLibrary,
        flags: i64,
        user: i32,
    ) -> Result<Vec<Option<(String, i64)>>, NotModelled> {
        let Some(name) = &library.name else {
            return Err(NotModelled("a shared library without a name"));
        };
        let mut consumers = Vec::new();
        for ps in self.packages_in_order() {
            if !info::is_available(&user_state(ps, user), flags) {
                continue;
            }
            let uses = match library.kind {
                2 => ps
                    .uses_static_libraries
                    .iter()
                    .any(|(n, v)| n == name && *v == library.version),
                3 => ps
                    .uses_sdk_libraries
                    .iter()
                    .any(|l| l.name == *name && l.version_major == library.version),
                _ => ps.pkg.as_ref().is_some_and(|p| {
                    p.uses_libraries.contains(name) || p.uses_optional_libraries.contains(name)
                }),
            };
            if !uses || self.filtered(Some(ps), self.calling_uid, user)? {
                continue;
            }
            let dependent = match ps.pkg.as_ref() {
                Some(p) if p.static_shared_library_name.is_some() => p
                    .manifest_package_name
                    .clone()
                    .ok_or(NotModelled("a static library without manifest name"))?,
                _ => ps.name.clone(),
            };
            consumers.push(Some((dependent, ps.version_code)));
        }
        Ok(consumers)
    }

    fn check_package_permission(
        &self,
        name: Option<&str>,
        permission: Option<&str>,
        user: i32,
    ) -> Result<i32, NotModelled> {
        if self.user(user).is_none() {
            return Ok(-1);
        }
        let ps = name.and_then(|name| self.state.packages.get(name));
        if ps.is_none() || self.filtered_including_uninstalled(ps, user)? {
            return Ok(-1);
        }
        let grants = user_state(ps.unwrap(), user).granted_permissions;
        Ok(if permission.is_some_and(|p| granted(&grants, p)) {
            0
        } else {
            -1
        })
    }

    pub(super) fn check_uid_permission(
        &self,
        uid: i32,
        permission: Option<&str>,
    ) -> Result<i32, NotModelled> {
        let user = user_id(uid);
        if self.user(user).is_none() {
            return Ok(-1);
        }
        if apps_filter::is_sdk_sandbox(uid) || apps_filter::is_isolated(uid) {
            return Err(NotModelled("permission uid owner mapping"));
        }
        let packages: Vec<&PackageState> = match setting(self.state, app_id(uid)) {
            Some(Setting::Package(ps)) => vec![ps],
            Some(Setting::Shared(su)) => apps_filter::shared_packages(self.state, su).collect(),
            None => Vec::new(),
        };
        if let Some(ps) = packages
            .into_iter()
            .find(|ps| user_state(ps, user).installed && ps.pkg.is_some())
        {
            return Ok(
                if permission.is_some_and(|p| granted(&user_state(ps, user).granted_permissions, p))
                {
                    0
                } else {
                    -1
                },
            );
        }
        let assignments = self
            .state
            .system
            .system_permissions
            .as_ref()
            .ok_or(NotModelled(
                "SystemConfig system-UID permission assignments",
            ))?;
        Ok(
            if assignments.get(&uid).is_some_and(|grants| {
                permission.is_some_and(|p| {
                    grants.contains(p) || fuller(p).is_some_and(|full| grants.contains(full))
                })
            }) {
                0
            } else {
                -1
            },
        )
    }

    fn appop_packages(&self, permission: Option<&str>, user: i32) -> Thrown<Vec<Option<String>>> {
        if let Err(e) = self.enforce_cross_user(user, false, false, "getAppOpPermissionPackages")? {
            return Ok(Err(e));
        }
        if permission.is_none()
            || self.user(user).is_none()
            || apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some()
        {
            return Ok(Ok(Vec::new()));
        }
        let mut names = Vec::new();
        for ps in self.packages_in_order() {
            let Some(pkg) = ps.pkg.as_ref() else {
                continue;
            };
            if pkg.is2(APEX)
                || !pkg
                    .requested_permissions
                    .iter()
                    .any(|p| Some(p.as_str()) == permission)
            {
                continue;
            }
            if !self.filtered_including_uninstalled(Some(ps), user)? {
                names.push(Some(pkg.package_name.clone()));
            }
        }
        Ok(Ok(names))
    }

    fn system_library_names(&self) -> Result<Option<Vec<Option<String>>>, NotModelled> {
        let entries = self.system_library_paths()?;
        Ok((!entries.is_empty()).then(|| entries.into_iter().map(|(name, _)| name).collect()))
    }

    fn system_library_paths(&self) -> Result<Vec<(Option<String>, Option<String>)>, NotModelled> {
        let libraries = self
            .state
            .shared_libraries
            .as_ref()
            .ok_or(NotModelled("the finalized shared library registry"))?;
        let mut entries = Vec::new();
        for library in libraries {
            if entries.iter().any(|(name, _)| name == &library.name) {
                continue;
            }
            let visible = if library.kind != 2 {
                true
            } else {
                let ps = library
                    .package_name
                    .as_ref()
                    .and_then(|name| self.state.packages.get(name));
                ps.is_some()
                    && !self.filter_shared_lib(
                        ps.unwrap(),
                        user_id(self.calling_uid),
                        MATCH_STATIC_SHARED_AND_SDK_LIBRARIES,
                    )?
            };
            if visible {
                entries.push((library.name.clone(), library.path.clone()));
            }
        }
        entries.sort_by_key(|(name, _)| name.as_deref().map_or(0, info::java_hash));
        Ok(entries)
    }

    fn is_instant(&self, name: &str, user: i32) -> Thrown<bool> {
        if let Err(e) = self.enforce_full_cross_user(user, "isInstantApp")? {
            return Ok(Err(e));
        }
        if apps_filter::is_isolated(self.calling_uid) {
            return Err(NotModelled("an isolated caller's owner"));
        }
        let Some(ps) = self.state.packages.get(name) else {
            return Ok(Ok(false));
        };
        if apps_filter::is_caller_same_app(self.state, Some(name), self.calling_uid)?
            || self.can_view_instant_apps()?
        {
            return Ok(Ok(user_state(ps, user).instant_app));
        }
        Err(NotModelled("instant app access grants"))
    }

    fn can_view_instant_apps(&self) -> Result<bool, NotModelled> {
        if self.calling_uid < 10000
            || self
                .uid_has_permission(self.calling_uid, "android.permission.ACCESS_INSTANT_APPS")?
        {
            return Ok(true);
        }
        if self.uid_has_permission(self.calling_uid, "android.permission.VIEW_INSTANT_APPS")? {
            return Err(NotModelled(
                "the default launcher and app prediction service",
            ));
        }
        Ok(false)
    }

    fn translate_names(
        &self,
        names: Option<&[Option<String>]>,
        canonical: bool,
    ) -> Result<Option<Vec<Option<String>>>, NotModelled> {
        let names = names.ok_or(NotModelled("a null package name array"))?;
        if apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some() {
            return Ok(Some(names.to_vec()));
        }
        let user = user_id(self.calling_uid);
        let can_view = self.can_view_instant_apps()?;
        let renamed = if canonical {
            Some(
                self.state
                    .renamed_packages
                    .as_ref()
                    .ok_or(NotModelled("the Settings renamed package table"))?,
            )
        } else {
            None
        };
        let mut result = Vec::with_capacity(names.len());
        for name in names {
            let ps = name.as_ref().and_then(|name| self.state.packages.get(name));
            let replacement = if let Some(renamed) = renamed {
                renamed
                    .iter()
                    .find(|(key, _)| Some(key) == name.as_ref())
                    .map(|(_, value)| value.clone())
            } else {
                ps.map(|ps| {
                    ps.real_name
                        .as_ref()
                        .ok_or(NotModelled("the package real-name owner"))
                })
                .transpose()?
                .and_then(Clone::clone)
            };
            let instant = ps.is_some_and(|ps| user_state(ps, user).instant_app);
            if replacement.is_some() && instant && !can_view {
                return Err(NotModelled("instant app access grants"));
            }
            result.push(replacement.or_else(|| name.clone()));
        }
        Ok(Some(result))
    }

    fn base_sdk_sandbox_uid(&self, uid: i32) -> Result<i32, NotModelled> {
        if !apps_filter::is_sdk_sandbox(uid) {
            return Ok(uid);
        }
        let name = self
            .state
            .system
            .sdk_sandbox_package
            .as_ref()
            .and_then(|n| n.as_deref())
            .ok_or(NotModelled("the base SDK sandbox package"))?;
        self.state
            .packages
            .get(name)
            .and_then(|ps| ps.pkg.as_ref())
            .map(|pkg| pkg.uid)
            .ok_or(NotModelled("the base SDK sandbox package code"))
    }

    fn uid_flags(&self, uid: i32, private: bool) -> Result<i32, NotModelled> {
        if apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some() {
            return Ok(0);
        }
        let uid = self.base_sdk_sandbox_uid(uid)?;
        let user = user_id(self.calling_uid);
        Ok(match setting(self.state, app_id(uid)) {
            Some(Setting::Shared(su)) if !self.shared_filtered(su, user, true)? => {
                if private {
                    su.private_flags
                } else {
                    su.flags
                }
            }
            Some(Setting::Package(ps))
                if !self.filtered_including_uninstalled(Some(ps), user)? =>
            {
                let (flags, private_flags) = ps
                    .setting_flags
                    .ok_or(NotModelled("raw package SettingBase flags"))?;
                if private { private_flags } else { flags }
            }
            _ => 0,
        })
    }

    fn visible_user_state(
        &self,
        name: &str,
        user: i32,
    ) -> Result<Option<PackageUserState>, NotModelled> {
        let ps = self.state.packages.get(name);
        if ps.is_none() || self.filtered_including_uninstalled(ps, user)? {
            return Ok(None);
        }
        Ok(Some(user_state(ps.unwrap(), user)))
    }

    fn hidden_setting(&self, name: &str, user: i32) -> Thrown<bool> {
        if !matches!(app_id(self.calling_uid), ROOT_UID | SYSTEM_UID)
            && !self.uid_has_permission(self.calling_uid, "android.permission.MANAGE_USERS")?
        {
            return Ok(Err(Exception::security("MANAGE_USERS permission required")));
        }
        if let Err(e) = self.enforce_full_cross_user(user, "getApplicationHidden for user")? {
            return Ok(Err(e));
        }
        Ok(Ok(self
            .visible_user_state(name, user)?
            .is_none_or(|u| u.hidden)))
    }

    fn all_packages(&self) -> Thrown<Vec<Option<String>>> {
        if !is_system_or_root_or_shell(self.calling_uid) {
            return Ok(Err(Exception::security(
                "getAllPackages is limited to privileged callers",
            )));
        }
        let user = user_id(self.calling_uid);
        let can_view = self.can_view_instant_apps()?;
        let mut result = Vec::new();
        for ps in self
            .packages_in_order()
            .into_iter()
            .filter(|ps| ps.pkg.is_some())
        {
            if !can_view && user_state(ps, user).instant_app {
                return Err(NotModelled("instant app access grants"));
            }
            result.push(Some(ps.pkg.as_ref().unwrap().package_name.clone()));
        }
        Ok(Ok(result))
    }

    pub(super) fn packages_holding_permissions(
        &self,
        permissions: &[Option<String>],
        flags: i64,
        user: i32,
    ) -> Thrown<Vec<PackageInfo>> {
        if self.user(user).is_none() {
            return Ok(Ok(Vec::new()));
        }
        let flags = match self.update_flags_for_package(flags, user)? {
            Ok(f) => f,
            Err(e) => return Ok(Err(e)),
        };
        if let Err(e) = self.enforce_full_cross_user(user, "get packages holding permissions")? {
            return Ok(Err(e));
        }
        let mut result = Vec::new();
        for ps in self.packages_in_order() {
            if ps.pkg.is_none() && flags & (MATCH_KNOWN_PACKAGES | MATCH_ARCHIVED_PACKAGES) == 0 {
                continue;
            }
            let state = user_state(ps, user);
            let granted: Vec<String> = permissions
                .iter()
                .filter_map(|p| p.as_ref())
                .filter(|p| state.granted_permissions.contains(p))
                .cloned()
                .collect();
            if granted.is_empty() {
                continue;
            }
            if let Some(mut info) = self.generate_package_info(ps, flags, user)? {
                if flags & GET_PERMISSIONS == 0 {
                    info.requested_permissions = Some(granted);
                }
                result.push(info);
            }
        }
        Ok(Ok(result))
    }

    fn package_available(&self, name: &str, user: i32) -> Thrown<bool> {
        if self.user(user).is_none() {
            return Ok(Ok(false));
        }
        if let Err(e) = self.enforce_cross_user(user, false, false, "is package available")? {
            return Ok(Err(e));
        }
        let Some((ps, _)) = self.package_of(name) else {
            return Ok(Ok(false));
        };
        let state = user_state(ps, user);
        Ok(Ok(!self.filtered(Some(ps), self.calling_uid, user)?
            && state.installed
            && !state.hidden))
    }

    fn package_gids(&self, name: &str, flags: i64, user: i32) -> Thrown<Option<Vec<i32>>> {
        if self.user(user).is_none() {
            return Ok(Ok(None));
        }
        let flags = match self.update_flags_for_package(flags, user)? {
            Ok(f) => f,
            Err(e) => return Ok(Err(e)),
        };
        if let Err(e) = self.enforce_cross_user(user, false, false, "getPackageGids")? {
            return Ok(Err(e));
        }
        let Some(ps) = self.state.packages.get(name) else {
            return Ok(Ok(None));
        };
        let u = user_state(ps, user);
        let system_match = flags & MATCH_SYSTEM_ONLY == 0 || ps.is.system;
        let available = (ps.pkg.is_some() && u.installed)
            || flags & (MATCH_KNOWN_PACKAGES | MATCH_ARCHIVED_PACKAGES) != 0;
        Ok(Ok((system_match
            && available
            && !self.filtered(Some(ps), self.calling_uid, user)?)
        .then(|| u.gids.clone())))
    }

    fn shared_uid(&self, name: Option<&str>) -> Result<i32, NotModelled> {
        if name.is_none()
            || apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some()
        {
            return Ok(-1);
        }
        match self.state.shared_users.get(name.unwrap()) {
            Some(su) if !self.shared_filtered(su, user_id(self.calling_uid), true)? => {
                Ok(su.app_id)
            }
            _ => Ok(-1),
        }
    }

    fn uid_privileged(&self, uid: i32) -> Result<bool, NotModelled> {
        if apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some() {
            return Ok(false);
        }
        let uid = self.base_sdk_sandbox_uid(uid)?;
        Ok(match setting(self.state, app_id(uid)) {
            Some(Setting::Package(ps)) => ps.is.privileged,
            Some(Setting::Shared(su)) => {
                apps_filter::shared_packages(self.state, su).any(|ps| ps.is.privileged)
            }
            None => false,
        })
    }

    fn installer_package(&self, name: &str) -> Thrown<Option<String>> {
        self.installer_package_internal(name, user_id(self.calling_uid))
    }
    pub(crate) fn installer_package_internal(
        &self,
        name: &str,
        user: i32,
    ) -> Thrown<Option<String>> {
        let Some(source) = self.install_source(name, user)? else {
            return Ok(Err(Exception::illegal_argument(format!(
                "Unknown package: {name}"
            ))));
        };
        let installer = source.and_then(|ps| ps.install_source.installer.as_ref());
        let Some(installer) = installer else {
            return Ok(Ok(None));
        };
        let ps = self.state.packages.get(installer);
        Ok(Ok((ps.is_some()
            && !self.filtered_including_uninstalled_not_archived(
                ps,
                self.calling_uid,
                user_id(self.calling_uid),
            )?)
        .then(|| installer.clone())))
    }

    fn enforce_full_cross_user(&self, user: i32, message: &str) -> Thrown<()> {
        if user < 0 {
            return Ok(Err(Exception::illegal_argument(format!(
                "Invalid userId {user}"
            ))));
        }
        if user_id(self.calling_uid) == user
            || matches!(app_id(self.calling_uid), ROOT_UID | SYSTEM_UID)
            || self.uid_has_permission(self.calling_uid, INTERACT_ACROSS_USERS_FULL)?
        {
            return Ok(Ok(()));
        }
        Ok(Err(Exception::security(format!(
            "{message}: UID {} requires {INTERACT_ACROSS_USERS_FULL} to access user {user}.",
            self.calling_uid
        ))))
    }

    fn install_reason(&self, name: &str, user: i32) -> Thrown<i32> {
        if let Err(e) = self.enforce_full_cross_user(user, "get install reason")? {
            return Ok(Err(e));
        }
        let ps = self.state.packages.get(name);
        if ps.is_none() || self.filtered_including_uninstalled(ps, user)? {
            return Ok(Ok(0));
        }
        Ok(Ok(user_state(ps.unwrap(), user).install_reason))
    }

    fn mime_group(&self, name: &str, group: Option<&str>) -> Thrown<Vec<Option<String>>> {
        if app_id(self.calling_uid) != SYSTEM_UID {
            let packages = self.packages_for_uid(self.calling_uid)?;
            if !packages
                .as_ref()
                .is_some_and(|p| p.iter().any(|p| p.as_deref() == Some(name)))
            {
                return Ok(Err(Exception::security(format!(
                    "Calling uid {} does not own package {name}",
                    self.calling_uid
                ))));
            }
            match self.package_info(name, VERSION_CODE_HIGHEST, 0, user_id(self.calling_uid))? {
                Err(e) => return Ok(Err(e)),
                Ok(None) => {
                    return Ok(Err(Exception::illegal_argument(format!(
                        "Unknown package {name} on user {}",
                        user_id(self.calling_uid)
                    ))));
                }
                Ok(Some(_)) => {}
            }
        }
        let Some(ps) = self.state.packages.get(name) else {
            return Ok(Ok(Vec::new()));
        };
        match ps.mime_groups.iter().find(|(n, _)| n.as_deref() == group) {
            Some((_, types)) => Ok(Ok(types.clone())),
            None => Ok(Err(Exception::illegal_argument(format!(
                "Unknown MIME group {} for package {name}",
                group.unwrap_or("null")
            )))),
        }
    }

    fn package_certificate(
        &self,
        name: &str,
        bytes: Option<&[u8]>,
        kind: i32,
    ) -> Result<bool, NotModelled> {
        let Some((ps, pkg)) = self.package_of(name) else {
            return Ok(false);
        };
        if self.filtered_including_uninstalled(Some(ps), user_id(self.calling_uid))? {
            return Ok(false);
        }
        Ok(pkg
            .signing_details
            .as_ref()
            .is_some_and(|s| certificate(s, bytes, kind)))
    }

    fn uid_signatures(
        &self,
        uid: i32,
    ) -> Result<Option<super::super::pkg::SigningDetails>, NotModelled> {
        let user = user_id(self.calling_uid);
        let signatures = match setting(self.state, app_id(uid)) {
            Some(Setting::Shared(su)) if !self.shared_filtered(su, user, true)? => {
                Some(su.signatures.as_ref())
            }
            Some(Setting::Package(ps))
                if !self.filtered_including_uninstalled(Some(ps), user)? =>
            {
                Some(ps.signatures.as_ref())
            }
            _ => None,
        };
        Ok(signatures.map(|s| match s {
            None => super::super::pkg::SigningDetails::default(),
            Some(s) => super::super::pkg::SigningDetails {
                signatures: Some(s.signatures.clone()),
                past_signing_certificates: s
                    .past_signatures
                    .as_ref()
                    .map(|p| p.iter().map(|(s, _)| s.clone()).collect()),
                scheme_version: s.scheme_version,
                ..Default::default()
            },
        }))
    }
}

fn certificate(s: &super::super::pkg::SigningDetails, bytes: Option<&[u8]>, kind: i32) -> bool {
    let Some(bytes) = bytes else {
        return false;
    };
    let matches = |cert: &[u8]| match kind {
        0 => cert == bytes,
        1 => {
            use sha2::Digest;
            sha2::Sha256::digest(cert).as_slice() == bytes
        }
        _ => false,
    };
    s.past_signing_certificates
        .as_ref()
        .is_some_and(|past| past.len() > 1 && past[..past.len() - 1].iter().any(|c| matches(c)))
        || matches!(s.signatures.as_deref(), Some([one]) if matches(one))
}

fn granted(grants: &[String], permission: &str) -> bool {
    let fuller = fuller(permission);
    grants
        .iter()
        .any(|p| p == permission || Some(p.as_str()) == fuller)
}

fn fuller(permission: &str) -> Option<&'static str> {
    match permission {
        "android.permission.ACCESS_COARSE_LOCATION" => {
            Some("android.permission.ACCESS_FINE_LOCATION")
        }
        "android.permission.INTERACT_ACROSS_USERS" => Some(INTERACT_ACROSS_USERS_FULL),
        _ => None,
    }
}

#[cfg(test)]
mod permission_tests {
    use super::*;

    #[test]
    fn app_metadata_source_uses_settings_and_encodes_missing_target_cause() {
        let state = State {
            packages: [(
                "p".into(),
                PackageState {
                    name: "p".into(),
                    app_id: 10100,
                    app_metadata_source: Some(2),
                    users: [(0, PackageUserState::default())].into(),
                    ..Default::default()
                },
            )]
            .into(),
            users: [(
                0,
                User {
                    id: 0,
                    ..Default::default()
                },
            )]
            .into(),
            system: System {
                permission_controller_package: Some(Some("p".into())),
                ..Default::default()
            },
            ..Default::default()
        };
        let filter = AppsFilter::new(&state, &Default::default()).unwrap();
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: 1000,
        };
        assert_eq!(query.app_metadata_source(Some("p"), 0).unwrap().unwrap(), 2);
        let mut args = Parcel::new();
        pm::GetAppMetadataSource {
            package_name: Some("missing".into()),
            user_id: 0,
        }
        .write(&mut args);
        let reply = query
            .answer(
                pm::DESCRIPTOR,
                pm::GET_APP_METADATA_SOURCE,
                &mut Reader::new(args.data(), args.objects()),
            )
            .unwrap();
        let mut reader = Reader::new(reply.data(), reply.objects());
        let dispatched = pm::read_get_app_metadata_source_reply(&mut reader)
            .unwrap()
            .unwrap_err();
        assert_eq!(dispatched.code, aim_binder_host::parcel::EX_PARCELABLE);
        assert!(dispatched.parcelable.is_some());
        assert_eq!(reader.remaining(), 0);

        assert_eq!(query.permission_controller().unwrap().unwrap(), "p");
        let error = query
            .app_metadata_source(Some("missing"), 0)
            .unwrap()
            .unwrap_err();
        assert_eq!(error.code, aim_binder_host::parcel::EX_PARCELABLE);
        let payload = error.parcelable.unwrap();
        let mut reader = Reader::new(payload.bytes(), &[]);
        assert_eq!(
            reader.read_string16().unwrap().as_deref(),
            Some("android.os.ParcelableException")
        );
        assert_eq!(
            reader.read_string16().unwrap().as_deref(),
            Some("android.content.pm.PackageManager$NameNotFoundException")
        );
        assert_eq!(reader.read_string16().unwrap().as_deref(), Some("missing"));
        assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn declared_library_consumers_match_versions_users_and_raw_null_lists() {
        let mut state = State {
            users: [(
                0,
                User {
                    id: 0,
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        };
        let package = |name: &str, id: i32, version: i64| PackageState {
            name: name.into(),
            app_id: id,
            version_code: version,
            pkg: Some(Arc::new(AndroidPackage {
                package_name: name.into(),
                uid: id,
                version_code: version as i32,
                booleans: booleans::ENABLED,
                base_apk_path: Some(format!("/data/app/{name}/base.apk")),
                path: Some(format!("/data/app/{name}")),
                ..Default::default()
            })),
            users: [(0, PackageUserState::default())].into(),
            ..Default::default()
        };
        state
            .packages
            .insert("provider".into(), package("provider", 10100, 3));
        let mut consumer = package("consumer", 10101, 7);
        consumer.uses_static_libraries = vec![("lib".into(), 1)];
        state.packages.insert("consumer".into(), consumer);
        let mut wrong = package("wrong", 10102, 8);
        wrong.uses_static_libraries = vec![("lib".into(), 2)];
        state.packages.insert("wrong".into(), wrong);
        state.shared_libraries = Some(vec![super::super::super::model::SharedLibrary {
            name: Some("lib".into()),
            kind: 2,
            version: 1,
            declaring: ("provider".into(), 3),
            ..Default::default()
        }]);
        let filter = AppsFilter::new(&state, &Default::default()).unwrap();
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: 1000,
        };
        let libraries = query
            .declared_libraries(Some("provider"), 0, 0)
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(libraries.items.len(), 1);
        assert_eq!(
            libraries.items[0].0.dependents,
            vec![Some(("consumer".into(), 7))]
        );
        assert_eq!(libraries.items[0].0.optional_dependents, None);
        assert!(libraries.items[0].0.dependents_initialized);
        assert!(
            query
                .declared_libraries(Some("provider"), 0, 42)
                .unwrap()
                .unwrap()
                .is_none()
        );
        drop(query);
        drop(filter);
        state
            .system
            .flags
            .push(("android.content.pm.sdk_lib_independence".into(), true));
        state
            .packages
            .get_mut("consumer")
            .unwrap()
            .uses_sdk_libraries = vec![super::super::super::settings::UsesSdkLibrary {
            name: "sdk".into(),
            version_major: 1,
            optional: true,
        }];
        state
            .shared_libraries
            .as_mut()
            .unwrap()
            .push(super::super::super::model::SharedLibrary {
                name: Some("sdk".into()),
                kind: 3,
                version: 1,
                declaring: ("provider".into(), 3),
                ..Default::default()
            });
        let filter = AppsFilter::new(&state, &Default::default()).unwrap();
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: 1000,
        };
        let shared = query
            .shared_libraries(Some("unowned"), 0, 0)
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(shared.items.len(), 2);
        assert_eq!(
            shared.items[1].0.optional_dependents,
            Some(vec![Some(("consumer".into(), 7))])
        );
        assert_eq!(
            shared.items[1].0.dependents,
            vec![Some(("consumer".into(), 7))]
        );
        drop(query);
        drop(filter);
        state.system.flags[0].1 = false;
        let filter = AppsFilter::new(&state, &Default::default()).unwrap();
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: 1000,
        };
        let shared = query
            .shared_libraries(Some("unowned"), 0, 0)
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(shared.items[1].0.optional_dependents, None);
    }

    #[test]
    fn metadata_queries_distinguish_missing_owners_empty_and_foreign_callers() {
        let mut state = State {
            system: System {
                sdk_sandbox_package: Some(None),
                initial_non_stopped_system_packages: Some(vec!["system.nonstopped".into()]),
                ..Default::default()
            },
            apex_inventory: Some(super::super::super::bootstrap::ApexInventory {
                packages: Some(vec![]),
                active: vec![],
            }),
            protected_broadcasts: Some(["p.protected".into()].into()),
            ..Default::default()
        };
        let filter = AppsFilter::new(&state, &Default::default()).unwrap();
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: 1000,
        };
        let mut args = Parcel::new();
        pm::GetSdkSandboxPackageName {}.write(&mut args);
        let reply = query
            .extra_package(
                pm::GET_SDK_SANDBOX_PACKAGE_NAME,
                &mut Reader::new(args.data(), args.objects()),
            )
            .unwrap();
        assert_eq!(
            pm::read_get_sdk_sandbox_package_name_reply(&mut Reader::new(
                reply.data(),
                reply.objects()
            ))
            .unwrap()
            .unwrap(),
            None
        );
        let mut args = Parcel::new();
        pm::GetAllApexDirectories {}.write(&mut args);
        let reply = query
            .extra_package(
                pm::GET_ALL_APEX_DIRECTORIES,
                &mut Reader::new(args.data(), args.objects()),
            )
            .unwrap();
        assert_eq!(
            pm::read_get_all_apex_directories_reply(&mut Reader::new(
                reply.data(),
                reply.objects()
            ))
            .unwrap()
            .unwrap(),
            Some(vec![])
        );
        let foreign = Query {
            calling_uid: 2000,
            ..query
        };
        let reply = foreign
            .extra_package(
                pm::GET_ALL_APEX_DIRECTORIES,
                &mut Reader::new(args.data(), args.objects()),
            )
            .unwrap();
        assert_eq!(
            Reader::new(reply.data(), reply.objects())
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        let mut args = Parcel::new();
        pm::IsProtectedBroadcast {
            action_name: Some("p.protected".into()),
        }
        .write(&mut args);
        let reply = foreign
            .extra_package(
                pm::IS_PROTECTED_BROADCAST,
                &mut Reader::new(args.data(), args.objects()),
            )
            .unwrap();
        assert!(
            pm::read_is_protected_broadcast_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .unwrap()
        );
        drop(foreign);
        drop(filter);
        state.protected_broadcasts = None;
        let filter = AppsFilter::new(&state, &Default::default()).unwrap();
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: 2000,
        };
        let mut args = Parcel::new();
        pm::IsProtectedBroadcast {
            action_name: Some("android.net.netmon.lingerExpired.42".into()),
        }
        .write(&mut args);
        let reply = query
            .extra_package(
                pm::IS_PROTECTED_BROADCAST,
                &mut Reader::new(args.data(), args.objects()),
            )
            .unwrap();
        assert!(
            pm::read_is_protected_broadcast_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .unwrap()
        );
    }

    #[test]
    fn system_uid_assignments_are_exact_and_never_implicit_root_grants() {
        let state = State {
            users: [
                (
                    0,
                    User {
                        id: 0,
                        ..Default::default()
                    },
                ),
                (
                    10,
                    User {
                        id: 10,
                        ..Default::default()
                    },
                ),
            ]
            .into(),
            system: System {
                system_permissions: Some(
                    [
                        (
                            2000,
                            ["android.permission.ACCESS_FINE_LOCATION".into()].into(),
                        ),
                        (1001000, [INTERACT_ACROSS_USERS_FULL.into()].into()),
                    ]
                    .into(),
                ),
                ..Default::default()
            },
            ..Default::default()
        };
        let filter = AppsFilter::new(&state, &Default::default()).unwrap();
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: 1000,
        };
        assert_eq!(
            query
                .check_uid_permission(2000, Some("android.permission.ACCESS_COARSE_LOCATION"))
                .unwrap(),
            0
        );
        assert_eq!(
            query.check_uid_permission(0, Some("p.unassigned")).unwrap(),
            -1
        );
        assert_eq!(
            query
                .check_uid_permission(1000, Some("android.permission.INTERACT_ACROSS_USERS"))
                .unwrap(),
            -1
        );
        assert_eq!(
            query
                .check_uid_permission(1001000, Some("android.permission.INTERACT_ACROSS_USERS"))
                .unwrap(),
            0
        );
        assert_eq!(
            query
                .check_uid_permission(42000, Some("p.unassigned"))
                .unwrap(),
            -1
        );
    }
}

struct Library(super::super::model::SharedLibrary);
impl aim_service_aidl::WriteParcelable for Library {
    fn write_to(&self, p: &mut Parcel) {
        info::write_library(p, &self.0);
    }
}

#[derive(Clone, Debug)]
pub(super) struct Instrumentation(pub(super) info::InstrumentationInfo);
impl aim_service_aidl::WriteParcelable for Instrumentation {
    fn write_to(&self, p: &mut Parcel) {
        self.0.write(p);
    }
}
