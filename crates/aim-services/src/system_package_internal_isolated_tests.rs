//! Real coordinator regression for AMS isolated callbacks during install/query lag.
use super::*;

#[test]
fn isolated_callback_preserves_durable_install_during_query_lag() {
    let mut fixture = Fixture::new();
    let bridge = fixture.attach();
    let before = publish(&fixture, &bridge);
    let snapshots = fixture
        .system
        .package_bootstrap
        .lock()
        .unwrap()
        .current
        .as_ref()
        .unwrap()
        .snapshots
        .clone()
        .unwrap();
    let mut installed_owner = before.scan().owner().clone();
    let setting = &mut installed_owner.settings.packages[0];
    setting.version_code = 7;
    setting.code_path = "/data/app/p-new".into();
    setting.last_update_time = 1234;
    setting.primary_cpu_abi = Some("arm64-v8a".into());
    let installed_setting = setting.clone();
    installed_owner
        .capture_replica_runtime(BTreeMap::from([(
            ("p".into(), false),
            before
                .scan()
                .owner()
                .replica_runtime("p", false)
                .unwrap()
                .unwrap()
                .clone(),
        )]))
        .unwrap();
    let disk = fixture.root.join("data/installed-path");
    let installed = snapshots
        .publish_after(
            before.scan(),
            installed_owner,
            before.scan().usage().clone(),
            |_| {
                std::fs::write(&disk, &installed_setting.code_path).map_err(|e| {
                    crate::package::owner::WriteError {
                        committed: false,
                        message: e.to_string(),
                    }
                })
            },
        )
        .unwrap();
    let version = fixture.system.package_bootstrap.lock().unwrap().version;
    // Genuine AMS callback against the old query capture, after canonical commit.
    // Both values are full user-10 UIDs; appId truncation would fail these asserts.
    fixture
        .system
        .internal_add_isolated_uid(1099001, 1010100, 1000, 42)
        .unwrap();
    let retained = fixture.system.capture_package_queries().unwrap();
    assert!(Arc::ptr_eq(&snapshots.capture(), &installed));
    assert!(Arc::ptr_eq(retained.scan(), before.scan()));
    assert_eq!(
        snapshots.capture().owner().settings.packages[0],
        installed_setting
    );
    assert_eq!(
        std::fs::read_to_string(&disk).unwrap(),
        installed_setting.code_path
    );
    assert_eq!(
        fixture.system.package_bootstrap.lock().unwrap().version,
        version
    );
    assert_eq!(
        retained.context().system.isolated_owners,
        [(1099001, 1010100)]
    );
    assert_eq!(
        retained.state().system.isolated_owners,
        [(1099001, 1010100)]
    );
    assert!(before.state().system.isolated_owners.is_empty());
    // The real post-install Context contract starts from the changed query base
    // and binds its inputs to the new committed setting, preserving the callback.
    let mut context = (**retained.context()).clone();
    context.scan_version = installed.version();
    let input = context.packages.get_mut(&("p".into(), false)).unwrap();
    input.version = installed_setting.version_code;
    input.path = installed_setting.code_path.clone();
    let projected =
        crate::package::scan_snapshot::query_state::Capture::new(installed.clone(), context)
            .unwrap();
    assert_eq!(projected.state().packages["p"].app_id, 10100);
    assert_eq!(
        projected.state().system.isolated_owners,
        [(1099001, 1010100)]
    );
    assert_eq!(
        projected.scan().owner().settings.packages[0],
        installed_setting
    );
    fixture
        .system
        .internal_remove_isolated_uid(1099001, 1000, 42)
        .unwrap();
    let removed = fixture.system.capture_package_queries().unwrap();
    assert!(removed.context().system.isolated_owners.is_empty());
    assert!(removed.state().system.isolated_owners.is_empty());
    assert!(Arc::ptr_eq(&snapshots.capture(), &installed));
    assert_eq!(
        snapshots.capture().owner().settings.packages[0],
        installed_setting
    );
    assert_eq!(
        retained.state().system.isolated_owners,
        [(1099001, 1010100)]
    );
    // Complete the install query handoff, then exercise the coherent publication.
    let mut context = (**removed.context()).clone();
    context.scan_version = installed.version();
    let input = context.packages.get_mut(&("p".into(), false)).unwrap();
    input.version = installed_setting.version_code;
    input.path = installed_setting.code_path.clone();
    let full = crate::package::scan_snapshot::query_state::Capture::new(installed.clone(), context)
        .unwrap();
    fixture
        .system
        .package_bootstrap
        .lock()
        .unwrap()
        .current
        .as_mut()
        .unwrap()
        .queries = Some(full);
    fixture
        .system
        .internal_add_isolated_uid(1099002, 1010100, 1000, 42)
        .unwrap();
    let coherent = fixture.system.capture_package_queries().unwrap();
    assert!(Arc::ptr_eq(coherent.scan(), &snapshots.capture()));
    assert_eq!(coherent.scan().version(), installed.version() + 1);
    assert_eq!(coherent.scan().metadata_revision(), installed.metadata_revision());
    assert_eq!(coherent.scan().usage(), installed.usage());
    assert_eq!(coherent.scan().owner(), installed.owner());
    let registry = crate::package::scan_snapshot::uid_owner_registry(coherent.scan()).unwrap();
    let mut registry = aim_binder_host::parcel::Reader::new(&registry, &[]);
    assert_eq!(registry.read_i64().unwrap(), coherent.scan().version() as i64);
    let count=registry.read_i32().unwrap();assert!(count>0);
    let mut found=false;
    for _ in 0..count {
        let id=registry.read_i32().unwrap();let kind=registry.read_i32().unwrap();
        let name=registry.read_string16().unwrap().unwrap();
        let retained=if kind==3 {Some(aim_service_aidl::read_byte_array(&mut registry).unwrap().unwrap())}else{None};
        if id==10100 {
            assert_eq!(name,"p");assert!(kind==1||kind==3);found=true;
            if let Some(bytes)=retained {
                let mut record=aim_binder_host::parcel::Reader::new(&bytes,&[]);
                let metadata=aim_service_aidl::read_byte_array(&mut record).unwrap().unwrap();
                let mut metadata=aim_binder_host::parcel::Reader::new(&metadata,&[]);
                assert_eq!(metadata.read_i64().unwrap(),coherent.scan().version() as i64);
            }
        }
    }
    assert!(found);
    assert_eq!(registry.remaining(), 0);
    assert_eq!(
        coherent.scan().owner().settings.packages[0],
        installed_setting
    );
    assert_eq!(
        coherent.context().system.isolated_owners,
        [(1099002, 1010100)]
    );
    assert_eq!(
        coherent.state().system.isolated_owners,
        [(1099002, 1010100)]
    );
}
