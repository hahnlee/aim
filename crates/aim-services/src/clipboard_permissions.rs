//! The clipboard's `UriPermissionOwner`, held by the URI grant service.
//! The app host calls its existing owner directly; a system_server keeps
//! ownership through the device bridge's two internal API leaves.

use std::sync::Arc;

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::{Binder, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_app_iurigrantsmanager as ugm;
use aim_service_aidl::dev_aim_server_ibridge as bridge;

use crate::app_host::{AppHost, uri_grants::Owner};
use crate::system::System;

/// `Intent.FLAG_GRANT_READ_URI_PERMISSION`.
const READ: i32 = 1;

pub(crate) enum UriOwner {
    Native { host: Arc<AppHost>, owner: Owner },
    Remote { bridge: Arc<Strong>, token: Strong },
}

struct StringUri<'a>(&'a str);
impl WriteParcelable for StringUri<'_> {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(1);
        p.write_string8(Some(self.0));
    }
}

impl UriOwner {
    pub(crate) fn grant(
        &self,
        system: &Arc<System>,
        source_uid: i32,
        target: &str,
        uri: &str,
        source_user: i32,
        target_user: i32,
    ) -> Result<(), Exception> {
        let _identity = aim_binder_host::local::clear_calling_identity();
        match self {
            Self::Native { host, owner } => {
                host.grant_clipboard_uri(*owner, source_uid, target, uri, source_user, target_user)
            }
            Self::Remote { token, .. } => system.call(
                "uri_grants",
                ugm::GRANT_URI_PERMISSION_FROM_OWNER,
                |p| {
                    ugm::GrantUriPermissionFromOwner {
                        owner: Some(Binder::Handle(token.handle)),
                        from_uid: source_uid,
                        target_pkg: Some(target.into()),
                        uri: Some(StringUri(uri)),
                        mode: READ,
                        source_user_id: source_user,
                        target_user_id: target_user,
                    }
                    .write(p)
                },
                ugm::read_grant_uri_permission_from_owner_reply,
            ),
        }
    }

    pub(crate) fn revoke(&self, uri: &str, source_user: i32) -> Result<(), Exception> {
        let _identity = aim_binder_host::local::clear_calling_identity();
        match self {
            Self::Native { host, owner } => {
                host.revoke_clipboard_uri(*owner, uri, source_user);
                Ok(())
            }
            Self::Remote { bridge, token } => {
                let mut data = Parcel::new();
                bridge::RevokeUriPermissionFromOwner {
                    owner: Some(Binder::Handle(token.handle)),
                    uri: Some(StringUri(uri)),
                    mode_flags: READ,
                    source_user_id: source_user,
                }
                .write(&mut data);
                let failed = |s| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        format!("URI grant bridge unavailable: status {s}"),
                    )
                };
                let reply = bridge
                    .transact(bridge::REVOKE_URI_PERMISSION_FROM_OWNER, &data, false)
                    .map_err(failed)?;
                bridge::read_revoke_uri_permission_from_owner_reply(&mut reply.reader())
                    .map_err(failed)?
            }
        }
    }
}
