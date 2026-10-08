//! System-only internal PMS operations. Native publication remains owned by System.
use aim_binder_host::{local::{Call, Reply, Service}, parcel::{Exception, Parcel, EX_ILLEGAL_STATE, UNKNOWN_TRANSACTION}};
use aim_service_aidl::dev_aim_server_ipackageinternalhost as api;
use std::sync::{Arc, Weak};
use crate::system::System;

pub struct Endpoint { system: Weak<System>, bridge: Arc<super::bootstrap::Bridge> }
impl Endpoint { pub fn new(system: Weak<System>, bridge: Arc<super::bootstrap::Bridge>) -> Self { Self { system, bridge } } }
impl Service for Endpoint {
    fn descriptor(&self) -> &str { api::DESCRIPTOR }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if !api::METHODS.iter().any(|(code,_)| *code == call.code) { return Err(UNKNOWN_TRANSACTION); }
        let position = call.data.position();
        call.data.enforce_interface(api::DESCRIPTOR)?;
        call.data.set_position(position);
        if call.sender_euid != crate::SYSTEM_UID {
            let mut reply = Parcel::new();
            reply.write_exception(&Exception::security("internal package host serves system UID only"));
            return Ok(reply);
        }
        let mut reply = Parcel::new();
        let result: Result<(), Exception> = (|| {
            let system = self.system.upgrade().ok_or_else(|| Exception::new(EX_ILLEGAL_STATE,"internal package host lifetime ended"))?;
            system.check_package_bootstrap(&self.bridge)?;
            match call.code {
                api::RESERVE_MUTATION => {
                    api::ReserveMutation::read(&mut call.data).map_err(|status| Exception::new(EX_ILLEGAL_STATE, format!("mutation reservation request: {status}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("mutation reservation trailing bytes")); }
                    let binder = system.reserve_package_mutation()?;
                    api::write_reserve_mutation_reply(&mut reply, Some(binder));
                }
                api::SET_KEEP_UNINSTALLED_PACKAGES => {
                    let a = api::SetKeepUninstalledPackages::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal setKeepUninstalledPackages request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    let (_, capture) = system.capture_package_scan_and_queries()?;
                    let capture = capture.ok_or_else(|| Exception::new(EX_ILLEGAL_STATE,"internal package capture unavailable"))?;
                    let resolver = crate::package::resolve::Resolver::default();
                    let resolution = resolver.resolution(capture.state()).map_err(|error| Exception::new(EX_ILLEGAL_STATE,format!("internal resolution owner: {error:?}")))?;
                    let query = crate::package::query::Query { state: capture.state(), filter: &resolution.apps_filter, calling_uid: a.calling_uid };
                    system.set_native_keep_uninstalled_packages(&query,a.packages)?;
                    api::write_set_keep_uninstalled_packages_reply(&mut reply);
                }
                api::REMOVE_ALL_NON_SYSTEM_PACKAGE_SUSPENSIONS => {
                    let a = api::RemoveAllNonSystemPackageSuspensions::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal removeAllNonSystemPackageSuspensions request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_remove_all_non_system_package_suspensions(a.user_id,a.calling_uid,a.calling_pid)?;
                    api::write_remove_all_non_system_package_suspensions_reply(&mut reply);
                }
                api::REMOVE_NON_SYSTEM_PACKAGE_SUSPENSIONS => {
                    let a = api::RemoveNonSystemPackageSuspensions::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal removeNonSystemPackageSuspensions request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_remove_non_system_package_suspensions(a.package_name,a.user_id,a.calling_uid,a.calling_pid)?;
                    api::write_remove_non_system_package_suspensions_reply(&mut reply);
                }
                api::REMOVE_DISTRACTING_PACKAGE_RESTRICTIONS => {
                    let a = api::RemoveDistractingPackageRestrictions::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal removeDistractingPackageRestrictions request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_remove_distracting_package_restrictions(a.package_name,a.user_id,a.calling_uid,a.calling_pid)?;
                    api::write_remove_distracting_package_restrictions_reply(&mut reply);
                }
                api::REMOVE_ALL_DISTRACTING_PACKAGE_RESTRICTIONS => {
                    let a = api::RemoveAllDistractingPackageRestrictions::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal removeAllDistractingPackageRestrictions request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_remove_all_distracting_package_restrictions(a.user_id,a.calling_uid,a.calling_pid)?;
                    api::write_remove_all_distracting_package_restrictions_reply(&mut reply);
                }
                api::FLUSH_PACKAGE_RESTRICTIONS => {
                    let a = api::FlushPackageRestrictions::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal flushPackageRestrictions request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    let (_, capture) = system.capture_package_scan_and_queries()?;
                    let capture = capture.ok_or_else(|| Exception::new(EX_ILLEGAL_STATE,"internal package capture unavailable"))?;
                    let resolver = crate::package::resolve::Resolver::default();
                    let resolution = resolver.resolution(capture.state()).map_err(|error| Exception::new(EX_ILLEGAL_STATE,format!("internal resolution owner: {error:?}")))?;
                    let query = crate::package::query::Query { state: capture.state(), filter: &resolution.apps_filter, calling_uid: a.calling_uid };
                    system.flush_native_package_restrictions(&query,a.user_id,&capture)?;
                    api::write_flush_package_restrictions_reply(&mut reply);
                }
                api::SET_PACKAGES_SUSPENDED_BY_ADMIN => {
                    let a = api::SetPackagesSuspendedByAdmin::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal setPackagesSuspendedByAdmin request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    let value = system.internal_set_packages_suspended_by_admin(a.user_id,a.packages,a.suspended,a.calling_uid,a.calling_pid)?;
                    api::write_set_packages_suspended_by_admin_reply(&mut reply,&value);
                }
                api::ADD_ISOLATED_UID => {
                    let a = api::AddIsolatedUid::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal addIsolatedUid request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_add_isolated_uid(a.isolated_uid,a.owner_uid,a.calling_uid,a.calling_pid)?;
                    api::write_add_isolated_uid_reply(&mut reply);
                }
                api::REMOVE_ISOLATED_UID => {
                    let a = api::RemoveIsolatedUid::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal removeIsolatedUid request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_remove_isolated_uid(a.isolated_uid,a.calling_uid,a.calling_pid)?;
                    api::write_remove_isolated_uid_reply(&mut reply);
                }
                api::NOTIFY_PACKAGE_USE => {
                    let a = api::NotifyPackageUse::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal notifyPackageUse request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_notify_package_use(a.package_name,a.reason,a.calling_uid,a.calling_pid)?;
                    api::write_notify_package_use_reply(&mut reply);
                }
                api::ON_PACKAGE_PROCESS_KILLED_FOR_UNINSTALL => {
                    let a = api::OnPackageProcessKilledForUninstall::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal onPackageProcessKilledForUninstall request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_on_package_process_killed_for_uninstall(a.package_name,a.calling_uid,a.calling_pid)?;
                    api::write_on_package_process_killed_for_uninstall_reply(&mut reply);
                }
                api::FREE_STORAGE => {
                    let a = api::FreeStorage::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal freeStorage request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_free_storage(a.volume_uuid,a.bytes,a.flags,a.calling_uid,a.calling_pid)?;
                    api::write_free_storage_reply(&mut reply);
                }
                api::FREE_ALL_APP_CACHE_ABOVE_QUOTA => {
                    let a = api::FreeAllAppCacheAboveQuota::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal freeAllAppCacheAboveQuota request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_free_all_app_cache_above_quota(a.volume_uuid,a.calling_uid,a.calling_pid)?;
                    api::write_free_all_app_cache_above_quota_reply(&mut reply);
                }
                api::SET_ENABLE_ROLLBACK_CODE => {
                    let a = api::SetEnableRollbackCode::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal setEnableRollbackCode request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_set_enable_rollback_code(a.token,a.code,a.calling_uid,a.calling_pid)?;
                    api::write_set_enable_rollback_code_reply(&mut reply);
                }
                api::FINISH_PACKAGE_INSTALL => {
                    let a = api::FinishPackageInstall::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal finishPackageInstall request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_finish_package_install(a.token,a.did_launch,a.calling_uid,a.calling_pid)?;
                    api::write_finish_package_install_reply(&mut reply);
                }
                api::REMOVE_LEGACY_DEFAULT_BROWSER_PACKAGE_NAME => {
                    let a = api::RemoveLegacyDefaultBrowserPackageName::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal removeLegacyDefaultBrowserPackageName request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    let value = system.internal_remove_legacy_default_browser_package_name(a.user_id,a.calling_uid,a.calling_pid)?;
                    api::write_remove_legacy_default_browser_package_name_reply(&mut reply,&value);
                }
                api::UNINSTALL_APEX => {
                    let a = api::UninstallApex::<crate::package::diagnostics::IntentSender>::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal uninstallApex request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_uninstall_apex(a.package_name,a.version,a.user_id,a.sender,a.flags,a.calling_uid,a.calling_pid)?;
                    api::write_uninstall_apex_reply(&mut reply);
                }
                api::UPDATE_RUNTIME_PERMISSIONS_FINGERPRINT => {
                    let a = api::UpdateRuntimePermissionsFingerprint::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal updateRuntimePermissionsFingerprint request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_update_runtime_permissions_fingerprint(a.user_id,a.calling_uid,a.calling_pid)?;
                    api::write_update_runtime_permissions_fingerprint_reply(&mut reply);
                }
                api::MIGRATE_LEGACY_OBB_DATA => {
                    let a = api::MigrateLegacyObbData::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal migrateLegacyObbData request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_migrate_legacy_obb_data(a.calling_uid,a.calling_pid)?;
                    api::write_migrate_legacy_obb_data_reply(&mut reply);
                }
                api::WRITE_SETTINGS => {
                    let a = api::WriteSettings::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal writeSettings request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_write_settings(a.r#async,a.calling_uid,a.calling_pid)?;
                    api::write_write_settings_reply(&mut reply);
                }
                api::WRITE_PERMISSION_SETTINGS => {
                    let a = api::WritePermissionSettings::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal writePermissionSettings request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_write_permission_settings(a.users,a.r#async,a.calling_uid,a.calling_pid)?;
                    api::write_write_permission_settings_reply(&mut reply);
                }
                api::SET_VISIBILITY_LOGGING => {
                    let a = api::SetVisibilityLogging::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal setVisibilityLogging request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_set_visibility_logging(a.package_name,a.enabled,a.calling_uid,a.calling_pid)?;
                    api::write_set_visibility_logging_reply(&mut reply);
                }
                api::UNSUSPEND_ADMIN_SUSPENDED_PACKAGES => {
                    let a = api::UnsuspendAdminSuspendedPackages::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal unsuspendAdminSuspendedPackages request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_unsuspend_admin_suspended_packages(a.user_id,a.calling_uid,a.calling_pid)?;
                    api::write_unsuspend_admin_suspended_packages_reply(&mut reply);
                }
                api::DELETE_OAT_ARTIFACTS_OF_PACKAGE => {
                    let a = api::DeleteOatArtifactsOfPackage::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal deleteOatArtifactsOfPackage request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    let value = system.internal_delete_oat_artifacts_of_package(a.package_name,a.calling_uid,a.calling_pid)?;
                    api::write_delete_oat_artifacts_of_package_reply(&mut reply,value);
                }
                api::RECONCILE_APPS_DATA => {
                    let a = api::ReconcileAppsData::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal reconcileAppsData request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_reconcile_apps_data(a.user_id,a.flags,a.migrate_apps_data,a.calling_uid,a.calling_pid)?;
                    api::write_reconcile_apps_data_reply(&mut reply);
                }
                api::SET_PACKAGE_STOPPED_STATE => {
                    let a = api::SetPackageStoppedState::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal setPackageStoppedState request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_set_package_stopped_state(a.package_name,a.stopped,a.user_id,a.calling_uid,a.calling_pid)?;
                    api::write_set_package_stopped_state_reply(&mut reply);
                }
                api::NOTIFY_COMPONENT_USED => {
                    let a = api::NotifyComponentUsed::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal notifyComponentUsed request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_notify_component_used(a.package_name,a.user_id,a.caller,a.debug_info,a.calling_uid,a.calling_pid)?;
                    api::write_notify_component_used_reply(&mut reply);
                }
                api::SEND_PACKAGE_RESTARTED_BROADCAST => {
                    let a = api::SendPackageRestartedBroadcast::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal sendPackageRestartedBroadcast request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_send_package_restarted_broadcast(a.package_name,a.uid,a.flags,a.calling_uid,a.calling_pid)?;
                    api::write_send_package_restarted_broadcast_reply(&mut reply);
                }
                api::SEND_PACKAGE_DATA_CLEARED_BROADCAST => {
                    let a = api::SendPackageDataClearedBroadcast::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal sendPackageDataClearedBroadcast request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_send_package_data_cleared_broadcast(a.package_name,a.uid,a.user_id,a.restore,a.instant,a.calling_uid,a.calling_pid)?;
                    api::write_send_package_data_cleared_broadcast_reply(&mut reply);
                }
                api::PRUNE_INSTANT_APPS => {
                    let a = api::PruneInstantApps::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal pruneInstantApps request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_prune_instant_apps(a.calling_uid,a.calling_pid)?;
                    api::write_prune_instant_apps_reply(&mut reply);
                }
                api::SHUTDOWN => {
                    let a = api::Shutdown::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal shutdown request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_shutdown(a.calling_uid,a.calling_pid)?;
                    api::write_shutdown_reply(&mut reply);
                }
                api::GET_HISTORICAL_SESSIONS => {
                    let a = api::GetHistoricalSessions::read(&mut call.data).map_err(|s| Exception::new(EX_ILLEGAL_STATE,format!("internal getHistoricalSessions request: {s}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    let value = system.internal_get_historical_sessions(a.user_id,a.calling_uid,a.calling_pid)?;
                    let entries = value.into_iter().map(Some).collect::<Vec<_>>();
                    api::write_get_historical_sessions_reply(&mut reply,Some(&entries));
                }
                api::GRANT_IMPLICIT_ACCESS => {
                    let a = api::GrantImplicitAccess::<crate::package::intent::Intent>::read(&mut call.data)
                        .map_err(|status| Exception::new(EX_ILLEGAL_STATE,format!("implicit access request: {status}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    system.internal_grant_implicit_access(a.user_id,a.intent,a.recipient_app_id,a.visible_uid,
                        a.direct,a.retain_on_update,a.calling_uid,a.calling_pid)?;
                    api::write_grant_implicit_access_reply(&mut reply);
                }
                api::GET_LEGACY_RUNTIME_PERMISSIONS_STATE_RECORD => {
                    let a = api::GetLegacyRuntimePermissionsStateRecord::read(&mut call.data)
                        .map_err(|status| Exception::new(EX_ILLEGAL_STATE,format!("legacy runtime state request: {status}")))?;
                    if call.data.remaining() != 0 { return Err(Exception::illegal_argument("internal package host trailing data")); }
                    if a.calling_uid < 0 || a.calling_pid < 0 { return Err(Exception::illegal_argument("invalid original caller identity")); }
                    let value = legacy_runtime_record(&system,a.user_id)?;
                    api::write_get_legacy_runtime_permissions_state_record_reply(&mut reply,&Some(value));
                }
                api::ARCHIVE_PACKAGE => {
                    let a=api::ArchivePackage::<crate::package::installer::preapproval::IntentSender>::read(&mut call.data)
                        .map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("archive internal request: {status}")))?;
                    if call.data.remaining()!=0 {return Err(Exception::illegal_argument("archive trailing data"));}
                    if a.calling_uid<0 || a.calling_pid<0 {return Err(Exception::illegal_argument("invalid archive caller"));}
                    let null=||Exception::new(aim_binder_host::parcel::EX_NULL_POINTER,"archive input is null");
                    let action=crate::package::installer::endpoint::ArchiveAction::Archive {
                        package:a.package_name.ok_or_else(null)?, caller:a.caller_package.ok_or_else(null)?, flags:a.flags,
                        receiver:a.receiver.ok_or_else(null)?, user:a.user_id,
                    };
                    let owner=system.package_installer_native_owner()?;
                    crate::package::installer::endpoint::Owners::archive(&*owner,a.calling_uid as u32,a.calling_pid,action)?;
                    api::write_archive_package_reply(&mut reply);
                }
                api::UNARCHIVE_PACKAGE => {
                    let a=api::UnarchivePackage::<crate::package::installer::preapproval::IntentSender>::read(&mut call.data)
                        .map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("unarchive internal request: {status}")))?;
                    if call.data.remaining()!=0 {return Err(Exception::illegal_argument("unarchive trailing data"));}
                    if a.calling_uid<0 || a.calling_pid<0 {return Err(Exception::illegal_argument("invalid unarchive caller"));}
                    let null=||Exception::new(aim_binder_host::parcel::EX_NULL_POINTER,"unarchive input is null");
                    let action=crate::package::installer::endpoint::ArchiveAction::Unarchive {
                        package:a.package_name.ok_or_else(null)?, caller:a.caller_package.ok_or_else(null)?,
                        receiver:a.receiver.ok_or_else(null)?, user:a.user_id, show_confirmation:a.show_confirmation,
                    };
                    let owner=system.package_installer_native_owner()?;
                    crate::package::installer::endpoint::Owners::archive(&*owner,a.calling_uid as u32,a.calling_pid,action)?;
                    api::write_unarchive_package_reply(&mut reply);
                }
                api::CLEAR_ARCHIVE_STATE => {
                    let a=api::ClearArchiveState::read(&mut call.data)
                        .map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("clear archive internal request: {status}")))?;
                    if call.data.remaining()!=0 {return Err(Exception::illegal_argument("clear archive trailing data"));}
                    system.internal_clear_archive_state(a.package_name,a.user_id,a.calling_uid,a.calling_pid)?;
                    api::write_clear_archive_state_reply(&mut reply);
                }
                api::GET_ACTIVE_UNARCHIVE_SESSION => {
                    let a=api::GetActiveUnarchiveSession::read(&mut call.data)
                        .map_err(|status|Exception::new(EX_ILLEGAL_STATE,format!("active unarchive session request: {status}")))?;
                    if call.data.remaining()!=0 {return Err(Exception::illegal_argument("active unarchive session trailing data"));}
                    let value=system.internal_active_unarchive_session(a.package_name,a.user_id,a.calling_uid,a.calling_pid)?;
                    api::write_get_active_unarchive_session_reply(&mut reply,value.as_ref());
                }
                _ => return Err(Exception::new(EX_ILLEGAL_STATE,"internal package dispatch mismatch")),
            }
            Ok(())
        })();
        if let Err(error) = result { reply = Parcel::new(); reply.write_exception(&error); }
        Ok(reply)
    }
}

fn legacy_runtime_record(system:&System,user:i32)->Result<Vec<u8>,Exception> {
    let capture=system.capture_package_queries()?;
    system.with_runtime_permission_metadata(&capture, |metadata| {
        let users=capture.state().users.keys().copied().collect::<std::collections::BTreeSet<_>>();
        legacy_runtime_record_from_owners(capture.scan().owner(),metadata,&users,user)
    })?
}

/// Same original runtime persistence envelope for constructor and full owners.
/// The caller supplies its real retained scan, metadata and UM user inventory.
pub(crate) fn legacy_runtime_record_from_owners(scan:&crate::package::scan::SigningScan,
    metadata:&crate::package::owner::runtime_metadata::State,users:&std::collections::BTreeSet<i32>,user:i32)->Result<Vec<u8>,Exception>{
    if user<0||!users.contains(&user){return Err(Exception::illegal_argument("legacy runtime permission user unavailable"));}
        let mut parcel=Parcel::new(); parcel.write_i32(1); parcel.write_i32(metadata.version(user));
        parcel.write_string16(metadata.fingerprint(user));
        let mut packages=Vec::new();
        for package in &scan.settings.packages {
            if package.shared_user {continue;}
            let state=scan.legacy_permissions(&package.name,false)
                .map_err(|error|Exception::new(EX_ILLEGAL_STATE,error))?
                .ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"legacy package permission owner unavailable"))?;
            let permissions=state.user(user).map(|state|state.permissions.clone()).unwrap_or_default();
            if permissions.is_empty() && !package.install_permissions_fixed {continue;}
            packages.push((package.name.clone(),permissions));
        }
        write_permission_owners(&mut parcel,&packages)?;
        let mut groups=Vec::new();
        for group in &scan.settings.shared_users {
            let state=scan.shared_legacy_permissions(&group.name)
                .map_err(|error|Exception::new(EX_ILLEGAL_STATE,error))?
                .ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"legacy shared permission owner unavailable"))?;
            groups.push((group.name.clone(),state.user(user).map(|state|state.permissions.clone()).unwrap_or_default()));
        }
        write_permission_owners(&mut parcel,&groups)?;
        Ok(parcel.data().to_vec())
}
fn write_permission_owners(parcel:&mut Parcel,owners:&[(String,Vec<crate::package::owner::legacy_permissions::Permission>)])->Result<(),Exception> {
    parcel.write_i32(owners.len() as i32);
    for (name,permissions) in owners {
        parcel.write_string16(Some(name)); parcel.write_i32(permissions.len() as i32);
        for permission in permissions {
            let name=permission.name.as_deref().ok_or_else(||Exception::new(EX_ILLEGAL_STATE,"legacy permission has no original name"))?;
            parcel.write_string16(Some(name)); parcel.write_bool(permission.granted); parcel.write_i32(permission.flags);
        }
    }
    Ok(())
}
