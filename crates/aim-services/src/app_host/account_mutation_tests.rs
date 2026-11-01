//! android-16.0.0_r1 AccountManagerTest: testAddAccountExplicitlyAndRemoveAccountExplicitly,
//! testAddAccountExplicitlyWithVisibility, testSetAccountVisibility and
//! testSetAccountVisibilityForPrivilegedPackage (CTS tests/tests/accounts).
use super::super::kept::RawBundle;
use super::validation_tests::{count, fixture, register_probe};
use super::*;
use aim_service_aidl::android_app_iapplicationthread as thread;
use rusqlite::Connection;
use std::sync::mpsc;
use std::time::Duration;

fn signed_fixture(
    host: &Arc<AppHost>,
) -> (Arc<AccountManager>, std::path::PathBuf, std::path::PathBuf) {
    let system = host.package_system().unwrap();
    let base = host.package_capture().unwrap();
    let mut owner = base.scan().owner().clone();
    // Real published signing identities: p owns the authenticator, q is unrelated.
    for setting in &mut owner.settings.packages {
        let certificate: &[u8] = if setting.name == "p" {
            include_bytes!("../../tests/fixtures/settings-certificate-1.der")
        } else {
            include_bytes!("../../tests/fixtures/settings-certificate-2.der")
        };
        setting.signatures = Some(Signatures {
            signatures: vec![certificate.to_vec()],
            ..Default::default()
        });
    }
    crate::system::complete_discovery_owner(&mut owner);
    let mut context = (**base.context()).clone();
    context.scan_version = base.scan().version() + 1;
    system
        .publish_package_scan_with_queries(
            &system.package_bootstrap().unwrap(),
            Some(base.scan()),
            owner,
            base.scan().usage().clone(),
            context,
        )
        .unwrap();
    fixture(host)
}

fn invoke(manager: &AccountManager, uid: u32, code: u32, p: &Parcel) -> Result<bool, Exception> {
    let p = manager
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: uid as i32,
            sender_euid: uid,
            data: p.reader(),
        })
        .unwrap();
    am::read_add_account_explicitly_reply(&mut p.reader()).unwrap()
}

fn add(
    manager: &AccountManager,
    uid: u32,
    account: &Account,
    with_visibility: bool,
) -> Result<bool, Exception> {
    let mut p = Parcel::new();
    let extras = super::super::bind::Bundle(vec![
        (
            "key".into(),
            crate::bundle::Value::String(Some("value".into())),
        ),
        ("number".into(), crate::bundle::Value::Int(7)),
    ]);
    let code = if with_visibility {
        am::AddAccountExplicitlyWithVisibility {
            account: Some(account.clone()),
            password: Some("secret".into()),
            extras: Some(extras),
            visibility: Some(
                ["q", "missing"]
                    .map(|package| {
                        (
                            ParcelValue::String(Some(package.into())),
                            ParcelValue::Integer(1),
                        )
                    })
                    .into(),
            ),
            op_package_name: Some("p".into()),
        }
        .write(&mut p);
        am::ADD_ACCOUNT_EXPLICITLY_WITH_VISIBILITY
    } else {
        am::AddAccountExplicitly {
            account: Some(account.clone()),
            password: None,
            extras: Some(extras),
            op_package_name: Some("p".into()),
        }
        .write(&mut p);
        am::ADD_ACCOUNT_EXPLICITLY
    };
    invoke(manager, uid, code, &p)
}

fn visibility(
    manager: &AccountManager,
    uid: u32,
    account: &Account,
    package: &str,
    value: i32,
) -> Result<bool, Exception> {
    let mut p = Parcel::new();
    am::SetAccountVisibility {
        a: Some(account.clone()),
        package_name: Some(package.into()),
        new_visibility: value,
    }
    .write(&mut p);
    invoke(manager, uid, am::SET_ACCOUNT_VISIBILITY, &p)
}

fn remove(manager: &AccountManager, uid: u32, account: Option<Account>) -> Result<bool, Exception> {
    let mut p = Parcel::new();
    am::RemoveAccountExplicitly { account }.write(&mut p);
    invoke(manager, uid, am::REMOVE_ACCOUNT_EXPLICITLY, &p)
}

struct Probe(
    Arc<AccountManager>,
    Account,
    Arc<Mutex<(bool, i32)>>,
    mpsc::Sender<String>,
);
impl Service for Probe {
    fn descriptor(&self) -> &str {
        thread::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let a = thread::ScheduleRegisteredReceiver::<RawIntent, RawBundle>::read(&mut call.data)?;
        assert_eq!(a.sending_user, 0);
        assert!(self.0.users.try_lock().is_ok());
        let owner = self.0.accounts(0).unwrap();
        assert!(owner.receivers.try_lock().is_ok());
        let state = owner.state.try_lock().unwrap();
        let (exists, visibility) = *self.2.lock().unwrap();
        assert_eq!(state.exists(&self.1), exists);
        assert_eq!(
            state
                .visibility
                .get(&self.1)
                .and_then(|v| v.get("q"))
                .copied()
                .unwrap_or(0),
            visibility
        );
        assert_eq!(state.db.account_id(&self.1).unwrap().is_some(), exists);
        if !exists {
            assert!(!state.user_data.contains_key(&self.1));
            assert!(!state.auth_tokens.contains_key(&self.1));
            assert!(!state.previous_names.contains_key(&self.1));
            assert_eq!(
                state
                    .db
                    .find_matching_grants_count_any_token(10100, &self.1)
                    .unwrap(),
                0
            );
        }
        self.3
            .send(a.intent.unwrap().intent.action.unwrap())
            .unwrap();
        Ok(Parcel::new())
    }
}

#[test]
fn explicit_account_setup_teardown_commit_rollback_and_locked_user() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let (manager, de, ce) = signed_fixture(host);
        let owner = manager.accounts(0).unwrap();
        let account = Account::new("setup".into(), "replaced".into());
        assert!(!add(&manager, 10100, &account, true).unwrap());
        for uid in [1000, 2000, 10101] {
            assert_eq!(
                add(&manager, uid, &account, false).unwrap_err().code,
                aim_binder_host::parcel::EX_SECURITY
            );
            assert!(remove(&manager, uid, Some(account.clone())).is_err());
        }
        assert!(!remove(&manager, 10100, None).unwrap());
        assert!(visibility(&manager, 10101, &account, "q", 1).is_err());
        host.set_user_unlocked_for_package_test(0, true);
        manager.accounts(0).unwrap();
        let expected = Arc::new(Mutex::new((true, 1)));
        let (send, receive) = mpsc::channel();
        register_probe(
            host,
            export,
            Arc::new(Probe(
                manager.clone(),
                account.clone(),
                expected.clone(),
                send,
            )),
        );
        owner
            .receivers
            .lock()
            .unwrap()
            .insert(None, HashMap::from([("p".into(), 1)]));
        let transitions = |n| {
            host.broadcasts.wait(true);
            let actions: Vec<_> = (0..n)
                .map(|_| receive.recv_timeout(Duration::from_secs(20)).unwrap())
                .collect();
            assert_eq!(
                actions
                    .iter()
                    .filter(|a| a.as_str() == "android.accounts.LOGIN_ACCOUNTS_CHANGED")
                    .count(),
                1
            );
            assert!(receive.try_recv().is_err());
        };
        let rows = |path| count(path, "SELECT count(*) FROM accounts WHERE name='setup'");
        let sql = Connection::open(&de).unwrap();
        sql.execute_batch("CREATE TRIGGER deny_add BEFORE INSERT ON visibility BEGIN SELECT RAISE(ABORT,'denied'); END;").unwrap();
        assert!(add(&manager, 10100, &account, true).is_err());
        assert!(!owner.state.lock().unwrap().exists(&account));
        assert_eq!(rows(&de), 0);
        assert_eq!(rows(&ce), 0);
        let mut reopened = AccountsDb::open(&de, &de.with_file_name("absent"), "en_US").unwrap();
        reopened.attach_ce(&ce).unwrap();
        assert!(reopened.account_id(&account).unwrap().is_none());
        assert!(
            reopened
                .find_user_extras_for_account(&account)
                .unwrap()
                .is_empty()
        );
        assert!(
            !reopened
                .find_all_visibility_values()
                .unwrap()
                .contains_key(&account)
        );
        sql.execute_batch("DROP TRIGGER deny_add").unwrap();
        assert!(add(&manager, 10100, &account, true).unwrap());
        transitions(2);
        assert!(!add(&manager, 10100, &account, true).unwrap());
        assert_eq!(rows(&de), 1);
        assert_eq!(
            count(&de, "SELECT _id FROM accounts WHERE name='setup'"),
            count(&ce, "SELECT _id FROM accounts WHERE name='setup'")
        );
        {
            let state = owner.state.lock().unwrap();
            assert_eq!(state.visibility[&account], HashMap::from([("q".into(), 1)]));
            assert_eq!(
                state
                    .db
                    .find_account_password_by_name_and_type("setup", "replaced")
                    .unwrap()
                    .as_deref(),
                Some("secret")
            );
            let extras = state.db.find_user_extras_for_account(&account).unwrap();
            assert_eq!(extras["key"].as_deref(), Some("value"));
            assert_eq!(extras["number"], None);
        }
        // CTS: authenticator visibility remains VISIBLE despite its stored entry.
        assert!(visibility(&manager, 10100, &account, "p", 3).unwrap());
        transitions(1);
        for package in ["p", "q"] {
            assert_eq!(
                manager
                    .resolve_visibility(&owner, &account, package, 0)
                    .unwrap(),
                1
            );
        }
        assert!(!visibility(&manager, 10100, &account, "missing", 1).unwrap());
        sql.execute_batch("CREATE TRIGGER deny_visibility BEFORE INSERT ON visibility BEGIN SELECT RAISE(ABORT,'denied'); END;").unwrap();
        assert!(visibility(&manager, 10100, &account, "q", 3).is_err());
        assert_eq!(owner.state.lock().unwrap().visibility[&account]["q"], 1);
        assert_eq!(
            reopened.find_all_visibility_values().unwrap()[&account]["q"],
            1
        );
        sql.execute_batch("DROP TRIGGER deny_visibility").unwrap();
        *expected.lock().unwrap() = (true, 3);
        assert!(visibility(&manager, 1000, &account, "q", 3).unwrap());
        transitions(1);
        assert!(visibility(&manager, 1000, &account, "q", 3).unwrap());
        sql.execute_batch(
            "INSERT INTO grants SELECT _id,'access',10100 FROM accounts WHERE name='setup'",
        )
        .unwrap();
        Connection::open(&ce).unwrap().execute_batch(
            "INSERT INTO authtokens (accounts_id,type,authtoken) SELECT _id,'token','secret' FROM accounts WHERE name='setup'"
        ).unwrap();
        {
            let mut state = owner.state.lock().unwrap();
            state.user_data.insert(account.clone(), HashMap::new());
            state.auth_tokens.insert(account.clone(), HashMap::new());
            state
                .previous_names
                .insert(account.clone(), Some("old".into()));
        }
        Connection::open(&ce).unwrap().execute_batch("CREATE TRIGGER deny_remove BEFORE DELETE ON accounts WHEN OLD.name='setup' BEGIN SELECT RAISE(ABORT,'denied'); END;").unwrap();
        assert!(remove(&manager, 10100, Some(account.clone())).is_err());
        assert!(owner.state.lock().unwrap().exists(&account));
        assert_eq!(rows(&de), 1);
        assert_eq!(rows(&ce), 1);
        assert_eq!(
            reopened
                .find_auth_tokens_by_account(&account)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            reopened
                .find_matching_grants_count_any_token(10100, &account)
                .unwrap(),
            1
        );
        Connection::open(&ce)
            .unwrap()
            .execute_batch("DROP TRIGGER deny_remove")
            .unwrap();
        *expected.lock().unwrap() = (false, 0);
        assert!(remove(&manager, 10100, Some(account.clone())).unwrap());
        transitions(2);
        assert!(!remove(&manager, 10100, Some(account.clone())).unwrap());
        assert!(
            reopened
                .find_user_extras_for_account(&account)
                .unwrap()
                .is_empty()
        );
        assert!(
            !reopened
                .find_all_visibility_values()
                .unwrap()
                .contains_key(&account)
        );
        assert_eq!(rows(&ce), 0);
        assert!(
            reopened
                .find_auth_tokens_by_account(&account)
                .unwrap()
                .is_empty()
        );
        // The plain add route shares identity and delivery. Removal while locked
        // preserves CE secrets until unlock reconciliation, even after CE was attached.
        *expected.lock().unwrap() = (true, 0);
        assert!(add(&manager, 10100, &account, false).unwrap());
        transitions(2);
        host.set_user_unlocked_for_package_test(0, false);
        *expected.lock().unwrap() = (false, 0);
        assert!(remove(&manager, 10100, Some(account.clone())).unwrap());
        transitions(2);
        assert_eq!(rows(&ce), 1);
        host.set_user_unlocked_for_package_test(0, true);
        manager.accounts(0).unwrap();
        assert_eq!(rows(&ce), 0);
        host.broadcasts.wait(true);
        assert!(receive.try_recv().is_err());
    });
}

#[test]
fn cts_explicit_setup_counts_visibility_defaults_and_teardown() {
    crate::system::with_discovery_session(|host, _, _| {
        let (manager, de, ce) = signed_fixture(host);
        host.set_user_unlocked_for_package_test(0, true);
        let owner = manager.accounts(0).unwrap();
        let account = Account::new("cts".into(), "replaced".into());
        let visible = || {
            manager
                .accounts_from_cache(&owner, 0, None, 10100, Some("p"), false)
                .unwrap()
        };
        let before = visible();
        assert!(!visibility(&manager, 10100, &account, "q", 1).unwrap());
        let long = Account::new("😀".repeat(101), "replaced".into());
        assert!(!add(&manager, 10100, &long, false).unwrap());
        assert!(add(&manager, 10100, &account, false).unwrap());
        assert_eq!(visible().len(), before.len() + 1);
        assert!(visible().contains(&account));
        let get = |package: &str| {
            let mut p = Parcel::new();
            am::GetAccountVisibility {
                a: Some(account.clone()),
                package_name: Some(package.into()),
            }
            .write(&mut p);
            let p = manager
                .transact(&mut Call {
                    code: am::GET_ACCOUNT_VISIBILITY,
                    flags: 0,
                    sender_pid: 10100,
                    sender_euid: 10100,
                    data: p.reader(),
                })
                .unwrap();
            am::read_get_account_visibility_reply(&mut p.reader())
                .unwrap()
                .unwrap()
        };
        assert_eq!(get("q"), VISIBILITY_USER_MANAGED_NOT_VISIBLE);
        assert_eq!(get(LEGACY_VISIBLE), VISIBILITY_USER_MANAGED_VISIBLE);
        assert_eq!(get(LEGACY_NOT_VISIBLE), VISIBILITY_USER_MANAGED_NOT_VISIBLE);
        assert!(visibility(&manager, 10100, &account, LEGACY_NOT_VISIBLE, 2).unwrap());
        assert_eq!(get("q"), 2);
        assert!(visibility(&manager, 10100, &account, "q", 3).unwrap());
        assert_eq!(get("q"), 3);
        assert!(visibility(&manager, 10100, &account, "p", 3).unwrap());
        assert_eq!(get("p"), 1);
        assert_eq!(get("q"), 3);
        // A concrete visibility change notifies its package even without a subscription.
        let notices = manager
            .mutation_notices(
                &owner,
                &owner.state.lock().unwrap(),
                &account,
                0,
                Some((&HashMap::from([("q".into(), 1)]), Some("q"))),
            )
            .unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].intent.package.as_deref(), Some("q"));
        assert_eq!(
            notices[0].intent.action.as_deref(),
            Some("android.accounts.action.VISIBLE_ACCOUNTS_CHANGED")
        );
        assert!(remove(&manager, 10100, Some(account.clone())).unwrap());
        assert_eq!(visible(), before);
        assert_eq!(get("q"), VISIBILITY_NOT_VISIBLE);
        let mut reopened = AccountsDb::open(&de, &de.with_file_name("absent"), "en_US").unwrap();
        reopened.attach_ce(&ce).unwrap();
        assert!(reopened.account_id(&account).unwrap().is_none());
        assert!(
            !reopened
                .find_all_visibility_values()
                .unwrap()
                .contains_key(&account)
        );
        let mut p = Parcel::new();
        am::AddAccountExplicitly::<Account, super::super::bind::Bundle> {
            account: None,
            password: None,
            extras: None,
            op_package_name: Some("p".into()),
        }
        .write(&mut p);
        assert_eq!(
            invoke(&manager, 10100, am::ADD_ACCOUNT_EXPLICITLY, &p)
                .unwrap_err()
                .code,
            aim_binder_host::parcel::EX_NULL_POINTER
        );
    });
}
