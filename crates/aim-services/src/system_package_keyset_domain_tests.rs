//! KeySetManagerService mutations and DVS writes share PMS's package owner.
//! android-16.0.0_r1, PackageManagerService / KeySetManagerService (#2637).
use super::*;
use crate::package::{domain_verification as domain, owner::key_sets};
use p256::elliptic_curve::sec1::ToEncodedPoint;

fn key(value: u8) -> Vec<u8> {
    let mut encoded = vec![
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08,
        0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
    ];
    encoded.extend_from_slice(
        p256::SecretKey::from_slice(&[value; 32])
            .unwrap()
            .public_key()
            .to_encoded_point(false)
            .as_bytes(),
    );
    encoded
}

#[test]
fn domain_write_retries_keyset_commit_before_query_publication_and_reload() {
    let session = session();
    let system = &session.fixture.system;
    let bridge = &session.bridge;
    let base = &session.old;
    let config = crate::package::system_config::SystemConfig::default();
    let mut domains = domain::owner::Owner::new(Default::default(), Default::default());
    let mut policies = BTreeMap::new();
    for setting in &base.scan().owner().settings.packages {
        domains
            .add(
                domain::owner::Input {
                    id: setting.domain_set_id.as_deref().unwrap(),
                    name: &setting.name,
                    code: Some(
                        base.scan().owner().loaded_packages()[&setting.name].runtime_package(),
                    ),
                    signatures: &[],
                    system: false,
                    restrict_domains: true,
                    pre_verified: None,
                },
                &config,
            )
            .unwrap();
        policies.insert(setting.name.clone(), true);
    }
    let context = (**base.context())
        .clone()
        .attach_boot_domains(
            base.scan().owner(),
            domain::owner::Boot {
                owner: domains,
                changes: vec![],
            },
            &config,
            &policies,
        )
        .unwrap();
    let mut context = context;
    context.scan_version += 1;
    let mut scan = base.scan().owner().clone();
    scan.settings.key_sets.versioned = true;
    let base = system
        .publish_package_scan_with_queries(
            bridge,
            Some(base.scan()),
            scan,
            base.scan().usage().clone(),
            context,
        )
        .unwrap();
    let (_, disk) = system.package_persistence_owner(&base).unwrap();
    disk.lock()
        .unwrap()
        .commit_scan_settings(base.scan())
        .unwrap();
    let snapshots = system
        .package_bootstrap
        .lock()
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .snapshots
        .clone()
        .unwrap();
    // A canonical commit may be waiting for disk. The domain writer already
    // holds disk, so it must return a retry without waiting for that commit.
    let pending = base
        .prepare_domain_update(base.domains().unwrap().owner().clone())
        .unwrap();
    let (entered, ready) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let worker_snapshots = snapshots.clone();
    let worker_base = base.scan().clone();
    let worker = std::thread::spawn(move || {
        worker_snapshots.publish_after(
            &worker_base,
            worker_base.owner().clone(),
            worker_base.usage().clone(),
            |_| {
                entered.send(()).unwrap();
                resume.recv_timeout(Duration::from_secs(5)).unwrap();
                Err(crate::package::owner::WriteError {
                    committed: false,
                    message: "cancel contended fixture commit".into(),
                })
            },
        )
    });
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    let result =
        system.commit_package_domains_if_current(bridge, pending, &mut disk.lock().unwrap());
    release.send(()).unwrap();
    assert!(worker.join().unwrap().is_err());
    assert!(result.unwrap().is_none());
    let mut latest = base.clone();
    // Pause each actual disk/canonical commit before its query handoff, as
    // NativePrepared does between persist_publish_install and publish_queries.
    for (name, scalar) in [("p", 1), ("q", 2)] {
        let mut desired = latest.domains().unwrap().owner().clone();
        desired
            .set_link_handling_internal(Some(name), false, 0, &[0])
            .unwrap();
        let pending = latest.prepare_domain_update(desired.clone()).unwrap();
        let mut scan = latest.scan().owner().clone();
        key_sets::register(&mut scan.settings, name, &[key(scalar)], None, &[]).unwrap();
        let committed = snapshots
            .publish_install_after(latest.scan(), scan, |snapshot| {
                disk.lock().unwrap().commit_scan_settings(snapshot)
            })
            .unwrap();
        let path = session.fixture.root.join("data/system/packages.xml");
        let bytes = std::fs::read(&path).unwrap();
        let counters = committed.owner().settings.key_sets.clone();
        assert!(Arc::ptr_eq(
            &latest,
            &system.capture_package_queries().unwrap()
        ));
        // This used to fail with "keyset commit rewound allocation counters".
        let result =
            system.commit_package_domains_if_current(bridge, pending, &mut disk.lock().unwrap());
        assert!(result.unwrap().is_none());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(Arc::ptr_eq(
            &latest,
            &system.capture_package_queries().unwrap()
        ));
        // An actual attempt to rewind still fails the persistence guard.
        let error = disk
            .lock()
            .unwrap()
            .commit_key_sets(&latest.scan().owner().settings)
            .unwrap_err();
        assert!(!error.committed);
        assert_eq!(error.message, "keyset commit rewound allocation counters");
        let projected = latest
            .prepare_committed_package_snapshot(committed.clone())
            .unwrap();
        {
            let mut state = system.package_bootstrap.lock().unwrap();
            state
                .current
                .as_mut()
                .unwrap()
                .set_queries(Some(projected.clone()));
            state.version = committed.version();
        }
        latest = system
            .commit_package_domains_if_current(
                bridge,
                projected.prepare_domain_update(desired).unwrap(),
                &mut disk.lock().unwrap(),
            )
            .unwrap()
            .unwrap();
        assert_eq!(latest.scan().owner().settings.key_sets, counters);
        let reopened = crate::package::owner::Store::open(&session.fixture.root.join("data"), &[0])
            .unwrap()
            .unwrap();
        assert_eq!(reopened.state().settings.key_sets, counters);
        assert_eq!(
            reopened.state().settings.domain_verification,
            latest.domains().unwrap().owner().persisted()
        );
        assert_eq!(counters.last_issued_key_id, i64::from(scalar));
        assert_eq!(counters.last_issued_key_set_id, i64::from(scalar));
    }
    // Continue allocation from reloaded settings, retaining both committed roles.
    let mut reloaded = crate::package::owner::Store::open(&session.fixture.root.join("data"), &[0])
        .unwrap()
        .unwrap();
    let mut settings = reloaded.state().settings.clone();
    key_sets::register(&mut settings, "p", &[key(3)], None, &[]).unwrap();
    reloaded.commit_key_sets(&settings).unwrap();
    let reopened = crate::package::owner::Store::open(&session.fixture.root.join("data"), &[0])
        .unwrap()
        .unwrap();
    assert_eq!(reopened.state().settings.key_sets.last_issued_key_id, 3);
    assert_eq!(reopened.state().settings.key_sets.last_issued_key_set_id, 3);
    assert_eq!(
        reopened
            .state()
            .settings
            .packages
            .iter()
            .find(|p| p.name == "q")
            .unwrap()
            .key_set_data
            .proper_signing_key_set,
        2
    );
}
