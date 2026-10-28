//! Live verifier identity through the app-host package bootstrap, android-16.0.0_r1.
use super::*;
use crate::package::{
    intent_filter::{IntentFilter, ParsedIntentInfo},
    pkg,
};
use aim_binder_host::parcel::EX_SECURITY;

#[test]
fn verifier_identity_follows_current_receivers_and_permission_capture() {
    crate::system::with_discovery_session(|host, publish, _| {
        publish(&mut |package| {
            package.booleans |= pkg::booleans::SYSTEM;
            let mut receiver = package.activities[0].clone();
            receiver.main.component.name = format!("{}.Verifier", package.package_name);
            let mut filter = IntentFilter::default();
            filter.add_action("android.intent.action.DOMAINS_NEED_VERIFICATION");
            receiver.main.component.intents = vec![ParsedIntentInfo {
                filter,
                ..Default::default()
            }];
            package.receivers = vec![receiver];
        });
        let leaf = PackageBootstrap::new(host.clone());
        let ask = |sender, uid| {
            let mut p = Parcel::new();
            bridge::IsDomainVerifierUid { uid }.write(&mut p);
            let response = leaf
                .transact(&mut Call {
                    code: bridge::IS_DOMAIN_VERIFIER_UID,
                    flags: 0,
                    sender_pid: 77,
                    sender_euid: sender,
                    data: p.reader(),
                })
                .unwrap();
            bridge::read_is_domain_verifier_uid_reply(&mut response.reader()).unwrap()
        };
        assert_eq!(ask(10100, 10100).unwrap_err().code, EX_SECURITY);
        assert_eq!(ask(1000, -1).unwrap_err().code, EX_ILLEGAL_ARGUMENT);
        let system = host.package_system().unwrap();
        let bridge = system.package_bootstrap().unwrap();
        for selected in [None, Some("p"), Some("q"), None] {
            let base = host.package_capture().unwrap();
            let mut context = (**base.context()).clone();
            for ((name, _), package) in &mut context.packages {
                for user in package.users.values_mut() {
                    user.granted_permissions = if selected == Some(name.as_str()) {
                        vec!["android.permission.DOMAIN_VERIFICATION_AGENT".into()]
                    } else {
                        vec![]
                    };
                }
            }
            context.scan_version = base.scan().version() + 1;
            system
                .publish_package_scan_with_queries(
                    &bridge,
                    Some(base.scan()),
                    base.scan().owner().clone(),
                    base.scan().usage().clone(),
                    context,
                )
                .unwrap();
            for (uid, name) in [
                (10100, "p"),
                (10101, "q"),
                (2000, "shell"),
                (19999, "unknown"),
            ] {
                assert_eq!(ask(1000, uid).unwrap(), selected == Some(name));
            }
        }
    });
    let leaf = PackageBootstrap::new(AppHost::new(crate::test_process(1000)));
    let mut p = Parcel::new();
    bridge::IsDomainVerifierUid { uid: 10100 }.write(&mut p);
    let response = leaf
        .transact(&mut Call {
            code: bridge::IS_DOMAIN_VERIFIER_UID,
            flags: 0,
            sender_pid: 77,
            sender_euid: 1000,
            data: p.reader(),
        })
        .unwrap();
    assert!(
        bridge::read_is_domain_verifier_uid_reply(&mut response.reader())
            .unwrap()
            .is_err()
    );
}
