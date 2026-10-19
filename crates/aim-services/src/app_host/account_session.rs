//! AccountManagerService's sessions with an account authenticator
//! (`Session` at `android-16.0.0_r1`): the authenticator's service bound
//! (`bindToAuthenticator`), the session's work run once it connects, its
//! answers relayed to the caller's `IAccountManagerResponse`, and the
//! session closed on its result, its error, its timeout or the caller's
//! death. A result's `KEY_INTENT` is checked as `checkKeyIntent` checks
//! it, and kept as it resolves; a session that expects an activity launch
//! stays open while the authenticator's activity runs. The sessions the
//! app host runs are `GetAccountsByTypeAndFeatureSession` (each account of
//! a type asked `hasFeatures`) and `addAccount`'s.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Exception, Parcel};
use aim_service_aidl::{
    WriteParcelable, android_accounts_iaccountauthenticator as authenticator,
    android_accounts_iaccountauthenticatorresponse as auth_response,
    android_accounts_iaccountmanagerresponse as response,
};

use super::AppHost;
use super::account_service::Account;
use super::kept::RawBundle;
use super::services::HostConnection;
use crate::bundle::Value;

/// `AccountManager.ERROR_CODE_*` and the bundle keys.
const ERROR_CODE_REMOTE_EXCEPTION: i32 = 1;
const ERROR_CODE_INVALID_RESPONSE: i32 = 5;
const KEY_ACCOUNTS: &str = "accounts";
const KEY_BOOLEAN_RESULT: &str = "booleanResult";
const KEY_INTENT: &str = "intent";
const KEY_AUTHTOKEN: &str = "authtoken";
const KEY_ERROR_CODE: &str = "errorCode";
const KEY_ERROR_MESSAGE: &str = "errorMessage";
pub(super) const ACTION_AUTHENTICATOR_INTENT: &str = "android.accounts.AccountAuthenticator";
/// `AccountManagerService.TIMEOUT_DELAY_MS`; `Context.BIND_AUTO_CREATE`.
const TIMEOUT: Duration = Duration::from_secs(15 * 60);
const BIND_AUTO_CREATE: i64 = 1;
/// The grant flags a `KEY_INTENT` may not carry
/// (`checkKeyIntentParceledCorrectly`).
const PROHIBITED_FLAGS: i32 = 0x1 | 0x2 | 0x40 | 0x80;
/// `CantAddAccountActivity`, an exported system activity a `KEY_INTENT`
/// may name (`isExportedSystemActivity`).
const CANT_ADD_ACCOUNT_ACTIVITY: &str = "android.accounts.CantAddAccountActivity";

/// The accounts of the session's type the caller may see, read when the
/// authenticator connects (`getAccountsFromCache`).
pub(super) type AccountsOfType = Box<dyn FnOnce() -> Result<Vec<Account>, String> + Send>;

/// What a session asks of its authenticator.
pub(super) enum Work {
    /// `GetAccountsByTypeAndFeatureSession`.
    Features {
        features: Vec<String>,
        accounts_of_type: Option<AccountsOfType>,
        accounts: Vec<Account>,
        current: usize,
        with_features: Vec<Account>,
    },
    /// `addAccount`'s `run`: the options with the caller's uid and pid.
    AddAccount {
        auth_token_type: Option<String>,
        required_features: Option<Vec<Option<String>>>,
        options: RawBundle,
    },
}

impl Work {
    pub(super) fn features(features: Vec<String>, accounts_of_type: AccountsOfType) -> Work {
        Work::Features {
            features,
            accounts_of_type: Some(accounts_of_type),
            accounts: Vec::new(),
            current: 0,
            with_features: Vec::new(),
        }
    }
}

/// What the session was made with (`Session`'s constructor).
pub(super) struct Kind {
    pub account_type: String,
    pub user: i32,
    pub expect_activity_launch: bool,
    pub strip_auth_token: bool,
}

struct State {
    /// The caller's response and its death link, until the session closes.
    response: Option<(Strong, u64)>,
    authenticator: Option<Strong>,
    work: Work,
}

/// A `Session`.
pub(super) struct Session {
    host: Arc<AppHost>,
    kind: Kind,
    /// This session as the authenticator's `IAccountAuthenticatorResponse`.
    binder: Mutex<Option<Binder>>,
    connection: Arc<dyn HostConnection>,
    state: Mutex<State>,
}

/// The session's connection to its authenticator.
struct Connection(Weak<Session>);

impl HostConnection for Connection {
    fn connected(&self, service: Option<Binder>, _dead: bool) {
        let session = self.0.clone();
        std::thread::spawn(move || {
            if let Some(s) = session.upgrade() {
                match service {
                    Some(Binder::Handle(h)) => s.on_service_connected(h),
                    // `onServiceDisconnected`, `onNullBinding`.
                    _ => s.respond_error(ERROR_CODE_REMOTE_EXCEPTION, "disconnected"),
                }
            }
        });
    }
}

/// The response's `onResult` of `bundle`.
fn on_result(response: &Strong, bundle: &RawBundle) {
    let mut p = Parcel::new();
    p.write_interface_token(response::DESCRIPTOR);
    p.write_i32(1);
    bundle.write_to(&mut p);
    let _ = response.transact(response::ON_RESULT, &p, true);
}

impl Session {
    /// The session for `response`, its timeout scheduled.
    pub(super) fn new(host: Arc<AppHost>, response: Strong, kind: Kind, work: Work) -> Arc<Self> {
        let session = Arc::new_cyclic(|me: &Weak<Session>| Session {
            host: host.clone(),
            kind,
            binder: Mutex::new(None),
            connection: Arc::new(Connection(me.clone())),
            state: Mutex::new(State {
                response: None,
                authenticator: None,
                work,
            }),
        });
        *session.binder.lock().unwrap() = Some(host.process.add_service(session.clone()));
        let weak = Arc::downgrade(&session);
        let death = host.process.link_to_death(
            &response,
            Box::new(move || {
                // `binderDied`.
                if let Some(s) = weak.upgrade() {
                    s.state.lock().unwrap().response = None;
                    s.close();
                }
            }),
        );
        session.state.lock().unwrap().response = Some((response, death));
        let weak = Arc::downgrade(&session);
        std::thread::spawn(move || {
            std::thread::sleep(TIMEOUT);
            // `onTimedOut`.
            if let Some(s) = weak.upgrade() {
                s.respond_error(ERROR_CODE_REMOTE_EXCEPTION, "timeout");
            }
        });
        session
    }

    pub(super) fn account_type(&self) -> &str {
        &self.kind.account_type
    }

    /// `bind`: the authenticator `(package, class)` bound, an error if it
    /// cannot be.
    pub(super) fn bind(&self, component: Option<(&str, &str)>) {
        let bound = component.map(|(package, class)| {
            self.host.bind_service_from_host(
                Some(ACTION_AUTHENTICATOR_INTENT),
                package,
                class,
                0,
                BIND_AUTO_CREATE,
                self.kind.user,
                self.connection.clone(),
            )
        });
        match bound {
            Some(Ok(true)) => {}
            Some(Err(e)) => {
                eprintln!("app host: accounts: bindService failed: {}", e.message);
                self.respond_error(ERROR_CODE_REMOTE_EXCEPTION, "bind failure");
            }
            _ => self.respond_error(ERROR_CODE_REMOTE_EXCEPTION, "bind failure"),
        }
    }

    /// `onServiceConnected`, then `run`.
    fn on_service_connected(&self, handle: u32) {
        let mut state = self.state.lock().unwrap();
        state.authenticator = Some(self.host.process.strong(handle));
        match &mut state.work {
            Work::Features {
                accounts_of_type,
                accounts,
                current,
                with_features,
                ..
            } => {
                let Some(read) = accounts_of_type.take() else {
                    return;
                };
                match read() {
                    Ok(read) => {
                        *accounts = read;
                        *current = 0;
                        with_features.clear();
                    }
                    Err(e) => {
                        eprintln!("app host: accounts: {e}");
                        drop(state);
                        return self.respond_error(ERROR_CODE_REMOTE_EXCEPTION, "remote exception");
                    }
                }
                drop(state);
                self.check_account();
            }
            Work::AddAccount {
                auth_token_type,
                required_features,
                options,
            } => {
                let Some(binder) = *self.binder.lock().unwrap() else {
                    return;
                };
                let mut p = Parcel::new();
                authenticator::AddAccount {
                    response: Some(binder),
                    account_type: Some(self.kind.account_type.clone()),
                    auth_token_type: auth_token_type.clone(),
                    required_features: required_features.clone(),
                    options: Some(options.clone()),
                }
                .write(&mut p);
                drop(state);
                if self
                    .host
                    .process
                    .transact(handle, authenticator::ADD_ACCOUNT, &p, true)
                    .is_err()
                {
                    self.respond_error(ERROR_CODE_REMOTE_EXCEPTION, "remote exception");
                }
            }
        }
    }

    /// `checkAccount`: the next account asked, or the result sent.
    fn check_account(&self) {
        let (authenticator, account, features) = {
            let state = self.state.lock().unwrap();
            let Work::Features {
                features,
                accounts,
                current,
                ..
            } = &state.work
            else {
                return;
            };
            if *current >= accounts.len() {
                drop(state);
                return self.send_features_result();
            }
            let Some(authenticator) = state.authenticator.as_ref().map(|a| a.handle) else {
                return;
            };
            (authenticator, accounts[*current].clone(), features.clone())
        };
        let Some(binder) = *self.binder.lock().unwrap() else {
            return;
        };
        let mut p = Parcel::new();
        authenticator::HasFeatures {
            response: Some(binder),
            account: Some(account),
            features: Some(features.into_iter().map(Some).collect()),
        }
        .write(&mut p);
        if self
            .host
            .process
            .transact(authenticator, authenticator::HAS_FEATURES, &p, true)
            .is_err()
        {
            self.respond_error(ERROR_CODE_REMOTE_EXCEPTION, "remote exception");
        }
    }

    /// `GetAccountsByTypeAndFeatureSession.sendResult`.
    fn send_features_result(&self) {
        let accounts = match &mut self.state.lock().unwrap().work {
            Work::Features { with_features, .. } => std::mem::take(with_features),
            Work::AddAccount { .. } => return,
        };
        let Some(response) = self.response_and_close() else {
            return;
        };
        let items = accounts
            .iter()
            .map(|a| {
                let mut p = Parcel::new();
                a.write_to(&mut p);
                ("android.accounts.Account".to_string(), p.data().to_vec())
            })
            .collect();
        let mut p = Parcel::new();
        p.write_interface_token(response::DESCRIPTOR);
        p.write_i32(1);
        crate::bundle::write(&mut p, &[(KEY_ACCOUNTS, Value::ParcelableArray(items))]);
        let _ = response.transact(response::ON_RESULT, &p, true);
    }

    /// `checkKeyIntent` of the authenticator `auth_uid`'s `bundle`: the
    /// bundle with its `KEY_INTENT` resolved to its activity and given
    /// clip data, or none if the intent may not be launched.
    fn check_key_intent(
        &self,
        auth_uid: i32,
        bundle: RawBundle,
    ) -> Result<Option<RawBundle>, Exception> {
        let invalid = |e| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("accounts: {e:?}"),
            )
        };
        let Some((class, intent)) = bundle.intent(KEY_INTENT).map_err(invalid)? else {
            return Ok(Some(bundle));
        };
        // `checkKeyIntentParceledCorrectly`.
        if class != "android.content.Intent" || intent.intent.flags & PROHIBITED_FLAGS != 0 {
            return Ok(None);
        }
        let intent = intent.with_plain_clip_data_if_none().map_err(invalid)?;
        let (state, resolution) = self.host.packages()?;
        let resolved = resolution
            .resolve_intent(&intent.intent, None, 0, self.kind.user, 1000)
            .map_err(|e| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("accounts: resolveActivity: {e:?}"),
                )
            })?;
        let Some(crate::package::component_resolver::Info::Activity(target)) =
            resolved.map(|r| r.info)
        else {
            return Ok(None);
        };
        if intent.intent.scheme() == Some("content") {
            return Ok(None);
        }
        let package = target
            .info
            .application_info
            .item
            .package_name
            .clone()
            .unwrap_or_default();
        let name = target.info.item.name.clone().unwrap_or_default();
        let system_activity = package == "android" && name == CANT_ADD_ACCOUNT_ACTIVITY;
        if !system_activity
            && !super::account_service::has_auth_capability(
                &state,
                target.info.application_info.uid,
                auth_uid,
            )
        {
            eprintln!(
                "app host: accounts: KEY_INTENT resolved to an Activity ({name}) in a package ({package}) that does not share a signature with the supplying authenticator ({}).",
                self.kind.account_type
            );
            return Ok(None);
        }
        Ok(Some(
            bundle
                .with_intent(KEY_INTENT, &intent.with_component(&package, &name))
                .map_err(invalid)?,
        ))
    }

    /// `Session.onResult` of the authenticator `auth_uid`'s `bundle`.
    fn on_session_result(&self, auth_uid: i32, bundle: Option<RawBundle>) -> Result<(), Exception> {
        let Some(bundle) = bundle else {
            if let Some(response) = self.response_and_close() {
                let mut p = Parcel::new();
                response::OnError {
                    error_code: ERROR_CODE_INVALID_RESPONSE,
                    error_message: Some("null bundle returned".into()),
                }
                .write(&mut p);
                let _ = response.transact(response::ON_ERROR, &p, true);
            }
            return Ok(());
        };
        let Some(bundle) = self.check_key_intent(auth_uid, bundle)? else {
            self.respond_error(
                ERROR_CODE_INVALID_RESPONSE,
                "invalid intent in bundle returned",
            );
            return Ok(());
        };
        // A sign-in notification of the account would be cancelled here
        // for a result with an auth token; the app host posts none.
        let launches =
            self.kind.expect_activity_launch && bundle.intent(KEY_INTENT).ok().flatten().is_some();
        let response = if launches {
            self.state
                .lock()
                .unwrap()
                .response
                .as_ref()
                .map(|(s, _)| self.host.process.strong(s.handle))
        } else {
            self.response_and_close()
        };
        let Some(response) = response else {
            return Ok(());
        };
        let bundle = if self.kind.strip_auth_token {
            match bundle
                .without(KEY_AUTHTOKEN)
                .ok()
                .map(|b| self.check_key_intent(auth_uid, b))
            {
                Some(Ok(Some(b))) => b,
                Some(Err(e)) => return Err(e),
                _ => {
                    self.respond_error(
                        ERROR_CODE_INVALID_RESPONSE,
                        "invalid intent in bundle returned",
                    );
                    return Ok(());
                }
            }
        } else {
            bundle
        };
        let mut raw = Parcel::new();
        bundle.write_to(&mut raw);
        let entries = crate::bundle::read(&mut raw.reader())
            .ok()
            .flatten()
            .unwrap_or_default();
        match (entries.get(KEY_ERROR_CODE), entries.get(KEY_ERROR_MESSAGE)) {
            (Some(Value::Int(code)), message) if *code > 0 => {
                let message = match message {
                    Some(Value::String(m)) => m.clone(),
                    _ => None,
                };
                let mut p = Parcel::new();
                response::OnError {
                    error_code: *code,
                    error_message: message,
                }
                .write(&mut p);
                let _ = response.transact(response::ON_ERROR, &p, true);
            }
            _ => on_result(&response, &bundle),
        }
        Ok(())
    }

    /// The caller told `code` and `message` (`onError`, `onTimedOut`,
    /// `onServiceDisconnected`), the session closed.
    fn respond_error(&self, code: i32, message: &str) {
        let Some(response) = self.response_and_close() else {
            return;
        };
        let mut p = Parcel::new();
        response::OnError {
            error_code: code,
            error_message: Some(message.into()),
        }
        .write(&mut p);
        let _ = response.transact(response::ON_ERROR, &p, true);
    }

    /// `getResponseAndClose`.
    fn response_and_close(&self) -> Option<Strong> {
        let response = self.state.lock().unwrap().response.take();
        if let Some((strong, death)) = &response {
            self.host.process.clear_death(strong, *death);
        }
        self.close();
        response.map(|(strong, _)| strong)
    }

    /// `close`: unbound from the authenticator.
    fn close(&self) {
        let bound = {
            let mut state = self.state.lock().unwrap();
            if let Work::Features {
                accounts_of_type, ..
            } = &mut state.work
            {
                *accounts_of_type = None;
            }
            state.authenticator.take().is_some()
        };
        if bound {
            self.host.unbind_service_from_host(&self.connection);
        }
    }
}

impl Service for Session {
    fn descriptor(&self) -> &str {
        auth_response::DESCRIPTOR
    }

    /// The authenticator's answers (`IAccountAuthenticatorResponse`).
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            auth_response::ON_RESULT => {
                let a = auth_response::OnResult::<RawBundle>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let features = matches!(self.state.lock().unwrap().work, Work::Features { .. });
                if !features {
                    if let Err(e) = self.on_session_result(call.sender_euid as i32, a.value) {
                        eprintln!("app host: accounts: onResult: {}", e.message);
                        self.respond_error(
                            ERROR_CODE_INVALID_RESPONSE,
                            "invalid intent in bundle returned",
                        );
                    }
                    return Ok(Parcel::new());
                }
                // `GetAccountsByTypeAndFeatureSession.onResult`.
                match a.value {
                    None => self.respond_error(ERROR_CODE_INVALID_RESPONSE, "null bundle"),
                    Some(bundle) => {
                        let mut raw = Parcel::new();
                        bundle.write_to(&mut raw);
                        let has = crate::bundle::read(&mut raw.reader())
                            .ok()
                            .flatten()
                            .is_some_and(|m| {
                                matches!(m.get(KEY_BOOLEAN_RESULT), Some(Value::Bool(true)))
                            });
                        if let Work::Features {
                            accounts,
                            current,
                            with_features,
                            ..
                        } = &mut self.state.lock().unwrap().work
                        {
                            if has && let Some(account) = accounts.get(*current).cloned() {
                                with_features.push(account);
                            }
                            *current += 1;
                        }
                        self.check_account();
                    }
                }
            }
            auth_response::ON_REQUEST_CONTINUED => {
                auth_response::OnRequestContinued::read(&mut call.data).map_err(|_| BAD_VALUE)?;
            }
            auth_response::ON_ERROR => {
                let a = auth_response::OnError::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.respond_error(a.error_code, a.error_message.as_deref().unwrap_or_default());
            }
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        }
        Ok(Parcel::new())
    }
}
