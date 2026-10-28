//! Original Java verifier front end over the session's NativeDomains and
//! durable package owner; other writes and verifier requests remain #1763.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_STATE, EX_SERVICE_SPECIFIC, Exception, Parcel,
};
use aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager as dv;

use super::kept::RawBundle;
use super::{AppHost, PER_USER_RANGE};
use crate::package::apps_filter::should_filter_application;
use crate::package::domain_verification::uri_parcel::{self, Group};
use crate::package::domain_verification::{agent, domain_set::DomainSet, parcels};
use crate::package::query::Query;
use crate::package::scan_snapshot::query_state::{Capture, NativeDomains};
use aim_service_aidl::dev_aim_server_iapphostdomainverification as original;

const SYSTEM_UID: i32 = 1000;
const INTERACT_ACROSS_USERS: &str = "android.permission.INTERACT_ACROSS_USERS";
/// `DomainVerificationManager.INTERNAL_ERROR_NAME_NOT_FOUND`.
const ERROR_NAME_NOT_FOUND: i32 = 1;
/// `PackageManager.VERSION_CODE_HIGHEST`.
const VERSION_CODE_HIGHEST: i64 = -1;

/// The `NameNotFoundException` `DomainVerificationManagerStub` rethrows.
fn name_not_found() -> Exception {
    Exception {
        code: EX_SERVICE_SPECIFIC,
        message: String::new(),
        service_specific: ERROR_NAME_NOT_FOUND,
        parcelable: None,
    }
}

pub struct DomainVerification {
    pub(super) host: Arc<AppHost>,
}

impl DomainVerification {
    pub fn new(host: Arc<AppHost>) -> Self {
        DomainVerification { host }
    }

    fn unimplemented(&self, what: &str) -> Exception {
        self.host
            .unimplemented_named("domain_verification", &format!("{what} (#1763)"))
    }

    pub(super) fn domains(capture: &Capture) -> Result<&NativeDomains, Exception> {
        capture
            .domains()
            .map(Arc::as_ref)
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "native domain owner unavailable"))
    }

    /// `getPackageUid(package, 0, user)`, or -1.
    fn package_uid(&self, capture: &Capture, package: &str, user: i32) -> Result<i32, Exception> {
        let state = capture.state();
        let resolution = super::package_resolution(capture, "domain")?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        query
            .package_uid(package, 0, user)
            .map_err(|e| self.unimplemented(e.0))?
    }

    /// The proxy's `isCallerVerifier`: `uid` is a verifier package's
    /// (`isCallerPackage`).
    pub(super) fn is_caller_verifier(
        &self,
        capture: &Capture,
        uid: i32,
    ) -> Result<bool, Exception> {
        let resolution = super::package_resolution(capture, "domain")?;
        let (legacy, modern) = agent::verifiers(capture.state(), &resolution)?;
        for package in agent::proxy_packages(legacy, modern) {
            if self.package_uid(capture, &package, uid / PER_USER_RANGE as i32)? == uid {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `Context.enforcePermission` of `permission` for `uid`.
    fn enforce(&self, permission: &str, uid: i32, message: &str) -> Result<(), Exception> {
        if self.host.check_permission(Some(permission), uid)? != 0 {
            return Err(Exception::security(format!(
                "{message}: uid {uid} does not have {permission}."
            )));
        }
        Ok(())
    }

    /// `DomainVerificationEnforcer.assertApprovedUserStateQuerent`:
    /// whether `package` is visible to the caller (`filterAppAccess`).
    fn assert_approved_user_state_querent(
        &self,
        capture: &Capture,
        uid: i32,
        package: &str,
        user: i32,
    ) -> Result<bool, Exception> {
        let caller_user = uid / PER_USER_RANGE as i32;
        if caller_user != user {
            self.enforce(
                INTERACT_ACROSS_USERS,
                uid,
                "Caller is not allowed to edit other users",
            )?;
        }
        let state = capture.state();
        let resolution = super::package_resolution(capture, "domain")?;
        for id in [caller_user, user] {
            if !state.users.contains_key(&id) {
                return Err(Exception::security(format!("User {id} does not exist")));
            }
        }
        let filtered = should_filter_application(
            &state,
            &resolution.apps_filter,
            state.packages.get(package),
            uid,
            user,
            true,
            true,
        )
        .map_err(|e| self.unimplemented(e.0))?;
        Ok(!filtered)
    }

    /// `getDomainVerificationUserState`: the package's web domains, each
    /// verified, selected by the user or neither, and whether the user
    /// allows its link handling; none for a package not attached.
    fn user_state(
        &self,
        uid: i32,
        name: Option<&str>,
        user: i32,
    ) -> Result<Option<parcels::UserState>, Exception> {
        let capture = self.host.package_capture()?;
        let name = name.ok_or_else(name_not_found)?;
        if !self.assert_approved_user_state_querent(&capture, uid, name, user)? {
            return Err(name_not_found());
        }
        let state = capture.state();
        let code = state
            .packages
            .get(name)
            .and_then(|ps| ps.pkg.as_deref())
            .ok_or_else(name_not_found)?;
        let Some((id, allowed, states)) = Self::domains(&capture)?
            .user_state(name, code, user)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?
        else {
            return Ok(None);
        };
        let mut prefix = Parcel::new();
        prefix.write_no_exception();
        prefix.write_i32(1);
        parcels::UserState::prepare(prefix.data().len(), &id, name, user, allowed, &states)
            .map(Some)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("domain user-state parcel: {e}")))
    }

    /// `queryValidVerificationPackageNames`.
    fn valid_verification_package_names(&self) -> Result<Vec<String>, Exception> {
        let capture = self.host.package_capture()?;
        Ok(Self::domains(&capture)?
            .owner()
            .valid_verification_package_names())
    }

    /// `getDomainVerificationInfo`: the package's states with its valid
    /// auto-verify domains, none for a package not attached or without
    /// them.
    fn info(&self, uid: i32, name: Option<&str>) -> Result<Option<parcels::Info>, Exception> {
        let capture = self.host.package_capture()?;
        let name = name.ok_or_else(name_not_found)?;
        let state = capture.state();
        let resolution = super::package_resolution(&capture, "domain")?;
        let query = Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        let resolved = query.resolve_internal_package_name(name, VERSION_CODE_HIGHEST);
        let code = state
            .packages
            .get(&resolved)
            .and_then(|ps| ps.pkg.as_deref())
            .ok_or_else(name_not_found)?;
        let Some((id, states)) = Self::domains(&capture)?
            .verification(name, code)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?
        else {
            return Ok(None);
        };
        let mut prefix = Parcel::new();
        prefix.write_no_exception();
        prefix.write_i32(1);
        parcels::Info::prepare(prefix.data().len(), &id, name, &states)
            .map(Some)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("domain info parcel: {e}")))
    }

    /// Only typed, original-Java-converted groups reach this system-UID backend.
    /// DVS publishes each key in order, including before a later conversion fails,
    /// and does not schedule a settings write from this setter.
    fn set_uri_group(
        &self,
        name: Option<&str>,
        id: Option<&str>,
        domain: Option<&str>,
        groups: Option<Vec<Option<Group>>>,
    ) -> Result<(), Exception> {
        let groups =
            uri_parcel::groups_to_model(groups).map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        let name = name.ok_or_else(name_not_found)?;
        let id =
            id.ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "missing URI package identity"))?;
        let domain =
            domain.ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "unvalidated URI domain"))?;
        let system = self.host.package_system()?;
        let bridge = system.package_bootstrap()?;
        loop {
            let capture = system.capture_package_queries_for(&bridge)?;
            let domains = Self::domains(&capture)?;
            if !domains
                .owner()
                .package(name)
                .is_some_and(|package| package.id == id)
            {
                // A lifecycle change after Java acquired the attached state
                // retires that map; never apply its updates to a new attachment.
                return Ok(());
            }
            let mut owner = domains.owner().clone();
            owner
                .set_uri_groups(name, &[(domain.to_owned(), Some(groups.clone()))])
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
            if owner.persisted() == domains.owner().persisted() {
                return Ok(());
            }
            let update = capture
                .prepare_runtime_domain_update(owner)
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
            if system
                .publish_runtime_package_domains(&bridge, update)?
                .is_some()
            {
                return Ok(());
            }
        }
    }
}

impl DomainVerification {
    fn forward(&self, call: &mut Call<'_>, uid: i32) -> Reply {
        let mut request = Parcel::new();
        let code = match call.code {
            dv::QUERY_VALID_VERIFICATION_PACKAGE_NAMES => {
                dv::QueryValidVerificationPackageNames::read(&mut call.data)?;
                original::QueryValidVerificationPackageNames { uid }.write(&mut request);
                original::QUERY_VALID_VERIFICATION_PACKAGE_NAMES
            }
            dv::GET_DOMAIN_VERIFICATION_INFO => {
                let a = dv::GetDomainVerificationInfo::read(&mut call.data)?;
                original::GetDomainVerificationInfo {
                    uid,
                    package_name: a.package_name,
                }
                .write(&mut request);
                original::GET_DOMAIN_VERIFICATION_INFO
            }
            dv::SET_DOMAIN_VERIFICATION_STATUS => {
                let a = dv::SetDomainVerificationStatus::<DomainSet>::read(&mut call.data)?;
                original::SetDomainVerificationStatus {
                    uid,
                    id: a.domain_set_id,
                    domains: a
                        .domains
                        .map(|domains| domains.forward(&self.host.process))
                        .transpose()?,
                    state: a.state,
                }
                .write(&mut request);
                original::SET_DOMAIN_VERIFICATION_STATUS
            }
            dv::SET_URI_RELATIVE_FILTER_GROUPS => {
                let a = dv::SetUriRelativeFilterGroups::<RawBundle>::read(&mut call.data)?;
                original::SetUriRelativeFilterGroups {
                    uid,
                    package_name: a.package_name,
                    groups: a.domain_to_groups_bundle,
                }
                .write(&mut request);
                original::SET_URI_RELATIVE_FILTER_GROUPS
            }
            dv::GET_URI_RELATIVE_FILTER_GROUPS => {
                let a = dv::GetUriRelativeFilterGroups::read(&mut call.data)?;
                original::GetUriRelativeFilterGroups {
                    uid,
                    package_name: a.package_name,
                    domains: a.domains,
                }
                .write(&mut request);
                original::GET_URI_RELATIVE_FILTER_GROUPS
            }
            _ => unreachable!(),
        };
        let frontend = self.host.domain_frontend.lock().unwrap().clone();
        match frontend {
            Some(frontend) => frontend
                .transact_as_owner(code, &request, false)
                .map(|reply| reply.into_parcel()),
            None => super::reply(Err(Exception::new(
                EX_ILLEGAL_STATE,
                "original domain verifier front end unavailable (#1763)",
            ))),
        }
    }
}

/// State access for the original Java front end. Never accepts app identities.
pub(super) struct Backend(DomainVerification);
impl Backend {
    pub(super) fn new(host: Arc<AppHost>) -> Self {
        Self(DomainVerification::new(host))
    }

    fn set_status(
        &self,
        pid: i32,
        args: original::SetDomainVerificationStatus<DomainSet>,
        mut capture: Arc<Capture>,
    ) -> Result<i32, Exception> {
        let system = self.0.host.package_system()?;
        let disk = loop {
            match system.package_persistence_owner(&capture) {
                Ok((_, disk)) => break disk,
                Err(error) => {
                    let current = self.0.host.package_capture()?;
                    if Arc::ptr_eq(&capture, &current) {
                        return Err(error);
                    }
                    capture = current;
                }
            }
        };
        // The retained Java enforcer checked the explicit caller. Acquire the
        // disk owner against a current capture; the native mutation also retries
        // if another package write publishes before its commit.
        let driver = crate::package::domain_verification::service::DomainQueries::with_persistence(
            system, disk,
        );
        driver.set_verifier_status(
            pid,
            SYSTEM_UID,
            args.id.as_deref(),
            args.domains,
            args.state,
        )
    }
}
impl Service for Backend {
    fn accepts_fds(&self) -> bool {
        true
    }
    fn descriptor(&self) -> &str {
        original::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID as u32 {
            return super::reply(Err(Exception::security(
                "domain verifier backend requires system UID",
            )));
        }
        let mut reply = Parcel::new();
        let result = match call.code {
            original::SET_SHELL_USER_SELECTION => {
                let a = original::SetShellUserSelection::read(&mut call.data)?;
                super::domain_shell::write_user(
                    &self.0.host,
                    a.package_name.as_deref(),
                    a.user,
                    a.enabled,
                    a.domains,
                    true,
                )
                .map(|()| original::write_set_shell_user_selection_reply(&mut reply))
            }
            original::SET_SHELL_LINK_HANDLING => {
                let a = original::SetShellLinkHandling::read(&mut call.data)?;
                super::domain_shell::write_user(
                    &self.0.host,
                    a.package_name.as_deref(),
                    a.user,
                    a.allowed,
                    None,
                    false,
                )
                .map(|()| original::write_set_shell_link_handling_reply(&mut reply))
            }
            original::READ_SHELL_STATE => {
                let a = original::ReadShellState::read(&mut call.data)?;
                super::domain_shell::read(&self.0.host, a).map(|bytes| {
                    original::write_read_shell_state_reply(&mut reply, &Some(bytes));
                })
            }
            original::QUERY_VALID_VERIFICATION_PACKAGE_NAMES => {
                original::QueryValidVerificationPackageNames::read(&mut call.data)?;
                self.0.valid_verification_package_names().map(|names| {
                    dv::write_query_valid_verification_package_names_reply(
                        &mut reply,
                        &Some(names.into_iter().map(Some).collect()),
                    )
                })
            }
            original::GET_DOMAIN_VERIFICATION_INFO => {
                let a = original::GetDomainVerificationInfo::read(&mut call.data)?;
                self.0.info(a.uid, a.package_name.as_deref()).map(|info| {
                    dv::write_get_domain_verification_info_reply(&mut reply, info.as_ref())
                })
            }
            original::SET_DOMAIN_VERIFICATION_STATUS => {
                let a = original::SetDomainVerificationStatus::<DomainSet>::read(&mut call.data)?;
                (|| {
                    let capture = self.0.host.package_capture()?;
                    let status = self.set_status(call.sender_pid as i32, a, capture)?;
                    dv::write_set_domain_verification_status_reply(&mut reply, status);
                    Ok(())
                })()
            }
            original::GET_DOMAIN_VERIFICATION_PACKAGE_ID => {
                let a = original::GetDomainVerificationPackageId::read(&mut call.data)?;
                (|| {
                    let capture = self.0.host.package_capture()?;
                    let domains = DomainVerification::domains(&capture)?;
                    let id = a
                        .package_name
                        .as_deref()
                        .and_then(|name| domains.owner().package(name))
                        .map(|package| package.id.clone());
                    original::write_get_domain_verification_package_id_reply(&mut reply, &id);
                    Ok(())
                })()
            }
            original::SET_URI_RELATIVE_FILTER_GROUP => {
                let a = original::SetUriRelativeFilterGroup::<Group>::read(&mut call.data)?;
                self.0
                    .set_uri_group(
                        a.package_name.as_deref(),
                        a.id.as_deref(),
                        a.domain.as_deref(),
                        a.groups,
                    )
                    .map(|()| original::write_set_uri_relative_filter_group_reply(&mut reply))
            }
            original::GET_URI_RELATIVE_FILTER_GROUPS => {
                let a = original::GetUriRelativeFilterGroups::read(&mut call.data)?;
                (|| {
                    let capture = self.0.host.package_capture()?;
                    let groups = DomainVerification::domains(&capture)?
                        .owner()
                        .uri_groups_query(a.package_name.as_deref(), a.domains.as_deref())
                        .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
                    let groups = parcels::UriGroups::prepare(&groups)
                        .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e.to_string()))?;
                    original::write_get_uri_relative_filter_groups_reply(&mut reply, Some(&groups));
                    Ok(())
                })()
            }
            code => {
                let name = original::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, name)| *name);
                Err(self.0.unimplemented(name))
            }
        };
        super::reply(result.map(|()| reply))
    }
}

impl Service for DomainVerification {
    fn accepts_fds(&self) -> bool {
        true
    }
    fn descriptor(&self) -> &str {
        dv::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code == crate::shell::SHELL_COMMAND_TRANSACTION {
            return self.shell(call);
        }
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            dv::QUERY_VALID_VERIFICATION_PACKAGE_NAMES
            | dv::GET_DOMAIN_VERIFICATION_INFO
            | dv::SET_DOMAIN_VERIFICATION_STATUS
            | dv::SET_URI_RELATIVE_FILTER_GROUPS
            | dv::GET_URI_RELATIVE_FILTER_GROUPS => return self.forward(call, uid),
            dv::GET_DOMAIN_VERIFICATION_USER_STATE => {
                let a = dv::GetDomainVerificationUserState::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.user_state(uid, a.package_name.as_deref(), a.user_id)
                    .map(|state| {
                        dv::write_get_domain_verification_user_state_reply(&mut p, state.as_ref())
                    })
            }
            code => {
                let name = dv::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self.unimplemented(name))
            }
        };
        Ok(result.map(|()| p).unwrap_or_else(|e| {
            let mut reply = Parcel::new();
            // `ServiceSpecificException(code)` has no message.
            let message = (e.code != EX_SERVICE_SPECIFIC).then_some(e.message.as_str());
            reply.write_exception_message(&e, message);
            reply
        }))
    }
}

#[cfg(test)]
#[path = "domain_verification_tests.rs"]
mod tests;
