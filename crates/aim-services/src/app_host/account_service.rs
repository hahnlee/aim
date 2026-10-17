//! `account`: AccountManagerService for the app host
//! (`AccountManagerService` at `android-16.0.0_r1`, ADR 0013): the
//! authenticators (`app_host/account_authenticators.rs`) and each user's
//! accounts as its databases hold them (`app_host/accounts_db.rs`), opened
//! as `getUserAccounts` opens them (grants of uninstalled apps purged,
//! the accounts validated against the authenticators, whose uids `meta`
//! keeps), and what apps read of them through the original's rules: the
//! types a caller sees and those it manages (signed as the
//! authenticator, `CertCapabilities.AUTH`), each account's visibility to
//! a package (stored, through the legacy keys, GET_ACCOUNTS before O,
//! READ_CONTACTS for a contacts authenticator, GET_ACCOUNTS_PRIVILEGED),
//! passwords, user data, auth tokens and previous names for the
//! authenticator's apps, and the listeners apps register by type. The
//! one user is a full user, never restricted, so shared accounts are not
//! filtered. Adding, removing and changing accounts, the authenticator
//! sessions (`getAuthToken`, `addAccount` and the rest), the broadcasts
//! of a change, access grants and devices with a device or profile owner
//! are not implemented (#1504).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception, Parcel, Reader,
    Result as ParcelResult,
};
use aim_service_aidl::android_accounts_iaccountmanager as am;
use aim_service_aidl::{ParcelValue, ReadParcelable, WriteParcelable};

use super::account_authenticators::{self, Authenticator, AuthenticatorDescription};
use super::accounts_db::{AccountsDb, DbError};
use super::{AppHost, PER_USER_RANGE, reply};
use crate::package::apps_filter::{self, NotModelled, Setting};
use crate::package::model::State;
use crate::package::query::Query;
use crate::package::settings::Signatures;

const SYSTEM_UID: i32 = 1000;
/// `AccountManager.VISIBILITY_*`.
const VISIBILITY_UNDEFINED: i32 = 0;
const VISIBILITY_VISIBLE: i32 = 1;
const VISIBILITY_USER_MANAGED_VISIBLE: i32 = 2;
const VISIBILITY_NOT_VISIBLE: i32 = 3;
const VISIBILITY_USER_MANAGED_NOT_VISIBLE: i32 = 4;
/// `AccountManager.PACKAGE_NAME_KEY_LEGACY_VISIBLE`, `_NOT_VISIBLE`.
const LEGACY_VISIBLE: &str = "android:accounts:key_legacy_visible";
const LEGACY_NOT_VISIBLE: &str = "android:accounts:key_legacy_not_visible";
/// `SIGNATURE_CHECK_*`.
const SIGNATURE_CHECK_MISMATCH: i32 = 0;
const SIGNATURE_CHECK_MATCH: i32 = 1;
const SIGNATURE_CHECK_UID_MATCH: i32 = 2;
/// `SigningDetails.CertCapabilities.AUTH`, `PAST_CERT_EXISTS`.
const CERT_AUTH: i32 = 16;
const PAST_CERT_EXISTS: i32 = 0;
/// `ENFORCE_PACKAGE_VISIBILITY_FILTERING`.
const ENFORCE_PACKAGE_VISIBILITY_FILTERING: i64 = 154_726_397;
/// `Build.VERSION_CODES.O`.
const O: i32 = 26;
const GET_ACCOUNTS: &str = "android.permission.GET_ACCOUNTS";
const GET_ACCOUNTS_PRIVILEGED: &str = "android.permission.GET_ACCOUNTS_PRIVILEGED";
const READ_CONTACTS: &str = "android.permission.READ_CONTACTS";
const WRITE_CONTACTS: &str = "android.permission.WRITE_CONTACTS";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";

/// `Account`: equal by name and type. The access id the service gives
/// each account it holds lets an app's process report its use
/// (`onAccountAccessed`).
#[derive(Clone, Debug)]
pub struct Account {
    pub name: String,
    pub kind: String,
    pub access_id: Option<String>,
}

impl Account {
    pub fn new(name: String, kind: String) -> Account {
        Account { name, kind, access_id: None }
    }
}

impl PartialEq for Account {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.kind == other.kind
    }
}

impl Eq for Account {}

impl Hash for Account {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.kind.hash(state);
    }
}

impl fmt::Display for Account {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Account {{name={}, type={}}}", self.name, self.kind)
    }
}

impl ReadParcelable for Account {
    /// `Account(Parcel)`, which refuses an empty name or type.
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let name = r.read_string16()?.filter(|n| !n.is_empty()).ok_or(BAD_VALUE)?;
        let kind = r.read_string16()?.filter(|t| !t.is_empty()).ok_or(BAD_VALUE)?;
        Ok(Account { name, kind, access_id: r.read_string16()? })
    }
}

impl WriteParcelable for Account {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some(&self.name));
        p.write_string16(Some(&self.kind));
        p.write_string16(self.access_id.as_deref());
    }
}

/// `UserHandle`: its user id.
pub struct UserHandle(i32);

impl ReadParcelable for UserHandle {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(UserHandle(r.read_i32()?))
    }
}

/// An account as a key of a parcelled `Map` (`writeValue` of a
/// parcelable: its class, then its fields).
fn account_value(account: &Account) -> ParcelValue {
    const VAL_PARCELABLE: i32 = 4;
    let mut p = Parcel::new();
    p.write_string16(Some("android.accounts.Account"));
    account.write_to(&mut p);
    ParcelValue::Custom { kind: VAL_PARCELABLE, bytes: p.data().to_vec() }
}

/// `UUID.randomUUID().toString()`.
fn random_uuid() -> String {
    let mut b = [0u8; 16];
    // SAFETY: fills the buffer it is given.
    unsafe { libc::arc4random_buf(b.as_mut_ptr().cast(), b.len()) };
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// What a call fails with: an exception, or the databases' failure, which
/// the calls that catch `SQLiteException` answer with no accounts.
enum Failure {
    Thrown(Exception),
    Db(DbError),
}

impl From<DbError> for Failure {
    fn from(e: DbError) -> Self {
        Failure::Db(e)
    }
}

/// A failure of the databases where it is thrown.
fn db_failed(e: DbError) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, e.0)
}

impl Failure {
    fn exception(self) -> Exception {
        match self {
            Failure::Thrown(e) => e,
            Failure::Db(e) => db_failed(e),
        }
    }
}

fn not_modelled(e: NotModelled) -> Exception {
    Exception::new(EX_UNSUPPORTED_OPERATION, format!("account: {}", e.0))
}

fn null(what: &str) -> Exception {
    Exception::new(EX_NULL_POINTER, format!("{what} cannot be null"))
}

fn is_same_app(uid: i32, other: i32) -> bool {
    apps_filter::app_id(uid) == apps_filter::app_id(other)
}

fn user_of(uid: i32) -> i32 {
    uid / PER_USER_RANGE as i32
}

/// `ComputerEngine.getSigningDetails`: the signatures of `uid`'s package
/// or shared user; none (`SigningDetails.UNKNOWN`) without one.
fn signing(state: &State, uid: i32) -> Option<&Signatures> {
    match apps_filter::setting(state, apps_filter::app_id(uid))? {
        Setting::Package(ps) => ps.signatures.as_ref(),
        Setting::Shared(su) => su.signatures.as_ref(),
    }
    .filter(|s| !s.signatures.is_empty())
}

/// `SigningDetails.hasCertificateInternal`: `cert` is the current signer,
/// or a past one granted `flags`.
fn has_certificate(details: &Signatures, cert: &[u8], flags: i32) -> bool {
    if let Some(past) = details.past_signatures.as_deref().filter(|p| !p.is_empty()) {
        for (past_cert, past_flags) in &past[..past.len() - 1] {
            if past_cert == cert && (flags == PAST_CERT_EXISTS || flags & past_flags == flags) {
                return true;
            }
        }
    }
    details.signatures.len() == 1 && details.signatures[0] == cert
}

/// `Signature.areExactMatch`.
fn signatures_match_exactly(a: &Signatures, b: &Signatures) -> bool {
    a.signatures.len() == b.signatures.len()
        && a.signatures.iter().all(|s| b.signatures.contains(s))
        && b.signatures.iter().all(|s| a.signatures.contains(s))
}

/// `PackageManagerInternal.hasSignatureCapability(server, client, AUTH)`:
/// `checkCapability` or `hasAncestorOrSelf`.
fn has_auth_capability(state: &State, server_uid: i32, client_uid: i32) -> bool {
    let (Some(server), Some(client)) = (signing(state, server_uid), signing(state, client_uid)) else {
        return false;
    };
    let check_capability = if client.signatures.len() > 1 {
        signatures_match_exactly(server, client)
    } else {
        has_certificate(server, &client.signatures[0], CERT_AUTH)
    };
    let ancestor_or_self = if server.signatures.len() > 1 {
        signatures_match_exactly(client, server)
    } else {
        has_certificate(client, &server.signatures[0], PAST_CERT_EXISTS)
    };
    check_capability || ancestor_or_self
}

/// What `resolveAccountVisibility` asks of a user's packages.
trait VisibilityInputs {
    /// `getPackageUidAsUser`: none for a package not installed.
    fn package_uid(&self, package: &str) -> Result<Option<i32>, Exception>;
    /// `checkPackageSignature`.
    fn package_signature(&self, kind: &str, uid: i32) -> Result<i32, Exception>;
    /// `isPermittedForPackage`.
    fn permitted(&self, package: &str, permissions: &[&str]) -> Result<bool, Exception>;
    /// `isProfileOwner`.
    fn profile_owner(&self, uid: i32) -> Result<bool, Exception>;
    /// `isPreOApplication`.
    fn pre_o(&self, package: &str) -> Result<bool, Exception>;
    /// `accountTypeManagesContacts`.
    fn manages_contacts(&self, kind: &str) -> Result<bool, Exception>;
}

/// `resolveAccountVisibility`: `account`'s visibility to `package`, never
/// `VISIBILITY_UNDEFINED`, `stored` giving the values stored by package
/// (`getAccountVisibilityFromCache`).
fn resolve_visibility(
    inputs: &impl VisibilityInputs,
    stored: &mut dyn FnMut(&str) -> i32,
    account: &Account,
    package: &str,
) -> Result<i32, Exception> {
    let Some(uid) = inputs.package_uid(package)? else {
        return Ok(VISIBILITY_NOT_VISIBLE);
    };
    // The system's visibility cannot be restricted, nor the
    // authenticator's own.
    if is_same_app(uid, SYSTEM_UID) {
        return Ok(VISIBILITY_VISIBLE);
    }
    let signature = inputs.package_signature(&account.kind, uid)?;
    if signature == SIGNATURE_CHECK_UID_MATCH {
        return Ok(VISIBILITY_VISIBLE);
    }
    let visibility = stored(package);
    if visibility != VISIBILITY_UNDEFINED {
        return Ok(visibility);
    }
    let privileged = inputs.permitted(package, &[GET_ACCOUNTS_PRIVILEGED])?;
    if inputs.profile_owner(uid)? {
        return Ok(VISIBILITY_VISIBLE);
    }
    let pre_o = inputs.pre_o(package)?;
    let legacy_visible = signature != SIGNATURE_CHECK_MISMATCH
        || pre_o && inputs.permitted(package, &[GET_ACCOUNTS, GET_ACCOUNTS_PRIVILEGED])?
        || inputs.permitted(package, &[READ_CONTACTS])? && inputs.manages_contacts(&account.kind)?
        || privileged;
    let (key, default) = if legacy_visible {
        (LEGACY_VISIBLE, VISIBILITY_USER_MANAGED_VISIBLE)
    } else {
        (LEGACY_NOT_VISIBLE, VISIBILITY_USER_MANAGED_NOT_VISIBLE)
    };
    Ok(match stored(key) {
        VISIBILITY_UNDEFINED => default,
        visibility => visibility,
    })
}

/// A user's packages, as the account manager asks them.
struct UserPackages<'a> {
    manager: &'a AccountManager,
    user: i32,
}

impl VisibilityInputs for UserPackages<'_> {
    fn package_uid(&self, package: &str) -> Result<Option<i32>, Exception> {
        self.manager.package_uid(package, self.user)
    }

    fn package_signature(&self, kind: &str, uid: i32) -> Result<i32, Exception> {
        self.manager.check_package_signature(kind, uid, self.user)
    }

    fn permitted(&self, package: &str, permissions: &[&str]) -> Result<bool, Exception> {
        self.manager.permitted_for_package(package, self.user, permissions)
    }

    fn profile_owner(&self, _uid: i32) -> Result<bool, Exception> {
        self.manager.is_profile_owner()
    }

    fn pre_o(&self, package: &str) -> Result<bool, Exception> {
        Ok(self.manager.target_sdk(package)?.is_none_or(|target| target < O))
    }

    fn manages_contacts(&self, kind: &str) -> Result<bool, Exception> {
        self.manager.manages_contacts(kind, self.user)
    }
}

/// `UserAccounts`: a user's databases and the caches over them.
struct UserAccounts {
    state: Mutex<Accounts>,
    /// `mReceiversForType`: by account type (`None` for all), each
    /// package's count of registered listeners.
    receivers: Mutex<HashMap<Option<String>, HashMap<String, i32>>>,
}

struct Accounts {
    db: AccountsDb,
    /// `accountCache`: the accounts by type, in the order they were read.
    accounts: Vec<(String, Vec<Account>)>,
    user_data: HashMap<Account, HashMap<String, Option<String>>>,
    auth_tokens: HashMap<Account, HashMap<String, Option<String>>>,
    visibility: HashMap<Account, HashMap<String, i32>>,
    previous_names: HashMap<Account, Option<String>>,
}

impl Accounts {
    /// `accountExistsCache`.
    fn exists(&self, account: &Account) -> bool {
        self.accounts.iter().any(|(_, list)| list.contains(account))
    }

    /// The accounts of `kind`, or of every type.
    fn of(&self, kind: Option<&str>) -> Vec<Account> {
        self.accounts.iter().filter(|(t, _)| kind.is_none_or(|k| k == t)).flat_map(|(_, list)| list.iter().cloned()).collect()
    }

    /// `getAccountVisibilityFromCache`.
    fn stored_visibility(&mut self, account: &Account, package: &str) -> i32 {
        *self.visibility.entry(account.clone()).or_default().get(package).unwrap_or(&VISIBILITY_UNDEFINED)
    }
}

pub struct AccountManager {
    host: Arc<AppHost>,
    /// `mAuthenticatorCache`'s services, by user.
    authenticators: Mutex<HashMap<i32, Arc<Vec<Authenticator>>>>,
    users: Mutex<HashMap<i32, Arc<UserAccounts>>>,
}

impl AccountManager {
    pub fn new(host: Arc<AppHost>) -> AccountManager {
        AccountManager { host, authenticators: Mutex::default(), users: Mutex::default() }
    }

    /// `mAuthenticatorCache.getAllServices(user)`.
    fn authenticators(&self, user: i32) -> Result<Arc<Vec<Authenticator>>, Exception> {
        let mut cache = self.authenticators.lock().unwrap();
        if let Some(found) = cache.get(&user) {
            return Ok(found.clone());
        }
        let found = Arc::new(account_authenticators::generate(&self.host, user)?);
        cache.insert(user, found.clone());
        Ok(found)
    }

    /// `f` of the package model, asked as the system.
    fn system_query<T>(&self, f: impl FnOnce(&Query<'_>) -> Result<T, NotModelled>) -> Result<T, Exception> {
        let (state, resolution) = self.host.packages()?;
        f(&Query { state: &state, filter: &resolution.apps_filter, calling_uid: SYSTEM_UID }).map_err(not_modelled)
    }

    /// `getPackageUidAsUser`: none for a package not installed.
    fn package_uid(&self, name: &str, user: i32) -> Result<Option<i32>, Exception> {
        let uid = self.system_query(|q| q.package_uid(name, 0, user))??;
        Ok((uid >= 0).then_some(uid))
    }

    /// `getPackagesForUid`.
    fn packages_for_uid(&self, uid: i32) -> Result<Vec<String>, Exception> {
        Ok(self.system_query(|q| q.packages_for_uid(uid))?.into_iter().flatten().flatten().collect())
    }

    /// Whether a package of `uid` has the system's `flag`
    /// (`isSystemUid`, `isPrivileged`).
    fn uid_package_is(&self, uid: i32, flag: impl Fn(&crate::package::model::PackageState) -> bool) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(self.packages_for_uid(uid)?.iter().any(|name| state.packages.get(name).is_some_and(&flag)))
    }

    /// `ApplicationInfo.targetSdkVersion` of `name`, if installed.
    fn target_sdk(&self, name: &str) -> Result<Option<i32>, Exception> {
        Ok(self.host.inputs()?.packages.application_info(name, 0).map(|ai| ai.target_sdk_version))
    }

    /// `getPackageNameForUid`: the uid's package with the oldest target.
    fn package_name_for_uid(&self, uid: i32) -> Result<Option<String>, Exception> {
        let names = self.packages_for_uid(uid)?;
        if names.len() <= 1 {
            return Ok(names.into_iter().next());
        }
        let mut oldest = (i32::MAX, names[0].clone());
        for name in names {
            if let Some(version) = self.target_sdk(&name)?
                && version < oldest.0
            {
                oldest = (version, name);
            }
        }
        Ok(Some(oldest.1))
    }

    /// `canCallerAccessPackage`.
    fn can_access_package(&self, package: Option<&str>, uid: i32, user: i32) -> Result<bool, Exception> {
        let Some(package) = package else { return Ok(false) };
        if !self.host.change_enabled(ENFORCE_PACKAGE_VISIBILITY_FILTERING, uid)? {
            return Ok(self.system_query(|q| q.package_uid_internal(package, 0, user, SYSTEM_UID))? != -1);
        }
        let (state, resolution) = self.host.packages()?;
        let filtered = apps_filter::should_filter_application(
            &state,
            &resolution.apps_filter,
            state.packages.get(package),
            uid,
            user,
            true,
            true,
        )
        .map_err(not_modelled)?;
        Ok(!filtered)
    }

    /// `isPermittedForPackage`: the package holds one of `permissions`,
    /// and its app op allows it.
    fn permitted_for_package(&self, package: &str, user: i32, permissions: &[&str]) -> Result<bool, Exception> {
        let Some(uid) = self.package_uid(package, user)? else { return Ok(false) };
        for permission in permissions {
            if self.system_query(|q| q.check_package_permission(Some(package), Some(permission), user))? != 0 {
                continue;
            }
            let op = match *permission {
                GET_ACCOUNTS => Some(super::appops_service::OP_GET_ACCOUNTS),
                READ_CONTACTS => Some(super::appops_service::OP_READ_CONTACTS),
                WRITE_CONTACTS => Some(super::appops_service::OP_WRITE_CONTACTS),
                _ => None,
            };
            match op {
                None => return Ok(true),
                Some(op) if super::appops_service::mode(&self.host, op, uid, Some(package))? == 0 => return Ok(true),
                Some(_) => {}
            }
        }
        Ok(false)
    }

    /// `isProfileOwner`: the app host runs no device policy, so a device
    /// with a device or profile owner is not served (#1504); else none is.
    fn is_profile_owner(&self) -> Result<bool, Exception> {
        if self.host.device_or_profile_owner()? {
            return Err(self.host.unimplemented_named("account", "a device or profile owner (#1504)"));
        }
        Ok(false)
    }

    /// `getTypesForCaller`: the authenticators' types, all of them or
    /// those `uid` is signed for.
    fn types_for_caller(&self, uid: i32, user: i32, otherwise_permitted: bool) -> Result<Vec<String>, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(self
            .authenticators(user)?
            .iter()
            .filter(|a| otherwise_permitted || has_auth_capability(&state, a.uid, uid))
            .map(|a| a.description.kind.clone())
            .collect())
    }

    /// `isAccountManagedByCaller`.
    fn managed_by_caller(&self, kind: Option<&str>, uid: i32, user: i32) -> Result<bool, Exception> {
        Ok(match kind {
            Some(kind) => self.types_for_caller(uid, user, false)?.iter().any(|t| t == kind),
            None => false,
        })
    }

    /// `checkPackageSignature`.
    fn check_package_signature(&self, kind: &str, uid: i32, user: i32) -> Result<i32, Exception> {
        let (state, _) = self.host.packages()?;
        for a in self.authenticators(user)?.iter().filter(|a| a.description.kind == kind) {
            if a.uid == uid {
                return Ok(SIGNATURE_CHECK_UID_MATCH);
            }
            if has_auth_capability(&state, a.uid, uid) {
                return Ok(SIGNATURE_CHECK_MATCH);
            }
        }
        Ok(SIGNATURE_CHECK_MISMATCH)
    }

    /// `accountTypeManagesContacts`: its authenticator may write contacts.
    fn manages_contacts(&self, kind: &str, user: i32) -> Result<bool, Exception> {
        match self.authenticators(user)?.iter().find(|a| a.description.kind == kind) {
            Some(a) => self.permitted_for_package(&a.description.package, user, &[WRITE_CONTACTS]),
            None => Ok(false),
        }
    }

    /// `isLocalUnlockedUser`.
    fn unlocked(&self, user: i32) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state.users.get(&user).is_some_and(|u| u.unlocking_or_unlocked))
    }

    /// `getUserAccounts`: `user`'s accounts, opened the first time.
    fn user_accounts(&self, user: i32) -> Result<Arc<UserAccounts>, Failure> {
        let mut users = self.users.lock().unwrap();
        if let Some(accounts) = users.get(&user) {
            return Ok(accounts.clone());
        }
        let data = &self.host.inputs().map_err(Failure::Thrown)?.data;
        let mut db = AccountsDb::open(&data.join(format!("system_de/{user}/accounts_de.db")))?;
        self.purge_old_grants(&db)?;
        if self.unlocked(user).map_err(Failure::Thrown)? {
            db.attach_ce(&data.join(format!("system_ce/{user}/accounts_ce.db")))?;
            // `syncDeCeAccountsLocked`.
            if !db.find_ce_accounts_not_in_de()?.is_empty() {
                return Err(Failure::Thrown(
                    self.host.unimplemented_named("account", "removing accounts deleted while the user was locked (#1504)"),
                ));
            }
        }
        let mut accounts = Accounts {
            db,
            accounts: Vec::new(),
            user_data: HashMap::new(),
            auth_tokens: HashMap::new(),
            visibility: HashMap::new(),
            previous_names: HashMap::new(),
        };
        self.validate(&mut accounts, user)?;
        let accounts = Arc::new(UserAccounts { state: Mutex::new(accounts), receivers: Mutex::default() });
        users.insert(user, accounts.clone());
        Ok(accounts)
    }

    /// `getUserAccounts` where the databases' failure is thrown.
    fn accounts(&self, user: i32) -> Result<Arc<UserAccounts>, Exception> {
        self.user_accounts(user).map_err(Failure::exception)
    }

    /// `purgeOldGrants`: grants to uids of no package are deleted.
    fn purge_old_grants(&self, db: &AccountsDb) -> Result<(), Failure> {
        let uids = match db.find_all_uid_grants() {
            Ok(uids) => uids,
            Err(e) => {
                eprintln!("app host: accounts: could not delete grants: {}", e.0);
                return Ok(());
            }
        };
        for uid in uids {
            if self.system_query(|q| q.packages_for_uid(uid)).map_err(Failure::Thrown)?.is_none() {
                db.delete_grants_by_uid(uid)?;
            }
        }
        Ok(())
    }

    /// `validateAccountsInternal`: the authenticators' uids in `meta`
    /// updated, and the accounts cached. Accounts whose authenticator is
    /// gone would be deleted, which is not implemented (#1504); the check
    /// comes before any write.
    fn validate(&self, accounts: &mut Accounts, user: i32) -> Result<(), Failure> {
        let mut known: Vec<(String, i32)> =
            self.authenticators(user).map_err(Failure::Thrown)?.iter().map(|a| (a.description.kind.clone(), a.uid)).collect();
        let meta = accounts.db.find_meta_auth_uid()?;
        let mut obsolete = Vec::new();
        let mut installed: Option<HashSet<i32>> = None;
        for (kind, uid) in meta {
            let known_uid = known.iter().find(|(t, _)| *t == kind).map(|(_, u)| *u);
            if known_uid == Some(uid) {
                known.retain(|(t, _)| *t != kind);
                continue;
            }
            // `getUidsOfInstalledOrUpdatedPackagesAsUser`.
            let installed = match &installed {
                Some(installed) => installed,
                None => {
                    let (state, _) = self.host.packages().map_err(Failure::Thrown)?;
                    installed.insert(
                        state
                            .packages
                            .values()
                            .filter(|ps| crate::package::info::user_state(ps, user).installed)
                            .map(|ps| user * PER_USER_RANGE as i32 + ps.app_id)
                            .collect(),
                    )
                }
            };
            if !installed.contains(&uid) || known_uid.is_some() {
                obsolete.push((kind, uid));
            }
        }
        let all = accounts.db.find_all_de_accounts()?;
        if all.iter().any(|(_, a)| obsolete.iter().any(|(kind, _)| *kind == a.kind)) {
            return Err(Failure::Thrown(
                self.host.unimplemented_named("account", "deleting the accounts of a removed authenticator (#1504)"),
            ));
        }
        for (kind, uid) in &obsolete {
            accounts.db.delete_meta_by_auth_type_and_uid(kind, *uid)?;
        }
        for (kind, uid) in &known {
            accounts.db.insert_or_replace_meta_auth_type_and_uid(kind, *uid)?;
        }
        accounts.accounts.clear();
        for (_, account) in all {
            let entry = match accounts.accounts.iter().position(|(t, _)| *t == account.kind) {
                Some(i) => &mut accounts.accounts[i].1,
                None => {
                    accounts.accounts.push((account.kind.clone(), Vec::new()));
                    &mut accounts.accounts.last_mut().unwrap().1
                }
            };
            entry.push(Account { access_id: Some(random_uuid()), ..account });
        }
        accounts.visibility.extend(accounts.db.find_all_visibility_values()?);
        Ok(())
    }

    /// `resolveAccountVisibility` of `account` to `package` in `user`.
    fn resolve_visibility(&self, accounts: &UserAccounts, account: &Account, package: &str, user: i32) -> Result<i32, Exception> {
        let mut stored = |key: &str| accounts.state.lock().unwrap().stored_visibility(account, key);
        resolve_visibility(&UserPackages { manager: self, user }, &mut stored, account, package)
    }

    /// `getAccountsFromCache` and `filterAccounts`: the accounts of `kind`
    /// (or of every type) visible to `package` (else to `uid`'s), those
    /// the user manages but hid too if `include_managed_not_visible`.
    fn accounts_from_cache(
        &self,
        accounts: &UserAccounts,
        user: i32,
        kind: Option<&str>,
        uid: i32,
        package: Option<&str>,
        include_managed_not_visible: bool,
    ) -> Result<Vec<Account>, Exception> {
        let unfiltered = accounts.state.lock().unwrap().of(kind);
        if unfiltered.is_empty() {
            return Ok(unfiltered);
        }
        let package = match package {
            Some(package) => package.to_string(),
            None => self.package_name_for_uid(uid)?.ok_or_else(|| null("packageName"))?,
        };
        let mut visible = Vec::new();
        for account in unfiltered {
            let visibility = self.resolve_visibility(accounts, &account, &package, user)?;
            if visibility == VISIBILITY_VISIBLE
                || visibility == VISIBILITY_USER_MANAGED_VISIBLE
                || include_managed_not_visible && visibility == VISIBILITY_USER_MANAGED_NOT_VISIBLE
            {
                visible.push(account);
            }
        }
        Ok(visible)
    }

    /// `getAccountsAsUserForPackage`.
    #[allow(clippy::too_many_arguments)]
    fn accounts_as_user_for_package(
        &self,
        calling_uid: i32,
        kind: Option<&str>,
        user: i32,
        calling_package: Option<&str>,
        package_uid: i32,
        op_package: Option<&str>,
        include_managed_not_visible: bool,
    ) -> Result<Vec<Account>, Exception> {
        if user != user_of(calling_uid)
            && calling_uid != SYSTEM_UID
            && self.host.check_permission(Some(INTERACT_ACROSS_USERS_FULL), calling_uid)? != 0
        {
            return Err(Exception::security(format!(
                "User {} trying to get account for {user}",
                user_of(calling_uid)
            )));
        }
        let managed = self.types_for_caller(calling_uid, user_of(calling_uid), false)?;
        let (mut uid, mut package) = (calling_uid, op_package);
        if package_uid != -1
            && (is_same_app(calling_uid, SYSTEM_UID) || kind.is_some_and(|k| managed.iter().any(|t| t == k)))
        {
            (uid, package) = (package_uid, calling_package);
        }
        let mut visible = self.types_for_caller(uid, user, true)?;
        if visible.is_empty() || kind.is_some_and(|k| !visible.iter().any(|t| t == k)) {
            return Ok(Vec::new());
        }
        if let Some(kind) = kind {
            visible = vec![kind.to_string()];
        }
        let found = (|| -> Result<Vec<Account>, Failure> {
            let accounts = self.user_accounts(user)?;
            let mut found = Vec::new();
            for kind in &visible {
                let of_kind = self.accounts_from_cache(&accounts, user, Some(kind), uid, package, include_managed_not_visible);
                found.extend(of_kind.map_err(Failure::Thrown)?);
            }
            Ok(found)
        })();
        match found {
            Err(Failure::Db(e)) => {
                eprintln!("app host: accounts: could not get accounts for user {user}: {}", e.0);
                Ok(Vec::new())
            }
            found => found.map_err(Failure::exception),
        }
    }

    /// `AppOpsManager.checkPackage`.
    fn check_package(&self, uid: i32, package: Option<&str>) -> Result<(), Exception> {
        let package = package.unwrap_or_default();
        if !self.host.owns_package(uid, package)? {
            return Err(Exception::security(format!("Package {package} does not belong to {uid}")));
        }
        Ok(())
    }

    /// `hasExplicitlyGrantedPermission` of any token type.
    fn granted_explicitly(&self, account: &Account, uid: i32) -> Result<bool, Exception> {
        if apps_filter::app_id(uid) == SYSTEM_UID {
            return Ok(true);
        }
        let accounts = self.accounts(user_of(uid))?;
        let count = accounts.state.lock().unwrap().db.find_matching_grants_count_any_token(uid, account).map_err(db_failed)?;
        if count > 0 {
            return Ok(true);
        }
        // `ActivityManager.isRunningInTestHarness`.
        let configured = self.host.configured.get().ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host not configured"))?;
        Ok((configured.properties)("ro.test_harness").is_some_and(|v| matches!(v.as_str(), "1" | "y" | "yes" | "on" | "true")))
    }

    /// `hasAccountAccess(account, package, uid)`.
    fn has_account_access(&self, account: &Account, package: Option<&str>, uid: i32) -> Result<bool, Exception> {
        let package = match package {
            Some(package) => package.to_string(),
            None => match self.package_name_for_uid(uid)? {
                Some(package) => package,
                None => return Ok(false),
            },
        };
        // `permissionIsGranted` of any token type.
        let user = user_of(uid);
        if apps_filter::app_id(uid) == SYSTEM_UID
            || self.uid_package_is(uid, |ps| ps.is.privileged)?
            || self.managed_by_caller(Some(&account.kind), uid, user)?
            || self.granted_explicitly(account, uid)?
        {
            return Ok(true);
        }
        let accounts = self.accounts(user)?;
        let visibility = self.resolve_visibility(&accounts, account, &package, user)?;
        Ok(visibility == VISIBILITY_VISIBLE || visibility == VISIBILITY_USER_MANAGED_VISIBLE)
    }

    /// The checks of `getPackagesAndVisibilityForAccount` and
    /// `getAccountVisibility`: the caller manages the type, or is the
    /// system's.
    fn may_read_visibility(&self, kind: &str, uid: i32) -> Result<bool, Exception> {
        Ok(self.managed_by_caller(Some(kind), uid, user_of(uid))? || self.uid_package_is(uid, |ps| ps.is.system)?)
    }
}

impl Service for AccountManager {
    fn descriptor(&self) -> &str {
        am::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let user = user_of(uid);
        match call.code {
            am::GET_PASSWORD => {
                let a = am::GetPassword::<Account>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let mut p = Parcel::new();
                    let account = a.account.ok_or_else(|| Exception::illegal_argument("account is null"))?;
                    if !self.managed_by_caller(Some(&account.kind), uid, user)? {
                        return Err(Exception::security(format!(
                            "uid {uid} cannot get secrets for accounts of type: {}",
                            account.kind
                        )));
                    }
                    let accounts = self.accounts(user)?;
                    // `readPasswordInternal`: none while the user is locked.
                    let password = match self.unlocked(user)? {
                        true => accounts
                            .state
                            .lock()
                            .unwrap()
                            .db
                            .find_account_password_by_name_and_type(&account.name, &account.kind)
                            .map_err(db_failed)?,
                        false => None,
                    };
                    am::write_get_password_reply(&mut p, &password);
                    Ok(p)
                })())
            }
            am::GET_USER_DATA => {
                let a = am::GetUserData::<Account>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let mut p = Parcel::new();
                    let account = a.account.ok_or_else(|| null("account"))?;
                    let key = a.key.ok_or_else(|| null("key"))?;
                    if !self.managed_by_caller(Some(&account.kind), uid, user)? {
                        return Err(Exception::security(format!(
                            "uid {uid} cannot get user data for accounts of type: {}",
                            account.kind
                        )));
                    }
                    let mut value = None;
                    if self.unlocked(user)? {
                        let accounts = self.accounts(user)?;
                        let mut state = accounts.state.lock().unwrap();
                        // `readUserDataInternal`.
                        if state.exists(&account) {
                            if !state.user_data.contains_key(&account) {
                                let extras = state.db.find_user_extras_for_account(&account).map_err(db_failed)?;
                                state.user_data.insert(account.clone(), extras);
                            }
                            value = state.user_data[&account].get(&key).cloned().flatten();
                        }
                    }
                    am::write_get_user_data_reply(&mut p, &value);
                    Ok(p)
                })())
            }
            am::GET_AUTHENTICATOR_TYPES => {
                let a = am::GetAuthenticatorTypes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let mut p = Parcel::new();
                    // `isCrossUser`.
                    if a.user_id != user
                        && uid != SYSTEM_UID
                        && self.host.check_permission(Some(INTERACT_ACROSS_USERS_FULL), uid)? != 0
                    {
                        return Err(Exception::security(format!(
                            "User {user} tying to get authenticator types for {}",
                            a.user_id
                        )));
                    }
                    let mut types: Vec<Option<AuthenticatorDescription>> = Vec::new();
                    for authenticator in self.authenticators(a.user_id)?.iter() {
                        if self.can_access_package(Some(&authenticator.description.package), uid, a.user_id)? {
                            types.push(Some(authenticator.description.clone()));
                        }
                    }
                    am::write_get_authenticator_types_reply(&mut p, Some(&types));
                    Ok(p)
                })())
            }
            am::GET_ACCOUNTS_FOR_PACKAGE => {
                let a = am::GetAccountsForPackage::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let mut p = Parcel::new();
                    if !is_same_app(uid, SYSTEM_UID) {
                        return Err(Exception::security(format!(
                            "getAccountsForPackage() called from unauthorized uid {uid} with uid={}",
                            a.uid
                        )));
                    }
                    let found = self.accounts_as_user_for_package(
                        uid,
                        None,
                        user,
                        a.package_name.as_deref(),
                        a.uid,
                        a.op_package_name.as_deref(),
                        true,
                    )?;
                    am::write_get_accounts_for_package_reply(&mut p, Some(&found.into_iter().map(Some).collect::<Vec<_>>()));
                    Ok(p)
                })())
            }
            am::GET_ACCOUNTS_BY_TYPE_FOR_PACKAGE => {
                let a = am::GetAccountsByTypeForPackage::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let mut p = Parcel::new();
                    self.check_package(uid, a.op_package_name.as_deref())?;
                    let kind = a.r#type.as_deref();
                    let package_uid = self.package_uid(a.package_name.as_deref().unwrap_or_default(), user)?;
                    let system = is_same_app(uid, SYSTEM_UID);
                    let found = match package_uid {
                        None => Vec::new(),
                        Some(_) if !system && kind.is_some() && !self.managed_by_caller(kind, uid, user)? => Vec::new(),
                        Some(package_uid) => self.accounts_as_user_for_package(
                            uid,
                            kind,
                            user,
                            a.package_name.as_deref(),
                            package_uid,
                            a.op_package_name.as_deref(),
                            system || kind.is_some(),
                        )?,
                    };
                    am::write_get_accounts_by_type_for_package_reply(
                        &mut p,
                        Some(&found.into_iter().map(Some).collect::<Vec<_>>()),
                    );
                    Ok(p)
                })())
            }
            am::GET_ACCOUNTS_AS_USER => {
                let a = am::GetAccountsAsUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let mut p = Parcel::new();
                    self.check_package(uid, a.op_package_name.as_deref())?;
                    let op = a.op_package_name.as_deref();
                    let found = self.accounts_as_user_for_package(uid, a.account_type.as_deref(), a.user_id, op, -1, op, false)?;
                    am::write_get_accounts_as_user_reply(&mut p, Some(&found.into_iter().map(Some).collect::<Vec<_>>()));
                    Ok(p)
                })())
            }
            am::PEEK_AUTH_TOKEN => {
                let a = am::PeekAuthToken::<Account>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let mut p = Parcel::new();
                    let account = a.account.ok_or_else(|| null("account"))?;
                    let kind = a.auth_token_type.ok_or_else(|| null("authTokenType"))?;
                    if !self.managed_by_caller(Some(&account.kind), uid, user)? {
                        return Err(Exception::security(format!(
                            "uid {uid} cannot peek the authtokens associated with accounts of type: {}",
                            account.kind
                        )));
                    }
                    let mut token = None;
                    if self.unlocked(user)? {
                        let accounts = self.accounts(user)?;
                        let mut state = accounts.state.lock().unwrap();
                        // `readAuthTokenInternal`.
                        if !state.auth_tokens.contains_key(&account) {
                            let tokens = state.db.find_auth_tokens_by_account(&account).map_err(db_failed)?;
                            state.auth_tokens.insert(account.clone(), tokens);
                        }
                        token = state.auth_tokens[&account].get(&kind).cloned().flatten();
                    }
                    am::write_peek_auth_token_reply(&mut p, &token);
                    Ok(p)
                })())
            }
            am::GET_PREVIOUS_NAME => {
                let a = am::GetPreviousName::<Account>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let mut p = Parcel::new();
                    let account = a.account.ok_or_else(|| null("account"))?;
                    let accounts = self.accounts(user)?;
                    let mut state = accounts.state.lock().unwrap();
                    // `readPreviousNameInternal`.
                    let name = match state.previous_names.get(&account) {
                        Some(name) => name.clone(),
                        None => {
                            let name = state.db.find_de_account_previous_name(&account).map_err(db_failed)?;
                            state.previous_names.insert(account, name.clone());
                            name
                        }
                    };
                    am::write_get_previous_name_reply(&mut p, &name);
                    Ok(p)
                })())
            }
            am::SOME_USER_HAS_ACCOUNT => {
                let a = am::SomeUserHasAccount::<Account>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let mut p = Parcel::new();
                    if !is_same_app(SYSTEM_UID, uid) {
                        return Err(Exception::security("Only system can check for accounts across users"));
                    }
                    // `getAllAccountsForSystemProcess`: every alive user's.
                    let (state, _) = self.host.packages()?;
                    let mut has = false;
                    if let Some(account) = &a.account {
                        for &user in state.users.keys() {
                            let accounts = self.accounts(user)?;
                            let all = self.accounts_from_cache(&accounts, user, None, SYSTEM_UID, Some("android"), false)?;
                            has |= all.contains(account);
                        }
                    }
                    am::write_some_user_has_account_reply(&mut p, has);
                    Ok(p)
                })())
            }
            am::GET_PACKAGES_AND_VISIBILITY_FOR_ACCOUNT => {
                let a = am::GetPackagesAndVisibilityForAccount::<Account>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let mut p = Parcel::new();
                    let account = a.account.ok_or_else(|| null("account"))?;
                    if !self.may_read_visibility(&account.kind, uid)? {
                        return Err(Exception::security(format!("uid {uid} cannot get secrets for account {account}")));
                    }
                    let accounts = self.accounts(user)?;
                    let mut state = accounts.state.lock().unwrap();
                    let map: Vec<(ParcelValue, ParcelValue)> = state
                        .visibility
                        .entry(account)
                        .or_default()
                        .iter()
                        .map(|(package, v)| (ParcelValue::String(Some(package.clone())), ParcelValue::Integer(*v)))
                        .collect();
                    am::write_get_packages_and_visibility_for_account_reply(&mut p, &Some(map));
                    Ok(p)
                })())
            }
            am::GET_ACCOUNT_VISIBILITY => {
                let a = am::GetAccountVisibility::<Account>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let mut p = Parcel::new();
                    let account = a.a.ok_or_else(|| null("account"))?;
                    let package = a.package_name.ok_or_else(|| null("packageName"))?;
                    if !self.may_read_visibility(&account.kind, uid)? {
                        return Err(Exception::security(format!(
                            "uid {uid} cannot get secrets for accounts of type: {}",
                            account.kind
                        )));
                    }
                    let accounts = self.accounts(user)?;
                    let legacy = [(LEGACY_VISIBLE, VISIBILITY_USER_MANAGED_VISIBLE), (LEGACY_NOT_VISIBLE, VISIBILITY_USER_MANAGED_NOT_VISIBLE)];
                    let visibility = match legacy.iter().find(|(key, _)| *key == package) {
                        Some((key, default)) => match accounts.state.lock().unwrap().stored_visibility(&account, key) {
                            VISIBILITY_UNDEFINED => *default,
                            visibility => visibility,
                        },
                        None if !self.can_access_package(Some(&package), uid, user)? => VISIBILITY_NOT_VISIBLE,
                        None => self.resolve_visibility(&accounts, &account, &package, user)?,
                    };
                    am::write_get_account_visibility_reply(&mut p, visibility);
                    Ok(p)
                })())
            }
            am::GET_ACCOUNTS_AND_VISIBILITY_FOR_PACKAGE => {
                let a = am::GetAccountsAndVisibilityForPackage::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let mut p = Parcel::new();
                    let system = is_same_app(uid, SYSTEM_UID);
                    let managed = self.types_for_caller(uid, user, system)?;
                    let kind = a.account_type;
                    if kind.as_ref().is_some_and(|k| !managed.contains(k)) || kind.is_none() && !system {
                        return Err(Exception::security(format!(
                            "getAccountsAndVisibilityForPackage() called from unauthorized uid {uid} with packageName={}",
                            a.package_name.as_deref().unwrap_or("null")
                        )));
                    }
                    let types = match kind {
                        Some(kind) => vec![kind],
                        None => managed,
                    };
                    let accounts = self.accounts(user)?;
                    let mut map = Vec::new();
                    let package = a.package_name.as_deref();
                    if self.can_access_package(package, uid, user)? {
                        for kind in &types {
                            let of_type = accounts.state.lock().unwrap().of(Some(kind));
                            for account in of_type {
                                let visibility = self.resolve_visibility(&accounts, &account, package.unwrap_or_default(), user)?;
                                map.push((account_value(&account), ParcelValue::Integer(visibility)));
                            }
                        }
                    }
                    am::write_get_accounts_and_visibility_for_package_reply(&mut p, &Some(map));
                    Ok(p)
                })())
            }
            am::REGISTER_ACCOUNT_LISTENER | am::UNREGISTER_ACCOUNT_LISTENER => {
                let register = call.code == am::REGISTER_ACCOUNT_LISTENER;
                let (types, package) = if register {
                    let a = am::RegisterAccountListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                    (a.account_types, a.op_package_name)
                } else {
                    let a = am::UnregisterAccountListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                    (a.account_types, a.op_package_name)
                };
                reply((|| {
                    self.check_package(uid, package.as_deref())?;
                    let accounts = self.accounts(user)?;
                    let package = package.unwrap_or_default();
                    let types = types.unwrap_or_else(|| vec![None]);
                    let mut receivers = accounts.receivers.lock().unwrap();
                    let mut p = Parcel::new();
                    if register {
                        for kind in types {
                            *receivers.entry(kind).or_default().entry(package.clone()).or_default() += 1;
                        }
                        am::write_register_account_listener_reply(&mut p);
                    } else {
                        for kind in types {
                            let counts = receivers
                                .get_mut(&kind)
                                .filter(|r| r.contains_key(&package))
                                .ok_or_else(|| Exception::illegal_argument("attempt to unregister wrong receiver"))?;
                            let count = counts.get_mut(&package).unwrap();
                            *count -= 1;
                            if *count == 0 {
                                counts.remove(&package);
                            }
                        }
                        am::write_unregister_account_listener_reply(&mut p);
                    }
                    Ok(p)
                })())
            }
            am::HAS_ACCOUNT_ACCESS => {
                let a = am::HasAccountAccess::<Account, UserHandle>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let mut p = Parcel::new();
                    if apps_filter::app_id(uid) != SYSTEM_UID {
                        return Err(Exception::security("Can be called only by system UID"));
                    }
                    let account = a.account.ok_or_else(|| null("account"))?;
                    let package = a.package_name.ok_or_else(|| null("packageName"))?;
                    let target = a.user_handle.ok_or_else(|| null("userHandle"))?.0;
                    if target < 0 {
                        return Err(Exception::illegal_argument(format!(
                            "user must be concrete is out of range of [0, {}] (too low)",
                            i32::MAX
                        )));
                    }
                    let access = match self.package_uid(&package, target)? {
                        Some(package_uid) => self.has_account_access(&account, Some(&package), package_uid)?,
                        None => false,
                    };
                    am::write_has_account_access_reply(&mut p, access);
                    Ok(p)
                })())
            }
            am::ON_ACCOUNT_ACCESSED => {
                let a = am::OnAccountAccessed::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    let mut p = Parcel::new();
                    if apps_filter::app_id(uid) != SYSTEM_UID {
                        // `getAccounts(user, "android")`, as the system.
                        let android = Some("android");
                        for account in self.accounts_as_user_for_package(SYSTEM_UID, None, user, android, -1, android, false)? {
                            if account.access_id == a.token && !self.has_account_access(&account, None, uid)? {
                                return Err(self.host.unimplemented_named("account", "granting an app access to an account (#1504)"));
                            }
                        }
                    }
                    am::write_on_account_accessed_reply(&mut p);
                    Ok(p)
                })())
            }
            code => {
                let name = am::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                reply(Err(self.host.unimplemented_named("account", &format!("{name} (#1504)"))))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signatures(current: &[&[u8]], past: Option<&[(&[u8], i32)]>) -> Signatures {
        Signatures {
            scheme_version: 3,
            signatures: current.iter().map(|s| s.to_vec()).collect(),
            past_signatures: past.map(|p| p.iter().map(|(c, f)| (c.to_vec(), *f)).collect()),
            ..Default::default()
        }
    }

    /// `checkCapability` and `hasAncestorOrSelf` of `AUTH`: the same
    /// signer, a rotated signer that granted AUTH to its past one, and
    /// the past one of a rotated authenticator.
    #[test]
    fn auth_capability_follows_the_signing_lineage() {
        let a = signatures(&[b"A"], None);
        assert!(has_certificate(&a, b"A", CERT_AUTH));
        assert!(!has_certificate(&a, b"B", CERT_AUTH));
        // B rotated from A, granting A AUTH: an app signed by A may manage
        // B's accounts, and B's app A's.
        let rotated = signatures(&[b"B"], Some(&[(b"A", CERT_AUTH), (b"B", 0)]));
        assert!(has_certificate(&rotated, b"A", CERT_AUTH));
        assert!(has_certificate(&rotated, b"A", PAST_CERT_EXISTS));
        // Without AUTH granted, only the lineage remains.
        let no_auth = signatures(&[b"B"], Some(&[(b"A", 1), (b"B", 0)]));
        assert!(!has_certificate(&no_auth, b"A", CERT_AUTH));
        assert!(has_certificate(&no_auth, b"A", PAST_CERT_EXISTS));
        let multiple = signatures(&[b"A", b"B"], None);
        assert!(signatures_match_exactly(&multiple, &signatures(&[b"B", b"A"], None)));
        assert!(!signatures_match_exactly(&multiple, &a));
    }

    /// The packages of the shared user or the package of a uid hold its
    /// signatures; a uid of neither has none.
    #[test]
    fn the_authenticators_signers_manage_its_accounts() {
        use crate::package::model::{PackageState, SharedUser};
        let mut state = State::default();
        for (name, app_id, signer) in [("auth", 10100, b"A"), ("same", 10101, b"A"), ("other", 10102, b"B")] {
            let ps = PackageState {
                name: name.into(),
                app_id,
                signatures: Some(signatures(&[signer], None)),
                ..Default::default()
            };
            state.packages.insert(name.into(), ps);
        }
        let shared = SharedUser {
            name: "shared".into(),
            app_id: 10103,
            signatures: Some(signatures(&[b"A"], None)),
            ..Default::default()
        };
        state.shared_users.insert("shared".into(), shared);
        assert!(has_auth_capability(&state, 10100, 10101));
        assert!(has_auth_capability(&state, 10100, 1_010_101));
        assert!(has_auth_capability(&state, 10100, 10103));
        assert!(!has_auth_capability(&state, 10100, 10102));
        assert!(!has_auth_capability(&state, 10100, 10999));
    }

    /// A user's packages for the visibility rules: each one's uid, whether
    /// it targets an SDK before O, and the permissions it holds; the
    /// authenticator of `com.example` is uid 10100's, and 10101 is signed
    /// as it is.
    #[derive(Default)]
    struct Packages {
        packages: HashMap<&'static str, (i32, bool, Vec<&'static str>)>,
        contacts_authenticator: bool,
    }

    impl VisibilityInputs for Packages {
        fn package_uid(&self, package: &str) -> Result<Option<i32>, Exception> {
            Ok(self.packages.get(package).map(|p| p.0))
        }

        fn package_signature(&self, kind: &str, uid: i32) -> Result<i32, Exception> {
            Ok(match (kind, uid) {
                ("com.example", 10100) => SIGNATURE_CHECK_UID_MATCH,
                ("com.example", 10101) => SIGNATURE_CHECK_MATCH,
                _ => SIGNATURE_CHECK_MISMATCH,
            })
        }

        fn permitted(&self, package: &str, permissions: &[&str]) -> Result<bool, Exception> {
            Ok(self.packages[package].2.iter().any(|p| permissions.contains(p)))
        }

        fn profile_owner(&self, _uid: i32) -> Result<bool, Exception> {
            Ok(false)
        }

        fn pre_o(&self, package: &str) -> Result<bool, Exception> {
            Ok(self.packages[package].1)
        }

        fn manages_contacts(&self, _kind: &str) -> Result<bool, Exception> {
            Ok(self.contacts_authenticator)
        }
    }

    /// `resolveAccountVisibility`'s cases: the system and the
    /// authenticator always see the account, a stored value wins, then
    /// the legacy keys' values or their defaults by signature,
    /// GET_ACCOUNTS before O, READ_CONTACTS for a contacts authenticator
    /// and GET_ACCOUNTS_PRIVILEGED.
    #[test]
    fn visibility_follows_the_original_rules() {
        let account = Account::new("a@example.com".into(), "com.example".into());
        let mut inputs = Packages::default();
        for (name, uid, pre_o, permissions) in [
            ("android", 1000, false, vec![]),
            ("auth", 10100, false, vec![]),
            ("signed", 10101, false, vec![]),
            ("plain", 10200, false, vec![]),
            ("old", 10201, true, vec![GET_ACCOUNTS]),
            ("new", 10202, false, vec![GET_ACCOUNTS]),
            ("contacts", 10203, false, vec![READ_CONTACTS]),
            ("privileged", 10204, false, vec![GET_ACCOUNTS_PRIVILEGED]),
        ] {
            inputs.packages.insert(name, (uid, pre_o, permissions));
        }
        let resolve = |inputs: &Packages, stored: &mut dyn FnMut(&str) -> i32, package: &str| {
            resolve_visibility(inputs, stored, &account, package).unwrap()
        };
        let none = &mut |_: &str| VISIBILITY_UNDEFINED;
        assert_eq!(resolve(&inputs, none, "missing"), VISIBILITY_NOT_VISIBLE);
        assert_eq!(resolve(&inputs, none, "android"), VISIBILITY_VISIBLE);
        assert_eq!(resolve(&inputs, &mut |_| VISIBILITY_NOT_VISIBLE, "auth"), VISIBILITY_VISIBLE);
        assert_eq!(resolve(&inputs, none, "signed"), VISIBILITY_USER_MANAGED_VISIBLE);
        assert_eq!(resolve(&inputs, none, "plain"), VISIBILITY_USER_MANAGED_NOT_VISIBLE);
        assert_eq!(resolve(&inputs, none, "old"), VISIBILITY_USER_MANAGED_VISIBLE);
        assert_eq!(resolve(&inputs, none, "new"), VISIBILITY_USER_MANAGED_NOT_VISIBLE);
        assert_eq!(resolve(&inputs, none, "contacts"), VISIBILITY_USER_MANAGED_NOT_VISIBLE);
        assert_eq!(resolve(&inputs, none, "privileged"), VISIBILITY_USER_MANAGED_VISIBLE);
        inputs.contacts_authenticator = true;
        assert_eq!(resolve(&inputs, none, "contacts"), VISIBILITY_USER_MANAGED_VISIBLE);
        let stored_for = |key: &'static str, value: i32| move |k: &str| if k == key { value } else { VISIBILITY_UNDEFINED };
        assert_eq!(resolve(&inputs, &mut stored_for("plain", VISIBILITY_VISIBLE), "plain"), VISIBILITY_VISIBLE);
        assert_eq!(
            resolve(&inputs, &mut stored_for(LEGACY_NOT_VISIBLE, VISIBILITY_NOT_VISIBLE), "plain"),
            VISIBILITY_NOT_VISIBLE
        );
        assert_eq!(resolve(&inputs, &mut stored_for(LEGACY_VISIBLE, VISIBILITY_VISIBLE), "signed"), VISIBILITY_VISIBLE);
    }

    #[test]
    fn accounts_are_equal_by_name_and_type() {
        let mut a = Account::new("a@example.com".into(), "com.example".into());
        let b = Account { access_id: Some(random_uuid()), ..a.clone() };
        assert_eq!(a, b);
        a.kind = "org.other".into();
        assert_ne!(a, b);
        assert_eq!(a.to_string(), "Account {name=a@example.com, type=org.other}");
    }

    /// `Account.writeToParcel` and `Account(Parcel)`, which refuses an
    /// empty name; as a `Map` key, `writeValue`'s parcelable.
    #[test]
    fn account_parcels_as_the_original() {
        let a = Account { name: "a".into(), kind: "t".into(), access_id: Some("id".into()) };
        let mut p = Parcel::new();
        a.write_to(&mut p);
        let back = Account::read_from(&mut p.reader()).unwrap();
        assert_eq!((back.name, back.kind, back.access_id.as_deref()), ("a".into(), "t".into(), Some("id")));
        let mut empty = Parcel::new();
        empty.write_string16(Some(""));
        empty.write_string16(Some("t"));
        empty.write_string16(None);
        assert!(Account::read_from(&mut empty.reader()).is_err());
        let mut p = Parcel::new();
        account_value(&a).write(&mut p);
        let mut r = p.reader();
        assert_eq!(r.read_i32().unwrap(), 4);
        let length = r.read_i32().unwrap() as usize;
        assert_eq!(length, r.remaining());
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("android.accounts.Account"));
        assert_eq!(Account::read_from(&mut r).unwrap(), a);
    }

    #[test]
    fn random_uuids_are_version_4() {
        let id = random_uuid();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
        assert_ne!(id, random_uuid());
    }
}
