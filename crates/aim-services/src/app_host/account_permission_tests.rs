//! Pinned AccountManagerService.hasAccountAccess, permissionIsGranted and
//! grantAppPermission/revokeAppPermission: live owner, generated calls and listeners.
use super::validation_tests::{count, fixture};
use super::*;
use aim_service_aidl::{
    dev_aim_server_iaccountpermissionlistener as listener, dev_aim_server_isyncaccounts as sync,
};
use rusqlite::Connection;
use std::sync::mpsc;
use std::time::Duration;

fn invoke(service: &dyn Service, uid: u32, code: u32, p: &Parcel) -> Parcel {
    service
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 1,
            sender_euid: uid,
            data: p.reader(),
        })
        .unwrap()
}
fn access(manager: &Arc<AccountManager>, account: &Account, uid: i32) -> bool {
    let mut p = Parcel::new();
    sync::HasAccountAccess {
        account: Some(account.clone()),
        uid,
    }
    .write(&mut p);
    sync::read_has_account_access_reply(
        &mut invoke(
            &SyncAccounts(manager.clone()),
            1000,
            sync::HAS_ACCOUNT_ACCESS,
            &p,
        )
        .reader(),
    )
    .unwrap()
    .unwrap()
}
fn update(
    manager: &AccountManager,
    caller: u32,
    account: &Account,
    uid: i32,
    grant: bool,
) -> Result<(), Exception> {
    let mut p = Parcel::new();
    am::UpdateAppPermission {
        account: Some(account.clone()),
        auth_token_type: Some("token".into()),
        uid,
        value: grant,
    }
    .write(&mut p);
    am::read_update_app_permission_reply(
        &mut invoke(manager, caller, am::UPDATE_APP_PERMISSION, &p).reader(),
    )
    .unwrap()
}
fn register(
    manager: &Arc<AccountManager>,
    target: Option<aim_binder_host::parcel::Binder>,
    caller: u32,
) -> Result<(), Exception> {
    let mut p = Parcel::new();
    sync::AddOnAppPermissionChangeListener { listener: target }.write(&mut p);
    sync::read_add_on_app_permission_change_listener_reply(
        &mut invoke(
            &SyncAccounts(manager.clone()),
            caller,
            sync::ADD_ON_APP_PERMISSION_CHANGE_LISTENER,
            &p,
        )
        .reader(),
    )
    .unwrap()
}

fn publish_signing(host: &Arc<AppHost>, p: Signatures, q: Signatures) {
    let system = host.package_system().unwrap();
    let base = host.package_capture().unwrap();
    let mut owner = base.scan().owner().clone();
    for setting in &mut owner.settings.packages {
        setting.signatures = Some(if setting.name == "p" {
            p.clone()
        } else {
            q.clone()
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
}
fn signer(current: &[u8], past: Option<(&[u8], i32)>) -> Signatures {
    Signatures {
        signatures: vec![current.to_vec()],
        past_signatures: past
            .map(|(cert, flags)| vec![(cert.to_vec(), flags), (current.to_vec(), 0)]),
        ..Default::default()
    }
}
const A: &[u8] = include_bytes!("../../tests/fixtures/settings-certificate-1.der");
const B: &[u8] = include_bytes!("../../tests/fixtures/settings-certificate-2.der");

#[test]
fn account_access_uses_current_and_rotated_signing_and_installed_identities() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        let (manager, de, _) = fixture(host);
        // No seeded grant may mask the signing decision.
        Connection::open(de)
            .unwrap()
            .execute("DELETE FROM grants", [])
            .unwrap();
        let account = Account::new("c".into(), "replaced".into());
        manager.accounts(0).unwrap();
        for (server, client, allowed) in [
            (signer(A, None), signer(B, None), false),
            (signer(A, None), signer(A, None), true),
            (signer(B, Some((A, CERT_AUTH))), signer(A, None), true),
            (signer(B, Some((A, 1))), signer(A, None), false),
            (signer(A, None), signer(B, Some((A, 1))), true),
            (signer(B, Some((A, CERT_AUTH))), signer(B, None), true),
        ] {
            publish_signing(host, server, client);
            assert!(access(&manager, &account, 10100)); // Same authenticator UID.
            assert_eq!(access(&manager, &account, 10101), allowed);
        }
        for uid in [-1, 10999, 110100] {
            assert!(!access(&manager, &account, uid));
        }
        assert!(
            !manager
                .has_account_access(&account, Some("missing"), 10100)
                .unwrap()
        );
        assert!(
            !manager
                .has_account_access(&account, Some("q"), 10100)
                .unwrap()
        );
        for uid in [1000, 10100, 10101] {
            assert!(!access(
                &manager,
                &Account::new("missing".into(), "replaced".into()),
                uid
            ));
        }
        let mut p = Parcel::new();
        sync::HasAccountAccess {
            account: Some(account),
            uid: 10100,
        }
        .write(&mut p);
        assert!(
            sync::read_has_account_access_reply(
                &mut invoke(
                    &SyncAccounts(manager.clone()),
                    10100,
                    sync::HAS_ACCOUNT_ACCESS,
                    &p
                )
                .reader()
            )
            .unwrap()
            .is_err()
        );
        assert!(!manager.users.lock().unwrap().contains_key(&1));
    });
}

struct Probe(
    Arc<AccountManager>,
    std::path::PathBuf,
    mpsc::Sender<bool>,
    Strong,
);
struct Sink;
impl Service for Sink {
    fn descriptor(&self) -> &str {
        listener::DESCRIPTOR
    }
    fn transact(&self, _: &mut Call<'_>) -> Reply {
        Ok(Parcel::new())
    }
}
impl Service for Probe {
    fn descriptor(&self) -> &str {
        listener::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert!(call.is_oneway());
        assert_eq!(call.code, listener::ON_APP_PERMISSION_CHANGED);
        let a = listener::OnAppPermissionChanged::<Account>::read(&mut call.data)?;
        let account = a.account.unwrap();
        assert_eq!(a.uid, 10101);
        assert!(self.0.users.try_lock().is_ok());
        let owner = self.0.accounts(0).unwrap();
        let state = owner.state.try_lock().unwrap();
        let granted = state
            .db
            .find_matching_grants_count_any_token(a.uid, &account)
            .unwrap()
            != 0;
        drop(state);
        assert_eq!(
            count(&self.1, "SELECT count(*) FROM grants WHERE uid=10101"),
            i32::from(granted)
        );
        assert_eq!(access(&self.0, &account, a.uid), granted);
        // Reentry includes listener registration's lock, not just cache reads.
        register(&self.0, Some(self.3.binder()), 1000).unwrap();
        self.2.send(granted).unwrap();
        Ok(Parcel::new())
    }
}

#[test]
fn explicit_grants_persist_and_callbacks_follow_commit_and_allow_reentry() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        publish_signing(host, signer(A, None), signer(B, None));
        let (manager, de, _) = fixture(host);
        let account = Account::new("c".into(), "replaced".into());
        let owner = manager.accounts(0).unwrap();
        let (send, receive) = mpsc::channel();
        let target = export(Arc::new(Probe(
            manager.clone(),
            de.clone(),
            send,
            export(Arc::new(Sink)),
        )));
        for uid in [0, 2000, 10100] {
            assert!(register(&manager, Some(target.binder()), uid).is_err());
        }
        register(&manager, Some(target.binder()), 1000).unwrap();
        assert!(update(&manager, 10100, &account, 10101, true).is_err());
        for uid in [-1, 10999, 110101] {
            assert!(update(&manager, 1000, &account, uid, true).is_err());
        }
        let missing = Account::new("missing".into(), "replaced".into());
        for grant in [true, false] {
            assert!(update(&manager, 1000, &missing, 10101, grant).is_err());
        }
        assert_eq!(count(&de, "SELECT count(*) FROM grants WHERE uid=10101"), 0);
        let sql = Connection::open(&de).unwrap();
        for grant in [true, true, false, false, true] {
            let (operation, trigger) = if grant {
                ("INSERT", "deny_grant")
            } else {
                ("DELETE", "deny_revoke")
            };
            sql.execute_batch(&format!("CREATE TRIGGER {trigger} BEFORE {operation} ON grants BEGIN SELECT RAISE(ABORT,'denied'); END;")).unwrap();
            let before = access(&manager, &account, 10101);
            if grant || before {
                assert!(update(&manager, 1000, &account, 10101, grant).is_err());
                assert_eq!(access(&manager, &account, 10101), before);
            }
            sql.execute_batch(&format!("DROP TRIGGER {trigger}"))
                .unwrap();
            update(&manager, 1000, &account, 10101, grant).unwrap();
            assert_eq!(
                receive.recv_timeout(Duration::from_secs(20)).unwrap(),
                grant
            );
            assert!(receive.try_recv().is_err());
            assert_eq!(
                owner
                    .state
                    .lock()
                    .unwrap()
                    .db
                    .find_matching_grants_count_any_token(10101, &account)
                    .unwrap(),
                i64::from(grant)
            );
        }
        drop(owner);
        manager.users.lock().unwrap().clear();
        assert!(access(&manager, &account, 10101)); // Reopened DE while locked.
        update(&manager, 1000, &account, 10101, false).unwrap();
        assert!(!receive.recv_timeout(Duration::from_secs(20)).unwrap());
        manager.users.lock().unwrap().clear();
        assert!(!access(&manager, &account, 10101));
        assert_eq!(count(&de, "SELECT count(*) FROM grants WHERE uid=10101"), 0);
    });
}

#[test]
fn shared_uid_members_use_the_shared_identity() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        let system = host.package_system().unwrap();
        let base = host.package_capture().unwrap();
        let mut owner = base.scan().owner().clone();
        owner
            .settings
            .shared_users
            .push(crate::package::settings::SharedUser {
                name: "accounts.shared".into(),
                app_id: 10100,
                signatures: Some(signer(A, None)),
                ..Default::default()
            });
        for setting in &mut owner.settings.packages {
            setting.app_id = 10100;
            setting.shared_user = true;
            setting.shared_user_app_id = Some(10100);
            setting.signatures = Some(signer(A, None));
        }
        owner.identities = crate::package::owner::shared_users::Bootstrap::restore(
            &Default::default(),
            &owner.settings,
        )
        .unwrap();
        for ps in base.state().packages.values() {
            let mut package = (**ps.pkg.as_ref().unwrap()).clone();
            package.uid = 10100;
            package.shared_user_id = Some("accounts.shared".into());
            owner.load_test_code(package);
        }
        let names = ["p", "q"].map(|n| (n.to_string(), false));
        let groups = owner
            .identities
            .shared_users
            .keys()
            .map(|name| (name.clone(), Default::default()))
            .collect();
        owner
            .capture_legacy_permissions(
                &[0],
                names.clone().map(|n| (n, Default::default())).into(),
                groups,
            )
            .unwrap();
        owner
            .capture_install_permissions_fixed(names.map(|n| (n, false)).into())
            .unwrap();
        crate::system::complete_discovery_owner(&mut owner);
        let mut context = (**base.context()).clone();
        for package in context.packages.values_mut() {
            package.app_id = 10100;
        }
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
        let (manager, _, _) = fixture(host);
        let account = Account::new("c".into(), "replaced".into());
        assert_eq!(manager.packages_for_uid(10100).unwrap().len(), 2);
        assert!(access(&manager, &account, 10100));
        for package in ["p", "q"] {
            assert!(
                manager
                    .has_account_access(&account, Some(package), 10100)
                    .unwrap()
            );
        }
        let unowned = Account::new("b".into(), "kept".into());
        assert!(!access(&manager, &unowned, 10100));
        update(&manager, 1000, &unowned, 10100, true).unwrap();
        for package in ["p", "q"] {
            assert!(
                manager
                    .has_account_access(&unowned, Some(package), 10100)
                    .unwrap()
            );
        }
        update(&manager, 1000, &unowned, 10100, false).unwrap();
        assert!(!access(&manager, &unowned, 10100));
        update(&manager, 1000, &account, 10100, true).unwrap();
        let accounts = manager.accounts(0).unwrap();
        assert_eq!(
            accounts
                .state
                .lock()
                .unwrap()
                .db
                .find_matching_grants_count_any_token(10100, &account)
                .unwrap(),
            2
        );
        update(&manager, 1000, &account, 10100, false).unwrap();
        // Revoking this token preserves the existing authenticator-owned token grant.
        assert_eq!(
            accounts
                .state
                .lock()
                .unwrap()
                .db
                .find_matching_grants_count_any_token(10100, &account)
                .unwrap(),
            1
        );
    });
}
