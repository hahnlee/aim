//! AccountManagerService's sessions with an account authenticator
//! (`Session` at `android-16.0.0_r1`): the authenticator's service bound
//! (`bindToAuthenticator`), the session's work run once it connects, its
//! answers relayed to the caller's `IAccountManagerResponse`, and the
//! session closed on its result, its error, its timeout or the caller's
//! death. The one session the app host runs is
//! `GetAccountsByTypeAndFeatureSession`: each account of a type asked
//! whether it has the features (`hasFeatures`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Parcel};
use aim_service_aidl::{
    WriteParcelable, android_accounts_iaccountauthenticator as authenticator,
    android_accounts_iaccountauthenticatorresponse as auth_response, android_accounts_iaccountmanagerresponse as response,
};

use super::AppHost;
use super::account_service::Account;
use super::services::HostConnection;
use crate::bundle::Value;

/// `AccountManager.ERROR_CODE_REMOTE_EXCEPTION`, `ERROR_CODE_INVALID_RESPONSE`,
/// `KEY_ACCOUNTS`, `KEY_BOOLEAN_RESULT`, `ACTION_AUTHENTICATOR_INTENT`.
const ERROR_CODE_REMOTE_EXCEPTION: i32 = 1;
const ERROR_CODE_INVALID_RESPONSE: i32 = 5;
const KEY_ACCOUNTS: &str = "accounts";
const KEY_BOOLEAN_RESULT: &str = "booleanResult";
pub(super) const ACTION_AUTHENTICATOR_INTENT: &str = "android.accounts.AccountAuthenticator";
/// `AccountManagerService.TIMEOUT_DELAY_MS`; `Context.BIND_AUTO_CREATE`.
const TIMEOUT: Duration = Duration::from_secs(60);
const BIND_AUTO_CREATE: i64 = 1;

/// The accounts of the session's type the caller may see, read when the
/// authenticator connects (`getAccountsFromCache`).
pub(super) type AccountsOfType = Box<dyn FnOnce() -> Result<Vec<Account>, String> + Send>;

#[derive(Default)]
struct State {
    /// The caller's response and its death link, until the session closes.
    response: Option<(Strong, u64)>,
    authenticator: Option<Strong>,
    accounts_of_type: Option<AccountsOfType>,
    accounts: Vec<Account>,
    current: usize,
    with_features: Vec<Account>,
}

/// A `GetAccountsByTypeAndFeatureSession`.
pub(super) struct FeaturesSession {
    host: Arc<AppHost>,
    features: Vec<String>,
    /// This session as the authenticator's `IAccountAuthenticatorResponse`.
    binder: Mutex<Option<Binder>>,
    connection: Arc<dyn HostConnection>,
    state: Mutex<State>,
}

/// The session's connection to its authenticator.
struct Connection(Weak<FeaturesSession>);

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

impl FeaturesSession {
    /// The session for `response`, its timeout scheduled.
    pub(super) fn new(host: Arc<AppHost>, response: Strong, features: Vec<String>, accounts_of_type: AccountsOfType) -> Arc<Self> {
        let session = Arc::new_cyclic(|me: &Weak<FeaturesSession>| FeaturesSession {
            host: host.clone(),
            features,
            binder: Mutex::new(None),
            connection: Arc::new(Connection(me.clone())),
            state: Mutex::new(State { accounts_of_type: Some(accounts_of_type), ..Default::default() }),
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

    /// `bind`: the authenticator `(package, class)` of `user` bound, an
    /// error if it cannot be.
    pub(super) fn bind(&self, component: Option<(&str, &str)>, user: i32) {
        let bound = component.map(|(package, class)| {
            self.host.bind_service_from_host(Some(ACTION_AUTHENTICATOR_INTENT), package, class, 0, BIND_AUTO_CREATE, user, self.connection.clone())
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
        let accounts_of_type = {
            let mut state = self.state.lock().unwrap();
            state.authenticator = Some(self.host.process.strong(handle));
            state.accounts_of_type.take()
        };
        let Some(accounts_of_type) = accounts_of_type else { return };
        match accounts_of_type() {
            Ok(accounts) => {
                let mut state = self.state.lock().unwrap();
                state.accounts = accounts;
                state.current = 0;
                state.with_features.clear();
            }
            Err(e) => {
                eprintln!("app host: accounts: {e}");
                return self.respond_error(ERROR_CODE_REMOTE_EXCEPTION, "remote exception");
            }
        }
        self.check_account();
    }

    /// `checkAccount`: the next account asked, or the result sent.
    fn check_account(&self) {
        let (authenticator, account) = {
            let state = self.state.lock().unwrap();
            if state.current >= state.accounts.len() {
                drop(state);
                return self.send_result();
            }
            let Some(authenticator) = state.authenticator.as_ref().map(|a| a.handle) else { return };
            (authenticator, state.accounts[state.current].clone())
        };
        let Some(binder) = *self.binder.lock().unwrap() else { return };
        let mut p = Parcel::new();
        authenticator::HasFeatures {
            response: Some(binder),
            account: Some(account),
            features: Some(self.features.iter().cloned().map(Some).collect()),
        }
        .write(&mut p);
        if self.host.process.transact(authenticator, authenticator::HAS_FEATURES, &p, true).is_err() {
            self.respond_error(ERROR_CODE_REMOTE_EXCEPTION, "remote exception");
        }
    }

    /// `sendResult`: the accounts with the features.
    fn send_result(&self) {
        let accounts = std::mem::take(&mut self.state.lock().unwrap().with_features);
        let Some(response) = self.response_and_close() else { return };
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

    /// The caller told `code` and `message` (`onError`, `onTimedOut`,
    /// `onServiceDisconnected`), the session closed.
    fn respond_error(&self, code: i32, message: &str) {
        let Some(response) = self.response_and_close() else { return };
        let mut p = Parcel::new();
        response::OnError { error_code: code, error_message: Some(message.into()) }.write(&mut p);
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
            state.accounts_of_type = None;
            state.authenticator.take().is_some()
        };
        if bound {
            self.host.unbind_service_from_host(&self.connection);
        }
    }
}

impl Service for FeaturesSession {
    fn descriptor(&self) -> &str {
        auth_response::DESCRIPTOR
    }

    /// The authenticator's answers (`IAccountAuthenticatorResponse`).
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            auth_response::ON_RESULT => {
                let a = auth_response::OnResult::<super::kept::RawBundle>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match a.value {
                    None => self.respond_error(ERROR_CODE_INVALID_RESPONSE, "null bundle"),
                    Some(bundle) => {
                        let mut raw = Parcel::new();
                        bundle.write_to(&mut raw);
                        let has = crate::bundle::read(&mut raw.reader())
                            .ok()
                            .flatten()
                            .is_some_and(|m| matches!(m.get(KEY_BOOLEAN_RESULT), Some(Value::Bool(true))));
                        {
                            let mut state = self.state.lock().unwrap();
                            if has && let Some(account) = state.accounts.get(state.current).cloned() {
                                state.with_features.push(account);
                            }
                            state.current += 1;
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
