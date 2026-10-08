//! A system-only lease on computed queries of one immutable package capture.
use super::query_state::Capture;
use crate::package::{query::Query};
use aim_binder_host::{local::LocalProcess, parcel::Binder};
use aim_binder_host::{
    local::{Call, Reply, Service},
    parcel::{
        BAD_VALUE, EX_ILLEGAL_STATE, EX_UNSUPPORTED_OPERATION, Exception, Parcel,
        UNKNOWN_TRANSACTION,
    },
};
use aim_service_aidl::{
    android_content_pm_ipackagemanager as pm, dev_aim_server_ipackagecomputer as api,
};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};

pub(crate) struct Computer {
    capture: Arc<Mutex<Option<Arc<Capture>>>>,
    process: Weak<LocalProcess>,
    query_binders: Mutex<BTreeMap<(i32, i32), Binder>>,
    uid_registry: Mutex<Option<Vec<u8>>>,
}
impl Computer {
    pub(crate) fn new(capture: Arc<Capture>, process: Weak<LocalProcess>) -> Self {
        Self {
            capture: Arc::new(Mutex::new(Some(capture))),
            process,
            query_binders: Mutex::new(BTreeMap::new()),
            uid_registry: Mutex::new(None),
        }
    }
}
struct FrozenBinder(Binder);
impl aim_service_aidl::WriteParcelable for FrozenBinder {
    fn write_to(&self, p: &mut Parcel) { p.write_binder(Some(self.0)); }
}

struct FrozenComponent(crate::package::intent::ComponentName);
impl aim_service_aidl::WriteParcelable for FrozenComponent {
    fn write_to(&self, p: &mut Parcel) { p.write_string16(Some(&self.0.package)); p.write_string16(Some(&self.0.class)); }
}

struct LibraryUser(String, i64);
impl aim_service_aidl::WriteParcelable for LibraryUser {
    fn write_to(&self, p: &mut Parcel) { p.write_string16(Some(&self.0)); p.write_i64(self.1); }
}

// ProcessInfo/ForInternedStringArraySet, pinned AOSP (Apache 2.0).
struct ProcessInfo(crate::package::pkg::Process);
impl aim_service_aidl::WriteParcelable for ProcessInfo {
    fn write_to(&self, p: &mut Parcel) {
        let value = &self.0;
        p.write_i32(2 | if value.use_embedded_dex { 32 } else { 0 });
        p.write_string16(value.name.as_deref());
        let mut denied = value.denied_permissions.clone();
        denied.sort_by_key(|name| crate::package::info::java_hash(name));
        p.write_i32(denied.len() as i32);
        for name in &denied { p.write_string16(Some(name)); }
        p.write_i32(value.gwp_asan_mode);
        p.write_i32(value.memtag_mode);
        p.write_i32(value.native_heap_zero_initialized);
    }
}

impl Service for Computer {
    fn descriptor(&self) -> &str {
        api::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if !api::METHODS.iter().any(|(code, _)| *code == call.code) {
            return Err(UNKNOWN_TRANSACTION);
        }
        let position = call.data.position();
        call.data.enforce_interface(api::DESCRIPTOR)?;
        call.data.set_position(position);
        if call.sender_euid != crate::SYSTEM_UID {
            let mut reply = Parcel::new();
            reply.write_exception(&Exception::security(
                "package computer serves system UID only",
            ));
            return Ok(reply);
        }
        if matches!(call.code,api::GET_NATIVE_RESOLVER_ACTIVITY|api::IS_NATIVE_RESOLVER_REPLACED) {
            if call.code==api::GET_NATIVE_RESOLVER_ACTIVITY {api::GetNativeResolverActivity::read(&mut call.data)?;}
            else {api::IsNativeResolverReplaced::read(&mut call.data)?;}
            let capture=self.capture.lock().unwrap().clone();let mut reply=Parcel::new();
            let owner=capture.as_ref().and_then(|capture|capture.state().system.resolver_owner.as_ref());
            match owner {
                Some(owner)=>if call.code==api::GET_NATIVE_RESOLVER_ACTIVITY {api::write_get_native_resolver_activity_reply(&mut reply,Some(&owner.activity));}
                    else {api::write_is_native_resolver_replaced_reply(&mut reply,owner.replaced);},
                None=>reply.write_exception(&Exception::new(EX_ILLEGAL_STATE,"native resolver capture owner unavailable")),
            }
            return Ok(reply);
        }
        enum Action {
            Version,
            LegacyDefinitions,
            CrossDomain(api::GetCrossProfileDomainApproval<crate::package::intent::Intent>),
            PreferredSelection(api::SelectPreferredActivity<crate::package::intent::Intent,crate::package::intent::ComponentName>),
            RawProviders(api::QueryRawProvidersRecord),
            RawSync(api::QueryRawSyncProvidersRecord),
            RawDump(api::DumpRawComponentsRecord),
            RawComponents(api::QueryRawComponentsRecord<crate::package::intent::Intent,crate::package::intent::ComponentName>),
            RawProvider(api::QueryRawProvider),
            SyncProviders(api::GetSyncProvidersRecord),
            CrossProfileActivity(api::GetActivityInfoCrossProfile<crate::package::intent::ComponentName>),
            Diagnostic(api::GetDiagnosticRecord),
            PreferredTokens(api::GetPreferredRecordTokens),
            InstantMetadata(api::HasInstantApplicationMetadata),
            PermissionUpgrade(api::IsPermissionUpgradeNeeded),
            ResolveIntent(api::ResolveIntentInternalRecord<crate::package::intent::Intent>),
            ResolveService(api::ResolveServiceInternalRecord<crate::package::intent::Intent>),
            Receivers(api::QueryIntentReceiversInternalRecord<crate::package::intent::Intent>),

            Services(api::QueryIntentServicesInternalRecord<crate::package::intent::Intent>),
            Activities(api::QueryIntentActivitiesInternalRecord<crate::package::intent::Intent>),
            ResolverComponent,
            ApksInApex(api::GetApksInApex),
            Upgrading(api::IsUpgradingFromLowerThan),
            KnownPackages(api::GetKnownPackageNames),
            PlatformSigning,
            CrossUserAllowed(api::HasCrossUserPermission),
            ActivitySupports(api::ActivitySupportsIntentAsUser<crate::package::intent::ComponentName, crate::package::intent::Intent>),
            InstantInstallerInfo,
            FrozenNames,
            FrozenCounts,
            InstantInstallerComponent,
            Provider(api::ResolveContentProvider),
            ActivityInternal(api::GetActivityInfoInternal<crate::package::intent::ComponentName>),
            CanAccessComponent(api::CanAccessComponent<crate::package::intent::ComponentName>),
            Visibility(api::GetVisibilityAllowList),
            LibraryUsers(api::GetSharedLibraryUsers),
            LibraryUsersOptional(api::GetSharedLibraryUsersOptional),
            SetupWizard,
            LookupUid(api::GetPackageLookupUid),
            Processes(api::GetProcessesForUid),
            FilterCandidate(api::ShouldFilterApplication),
            BlockUninstall(api::GetBlockUninstall),
            LibraryRegistry,
            ExplicitUid(api::GetPackageUidWithCaller),
            CallerSame(api::IsCallerSameApp),
            InstantName(api::GetInstantAppPackageName),
            ComponentEnabled(api::GetComponentEnabledSetting<crate::package::intent::ComponentName>),
            InstalledApps(api::GetInstalledApplications),
            InstantInternal(api::IsInstantAppInternal),
            CanView(api::CanViewInstantApps),
            AllUserSignatures(api::CheckUidSignaturesForAllUsers),
            EnforceCross(api::EnforceCrossUserPermission),

            Application(api::GetApplicationInfo),
            Package(api::GetPackageInfo),
            Filter(api::FilterAppAccess),
            Close,
            Forward(u32, Parcel, i32),
            Installer(api::GetInstallerPackageName),
            InternalUid(api::GetPackageUidInternal),
            InternalName(api::ResolveInternalPackageName),
            Same(api::IsSameApp),
            UidAccess(api::FilterUidAccess),
            CanQuery(api::CanQueryPackage),
            VersionedInfo(api::GetPackageInfoInternal),
            FilteredState(api::GetPackageStateFilteredName),
            UidTargetSdk(api::GetUidTargetSdkVersion),
            QueryBinder(api::GetPackageManagerQueryBinder),
            Persistent(api::GetPersistentApplications),
            Startability(api::GetPackageStartability),
            UidRegistryLength,
            UidRegistryChunk(api::GetUidOwnerRegistryChunk),
        }
        let action = match call.code {
            api::GET_CROSS_PROFILE_DOMAIN_APPROVAL => Action::CrossDomain(api::GetCrossProfileDomainApproval::read(&mut call.data)?),
            api::SELECT_PREFERRED_ACTIVITY => Action::PreferredSelection(api::SelectPreferredActivity::read(&mut call.data)?),
            api::QUERY_RAW_PROVIDERS_RECORD => Action::RawProviders(api::QueryRawProvidersRecord::read(&mut call.data)?),
            api::QUERY_RAW_SYNC_PROVIDERS_RECORD => Action::RawSync(api::QueryRawSyncProvidersRecord::read(&mut call.data)?),
            api::DUMP_RAW_COMPONENTS_RECORD => Action::RawDump(api::DumpRawComponentsRecord::read(&mut call.data)?),
            api::QUERY_RAW_COMPONENTS_RECORD => Action::RawComponents(api::QueryRawComponentsRecord::read(&mut call.data)?),
            api::QUERY_RAW_PROVIDER => Action::RawProvider(api::QueryRawProvider::read(&mut call.data)?),
            api::GET_SYNC_PROVIDERS_RECORD => Action::SyncProviders(api::GetSyncProvidersRecord::read(&mut call.data)?),
            api::GET_ACTIVITY_INFO_CROSS_PROFILE => Action::CrossProfileActivity(api::GetActivityInfoCrossProfile::read(&mut call.data)?),
            api::GET_LEGACY_PERMISSION_DEFINITIONS_RECORD => { api::GetLegacyPermissionDefinitionsRecord::read(&mut call.data)?; Action::LegacyDefinitions },
            api::GET_PERSISTENT_APPLICATIONS => Action::Persistent(api::GetPersistentApplications::read(&mut call.data)?),
            api::GET_PACKAGE_STARTABILITY => Action::Startability(api::GetPackageStartability::read(&mut call.data)?),
            api::GET_PACKAGE_UID_WITH_CALLER => Action::ExplicitUid(api::GetPackageUidWithCaller::read(&mut call.data)?),
            api::IS_CALLER_SAME_APP => Action::CallerSame(api::IsCallerSameApp::read(&mut call.data)?),
            api::GET_INSTANT_APP_PACKAGE_NAME => Action::InstantName(api::GetInstantAppPackageName::read(&mut call.data)?),
            api::GET_COMPONENT_ENABLED_SETTING => Action::ComponentEnabled(api::GetComponentEnabledSetting::read(&mut call.data)?),
            api::GET_INSTALLED_APPLICATIONS => Action::InstalledApps(api::GetInstalledApplications::read(&mut call.data)?),
            api::IS_INSTANT_APP_INTERNAL => Action::InstantInternal(api::IsInstantAppInternal::read(&mut call.data)?),
            api::CAN_VIEW_INSTANT_APPS => Action::CanView(api::CanViewInstantApps::read(&mut call.data)?),
            api::CHECK_UID_SIGNATURES_FOR_ALL_USERS => Action::AllUserSignatures(api::CheckUidSignaturesForAllUsers::read(&mut call.data)?),
            api::ENFORCE_CROSS_USER_PERMISSION => Action::EnforceCross(api::EnforceCrossUserPermission::read(&mut call.data)?),
            api::GET_BLOCK_UNINSTALL => Action::BlockUninstall(api::GetBlockUninstall::read(&mut call.data)?),
            api::GET_SHARED_LIBRARY_REGISTRY => { api::GetSharedLibraryRegistry::read(&mut call.data)?; Action::LibraryRegistry },
            api::SHOULD_FILTER_APPLICATION => Action::FilterCandidate(api::ShouldFilterApplication::read(&mut call.data)?),
            api::GET_PROCESSES_FOR_UID => Action::Processes(api::GetProcessesForUid::read(&mut call.data)?),
            api::GET_PACKAGE_LOOKUP_UID => Action::LookupUid(api::GetPackageLookupUid::read(&mut call.data)?),
            api::GET_SETUP_WIZARD_PACKAGE_NAME => { api::GetSetupWizardPackageName::read(&mut call.data)?; Action::SetupWizard },
            api::GET_SHARED_LIBRARY_USERS => Action::LibraryUsers(api::GetSharedLibraryUsers::read(&mut call.data)?),
            api::GET_SHARED_LIBRARY_USERS_OPTIONAL => Action::LibraryUsersOptional(api::GetSharedLibraryUsersOptional::read(&mut call.data)?),
            api::GET_VISIBILITY_ALLOW_LIST => Action::Visibility(api::GetVisibilityAllowList::read(&mut call.data)?),
            api::GET_ACTIVITY_INFO_INTERNAL => Action::ActivityInternal(api::GetActivityInfoInternal::read(&mut call.data)?),
            api::CAN_ACCESS_COMPONENT => Action::CanAccessComponent(api::CanAccessComponent::read(&mut call.data)?),
            api::RESOLVE_CONTENT_PROVIDER => Action::Provider(api::ResolveContentProvider::read(&mut call.data)?),
            api::GET_INSTANT_APP_INSTALLER_COMPONENT => { api::GetInstantAppInstallerComponent::read(&mut call.data)?; Action::InstantInstallerComponent },
            api::GET_FROZEN_PACKAGE_NAMES => { api::GetFrozenPackageNames::read(&mut call.data)?; Action::FrozenNames },
            api::GET_FROZEN_PACKAGE_COUNTS => { api::GetFrozenPackageCounts::read(&mut call.data)?; Action::FrozenCounts },
            api::GET_INSTANT_APP_INSTALLER_INFO_RECORD => { api::GetInstantAppInstallerInfoRecord::read(&mut call.data)?; Action::InstantInstallerInfo },
            api::ACTIVITY_SUPPORTS_INTENT_AS_USER => Action::ActivitySupports(api::ActivitySupportsIntentAsUser::read(&mut call.data)?),
            api::HAS_CROSS_USER_PERMISSION => Action::CrossUserAllowed(api::HasCrossUserPermission::read(&mut call.data)?),
            api::GET_PLATFORM_SIGNING_DETAILS_RECORD => { api::GetPlatformSigningDetailsRecord::read(&mut call.data)?; Action::PlatformSigning },
            api::GET_KNOWN_PACKAGE_NAMES => Action::KnownPackages(api::GetKnownPackageNames::read(&mut call.data)?),
            api::IS_UPGRADING_FROM_LOWER_THAN => Action::Upgrading(api::IsUpgradingFromLowerThan::read(&mut call.data)?),
            api::GET_APKS_IN_APEX => Action::ApksInApex(api::GetApksInApex::read(&mut call.data)?),
            api::GET_RESOLVER_COMPONENT => { api::GetResolverComponent::read(&mut call.data)?; Action::ResolverComponent },
            api::QUERY_INTENT_ACTIVITIES_INTERNAL_RECORD => Action::Activities(api::QueryIntentActivitiesInternalRecord::read(&mut call.data)?),
            api::QUERY_INTENT_SERVICES_INTERNAL_RECORD => Action::Services(api::QueryIntentServicesInternalRecord::read(&mut call.data)?),
            api::RESOLVE_INTENT_INTERNAL_RECORD => Action::ResolveIntent(api::ResolveIntentInternalRecord::read(&mut call.data)?),
            api::RESOLVE_SERVICE_INTERNAL_RECORD => Action::ResolveService(api::ResolveServiceInternalRecord::read(&mut call.data)?),
            api::QUERY_INTENT_RECEIVERS_INTERNAL_RECORD => Action::Receivers(api::QueryIntentReceiversInternalRecord::read(&mut call.data)?),
            api::IS_PERMISSION_UPGRADE_NEEDED => Action::PermissionUpgrade(api::IsPermissionUpgradeNeeded::read(&mut call.data)?),
            api::HAS_INSTANT_APPLICATION_METADATA => Action::InstantMetadata(api::HasInstantApplicationMetadata::read(&mut call.data)?),
            api::GET_PREFERRED_RECORD_TOKENS => Action::PreferredTokens(api::GetPreferredRecordTokens::read(&mut call.data)?),
            api::GET_DIAGNOSTIC_RECORD => Action::Diagnostic(api::GetDiagnosticRecord::read(&mut call.data)?),
            api::GET_VERSION => {
                api::GetVersion::read(&mut call.data)?;
                Action::Version
            }
            api::GET_APPLICATION_INFO => {
                Action::Application(api::GetApplicationInfo::read(&mut call.data)?)
            }
            api::GET_PACKAGE_INFO => Action::Package(api::GetPackageInfo::read(&mut call.data)?),
            api::FILTER_APP_ACCESS => Action::Filter(api::FilterAppAccess::read(&mut call.data)?),
            api::CLOSE => {
                api::Close::read(&mut call.data)?;
                Action::Close
            }
            api::GET_PACKAGE_UID => {
                let a = api::GetPackageUid::read(&mut call.data)?;
                let mut data = Parcel::new();
                pm::GetPackageUid {
                    package_name: a.package_name,
                    flags: a.flags,
                    user_id: a.user_id,
                }
                .write(&mut data);
                Action::Forward(pm::GET_PACKAGE_UID, data, a.calling_uid)
            }
            api::GET_PACKAGES_FOR_UID => {
                let a = api::GetPackagesForUid::read(&mut call.data)?;
                let mut data = Parcel::new();
                pm::GetPackagesForUid { uid: a.uid }.write(&mut data);
                Action::Forward(pm::GET_PACKAGES_FOR_UID, data, a.calling_uid)
            }
            api::GET_NAME_FOR_UID => {
                let a = api::GetNameForUid::read(&mut call.data)?;
                let mut data = Parcel::new();
                pm::GetNameForUid { uid: a.uid }.write(&mut data);
                Action::Forward(pm::GET_NAME_FOR_UID, data, a.calling_uid)
            }
            api::IS_INSTANT_APP => {
                let a = api::IsInstantApp::read(&mut call.data)?;
                let mut data = Parcel::new();
                pm::IsInstantApp {
                    package_name: a.package_name,
                    user_id: a.user_id,
                }
                .write(&mut data);
                Action::Forward(pm::IS_INSTANT_APP, data, a.calling_uid)
            }
            api::GET_TARGET_SDK_VERSION => {
                let a = api::GetTargetSdkVersion::read(&mut call.data)?;
                let mut data = Parcel::new();
                pm::GetTargetSdkVersion {
                    package_name: a.package_name,
                }
                .write(&mut data);
                Action::Forward(pm::GET_TARGET_SDK_VERSION, data, a.calling_uid)
            }
            api::GET_INSTALLER_PACKAGE_NAME => {
                Action::Installer(api::GetInstallerPackageName::read(&mut call.data)?)
            }
            api::GET_PACKAGE_UID_INTERNAL => {
                Action::InternalUid(api::GetPackageUidInternal::read(&mut call.data)?)
            }
            api::RESOLVE_INTERNAL_PACKAGE_NAME => {
                Action::InternalName(api::ResolveInternalPackageName::read(&mut call.data)?)
            }
            api::IS_SAME_APP => Action::Same(api::IsSameApp::read(&mut call.data)?),
            api::FILTER_UID_ACCESS => {
                Action::UidAccess(api::FilterUidAccess::read(&mut call.data)?)
            }
            api::CAN_QUERY_PACKAGE => Action::CanQuery(api::CanQueryPackage::read(&mut call.data)?),
            api::GET_PACKAGE_INFO_INTERNAL => {
                Action::VersionedInfo(api::GetPackageInfoInternal::read(&mut call.data)?)
            }
            api::GET_PACKAGE_STATE_FILTERED_NAME => {
                Action::FilteredState(api::GetPackageStateFilteredName::read(&mut call.data)?)
            }
            api::GET_UID_TARGET_SDK_VERSION => {
                Action::UidTargetSdk(api::GetUidTargetSdkVersion::read(&mut call.data)?)
            }
            api::GET_UID_OWNER_REGISTRY_LENGTH => {
                api::GetUidOwnerRegistryLength::read(&mut call.data)?;
                Action::UidRegistryLength
            }
            api::GET_UID_OWNER_REGISTRY_CHUNK => {
                Action::UidRegistryChunk(api::GetUidOwnerRegistryChunk::read(&mut call.data)?)
            }
            api::GET_PACKAGE_MANAGER_QUERY_BINDER => {
                Action::QueryBinder(api::GetPackageManagerQueryBinder::read(&mut call.data)?)
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        if call.data.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        let mut reply = Parcel::new();
        if matches!(action, Action::Close) {
            self.capture.lock().unwrap().take();
            self.uid_registry.lock().unwrap().take();
            self.query_binders.lock().unwrap().clear();
            api::write_close_reply(&mut reply);
            return Ok(reply);
        }
        let Some(capture) = self.capture.lock().unwrap().clone() else {
            reply.write_exception(&Exception::new(
                EX_ILLEGAL_STATE,
                "package computer is closed",
            ));
            return Ok(reply);
        };
        if matches!(action, Action::LegacyDefinitions) {
            let record=legacy_permission_definitions_record(capture.scan());
            api::write_get_legacy_permission_definitions_record_reply(&mut reply,&Some(record));
            return Ok(reply);
        }
        if matches!(action, Action::Version) {
            api::write_get_version_reply(&mut reply, capture.scan().version() as i64);
            return Ok(reply);
        }
        if let Action::QueryBinder(a) = action {
            if !valid_original_caller(a.calling_uid, a.calling_pid) {
                reply.write_exception(&Exception::illegal_argument(
                    "invalid original query caller",
                ));
                return Ok(reply);
            }
            let Some(process) = self.process.upgrade() else {
                reply.write_exception(&Exception::new(EX_ILLEGAL_STATE, "query process stopped"));
                return Ok(reply);
            };
            let mut binders = self.query_binders.lock().unwrap();
            let binder = *binders
                .entry((a.calling_uid, a.calling_pid))
                .or_insert_with(|| {
                    process.add_service(Arc::new(super::readonly_query::ReadOnlyQuery {
                        capture: self.capture.clone(),
                        uid: a.calling_uid,
                        pid: a.calling_pid,
                                }))
                });
            api::write_get_package_manager_query_binder_reply(&mut reply, Some(binder));
            return Ok(reply);
        }
        if matches!(
            action,
            Action::UidRegistryLength | Action::UidRegistryChunk(_)
        ) {
            let mut registry = self.uid_registry.lock().unwrap();
            if registry.is_none() {
                match super::uid_record::captured(capture.scan()) {
                    Ok(bytes) => *registry = Some(bytes),
                    Err(error) => {
                        reply.write_exception(&Exception::new(EX_ILLEGAL_STATE, error));
                        return Ok(reply);
                    }
                }
            }
            let bytes = registry.as_ref().unwrap();
            match action {
                Action::UidRegistryLength => {
                    api::write_get_uid_owner_registry_length_reply(&mut reply, bytes.len() as i32)
                }
                Action::UidRegistryChunk(a) => {
                    let range = (a.offset >= 0 && a.length > 0 && a.length <= 65536)
                        .then(|| (a.offset as usize).checked_add(a.length as usize))
                        .flatten();
                    match range.and_then(|end| bytes.get(a.offset as usize..end)) {
                        Some(chunk) => api::write_get_uid_owner_registry_chunk_reply(
                            &mut reply,
                            &Some(chunk.to_vec()),
                        ),
                        None => reply.write_exception(&Exception::illegal_argument(
                            "invalid UID registry chunk",
                        )),
                    }
                }
                _ => unreachable!(),
            }
            return Ok(reply);
        }
        let resolution = match capture.resolution() {
            Ok(value) => value,
            Err(error) => return error.reply(),
        };
        let uid = match &action {
            Action::CrossDomain(_) | Action::RawProviders(_) | Action::RawSync(_) | Action::RawDump(_) | Action::RawProvider(_) | Action::LegacyDefinitions | Action::BlockUninstall(_) | Action::LibraryRegistry | Action::SetupWizard | Action::Visibility(_) | Action::InstantInstallerComponent | Action::InstantInstallerInfo | Action::PlatformSigning | Action::Upgrading(_) | Action::ApksInApex(_) | Action::ResolverComponent | Action::PermissionUpgrade(_) | Action::InstantMetadata(_) | Action::PreferredTokens(_) | Action::Diagnostic(_) | Action::FrozenNames | Action::FrozenCounts => crate::SYSTEM_UID as i32,
            Action::ResolveIntent(a) => a.calling_uid,
            Action::ResolveService(a) => a.calling_uid,
            Action::Receivers(a) => a.calling_uid,
            Action::Services(a) => a.calling_uid,
            Action::Activities(a) => a.calling_uid,
            Action::KnownPackages(a) => a.calling_uid,
            Action::CrossUserAllowed(a) => a.calling_uid,
            Action::ActivitySupports(a) => a.calling_uid,
            Action::Provider(a) => a.calling_uid,
            Action::PreferredSelection(a) => a.calling_uid,
            Action::RawComponents(a) => a.calling_uid,
            Action::SyncProviders(a) => a.calling_uid,
            Action::CrossProfileActivity(a) => a.calling_uid,
            Action::ActivityInternal(a) => a.calling_uid,
            Action::CanAccessComponent(a) => a.calling_uid,
            Action::LibraryUsers(a) => a.calling_uid,
            Action::LibraryUsersOptional(a) => a.calling_uid,
            Action::LookupUid(a) => a.calling_uid,
            Action::Processes(a) => a.calling_uid,
            Action::FilterCandidate(a) => a.calling_uid,
            Action::ExplicitUid(a) => a.calling_uid,
            Action::CallerSame(a) => a.calling_uid,
            Action::InstantName(a) => a.calling_uid,
            Action::ComponentEnabled(a) => a.calling_uid,
            Action::InstalledApps(a) => a.calling_uid,
            Action::InstantInternal(a) => a.calling_uid,
            Action::CanView(a) => a.calling_uid,
            Action::AllUserSignatures(a) => a.calling_uid,
            Action::EnforceCross(a) => a.calling_uid,
            Action::Persistent(a) => a.calling_uid,
            Action::Startability(a) => a.calling_uid,
            Action::Application(a) => a.calling_uid,
            Action::Package(a) => a.calling_uid,
            Action::Filter(a) => a.calling_uid,
            Action::Forward(_, _, uid) => *uid,
            Action::Installer(a) => a.calling_uid,
            Action::InternalUid(_) => 1000,
            Action::InternalName(a) => a.calling_uid,
            Action::Same(a) => a.calling_uid,
            Action::UidAccess(a) => a.calling_uid,
            Action::CanQuery(a) => a.calling_uid,
            Action::VersionedInfo(a) => a.calling_uid,
            Action::FilteredState(a) => a.calling_uid,
            Action::UidTargetSdk(_) => crate::SYSTEM_UID as i32,
            _ => unreachable!(),
        };
        if uid < 0 {
            reply.write_exception(&Exception::illegal_argument("negative caller UID"));
            return Ok(reply);
        }
        let invalid_pid = match &action {
            Action::ResolveIntent(a) => !valid_original_caller(uid, a.calling_pid),
            Action::ResolveService(a) => !valid_original_caller(uid, a.calling_pid),
            Action::Receivers(a) => !valid_original_caller(uid, a.calling_pid),
            Action::Services(a) => !valid_original_caller(uid, a.calling_pid),
            Action::Activities(a) => !valid_original_caller(uid, a.calling_pid),
            Action::KnownPackages(a) => !valid_original_caller(uid, a.calling_pid),
            Action::CrossUserAllowed(a) => !valid_original_caller(uid, a.calling_pid),
            Action::ActivitySupports(a) => !valid_original_caller(uid, a.calling_pid),
            Action::Provider(a) => !valid_original_caller(uid, a.calling_pid),
            Action::PreferredSelection(a) => !valid_original_caller(uid, a.calling_pid),
            Action::RawComponents(a) => !valid_original_caller(uid, a.calling_pid),
            Action::SyncProviders(a) => !valid_original_caller(uid, a.calling_pid),
            Action::CrossProfileActivity(a) => !valid_original_caller(uid, a.calling_pid),
            Action::ActivityInternal(a) => !valid_original_caller(uid, a.calling_pid),
            Action::CanAccessComponent(a) => !valid_original_caller(uid, a.calling_pid),
            Action::LibraryUsers(a) => !valid_original_caller(uid, a.calling_pid),
            Action::LibraryUsersOptional(a) => !valid_original_caller(uid, a.calling_pid),
            Action::LookupUid(a) => !valid_original_caller(uid, a.calling_pid),
            Action::Processes(a) => !valid_original_caller(uid, a.calling_pid),
            Action::FilterCandidate(a) => !valid_original_caller(uid, a.calling_pid),
            Action::ExplicitUid(a) => !valid_original_caller(uid, a.calling_pid),
            Action::CallerSame(a) => !valid_original_caller(uid, a.calling_pid),
            Action::InstantName(a) => !valid_original_caller(uid, a.calling_pid),
            Action::ComponentEnabled(a) => !valid_original_caller(uid, a.calling_pid),
            Action::InstalledApps(a) => !valid_original_caller(uid, a.calling_pid),
            Action::InstantInternal(a) => !valid_original_caller(uid, a.calling_pid),
            Action::CanView(a) => !valid_original_caller(uid, a.calling_pid),
            Action::AllUserSignatures(a) => !valid_original_caller(uid, a.calling_pid),
            Action::EnforceCross(a) => !valid_original_caller(uid, a.calling_pid),
            Action::Persistent(a) => !valid_original_caller(uid, a.calling_pid),
            Action::Startability(a) => !valid_original_caller(uid, a.calling_pid),
            _ => false,
        };
        if invalid_pid {
            reply.write_exception(&Exception::illegal_argument("invalid original caller PID"));
            return Ok(reply);
        }
        let query = Query {
            state: capture.state(),
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        let unsupported = |reply: &mut Parcel, error: crate::package::apps_filter::NotModelled| {
            reply.write_exception(&Exception::new(EX_UNSUPPORTED_OPERATION, error.0))
        };
        match action {
            Action::CrossProfileActivity(a) => match query.internal_activity_info_cross_profile(a.component.as_ref(),a.flags,a.user_id) {
                Ok(Ok(value)) => api::write_get_activity_info_cross_profile_reply(&mut reply,value.as_ref()),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply,error),
            },
            Action::SyncProviders(a) => match query.internal_sync_providers(a.safe_mode) {
                Ok(values) => {
                    let mut record=Parcel::new();record.write_i32(values.len() as i32);
                    for (name,provider) in values {
                        record.write_string16(Some(&name));record.write_i32(1);
                        aim_service_aidl::WriteParcelable::write_to(&provider,&mut record);
                    }
                    api::write_get_sync_providers_record_reply(&mut reply,&Some(record.data().to_vec()));
                }
                Err(error) => unsupported(&mut reply,error),
            },
            Action::RawComponents(a) => match capture.raw_components_record(&resolution,a.kind,a.intent.as_ref(),a.resolved_type.as_deref(),a.flags,a.package_name.as_deref(),a.subset.as_deref(),a.user_id) {
                Ok(value)=>api::write_query_raw_components_record_reply(&mut reply,&Some(value)),
                Err(error)=>match error { super::components::RawError::Gap(error)=>unsupported(&mut reply,error),super::components::RawError::Original(error)=>reply.write_exception(&error),super::components::RawError::Transport(status)=>return Err(status) },
            },
            Action::RawProvider(a) => match capture.raw_provider(a.authority.as_deref(),a.flags,a.user_id) {
                Ok(value)=>api::write_query_raw_provider_reply(&mut reply,value.as_ref()),Err(error)=>unsupported(&mut reply,error),
            },
            Action::RawProviders(a) => match capture.raw_providers_record(a.process_name.as_deref(),a.metadata_key.as_deref(),a.uid,a.flags,a.user_id) {
                Ok(value)=>api::write_query_raw_providers_record_reply(&mut reply,&Some(value)),
                Err(error)=>match error{super::components::RawError::Gap(error)=>unsupported(&mut reply,error),super::components::RawError::Original(error)=>reply.write_exception(&error),super::components::RawError::Transport(status)=>return Err(status)},
            },
            Action::RawSync(a) => match capture.raw_sync_providers_record(a.safe_mode,a.user_id) {
                Ok(value)=>api::write_query_raw_sync_providers_record_reply(&mut reply,&Some(value)),Err(error)=>unsupported(&mut reply,error),
            },
            Action::RawDump(a) => match capture.raw_components_dump_record(&resolution,a.kind,a.package_name.as_deref(),a.dump_state.as_deref()) {
                Ok(value)=>api::write_dump_raw_components_record_reply(&mut reply,&Some(value)),Err(error)=>unsupported(&mut reply,error),
            },
            Action::PreferredSelection(a) => match capture.preferred_selection(&query,a.intent.as_ref(),a.resolved_type.as_deref(),a.flags,a.candidates.as_deref(),a.matches.as_deref(),a.always,a.remove_matches,a.query_may_be_filtered,a.device_provisioned,a.user_id) {
                Ok(value)=>api::write_select_preferred_activity_reply(&mut reply,&Some(value)),
                Err(error)=>match error{super::components::RawError::Gap(error)=>unsupported(&mut reply,error),super::components::RawError::Original(error)=>reply.write_exception(&error),super::components::RawError::Transport(status)=>return Err(status)},
            },
            Action::CrossDomain(a) => match resolution.cross_profile_domain_approval(a.intent.as_ref(),a.resolved_type.as_deref(),a.flags,a.source_user_id,a.parent_user_id) {
                Ok(value)=>api::write_get_cross_profile_domain_approval_reply(&mut reply,&value.map(|value|vec![value])),
                Err(error)=>match error{crate::package::resolve::ResolutionError::Original(error)=>reply.write_exception(&error),crate::package::resolve::ResolutionError::NotModelled(error)=>unsupported(&mut reply,error),crate::package::resolve::ResolutionError::UriMatching(error)=>match error.binder_exception(){Some(error)=>reply.write_exception(&error),None=>return Err(UNKNOWN_TRANSACTION)}},
            },
            Action::LegacyDefinitions => unreachable!(),
            Action::ResolverComponent => match capture.state().system.resolver_owner.as_ref() {
                Some(owner) => {
                    let item = &owner.activity.info.item;
                    let (Some(package), Some(class)) = (&item.package_name, &item.name) else {
                        unsupported(&mut reply, crate::package::apps_filter::NotModelled("captured resolver component identity unavailable"));
                        return Ok(reply);
                    };
                    let value = FrozenComponent(crate::package::intent::ComponentName {
                        package: package.clone(), class: class.clone(),
                    });
                    api::write_get_resolver_component_reply(&mut reply, Some(&value));
                },
                None => unsupported(&mut reply, crate::package::apps_filter::NotModelled("captured resolver owner unavailable")),
            },
            Action::ApksInApex(a) => match capture.apks_in_apex(a.package_name.as_deref()) {
                Ok(value) => {
                    let value = value.map(|names| names.into_iter().map(Some).collect::<Vec<_>>());
                    api::write_get_apks_in_apex_reply(&mut reply, &value);
                },
                Err(error) => unsupported(&mut reply, error),
            },
            Action::Diagnostic(a) => match capture.diagnostic_record(a.kind, a.dump_type, a.package_name.as_deref(),
                    a.permission_names.as_deref(), a.check_in, a.dump_state.as_deref()) {
                Ok(value) => api::write_get_diagnostic_record_reply(&mut reply, &Some(value)),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::PreferredTokens(a) => match capture.preferred_record_tokens(a.user_id, a.kind) {
                Ok(tokens) => {
                    let tokens = tokens.map(|tokens| tokens.into_iter().map(|token| Some(FrozenBinder(token))).collect::<Vec<_>>());
                    api::write_get_preferred_record_tokens_reply(&mut reply, tokens.as_deref());
                },
                Err(error) => unsupported(&mut reply, error),
            },
            Action::InstantMetadata(a) => {
                let Some(owner) = capture.state().system.instant_registry.as_ref() else {
                    unsupported(&mut reply, crate::package::apps_filter::NotModelled("native instant metadata owner unavailable")); return Ok(reply);
                };
                match owner.has_metadata(a.package_name.as_deref(), a.user_id) {
                    Ok(value) => api::write_has_instant_application_metadata_reply(&mut reply, value),
                    Err(error) => unsupported(&mut reply, error),
                }
            },
            Action::PermissionUpgrade(a) => match capture.permission_upgrade_needed(a.user_id) {
                Ok(value) => api::write_is_permission_upgrade_needed_reply(&mut reply, value),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::ResolveIntent(a) => match resolution.resolve_intent_internal_record(a.intent.as_ref(), a.resolved_type.as_deref(), a.flags, a.private_resolve_flags, a.user_id, a.resolve_for_start, a.filter_calling_uid, a.filter_calling_pid, a.calling_uid, a.calling_pid) {
                Ok(value) => api::write_resolve_intent_internal_record_reply(&mut reply, &value),
                Err(crate::package::resolve::QueryError::Original(error)) => reply.write_exception(&error),
                Err(crate::package::resolve::QueryError::Transport(status)) => return Err(status),
                Err(crate::package::resolve::QueryError::NotModelled(error)) => unsupported(&mut reply, error),
            },
            Action::ResolveService(a) => match resolution.resolve_service_internal_record(a.intent.as_ref(), a.resolved_type.as_deref(), a.flags, a.user_id, a.filter_calling_uid, a.filter_calling_pid, a.resolve_for_start, a.calling_uid, a.calling_pid) {
                Ok(value) => api::write_resolve_service_internal_record_reply(&mut reply, &value),
                Err(crate::package::resolve::QueryError::Original(error)) => reply.write_exception(&error),
                Err(crate::package::resolve::QueryError::Transport(status)) => return Err(status),
                Err(crate::package::resolve::QueryError::NotModelled(error)) => unsupported(&mut reply, error),
            },
            Action::Receivers(a) => match resolution.query_receivers_internal_record(a.intent.as_ref(), a.resolved_type.as_deref(), a.flags, a.user_id, a.filter_calling_uid, a.filter_calling_pid, a.for_send, a.calling_uid, a.calling_pid) {
                Ok(value) => api::write_query_intent_receivers_internal_record_reply(&mut reply, &Some(value)),
                Err(crate::package::resolve::QueryError::Original(error)) => reply.write_exception(&error),
                Err(crate::package::resolve::QueryError::Transport(status)) => return Err(status),
                Err(crate::package::resolve::QueryError::NotModelled(error)) => unsupported(&mut reply, error),
            },
            Action::Services(a) => match resolution.query_services_internal_record(a.intent.as_ref(), a.resolved_type.as_deref(), a.flags,
                    a.user_id, a.filter_calling_uid, a.filter_calling_pid, a.include_instant_apps, a.resolve_for_start,
                    a.calling_uid, a.calling_pid) {
                Ok(value) => api::write_query_intent_services_internal_record_reply(&mut reply, &Some(value)),
                Err(crate::package::resolve::QueryError::Original(error)) => reply.write_exception(&error),
                Err(crate::package::resolve::QueryError::Transport(status)) => return Err(status),
                Err(crate::package::resolve::QueryError::NotModelled(error)) => unsupported(&mut reply, error),
            },
            Action::Activities(a) => match resolution.query_activities_internal_record(a.intent.as_ref(), a.resolved_type.as_deref(), a.flags,
                    a.private_resolve_flags, a.filter_calling_uid, a.filter_calling_pid, a.user_id,
                    a.resolve_for_start, a.allow_dynamic_splits, a.calling_uid, a.calling_pid) {
                Ok(value) => api::write_query_intent_activities_internal_record_reply(&mut reply, &Some(value)),
                Err(crate::package::resolve::QueryError::Original(error)) => reply.write_exception(&error),
                Err(crate::package::resolve::QueryError::Transport(status)) => return Err(status),
                Err(crate::package::resolve::QueryError::NotModelled(error)) => unsupported(&mut reply, error),
            },
            Action::Upgrading(a) => {
                let Some(owner) = capture.state().system.lifecycle.as_ref() else {
                    unsupported(&mut reply, crate::package::apps_filter::NotModelled("native boot upgrade owner unavailable")); return Ok(reply);
                };
                api::write_is_upgrading_from_lower_than_reply(&mut reply, owner.is_upgrading_from_lower_than(a.sdk_version));
            },
            Action::KnownPackages(a) => {
                let Some(owner) = capture.state().system.roles.as_ref() else {
                    unsupported(&mut reply, crate::package::apps_filter::NotModelled("captured native known package owner unavailable")); return Ok(reply);
                };
                match owner.known_packages(&query, a.kind, a.user_id) {
                    Ok(values) => api::write_get_known_package_names_reply(&mut reply, &Some(values)),
                    Err(error) => unsupported(&mut reply, error),
                }
            },
            Action::PlatformSigning => {
                let value = match capture.platform_signing_record() {
                    Ok(value) => Some(value.to_vec()),
                    Err(error) => { unsupported(&mut reply, error); return Ok(reply); }
                };
                api::write_get_platform_signing_details_record_reply(&mut reply, &value);
            },
            Action::CrossUserAllowed(a) => match query.internal_has_cross_user_permission(a.filter_calling_uid, a.user_id, a.require_full_permission) {
                Ok(Ok(value)) => api::write_has_cross_user_permission_reply(&mut reply, value),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::ActivitySupports(a) => match query.internal_activity_supports_intent(a.resolve_component.as_ref(), a.component.as_ref(), a.intent.as_ref(), a.resolved_type.as_deref(), a.user_id) {
                Ok(Ok(value)) => api::write_activity_supports_intent_as_user_reply(&mut reply, value),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::InstantInstallerInfo => {
                let Some(owner) = capture.state().system.instant_components.as_ref() else {
                    unsupported(&mut reply, crate::package::apps_filter::NotModelled("captured native instant installer info owner unavailable")); return Ok(reply);
                };
                let value = owner.installer_info_record().map(|value| value.to_vec());
                api::write_get_instant_app_installer_info_record_reply(&mut reply, &value);
            },
            Action::FrozenNames | Action::FrozenCounts => {
                let values = match capture.frozen_packages() {
                    Ok(values) => values,
                    Err(error) => { unsupported(&mut reply, error); return Ok(reply); }
                };
                if matches!(action, Action::FrozenNames) {
                    let names = values.keys().cloned().map(Some).collect::<Vec<_>>();
                    api::write_get_frozen_package_names_reply(&mut reply, &Some(names));
                } else {
                    let counts = values.values().copied().collect::<Vec<_>>();
                    api::write_get_frozen_package_counts_reply(&mut reply, &Some(counts));
                }
            },
            Action::InstantInstallerComponent => {
                let Some(owner) = capture.state().system.instant_components.as_ref() else {
                    unsupported(&mut reply, crate::package::apps_filter::NotModelled("captured native instant component owner unavailable")); return Ok(reply);
                };
                let value = owner.installer_component().map(FrozenComponent);
                api::write_get_instant_app_installer_component_reply(&mut reply, value.as_ref());
            },
            Action::Provider(a) => {
                if let Some(name) = a.authority.as_deref() {
                    match resolution.resolve_content_provider_for_query(name, a.flags, a.user_id, a.filter_calling_uid, a.calling_uid) {
                        Ok(value) => api::write_resolve_content_provider_reply(&mut reply, value.as_ref()),
                        Err(crate::package::resolve::ResolutionError::Original(error)) => reply.write_exception(&error),
                        Err(crate::package::resolve::ResolutionError::NotModelled(error)) => unsupported(&mut reply, error),
                        Err(crate::package::resolve::ResolutionError::UriMatching(error)) => match error.binder_exception() { Some(error) => reply.write_exception(&error), None => return Err(UNKNOWN_TRANSACTION) },
                    }
                } else if capture.state().users.contains_key(&a.user_id) {
                    reply.write_exception(&Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "authority is null"));
                } else { api::write_resolve_content_provider_reply::<crate::package::info::ProviderInfo>(&mut reply, None); }
            },
            Action::ActivityInternal(a) => match query.internal_activity_info(a.component.as_ref(), a.flags, a.filter_calling_uid, a.user_id) {
                Ok(Ok(value)) => api::write_get_activity_info_internal_reply(&mut reply, value.as_ref()),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::CanAccessComponent(a) => match query.internal_can_access_component(a.component.as_ref(), a.filter_calling_uid, a.user_id) {
                Ok(Ok(value)) => api::write_can_access_component_reply(&mut reply, value),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::Visibility(a) => match query.internal_visibility_allow_list(a.package_name.as_deref(), a.user_id, a.check_only) {
                Ok(value) => api::write_get_visibility_allow_list_reply(&mut reply, &value),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::LibraryUsers(a) => match query.internal_library_users(a.name.as_deref(), a.version, a.library_type, a.flags, a.filter_calling_uid, a.user_id) {
                Ok((users, _)) => {
                    let users = users.map(|users| users.into_iter().map(|(name, version)| Some(LibraryUser(name, version))).collect::<Vec<_>>());
                    api::write_get_shared_library_users_reply::<LibraryUser>(&mut reply, users.as_deref());
                },
                Err(error) => unsupported(&mut reply, error),
            },
            Action::LibraryUsersOptional(a) => match query.internal_library_users(a.name.as_deref(), a.version, a.library_type, a.flags, a.filter_calling_uid, a.user_id) {
                Ok((_, optional)) => api::write_get_shared_library_users_optional_reply(&mut reply, &optional),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::SetupWizard => {
                let Some(owner) = capture.state().system.roles.as_ref() else {
                    unsupported(&mut reply, crate::package::apps_filter::NotModelled("captured native boot role owner unavailable")); return Ok(reply);
                };
                let value = owner.package(crate::package::roles::Role::SetupWizard, &query);
                match value { Ok(value) => api::write_get_setup_wizard_package_name_reply(&mut reply, &value), Err(error) => reply.write_exception(&error) };
            },
            Action::LookupUid(a) => match query.internal_package_lookup_uid(a.uid, a.known_isolated_compute_app) {
                Ok(Ok(value)) => api::write_get_package_lookup_uid_reply(&mut reply, value),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::Processes(a) => {
                let uid = match query.internal_setting_uid(a.uid) {
                    Ok(uid) => uid,
                    Err(error) => { unsupported(&mut reply, error); return Ok(reply); }
                };
                let records = match crate::package::apps_filter::setting(capture.state(), crate::package::apps_filter::app_id(uid)) {
                    Some(crate::package::apps_filter::Setting::Shared(group)) => {
                        match capture.scan().owner().shared_processes(&group.name) {
                            Ok(Some(owner)) => Some(owner.records()),
                            _ => { unsupported(&mut reply, crate::package::apps_filter::NotModelled("captured shared process owner unavailable")); return Ok(reply); }
                        }
                    },
                    Some(crate::package::apps_filter::Setting::Package(state)) => state.pkg.as_deref().and_then(|pkg| pkg.processes.as_deref()),
                    None => None,
                };
                let values = records.map(|records| records.iter().cloned().map(|process| Some(ProcessInfo(process))).collect::<Vec<_>>());
                api::write_get_processes_for_uid_reply::<ProcessInfo>(&mut reply, values.as_deref());
            },
            Action::FilterCandidate(a) => match query.internal_filter_candidate(a.owner_kind, a.owner_name.as_deref(), a.package_name.as_deref(), a.app_id, a.filter_calling_uid, a.user_id, a.filter_uninstalled) {
                Ok(value) => api::write_should_filter_application_reply(&mut reply, value),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::BlockUninstall(a) => match query.internal_block_uninstall(a.user_id, a.package_name.as_deref()) {
                Ok(value) => api::write_get_block_uninstall_reply(&mut reply, value),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::LibraryRegistry => match query.internal_shared_libraries() {
                Ok(value) => { let value = value.into_iter().map(Some).collect::<Vec<_>>(); api::write_get_shared_library_registry_reply(&mut reply, Some(&value)); },
                Err(error) => unsupported(&mut reply, error),
            },
            Action::ExplicitUid(a) => match query.package_uid_internal(a.package_name.as_deref().unwrap_or_default(), a.flags, a.user_id, a.filter_calling_uid) {
                Ok(value) => api::write_get_package_uid_with_caller_reply(&mut reply, value),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::CallerSame(a) => match query.internal_caller_same_app(a.package_name.as_deref(), a.uid, a.resolve_isolated_uid) {
                Ok(Ok(value)) => api::write_is_caller_same_app_reply(&mut reply, value),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::InstantName(a) => match query.internal_instant_package_name(a.uid) {
                Ok(Ok(value)) => api::write_get_instant_app_package_name_reply(&mut reply, &value),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::ComponentEnabled(a) => match query.internal_component_enabled(a.component.as_ref(), a.filter_calling_uid, a.user_id, a.internal) {
                Ok(Ok(value)) => api::write_get_component_enabled_setting_reply(&mut reply, value),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::InstalledApps(a) => match query.internal_installed_applications(a.flags, a.user_id, a.filter_calling_uid, a.force_allow_cross_user) {
                Ok(Ok(value)) => {
                    let process=self.process.upgrade().ok_or(BAD_VALUE)?;
                    let slice=crate::package::list_slice::Slice::new(process,"android.content.pm.ApplicationInfo",value);
                    api::write_get_installed_applications_reply(&mut reply,Some(&slice));
                },
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::InstantInternal(a) => match query.internal_is_instant(a.package_name.as_deref().unwrap_or_default(), a.user_id, a.filter_calling_uid) {
                Ok(value) => api::write_is_instant_app_internal_reply(&mut reply, value),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::CanView(a) => match query.internal_can_view_instant(a.filter_calling_uid, a.user_id) {
                Ok(value) => api::write_can_view_instant_apps_reply(&mut reply, value),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::AllUserSignatures(a) => match query.internal_check_uid_signatures_all_users(a.uid1, a.uid2) {
                Ok(Ok(value)) => api::write_check_uid_signatures_for_all_users_reply(&mut reply, value),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::EnforceCross(a) => match query.internal_enforce_cross_user(a.filter_calling_uid, a.user_id, a.require_full_permission, a.check_shell, a.message.as_deref().unwrap_or("null")) {
                Ok(Ok(())) => api::write_enforce_cross_user_permission_reply(&mut reply),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::Persistent(a) => match query.persistent_applications_captured(a.safe_mode, a.flags) {
                Ok(value) => {
                    let value = value.into_iter().map(Some).collect::<Vec<_>>();
                    api::write_get_persistent_applications_reply(&mut reply, Some(&value));
                },
                Err(error) => unsupported(&mut reply, error),
            },
            Action::Startability(a) => match query.package_startability_captured(a.safe_mode,
                    a.package_name.as_deref(), a.filter_calling_uid, a.user_id) {
                Ok(value) => api::write_get_package_startability_reply(&mut reply, value),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::Application(a) => match query.application_info_internal(
                a.package_name.as_deref().unwrap_or_default(),
                a.flags,
                a.user_id,
                a.filter_calling_uid,
            ) {
                Ok(Ok(value)) => api::write_get_application_info_reply(&mut reply, value.as_ref()),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::Package(a) => match query.package_info_internal(
                a.package_name.as_deref().unwrap_or_default(),
                -1,
                a.flags,
                a.user_id,
                a.filter_calling_uid,
            ) {
                Ok(Ok(value)) => api::write_get_package_info_reply(&mut reply, value.as_ref()),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::Filter(a) => match crate::package::apps_filter::should_filter_application(
                capture.state(),
                &resolution.apps_filter,
                capture
                    .state()
                    .packages
                    .get(a.package_name.as_deref().unwrap_or_default()),
                a.calling_uid,
                a.user_id,
                a.filter_uninstalled,
                true,
            ) {
                Ok(value) => api::write_filter_app_access_reply(&mut reply, value),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::Forward(code, data, _) => match query.answer(
                pm::DESCRIPTOR,
                code,
                &mut aim_binder_host::parcel::Reader::new(data.data(), data.objects()),
            ) {
                Ok(value) => return Ok(value),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::Installer(a) => match query.installer_package_internal(
                a.package_name.as_deref().unwrap_or_default(),
                a.user_id,
            ) {
                Ok(Ok(value)) => api::write_get_installer_package_name_reply(&mut reply, &value),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::InternalUid(a) => match query.package_uid_internal(
                a.package_name.as_deref().unwrap_or_default(),
                a.flags,
                a.user_id,
                1000,
            ) {
                Ok(value) => api::write_get_package_uid_internal_reply(&mut reply, value),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::InternalName(a) => match a
                .package_name
                .as_deref()
                .map(|name| query.internal_resolve_name(name, a.version_code))
                .transpose()
            {
                Ok(value) => api::write_resolve_internal_package_name_reply(&mut reply, &value),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::Same(a) => match query.internal_same_app(
                a.package_name.as_deref(),
                a.flags,
                a.comparison_uid,
                a.user_id,
            ) {
                Ok(Ok(value)) => api::write_is_same_app_reply(&mut reply, value),
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::UidAccess(a) => match query.internal_filter_uid(a.target_uid, a.calling_uid) {
                Ok(value) => api::write_filter_uid_access_reply(&mut reply, value),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::CanQuery(a) => {
                match query.internal_can_query(a.query_uid, a.target_package_name.as_deref()) {
                    Ok(Ok(value)) => api::write_can_query_package_reply(&mut reply, value),
                    Ok(Err(error)) => reply.write_exception(&error),
                    Err(error) => unsupported(&mut reply, error),
                }
            }
            Action::VersionedInfo(a) => match query.package_info_internal(
                a.package_name.as_deref().unwrap_or_default(),
                a.version_code,
                a.flags,
                a.user_id,
                a.filter_calling_uid,
            ) {
                Ok(Ok(value)) => {
                    api::write_get_package_info_internal_reply(&mut reply, value.as_ref())
                }
                Ok(Err(error)) => reply.write_exception(&error),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::FilteredState(a) => match query.internal_filtered_package_name(
                a.package_name.as_deref().unwrap_or_default(),
                a.user_id,
            ) {
                Ok(value) => api::write_get_package_state_filtered_name_reply(&mut reply, &value),
                Err(error) => unsupported(&mut reply, error),
            },
            Action::UidTargetSdk(a) => match query.internal_uid_target_sdk(a.uid) {
                Ok(value) => api::write_get_uid_target_sdk_version_reply(&mut reply, value),
                Err(error) => unsupported(&mut reply, error),
            },
            _ => unreachable!(),
        }
        Ok(reply)
    }
}

pub(crate) fn legacy_permission_definitions_record(snapshot:&super::Snapshot)->Vec<u8>{
    let mut record = Parcel::new(); record.write_i32(1); record.write_i64(snapshot.version() as i64);
    for definitions in [&snapshot.owner().settings.permissions, &snapshot.owner().settings.permission_trees] {
        record.write_i32(definitions.len() as i32);
        for permission in definitions {
            record.write_string16(Some(&permission.name)); record.write_string16(Some(&permission.package));
            record.write_i32(permission.protection_level);
            let (kind, uid, gids) = match &permission.owner {
                crate::package::settings::PermissionOwner::Manifest => (0,0,Vec::new()),
                crate::package::settings::PermissionOwner::Config { uid,gids } => (1,*uid,gids.clone()),
                crate::package::settings::PermissionOwner::Dynamic => (2,0,Vec::new()),
            };
            record.write_i32(kind); record.write_i32(uid); aim_service_aidl::write_int_array(&mut record,Some(&gids));
            record.write_i32(permission.dynamic.as_ref().map_or(0, |(icon,_)|*icon));
            record.write_string16(permission.dynamic.as_ref().and_then(|(_,label)|label.as_deref()));
        }
    }
    record.data().to_vec()
}

// Binder omits the sender PID for one-way transactions; original ART reports 0.
fn valid_original_caller(uid: i32, pid: i32) -> bool {
    uid >= 0 && pid >= 0
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn trusted_original_oneway_identity_accepts_zero_pid_and_rejects_negative_values() {
        assert!(valid_original_caller(1000, 0));
        assert!(valid_original_caller(10123, 0));
        assert!(valid_original_caller(10123, 123));
        assert!(!valid_original_caller(10123, -1));
        assert!(!valid_original_caller(-1, 0));
    }

    #[test]
    fn forwarded_zero_pid_does_not_authenticate_a_foreign_transport() {
        let computer = Computer {
            capture: Arc::new(Mutex::new(None)),
            process: Weak::new(),
            query_binders: Mutex::new(BTreeMap::new()),
            uid_registry: Mutex::new(None),
        };
        let mut request = Parcel::new();
        request.write_interface_token(api::DESCRIPTOR);
        request.write_i32(1000);
        request.write_i32(0);
        let mut call = Call {
            code: api::GET_PACKAGE_MANAGER_QUERY_BINDER,
            flags: 0,
            sender_pid: 123,
            sender_euid: 10123,
            data: request.reader(),
        };
        let reply = computer.transact(&mut call).unwrap();
        let error = reply.reader().read_exception().unwrap().unwrap_err();
        assert_eq!(error.code, aim_binder_host::parcel::EX_SECURITY);
    }
}
