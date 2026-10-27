//! Domain verification queries over the session's retained NativeDomains and
//! package code. Writes and verifier requests remain unimplemented (#1763).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_STATE, EX_SERVICE_SPECIFIC, Exception, Parcel,
};
use aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager as dv;

use super::{AppHost, PER_USER_RANGE};
use crate::package::apps_filter::should_filter_application;
use crate::package::domain_verification::{agent, parcels};
use crate::package::query::Query;
use crate::package::scan_snapshot::query_state::{Capture, NativeDomains};

const SYSTEM_UID: i32 = 1000;
const DOMAIN_VERIFICATION_AGENT: &str = "android.permission.DOMAIN_VERIFICATION_AGENT";
const INTENT_FILTER_VERIFICATION_AGENT: &str =
    "android.permission.INTENT_FILTER_VERIFICATION_AGENT";
const QUERY_ALL_PACKAGES: &str = "android.permission.QUERY_ALL_PACKAGES";
const DUMP: &str = "android.permission.DUMP";
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
    host: Arc<AppHost>,
}

impl DomainVerification {
    pub fn new(host: Arc<AppHost>) -> Self {
        DomainVerification { host }
    }

    fn unimplemented(&self, what: &str) -> Exception {
        self.host
            .unimplemented_named("domain_verification", &format!("{what} (#1763)"))
    }

    fn domains(capture: &Capture) -> Result<&NativeDomains, Exception> {
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
    fn is_caller_verifier(&self, capture: &Capture, uid: i32) -> Result<bool, Exception> {
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

    /// `DomainVerificationEnforcer.assertApprovedQuerent`.
    fn assert_approved_querent(&self, capture: &Capture, uid: i32) -> Result<(), Exception> {
        if matches!(uid, 0 | 1000 | 2000) {
            return Ok(());
        }
        if !self.is_caller_verifier(capture, uid)? {
            return self.enforce(
                DUMP,
                uid,
                &format!("Caller {uid} is not allowed to query domain verification state"),
            );
        }
        self.enforce(
            QUERY_ALL_PACKAGES,
            uid,
            &format!("Caller {uid} does not hold {QUERY_ALL_PACKAGES}"),
        )
    }

    /// `DomainVerificationEnforcer.assertApprovedVerifier`.
    fn assert_approved_verifier(&self, capture: &Capture, uid: i32) -> Result<(), Exception> {
        if matches!(uid, 0 | 1000 | 2000) {
            return Ok(());
        }
        let held = |permission| {
            Ok::<_, Exception>(self.host.check_permission(Some(permission), uid)? == 0)
        };
        let mut legacy = false;
        if !held(DOMAIN_VERIFICATION_AGENT)? {
            legacy = held(INTENT_FILTER_VERIFICATION_AGENT)?;
            if !legacy {
                return Err(Exception::security(format!(
                    "Caller {uid} does not hold {DOMAIN_VERIFICATION_AGENT}"
                )));
            }
        }
        if !legacy {
            self.enforce(
                QUERY_ALL_PACKAGES,
                uid,
                &format!("Caller {uid} does not hold {QUERY_ALL_PACKAGES}"),
            )?;
        }
        if !self.is_caller_verifier(capture, uid)? {
            return Err(Exception::security(format!(
                "Caller {uid} is not the approved domain verification agent"
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
    fn valid_verification_package_names(&self, uid: i32) -> Result<Vec<String>, Exception> {
        let capture = self.host.package_capture()?;
        self.assert_approved_verifier(&capture, uid)?;
        Ok(Self::domains(&capture)?
            .owner()
            .valid_verification_package_names())
    }

    /// `getDomainVerificationInfo`: the package's states with its valid
    /// auto-verify domains, none for a package not attached or without
    /// them.
    fn info(&self, uid: i32, name: Option<&str>) -> Result<Option<parcels::Info>, Exception> {
        let capture = self.host.package_capture()?;
        self.assert_approved_querent(&capture, uid)?;
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
}

impl Service for DomainVerification {
    fn descriptor(&self) -> &str {
        dv::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            dv::QUERY_VALID_VERIFICATION_PACKAGE_NAMES => {
                dv::QueryValidVerificationPackageNames::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.valid_verification_package_names(uid).map(|names| {
                    dv::write_query_valid_verification_package_names_reply(
                        &mut p,
                        &Some(names.into_iter().map(Some).collect()),
                    )
                })
            }
            dv::GET_DOMAIN_VERIFICATION_INFO => {
                let a =
                    dv::GetDomainVerificationInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.info(uid, a.package_name.as_deref())
                    .map(|info| dv::write_get_domain_verification_info_reply(&mut p, info.as_ref()))
            }
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
