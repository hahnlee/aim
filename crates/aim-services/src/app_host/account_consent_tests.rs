//! Checks for the retained Android 16 consent Activity protocol.
use super::super::kept::RawBundle;
use super::consent::{ACCESS_TOKEN, Consent};
use super::validation_tests::{count, fixture};
use super::*;
use crate::bundle::Value;
use aim_binder_host::parcel::EX_SECURITY;
use aim_service_aidl::android_accounts_iaccountauthenticatorresponse as response;
use aim_service_aidl::android_os_iremotecallback as callback;
use std::sync::mpsc;
use std::time::Duration;

struct ResultProbe(mpsc::Sender<bool>);
impl Service for ResultProbe {
    fn descriptor(&self) -> &str {
        callback::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert!(call.is_oneway());
        assert_eq!(call.code, callback::SEND_RESULT);
        let a = callback::SendResult::<RawBundle>::read(&mut call.data)?;
        self.0
            .send(a.data.unwrap().boolean("booleanResult", false)?)
            .unwrap();
        Ok(Parcel::new())
    }
}

fn request(manager: &AccountManager, callback: Strong) -> (Account, Arc<Consent>) {
    let account = Account::new("c".into(), "replaced".into());
    let intent = manager
        .prepare_access_consent(&account, "q", 0, Some(callback), true, None)
        .unwrap()
        .unwrap();
    decode(manager, intent)
}

fn decode(manager: &AccountManager, intent: RawIntent) -> (Account, Arc<Consent>) {
    assert_eq!(
        intent.intent.component.as_ref().unwrap().class,
        "android.accounts.GrantCredentialsPermissionActivity"
    );
    assert_eq!(intent.intent.flags, 0);
    let values = intent.test_extras();
    assert_eq!(
        values["authTokenType"],
        Value::String(Some(ACCESS_TOKEN.into()))
    );
    assert_eq!(values["uid"], Value::Int(10101));
    let p = intent.parcel();
    let Value::Lazy { start, .. } = values["account"] else {
        panic!()
    };
    let mut r = p.reader();
    r.set_position(start);
    assert_eq!(
        r.read_string16().unwrap().as_deref(),
        Some("android.accounts.Account")
    );
    let account = Account::read_from(&mut r).unwrap();
    let Value::Lazy { start, .. } = values["response"] else {
        panic!()
    };
    r.set_position(start);
    assert_eq!(
        r.read_string16().unwrap().as_deref(),
        Some("android.accounts.AccountAuthenticatorResponse")
    );
    assert!(r.read_binder().unwrap().is_some());
    let request = manager.consents.lock().unwrap()[account.access_id.as_ref().unwrap()]
        .upgrade()
        .unwrap();
    (account, request)
}

fn complete(request: &Consent, allow: bool) {
    let mut p = Parcel::new();
    let code = if allow {
        response::OnResult {
            value: Some(RawBundle::empty()),
        }
        .write(&mut p);
        response::ON_RESULT
    } else {
        response::OnError {
            error_code: 4,
            error_message: Some("canceled".into()),
        }
        .write(&mut p);
        response::ON_ERROR
    };
    let mut call = Call {
        code,
        flags: 1,
        sender_pid: 1,
        sender_euid: 1000,
        data: p.reader(),
    };
    request.transact(&mut call).unwrap();
}

#[test]
fn consent_rejects_changed_account_or_uid_and_does_not_prompt_hidden_accounts() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let (manager, de, _) = fixture(host);
        let (send, receive) = mpsc::channel();
        let callback = export(Arc::new(ResultProbe(send)));
        let (account, pending) = request(&manager, host.process.strong(callback.handle));
        for uid in [10100, 10999] {
            assert!(
                manager
                    .update_app_permission(&account, ACCESS_TOKEN, uid, true)
                    .is_err()
            );
        }
        let mut wrong = account.clone();
        wrong.name = "b".into();
        wrong.kind = "kept".into();
        assert!(
            manager
                .update_app_permission(&wrong, ACCESS_TOKEN, 10101, true)
                .is_err()
        );
        complete(&pending, false);
        assert!(!receive.recv_timeout(Duration::from_secs(20)).unwrap());
        assert_eq!(count(&de, "SELECT count(*) FROM grants WHERE uid=10101"), 0);
        let owner = manager.accounts(0).unwrap();
        owner
            .state
            .lock()
            .unwrap()
            .visibility
            .entry(account.clone())
            .or_default()
            .insert("q".into(), VISIBILITY_NOT_VISIBLE);
        assert!(
            manager
                .prepare_access_consent(&account, "q", 0, Some(callback), true, None)
                .unwrap()
                .is_none()
        );
        assert!(receive.try_recv().is_err());
        assert!(
            manager
                .prepare_access_consent(&account, "missing", 0, None, false, None)
                .unwrap()
                .is_none()
        );
    });
}

use aim_service_aidl::{
    dev_aim_server_iaccountconsentnotification as notice, dev_aim_server_isyncaccounts as sync,
};
struct NotificationProbe(mpsc::Sender<Option<RawIntent>>);
impl Service for NotificationProbe {
    fn descriptor(&self) -> &str {
        notice::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert!(call.is_oneway());
        let a = notice::Update::<RawIntent>::read(&mut call.data)?;
        assert_eq!(a.uid, 10101);
        assert!(a.tag.unwrap().starts_with("AccountManagerService:38:"));
        let _ = self.0.send(a.intent);
        Ok(Parcel::new())
    }
}
fn internal(
    manager: &Arc<AccountManager>,
    callback: &Strong,
    notification: &Strong,
    uid: u32,
) -> Result<(), Exception> {
    let mut p = Parcel::new();
    sync::RequestAccountAccess {
        account: Some(Account::new("c".into(), "replaced".into())),
        package_name: Some("q".into()),
        user_id: 0,
        callback: Some(callback.binder()),
        notification: Some(notification.binder()),
    }
    .write(&mut p);
    let p = SyncAccounts(manager.clone())
        .transact(&mut Call {
            code: sync::REQUEST_ACCOUNT_ACCESS,
            flags: 0,
            sender_pid: 1,
            sender_euid: uid,
            data: p.reader(),
        })
        .unwrap();
    sync::read_request_account_access_reply(&mut p.reader()).unwrap()
}
fn next(rx: &mpsc::Receiver<Option<RawIntent>>) -> Option<RawIntent> {
    rx.recv_timeout(Duration::from_secs(20)).unwrap()
}

#[test]
fn internal_consent_notification_lifetimes_and_live_write_validation() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let (manager, de, _) = super::mutation_tests::signed_fixture(host);
        let (tx, results) = mpsc::channel();
        let callback = export(Arc::new(ResultProbe(tx)));
        let (tx, notifications) = mpsc::channel();
        let notification = export(Arc::new(NotificationProbe(tx)));
        assert_eq!(
            internal(&manager, &callback, &notification, 10101)
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        for operation in [Some(true), Some(false), None] {
            internal(&manager, &callback, &notification, 1000).unwrap();
            let (account, pending) = decode(&manager, next(&notifications).unwrap());
            let owner = manager.accounts(0).unwrap();
            manager
                .validate(&owner, &mut owner.state.lock().unwrap(), 0)
                .map_err(Failure::exception)
                .unwrap();
            if let Some(grant) = operation {
                manager
                    .update_app_permission(&account, ACCESS_TOKEN, 10101, grant)
                    .unwrap();
                assert!(next(&notifications).is_none());
            }
            complete(&pending, operation == Some(true));
            complete(&pending, true);
            complete(&pending, false);
            if operation.is_none() {
                assert!(next(&notifications).is_none());
            }
            assert_eq!(
                results.recv_timeout(Duration::from_secs(20)).unwrap(),
                operation == Some(true)
            );
            if operation == Some(true) {
                internal(&manager, &callback, &notification, 1000).unwrap();
                assert!(results.recv_timeout(Duration::from_secs(20)).unwrap());
                manager
                    .update_app_permission(
                        &Account::new("c".into(), "replaced".into()),
                        ACCESS_TOKEN,
                        10101,
                        false,
                    )
                    .unwrap();
            }
        }
        internal(&manager, &callback, &notification, 1000).unwrap();
        let (stale, old) = decode(&manager, next(&notifications).unwrap());
        internal(&manager, &callback, &notification, 1000).unwrap();
        assert!(next(&notifications).is_none());
        let (_, replacement) = decode(&manager, next(&notifications).unwrap());
        assert!(!results.recv_timeout(Duration::from_secs(20)).unwrap());
        assert!(
            manager
                .update_app_permission(&stale, ACCESS_TOKEN, 10101, true)
                .is_err()
        );
        complete(&old, true);
        complete(&replacement, false);
        assert!(next(&notifications).is_none());
        assert!(!results.recv_timeout(Duration::from_secs(20)).unwrap());
        for removal in [false, true] {
            internal(&manager, &callback, &notification, 1000).unwrap();
            let (account, pending) = decode(&manager, next(&notifications).unwrap());
            if removal {
                host.set_user_unlocked_for_package_test(0, true);
                assert!(
                    manager
                        .explicit_remove(10100, Some(account.clone()))
                        .unwrap()
                );
                assert!(
                    manager
                        .explicit_add(
                            10100,
                            Account::new("c".into(), "replaced".into()),
                            None,
                            HashMap::new(),
                            HashMap::new()
                        )
                        .unwrap()
                );
            } else {
                manager
                    .update_app_permission(
                        &Account::new("c".into(), "replaced".into()),
                        ACCESS_TOKEN,
                        10101,
                        false,
                    )
                    .unwrap();
            }
            assert!(next(&notifications).is_none());
            assert!(!results.recv_timeout(Duration::from_secs(20)).unwrap());
            assert!(
                manager
                    .update_app_permission(&account, ACCESS_TOKEN, 10101, true)
                    .is_err()
            );
            complete(&pending, true);
        }
        internal(&manager, &callback, &notification, 1000).unwrap();
        let (stale, pending) = decode(&manager, next(&notifications).unwrap());
        manager
            .update_app_permission(
                &Account::new("c".into(), "replaced".into()),
                ACCESS_TOKEN,
                10101,
                true,
            )
            .unwrap();
        let base = host.package_capture().unwrap();
        let mut owner = base.scan().owner().clone();
        let mut target = owner.scanned_user_states("q").unwrap()[&0].clone();
        target.installed = false;
        owner.set_user_state("q", 0, target).unwrap();
        crate::system::complete_discovery_owner(&mut owner);
        let mut context = (**base.context()).clone();
        context.scan_version = base.scan().version() + 1;
        host.package_system()
            .unwrap()
            .publish_package_scan_with_queries(
                &host.package_system().unwrap().package_bootstrap().unwrap(),
                Some(base.scan()),
                owner,
                base.scan().usage().clone(),
                context,
            )
            .unwrap();
        // The write itself rejects disappearance, before monitor delivery.
        assert!(
            manager
                .update_app_permission(&stale, ACCESS_TOKEN, 10101, true)
                .is_err()
        );
        super::super::package_writes::PackageMonitor::package_changed(
            manager.as_ref(),
            &super::super::package_writes::PackageChanged {
                package: "q",
                user: 0,
                components: &[],
                removed: true,
            },
        )
        .unwrap();
        assert!(next(&notifications).is_none());
        assert!(!results.recv_timeout(Duration::from_secs(20)).unwrap());
        complete(&pending, true);
        assert!(results.try_recv().is_err());
        assert!(notifications.try_recv().is_err());
        assert_eq!(count(&de, "SELECT count(*) FROM grants WHERE uid=10101"), 0);
    });
}

#[test]
fn consent_callback_process_death_closes_the_write_capability() {
    crate::system::with_discovery_binders_lifecycle(&[], true, |host, _, _, export, die| {
        let (manager, de, _) = fixture(host);
        let (tx, _rx) = mpsc::channel();
        let callback = export(Arc::new(ResultProbe(tx)));
        let (account, pending) = request(&manager, callback);
        die();
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !pending.state.lock().unwrap().closed {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(
            manager
                .update_app_permission(&account, ACCESS_TOKEN, 10101, true)
                .is_err()
        );
        complete(&pending, true);
        assert_eq!(count(&de, "SELECT count(*) FROM grants WHERE uid=10101"), 0);
    });
}

#[test]
fn public_consent_factory_guards_nulls_users_packages_and_missing_accounts() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        let (manager, _, _) = fixture(host);
        use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT as IA, EX_NULL_POINTER as NP};
        for (caller, present, package, user, error) in [
            (10101, true, Some("q"), Some(0), Some(EX_SECURITY)),
            (1000, false, Some("q"), Some(0), Some(NP)),
            (1000, true, None, Some(0), Some(NP)),
            (1000, true, Some("q"), None, Some(NP)),
            (1000, true, Some("q"), Some(-1), Some(IA)),
            (1000, true, Some("absent"), Some(0), None),
            (1000, true, Some("q"), Some(0), None),
        ] {
            let mut p = Parcel::new();
            am::CreateRequestAccountAccessIntentSenderAsUser {
                account: present.then(|| Account::new("missing".into(), "type".into())),
                package_name: package.map(str::to_string),
                user_handle: user.map(UserHandle),
            }
            .write(&mut p);
            let p = manager
                .transact(&mut Call {
                    code: am::CREATE_REQUEST_ACCOUNT_ACCESS_INTENT_SENDER_AS_USER,
                    flags: 0,
                    sender_pid: 1,
                    sender_euid: caller,
                    data: p.reader(),
                })
                .unwrap();
            let mut r = p.reader();
            match r.read_exception().unwrap() {
                Err(e) => assert_eq!(Some(e.code), error),
                Ok(()) => {
                    assert_eq!(error, None);
                    assert_eq!(r.read_i32().unwrap(), i32::from(package == Some("q")));
                }
            }
        }
    });
}
