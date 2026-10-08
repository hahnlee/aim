//! Read-only public query protocol bound to one immutable Computer capture and trusted caller.
use super::query_state::Capture;
use crate::package::{
    query::Query,
    resolve::QueryError,
};
use aim_binder_host::{
    local::{Call, Reply, Service},
    parcel::{EX_ILLEGAL_STATE, EX_UNSUPPORTED_OPERATION, Exception, Parcel, UNKNOWN_TRANSACTION},
};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;
use std::sync::{Arc, Mutex};

pub(super) struct ReadOnlyQuery {
    pub capture: Arc<Mutex<Option<Arc<Capture>>>>,
    pub uid: i32,
    pub pid: i32,
}

fn allowed(code: u32) -> bool {
    matches!(
        code,
        pm::GET_ARCHIVED_PACKAGE
            | pm::GET_ARCHIVED_APP_ICON
            | pm::IS_APP_ARCHIVABLE
            | pm::CAN_FORWARD_TO
            | pm::ACTIVITY_SUPPORTS_INTENT_AS_USER
            | pm::CANONICAL_TO_CURRENT_PACKAGE_NAMES
            | pm::CAN_PACKAGE_QUERY
            | pm::GET_BLOCK_UNINSTALL_FOR_USER
            | pm::CHECK_PACKAGE_STARTABLE
            | pm::CHECK_PERMISSION
            | pm::CHECK_SIGNATURES
            | pm::CHECK_UID_PERMISSION
            | pm::CHECK_UID_SIGNATURES
            | pm::CURRENT_TO_CANONICAL_PACKAGE_NAMES
            | pm::GET_ACTIVITY_INFO
            | pm::GET_ALL_APEX_DIRECTORIES
            | pm::GET_ALL_INTENT_FILTERS
            | pm::GET_ALL_PACKAGES
            | pm::GET_APPLICATION_ENABLED_SETTING
            | pm::GET_APPLICATION_HIDDEN_SETTING_AS_USER
            | pm::GET_APPLICATION_INFO
            | pm::GET_APP_METADATA_SOURCE
            | pm::GET_APP_OP_PERMISSION_PACKAGES
            | pm::GET_APP_PREDICTION_SERVICE_PACKAGE_NAME
            | pm::GET_ATTENTION_SERVICE_PACKAGE_NAME
            | pm::GET_COMPONENT_ENABLED_SETTING
            | pm::GET_DECLARED_SHARED_LIBRARIES
            | pm::GET_DEFAULT_TEXT_CLASSIFIER_PACKAGE_NAME
            | pm::FIND_PERSISTENT_PREFERRED_ACTIVITY
            | pm::GET_FLAGS_FOR_UID
            | pm::GET_HARMFUL_APP_WARNING
            | pm::GET_INCIDENT_REPORT_APPROVER_PACKAGE_NAME
            | pm::GET_INITIAL_NON_STOPPED_SYSTEM_PACKAGES
            | pm::GET_INSTANT_APPS
            | pm::GET_INSTANT_APP_COOKIE
            | pm::GET_INSTANT_APP_ICON
            | pm::GET_INSTANT_APP_RESOLVER_COMPONENT
            | pm::GET_INSTANT_APP_INSTALLER_COMPONENT
            | pm::GET_INSTANT_APP_RESOLVER_SETTINGS_COMPONENT
            | pm::GET_INSTALLED_APPLICATIONS
            | pm::GET_INSTALLED_MODULES
            | pm::GET_INSTALLED_PACKAGES
            | pm::GET_INSTALLER_PACKAGE_NAME
            | pm::GET_INSTALL_REASON
            | pm::GET_INSTALL_SOURCE_INFO
            | pm::GET_INSTRUMENTATION_INFO_AS_USER
            | pm::GET_INTENT_FILTER_VERIFICATIONS
            | pm::GET_INTENT_VERIFICATION_STATUS
            | pm::GET_KEY_SET_BY_ALIAS
            | pm::GET_MIME_GROUP
            | pm::GET_MODULE_INFO
            | pm::GET_NAMES_FOR_UIDS
            | pm::GET_NAME_FOR_UID
            | pm::GET_PACKAGES_FOR_UID
            | pm::GET_PACKAGES_HOLDING_PERMISSIONS
            | pm::GET_PACKAGE_GIDS
            | pm::GET_PACKAGE_INFO
            | pm::GET_PACKAGE_INFO_VERSIONED
            | pm::GET_PACKAGE_UID
            | pm::GET_PERMISSION_CONTROLLER_PACKAGE_NAME
            | pm::GET_PERSISTENT_APPLICATIONS
            | pm::GET_PRIVATE_FLAGS_FOR_UID
            | pm::GET_PROPERTY_AS_USER
            | pm::GET_PROVIDER_INFO
            | pm::GET_RECEIVER_INFO
            | pm::GET_ROTATION_RESOLVER_PACKAGE_NAME
            | pm::GET_SDK_SANDBOX_PACKAGE_NAME
            | pm::GET_SERVICES_SYSTEM_SHARED_LIBRARY_PACKAGE_NAME
            | pm::GET_SERVICE_INFO
            | pm::GET_SETUP_WIZARD_PACKAGE_NAME
            | pm::GET_SHARED_LIBRARIES
            | pm::GET_SHARED_SYSTEM_SHARED_LIBRARY_PACKAGE_NAME
            | pm::GET_SIGNING_KEY_SET
            | pm::GET_SPLASH_SCREEN_THEME
            | pm::GET_SUSPENDED_PACKAGE_APP_EXTRAS
            | pm::GET_SUSPENDING_PACKAGE
            | pm::GET_SYSTEM_AVAILABLE_FEATURES
            | pm::GET_SYSTEM_CAPTIONS_SERVICE_PACKAGE_NAME
            | pm::GET_SYSTEM_SHARED_LIBRARY_NAMES
            | pm::GET_SYSTEM_SHARED_LIBRARY_NAMES_AND_PATHS
            | pm::GET_SYSTEM_TEXT_CLASSIFIER_PACKAGE_NAME
            | pm::GET_TARGET_SDK_VERSION
            | pm::GET_UID_FOR_SHARED_USER
            | pm::GET_USER_MIN_ASPECT_RATIO
            | pm::HAS_SIGNING_CERTIFICATE
            | pm::HAS_SYSTEM_FEATURE
            | pm::HAS_SYSTEM_UID_ERRORS
            | pm::HAS_UID_SIGNING_CERTIFICATE
            | pm::IS_PAGE_SIZE_COMPAT_ENABLED
            | pm::GET_PAGE_SIZE_COMPAT_WARNING_MESSAGE
            | pm::IS_FIRST_BOOT
            | pm::IS_DEVICE_UPGRADING
            | pm::IS_SAFE_MODE
            | pm::IS_INSTANT_APP
            | pm::IS_PACKAGE_AVAILABLE
            | pm::IS_PACKAGE_QUARANTINED_FOR_USER
            | pm::IS_PACKAGE_SIGNED_BY_KEY_SET
            | pm::IS_PACKAGE_SIGNED_BY_KEY_SET_EXACTLY
            | pm::IS_PACKAGE_STOPPED_FOR_USER
            | pm::IS_PACKAGE_SUSPENDED_FOR_USER
            | pm::IS_PROTECTED_BROADCAST
            | pm::IS_UID_PRIVILEGED
            | pm::QUERY_CONTENT_PROVIDERS
            | pm::QUERY_INSTRUMENTATION_AS_USER
            | pm::QUERY_INTENT_ACTIVITY_OPTIONS
            | pm::QUERY_INTENT_ACTIVITIES
            | pm::QUERY_INTENT_CONTENT_PROVIDERS
            | pm::QUERY_INTENT_RECEIVERS
            | pm::QUERY_INTENT_SERVICES
            | pm::QUERY_PROPERTY
            | pm::RESOLVE_CONTENT_PROVIDER_FOR_UID
            | pm::RESOLVE_CONTENT_PROVIDER
            | pm::RESOLVE_INTENT
            | pm::RESOLVE_SERVICE
    )
}

impl Service for ReadOnlyQuery {
    fn descriptor(&self) -> &str {
        pm::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if !allowed(call.code) {
            return Err(UNKNOWN_TRANSACTION);
        }
        let mut reply = Parcel::new();
        if call.sender_euid != crate::SYSTEM_UID {
            reply.write_exception(&Exception::security(
                "retained query transport serves system UID only",
            ));
            return Ok(reply);
        }
        let start = call.data.position();
        call.data.enforce_interface(pm::DESCRIPTOR)?;
        call.data.set_position(start);
        let Some(capture) = self.capture.lock().unwrap().clone() else {
            reply.write_exception(&Exception::new(
                EX_ILLEGAL_STATE,
                "retained query capture is closed",
            ));
            return Ok(reply);
        };
        // Transport credentials stay system-only; semantic checks use the captured original caller.
        call.sender_euid = self.uid as u32;
        call.sender_pid = self.pid;
        if let Some(result) =
            capture.resolver()
                .query(capture.state(), call.code, self.uid, &mut call.data)
        {
            return match result {
                Ok(reply) => Ok(reply),
                Err(QueryError::Original(error)) => { let mut reply = Parcel::new(); reply.write_exception(&error); Ok(reply) },
                Err(QueryError::Transport(status)) => Err(status),
                Err(QueryError::NotModelled(error)) => {
                    reply.write_exception(&Exception::new(EX_UNSUPPORTED_OPERATION, error.0));
                    Ok(reply)
                }
            };
        }
        let resolution = match capture.resolution() {
            Ok(value) => value,
            Err(error) => return error.reply(),
        };
        let query = Query {
            state: capture.state(),
            filter: &resolution.apps_filter,
            calling_uid: self.uid,
        };
        match query.answer(pm::DESCRIPTOR, call.code, &mut call.data) {
            Ok(reply) => Ok(reply),
            Err(error) => {
                reply.write_exception(&Exception::new(EX_UNSUPPORTED_OPERATION, error.0));
                Ok(reply)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_read_allowlist_rejects_every_mutation_before_decoding() {
        for code in [
            pm::SET_APPLICATION_ENABLED_SETTING,
            pm::SET_COMPONENT_ENABLED_SETTING,
            pm::SET_PACKAGES_SUSPENDED_AS_USER,
            pm::DELETE_PACKAGE_AS_USER,
            pm::ADD_PERMISSION,
            pm::SET_RUNTIME_PERMISSIONS_VERSION,
            pm::SET_BLOCK_UNINSTALL_FOR_USER,
            pm::ENTER_SAFE_MODE,
        ] {
            assert!(!allowed(code));
        }
        assert!(allowed(pm::GET_KEY_SET_BY_ALIAS));
        assert!(allowed(pm::HAS_SIGNING_CERTIFICATE));
        assert!(allowed(pm::QUERY_INTENT_ACTIVITIES));
        for code in [pm::GET_BLOCK_UNINSTALL_FOR_USER, pm::CAN_PACKAGE_QUERY,
            pm::GET_INSTANT_APP_RESOLVER_COMPONENT, pm::GET_INSTANT_APP_INSTALLER_COMPONENT,
            pm::GET_INSTANT_APP_RESOLVER_SETTINGS_COMPONENT, pm::IS_PAGE_SIZE_COMPAT_ENABLED,
            pm::GET_PAGE_SIZE_COMPAT_WARNING_MESSAGE, pm::ACTIVITY_SUPPORTS_INTENT_AS_USER,
            pm::FIND_PERSISTENT_PREFERRED_ACTIVITY, pm::RESOLVE_CONTENT_PROVIDER_FOR_UID, pm::IS_FIRST_BOOT, pm::IS_DEVICE_UPGRADING,
            pm::IS_SAFE_MODE, pm::GET_PERSISTENT_APPLICATIONS, pm::CHECK_PACKAGE_STARTABLE,
            pm::QUERY_INTENT_ACTIVITY_OPTIONS] {
            assert!(allowed(code));
        }
    }
    #[test]
    fn transport_guard_rejects_mutation_and_foreign_uid_before_argument_decode() {
        use aim_binder_host::parcel::{EX_SECURITY, Reader};
        let service = ReadOnlyQuery {
            capture: Arc::new(Mutex::new(None)),
            uid: 10123,
            pid: 10,
        };
        let mut write = Call {
            code: pm::SET_APPLICATION_ENABLED_SETTING,
            flags: 0,
            sender_pid: 1,
            sender_euid: crate::SYSTEM_UID,
            data: Reader::new(&[], &[]),
        };
        assert_eq!(
            service.transact(&mut write).unwrap_err(),
            UNKNOWN_TRANSACTION
        );
        let mut foreign = Call {
            code: pm::GET_PACKAGE_INFO,
            flags: 0,
            sender_pid: 1,
            sender_euid: 10123,
            data: Reader::new(&[], &[]),
        };
        let reply = service.transact(&mut foreign).unwrap();
        let mut reader = Reader::new(reply.data(), reply.objects());
        assert_eq!(
            reader.read_exception().unwrap().unwrap_err().code,
            EX_SECURITY
        );
        let mut parcel = Parcel::new();
        parcel.write_interface_token(pm::DESCRIPTOR);
        let mut closed = Call {
            code: pm::GET_PACKAGE_INFO,
            flags: 0,
            sender_pid: 1,
            sender_euid: crate::SYSTEM_UID,
            data: Reader::new(parcel.data(), parcel.objects()),
        };
        let reply = service.transact(&mut closed).unwrap();
        let mut reader = Reader::new(reply.data(), reply.objects());
        assert_eq!(
            reader.read_exception().unwrap().unwrap_err().code,
            EX_ILLEGAL_STATE
        );
    }
}
