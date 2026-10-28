//! Original Java clone-provider policy, over the live UserManager owner.
use super::AppHost;
use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::dev_aim_server_iproviderqueries as queries;
use std::sync::Arc;

/// Consume the complete original ProviderInfo parcel; the grant owner reads
/// only its authority. Reuse the pinned component/ApplicationInfo decoder.
pub(super) struct ProviderAuthority(pub Option<String>);
impl aim_service_aidl::ReadParcelable for ProviderAuthority {
    fn read_from(
        r: &mut aim_binder_host::parcel::Reader<'_>,
    ) -> aim_binder_host::parcel::Result<Self> {
        let info = crate::package::reply::provider_info(r)?;
        use crate::shadow::Value;
        let Value::Fields(fields) = info else {
            return Err(aim_binder_host::parcel::BAD_VALUE);
        };
        match fields.into_iter().find(|(name, _)| name == "authority") {
            Some((_, Value::Str(authority))) => Ok(Self(Some(authority))),
            Some((_, Value::Null)) => Ok(Self(None)),
            _ => Err(aim_binder_host::parcel::BAD_VALUE),
        }
    }
}

impl AppHost {
    pub(super) fn attach_provider_queries(&self, owner: Option<Binder>) -> Result<(), Exception> {
        let Some(Binder::Handle(handle)) = owner else {
            return Err(Exception::illegal_argument("no provider query bridge"));
        };
        let owner = Arc::new(self.process.strong(handle));
        let weak = self.me.clone();
        self.process.link_to_death(
            &owner,
            Box::new(move || {
                if let Some(host) = weak.upgrade() {
                    let mut current = host.provider_queries.lock().unwrap();
                    if current.as_ref().is_some_and(|p| p.handle == handle) {
                        current.take();
                    }
                }
            }),
        );
        *self.provider_queries.lock().unwrap() = Some(owner);
        Ok(())
    }

    pub(super) fn provider_clone_redirected(
        &self,
        authority: Option<String>,
        calling_uid: i32,
        user_id: i32,
    ) -> Result<bool, Exception> {
        let owner = self
            .provider_queries
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "provider query owner unavailable"))?;
        let mut data = Parcel::new();
        queries::IsCloneRedirected {
            authority,
            calling_uid,
            user_id,
        }
        .write(&mut data);
        let transport = |status| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("provider query: binder status {status}"),
            )
        };
        let reply = owner
            .transact(queries::IS_CLONE_REDIRECTED, &data, false)
            .map_err(transport)?;
        queries::read_is_clone_redirected_reply(&mut reply.reader()).map_err(transport)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::local::{Call, Reply, Service};
    use aim_binder_host::parcel::{EX_SECURITY, UNKNOWN_TRANSACTION};
    use aim_service_aidl::{
        dev_aim_server_ipackagebootstrapbridge as bridge, dev_aim_server_isystemprocess as sp,
    };

    struct Original;
    impl Service for Original {
        fn descriptor(&self) -> &str {
            queries::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(call.sender_euid, 1000);
            let a = queries::IsCloneRedirected::read(&mut call.data)?;
            assert_eq!((a.calling_uid, a.user_id), (1010100, 0));
            let mut p = Parcel::new();
            match a.authority.as_deref() {
                Some("transport") => return Err(UNKNOWN_TRANSACTION),
                Some("denied") => p.write_exception(&Exception::security("owner denied")),
                _ => queries::write_is_clone_redirected_reply(
                    &mut p,
                    a.authority.as_deref() == Some("media"),
                ),
            }
            Ok(p)
        }
    }

    #[test]
    fn provider_queries_forward_scope_and_preserve_owner_failures() {
        crate::system::with_discovery_binders(|host, _, _, export| {
            let leaf = super::super::package_bootstrap::PackageBootstrap::new(host.clone());
            let ask = |uid, authority: &str| {
                let mut p = Parcel::new();
                bridge::IsProviderCloneRedirected {
                    authority: Some(authority.into()),
                    calling_uid: 1010100,
                    user_id: 0,
                }
                .write(&mut p);
                let r = leaf
                    .transact(&mut Call {
                        code: bridge::IS_PROVIDER_CLONE_REDIRECTED,
                        flags: 0,
                        sender_pid: 77,
                        sender_euid: uid,
                        data: p.reader(),
                    })
                    .unwrap();
                bridge::read_is_provider_clone_redirected_reply(&mut r.reader()).unwrap()
            };
            assert_eq!(ask(1000, "media").unwrap_err().code, EX_ILLEGAL_STATE);
            let service = super::super::system_process::SystemProcess::new(host.clone());
            let owner = export(Arc::new(Original));
            let mut p = Parcel::new();
            sp::AttachProviderQueries {
                queries: Some(Binder::Handle(owner.handle)),
            }
            .write(&mut p);
            for uid in [10100, 1000] {
                let r = service
                    .transact(&mut Call {
                        code: sp::ATTACH_PROVIDER_QUERIES,
                        flags: 0,
                        sender_pid: 77,
                        sender_euid: uid,
                        data: p.reader(),
                    })
                    .unwrap();
                let result = sp::read_attach_provider_queries_reply(&mut r.reader()).unwrap();
                if uid == 1000 {
                    result.unwrap();
                } else {
                    assert_eq!(result.unwrap_err().code, EX_SECURITY);
                }
            }
            assert!(ask(1000, "media").unwrap());
            assert!(!ask(1000, "other").unwrap());
            assert_eq!(ask(10100, "media").unwrap_err().code, EX_SECURITY);
            assert_eq!(
                ask(1000, "denied").unwrap_err(),
                Exception::security("owner denied")
            );
            assert!(
                ask(1000, "transport")
                    .unwrap_err()
                    .message
                    .contains("binder status")
            );
            host.provider_queries.lock().unwrap().take();
            assert_eq!(ask(1000, "media").unwrap_err().code, EX_ILLEGAL_STATE);
        });
    }
}
