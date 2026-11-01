//! AccountManagerService.validateAccountsInternal / syncDeCeAccountsLocked,
//! android-16.0.0_r1: persistence, update protection and post-commit delivery.
use super::super::{broadcasts, kept::RawBundle};
use super::*;
use aim_binder_host::local::Strong;
use aim_service_aidl::android_app_iapplicationthread as thread;
use rusqlite::Connection;
use std::sync::mpsc;
use std::time::Duration;

fn count(path: &std::path::Path, sql: &str) -> i32 {
    Connection::open(path)
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}

fn authenticator(kind: &str, uid: i32) -> Authenticator {
    Authenticator {
        description: AuthenticatorDescription {
            kind: kind.into(),
            package: "p".into(),
            label_id: 0,
            icon_id: 0,
            small_icon_id: 0,
            account_preferences_id: 0,
            custom_tokens: false,
        },
        uid,
        class: "p.Authenticator".into(),
    }
}

fn fixture(host: &Arc<AppHost>) -> (Arc<AccountManager>, std::path::PathBuf, std::path::PathBuf) {
    host.set_user_unlocked_for_package_test(0, false);
    let data = &host.platform().unwrap().data;
    let de = data.join("system_de/0/accounts_de.db");
    let ce = data.join("system_ce/0/accounts_ce.db");
    for path in [&de, &ce] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    }
    let mut db = AccountsDb::open(&de, &data.join("absent.db"), "en_US").unwrap();
    db.attach_ce(&ce).unwrap();
    drop(db);
    for path in [&de, &ce] {
        Connection::open(path).unwrap().execute_batch(
            "INSERT INTO accounts (_id,name,type) VALUES (1,'a','gone'),(2,'b','kept'),(3,'c','replaced');"
        ).unwrap();
    }
    Connection::open(&de).unwrap().execute_batch(
        "INSERT INTO meta VALUES ('auth_uid_for_type:gone','99999'),('auth_uid_for_type:kept','10100'),('auth_uid_for_type:replaced','10100');
         INSERT INTO visibility VALUES (1,'p',1),(1,'q',3);
         INSERT INTO grants VALUES (1,'access',10100),(3,'access',10100);"
    ).unwrap();
    Connection::open(&ce)
        .unwrap()
        .execute_batch(
            "UPDATE accounts SET password='secret';
         INSERT INTO accounts (_id,name,type,password) VALUES (4,'orphan','gone','secret');
         INSERT INTO extras (accounts_id,key,value) VALUES (1,'key','value');
         INSERT INTO authtokens (accounts_id,type,authtoken) VALUES (1,'token','secret');",
        )
        .unwrap();
    let manager = AccountManager::new(host.clone());
    manager.authenticators.lock().unwrap().insert(
        0,
        Arc::new(vec![
            authenticator("gone", 99999),
            authenticator("replaced", 10100),
        ]),
    );
    (manager, de, ce)
}

struct Probe(Arc<AccountManager>, mpsc::Sender<(RawIntent, i32)>);
impl Service for Probe {
    fn descriptor(&self) -> &str {
        thread::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.code, thread::SCHEDULE_REGISTERED_RECEIVER);
        let a = thread::ScheduleRegisteredReceiver::<RawIntent, RawBundle>::read(&mut call.data)?;
        // Reenter the owner and inspect the committed generation during delivery.
        assert!(self.0.users.try_lock().is_ok());
        let owner = self.0.accounts(0).unwrap();
        let state = owner.state.try_lock().unwrap();
        let gone = Account::new("a".into(), "gone".into());
        assert!(!state.exists(&gone));
        assert!(!state.user_data.contains_key(&gone));
        assert!(!state.auth_tokens.contains_key(&gone));
        assert!(!state.visibility.contains_key(&gone));
        assert_eq!(
            state
                .db
                .find_matching_grants_count_any_token(10100, &gone)
                .unwrap(),
            0
        );
        drop(state);
        assert!(!self.0.has_account_access(&gone, Some("p"), 1000).unwrap());
        self.1.send((a.intent.unwrap(), a.sending_user)).unwrap();
        Ok(Parcel::new())
    }
}

fn listen(
    host: &Arc<AppHost>,
    export: &dyn Fn(Arc<dyn Service>) -> Strong,
    manager: Arc<AccountManager>,
) -> mpsc::Receiver<(RawIntent, i32)> {
    // Reuse the real loaded-process fixture, then register an all-user receiver.
    let _unused =
        broadcasts::internal_package_tests::register_filter(host, export, "p", 1000, false);
    let (send, receive) = mpsc::channel();
    let probe = export(Arc::new(Probe(manager, send)));
    let binder = probe.binder();
    host.apps.lock().unwrap().get_mut(&1000).unwrap().thread = Some(probe);
    let mut filter = crate::package::intent_filter::IntentFilter::default();
    filter.add_action("android.accounts.LOGIN_ACCOUNTS_CHANGED");
    filter.add_action("android.accounts.action.VISIBLE_ACCOUNTS_CHANGED");
    let data = Parcel::new();
    host.broadcasts
        .register(
            host,
            &Call {
                code: 0,
                flags: 0,
                sender_pid: 1000,
                sender_euid: 1000,
                data: data.reader(),
            },
            broadcasts::Registration {
                caller: Some(binder),
                caller_package: Some("p".into()),
                feature_id: None,
                receiver_id: None,
                receiver: Some(binder),
                filter: Some({
                    let mut p = Parcel::new();
                    filter.write(&mut p);
                    broadcasts::ParcelFilter::read_from(&mut p.reader()).unwrap()
                }),
                permission: None,
                user: super::super::USER_ALL,
                flags: 2,
            },
        )
        .unwrap();
    receive
}

#[test]
fn validation_commits_before_delivery_and_reconciles_cached_unlock() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let (manager, de, ce) = fixture(host);
        let owner = manager.accounts(0).unwrap();
        let before = manager.authenticators(0).unwrap();
        let gone = Account::new("a".into(), "gone".into());
        owner
            .receivers
            .lock()
            .unwrap()
            .insert(None, HashMap::from([("p".into(), 1), ("q".into(), 1)]));
        {
            let mut state = owner.state.lock().unwrap();
            assert!(!state.db.is_ce_attached());
            state.user_data.insert(gone.clone(), HashMap::new());
            state.auth_tokens.insert(gone.clone(), HashMap::new());
            state
                .previous_names
                .insert(gone.clone(), Some("old".into()));
        }
        let receive = listen(host, export, manager.clone());
        let sql = Connection::open(&de).unwrap();
        sql.execute_batch("CREATE TRIGGER deny_delete BEFORE DELETE ON accounts BEGIN SELECT RAISE(ABORT,'denied'); END;").unwrap();
        assert!(manager.validate_accounts(0).is_err());
        assert!(owner.state.lock().unwrap().exists(&gone));
        assert_eq!(
            count(&de, "SELECT count(*) FROM meta WHERE value='99999'"),
            1
        );
        assert!(receive.try_recv().is_err());
        sql.execute_batch("DROP TRIGGER deny_delete").unwrap();
        manager.validate_accounts(0).unwrap();
        assert!(!Arc::ptr_eq(&before, &manager.authenticators(0).unwrap()));
        for action in [
            "android.accounts.action.VISIBLE_ACCOUNTS_CHANGED",
            "android.accounts.LOGIN_ACCOUNTS_CHANGED",
        ] {
            let (intent, sender) = receive.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(intent.intent.action.as_deref(), Some(action));
            assert_eq!(sender, 0);
        }
        assert!(
            !owner
                .state
                .lock()
                .unwrap()
                .previous_names
                .contains_key(&gone)
        );
        assert_eq!(count(&ce, "SELECT count(*) FROM accounts WHERE _id=1"), 1);
        host.set_user_unlocked_for_package_test(0, true);
        let ce_sql = Connection::open(&ce).unwrap();
        ce_sql.execute_batch("CREATE TRIGGER deny_orphan BEFORE DELETE ON accounts WHEN OLD._id=4 BEGIN SELECT RAISE(ABORT,'denied'); END;").unwrap();
        assert!(manager.validate_accounts(0).is_err());
        assert_eq!(count(&ce, "SELECT count(*) FROM accounts WHERE _id=4"), 1);
        ce_sql.execute_batch("DROP TRIGGER deny_orphan").unwrap();
        manager.validate_accounts(0).unwrap();
        assert!(Arc::ptr_eq(&owner, &manager.accounts(0).unwrap()));
        assert!(owner.state.lock().unwrap().db.is_ce_attached());
        manager
            .authenticators
            .lock()
            .unwrap()
            .insert(0, Arc::new(vec![authenticator("replaced", 10101)]));
        assert_eq!(
            manager.accounts(0).unwrap().state.lock().unwrap().of(None),
            [Account::new("b".into(), "kept".into())]
        );
        let (_, sender) = receive.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(sender, 0);
        manager.validate_accounts(0).unwrap();
        host.broadcasts.wait(true);
        assert!(receive.try_recv().is_err());
        let mut reopened = AccountsDb::open(&de, &de.with_file_name("absent"), "en_US").unwrap();
        reopened.attach_ce(&ce).unwrap();
        assert!(reopened.find_ce_accounts_not_in_de().unwrap().is_empty());
        assert!(
            reopened
                .find_account_password_by_name_and_type("a", "gone")
                .unwrap()
                .is_none()
        );
        assert!(
            reopened
                .find_user_extras_for_account(&gone)
                .unwrap()
                .is_empty()
        );
        assert!(
            reopened
                .find_auth_tokens_by_account(&gone)
                .unwrap()
                .is_empty()
        );
        assert!(reopened.find_all_visibility_values().unwrap().is_empty());
        assert!(reopened.find_all_uid_grants().unwrap().is_empty());
        for table in ["extras", "authtokens"] {
            assert_eq!(count(&ce, &format!("SELECT count(*) FROM {table}")), 0);
        }
        assert_eq!(count(&ce, "SELECT count(*) FROM accounts"), 1);
    });
}

#[test]
fn removal_recipients_use_prior_visibility_and_ce_write_failure_rolls_back() {
    crate::system::with_discovery_session(|host, publish, _| {
        publish(&mut |pkg| {
            if ["p", "q"].contains(&pkg.package_name.as_str()) {
                let mut receiver = pkg.activities[0].clone();
                receiver.main.enabled = true;
                receiver.main.exported = true;
                receiver.main.component.intents =
                    vec![crate::package::intent_filter::ParsedIntentInfo {
                        filter: crate::package::intent_filter::IntentFilter {
                            actions: vec!["android.accounts.action.ACCOUNT_REMOVED".into()],
                            ..Default::default()
                        },
                        ..Default::default()
                    }];
                pkg.receivers = vec![receiver];
            }
        });
        let (manager, de, ce) = fixture(host);
        let owner = manager.accounts(0).unwrap();
        let gone = Account::new("a".into(), "gone".into());
        let notices = manager
            .removal_notices(&owner, &owner.state.lock().unwrap(), &gone, 0)
            .unwrap();
        assert_eq!(notices.len(), 1);
        assert_eq!(notices[0].intent.package.as_deref(), Some("p"));
        assert_eq!(
            notices[0].test_extras()["authAccount"],
            crate::bundle::Value::String(Some("a".into()))
        );
        host.set_user_unlocked_for_package_test(0, true);
        manager.accounts(0).unwrap();
        Connection::open(&ce).unwrap().execute_batch("CREATE TRIGGER deny_delete BEFORE DELETE ON accounts WHEN OLD._id=1 BEGIN SELECT RAISE(ABORT,'denied'); END;").unwrap();
        assert!(manager.validate_accounts(0).is_err());
        assert!(owner.state.lock().unwrap().exists(&gone));
        assert_eq!(
            count(&de, "SELECT count(*) FROM grants WHERE accounts_id=1"),
            1
        );
        assert_eq!(count(&ce, "SELECT count(*) FROM accounts WHERE _id=1"), 1);
    });
}
