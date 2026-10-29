//! Process-lifetime compatibility fronts for the app host's package services.
//! Every call enters the native implementation with its original identity,
//! parcel cursor and flags, including handles retained before construction.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_service_aidl::{
    android_content_pm_ipackagemanager as pm, android_content_pm_ipackagemanagernative as native,
};

use super::AppHost;

pub struct PackageService {
    host: Arc<AppHost>,
    native: bool,
}

impl PackageService {
    pub fn new(host: Arc<AppHost>) -> Self {
        Self {
            host,
            native: false,
        }
    }

    pub fn native(host: Arc<AppHost>) -> Self {
        Self { host, native: true }
    }
}

impl Service for PackageService {
    fn descriptor(&self) -> &str {
        if self.native {
            native::DESCRIPTOR
        } else {
            pm::DESCRIPTOR
        }
    }

    fn accepts_fds(&self) -> bool {
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let system = match self.host.package_system() {
            Ok(system) => system,
            Err(error) => return super::reply(Err(error)),
        };
        // Computer reads need the complete scan; owner operations need construction.
        // ApplicationInfo retains the pre-attach boot-result ordering of #2392.
        let ready = if scan_query(self.native, call.code) {
            self.host.package_boot.wait_scan()
        } else {
            self.host.package_boot.runtime.wait().clone()
        };
        if let Err(error) = ready {
            return super::reply(Err(error));
        }
        let (public, native) = crate::package::service::PackageQueries::from_system(system);
        if self.native {
            native.transact(call)
        } else {
            public.transact(call)
        }
    }
}

/// These methods read the scan's package/UID, component, feature and signing
/// projections. Remaining transactions enter runtime owners (or report their
/// existing unsupported error) after construction, as before.
fn scan_query(is_native: bool, code: u32) -> bool {
    if is_native {
        matches!(
            code,
            native::GET_NAMES_FOR_UIDS
                | native::GET_PACKAGE_UID
                | native::GET_INSTALLER_FOR_PACKAGE
                | native::GET_VERSION_CODE_FOR_PACKAGE
                | native::IS_AUDIO_PLAYBACK_CAPTURE_ALLOWED
                | native::GET_LOCATION_FLAGS
                | native::GET_TARGET_SDK_VERSION_FOR_PACKAGE
        )
    } else {
        matches!(
            code,
            pm::GET_PACKAGE_INFO
                | pm::GET_INSTALL_SOURCE_INFO
                | pm::GET_COMPONENT_ENABLED_SETTING
                | pm::GET_APPLICATION_ENABLED_SETTING
                | pm::HAS_SYSTEM_FEATURE
                | pm::GET_SYSTEM_AVAILABLE_FEATURES
                | pm::GET_PACKAGES_FOR_UID
                | pm::GET_NAME_FOR_UID
                | pm::GET_NAMES_FOR_UIDS
                | pm::GET_PACKAGE_UID
                | pm::GET_PACKAGE_GIDS
                | pm::GET_ACTIVITY_INFO
                | pm::GET_RECEIVER_INFO
                | pm::GET_SERVICE_INFO
                | pm::GET_PROVIDER_INFO
                | pm::GET_INSTALLED_PACKAGES
                | pm::GET_INSTALLED_APPLICATIONS
                | pm::CHECK_SIGNATURES
                | pm::CHECK_UID_SIGNATURES
                | pm::HAS_SIGNING_CERTIFICATE
                | pm::HAS_UID_SIGNING_CERTIFICATE
                | pm::GET_FLAGS_FOR_UID
                | pm::GET_PRIVATE_FLAGS_FOR_UID
                | pm::IS_UID_PRIVILEGED
                | pm::GET_UID_FOR_SHARED_USER
                | pm::GET_TARGET_SDK_VERSION
                | pm::GET_INSTALLER_PACKAGE_NAME
                | pm::GET_INSTALL_REASON
                | pm::GET_ALL_PACKAGES
                | pm::GET_PACKAGE_INFO_VERSIONED
                | pm::IS_PACKAGE_AVAILABLE
                | pm::IS_INSTANT_APP
                | pm::GET_APPLICATION_HIDDEN_SETTING_AS_USER
                | pm::GET_PACKAGES_HOLDING_PERMISSIONS
                | pm::GET_PERMISSION_CONTROLLER_PACKAGE_NAME
                | pm::GET_APP_OP_PERMISSION_PACKAGES
                | pm::GET_SYSTEM_SHARED_LIBRARY_NAMES
                | pm::GET_SYSTEM_SHARED_LIBRARY_NAMES_AND_PATHS
                | pm::GET_SHARED_LIBRARIES
                | pm::GET_DECLARED_SHARED_LIBRARIES
                | pm::GET_PROPERTY_AS_USER
                | pm::QUERY_PROPERTY
                | pm::QUERY_CONTENT_PROVIDERS
                | pm::QUERY_SYNC_PROVIDERS
                | pm::GET_INSTRUMENTATION_INFO_AS_USER
                | pm::QUERY_INSTRUMENTATION_AS_USER
                | pm::GET_KEY_SET_BY_ALIAS
                | pm::GET_SIGNING_KEY_SET
                | pm::IS_PACKAGE_SIGNED_BY_KEY_SET
                | pm::IS_PACKAGE_SIGNED_BY_KEY_SET_EXACTLY
                | pm::GET_MIME_GROUP
                | pm::CURRENT_TO_CANONICAL_PACKAGE_NAMES
                | pm::CANONICAL_TO_CURRENT_PACKAGE_NAMES
                | pm::IS_FIRST_BOOT
                | pm::IS_DEVICE_UPGRADING
                | pm::IS_SAFE_MODE
                | pm::GET_PERSISTENT_APPLICATIONS
                | pm::CHECK_PACKAGE_STARTABLE
                | pm::ACTIVITY_SUPPORTS_INTENT_AS_USER
                | pm::GET_PERMISSION_GROUP_INFO
                | pm::GET_WELLBEING_PACKAGE_NAME
                | pm::GET_DEFAULT_TEXT_CLASSIFIER_PACKAGE_NAME
                | pm::GET_SYSTEM_TEXT_CLASSIFIER_PACKAGE_NAME
                | pm::GET_APP_PREDICTION_SERVICE_PACKAGE_NAME
                | pm::GET_INCIDENT_REPORT_APPROVER_PACKAGE_NAME
                | pm::GET_ATTENTION_SERVICE_PACKAGE_NAME
                | pm::GET_ROTATION_RESOLVER_PACKAGE_NAME
                | pm::GET_SYSTEM_CAPTIONS_SERVICE_PACKAGE_NAME
                | pm::GET_SETUP_WIZARD_PACKAGE_NAME
                | pm::GET_SERVICES_SYSTEM_SHARED_LIBRARY_PACKAGE_NAME
                | pm::GET_SHARED_SYSTEM_SHARED_LIBRARY_PACKAGE_NAME
                | pm::GET_INSTANT_APP_RESOLVER_COMPONENT
                | pm::GET_INSTANT_APP_INSTALLER_COMPONENT
                | pm::GET_INSTANT_APP_RESOLVER_SETTINGS_COMPONENT
                | pm::GET_PAGE_SIZE_COMPAT_WARNING_MESSAGE
                | pm::IS_PAGE_SIZE_COMPAT_ENABLED
                | pm::GET_BLOCK_UNINSTALL_FOR_USER
                | pm::CAN_PACKAGE_QUERY
                | pm::GET_SUSPENDING_PACKAGE
                | pm::GET_SUSPENDED_PACKAGE_APP_EXTRAS
                | pm::GET_SDK_SANDBOX_PACKAGE_NAME
                | pm::GET_ALL_APEX_DIRECTORIES
                | pm::GET_INITIAL_NON_STOPPED_SYSTEM_PACKAGES
                | pm::IS_PROTECTED_BROADCAST
                | pm::GET_APP_METADATA_SOURCE
                | pm::GET_SPLASH_SCREEN_THEME
                | pm::GET_USER_MIN_ASPECT_RATIO
                | pm::IS_PACKAGE_SUSPENDED_FOR_USER
                | pm::IS_PACKAGE_QUARANTINED_FOR_USER
                | pm::IS_PACKAGE_STOPPED_FOR_USER
                | pm::GET_HARMFUL_APP_WARNING
                | pm::GET_INTENT_VERIFICATION_STATUS
                | pm::GET_INTENT_FILTER_VERIFICATIONS
                | pm::HAS_SYSTEM_UID_ERRORS
                | pm::GET_INSTALL_LOCATION
                | pm::IS_PACKAGE_DEVICE_ADMIN_ON_ANY_USER
                | pm::IS_PACKAGE_STATE_PROTECTED
                | pm::QUERY_INTENT_ACTIVITIES
                | pm::QUERY_INTENT_SERVICES
                | pm::QUERY_INTENT_RECEIVERS
                | pm::QUERY_INTENT_CONTENT_PROVIDERS
                | pm::RESOLVE_SERVICE
                | pm::RESOLVE_CONTENT_PROVIDER
                | pm::RESOLVE_CONTENT_PROVIDER_FOR_UID
                | pm::GET_ALL_INTENT_FILTERS
        )
    }
}
