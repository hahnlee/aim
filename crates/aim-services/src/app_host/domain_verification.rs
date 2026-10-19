//! `domain_verification`: DomainVerificationService for the app host
//! (`android-16.0.0_r1`): the persisted domain verification state with
//! each package attached as `addPackage` attaches it at boot (its domains
//! collected from its code, `RESTRICT_DOMAINS` as platform compat decides
//! it for the package), when first asked; the packages with auto-verify
//! domains (`queryValidVerificationPackageNames`) to the verification
//! agent `DomainVerificationProxy.makeProxy` makes of PackageManagerService's
//! verifiers, and a package's verification info (`getDomainVerificationInfo`)
//! to it, `DUMP` holders and the system; a package's state for a user
//! (`getDomainVerificationUserState`) to a caller it is visible to. The
//! owners of a domain, the writes, persisting what attaching changed and the requests
//! to the verifier are not implemented (#1763).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, OnceLock};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, EX_SERVICE_SPECIFIC, Exception, Parcel};
use aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager as dv;

use super::{AppHost, PER_USER_RANGE};
use crate::package::domain_verification::owner::{Input, Owner};
use crate::package::domain_verification::{agent, parcels};
use crate::package::apps_filter::should_filter_application;
use crate::package::query::Query;

const SYSTEM_UID: i32 = 1000;
const DOMAIN_VERIFICATION_AGENT: &str = "android.permission.DOMAIN_VERIFICATION_AGENT";
const INTENT_FILTER_VERIFICATION_AGENT: &str = "android.permission.INTENT_FILTER_VERIFICATION_AGENT";
const QUERY_ALL_PACKAGES: &str = "android.permission.QUERY_ALL_PACKAGES";
const DUMP: &str = "android.permission.DUMP";
const INTERACT_ACROSS_USERS: &str = "android.permission.INTERACT_ACROSS_USERS";
/// `DomainVerificationCollector.RESTRICT_DOMAINS`.
const RESTRICT_DOMAINS: i64 = 175_408_749;
/// `DomainVerificationManager.INTERNAL_ERROR_NAME_NOT_FOUND`.
const ERROR_NAME_NOT_FOUND: i32 = 1;
/// `PackageManager.VERSION_CODE_HIGHEST`.
const VERSION_CODE_HIGHEST: i64 = -1;

/// The `NameNotFoundException` `DomainVerificationManagerStub` rethrows.
fn name_not_found() -> Exception {
    Exception { code: EX_SERVICE_SPECIFIC, message: String::new(), service_specific: ERROR_NAME_NOT_FOUND, parcelable: None }
}

pub struct DomainVerification {
    host: Arc<AppHost>,
    /// `mAttachedPkgStates` and the rest of the settings, once attached.
    owner: OnceLock<Result<Owner, String>>,
    /// The packages of the proxy's verifiers.
    verifiers: OnceLock<Vec<String>>,
}

impl DomainVerification {
    pub fn new(host: Arc<AppHost>) -> Self {
        DomainVerification { host, owner: OnceLock::new(), verifiers: OnceLock::new() }
    }

    fn unimplemented(&self, what: &str) -> Exception {
        self.host.unimplemented_named("domain_verification", &format!("{what} (#1763)"))
    }

    /// `isChangeEnabled(RESTRICT_DOMAINS)` of a package's mock application
    /// info: its name and target SDK.
    fn restrict_domains(&self, package: &str, target_sdk: i32) -> Result<bool, Exception> {
        Ok(self.host.inputs()?.compat.is_enabled(RESTRICT_DOMAINS, Some(package), target_sdk))
    }

    /// The persisted state with each package of code attached in the
    /// scan's order (`addPackage`).
    fn owner(&self) -> Result<&Owner, Exception> {
        let inputs = self.host.inputs()?;
        let attach = || -> Result<Owner, String> {
            let domains = inputs.packages.domains();
            let state = inputs.packages.state();
            let mut owner = Owner::new(domains.state.clone(), domains.legacy_info.clone());
            for package in &domains.packages {
                let Some(code) = state.packages.get(&package.name).and_then(|ps| ps.pkg.as_deref()) else { continue };
                let id = package.id.as_deref().ok_or_else(|| format!("{}: no domain set id", package.name))?;
                let signatures = package.signatures.as_deref().ok_or_else(|| format!("{}: no signers", package.name))?;
                let restrict_domains = inputs.compat.is_enabled(RESTRICT_DOMAINS, Some(&code.package_name), code.target_sdk_version);
                let input = Input { id, name: &package.name, code: Some(code), signatures, system: package.system, restrict_domains, pre_verified: None };
                owner.add(input, &domains.config)?;
            }
            Ok(owner)
        };
        self.owner
            .get_or_init(attach)
            .as_ref()
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("domain_verification: {e}")))
    }

    /// `getPackageUid(package, 0, user)`, or -1.
    fn package_uid(&self, package: &str, user: i32) -> Result<i32, Exception> {
        let (state, resolution) = self.host.packages()?;
        let query = Query { state: &state, filter: &resolution.apps_filter, calling_uid: SYSTEM_UID };
        query.package_uid(package, 0, user).map_err(|e| self.unimplemented(e.0))?
    }

    /// The proxy's `isCallerVerifier`: `uid` is a verifier package's
    /// (`isCallerPackage`).
    fn is_caller_verifier(&self, uid: i32) -> Result<bool, Exception> {
        let verifiers = match self.verifiers.get() {
            Some(verifiers) => verifiers,
            None => {
                let (state, resolution) = self.host.packages()?;
                let (legacy, modern) = agent::verifiers(&state, &resolution)?;
                self.verifiers.get_or_init(|| agent::proxy_packages(legacy, modern))
            }
        };
        for package in verifiers {
            if self.package_uid(package, uid / PER_USER_RANGE as i32)? == uid {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `Context.enforcePermission` of `permission` for `uid`.
    fn enforce(&self, permission: &str, uid: i32, message: &str) -> Result<(), Exception> {
        if self.host.check_permission(Some(permission), uid)? != 0 {
            return Err(Exception::security(format!("{message}: uid {uid} does not have {permission}.")));
        }
        Ok(())
    }

    /// `DomainVerificationEnforcer.assertApprovedQuerent`.
    fn assert_approved_querent(&self, uid: i32) -> Result<(), Exception> {
        if matches!(uid, 0 | 1000 | 2000) {
            return Ok(());
        }
        if !self.is_caller_verifier(uid)? {
            return self.enforce(DUMP, uid, &format!("Caller {uid} is not allowed to query domain verification state"));
        }
        self.enforce(QUERY_ALL_PACKAGES, uid, &format!("Caller {uid} does not hold {QUERY_ALL_PACKAGES}"))
    }

    /// `DomainVerificationEnforcer.assertApprovedVerifier`.
    fn assert_approved_verifier(&self, uid: i32) -> Result<(), Exception> {
        if matches!(uid, 0 | 1000 | 2000) {
            return Ok(());
        }
        let held = |permission| Ok::<_, Exception>(self.host.check_permission(Some(permission), uid)? == 0);
        let mut legacy = false;
        if !held(DOMAIN_VERIFICATION_AGENT)? {
            legacy = held(INTENT_FILTER_VERIFICATION_AGENT)?;
            if !legacy {
                return Err(Exception::security(format!("Caller {uid} does not hold {DOMAIN_VERIFICATION_AGENT}")));
            }
        }
        if !legacy {
            self.enforce(QUERY_ALL_PACKAGES, uid, &format!("Caller {uid} does not hold {QUERY_ALL_PACKAGES}"))?;
        }
        if !self.is_caller_verifier(uid)? {
            return Err(Exception::security(format!("Caller {uid} is not the approved domain verification agent")));
        }
        Ok(())
    }

    /// `DomainVerificationEnforcer.assertApprovedUserStateQuerent`:
    /// whether `package` is visible to the caller (`filterAppAccess`).
    fn assert_approved_user_state_querent(&self, uid: i32, package: &str, user: i32) -> Result<bool, Exception> {
        let caller_user = uid / PER_USER_RANGE as i32;
        if caller_user != user {
            self.enforce(INTERACT_ACROSS_USERS, uid, "Caller is not allowed to edit other users")?;
        }
        let (state, resolution) = self.host.packages()?;
        for id in [caller_user, user] {
            if !state.users.contains_key(&id) {
                return Err(Exception::security(format!("User {id} does not exist")));
            }
        }
        let filtered = should_filter_application(&state, &resolution.apps_filter, state.packages.get(package), uid, user, true, true)
            .map_err(|e| self.unimplemented(e.0))?;
        Ok(!filtered)
    }

    /// `getDomainVerificationUserState`: the package's web domains, each
    /// verified, selected by the user or neither, and whether the user
    /// allows its link handling; none for a package not attached.
    fn user_state(&self, uid: i32, name: Option<&str>, user: i32) -> Result<Option<parcels::UserState>, Exception> {
        let name = name.ok_or_else(name_not_found)?;
        if !self.assert_approved_user_state_querent(uid, name, user)? {
            return Err(name_not_found());
        }
        let (state, _) = self.host.packages()?;
        let code = state.packages.get(name).and_then(|ps| ps.pkg.as_deref()).ok_or_else(name_not_found)?;
        let restrict = self.restrict_domains(&code.package_name, code.target_sdk_version)?;
        let config = &self.host.inputs()?.packages.domains().config;
        let Some((id, allowed, states)) = self.owner()?.user_state(name, code, restrict, config, user).map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))? else {
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
        self.assert_approved_verifier(uid)?;
        Ok(self.owner()?.valid_verification_package_names())
    }

    /// `getDomainVerificationInfo`: the package's states with its valid
    /// auto-verify domains, none for a package not attached or without
    /// them.
    fn info(&self, uid: i32, name: Option<&str>) -> Result<Option<parcels::Info>, Exception> {
        self.assert_approved_querent(uid)?;
        let name = name.ok_or_else(name_not_found)?;
        let (state, resolution) = self.host.packages()?;
        let query = Query { state: &state, filter: &resolution.apps_filter, calling_uid: uid };
        let resolved = query.resolve_internal_package_name(name, VERSION_CODE_HIGHEST);
        let code = state.packages.get(&resolved).and_then(|ps| ps.pkg.as_deref()).ok_or_else(name_not_found)?;
        let restrict = self.restrict_domains(&code.package_name, code.target_sdk_version)?;
        let config = &self.host.inputs()?.packages.domains().config;
        let Some((id, states)) = self.owner()?.verification(name, code, restrict, config).map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))? else {
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
                dv::QueryValidVerificationPackageNames::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.valid_verification_package_names(uid)
                    .map(|names| dv::write_query_valid_verification_package_names_reply(&mut p, &Some(names.into_iter().map(Some).collect())))
            }
            dv::GET_DOMAIN_VERIFICATION_INFO => {
                let a = dv::GetDomainVerificationInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.info(uid, a.package_name.as_deref()).map(|info| dv::write_get_domain_verification_info_reply(&mut p, info.as_ref()))
            }
            dv::GET_DOMAIN_VERIFICATION_USER_STATE => {
                let a = dv::GetDomainVerificationUserState::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.user_state(uid, a.package_name.as_deref(), a.user_id)
                    .map(|state| dv::write_get_domain_verification_user_state_reply(&mut p, state.as_ref()))
            }
            code => {
                let name = dv::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
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
