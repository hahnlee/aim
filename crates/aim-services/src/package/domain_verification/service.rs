//! Native domain queries, pending the complete service's migration gates (#957).
use std::sync::{Arc, Weak};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_STATE, EX_UNSUPPORTED_OPERATION, Exception, Parcel, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager as api;

use super::enforcer::Operation;
use crate::system::System;

pub struct DomainQueries {
    system: Weak<System>,
}

impl DomainQueries {
    pub fn from_system(system: &Arc<System>) -> Arc<Self> {
        Arc::new(Self {
            system: Arc::downgrade(system),
        })
    }

    fn names(&self, pid: i32, uid: i32) -> Result<Vec<String>, Exception> {
        let system = self.system.upgrade().ok_or_else(|| {
            Exception::new(EX_ILLEGAL_STATE, "native system owner is unavailable")
        })?;
        let capture = system.capture_package_queries()?;
        let domains = capture.domains().ok_or_else(|| {
            Exception::new(EX_ILLEGAL_STATE, "native domain owner is unavailable")
        })?;
        let bridge = system.package_bootstrap()?;
        system.authorize_package_domain(&bridge, &capture, pid, uid, Operation::Verifier)?;
        Ok(domains.owner().valid_verification_package_names())
    }
    fn info(
        &self,
        pid: i32,
        uid: i32,
        name: Option<&str>,
    ) -> Result<Option<super::parcels::Info>, Exception> {
        let system = self.system.upgrade().ok_or_else(|| {
            Exception::new(EX_ILLEGAL_STATE, "native system owner is unavailable")
        })?;
        let capture = system.capture_package_queries()?;
        if capture.domains().is_none() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "native domain owner is unavailable",
            ));
        }
        let bridge = system.package_bootstrap()?;
        system.authorize_package_domain(&bridge, &capture, pid, uid, Operation::Info)?;
        let missing = || Exception {
            code: aim_binder_host::parcel::EX_SERVICE_SPECIFIC,
            message: String::new(),
            service_specific: 1,
        };
        let name = name.ok_or_else(missing)?;
        let normalized = capture
            .scan()
            .owner()
            .settings
            .renamed_packages
            .iter()
            .rev()
            .find(|(new, _)| new == name)
            .map_or(name, |(_, old)| old.as_str());
        let resolver = crate::package::resolve::Resolver::default();
        let resolution = resolver.resolution(capture.state()).map_err(|e| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("domain package resolution: {e:?}"),
            )
        })?;
        let query = crate::package::query::Query {
            state: capture.state(),
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        let resolved = query.resolve_internal_package_name(normalized, -1);
        let code = capture
            .scan()
            .owner()
            .loaded_packages()
            .get(&resolved)
            .ok_or_else(missing)?;
        let verification = capture
            .domains()
            .unwrap()
            .verification(name, &code.package)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        verification
            .as_ref()
            .map(|(id, states)| {
                let mut prefix = Parcel::new();
                prefix.write_no_exception();
                prefix.write_i32(1);
                super::parcels::Info::prepare(prefix.data().len(), id, name, states).map_err(|e| {
                    Exception::new(EX_ILLEGAL_STATE, format!("domain info parcel: {e}"))
                })
            })
            .transpose()
    }
}

impl Service for DomainQueries {
    fn descriptor(&self) -> &str {
        api::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if !api::METHODS.iter().any(|(code, _)| *code == call.code) {
            return Err(UNKNOWN_TRANSACTION);
        }
        let mut reply = Parcel::new();
        if call.code == api::GET_DOMAIN_VERIFICATION_INFO {
            let args = api::GetDomainVerificationInfo::read(&mut call.data)?;
            if call.data.remaining() != 0 {
                return Err(BAD_VALUE);
            }
            match self.info(
                call.sender_pid,
                call.sender_euid as i32,
                args.package_name.as_deref(),
            ) {
                Ok(info) => {
                    api::write_get_domain_verification_info_reply(&mut reply, info.as_ref())
                }
                Err(error) if error.code == aim_binder_host::parcel::EX_SERVICE_SPECIFIC => {
                    reply.write_exception_message(&error, None)
                }
                Err(error) => reply.write_exception(&error),
            }
            return Ok(reply);
        }
        if call.code != api::QUERY_VALID_VERIFICATION_PACKAGE_NAMES {
            call.data.enforce_interface(api::DESCRIPTOR)?;
            reply.write_exception(&Exception::new(
                EX_UNSUPPORTED_OPERATION,
                "native domain method is not implemented",
            ));
            return Ok(reply);
        }
        api::QueryValidVerificationPackageNames::read(&mut call.data)?;
        if call.data.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        match self.names(call.sender_pid, call.sender_euid as i32) {
            Ok(names) => api::write_query_valid_verification_package_names_reply(
                &mut reply,
                &Some(names.into_iter().map(Some).collect()),
            ),
            Err(error) => reply.write_exception(&error),
        }
        Ok(reply)
    }
}
