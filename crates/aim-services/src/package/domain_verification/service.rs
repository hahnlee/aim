//! Native domain endpoint, pending the complete service's migration gates (#957).
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_STATE, EX_UNSUPPORTED_OPERATION, Exception, Parcel, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager as api;

use super::enforcer::Operation;
use crate::system::System;

pub struct DomainQueries {
    system: Weak<System>,
    persistence: Option<Arc<Mutex<crate::package::owner::Store>>>,
}

impl DomainQueries {
    pub fn from_system(system: &Arc<System>) -> Arc<Self> {
        Arc::new(Self {
            system: Arc::downgrade(system),
            persistence: None,
        })
    }
    pub fn with_persistence(
        system: &Arc<System>,
        persistence: Arc<Mutex<crate::package::owner::Store>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            system: Arc::downgrade(system),
            persistence: Some(persistence),
        })
    }

    fn set_link_handling(
        &self,
        pid: i32,
        uid: i32,
        name: Option<&str>,
        allowed: bool,
        user: i32,
    ) -> Result<(), Exception> {
        let system = self.system.upgrade().ok_or_else(|| {
            Exception::new(EX_ILLEGAL_STATE, "native system owner is unavailable")
        })?;
        loop {
            let capture = system.capture_package_queries()?;
            let domains = capture.domains().ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "native domain owner is unavailable")
            })?;
            let bridge = system.package_bootstrap()?;
            let visible = match system.authorize_package_domain(
                &bridge,
                &capture,
                pid,
                uid,
                Operation::UserSelect(name, user),
            ) {
                Ok(visible) => visible,
                Err(error) => {
                    system.check_package_bootstrap(&bridge)?;
                    if !Arc::ptr_eq(&capture, &system.capture_package_queries()?) {
                        continue;
                    }
                    return Err(error);
                }
            };
            let missing = || Exception {
                code: aim_binder_host::parcel::EX_SERVICE_SPECIFIC,
                message: String::new(),
                service_specific: 1,
            };
            if !visible {
                return Err(missing());
            }
            let name = name
                .filter(|name| domains.owner().package(name).is_some())
                .ok_or_else(missing)?;
            let persistence = self.persistence.as_ref().ok_or_else(|| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    "native domain persistence owner is unavailable",
                )
            })?;
            let mut owner = domains.owner().clone();
            owner
                .set_link_handling_internal(Some(name), allowed, user, &[])
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
            let update = capture
                .prepare_domain_update(owner)
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
            match system.commit_package_domains_if_current(
                &bridge,
                update,
                &mut persistence.lock().unwrap(),
            ) {
                Ok(Some(_)) => return Ok(()),
                Ok(None) => continue,
                Err(error) => {
                    return Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        format!(
                            "domain write failed (committed={}): {}",
                            error.committed, error.message
                        ),
                    ));
                }
            }
        }
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
    fn set_verifier_status(
        &self,
        pid: i32,
        uid: i32,
        id: Option<&str>,
        hosts: Option<super::domain_set::DomainSet>,
        state: i32,
    ) -> Result<i32, Exception> {
        use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_NULL_POINTER};
        let system = self.system.upgrade().ok_or_else(|| {
            Exception::new(EX_ILLEGAL_STATE, "native system owner is unavailable")
        })?;
        let bridge = system.package_bootstrap()?;
        let strict = bridge
            .domain_uuid_strict_validation()
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("UUID policy: {e:?}")))?;
        system.check_package_bootstrap(&bridge)?;
        let id = id.ok_or_else(|| Exception::new(EX_NULL_POINTER, if strict {
            "Attempt to invoke virtual method 'int java.lang.String.length()' on a null object reference"
        } else { "Attempt to invoke virtual method 'java.lang.String[] java.lang.String.split(java.lang.String)' on a null object reference" }))?;
        let id =
            super::uuid::parse(id, strict).map_err(|e| Exception::new(EX_ILLEGAL_ARGUMENT, e))?;
        let hosts = hosts.ok_or_else(|| Exception::new(EX_NULL_POINTER,
            "Attempt to invoke virtual method 'java.util.Set android.content.pm.verify.domain.DomainSet.getDomains()' on a null object reference"))?
            .resolve(&system.process()).map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("DomainSet blob: {e}")))?;
        if state != 1 && state < 1024 {
            return Err(Exception::new(
                EX_ILLEGAL_ARGUMENT,
                format!("Caller is not allowed to set state code {state}"),
            ));
        }
        loop {
            let capture = system.capture_package_queries()?;
            let domains = capture.domains().ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "native domain owner is unavailable")
            })?;
            match system.authorize_package_domain(&bridge, &capture, pid, uid, Operation::Verifier)
            {
                Ok(_) => (),
                Err(error) => {
                    system.check_package_bootstrap(&bridge)?;
                    if !Arc::ptr_eq(&capture, &system.capture_package_queries()?) {
                        continue;
                    }
                    return Err(error);
                }
            }
            let Some(package) = domains.owner().package_by_id(&id) else {
                return Ok(1);
            };
            let code = capture
                .scan()
                .owner()
                .loaded_packages()
                .get(&package.name)
                .ok_or_else(|| Exception {
                    code: aim_binder_host::parcel::EX_SERVICE_SPECIFIC,
                    message: String::new(),
                    service_specific: 1,
                })?;
            if hosts.is_empty() {
                return Err(Exception::new(
                    EX_ILLEGAL_ARGUMENT,
                    "Provided domain set cannot be empty",
                ));
            }
            let policy = domains
                .collector_policy(&code.package.package_name)
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
            let declared = super::collector::collect(
                &code.package,
                policy,
                super::collector::Kind::ValidAutoVerify,
            );
            if hosts
                .iter()
                .any(|host| host.as_ref().is_none_or(|host| !declared.contains(host)))
            {
                return Ok(2);
            }
            let persistence = self.persistence.as_ref().ok_or_else(|| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    "native domain persistence owner is unavailable",
                )
            })?;
            let mut owner = domains.owner().clone();
            let mut names = hosts.iter().flatten().cloned().collect();
            let status = owner
                .set_verifier_status(&id, Some(&code.package), policy, &mut names, state)
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
            if status != 0 {
                return Ok(status);
            }
            let update = capture
                .prepare_domain_update(owner)
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
            match system.commit_package_domains_if_current(
                &bridge,
                update,
                &mut persistence.lock().unwrap(),
            ) {
                Ok(Some(_)) => return Ok(0),
                Ok(None) => continue,
                Err(error) => {
                    return Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        format!(
                            "domain write failed (committed={}): {}",
                            error.committed, error.message
                        ),
                    ));
                }
            }
        }
    }
    fn info(
        &self,
        pid: i32,
        uid: i32,
        name: Option<&str>,
    ) -> Result<Option<super::parcels::Info>, Exception> {
        self.query(
            pid,
            uid,
            name,
            Operation::Info,
            |domains, name, code, prefix| {
                domains
                    .verification(name, code)
                    .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?
                    .as_ref()
                    .map(|(id, states)| {
                        super::parcels::Info::prepare(prefix, id, name, states).map_err(|e| {
                            Exception::new(EX_ILLEGAL_STATE, format!("domain info parcel: {e}"))
                        })
                    })
                    .transpose()
            },
        )
    }
    fn user_state(
        &self,
        pid: i32,
        uid: i32,
        name: Option<&str>,
        user: i32,
    ) -> Result<Option<super::parcels::UserState>, Exception> {
        self.query(
            pid,
            uid,
            name,
            Operation::UserQuery(name, user),
            |domains, name, code, prefix| {
                domains
                    .user_state(name, code, user)
                    .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?
                    .as_ref()
                    .map(|(id, allowed, states)| {
                        super::parcels::UserState::prepare(prefix, id, name, user, *allowed, states)
                            .map_err(|e| {
                                Exception::new(
                                    EX_ILLEGAL_STATE,
                                    format!("domain user-state parcel: {e}"),
                                )
                            })
                    })
                    .transpose()
            },
        )
    }
    fn query<T>(
        &self,
        pid: i32,
        uid: i32,
        name: Option<&str>,
        operation: Operation<'_>,
        build: impl FnOnce(
            &crate::package::scan_snapshot::query_state::NativeDomains,
            &str,
            &crate::package::pkg::AndroidPackage,
            usize,
        ) -> Result<Option<T>, Exception>,
    ) -> Result<Option<T>, Exception> {
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
        let allowed = system.authorize_package_domain(&bridge, &capture, pid, uid, operation)?;
        let missing = || Exception {
            code: aim_binder_host::parcel::EX_SERVICE_SPECIFIC,
            message: String::new(),
            service_specific: 1,
        };
        if !allowed {
            return Err(missing());
        }
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
        let mut prefix = Parcel::new();
        prefix.write_no_exception();
        prefix.write_i32(1);
        build(
            capture.domains().unwrap(),
            name,
            &code.package,
            prefix.data().len(),
        )
    }
    fn owners(
        &self,
        pid: i32,
        uid: i32,
        host: Option<&str>,
        user: i32,
    ) -> Result<Vec<Option<super::parcels::Owner>>, Exception> {
        let host =
            host.ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, ""))?;
        let system = self.system.upgrade().ok_or_else(|| {
            Exception::new(EX_ILLEGAL_STATE, "native system owner is unavailable")
        })?;
        let capture = system.capture_package_queries()?;
        let domains = capture.domains().ok_or_else(|| {
            Exception::new(EX_ILLEGAL_STATE, "native domain owner is unavailable")
        })?;
        let bridge = system.package_bootstrap()?;
        system.authorize_package_domain(&bridge, &capture, pid, uid, Operation::Owners(user))?;
        let result = domains.owners(capture.scan().owner(), host, user, |name, sdk| {
            bridge
                .domain_verification_settings_v2(name, sdk)
                .map_err(|e| format!("{e:?}"))
        });
        system.check_package_domain_capture(&bridge, &capture)?;
        result
            .map(|owners| {
                owners
                    .into_iter()
                    .map(|(name, overrideable)| Some(super::parcels::Owner { name, overrideable }))
                    .collect()
            })
            .map_err(|e| match e {
                super::owner::OwnersError::Input(message) => {
                    Exception::new(EX_ILLEGAL_STATE, message)
                }
                super::owner::OwnersError::Ordering(message) => {
                    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT, message)
                }
            })
    }
    fn uri_groups(
        &self,
        name: Option<&str>,
        hosts: Option<&[Option<String>]>,
    ) -> Result<super::parcels::UriGroups, Exception> {
        let system = self.system.upgrade().ok_or_else(|| {
            Exception::new(EX_ILLEGAL_STATE, "native system owner is unavailable")
        })?;
        let capture = system.capture_package_queries()?;
        let domains = capture.domains().ok_or_else(|| {
            Exception::new(EX_ILLEGAL_STATE, "native domain owner is unavailable")
        })?;
        let groups = domains
            .owner()
            .uri_groups_query(name, hosts)
            .map_err(|e| Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, e))?;
        super::parcels::UriGroups::prepare(&groups)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("URI-group parcel: {e}")))
    }
}

impl Service for DomainQueries {
    fn accepts_fds(&self) -> bool {
        true
    }
    fn descriptor(&self) -> &str {
        api::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if !api::METHODS.iter().any(|(code, _)| *code == call.code) {
            return Err(UNKNOWN_TRANSACTION);
        }
        let mut reply = Parcel::new();
        if call.code == api::SET_DOMAIN_VERIFICATION_STATUS {
            let args = api::SetDomainVerificationStatus::<super::domain_set::DomainSet>::read(
                &mut call.data,
            )?;
            if call.data.remaining() != 0 {
                return Err(BAD_VALUE);
            }
            match self.set_verifier_status(
                call.sender_pid,
                call.sender_euid as i32,
                args.domain_set_id.as_deref(),
                args.domains,
                args.state,
            ) {
                Ok(status) => api::write_set_domain_verification_status_reply(&mut reply, status),
                Err(error) if error.code == aim_binder_host::parcel::EX_SERVICE_SPECIFIC => {
                    reply.write_exception_message(&error, None)
                }
                Err(error) => reply.write_exception(&error),
            }
            return Ok(reply);
        }
        if call.code == api::GET_URI_RELATIVE_FILTER_GROUPS {
            let args = api::GetUriRelativeFilterGroups::read(&mut call.data)?;
            if call.data.remaining() != 0 {
                return Err(BAD_VALUE);
            }
            match self.uri_groups(args.package_name.as_deref(), args.domains.as_deref()) {
                Ok(groups) => {
                    api::write_get_uri_relative_filter_groups_reply(&mut reply, Some(&groups))
                }
                Err(error) => reply.write_exception(&error),
            }
            return Ok(reply);
        }
        if call.code == api::SET_DOMAIN_VERIFICATION_LINK_HANDLING_ALLOWED {
            let args = api::SetDomainVerificationLinkHandlingAllowed::read(&mut call.data)?;
            if call.data.remaining() != 0 {
                return Err(BAD_VALUE);
            }
            match self.set_link_handling(
                call.sender_pid,
                call.sender_euid as i32,
                args.package_name.as_deref(),
                args.allowed,
                args.user_id,
            ) {
                Ok(()) => {
                    api::write_set_domain_verification_link_handling_allowed_reply(&mut reply)
                }
                Err(error) if error.code == aim_binder_host::parcel::EX_SERVICE_SPECIFIC => {
                    reply.write_exception_message(&error, None)
                }
                Err(error) => reply.write_exception(&error),
            }
            return Ok(reply);
        }
        if call.code == api::GET_OWNERS_FOR_DOMAIN {
            let args = api::GetOwnersForDomain::read(&mut call.data)?;
            if call.data.remaining() != 0 {
                return Err(BAD_VALUE);
            }
            match self.owners(
                call.sender_pid,
                call.sender_euid as i32,
                args.domain.as_deref(),
                args.user_id,
            ) {
                Ok(owners) => api::write_get_owners_for_domain_reply(&mut reply, Some(&owners)),
                Err(error) if error.code == aim_binder_host::parcel::EX_NULL_POINTER => {
                    reply.write_exception_message(&error, None)
                }
                Err(error) => reply.write_exception(&error),
            }
            return Ok(reply);
        }
        if call.code == api::GET_DOMAIN_VERIFICATION_USER_STATE {
            let args = api::GetDomainVerificationUserState::read(&mut call.data)?;
            if call.data.remaining() != 0 {
                return Err(BAD_VALUE);
            }
            match self.user_state(
                call.sender_pid,
                call.sender_euid as i32,
                args.package_name.as_deref(),
                args.user_id,
            ) {
                Ok(state) => {
                    api::write_get_domain_verification_user_state_reply(&mut reply, state.as_ref())
                }
                Err(error) if error.code == aim_binder_host::parcel::EX_SERVICE_SPECIFIC => {
                    reply.write_exception_message(&error, None)
                }
                Err(error) => reply.write_exception(&error),
            }
            return Ok(reply);
        }
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
