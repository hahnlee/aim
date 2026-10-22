//! Original UM/PermissionChecker preflight producer for private resolution.
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::dev_aim_server_ipackageresolutionpolicy as api;
use std::sync::Arc;

use crate::system::ServiceOwner;

/// The policy: another process's node, or this process's own where it
/// serves the bridge.
pub struct Owner {
    owner: ServiceOwner,
}
impl Owner {
    pub fn new(owner: ServiceOwner) -> Arc<Self> {
        Arc::new(Self { owner })
    }
    pub fn known_isolated_compute(&self, uid: i32) -> Result<bool, Exception> {
        let mut request = Parcel::new();
        api::IsKnownIsolatedComputeApp { uid }.write(&mut request);
        let reply = self
            .owner
            .transact(api::IS_KNOWN_ISOLATED_COMPUTE_APP, &request, false)
            .map_err(|status| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("isolated compute policy transport: {status}"),
                )
            })?;
        api::read_is_known_isolated_compute_app_reply(&mut reply.reader()).map_err(|status| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("isolated compute policy reply: {status}"),
            )
        })?
    }
    pub fn profile(&self, uid: i32, user: i32, name: Option<String>) -> Result<bool, Exception> {
        let mut request = Parcel::new();
        api::ProfilePermission {
            calling_uid: uid,
            target_user: user,
            package_name: name,
        }
        .write(&mut request);
        let reply = self
            .owner
            .transact(api::PROFILE_PERMISSION, &request, false)
            .map_err(|status| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("resolution preflight transport: {status}"),
                )
            })?;
        api::read_profile_permission_reply(&mut reply.reader()).map_err(|status| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("resolution preflight reply: {status}"),
            )
        })?
    }
}
impl std::fmt::Debug for Owner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolutionPolicy").finish_non_exhaustive()
    }
}
impl PartialEq for Owner {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}
