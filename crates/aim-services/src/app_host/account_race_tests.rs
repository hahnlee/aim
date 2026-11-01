//! SyncManager's register-before-snapshot refresh contract (Android 16).
use super::validation_tests::{count, fixture, listen};
use super::*;
use aim_service_aidl::dev_aim_server_isyncaccounts as sync;
use rusqlite::Connection;
use std::cell::RefCell;
use std::sync::{Barrier, mpsc};
use std::time::Duration;

thread_local! {
    static PAUSE: RefCell<Option<(&'static str, Box<dyn FnOnce()>)>> = RefCell::default();
}

pub(super) fn checkpoint(phase: &str) {
    let pause = PAUSE.with(|p| {
        let mut p = p.borrow_mut();
        if p.as_ref().is_some_and(|(at, _)| *at == phase) {
            p.take()
        } else {
            None
        }
    });
    if let Some((_, pause)) = pause {
        pause();
    }
}

// Bounded rendezvous: a failed assertion cannot strand another worker forever.
fn pause_at(phase: &'static str) -> (mpsc::Receiver<()>, mpsc::Sender<()>, impl FnOnce()) {
    let (entered, waiting) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    (waiting, release, move || {
        PAUSE.with(|p| {
            *p.borrow_mut() = Some((
                phase,
                Box::new(move || {
                    entered.send(()).unwrap();
                    resume.recv_timeout(Duration::from_secs(20)).unwrap();
                }),
            ));
        })
    })
}

pub(super) fn invoke(manager: &Arc<AccountManager>, code: u32) -> Result<Vec<Account>, Exception> {
    let mut p = Parcel::new();
    if code == sync::VALIDATE_ACCOUNTS {
        sync::ValidateAccounts { user_id: 0 }.write(&mut p);
    } else {
        p.write_interface_token(sync::DESCRIPTOR);
    }
    let p = SyncAccounts(manager.clone())
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 1000,
            sender_euid: 1000,
            data: p.reader(),
        })
        .unwrap();
    if code == sync::VALIDATE_ACCOUNTS {
        sync::read_validate_accounts_reply(&mut p.reader()).unwrap()?;
        return Ok(Vec::new());
    }
    Ok(
        sync::read_get_running_accounts_for_system_reply::<Account>(&mut p.reader())
            .unwrap()?
            .unwrap()
            .into_iter()
            .map(Option::unwrap)
            .collect(),
    )
}

fn register(manager: &Arc<AccountManager>) {
    let mut p = Parcel::new();
    am::RegisterAccountListener {
        account_types: None,
        op_package_name: Some("p".into()),
    }
    .write(&mut p);
    manager
        .transact(&mut Call {
            code: am::REGISTER_ACCOUNT_LISTENER,
            flags: 0,
            sender_pid: 10100,
            sender_euid: 10100,
            data: p.reader(),
        })
        .unwrap()
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
}

fn check(manager: &Arc<AccountManager>, removed: bool) {
    for code in [
        sync::GET_RUNNING_ACCOUNTS_FOR_SYSTEM,
        sync::GET_ALL_ACCOUNTS_FOR_SYSTEM_PROCESS,
    ] {
        let found = invoke(manager, code).unwrap();
        let mut names: Vec<_> = found.iter().map(|a| a.name.as_str()).collect();
        names.sort();
        assert_eq!(
            names,
            if removed {
                vec!["b", "c"]
            } else {
                vec!["a", "b", "c"]
            }
        );
    }
}

fn transitions(host: &Arc<AppHost>, receive: &mpsc::Receiver<(RawIntent, i32)>) {
    host.broadcasts.wait(true);
    let mut actions: Vec<_> = (0..2)
        .map(|_| {
            let (i, user) = receive.recv_timeout(Duration::from_secs(20)).unwrap();
            assert_eq!(user, 0);
            i.intent.action.unwrap()
        })
        .collect();
    actions.sort();
    assert_eq!(
        actions,
        [
            "android.accounts.LOGIN_ACCOUNTS_CHANGED",
            "android.accounts.action.VISIBLE_ACCOUNTS_CHANGED"
        ]
    );
    assert!(receive.try_recv().is_err());
}

#[test]
fn sync_accounts_race_register_before_snapshot_and_concurrent_revalidation() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let (manager, de, _) = fixture(host);
        manager.accounts(0).unwrap();
        check(&manager, false);
        let receive = listen(host, export, manager.clone());
        let registered = Barrier::new(2);
        let start = Barrier::new(4);
        std::thread::scope(|scope| {
            let registration = scope.spawn(|| {
                register(&manager);
                registered.wait();
            });
            registered.wait();
            registration.join().unwrap();
            // Pin the initial snapshot's generation while two validators start.
            let (waiting, release, install) = pause_at("snapshot");
            let initial = scope.spawn(|| {
                install();
                let found = invoke(&manager, sync::GET_RUNNING_ACCOUNTS_FOR_SYSTEM).unwrap();
                assert_eq!(found.len(), 3);
            });
            waiting.recv_timeout(Duration::from_secs(20)).unwrap();
            assert_eq!(count(&de, "SELECT count(*) FROM accounts"), 3);
            let validators: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        start.wait();
                        invoke(&manager, sync::VALIDATE_ACCOUNTS).unwrap();
                    })
                })
                .collect();
            let overlap = scope.spawn(|| {
                start.wait();
                let found = invoke(&manager, sync::GET_ALL_ACCOUNTS_FOR_SYSTEM_PROCESS).unwrap();
                assert!(found.len() == 2 || found.len() == 3);
            });
            start.wait();
            release.send(()).unwrap();
            initial.join().unwrap();
            for worker in validators {
                worker.join().unwrap();
            }
            overlap.join().unwrap();
        });
        check(&manager, true);
        transitions(host, &receive);
        invoke(&manager, sync::VALIDATE_ACCOUNTS).unwrap();
        host.broadcasts.wait(true);
        assert!(receive.try_recv().is_err());
    });
}

#[test]
fn sync_accounts_race_failed_validation_and_overlapping_commit() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let (manager, de, ce) = fixture(host);
        manager.accounts(0).unwrap();
        host.set_user_unlocked_for_package_test(0, true);
        manager.accounts(0).unwrap();
        let receive = listen(host, export, manager.clone());
        register(&manager);
        let sql = Connection::open(&ce).unwrap();
        sql.execute_batch("CREATE TRIGGER deny_delete BEFORE DELETE ON accounts WHEN OLD._id=1 BEGIN SELECT RAISE(ABORT,'denied'); END;").unwrap();
        assert_eq!(
            invoke(&manager, sync::VALIDATE_ACCOUNTS).unwrap_err().code,
            EX_ILLEGAL_STATE
        );
        // Snapshots must propagate the failed generation, never report a successful empty refresh.
        assert!(invoke(&manager, sync::GET_RUNNING_ACCOUNTS_FOR_SYSTEM).is_err());
        assert!(invoke(&manager, sync::GET_ALL_ACCOUNTS_FOR_SYSTEM_PROCESS).is_err());
        host.broadcasts.wait(true);
        assert!(receive.try_recv().is_err());
        assert_eq!(count(&de, "SELECT count(*) FROM accounts"), 3);
        assert_eq!(count(&ce, "SELECT count(*) FROM accounts WHERE _id=1"), 1);
        sql.execute_batch("DROP TRIGGER deny_delete").unwrap();
        let start = Barrier::new(3);
        std::thread::scope(|scope| {
            let (waiting, release, install) = pause_at("commit");
            let validation = scope.spawn(|| {
                install();
                invoke(&manager, sync::VALIDATE_ACCOUNTS).unwrap();
            });
            waiting.recv_timeout(Duration::from_secs(20)).unwrap();
            // Another connection still sees the old committed DE/CE generation.
            assert_eq!(count(&de, "SELECT count(*) FROM accounts"), 3);
            assert_eq!(count(&ce, "SELECT count(*) FROM accounts WHERE _id=1"), 1);
            let snapshots: Vec<_> = [
                sync::GET_RUNNING_ACCOUNTS_FOR_SYSTEM,
                sync::GET_ALL_ACCOUNTS_FOR_SYSTEM_PROCESS,
            ]
            .into_iter()
            .map(|code| {
                let start = &start;
                let manager = &manager;
                let de = &de;
                let ce = &ce;
                scope.spawn(move || {
                    start.wait();
                    let found = invoke(manager, code).unwrap();
                    assert_eq!(found.len(), 2);
                    assert!(found.iter().all(|a| a.name != "a"));
                    assert_eq!(count(&de, "SELECT count(*) FROM accounts"), 2);
                    assert_eq!(count(&ce, "SELECT count(*) FROM accounts WHERE _id=1"), 0);
                })
            })
            .collect();
            start.wait();
            release.send(()).unwrap();
            validation.join().unwrap();
            for snapshot in snapshots {
                snapshot.join().unwrap();
            }
        });
        check(&manager, true);
        transitions(host, &receive);
    });
}
