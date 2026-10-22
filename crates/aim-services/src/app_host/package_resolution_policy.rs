//! `IPackageResolutionPolicy` for the app host: what the native package
//! resolver asks of the user and permission owners, answered as
//! `NativePackageResolutionPolicy` answers it (`android-16.0.0_r1`'s
//! UserManagerService and PermissionChecker).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_ipackageresolutionpolicy as policy;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;
const INTERACT_ACROSS_PROFILES: &str = "android.permission.INTERACT_ACROSS_PROFILES";
/// `UserHandle.PER_USER_RANGE`.
const PER_USER_RANGE: i32 = 100_000;

pub struct ResolutionPolicy {
    host: Arc<AppHost>,
}

impl ResolutionPolicy {
    pub fn new(host: Arc<AppHost>) -> Self {
        ResolutionPolicy { host }
    }
}

impl Service for ResolutionPolicy {
    fn descriptor(&self) -> &str {
        policy::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID {
            return reply(Err(Exception::security("Native resolution owner required")));
        }
        let host = &self.host;
        let mut p = Parcel::new();
        let result = match call.code {
            // `NativeUserManagerBridge.isKnownIsolatedComputeApp`: an
            // isolated uid of the hotword detection service or of the
            // on-device inference service, neither of which the app host
            // serves.
            policy::IS_KNOWN_ISOLATED_COMPUTE_APP => {
                policy::IsKnownIsolatedComputeApp::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                policy::write_is_known_isolated_compute_app_reply(&mut p, false);
                Ok(())
            }
            // A profile of the caller's group, and INTERACT_ACROSS_PROFILES
            // by `checkPermissionForPreflight`.
            policy::PROFILE_PERMISSION => {
                let a = policy::ProfilePermission::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                super::user_service::same_profile_group(
                    host,
                    a.calling_uid.div_euclid(PER_USER_RANGE),
                    a.target_user,
                )
                .and_then(|same| match same {
                    false => Ok(false),
                    true => super::permission_checker::preflight(
                        host,
                        INTERACT_ACROSS_PROFILES,
                        a.calling_uid,
                        a.package_name.as_deref().unwrap_or_default(),
                    ),
                })
                .map(|granted| policy::write_profile_permission_reply(&mut p, granted))
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| p))
    }
}
