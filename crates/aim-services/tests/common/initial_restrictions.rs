//! Initialized native first-write documents read by original Settings.
use aim_services::package::{
    owner::recovery,
    restrictions::Restrictions,
    scan::{CapturedUsers, SigningScan},
    settings::{Settings, Version},
    system_config::SystemConfig,
};
use std::{collections::BTreeMap, fs, path::Path};
pub fn export(directory: &Path) {
    let settings_xml=b"<packages><package name='p' codePath='/p' userId='10001' domainSetId='00000000-0000-0000-0000-000000000001'/></packages>";
    let inputs = [
        "<package-restrictions><pkg name='p' inst='true' stopped='false' enabled='0' ceDataInode='11' deDataInode='12' first-install-time='77'/></package-restrictions>",
        "<package-restrictions><pkg name='p' inst='false' stopped='true' nl='true' hidden='true' enabled='2' enabledCaller='owner' ceDataInode='21' deDataInode='22' first-install-time='88'><enabled-components><item name='p.Enabled'/></enabled-components><disabled-components><item name='p.Disabled'/></disabled-components></pkg></package-restrictions>",
        "<package-restrictions><pkg name='p' suspended='true'><suspend-params suspending-package='android' quarantined='true'><dialog-info title='Stopped' buttonAction='0'/><app-extras><int name='count' value='3'/><pbundle_as_map name='nested'><long name='time' value='4'/></pbundle_as_map><string-array name='names' num='2'><item value='one'/><item value='two'/></string-array></app-extras><launcher-extras><boolean name='shown' value='true'/></launcher-extras></suspend-params><archive-state installer-title='Installer' archive-time='77'><archive-activity-info activity-title='Archived' original-component-name='p/.Main' icon-path='/data/archive/icon.png'/></archive-state></pkg></package-restrictions>",
    ];
    for (index, input) in inputs.iter().enumerate() {
        let data = directory.join(format!("initial-restrictions-native-{index}"));
        fs::create_dir_all(data.join("system/users/0")).unwrap();
        let path = data.join("system/users/0/package-restrictions.xml");
        fs::write(&path, b"unread malformed main").unwrap();
        fs::write(
            path.with_file_name("package-restrictions.xml.reservecopy"),
            b"unread malformed reserve",
        )
        .unwrap();
        let mut settings = Settings::parse(&aim_android_xml::read(settings_xml).unwrap()).unwrap();
        let current = Version {
            sdk_version: 36,
            database_version: 3,
            ..Default::default()
        };
        let (mut store, report) = recovery::Plan::inspect(&data)
            .unwrap()
            .recover_boot(&[0], &mut settings, &current, |_, _| {
                panic!("absent settings read")
            })
            .unwrap();
        assert!(report.first_boot);
        let root = aim_android_xml::read(input.as_bytes()).unwrap();
        let state = Restrictions::parse(&root).unwrap().packages.remove(0).1;
        let mut scan = SigningScan::new(&SystemConfig::default(), &settings, 36).unwrap();
        scan.capture_user_states(BTreeMap::from([(
            ("p".into(), false),
            CapturedUsers {
                states: BTreeMap::from([(0, state)]),
                active_aliases: Default::default(),
            },
        )]))
        .unwrap();
        let snapshot = aim_services::package::scan_snapshot::Store::new(
            scan.clone(), aim_services::package::owner::usage::Usage::new(["p"]),
        ).unwrap().capture();
        store.commit_scan_settings(&snapshot).unwrap();
        store.claim_unread_restrictions(0).unwrap();
        store
            .commit_initial_scan_restrictions(
                &scan,
                0,
                false,
                aim_android_xml::read(b"<package-restrictions/>").unwrap(),
            )
            .unwrap();
        let bytes = fs::read(&path).unwrap();
        assert_eq!(
            bytes,
            fs::read(path.with_file_name("package-restrictions.xml.reservecopy")).unwrap()
        );
        let reopened = aim_services::package::owner::Store::open(&data, &[0])
            .unwrap()
            .unwrap();
        assert_eq!(
            reopened.state().users[0].1.restrictions,
            store.state().users[0].1.restrictions
        );
        let restrictions = Restrictions::parse(&aim_android_xml::read(&bytes).unwrap()).unwrap();
        let p = &restrictions.packages[0].1;
        let trace = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            p.installed,
            p.stopped,
            p.not_launched,
            p.hidden,
            p.enabled,
            p.ce_data_inode,
            p.de_data_inode,
            p.first_install_time,
            p.last_disable_app_caller.as_deref().unwrap_or("null")
        );
        fs::write(
            directory.join(format!("initial-restrictions-{index}.expected")),
            trace,
        )
        .unwrap();
    }
}
