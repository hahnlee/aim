//! Additional snapshot queries from the pinned ComputerEngine and PMS owner.
use super::*;
use aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT;

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
            pm::GET_APP_METADATA_FD => {
                let a = args(pm::GetAppMetadataFd::read(r))?;
                thrown(self.app_metadata_fd(a.package_name.as_deref(), a.user_id),
                    |p, file| pm::write_get_app_metadata_fd_reply(p, file.as_ref()))
            }
            pm::GET_LAUNCH_INTENT_SENDER_FOR_PACKAGE => {
                let a = args(pm::GetLaunchIntentSenderForPackage::read(r))?;
                thrown(self.launch_sender(a.package_name.as_deref(), a.calling_package.as_deref(), a.feature_id.as_deref(), a.user_id),
                    |p, sender| pm::write_get_launch_intent_sender_for_package_reply(p, Some(&sender)))
            }
            pm::GET_INSTANT_APP_ANDROID_ID => {
                let a = args(pm::GetInstantAppAndroidId::read(r))?;
                thrown(self.instant_android_id(a.package_name.as_deref(), a.user_id),
                    |p, id| pm::write_get_instant_app_android_id_reply(p, &id))
            }
            pm::QUERY_SYNC_PROVIDERS => {
                let a = args(pm::QuerySyncProviders::<SyncProvider>::read(r))?;
                let result = self.sync_providers(a.out_names, a.out_info)?;
                Ok(reply(|p| pm::write_query_sync_providers_reply(p, &result)))
            }
            pm::GET_PERMISSION_GROUP_INFO => {
                let a = args(pm::GetPermissionGroupInfo::read(r))?;
                let owner = self.state.system.permission_groups.as_ref().ok_or(NotModelled("original PermissionManagerService group owner unavailable"))?;
                thrown(Ok(owner.group(a.name.as_deref(), a.flags)),
                    |p, group| pm::write_get_permission_group_info_reply(p, group.as_ref()))
            }
            pm::GET_WELLBEING_PACKAGE_NAME => {
                args(pm::GetWellbeingPackageName::read(r))?;
                let policy = self.state.system.user_policy.as_ref().ok_or(NotModelled("live RoleManager wellbeing owner unavailable"))?;
                thrown(Ok(policy.role_holders("android.app.role.SYSTEM_WELLBEING", 0).map(|holders| holders.into_iter().next())),
                    |p, name| pm::write_get_wellbeing_package_name_reply(p, &name))
            }
            pm::GET_ARCHIVED_PACKAGE => {
                let a = args(pm::GetArchivedPackage::read(r))?;
                thrown(self.archived_package(a.package_name.as_deref(), a.user_id),
                    |p, package| pm::write_get_archived_package_reply(p, package.as_ref()))
            }
            pm::GET_ARCHIVED_APP_ICON => {
                let a = args(pm::GetArchivedAppIcon::<super::super::archive::UserHandle>::read(r))?;
                thrown(self.archived_icon(a.package_name.as_deref(), a.user.as_ref().map(|user| user.0), a.calling_package_name.as_deref()),
                    |p, icon| pm::write_get_archived_app_icon_reply(p, icon.as_ref()))
            }
            pm::IS_APP_ARCHIVABLE => {
                let a = args(pm::IsAppArchivable::<super::super::archive::UserHandle>::read(r))?;
                thrown(self.archivable(a.package_name.as_deref(), a.user.as_ref().map(|user| user.0)), pm::write_is_app_archivable_reply)
            }
            pm::CAN_REQUEST_PACKAGE_INSTALLS => {
                let a = args(pm::CanRequestPackageInstalls::read(r))?;
                thrown(self.internal_can_request_installs(a.package_name.as_deref(), self.calling_uid, a.user_id, true),
                    pm::write_can_request_package_installs_reply)
            }
            pm::IS_AUTO_REVOKE_WHITELISTED => {
                let a = args(pm::IsAutoRevokeWhitelisted::read(r))?;
                thrown(Ok(self.security_policy()?.auto_revoke(self.calling_uid, a.package_name.as_deref())
                    .map(|mode| mode == 1)), pm::write_is_auto_revoke_whitelisted_reply)
            }
            pm::GET_INSTALL_LOCATION => {
                args(pm::GetInstallLocation::read(r))?;
                thrown(Ok(self.security_policy()?.install_location()), pm::write_get_install_location_reply)
            }
            pm::IS_PACKAGE_DEVICE_ADMIN_ON_ANY_USER => {
                let a = args(pm::IsPackageDeviceAdminOnAnyUser::read(r))?;
                thrown(self.device_admin_any_user(a.package_name.as_deref()), pm::write_is_package_device_admin_on_any_user_reply)
            }
            pm::IS_PACKAGE_STATE_PROTECTED => {
                let a = args(pm::IsPackageStateProtected::read(r))?;
                thrown(self.package_state_protected(a.package_name.as_deref(), a.user_id), pm::write_is_package_state_protected_reply)
            }
            pm::GET_INSTANT_APP_COOKIE => {
                let a = args(pm::GetInstantAppCookie::read(r))?;
                thrown(self.instant_cookie(a.package_name.as_deref(), a.user_id),
                    |p, value| pm::write_get_instant_app_cookie_reply(p, &value))
            }
            pm::SET_INSTANT_APP_COOKIE => {
                let a = args(pm::SetInstantAppCookie::read(r))?;
                if r.remaining() != 0 { return Err(NotModelled("trailing instant cookie setter arguments")); }
                thrown(self.set_instant_cookie(a.package_name.as_deref(), a.cookie, a.user_id),
                    pm::write_set_instant_app_cookie_reply)
            }
            pm::GET_INSTANT_APP_ICON => {
                let a = args(pm::GetInstantAppIcon::read(r))?;
                thrown(self.instant_icon(a.package_name.as_deref(), a.user_id),
                    |p, value| pm::write_get_instant_app_icon_reply(p, value.as_ref()))
            }
            pm::GET_INSTANT_APPS => {
                let a = args(pm::GetInstantApps::read(r))?;
                thrown(self.instant_apps(a.user_id), |p, value| pm::write_get_instant_apps_reply(p,
                    value.as_ref().map(|items| ListSlice { creator: "android.content.pm.InstantAppInfo".into(), items: items.clone() }).as_ref()))
            }
            pm::ENTER_SAFE_MODE => {
                args(pm::EnterSafeMode::read(r))?;
                if r.remaining() != 0 { return Err(NotModelled("trailing safe-mode setter arguments")); }
                let owner = self.lifecycle_owner()?;
                thrown(Ok(owner.enter_safe_mode(self.calling_uid).map_err(Exception::security)),
                    |p, ()| pm::write_enter_safe_mode_reply(p))
            }
            pm::IS_FIRST_BOOT => {
                args(pm::IsFirstBoot::read(r))?;
                let owner = self.lifecycle_owner()?;
                Ok(reply(|p| pm::write_is_first_boot_reply(p, owner.first_boot())))
            }
            pm::IS_DEVICE_UPGRADING => {
                args(pm::IsDeviceUpgrading::read(r))?;
                let owner = self.lifecycle_owner()?;
                Ok(reply(|p| pm::write_is_device_upgrading_reply(p, owner.device_upgrading())))
            }
            pm::IS_SAFE_MODE => {
                args(pm::IsSafeMode::read(r))?;
                let owner = self.lifecycle_owner()?;
                Ok(reply(|p| pm::write_is_safe_mode_reply(p, owner.safe_mode())))
            }
            pm::GET_PERSISTENT_APPLICATIONS => {
                let a = args(pm::GetPersistentApplications::read(r))?;
                let items = self.persistent_applications(a.flags)?;
                Ok(reply(|p| pm::write_get_persistent_applications_reply(p,
                    Some(&ListSlice { creator: "android.content.pm.ApplicationInfo".into(), items }))))
            }
            pm::CHECK_PACKAGE_STARTABLE => {
                let a = args(pm::CheckPackageStartable::read(r))?;
                thrown(self.check_startable(a.package_name.as_deref(), a.user_id),
                    |p, ()| pm::write_check_package_startable_reply(p))
            }
            pm::ACTIVITY_SUPPORTS_INTENT_AS_USER => {
                let a = args(pm::ActivitySupportsIntentAsUser::<ComponentName, super::super::intent::Intent>::read(r))?;
                thrown(self.activity_supports_intent(a.class_name.as_ref(), a.intent.as_ref(),
                    a.resolved_type.as_deref(), a.user_id), pm::write_activity_supports_intent_as_user_reply)
            }
            pm::GET_INSTANT_APP_RESOLVER_COMPONENT => {
                args(pm::GetInstantAppResolverComponent::read(r))?;
                let owner = self
                    .state
                    .system
                    .instant_components
                    .as_ref()
                    .ok_or(NotModelled("instant component owner unavailable"))?;
                if owner.needs_resolution() && apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_none() {
                    if let Err(error) = self.enforce_cross_user(0, false, false, "query intent services")? {
                        return thrown(Ok(Err(error)), |_: &mut Parcel, _: ()| {});
                    }
                }
                let value = owner.resolver(self)?.map(InstantComponent);
                Ok(reply(|p| {
                    pm::write_get_instant_app_resolver_component_reply(p, value.as_ref())
                }))
            }
            pm::GET_INSTANT_APP_INSTALLER_COMPONENT => {
                args(pm::GetInstantAppInstallerComponent::read(r))?;
                let owner = self
                    .state
                    .system
                    .instant_components
                    .as_ref()
                    .ok_or(NotModelled("instant component owner unavailable"))?;
                let value = owner.installer(self)?.map(InstantComponent);
                Ok(reply(|p| {
                    pm::write_get_instant_app_installer_component_reply(p, value.as_ref())
                }))
            }
            pm::GET_INSTANT_APP_RESOLVER_SETTINGS_COMPONENT => {
                args(pm::GetInstantAppResolverSettingsComponent::read(r))?;
                let owner = self
                    .state
                    .system
                    .instant_components
                    .as_ref()
                    .ok_or(NotModelled("instant component owner unavailable"))?;
                let value = owner.settings().map(InstantComponent);
                Ok(reply(|p| {
                    pm::write_get_instant_app_resolver_settings_component_reply(p, value.as_ref())
                }))
            }
            pm::IS_PAGE_SIZE_COMPAT_ENABLED => {
                let a = args(pm::IsPageSizeCompatEnabled::read(r))?;
                thrown(
                    self.page_size_compat_enabled(a.package_name.as_deref()),
                    pm::write_is_page_size_compat_enabled_reply,
                )
            }
            pm::GET_PAGE_SIZE_COMPAT_WARNING_MESSAGE => {
                let a = args(pm::GetPageSizeCompatWarningMessage::read(r))?;
                thrown(
                    self.page_size_compat_warning(a.package_name.as_deref()),
                    |p, v| pm::write_get_page_size_compat_warning_message_reply(p, &v),
                )
            }
            pm::GET_BLOCK_UNINSTALL_FOR_USER => {
                let a = args(pm::GetBlockUninstallForUser::read(r))?;
                thrown(
                    self.block_uninstall(a.package_name.as_deref(), a.user_id),
                    pm::write_get_block_uninstall_for_user_reply,
                )
            }
            pm::CAN_PACKAGE_QUERY => {
                let a = args(pm::CanPackageQuery::read(r))?;
                thrown(
                    self.can_package_query(
                        a.source_package_name.as_deref(),
                        a.target_package_names.as_deref(),
                        a.user_id,
                    ),
                    |p, v| pm::write_can_package_query_reply(p, &Some(v)),
                )
            }
            pm::GET_SUSPENDING_PACKAGE => {
                let a = args(pm::GetSuspendingPackage::read(r))?;
                thrown(
                    self.suspending_package(a.package_name.as_deref(), a.user_id),
                    |p, v| pm::write_get_suspending_package_reply(p, &v),
                )
            }
            pm::GET_SUSPENDED_PACKAGE_APP_EXTRAS => {
                let a = args(pm::GetSuspendedPackageAppExtras::read(r))?;
                thrown(
                    self.suspended_extras(a.package_name.as_deref(), a.user_id),
                    |p, v| pm::write_get_suspended_package_app_extras_reply(p, v.as_ref()),
                )
            }
            pm::GET_SHARED_SYSTEM_SHARED_LIBRARY_PACKAGE_NAME => {
                args(pm::GetSharedSystemSharedLibraryPackageName::read(r))?;
                thrown(
                    self.configured_role(super::super::roles::Role::SharedSystemLibrary),
                    |p, v| pm::write_get_shared_system_shared_library_package_name_reply(p, &v),
                )
            }
            pm::GET_INTENT_VERIFICATION_STATUS => {
                let a = args(pm::GetIntentVerificationStatus::read(r))?;
                thrown(
                    self.legacy_domain_state(a.package_name.as_deref(), a.user_id),
                    pm::write_get_intent_verification_status_reply,
                )
            }
            pm::GET_KEY_SET_BY_ALIAS => {
                let a = args(pm::GetKeySetByAlias::read(r))?;
                thrown(
                    self.keyset(a.package_name.as_deref(), a.alias.as_deref(), false),
                    |p, v| pm::write_get_key_set_by_alias_reply(p, v.as_ref()),
                )
            }
            pm::GET_SIGNING_KEY_SET => {
                let a = args(pm::GetSigningKeySet::read(r))?;
                thrown(
                    self.keyset(a.package_name.as_deref(), None, true),
                    |p, v| pm::write_get_signing_key_set_reply(p, v.as_ref()),
                )
            }
            pm::IS_PACKAGE_SIGNED_BY_KEY_SET => {
                let a =
                    args(pm::IsPackageSignedByKeySet::<super::super::keysets::KeySet>::read(r))?;
                thrown(
                    self.signed_by_keyset(a.package_name.as_deref(), a.ks.as_ref(), false),
                    pm::write_is_package_signed_by_key_set_reply,
                )
            }
            pm::IS_PACKAGE_SIGNED_BY_KEY_SET_EXACTLY => {
                let a = args(pm::IsPackageSignedByKeySetExactly::<
                    super::super::keysets::KeySet,
                >::read(r))?;
                thrown(
                    self.signed_by_keyset(a.package_name.as_deref(), a.ks.as_ref(), true),
                    pm::write_is_package_signed_by_key_set_exactly_reply,
                )
            }
            pm::GET_DEFAULT_TEXT_CLASSIFIER_PACKAGE_NAME => {
                args(pm::GetDefaultTextClassifierPackageName::read(r))?;
                thrown(
                    self.configured_role(super::super::roles::Role::DefaultTextClassifier),
                    |p, v| pm::write_get_default_text_classifier_package_name_reply(p, &v),
                )
            }
            pm::GET_SYSTEM_TEXT_CLASSIFIER_PACKAGE_NAME => {
                args(pm::GetSystemTextClassifierPackageName::read(r))?;
                thrown(
                    self.configured_role(super::super::roles::Role::SystemTextClassifier),
                    |p, v| pm::write_get_system_text_classifier_package_name_reply(p, &v),
                )
            }
            pm::GET_APP_PREDICTION_SERVICE_PACKAGE_NAME => {
                args(pm::GetAppPredictionServicePackageName::read(r))?;
                thrown(
                    self.configured_role(super::super::roles::Role::AppPrediction),
                    |p, v| pm::write_get_app_prediction_service_package_name_reply(p, &v),
                )
            }
            pm::GET_INCIDENT_REPORT_APPROVER_PACKAGE_NAME => {
                args(pm::GetIncidentReportApproverPackageName::read(r))?;
                thrown(
                    self.configured_role(super::super::roles::Role::IncidentApprover),
                    |p, v| pm::write_get_incident_report_approver_package_name_reply(p, &v),
                )
            }
            pm::GET_ATTENTION_SERVICE_PACKAGE_NAME => {
                args(pm::GetAttentionServicePackageName::read(r))?;
                thrown(
                    self.configured_role(super::super::roles::Role::Attention),
                    |p, v| pm::write_get_attention_service_package_name_reply(p, &v),
                )
            }
            pm::GET_ROTATION_RESOLVER_PACKAGE_NAME => {
                args(pm::GetRotationResolverPackageName::read(r))?;
                thrown(
                    self.configured_role(super::super::roles::Role::RotationResolver),
                    |p, v| pm::write_get_rotation_resolver_package_name_reply(p, &v),
                )
            }
            pm::GET_SYSTEM_CAPTIONS_SERVICE_PACKAGE_NAME => {
                args(pm::GetSystemCaptionsServicePackageName::read(r))?;
                thrown(
                    self.configured_role(super::super::roles::Role::SystemCaptions),
                    |p, v| pm::write_get_system_captions_service_package_name_reply(p, &v),
                )
            }
            pm::GET_SETUP_WIZARD_PACKAGE_NAME => {
                args(pm::GetSetupWizardPackageName::read(r))?;
                thrown(
                    self.configured_role(super::super::roles::Role::SetupWizard),
                    |p, v| pm::write_get_setup_wizard_package_name_reply(p, &v),
                )
            }
            pm::GET_SERVICES_SYSTEM_SHARED_LIBRARY_PACKAGE_NAME => {
                args(pm::GetServicesSystemSharedLibraryPackageName::read(r))?;
                thrown(
                    self.configured_role(super::super::roles::Role::ServicesExtension),
                    |p, v| pm::write_get_services_system_shared_library_package_name_reply(p, &v),
                )
            }
            pm::GET_PROPERTY_AS_USER => {
                let a = args(pm::GetPropertyAsUser::read(r))?;
                thrown(
                    self.package_property(
                        a.property_name.as_deref(),
                        a.package_name.as_deref(),
                        a.class_name.as_deref(),
                        a.user_id,
                    ),
                    |p, v| pm::write_get_property_as_user_reply(p, v.as_ref()),
                )
            }
            pm::QUERY_PROPERTY => {
                let a = args(pm::QueryProperty::read(r))?;
                thrown(
                    self.query_properties(a.property_name.as_deref(), a.component_type),
                    |p, v| {
                        pm::write_query_property_reply(
                            p,
                            Some(&ListSlice {
                                creator: "android.content.pm.PackageManager$Property".into(),
                                items: v,
                            }),
                        )
                    },
                )
            }
            pm::QUERY_CONTENT_PROVIDERS => {
                let a = args(pm::QueryContentProviders::read(r))?;
                thrown(
                    self.content_providers(
                        a.process_name.as_deref(),
                        a.uid,
                        a.flags,
                        a.meta_data_key.as_deref(),
                    ),
                    |p, v| {
                        pm::write_query_content_providers_reply(
                            p,
                            Some(&ListSlice {
                                creator: "android.content.pm.ProviderInfo".into(),
                                items: v,
                            }),
                        )
                    },
                )
            }
            pm::GET_INTENT_FILTER_VERIFICATIONS => {
                args(pm::GetIntentFilterVerifications::read(r))?;
                // IPackageManagerBase explicitly retires this API with an empty slice.
                Ok(reply(|p| {
                    pm::write_get_intent_filter_verifications_reply(p, Some(&EmptyListSlice))
                }))
            }
            pm::HAS_SYSTEM_UID_ERRORS => {
                args(pm::HasSystemUidErrors::read(r))?;
                // The pinned IPackageManagerBase unconditionally reports false.
                Ok(reply(|p| pm::write_has_system_uid_errors_reply(p, false)))
            }
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
                thrown(
                    self.check_uid_signatures_captured(a.uid1, a.uid2, false),
                    pm::write_check_uid_signatures_reply,
                )
            }
            _ => Err(NotModelled("a method not modelled")),
        }
    }

    fn lifecycle_owner(&self) -> Result<&super::super::lifecycle::Owner, NotModelled> {
        self.state.system.lifecycle.as_deref().ok_or(NotModelled("native PMS lifecycle owner unavailable"))
    }
    pub(super) fn persistent_applications(&self, flags: i32) -> Result<Vec<ApplicationInfo>, NotModelled> {
        if apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some() { return Ok(Vec::new()); }
        self.persistent_applications_captured(self.lifecycle_owner()?.safe_mode(), flags)
    }
    pub(crate) fn persistent_applications_captured(&self, safe_mode: bool, flags: i32) -> Result<Vec<ApplicationInfo>, NotModelled> {
        let user = user_id(self.calling_uid);
        let entries = if let Some(registry) = &self.state.package_registry {
            let ordered = registry.ordered_package_names();
            let expected = self.state.packages.values().filter(|package| package.pkg.is_some()).count();
            if ordered.len() != expected {
                return Err(NotModelled("persistent application registration inventory differs"));
            }
            ordered.into_iter().map(|name| self.state.packages.get(name)
                .filter(|package| package.pkg.is_some())
                .ok_or(NotModelled("persistent application registered package unavailable")))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            let mut entries = self.state.packages.values().filter(|package| package.pkg.is_some()).collect::<Vec<_>>();
            entries.sort_by_key(|package| info::java_hash(&package.name));
            if entries.windows(2).any(|pair| info::java_hash(&pair[0].name) == info::java_hash(&pair[1].name)) {
                return Err(NotModelled("persistent application hash collision registration owner unavailable"));
            }
            entries
        };
        let mut items = Vec::new();
        let flags = i64::from(flags);
        for ps in entries {
            let Some(package) = ps.pkg.as_deref() else { continue; };
            let aware = package.booleans & booleans::DIRECT_BOOT_AWARE != 0;
            if package.booleans & booleans::PERSISTENT == 0 || safe_mode && !ps.is.system
                || !((flags & MATCH_DIRECT_BOOT_UNAWARE != 0 && !aware)
                    || (flags & MATCH_DIRECT_BOOT_AWARE != 0 && aware)) { continue; }
            let state = user_state(ps, user);
            if let Some(info) = generate_application_info(&self.target(ps, package, &state, user), flags) {
                items.push(info);
            }
        }
        Ok(items)
    }
    pub(super) fn check_startable(&self, name: Option<&str>, user: i32) -> Thrown<()> {
        if apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some() {
            return Ok(Err(Exception::security("Instant applications don't have access to this method")));
        }
        if !self.state.users.contains_key(&user) { return Ok(Err(Exception::security("User doesn't exist"))); }
        if let Err(error) = self.enforce_cross_user(user, false, false, "checkPackageStartable")? { return Ok(Err(error)); }
        let status = self.package_startability_captured(self.lifecycle_owner()?.safe_mode(), name, self.calling_uid, user)?;
        let name = name.unwrap_or("null");
        let message = match status {
            1 => format!("Package {name} was not found!"),
            2 => format!("Package {name} not a system app!"),
            3 => format!("Package {name} is currently frozen!"),
            4 => format!("Package {name} is not encryption aware!"),
            _ => return Ok(Ok(())),
        };
        Ok(Err(Exception::security(message)))
    }
    pub(crate) fn package_startability_captured(&self, safe_mode: bool, name: Option<&str>, filter_uid: i32, user: i32) -> Result<i32, NotModelled> {
        let owner = self.lifecycle_owner()?;
        owner.check_frozen_publication().map_err(|_| NotModelled("freezer generation publication failed"))?;
        let unlocked = owner.ce_storage_unlocked(user).map_err(|_| NotModelled("original CE storage owner query failed"))?;
        let resolved = name.map(|name| self.resolve_internal_package_name(name, VERSION_CODE_HIGHEST));
        let package = resolved.as_ref().and_then(|name| self.state.packages.get(name));
        if package.is_none() || self.filtered(package, filter_uid, user)?
            || !user_state(package.unwrap(), user).installed { return Ok(1); }
        let package = package.unwrap();
        if safe_mode && !package.is.system { return Ok(2); }
        if owner.checked_is_frozen(name.unwrap_or("null"))
            .map_err(|_| NotModelled("freezer generation publication failed"))? { return Ok(3); }
        if !unlocked {
            let pkg = package.pkg.as_deref().ok_or(NotModelled("startability parsed package owner unavailable"))?;
            if pkg.booleans & (booleans::DIRECT_BOOT_AWARE | booleans::PARTIALLY_DIRECT_BOOT_AWARE) == 0 { return Ok(4); }
        }
        Ok(0)
    }

    pub(super) fn activity_supports_intent(&self, component: Option<&ComponentName>,
        intent: Option<&super::super::intent::Intent>, resolved_type: Option<&str>, user: i32) -> Thrown<bool> {
        if let Err(error) = self.enforce_cross_user(user, false, false, "activitySupportsIntentAsUser")? {
            return Ok(Err(error));
        }
        let Some(component) = component else {
            return Ok(Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "component is null")));
        };
        let resolver = self.state.platform.custom_resolver.as_deref().map(|flat| {
            flat.split_once('/').map(|(package, class)| ComponentName {
                package: package.into(), class: if class.starts_with('.') { format!("{package}{class}") } else { class.into() }
            })
        }).unwrap_or_else(|| Some(ComponentName { package: "android".into(), class: "com.android.internal.app.ResolverActivity".into() }));
        if resolver.as_ref() == Some(component) { return Ok(Ok(true)); }
        let Some((ps, package)) = self.package_of(&component.package) else { return Ok(Ok(false)); };
        let Some(activity) = package.activities.iter().find(|activity| activity.main.component.name == component.class) else { return Ok(Ok(false)); };
        if self.filtered_component(Some(ps), component, 1, self.calling_uid, user)? { return Ok(Ok(false)); }
        let Some(intent) = intent else {
            return Ok(Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "intent is null")));
        };
        for info in &activity.main.component.intents {
            let matches = match info.filter.matches(intent.action.as_deref(), resolved_type,
                intent.data.as_ref().and_then(|data| data.scheme()),
                intent.data.as_ref(), intent.categories.as_deref(), false, None) {
                Ok(value) => value,
                Err(error) => return match error.binder_exception() {
                    Some(exception) => Ok(Err(exception)),
                    None => Err(NotModelled("activity URI matching exception not serialized by Parcel")),
                },
            };
            if matches >= 0 { return Ok(Ok(true)); }
        }
        Ok(Ok(false))
    }

    pub(crate) fn internal_block_uninstall(&self, user: i32, name: Option<&str>) -> Result<bool, NotModelled> {
        let blocks = self.state.system.uninstall_blocks.as_ref().ok_or(NotModelled("native Settings block-uninstall owner unavailable"))?;
        Ok(blocks.get(user, name))
    }
    pub(crate) fn internal_shared_libraries(&self) -> Result<Vec<Library>, NotModelled> {
        let libraries = self.state.shared_libraries.as_ref().ok_or(NotModelled("native SharedLibraries owner unavailable"))?;
        Ok(libraries.iter().cloned().map(Library).collect())
    }

    fn block_uninstall(&self, name: Option<&str>, user: i32) -> Thrown<bool> {
        let package = name.and_then(|name| self.state.packages.get(name));
        if package.is_none() || self.filtered_including_uninstalled(package, user)? {
            return Ok(Ok(false));
        }
        let blocks = self.state.system.uninstall_blocks.as_ref()
            .ok_or(NotModelled("the native Settings block-uninstall owner"))?;
        Ok(Ok(blocks.get(user, name)))
    }

    fn can_package_query(
        &self,
        source: Option<&str>,
        targets: Option<&[Option<String>]>,
        user: i32,
    ) -> Thrown<Vec<bool>> {
        let Some(targets) = targets else {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "Attempt to get length of null array",
            )));
        };
        let results = vec![false; targets.len()];
        if !self.state.users.contains_key(&user) {
            return Ok(Ok(results));
        }
        if let Err(error) = self.enforce_cross_user(user, false, false, "can package query")? {
            return Ok(Err(error));
        }
        let source_name =
            source.map(|name| self.resolve_internal_package_name(name, VERSION_CODE_HIGHEST));
        let source_setting = source_name
            .as_ref()
            .and_then(|name| self.state.packages.get(name));
        let mut missing = source_setting.is_none()
            || self.filtered_including_uninstalled(source_setting, user)?;
        let mut settings = Vec::with_capacity(targets.len());
        for name in targets {
            if missing {
                break;
            }
            let resolved = name
                .as_ref()
                .map(|name| self.resolve_internal_package_name(name, VERSION_CODE_HIGHEST));
            let setting = resolved
                .as_ref()
                .and_then(|name| self.state.packages.get(name));
            missing = setting.is_none() || self.filtered_including_uninstalled(setting, user)?;
            settings.push(setting);
        }
        if missing {
            let names = targets
                .iter()
                .map(|name| name.as_deref().unwrap_or("null"))
                .collect::<Vec<_>>()
                .join(", ");
            let message = format!(
                "Package(s) {} and/or [{}] not found.",
                source.unwrap_or("null"),
                names
            );
            let mut payload = Parcel::new();
            payload.write_string16(Some("android.os.ParcelableException"));
            payload.write_string16(Some(
                "android.content.pm.PackageManager$NameNotFoundException",
            ));
            payload.write_string16(Some(&message));
            let outer =
                format!("android.content.pm.PackageManager$NameNotFoundException: {message}");
            return Ok(Err(Exception::parcelable(Some(&outer), &payload)
                .map_err(|_| NotModelled("package query exception payload"))?));
        }
        let source_uid = apps_filter::uid(user, source_setting.unwrap().app_id);
        settings
            .into_iter()
            .map(|setting| {
                self.filtered(setting, source_uid, user)
                    .map(|filtered| !filtered)
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Ok)
    }

    fn suspending_package(&self, name: Option<&str>, user: i32) -> Thrown<Option<String>> {
        if let Err(error) = self.full_cross_user(user, false)? {
            return Ok(Err(error));
        }
        let package = name.and_then(|name| self.state.packages.get(name));
        if package.is_none() || self.filtered_including_uninstalled(package, user)? {
            return Ok(Ok(None));
        }
        let state = user_state(package.unwrap(), user);
        if state.suspended_by.is_empty() {
            return Ok(Ok(None));
        }
        let suspensions = state
            .suspensions
            .as_ref()
            .ok_or(NotModelled("native suspension params owner unavailable"))?;
        let (mut last, mut system, mut quarantined) = (None, None, None);
        for suspension in suspensions {
            last = Some(suspension.package.clone());
            if suspension.package == "android" {
                system = last.clone();
            }
            let Some(params) = &suspension.params else {
                return Ok(Err(Exception::new(
                    aim_binder_host::parcel::EX_NULL_POINTER,
                    "Attempt to invoke virtual method 'boolean com.android.server.pm.pkg.SuspendParams.isQuarantined()' on a null object reference",
                )));
            };
            if params.quarantined && quarantined.is_none() {
                quarantined = last.clone();
            }
        }
        Ok(Ok(quarantined.or(system).or(last)))
    }
    fn suspended_extras(&self, name: Option<&str>, user: i32) -> Thrown<Option<SuspensionExtras>> {
        let uid = match self.package_uid(name.unwrap_or_default(), 0, user)? {
            Ok(uid) => uid,
            Err(error) => return Ok(Err(error)),
        };
        if uid != self.calling_uid {
            return Ok(Err(Exception::security(format!(
                "Calling package {} does not belong to calling uid {}",
                name.unwrap_or("null"),
                self.calling_uid
            ))));
        }
        let name =
            self.resolve_internal_package_name(name.unwrap_or_default(), VERSION_CODE_HIGHEST);
        let Some(package) = self.state.packages.get(&name) else {
            return Ok(Ok(None));
        };
        let state = user_state(package, user);
        if state.suspended_by.is_empty() {
            return Ok(Ok(None));
        }
        let suspensions = state
            .suspensions
            .as_ref()
            .ok_or(NotModelled("native suspension params owner unavailable"))?;
        let mut bundle = super::super::restrictions::persistable::Bundle::default();
        for suspension in suspensions {
            if let Some(extras) = suspension
                .params
                .as_ref()
                .and_then(|params| params.app_extras.as_ref())
            {
                let mut entries = extras.entries.iter().collect::<Vec<_>>();
                entries.sort_by_key(|(key, _)| {
                    key.as_deref()
                        .map_or(0, super::super::parse::parcel::java_hash)
                });
                for (key, value) in entries {
                    if let Some((_, old)) = bundle.entries.iter_mut().find(|(name, _)| name == key)
                    {
                        *old = value.clone();
                    } else {
                        bundle.entries.push((key.clone(), value.clone()));
                    }
                }
            }
        }
        if bundle.entries.is_empty() {
            return Ok(Ok(None));
        }
        let payload = bundle
            .parcel()
            .map_err(|_| NotModelled("native suspension extras parcel is invalid"))?;
        Ok(Ok(Some(SuspensionExtras(payload))))
    }

    fn legacy_domain_state(&self, name: Option<&str>, user: i32) -> Thrown<i32> {
        let caller_user = user_id(self.calling_uid);
        if caller_user != user {
            if let Err(error) = self.enforce_cross_user(
                user,
                true,
                false,
                "Caller is not allowed to edit other users",
            )? {
                return Ok(Err(error));
            }
        }
        for id in [caller_user, user] {
            if self.user(id).is_none() {
                return Ok(Err(Exception::security(format!(
                    "User {id} does not exist"
                ))));
            }
        }
        let package = name.and_then(|name| self.state.packages.get(name));
        if self.filtered(package, self.calling_uid, user)? {
            return Ok(Ok(0));
        }
        let states = self
            .state
            .legacy_domains
            .as_ref()
            .ok_or(NotModelled("native legacy domain owner unavailable"))?;
        Ok(Ok(states
            .iter()
            .find(|(package, _)| package.as_deref() == name)
            .and_then(|(_, states)| {
                states
                    .iter()
                    .find(|(id, _)| *id == user)
                    .map(|(_, state)| *state)
            })
            .unwrap_or(0)))
    }

    fn keyset_package(&self, name: &str) -> Thrown<&PackageState> {
        let package = self.package_of(name).map(|(ps, _)| ps);
        let visible = match package {
            Some(ps) => {
                !self.filtered_including_uninstalled(Some(ps), user_id(self.calling_uid))?
            }
            None => false,
        };
        if !visible {
            return Ok(Err(Exception::new(
                EX_ILLEGAL_ARGUMENT,
                format!("Unknown package: {name}"),
            )));
        }
        Ok(Ok(package.unwrap()))
    }
    fn keyset(
        &self,
        name: Option<&str>,
        alias: Option<&str>,
        signing: bool,
    ) -> Thrown<Option<super::super::keysets::KeySet>> {
        let Some(name) = name else {
            return Ok(Ok(None));
        };
        if !signing && alias.is_none() {
            return Ok(Ok(None));
        }
        let ps = match self.keyset_package(name)? {
            Ok(ps) => ps,
            Err(e) => return Ok(Err(e)),
        };
        if signing
            && app_id(self.calling_uid) != app_id(ps.pkg.as_ref().unwrap().uid)
            && self.calling_uid != SYSTEM_UID
        {
            return Ok(Err(Exception::security(
                "May not access signing KeySet of other apps.",
            )));
        }
        let data = ps
            .key_set_data
            .as_ref()
            .ok_or(NotModelled("native package keyset owner unavailable"))?;
        let id = if signing {
            data.proper_signing_key_set
        } else {
            match data
                .defined_key_sets
                .iter()
                .find(|(key, _)| key.as_deref() == alias)
            {
                Some((_, id)) => *id,
                None => {
                    let mut aliases = data.defined_key_sets.iter().collect::<Vec<_>>();
                    aliases.sort_by_key(|(alias, _)| {
                        alias
                            .as_deref()
                            .map_or(0, super::super::parse::parcel::java_hash)
                    });
                    let aliases = aliases
                        .iter()
                        .map(|(alias, id)| format!("{}={id}", alias.as_deref().unwrap_or("null")))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Ok(Err(Exception::new(
                        EX_ILLEGAL_ARGUMENT,
                        format!(
                            "Unknown KeySet alias: {}, aliases = {{{aliases}}}",
                            alias.unwrap()
                        ),
                    )));
                }
            }
        };
        let pool = self
            .state
            .key_sets
            .as_ref()
            .ok_or(NotModelled("native keyset pool unavailable"))?;
        if !pool.key_sets.iter().any(|(key, _)| *key == id) {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "null value for KeySet IBinder token",
            )));
        }
        let tokens = self
            .state
            .system
            .key_set_tokens
            .as_ref()
            .ok_or(NotModelled("native keyset Binder owner unavailable"))?;
        Ok(Ok(Some(super::super::keysets::KeySet {
            token: Some(tokens.token(id)),
        })))
    }
    fn signed_by_keyset(
        &self,
        name: Option<&str>,
        keyset: Option<&super::super::keysets::KeySet>,
        exact: bool,
    ) -> Thrown<bool> {
        if keyset.is_some_and(|keyset| keyset.token.is_none()) {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "null value for KeySet IBinder token",
            )));
        }
        if apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some() {
            return Ok(Ok(false));
        }
        let (Some(name), Some(keyset)) = (name, keyset) else {
            return Ok(Ok(false));
        };
        let ps = match self.keyset_package(name)? {
            Ok(ps) => ps,
            Err(e) => return Ok(Err(e)),
        };
        let Some(token) = keyset.token else {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "null value for KeySet IBinder token",
            )));
        };
        let tokens = self
            .state
            .system
            .key_set_tokens
            .as_ref()
            .ok_or(NotModelled("native keyset Binder owner unavailable"))?;
        let Some(id) = tokens.id(token) else {
            return Ok(Ok(false));
        };
        let pool = self
            .state
            .key_sets
            .as_ref()
            .ok_or(NotModelled("native keyset pool unavailable"))?;
        let data = ps
            .key_set_data
            .as_ref()
            .ok_or(NotModelled("native package keyset owner unavailable"))?;
        if exact && data.proper_signing_key_set == -1 {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "Package has no KeySet data",
            )));
        }
        let Some((_, test)) = pool.key_sets.iter().find(|(key, _)| *key == id) else {
            return Ok(Ok(false));
        };
        let Some((_, proper)) = pool
            .key_sets
            .iter()
            .find(|(key, _)| *key == data.proper_signing_key_set)
        else {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "Attempt to invoke virtual method on a null object reference",
            )));
        };
        Ok(Ok(test.iter().all(|key| proper.contains(key))
            && (!exact || proper.iter().all(|key| test.contains(key)))))
    }

    fn configured_role(&self, role: super::super::roles::Role) -> Thrown<Option<String>> {
        if role == super::super::roles::Role::SetupWizard && self.calling_uid != 1000 {
            return Ok(Err(Exception::security("Non-system caller")));
        }
        let owner = self
            .state
            .system
            .roles
            .as_ref()
            .ok_or(NotModelled("native configured role owner unavailable"))?;
        Ok(owner.package(role, self))
    }

    fn cross_profile_property(&self, user: i32) -> Thrown<()> {
        let uid = self.calling_uid;
        if user < 0 || user == user_id(uid) || matches!(app_id(uid), ROOT_UID | SYSTEM_UID) {
            return self.enforce_cross_user(user, false, false, "getPropertyAsUser");
        }
        if self.uid_has_permission(uid, INTERACT_ACROSS_USERS_FULL)?
            || self.uid_has_permission(uid, INTERACT_ACROSS_USERS)?
        {
            return Ok(Ok(()));
        }
        let caller = self.user(user_id(uid));
        let target = self.user(user);
        let same = caller.zip(target).is_some_and(|(c, t)| {
            c.profile_group_id >= 0 && c.profile_group_id == t.profile_group_id
        });
        if same && self.uid_has_permission(uid, "android.permission.INTERACT_ACROSS_PROFILES")? {
            return Err(NotModelled("cross-profile property AppOps preflight owner"));
        }
        let profiles = if same {
            " or android.permission.INTERACT_ACROSS_PROFILES"
        } else {
            ""
        };
        Ok(Err(Exception::security(format!(
            "getPropertyAsUser: UID {uid} requires {INTERACT_ACROSS_USERS_FULL} or {INTERACT_ACROSS_USERS}{profiles} to access user {user}."
        ))))
    }

    pub(super) fn package_property(
        &self,
        name: Option<&str>,
        package: Option<&str>,
        class: Option<&str>,
        user: i32,
    ) -> Thrown<Option<Property>> {
        let (Some(name), Some(package)) = (name, package) else {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "null property or package name",
            )));
        };
        if let Err(e) = self.cross_profile_property(user)? {
            return Ok(Err(e));
        }
        let resolved = self.resolve_internal_package_name(package, VERSION_CODE_HIGHEST);
        if self.visible_user_state(&resolved, user)?.is_none() {
            return Ok(Ok(None));
        }
        if let Some(registry) = &self.state.package_registry {
            return Ok(Ok(registry
                .property(name, package, class)
                .cloned()
                .map(Property::checked)
                .transpose()?));
        }
        let Some((_, pkg)) = self.package_of(package) else {
            return Ok(Ok(None));
        };
        if class.is_none() {
            return Ok(Ok(pkg
                .properties
                .iter()
                .flatten()
                .rev()
                .find(|(key, _)| key == name)
                .map(|(_, property)| Property::checked(property.clone()))
                .transpose()?));
        }
        for kind in [1, 4, 2, 3] {
            if let Some(property) = properties(pkg, kind)
                .into_iter()
                .rev()
                .flat_map(|p| p.iter().rev())
                .find(|(key, p)| key == name && p.class_name.as_deref() == class)
                .map(|(_, p)| Property::checked(p.clone()))
                .transpose()?
            {
                return Ok(Ok(Some(property)));
            }
        }
        Ok(Ok(None))
    }

    pub(super) fn query_properties(&self, name: Option<&str>, kind: i32) -> Thrown<Vec<Property>> {
        let Some(name) = name else {
            return Ok(Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "null property name",
            )));
        };
        if !(1..=5).contains(&kind) {
            return Ok(Ok(Vec::new()));
        }
        if let Some(registry) = &self.state.package_registry {
            let mut result = Vec::new();
            for (package, properties) in registry.property_packages(name, kind) {
                let resolved = self.resolve_internal_package_name(package, VERSION_CODE_HIGHEST);
                if self
                    .visible_user_state(&resolved, user_id(self.calling_uid))?
                    .is_none()
                {
                    continue;
                }
                for property in properties {
                    result.push(Property::checked(property.clone())?);
                }
            }
            return Ok(Ok(result));
        }
        let mut packages = self.packages_in_order();
        if packages
            .windows(2)
            .any(|p| info::java_hash(&p[0].name) == info::java_hash(&p[1].name))
        {
            return Err(NotModelled(
                "property package registry collision insertion order",
            ));
        }
        let mut result = Vec::new();
        for ps in packages.drain(..) {
            let Some(pkg) = ps.pkg.as_deref() else {
                continue;
            };
            if self.filtered_including_uninstalled(Some(ps), user_id(self.calling_uid))? {
                continue;
            }
            for group in properties(pkg, kind) {
                for (key, property) in group {
                    if key == name {
                        result.push(Property::checked(property.clone())?);
                    }
                }
            }
        }
        Ok(Ok(result))
    }

    pub(super) fn content_providers(
        &self,
        process: Option<&str>,
        uid: i32,
        flags: i64,
        metadata: Option<&str>,
    ) -> Thrown<Vec<info::ProviderInfo>> {
        let user = if process.is_some() {
            user_id(uid)
        } else {
            user_id(self.calling_uid)
        };
        if let Err(e) = self.enforce_cross_user(user, false, false, "queryContentProviders")? {
            return Ok(Err(e));
        }
        if self.user(user).is_none() {
            return Ok(Ok(Vec::new()));
        }
        let flags = self.update_flags_for_component(flags, user)?;
        let mut registered: Vec<(&PackageState, &AndroidPackage, &super::super::pkg::Provider)> =
            Vec::new();
        for ps in self.state.packages.values() {
            let Some(pkg) = ps.pkg.as_deref() else {
                continue;
            };
            for provider in &pkg.providers {
                if let Some(index) = registered.iter().position(|(_, _, p)| {
                    p.main.component.package_name == provider.main.component.package_name
                        && p.main.component.name == provider.main.component.name
                }) {
                    registered[index] = (ps, pkg, provider);
                } else {
                    registered.push((ps, pkg, provider));
                }
            }
        }
        if let Some(registry) = &self.state.package_registry {
            registered.clear();
            for provider in registry.providers() {
                let (ps, pkg) = self
                    .package_of(&provider.package)
                    .ok_or(NotModelled("registered provider has no current code"))?;
                registered.push((ps, pkg, &provider.value));
            }
        }
        let hash = |p: &super::super::pkg::Provider| {
            info::java_hash(&p.main.component.package_name)
                .wrapping_add(info::java_hash(&p.main.component.name))
        };
        registered.sort_by_key(|(_, _, p)| hash(p));
        if self.state.package_registry.is_none()
            && registered.windows(2).any(|p| hash(p[0].2) == hash(p[1].2))
        {
            return Err(NotModelled("provider registry collision insertion order"));
        }
        let mut result = Vec::new();
        for (ps, pkg, provider) in registered.into_iter().rev() {
            let Some(authority) = provider.authority.as_deref() else {
                continue;
            };
            if self.state.package_registry.is_none() && provider.syncable && authority.contains(';')
            {
                return Err(NotModelled("syncable provider registered authority owner"));
            }
            if let Some(process) = process {
                let owner = provider
                    .main
                    .process_name
                    .as_deref()
                    .ok_or(NotModelled("provider process owner"))?;
                if owner != process || app_id(pkg.uid) != app_id(uid) {
                    continue;
                }
            }
            if let Some(key) = metadata {
                // ParsedComponent.getMetaData exposes an empty Bundle when
                // no manifest metadata was declared; a missing key is a miss.
                let Some(values) = &provider.main.component.meta_data else {
                    continue;
                };
                if !values.0.iter().any(|(name, _)| name == key) {
                    continue;
                }
            }
            let state = user_state(ps, user);
            let component=ComponentName{package:provider.main.component.package_name.clone(),class:provider.main.component.name.clone()};
            if !info::is_enabled_and_matches(ps, &provider.main, flags, user)
                || self.filtered_component(Some(ps), &component, 4, self.calling_uid, user)?
            {
                continue;
            }
            if let Some(info) = info::generate_provider_info(
                &self.target(ps, pkg, &state, user),
                provider,
                flags,
                None,
            ) {
                result.push(info);
            }
        }
        result.sort_by_key(|p| std::cmp::Reverse(p.init_order));
        Ok(Ok(result))
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
        let instrument = if let Some(registry) = &self.state.package_registry {
            registry
                .instruments()
                .into_iter()
                .find(|i| {
                    i.package == component.package && i.value.component.name == component.class
                })
                .map(|i| &i.value)
        } else {
            pkg.instrumentations.iter().rev().find(|i| {
                i.component.name == component.class && i.component.package_name == component.package
            })
        };
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
        if let Some(registry) = &self.state.package_registry {
            registered.clear();
            for instrument in registry.instruments() {
                let (ps, pkg) = self.package_of(&instrument.package).ok_or(NotModelled(
                    "registered instrumentation has no current code",
                ))?;
                registered.push((ps, pkg, &instrument.value));
            }
        }
        let hash = |i: &super::super::pkg::Instrumentation| {
            info::java_hash(&i.component.package_name)
                .wrapping_add(info::java_hash(&i.component.name))
        };
        registered.sort_by_key(|(_, _, i)| hash(i));
        if self.state.package_registry.is_none()
            && registered
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

    fn app_metadata_fd(&self, name: Option<&str>, user: i32) -> Thrown<Option<super::super::app_metadata::Descriptor>> {
        if !self.uid_has_permission(self.calling_uid, "android.permission.GET_APP_METADATA")? {
            return Ok(Err(Exception::security("android.permission.GET_APP_METADATA required")));
        }
        let resolved = name.map(|name| self.resolve_internal_package_name(name, VERSION_CODE_HIGHEST));
        let package = resolved.as_ref().and_then(|name| self.state.packages.get(name));
        if package.is_none() || self.filtered_including_uninstalled(package, user)? { return Ok(Err(name_not_found(name.unwrap_or("null")))); }
        let path = package.unwrap().app_metadata_file_path.as_ref()
            .ok_or(NotModelled("native Settings app metadata file path owner unavailable"))?;
        let Some(path) = path else { return Ok(Ok(None)); };
        let owner = self.state.system.app_metadata_files.as_ref().ok_or(NotModelled("native app metadata filesystem owner unavailable"))?;
        owner.open(path).map(Ok).map_err(|_| NotModelled("native app metadata path mapping or Binder FD export failed"))
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

    pub(crate) fn check_package_permission(
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

    fn sync_providers(&self, mut names: Option<Vec<Option<String>>>, mut infos: Option<Vec<Option<SyncProvider>>>)
        -> Result<pm::QuerySyncProvidersReply<SyncProvider>, NotModelled> {
        if apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some() {
            return Ok(pm::QuerySyncProvidersReply { out_names: names, out_info: infos });
        }
        let safe_mode = self.lifecycle_owner()?.safe_mode();
        let registry = self.state.package_registry.as_ref().ok_or(NotModelled("native provider authority registry unavailable"))?;
        let user = user_id(self.calling_uid);
        let mut added_names = Vec::new(); let mut added_infos = Vec::new();
        for (name, registered) in registry.ordered_authorities().into_iter().rev() {
            if !registered.value.syncable { continue; }
            let Some((package, pkg)) = self.package_of(&registered.package) else { continue; };
            if safe_mode && !package.is.system { continue; }
            let state = user_state(package, user);
            let target = self.target(package, pkg, &state, user);
            let Some(application) = generate_application_info(&target, 0) else { continue; };
            let Some(info) = info::generate_provider_info(&target, &registered.value, 0, Some(Arc::new(application))) else { continue; };
            let component = ComponentName { package: registered.package.clone(), class: registered.value.main.component.name.clone() };
            if self.filtered_component(Some(package), &component, 4, self.calling_uid, user)? { continue; }
            added_names.push(Some(name.into())); added_infos.push(Some(SyncProvider::Native(info)));
        }
        if !added_names.is_empty() {
            names.as_mut().ok_or(NotModelled("original querySyncProviders null output names list"))?.extend(added_names);
        }
        if !added_infos.is_empty() {
            infos.as_mut().ok_or(NotModelled("original querySyncProviders null output provider list"))?.extend(added_infos);
        }
        Ok(pm::QuerySyncProvidersReply { out_names: names, out_info: infos })
    }

    fn archive_owner(&self) -> Result<&super::super::archive::Owner, NotModelled> {
        self.state.system.archive_owner.as_deref().ok_or(NotModelled("native PackageArchiver owner unavailable"))
    }
    fn archived_package(&self, name: Option<&str>, user: i32) -> Thrown<Option<super::super::archive::Package>> {
        let Some(name) = name else { return Ok(Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "packageName is null"))); };
        if let Err(exception) = self.internal_enforce_cross_user(self.calling_uid, user, true, true, "getArchivedPackage")? { return Ok(Err(exception)); }
        let Some(package) = self.state.packages.get(name) else { return Ok(Ok(None)); };
        if self.filtered(Some(package), self.calling_uid, user)? { return Ok(Ok(None)); }
        let state = user_state(package, user);
        if state.archive_state.is_none() && !state.installed { return Ok(Ok(None)); }
        let private = package.setting_flags.ok_or(NotModelled("archived package raw Settings flags unavailable"))?.1;
        let activities = match self.archive_owner()?.activities(package, user) {
            Ok(activities) if !activities.is_empty() => activities,
            _ => return Ok(Err(Exception::illegal_argument("Package does not have a main activity"))),
        };
        let signing = package.signatures.as_ref().map(|details| info::SigningInfo {
            scheme_version: details.scheme_version, signatures: details.signatures.clone(), public_keys: details.public_keys.clone(),
            past_signing_certificates: details.past_signatures.as_ref().map(|past| past.iter().map(|(certificate, _)| certificate.clone()).collect()),
        });
        Ok(Ok(Some(super::super::archive::Package { name: name.into(), signing, version: package.version_code,
            target_sdk: package.target_sdk_version, device_storage: package.is.default_to_device_protected_storage,
            legacy_storage: private & (1 << 29) != 0, fragile: private & (1 << 24) != 0, activities })))
    }
    fn archived_icon(&self, name: Option<&str>, user: Option<i32>, calling_package: Option<&str>) -> Thrown<Option<super::super::instant::Bitmap>> {
        let owner = self.archive_owner()?;
        let overlay = match owner.overlay_enabled(self.calling_uid, calling_package.unwrap_or("null")) {
            Ok(overlay) => overlay, Err(exception) => return Ok(Err(exception)),
        };
        let (Some(name), Some(user)) = (name, user) else { return Ok(Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "package or user is null"))); };
        let Some(package) = self.state.packages.get(name) else { return Ok(Ok(None)); };
        if self.filtered(Some(package), self.calling_uid, user)? { return Ok(Ok(None)); }
        Ok(owner.icon(package, user, overlay))
    }
    fn archivable(&self, name: Option<&str>, user: Option<i32>) -> Thrown<bool> {
        let (Some(name), Some(user)) = (name, user) else { return Ok(Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "package or user is null"))); };
        if let Err(exception) = self.internal_enforce_cross_user(self.calling_uid, user, true, true, "isAppArchivable")? { return Ok(Err(exception)); }
        let Some(package) = self.state.packages.get(name) else { return Ok(Err(name_not_found(name))); };
        if self.filtered(Some(package), self.calling_uid, user)? { return Ok(Err(name_not_found(name))); }
        if package.is.system || package.is.updated_system_app { return Ok(Ok(false)); }
        let owner = self.archive_owner()?;
        match owner.opted_out(info::uid(user, package.app_id), name) { Ok(true) => return Ok(Ok(false)), Err(exception) => return Ok(Err(exception)), _ => {} }
        let installer = package.install_source.update_owner.as_ref().or(package.install_source.installer.as_ref());
        let Some(installer) = installer.filter(|installer| !installer.is_empty()) else { return Ok(Ok(false)); };
        let Some((installer_state, installer_pkg)) = self.package_of(installer) else { return Ok(Ok(false)); };
        let installer_user = user_state(installer_state, user);
        if generate_application_info(&self.target(installer_state, installer_pkg, &installer_user, user), 0).is_none() { return Ok(Ok(false)); }
        if self.calling_uid != 2000 {
            let resolution = super::super::resolve::Resolution::new(Arc::new(self.state.clone()), &apps_filter::Config {
                force_system_packages_queryable: self.state.system.force_system_packages_queryable,
                force_queryable_packages: self.state.system.force_queryable_packages.clone(),
            }).map_err(|_| NotModelled("native archivable installer receiver registry unavailable"))?;
            let receivers = resolution.query_intent_receivers(&super::super::intent::Intent {
                action: Some("android.intent.action.UNARCHIVE_PACKAGE".into()), package: Some(installer.clone()), ..Default::default()
            }, None, 0, user, SYSTEM_UID).map_err(|_| NotModelled("native archivable installer receiver resolution unavailable"))?;
            if receivers.is_empty() { return Ok(Ok(false)); }
        }
        match owner.launcher(name, user) { Ok(activities) => Ok(Ok(!activities.is_empty())), Err(_) => Ok(Ok(false)) }
    }

    pub(crate) fn internal_can_request_installs(&self, name: Option<&str>, caller: i32, user: i32, throw_missing_permission: bool) -> Thrown<bool> {
        let uid = self.package_uid_internal(name.unwrap_or_default(), 0, user, caller)?;
        if caller != uid && !matches!(caller, 0 | SYSTEM_UID) {
            return Ok(Err(Exception::security(format!("Caller uid {caller} does not own package {}", name.unwrap_or("null")))));
        }
        if self.internal_is_instant(name.unwrap_or_default(), user, SYSTEM_UID)? { return Ok(Ok(false)); }
        let Some((_, package)) = self.package_of(name.unwrap_or_default()) else { return Ok(Ok(false)); };
        if package.target_sdk_version < 26 { return Ok(Ok(false)); }
        if !package.requested_permissions.iter().any(|permission| permission == "android.permission.REQUEST_INSTALL_PACKAGES") {
            if throw_missing_permission { return Ok(Err(Exception::security("Need to declare android.permission.REQUEST_INSTALL_PACKAGES to call this api"))); }
            return Ok(Ok(false));
        }
        Ok(self.security_policy()?.install_disabled(name.unwrap_or_default(), uid, user).map(|disabled| !disabled))
    }
    fn device_admin_any_user(&self, name: Option<&str>) -> Thrown<bool> {
        if !self.uid_has_permission(self.calling_uid, "android.permission.MANAGE_USERS")? {
            return Ok(Err(Exception::security("android.permission.MANAGE_USERS permission is required to call this API")));
        }
        if apps_filter::instant_app_package_name(self.state, self.calling_uid)?.is_some()
            && !apps_filter::is_caller_same_app(self.state, name, self.calling_uid)? { return Ok(Ok(false)); }
        let setting_exists = name.is_some_and(|name| self.state.packages.contains_key(name));
        Ok(self.security_policy()?.admin_any_user(name, setting_exists))
    }
    fn package_state_protected(&self, name: Option<&str>, user: i32) -> Thrown<bool> {
        if let Err(exception) = self.internal_enforce_cross_user(self.calling_uid, user, false, true, "isPackageStateProtected")? { return Ok(Err(exception)); }
        if !matches!(app_id(self.calling_uid), 0 | SYSTEM_UID)
            && !self.uid_has_permission(self.calling_uid, "android.permission.MANAGE_DEVICE_ADMINS")? {
            return Ok(Err(Exception::security("Caller must have the android.permission.MANAGE_DEVICE_ADMINS permission.")));
        }
        Ok(Ok(self.security_policy()?.protected(user, name)))
    }

    fn instant_owner(&self) -> Result<&super::super::instant::Owner, NotModelled> {
        self.state.system.instant_registry.as_deref().ok_or(NotModelled("native instant registry owner unavailable"))
    }
    fn instant_android_id(&self, name: Option<&str>, user: i32) -> Thrown<Option<String>> {
        if !self.uid_has_permission(self.calling_uid, "android.permission.ACCESS_INSTANT_APPS")? {
            return Ok(Err(Exception::security("android.permission.ACCESS_INSTANT_APPS required")));
        }
        if let Err(exception) = self.internal_enforce_cross_user(self.calling_uid, user, true, false, "getInstantAppAndroidId")? { return Ok(Err(exception)); }
        match self.is_instant(name.unwrap_or_default(), user)? { Err(exception) => return Ok(Err(exception)), Ok(false) => return Ok(Ok(None)), Ok(true) => {} }
        match self.instant_owner()?.android_id(user, name.unwrap_or_default()) {
            Ok(id) => Ok(Ok(Some(id))), Err(message) => Ok(Err(Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message))),
        }
    }

    fn instant_cookie(&self, name: Option<&str>, user: i32) -> Thrown<Option<Vec<u8>>> {
        if let Err(error) = self.internal_enforce_cross_user(self.calling_uid, user, true, false, "getInstantAppCookie")? { return Ok(Err(error)); }
        if !apps_filter::is_caller_same_app(self.state, name, self.calling_uid)? { return Ok(Ok(None)); }
        let resolved = self.resolve_internal_package_name(name.unwrap_or_default(), VERSION_CODE_HIGHEST);
        let Some((_, package)) = self.package_of(&resolved) else { return Ok(Ok(None)); };
        self.instant_owner()?.cookie(user, &package.package_name).map(Ok)
            .map_err(|_| NotModelled("native instant cookie disk identity unavailable"))
    }
    fn set_instant_cookie(&self, name: Option<&str>, cookie: Option<Vec<u8>>, user: i32) -> Thrown<bool> {
        if let Err(error) = self.internal_enforce_cross_user(self.calling_uid, user, true, true, "setInstantAppCookie")? { return Ok(Err(error)); }
        if !apps_filter::is_caller_same_app(self.state, name, self.calling_uid)? { return Ok(Ok(false)); }
        let resolved = self.resolve_internal_package_name(name.unwrap_or_default(), VERSION_CODE_HIGHEST);
        let Some((_, package)) = self.package_of(&resolved) else { return Ok(Ok(false)); };
        self.instant_owner()?.set_cookie(user, package, cookie).map(Ok)
            .map_err(|_| NotModelled("native instant cookie signing or disk owner unavailable"))
    }
    fn instant_permission(&self, user: i32, operation: &str) -> Thrown<()> {
        if !self.internal_can_view_instant(self.calling_uid, user)?
            && !self.uid_has_permission(self.calling_uid, "android.permission.ACCESS_INSTANT_APPS")? {
            return Ok(Err(Exception::security(format!("{operation}: requires android.permission.ACCESS_INSTANT_APPS"))));
        }
        self.internal_enforce_cross_user(self.calling_uid, user, true, false, operation)
    }
    fn instant_icon(&self, name: Option<&str>, user: i32) -> Thrown<Option<super::super::instant::Bitmap>> {
        if let Err(error) = self.instant_permission(user, "getInstantAppIcon")? { return Ok(Err(error)); }
        self.instant_owner()?.icon(user, name.unwrap_or("null")).map(Ok)
            .map_err(|_| NotModelled("native instant icon disk or codec owner unavailable"))
    }
    fn instant_apps(&self, user: i32) -> Thrown<Option<Vec<super::super::instant::App>>> {
        if let Err(error) = self.instant_permission(user, "getEphemeralApplications")? { return Ok(Err(error)); }
        let owner = self.instant_owner()?;
        let mut apps = Vec::new();
        for package in self.packages_in_order() {
            let state = user_state(package, user);
            if !state.instant_app || !state.installed { continue; }
            let Some(pkg) = package.pkg.as_deref() else { continue; };
            let application = generate_application_info(&self.target(package, pkg, &state, user), 0);
            apps.push(super::super::instant::App { package: None, label: None,
                requested: pkg.requested_permissions.iter().cloned().map(Some).collect(),
                granted: state.granted_permissions.iter().cloned().map(Some).collect(), application });
        }
        match owner.uninstalled(user) {
            Ok(uninstalled) => apps.extend(uninstalled),
            Err(message) => {
                let code = if message == "null InstantAppInfo in original uninstalled metadata list" {
                    aim_binder_host::parcel::EX_NULL_POINTER
                } else { aim_binder_host::parcel::EX_ILLEGAL_STATE };
                return Ok(Err(Exception::new(code, message)));
            }
        }
        Ok(Ok((!apps.is_empty()).then_some(apps)))
    }

    pub(crate) fn internal_can_view_instant(&self, filter_uid: i32, user: i32) -> Result<bool, NotModelled> {
        if filter_uid < 10000 || self.uid_has_permission(self.calling_uid, "android.permission.ACCESS_INSTANT_APPS")? { return Ok(true); }
        if self.uid_has_permission(self.calling_uid, "android.permission.VIEW_INSTANT_APPS")? {
            if let Some(home) = super::preferred::default_home_for_instant(self, user)? {
                if apps_filter::is_caller_same_app(self.state, Some(&home.package), filter_uid)? {
                    return Ok(true);
                }
            }
            let roles = self.state.system.roles.as_ref()
                .ok_or(NotModelled("native app-prediction instant visibility owner unavailable"))?;
            let prediction = roles.package(super::super::roles::Role::AppPrediction, self)
                .map_err(|_| NotModelled("native app-prediction instant visibility owner failed"))?;
            return prediction.as_deref().map(|name| apps_filter::is_caller_same_app(self.state, Some(name), filter_uid))
                .transpose().map(|same| same.unwrap_or(false));
        }
        Ok(false)
    }
    pub(crate) fn internal_is_instant(&self, name: &str, user: i32, mut filter_uid: i32) -> Result<bool, NotModelled> {
        if apps_filter::is_isolated(filter_uid) {
            filter_uid = self.state.system.isolated_owners.iter().find(|(isolated, _)| *isolated == filter_uid)
                .map(|(_, owner)| *owner).ok_or(NotModelled("isolated instant caller owner"))?;
        }
        let Some(package) = self.state.packages.get(name) else { return Ok(false); };
        if apps_filter::is_caller_same_app(self.state, Some(name), filter_uid)?
            || self.internal_can_view_instant(filter_uid, user)? { return Ok(user_state(package, user).instant_app); }
        let access = self.state.system.instant_access.as_ref()
            .ok_or(NotModelled("native instant app access-grant owner"))?;
        Ok(access.granted(user, app_id(filter_uid), package.app_id) && user_state(package, user).instant_app)
    }
    pub(crate) fn internal_check_uid_signatures_all_users(&self, uid1: i32, uid2: i32) -> Thrown<i32> {
        self.check_uid_signatures_captured(uid1, uid2, true)
    }

    fn is_instant(&self, name: &str, user: i32) -> Thrown<bool> {
        if let Err(e) = self.enforce_full_cross_user(user, "isInstantApp")? { return Ok(Err(e)); }
        self.internal_is_instant(name, user, self.calling_uid).map(Ok)
    }

    fn can_view_instant_apps(&self) -> Result<bool, NotModelled> {
        self.internal_can_view_instant(self.calling_uid, user_id(self.calling_uid))
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

    pub(crate) fn check_uid_signatures_captured(
        &self,
        uid1: i32,
        uid2: i32,
        all_users: bool,
    ) -> Thrown<i32> {
        let users = if all_users {
            [user_id(uid1), user_id(uid2)]
        } else {
            [user_id(self.calling_uid); 2]
        };
        if all_users {
            for user in users {
                if let Err(error) =
                    self.enforce_cross_user(user, false, false, "checkUidSignaturesForAllUsers")?
                {
                    return Ok(Err(error));
                }
            }
        }
        let (s1, s2) = (
            self.uid_signatures_at(uid1, users[0])?,
            self.uid_signatures_at(uid2, users[1])?,
        );
        Ok(Ok(match (s1, s2) {
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
        }))
    }
    fn uid_signatures(
        &self,
        uid: i32,
    ) -> Result<Option<super::super::pkg::SigningDetails>, NotModelled> {
        self.uid_signatures_at(uid, user_id(self.calling_uid))
    }
    fn uid_signatures_at(
        &self,
        uid: i32,
        user: i32,
    ) -> Result<Option<super::super::pkg::SigningDetails>, NotModelled> {
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

pub(crate) struct Library(pub(crate) super::super::model::SharedLibrary);
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

#[derive(Clone, Debug)]
pub(super) struct Property(pub(super) super::super::pkg::Property, PropertyWireValue);
#[derive(Clone, Debug)]
enum PropertyWireValue {
    Bool(bool),
    Float(f32),
    Int(i32),
    Resource(i32),
    String(Option<String>),
}
impl Property {
    fn checked(raw: super::super::pkg::Property) -> Result<Self, NotModelled> {
        use super::super::pkg::PropertyValue;
        let value = match &raw.value {
            PropertyValue::Bool(v) => PropertyWireValue::Bool(*v),
            PropertyValue::Float(v) => PropertyWireValue::Float(*v),
            PropertyValue::Int(v) => PropertyWireValue::Int(*v),
            PropertyValue::Resource(v) => PropertyWireValue::Resource(*v),
            PropertyValue::String(v) => PropertyWireValue::String(v.clone()),
            PropertyValue::Unknown(_) => return Err(NotModelled("unknown parsed property type")),
        };
        Ok(Self(raw, value))
    }
}
impl aim_service_aidl::WriteParcelable for Property {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(self.0.name.as_deref());
        p.write_i32(match self.1 {
            PropertyWireValue::Bool(_) => 1,
            PropertyWireValue::Float(_) => 2,
            PropertyWireValue::Int(_) => 3,
            PropertyWireValue::Resource(_) => 4,
            PropertyWireValue::String(_) => 5,
        });
        p.write_string16(self.0.package_name.as_deref());
        p.write_string16(self.0.class_name.as_deref());
        match &self.1 {
            PropertyWireValue::Bool(v) => p.write_bool(*v),
            PropertyWireValue::Float(v) => p.write_f32(*v),
            PropertyWireValue::Int(v) | PropertyWireValue::Resource(v) => p.write_i32(*v),
            PropertyWireValue::String(v) => p.write_string16(v.as_deref()),
        }
    }
}
fn properties(pkg: &AndroidPackage, kind: i32) -> Vec<&[(String, super::super::pkg::Property)]> {
    match kind {
        1 => pkg
            .activities
            .iter()
            .filter_map(|p| p.main.component.properties.as_deref())
            .collect(),
        2 => pkg
            .receivers
            .iter()
            .filter_map(|p| p.main.component.properties.as_deref())
            .collect(),
        3 => pkg
            .services
            .iter()
            .filter_map(|p| p.main.component.properties.as_deref())
            .collect(),
        4 => pkg
            .providers
            .iter()
            .filter_map(|p| p.main.component.properties.as_deref())
            .collect(),
        5 => pkg.properties.as_deref().into_iter().collect(),
        _ => Vec::new(),
    }
}
struct EmptyListSlice;
impl aim_service_aidl::WriteParcelable for EmptyListSlice {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(0);
    }
}

struct SuspensionExtras(Parcel);
impl aim_service_aidl::WriteParcelable for SuspensionExtras {
    fn write_to(&self, p: &mut Parcel) {
        p.write_raw(self.0.data(), self.0.objects());
    }
}

struct InstantComponent(ComponentName);
impl aim_service_aidl::WriteParcelable for InstantComponent {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some(&self.0.package));
        p.write_string16(Some(&self.0.class));
    }
}

fn name_not_found(name: &str) -> Exception {
    let mut payload = Parcel::new(); payload.write_string16(Some("android.os.ParcelableException"));
    payload.write_string16(Some("android.content.pm.PackageManager$NameNotFoundException")); payload.write_string16(Some(name));
    Exception::parcelable(Some(&format!("android.content.pm.PackageManager$NameNotFoundException: {name}")), &payload)
        .expect("pure NameNotFoundException payload owns no Binder capability")
}

enum SyncProvider { Original(Parcel), Native(info::ProviderInfo) }
impl ReadParcelable for SyncProvider {
    fn read_from(reader: &mut Reader<'_>) -> ParcelResult<Self> {
        let start = reader.position(); super::super::reply::provider_info(reader)?;
        let (bytes, objects) = reader.since(start); let mut body = Parcel::new(); body.write_raw(bytes, &objects);
        Ok(Self::Original(body))
    }
}
impl aim_service_aidl::WriteParcelable for SyncProvider {
    fn write_to(&self, parcel: &mut Parcel) {
        match self { Self::Original(body) => parcel.write_raw_files(body.data(), body.objects(), body.files()),
            Self::Native(info) => info.write_to(parcel) }
    }
}
