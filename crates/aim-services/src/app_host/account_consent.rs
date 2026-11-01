//! Native request identity and the original GrantCredentialsPermissionActivity's
//! updateAppPermission -> IAccountAuthenticatorResponse completion protocol.
use super::super::kept::RawBundle;
use super::*;
use aim_binder_host::parcel::{Binder, UNKNOWN_TRANSACTION};
use aim_service_aidl::android_accounts_iaccountauthenticatorresponse as response;
use aim_service_aidl::android_os_iremotecallback as callback;
use std::sync::Weak;

pub(super) const ACCESS_TOKEN: &str = "android.accounts.AccountManager.ACCOUNT_ACCESS_TOKEN_TYPE";

#[derive(Default)]
pub(super) struct Completion {
    pub(super) closed: bool,
    pub(super) decision: Option<bool>,
    callback: Option<(Strong, u64)>,
    notification: Option<Strong>,
}

pub(super) struct Consent {
    manager: Weak<AccountManager>,
    pub(super) account: Account,
    account_id: Option<i64>,
    pub(super) package: String,
    pub(super) uid: i32,
    pub(super) state: Mutex<Completion>,
}

impl Consent {
    pub(super) fn matches(
        &self,
        account: &Account,
        uid: i32,
        state: &Accounts,
    ) -> Result<bool, Exception> {
        let id = state.db.account_id(account).map_err(db_failed)?;
        Ok(self.account == *account && self.uid == uid && id.is_some() && id == self.account_id)
    }

    pub(super) fn installed(&self, manager: &AccountManager) -> Result<bool, Exception> {
        Ok(manager.package_uid(&self.package, user_of(self.uid))? == Some(self.uid))
    }

    pub(super) fn cancel_notification(&self, state: &mut Completion) {
        if let Some(target) = state.notification.take() {
            if let Err(e) = self.notify(&target, None) {
                eprintln!("account notification cancel: {}", e.message);
            }
        }
    }

    fn notify(&self, target: &Strong, intent: Option<RawIntent>) -> Result<(), Exception> {
        use aim_service_aidl::dev_aim_server_iaccountconsentnotification as n;
        let mut p = Parcel::new();
        n::Update {
            intent,
            tag: Some(notification_tag(&self.account, self.uid)),
            uid: self.uid,
            account_name: Some(self.account.name.clone()),
            package_name: Some(self.package.clone()),
        }
        .write(&mut p);
        target
            .transact(n::UPDATE, &p, true)
            .map(|_| ())
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("account notification: {e:#x}")))
    }

    pub(super) fn finish(&self, success: bool, deliver: bool) -> Result<(), Exception> {
        let Some(manager) = self.manager.upgrade() else {
            return Ok(());
        };
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Ok(());
        }
        let granted = if success && state.decision == Some(true) && self.installed(&manager)? {
            let owner = manager
                .users
                .lock()
                .unwrap()
                .get(&user_of(self.uid))
                .cloned()
                .ok_or_else(|| Exception::illegal_argument("missing consent owner"))?;
            let accounts = owner.state.lock().unwrap();
            self.matches(&self.account, self.uid, &accounts)?
                && accounts
                    .db
                    .find_matching_grants_count_any_token(self.uid, &self.account)
                    .map_err(db_failed)?
                    > 0
        } else {
            false
        };
        state.closed = true;
        self.cancel_notification(&mut state);
        let target = state.callback.take();
        drop(state);
        if let Some((target, death)) = target {
            manager.host.process.clear_death(&target, death);
            if deliver {
                send_result(&target, granted);
            }
        }
        Ok(())
    }
}

// Account.hashCode / String.hashCode and SystemMessage.NOTE_ACCOUNT_CREDENTIAL_PERMISSION,
// android-16.0.0_r1 AccountManagerService.getCredentialPermissionNotificationId.
fn notification_tag(account: &Account, uid: i32) -> String {
    let hash = |s: &str| {
        s.encode_utf16()
            .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
    };
    let account_hash = 527i32
        .wrapping_add(hash(&account.name))
        .wrapping_mul(31)
        .wrapping_add(hash(&account.kind));
    format!(
        "AccountManagerService:38:{account_hash}:{}:{uid}",
        hash(ACCESS_TOKEN)
    )
}

impl Drop for Consent {
    fn drop(&mut self) {
        if let Err(e) = self.finish(false, false) {
            eprintln!("account consent cleanup: {}", e.message);
        }
    }
}

fn send_result(target: &Strong, granted: bool) {
    let mut p = Parcel::new();
    p.write_interface_token(callback::DESCRIPTOR);
    p.write_i32(1);
    crate::bundle::write(
        &mut p,
        &[("booleanResult", crate::bundle::Value::Bool(granted))],
    );
    if let Err(e) = target.transact(callback::SEND_RESULT, &p, true) {
        eprintln!("account consent callback failed: {e:#x}");
    }
}

impl Service for Consent {
    fn descriptor(&self) -> &str {
        response::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID as u32 {
            return reply(Err(Exception::security(
                "account consent requires system uid",
            )));
        }
        let result = match call.code {
            response::ON_RESULT => {
                response::OnResult::<RawBundle>::read(&mut call.data)?;
                self.finish(true, true)
            }
            response::ON_ERROR => {
                response::OnError::read(&mut call.data)?;
                self.finish(false, true)
            }
            response::ON_REQUEST_CONTINUED => {
                response::OnRequestContinued::read(&mut call.data)?;
                Ok(())
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| Parcel::new()))
    }
}

impl AccountManager {
    /// Internal requests suppress hidden accounts before the access shortcut.
    /// The public IntentSender factory does not apply that suppression.
    pub(super) fn prepare_access_consent(
        &self,
        account: &Account,
        package: &str,
        user: i32,
        callback: Option<Strong>,
        internal: bool,
        notification: Option<Strong>,
    ) -> Result<Option<RawIntent>, Exception> {
        require_user(user)?;
        let _order = self.consent_requests.lock().unwrap();
        let Some(uid) = self.package_uid(package, user)? else {
            return Ok(None);
        };
        let owner = self.accounts(user)?;
        if internal {
            if self.resolve_visibility(&owner, account, package, user)? == VISIBILITY_NOT_VISIBLE {
                return Ok(None);
            }
            if self.has_account_access(account, Some(package), uid)? {
                if let Some(target) = callback {
                    send_result(&target, true);
                }
                return Ok(None);
            }
        }
        let state = owner.state.lock().unwrap();
        let account_id = state.db.account_id(account).map_err(db_failed)?;
        drop(state);
        for old in self.pending_consents() {
            if old.account == *account && old.uid == uid {
                old.finish(false, true)?;
            }
        }
        let consent = Arc::new(Consent {
            manager: self.me.clone(),
            account: account.clone(),
            account_id,
            package: package.into(),
            uid,
            state: Mutex::new(Completion {
                notification,
                ..Default::default()
            }),
        });
        if let Some(target) = callback {
            let mut state = consent.state.lock().unwrap();
            let weak = Arc::downgrade(&consent);
            let death = self.host.process.link_to_death(
                &target,
                Box::new(move || {
                    if let Some(consent) = weak.upgrade() {
                        if let Err(e) = consent.finish(false, false) {
                            eprintln!("account consent death cleanup failed: {}", e.message);
                        }
                    }
                }),
            );
            state.callback = Some((target, death));
        }
        // The Activity returns this opaque access identity unchanged. Keep the
        // durable row identity separately so remove/re-add cannot authorize it.
        let nonce = format!("aim-consent:{}", random_uuid());
        self.consents
            .lock()
            .unwrap()
            .insert(nonce.clone(), Arc::downgrade(&consent));
        let binder = self.host.process.add_callback(consent.clone());
        let mut value = Parcel::new();
        Account {
            access_id: Some(nonce),
            ..account.clone()
        }
        .write_to(&mut value);
        let mut response_value = Parcel::new();
        response_value.write_binder(Some(binder));
        let mut token = Parcel::new();
        crate::bundle::write(
            &mut token,
            &[
                ("uid", crate::bundle::Value::Int(uid)),
                (
                    "authTokenType",
                    crate::bundle::Value::String(Some(ACCESS_TOKEN.into())),
                ),
            ],
        );
        let extras = (|| {
            RawBundle::read_from(&mut token.reader())?
                .with_parcelable("account", "android.accounts.Account", &value)?
                .with_parcelable(
                    "response",
                    "android.accounts.AccountAuthenticatorResponse",
                    &response_value,
                )
        })()
        .map_err(|_| Exception::illegal_argument("consent extras"))?;
        let intent = RawIntent::of(None, 0, &[])
            .with_component(
                "android",
                "android.accounts.GrantCredentialsPermissionActivity",
            )
            .with_category(&format!("{}{package}", notification_tag(account, uid)))
            .and_then(|intent| intent.with_extras_bundle(&extras))
            .map_err(|_| Exception::illegal_argument("consent intent"))?;
        let state = consent.state.lock().unwrap();
        if !state.closed {
            if let Some(target) = &state.notification {
                consent.notify(target, Some(intent.clone()))?;
            }
        }
        Ok(Some(intent))
    }

    pub(super) fn pending_consents(&self) -> Vec<Arc<Consent>> {
        let mut requests = self.consents.lock().unwrap();
        requests.retain(|_, r| r.strong_count() != 0);
        requests.values().filter_map(Weak::upgrade).collect()
    }

    pub(super) fn prune_consents(&self, removed_package: Option<&str>) -> Result<(), Exception> {
        for request in self.pending_consents() {
            let owner = self
                .users
                .lock()
                .unwrap()
                .get(&user_of(request.uid))
                .cloned();
            let live = match owner {
                Some(o) => {
                    request.matches(&request.account, request.uid, &o.state.lock().unwrap())?
                }
                None => false,
            };
            if !live
                || removed_package == Some(request.package.as_str())
                || !request.installed(self)?
            {
                request.finish(false, true)?;
            }
        }
        Ok(())
    }

    pub(super) fn access_intent_sender(
        &self,
        account: &Account,
        package: &str,
        user: i32,
    ) -> Result<Option<Binder>, Exception> {
        let Some(intent) =
            self.prepare_access_consent(account, package, user, None, false, None)?
        else {
            return Ok(None);
        };
        self.host.get_intent_sender(
            SYSTEM_UID,
            super::super::pending_intents::Request {
                kind: super::super::pending_intents::INTENT_SENDER_ACTIVITY,
                package: Some("android".into()),
                feature: None,
                token: None,
                result_who: None,
                request_code: 0,
                intents: Some(vec![Some(intent)]),
                resolved_types: None,
                flags: (1 << 30) | (1 << 28) | (1 << 26),
                options: None,
                user,
            },
        )
    }
}
