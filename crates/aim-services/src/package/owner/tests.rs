use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) struct Data(pub(super) PathBuf);

impl Data {
    pub(super) fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "aim-package-owner-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    pub(super) fn settings(&self) -> PathBuf {
        let dir = self.0.join("system/users/0");
        fs::create_dir_all(&dir).unwrap();
        fs::write(self.0.join("system/packages.xml"),
            b"<packages><package name='example.app' codePath='/data/app/example' userId='10100' it='12' /></packages>").unwrap();
        dir.join("package-restrictions.xml")
    }
}

impl Drop for Data {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

const RESTRICTIONS: &[u8] = b"<package-restrictions><pkg name='example.app' stopped='true' inst='true'><suspend-params suspending-package='android'><dialog-info dialogMessage='keep me' /></suspend-params></pkg><crossProfile-intent-filters><item targetUserId='10'><filter><action name='example.ACTION' /></filter></item></crossProfile-intent-filters></package-restrictions>";

#[test]
fn enabled_write_preserves_typed_suspension_and_text_cdata_meaning() {
    use crate::package::restrictions::persistable::{Bundle, Value as Persistable};
    let data = Data::new();
    let path = data.settings();
    fs::write(&path, b"<package-restrictions><pkg name='example.app'><suspend-params suspending-package='android' quarantined='true'><app-extras><string name='message'><![CDATA[kept]]> text</string><int-array name='slots' num='3'><item value='7'/></int-array></app-extras></suspend-params></pkg></package-restrictions>").unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let suspension = store.state().users[0].1.restrictions.packages[0]
        .1
        .suspensions[0]
        .clone();
    assert_eq!(
        suspension.app_extras,
        Some(Bundle {
            entries: vec![
                (
                    Some("message".into()),
                    Persistable::String("kept text".into())
                ),
                (Some("slots".into()), Persistable::Ints(vec![7, 0, 0]))
            ]
        })
    );
    store
        .commit_enabled(
            "example.app",
            0,
            &Enabled {
                enabled: 2,
                last_disable_app_caller: None,
                enabled_components: BTreeSet::new(),
                disabled_components: BTreeSet::new(),
            },
        )
        .unwrap();
    let reread = State::read(&data.0, &[0]).unwrap().unwrap();
    assert_eq!(reread, *store.state());
    assert_eq!(
        reread.users[0].1.restrictions.packages[0].1.suspensions[0],
        suspension
    );
    assert!(fs::read(path).unwrap().starts_with(abx::MAGIC));
}

#[test]
fn enabled_write_accepts_unchanged_nan_payload_and_rejects_external_signed_zero_change() {
    let data = Data::new();
    let path = data.settings();
    let mut root = aim_android_xml::read(b"<package-restrictions><pkg name='example.app'><suspend-params suspending-package='android'><app-extras><double name='value' value='0'/></app-extras></suspend-params></pkg></package-restrictions>").unwrap();
    fn set(root: &mut Element, value: f64) {
        let child = root
            .content
            .iter_mut()
            .find_map(|n| match n {
                Node::Element(e) => Some(e),
                _ => None,
            })
            .unwrap();
        if child.name == "double" {
            attribute(child, "value", Some(Value::Double(value)));
        } else {
            set(child, value);
        }
    }
    let enabled = Enabled {
        enabled: 2,
        last_disable_app_caller: None,
        enabled_components: BTreeSet::new(),
        disabled_components: BTreeSet::new(),
    };
    set(&mut root, f64::from_bits(0xfff0000000000001));
    fs::write(&path, abx::write(&root).unwrap()).unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    store.commit_enabled("example.app", 0, &enabled).unwrap();
    let reread = Store::open(&data.0, &[0]).unwrap().unwrap();
    assert_eq!(store.state(), reread.state());

    set(&mut root, 0.0);
    fs::write(&path, abx::write(&root).unwrap()).unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    set(&mut root, -0.0);
    let external = abx::write(&root).unwrap();
    fs::write(&path, &external).unwrap();
    let error = store
        .commit_enabled("example.app", 0, &enabled)
        .unwrap_err();
    assert!(!error.committed);
    assert!(error.message.contains("changed outside the native owner"));
    assert_eq!(fs::read(path).unwrap(), external);
}

#[test]
fn preferred_clearings_preserve_last_choices_persistent_filters_and_other_users() {
    let data = Data::new();
    let path = data.settings();
    let input = b"<package-restrictions future='keep'><pkg name='example.app' stopped='true'/><preferred-activities><item name='removed/.Always' always='true' set='0'><filter><action name='always'/></filter><future/></item><item name='removed/.Default' set='0'><filter/></item><item name='removed/.Last' always='false' set='0'><filter/></item><item name='kept/.Main' always='true' set='1'><set name='removed/.Candidate'/><filter/></item><future-list/></preferred-activities><persistent-preferred-activities><item name='removed/.Policy' set-by-dpm='true'><filter/></item></persistent-preferred-activities><crossProfile-intent-filters><item ownerPackage='removed'/></crossProfile-intent-filters><future-root/></package-restrictions>";
    fs::write(&path, input).unwrap();
    let other = data.0.join("system/users/10/package-restrictions.xml");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    fs::write(&other, input).unwrap();
    let mut store = Store::open(&data.0, &[0, 10]).unwrap().unwrap();
    let state = store.state().clone();
    assert!(
        store
            .clear_package_preferred_activities(0, Some("removed"))
            .unwrap()
    );
    assert_eq!(store.state(), &state);
    assert_eq!(fs::read(&other).unwrap(), input);
    let bytes = fs::read(&path).unwrap();
    assert_eq!(fs::read(sibling(&path, ".reservecopy")).unwrap(), bytes);
    let root = aim_android_xml::read(&bytes).unwrap();
    let mut expected = aim_android_xml::read(input).unwrap();
    let list = expected
        .content
        .iter_mut()
        .find_map(|node| match node {
            Node::Element(e) if e.name == "preferred-activities" => Some(e),
            _ => None,
        })
        .unwrap();
    list.content.retain(|node| !matches!(node, Node::Element(e) if matches!(e.string("name").as_deref(), Some("removed/.Always" | "removed/.Default"))));
    assert_eq!(root, expected);
    let mut reopened = Store::open(&data.0, &[0, 10]).unwrap().unwrap();
    assert!(
        !reopened
            .clear_package_preferred_activities(0, Some("removed"))
            .unwrap()
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    // Detect an external writer before replacing any of its data.
    let mut external = root.clone();
    external
        .attrs
        .push(("external".into(), Value::String("writer".into())));
    fs::write(&path, abx::write(&external).unwrap()).unwrap();
    assert!(
        !reopened
            .clear_package_preferred_activities(0, None)
            .unwrap_err()
            .committed
    );
    fs::write(&path, &bytes).unwrap();
    assert!(
        reopened
            .clear_package_preferred_activities(0, None)
            .unwrap()
    );
    let root = aim_android_xml::read(&fs::read(&path).unwrap()).unwrap();
    let list = root
        .children()
        .find(|e| e.name == "preferred-activities")
        .unwrap();
    assert!(list.children().all(|e| e.name != "item"));
    assert!(
        root.children()
            .any(|e| e.name == "persistent-preferred-activities")
    );
    assert_eq!(fs::read(&other).unwrap(), input);
    assert!(
        !reopened
            .clear_package_preferred_activities(20, None)
            .unwrap_err()
            .committed
    );
}

#[test]
fn renamed_package_cleanup_persists_only_the_real_name_key() {
    let data = Data::new();
    let restrictions = data.settings();
    fs::write(restrictions, RESTRICTIONS).unwrap();
    let path = data.0.join("system/packages.xml");
    let input = b"<packages future='keep'><package name='example.app' codePath='/data/app/example' userId='10100'/><renamed-package new='real' old='internal'/><renamed-package new='other' old='internal' future='keep'/><future-owner/></packages>";
    fs::write(&path, input).unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    assert!(!store.commit_removed_renamed_package("internal").unwrap());
    assert_eq!(fs::read(&path).unwrap(), input);
    assert!(store.commit_removed_renamed_package("real").unwrap());
    assert!(!store.commit_removed_renamed_package("real").unwrap());
    let bytes = fs::read(&path).unwrap();
    let mut expected = aim_android_xml::read(input).unwrap();
    expected.content.retain(|node| !matches!(node, Node::Element(e) if e.name == "renamed-package" && e.string("new").as_deref() == Some("real")));
    assert_eq!(aim_android_xml::read(&bytes).unwrap(), expected);
    assert_eq!(fs::read(sibling(&path, ".reservecopy")).unwrap(), bytes);
    assert_eq!(
        Store::open(&data.0, &[0])
            .unwrap()
            .unwrap()
            .state()
            .settings
            .renamed_packages,
        [("other".into(), "internal".into())]
    );
}

#[test]
fn package_list_commit_validates_owner_inventory_preserves_gid_order_and_detects_external_writes() {
    let data = Data::new();
    let restrictions = data.settings();
    fs::write(restrictions, RESTRICTIONS).unwrap();
    let path = data.0.join("system/packages.list");
    let old = "example.app 10100 0 /data/user/0/example.app default none 0 0 0 @null\n";
    fs::write(&path, old).unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let mut entries = store.state().list.clone();
    entries[0].gids = vec![3003, 1003003, 3003];
    entries[0].debuggable = true;
    entries[0].profileable_from_shell = true;
    entries[0].profileable = true;
    entries[0].installer = "store".into();
    for mutation in [0, 1, 2, 3, 4] {
        let mut invalid = entries.clone();
        match mutation {
            0 => invalid[0].name = "unknown".into(),
            1 => invalid[0].uid += 1,
            2 => invalid[0].version_code += 1,
            3 => invalid[0].data_dir = "/data/space path".into(),
            _ => invalid.push(invalid[0].clone()),
        }
        assert!(!store.commit_package_list(&invalid).unwrap_err().committed);
        assert_eq!(fs::read_to_string(&path).unwrap(), old);
    }
    fs::write(&path, old.replace("none", "3002")).unwrap();
    assert!(!store.commit_package_list(&entries).unwrap_err().committed);
    fs::write(&path, old).unwrap();
    fs::write(sibling(&path, ".tmp"), "partial").unwrap();
    store.commit_package_list(&entries).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "example.app 10100 1 /data/user/0/example.app default 3003,1003003,3003 1 0 1 store\n"
    );
    assert!(!sibling(&path, ".tmp").exists());
    assert_eq!(store.state().list, entries);
    let metadata = guest_inode::read(&path).unwrap().unwrap();
    assert_eq!(
        metadata,
        GuestInode {
            uid: Some(1000),
            gid: Some(aim_service_aidl::android_os_process::PACKAGE_INFO_GID),
            mode: Some(0o640)
        }
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(
        Store::open(&data.0, &[0]).unwrap().unwrap().state().list,
        entries
    );
}

#[test]
fn removed_setting_persistence_preserves_signer_references_sources_and_other_user_xml() {
    let data = Data::new();
    let restriction = data.settings();
    let input = b"<packages future='keep'><package name='store' codePath='/data/app/store' userId='10100'><sigs count='1' schemeVersion='3'><cert index='0' key='0102'/></sigs></package><package name='app' codePath='/data/app/app' userId='10101' installer='store' installerUid='10100' installInitiator='store' installOriginator='store' updateOwner='store' installerAttributionTag='tag'><sigs count='1' schemeVersion='3'><cert index='0'/><pastSigs count='1'><cert index='0' flags='7'/></pastSigs></sigs><install-initiator-sigs count='1' schemeVersion='3'><cert index='0'/></install-initiator-sigs><future-package/></package><updated-package name='app' codePath='/system/app/app' userId='10101' installer='store'/><future-owner/></packages>";
    fs::write(data.0.join("system/packages.xml"), input).unwrap();
    let user_xml = b"<package-restrictions future='keep'><pkg name='store' stopped='true'/><pkg name='app' enabled='2'><future-package/></pkg><preferred-activities><future-list/></preferred-activities><future-root/></package-restrictions>";
    fs::write(&restriction, user_xml).unwrap();
    let other = data.0.join("system/users/10/package-restrictions.xml");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    fs::write(&other, user_xml).unwrap();
    let mut store = Store::open(&data.0, &[0, 10]).unwrap().unwrap();
    assert!(
        store
            .commit_removed_package_restrictions("store", 0)
            .is_err()
    );
    let mut desired = store.state().settings.clone();
    let mut installers = install_sources::Installers::restore(&desired);
    desired.packages.remove(0);
    installers.remove("store", &mut desired);
    let mut wrong = desired.clone();
    wrong.packages[0].app_id = 10500;
    assert!(
        !store
            .commit_removed_package_setting(&wrong, "store")
            .unwrap_err()
            .committed
    );
    assert_eq!(fs::read(data.0.join("system/packages.xml")).unwrap(), input);
    store
        .commit_removed_package_setting(&desired, "store")
        .unwrap();
    assert_eq!(store.state().settings, desired);
    assert!(
        store.state().users.iter().all(|(_, u)| u
            .restrictions
            .packages
            .iter()
            .all(|(n, _)| n != "store"))
    );
    let root =
        aim_android_xml::read(&fs::read(data.0.join("system/packages.xml")).unwrap()).unwrap();
    assert!(root.children().any(|e| e.name == "future-owner"));
    assert!(
        root.children()
            .find(|e| e.name == "package")
            .unwrap()
            .children()
            .any(|e| e.name == "future-package")
    );
    assert!(
        store
            .commit_removed_package_restrictions("store", 0)
            .unwrap()
    );
    assert_eq!(fs::read(&other).unwrap(), user_xml);
    assert!(
        !store
            .commit_removed_package_restrictions("store", 0)
            .unwrap()
    );
    assert!(
        store
            .commit_removed_package_restrictions("store", 10)
            .unwrap()
    );
    let reopened = Store::open(&data.0, &[0, 10]).unwrap().unwrap();
    assert_eq!(reopened.state().settings, desired);
    for user in [0, 10] {
        let path = data
            .0
            .join(format!("system/users/{user}/package-restrictions.xml"));
        let bytes = fs::read(&path).unwrap();
        assert_eq!(fs::read(sibling(&path, ".reservecopy")).unwrap(), bytes);
        let root = aim_android_xml::read(&bytes).unwrap();
        assert_eq!(root.attrs, aim_android_xml::read(user_xml).unwrap().attrs);
        assert!(root.children().any(|e| e.name == "future-root"));
        assert_eq!(root.children().filter(|e| e.name == "pkg").count(), 1);
    }
}

#[test]
fn removed_setting_writer_accepts_live_installer_registry_history_and_preserves_unknown_groups() {
    let root = aim_android_xml::read(b"<packages><package name='store' codePath='/data/app/store' userId='10100'/><package name='app' codePath='/data/app/app' userId='10101' updateOwner='store' installerAttributionTag='tag'/><shared-user userId='10199'><future-group/></shared-user></packages>").unwrap();
    let original = super::super::settings::Settings::parse(&root).unwrap();
    let mut desired = original.clone();
    let mut installers = install_sources::Installers::restore(&desired);
    installers.add(&super::super::settings::InstallSource {
        originating_package: Some("store".into()),
        ..Default::default()
    });
    desired.packages.remove(0);
    installers.remove("store", &mut desired);
    let output = removal::replace(&root, &desired, "store").unwrap();
    assert_eq!(
        super::super::settings::Settings::parse(&output).unwrap(),
        desired
    );
    assert_eq!(
        output.children().find(|e| e.name == "shared-user"),
        root.children().find(|e| e.name == "shared-user")
    );
    let mut unregistered = original;
    unregistered.packages.remove(0);
    assert!(removal::replace(&root, &unregistered, "store").is_ok());
    let mut invalid = desired;
    invalid.packages[0].install_source.update_owner = Some("other".into());
    assert!(removal::replace(&root, &invalid, "store").is_err());
}

#[test]
fn removed_shared_setting_persistence_preserves_disabled_uid_reservation_and_external_writers() {
    for disabled in [false, true] {
        let data = Data::new();
        let restrictions = data.settings();
        fs::write(restrictions, b"<package-restrictions/>").unwrap();
        let input = format!(
            "<packages><shared-user name='group' userId='10100'><future-group/></shared-user><package name='app' codePath='/data/app/app' sharedUserId='10100'/>{}<future-owner/></packages>",
            if disabled {
                "<updated-package name='app' codePath='/system/app/app' sharedUserId='10100'/>"
            } else {
                ""
            }
        );
        let path = data.0.join("system/packages.xml");
        fs::write(&path, &input).unwrap();
        let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
        let mut desired = store.state().settings.clone();
        desired.packages.clear();
        if !disabled {
            desired.shared_users.clear();
        }
        let mut external = aim_android_xml::read(input.as_bytes()).unwrap();
        external.attrs.push(("external".into(), Value::Bool(true)));
        fs::write(&path, abx::write(&external).unwrap()).unwrap();
        assert!(
            !store
                .commit_removed_package_setting(&desired, "app")
                .unwrap_err()
                .committed
        );
        assert_eq!(
            aim_android_xml::read(&fs::read(&path).unwrap()).unwrap(),
            external
        );
        fs::write(&path, &input).unwrap();
        store
            .commit_removed_package_setting(&desired, "app")
            .unwrap();
        let reopened = Store::open(&data.0, &[0]).unwrap().unwrap();
        assert_eq!(reopened.state().settings, desired);
        assert_eq!(
            reopened.state().settings.shared_users.len(),
            usize::from(disabled)
        );
    }
}

#[test]
fn all_user_preferred_cleanup_reports_committed_users_on_later_writer_conflict() {
    let data = Data::new();
    data.settings();
    let input = b"<package-restrictions><preferred-activities><item name='removed/.Main' set='0'><filter/></item></preferred-activities></package-restrictions>";
    let path = |user| {
        data.0
            .join(format!("system/users/{user}/package-restrictions.xml"))
    };
    for user in [0, 10, 20] {
        fs::create_dir_all(path(user).parent().unwrap()).unwrap();
        fs::write(path(user), input).unwrap();
    }
    let mut store = Store::open(&data.0, &[0, 10, 20]).unwrap().unwrap();
    let mut external = aim_android_xml::read(input).unwrap();
    external
        .attrs
        .push(("external".into(), Value::String("writer".into())));
    fs::write(path(10), abx::write(&external).unwrap()).unwrap();
    let error = store
        .clear_all_package_preferred_activities(Some("removed"))
        .unwrap_err();
    assert_eq!(error.changed_users, [0]);
    assert!(!error.error.committed);
    let first = aim_android_xml::read(&fs::read(path(0)).unwrap()).unwrap();
    assert!(!super::super::preferred::has_preferred_resolver(&first));
    assert_eq!(
        aim_android_xml::read(&fs::read(path(10)).unwrap()).unwrap(),
        external
    );
    assert_eq!(fs::read(path(20)).unwrap(), input);
    // Retry continues after the successful user; no stale changed-user flags.
    fs::write(path(10), input).unwrap();
    assert_eq!(
        store
            .clear_all_package_preferred_activities(Some("removed"))
            .unwrap(),
        [10, 20]
    );
    assert!(
        store
            .clear_all_package_preferred_activities(Some("removed"))
            .unwrap()
            .is_empty()
    );
    // In-process empty resolvers remain known, but reopening empty XML does not
    // create a resolver, exactly as original Settings.readPreferredActivities.
    assert_eq!(
        store.preferred_users.iter().copied().collect::<Vec<_>>(),
        [0, 10, 20]
    );
    let reopened = Store::open(&data.0, &[0, 10, 20]).unwrap().unwrap();
    assert!(reopened.preferred_users.is_empty());
}

#[test]
fn update_owner_clearings_preserve_other_records_and_reject_unrelated_writes() {
    let data = Data::new();
    let restrictions = data.settings();
    fs::write(&restrictions, RESTRICTIONS).unwrap();
    let path = data.0.join("system/packages.xml");
    fs::write(&path, b"<packages future='keep'><package name='free' codePath='/data/app/free' userId='10100' updateOwner='installer' installer='store' installerUid='10102'><future-package/></package><package name='fixed' codePath='/system/app/fixed' userId='10101' updateOwner='fixed.installer'/><updated-package name='free' codePath='/system/app/free' userId='10100' updateOwner='factory.installer'/><future-owner/></packages>").unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let original = store.state().settings.clone();
    let users = store.state().users.clone();
    let mut desired = original.clone();
    desired.packages[0].install_source.update_owner = None;
    store.commit_update_owner_clearings(&desired).unwrap();
    assert_eq!(store.state().settings, desired);
    let reopened = Store::open(&data.0, &[0]).unwrap().unwrap();
    assert_eq!(reopened.state().settings, desired);
    assert_eq!(reopened.state().users, users);
    assert_eq!(fs::read(&restrictions).unwrap(), RESTRICTIONS);
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.starts_with(abx::MAGIC));
    assert_eq!(fs::read(sibling(&path, ".reservecopy")).unwrap(), bytes);
    let root = aim_android_xml::read(&bytes).unwrap();
    assert_eq!(root.string("future").as_deref(), Some("keep"));
    assert!(root.children().any(|e| e.name == "future-owner"));
    let free = root
        .children()
        .find(|e| e.name == "package" && e.string("name").as_deref() == Some("free"))
        .unwrap();
    assert!(free.string("updateOwner").is_none());
    assert!(free.children().any(|e| e.name == "future-package"));
    assert_eq!(
        root.children()
            .find(|e| e.name == "updated-package")
            .unwrap()
            .string("updateOwner")
            .as_deref(),
        Some("factory.installer")
    );
    let mut invalid = desired.clone();
    invalid.packages[0].install_source.update_owner = Some("new.installer".into());
    assert!(
        !store
            .commit_update_owner_clearings(&invalid)
            .unwrap_err()
            .committed
    );
    invalid = desired.clone();
    invalid.packages[0].install_source.installer = None;
    assert!(
        !store
            .commit_update_owner_clearings(&invalid)
            .unwrap_err()
            .committed
    );
    invalid = desired.clone();
    invalid.disabled_system_packages[0]
        .install_source
        .update_owner = Some("unexpected.installer".into());
    assert!(
        !store
            .commit_update_owner_clearings(&invalid)
            .unwrap_err()
            .committed
    );
    invalid = desired.clone();
    invalid.packages.pop();
    assert!(
        !store
            .commit_update_owner_clearings(&invalid)
            .unwrap_err()
            .committed
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(store.state().settings, desired);
    // Another writer invalidates the native document before any file writes.
    fs::write(&path, b"<packages/>").unwrap();
    assert!(
        !store
            .commit_update_owner_clearings(&desired)
            .unwrap_err()
            .committed
    );
    assert_eq!(fs::read(&path).unwrap(), b"<packages/>");
    assert_eq!(store.state().settings, desired);
}

#[test]
fn scanned_keyset_commit_reopens_and_rejects_unrelated_or_counter_changes() {
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    let key = |scalar: u8| {
        let mut encoded = vec![
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06,
            0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
        ];
        let mut bytes = [0; 32];
        bytes[31] = scalar;
        let secret = p256::SecretKey::from_slice(&bytes).unwrap();
        encoded.extend_from_slice(secret.public_key().to_encoded_point(false).as_bytes());
        encoded
    };
    let data = Data::new();
    let restrictions = data.settings();
    fs::write(&restrictions, RESTRICTIONS).unwrap();
    let path = data.0.join("system/packages.xml");
    fs::write(&path, b"<packages future='keep'><package name='a' codePath='/system/app/a' userId='10100'><future-package/></package><package name='b' codePath='/system/app/b' userId='10101'/><keyset-settings version='1'><keys><future-key/></keys><keysets><future-set/></keysets><lastIssuedKeyId value='9'/><lastIssuedKeySetId value='10'/><future-owner/></keyset-settings></packages>").unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let users = store.state().users.clone();
    let mut desired = store.state().settings.clone();
    let signer = key(1);
    let alias = key(2);
    key_sets::register(
        &mut desired,
        "a",
        &[signer.clone()],
        Some(&[("next".into(), vec![alias.clone()])]),
        &["next".into()],
    )
    .unwrap();
    key_sets::register(&mut desired, "b", &[signer], None, &[]).unwrap();
    store.commit_key_sets(&desired).unwrap();
    assert_eq!(desired.packages[0].key_set_data.proper_signing_key_set, 11);
    assert_eq!(desired.packages[1].key_set_data.proper_signing_key_set, 11);
    assert_eq!(desired.packages[0].key_set_data.upgrade_key_sets, [12]);
    assert_eq!(desired.key_sets.last_issued_key_id, 11);
    assert_eq!(desired.key_sets.last_issued_key_set_id, 12);
    let reopened = Store::open(&data.0, &[0]).unwrap().unwrap();
    assert_eq!(reopened.state().settings, desired);
    assert_eq!(reopened.state().users, users);
    assert_eq!(fs::read(&restrictions).unwrap(), RESTRICTIONS);
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.starts_with(abx::MAGIC));
    assert_eq!(fs::read(sibling(&path, ".reservecopy")).unwrap(), bytes);
    let root = aim_android_xml::read(&bytes).unwrap();
    assert_eq!(root.string("future").as_deref(), Some("keep"));
    let global = root
        .children()
        .find(|e| e.name == "keyset-settings")
        .unwrap();
    assert!(global.children().any(|e| e.name == "future-owner"));
    assert!(
        global
            .children()
            .find(|e| e.name == "keys")
            .unwrap()
            .children()
            .any(|e| e.name == "future-key")
    );
    assert!(
        global
            .children()
            .find(|e| e.name == "keysets")
            .unwrap()
            .children()
            .any(|e| e.name == "future-set")
    );
    assert!(
        root.children()
            .find(|e| e.name == "package")
            .unwrap()
            .children()
            .any(|e| e.name == "future-package")
    );
    key_sets::register(&mut desired, "a", &[alias], Some(&[]), &[]).unwrap();
    desired.key_sets.last_issued_key_id = 99;
    desired.key_sets.last_issued_key_set_id = 100;
    store.commit_key_sets(&desired).unwrap();
    assert_eq!(
        Store::open(&data.0, &[0])
            .unwrap()
            .unwrap()
            .state()
            .settings,
        desired
    );
    let bytes = fs::read(&path).unwrap();
    let mut invalid = desired.clone();
    invalid.packages[0].app_id += 1;
    assert!(!store.commit_key_sets(&invalid).unwrap_err().committed);
    invalid = desired.clone();
    invalid.key_sets.last_issued_key_id -= 1;
    assert!(!store.commit_key_sets(&invalid).unwrap_err().committed);
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn boot_removal_metadata_commit_retains_uid_users_legacy_domains_and_unknown_xml() {
    let data = Data::new();
    let restrictions = data.settings();
    fs::write(&restrictions, RESTRICTIONS).unwrap();
    let path = data.0.join("system/packages.xml");
    fs::write(&path, br#"<packages future='keep'>
      <package name='a' codePath='/system/app/a' userId='10100'><proper-signing-keyset identifier='1'/><defined-keyset alias='common' identifier='1'/><defined-keyset alias='only' identifier='2'/><upgrade-keyset identifier='2'/><future value='retained'/></package>
      <package name='b' codePath='/system/app/b' userId='10101'><proper-signing-keyset identifier='1'/></package>
      <keyset-settings version='1'><keys><public-key identifier='1' value='QQ=='/><public-key identifier='2' value='Qg=='/><future-key/></keys><keysets><keyset identifier='1'><key-id identifier='1'/></keyset><keyset identifier='2'><key-id identifier='1'/><key-id identifier='2'/></keyset></keysets><lastIssuedKeyId value='99'/><lastIssuedKeySetId value='100'/></keyset-settings>
      <domain-verifications><active><package-state packageName='a' id='00000000-0000-0000-0000-000000000001'/><package-state packageName='b' id='00000000-0000-0000-0000-000000000002'/><future-domain/></active><restored><package-state packageName='a' id='00000000-0000-0000-0000-000000000003'/></restored></domain-verifications>
      <domain-verifications-legacy><user-states packageName='a'><user-state userId='0' state='2'/></user-states></domain-verifications-legacy><future-owner/>
    </packages>"#).unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let old = store.state().clone();
    store.commit_removed_boot_metadata("a").unwrap();
    let state = store.state();
    assert_eq!(state.settings.packages[0].app_id, 10100);
    assert_eq!(
        state.settings.packages[0].key_set_data,
        super::super::settings::KeySetData::default()
    );
    assert_eq!(state.settings.packages[1], old.settings.packages[1]);
    assert_eq!(state.settings.key_sets.key_sets, [(1, vec![1])]);
    assert_eq!(state.settings.key_sets.public_keys, [(1, vec![b'A'])]);
    assert_eq!(state.settings.key_sets.last_issued_key_id, 99);
    assert_eq!(state.settings.key_sets.last_issued_key_set_id, 100);
    assert_eq!(state.settings.domain_verification.active.len(), 1);
    assert_eq!(state.settings.domain_verification.active[0].name, "b");
    assert!(state.settings.domain_verification.restored.is_empty());
    assert_eq!(
        state.settings.domain_verification.legacy,
        old.settings.domain_verification.legacy
    );
    assert_eq!(state.users, old.users);
    assert_eq!(fs::read(&restrictions).unwrap(), RESTRICTIONS);
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.starts_with(abx::MAGIC));
    assert_eq!(bytes, fs::read(sibling(&path, ".reservecopy")).unwrap());
    let root = aim_android_xml::read(&bytes).unwrap();
    assert_eq!(root.string("future").as_deref(), Some("keep"));
    assert!(root.children().any(|e| e.name == "future-owner"));
    let package = root
        .children()
        .find(|e| e.name == "package" && e.string("name").as_deref() == Some("a"))
        .unwrap();
    assert!(package.children().any(|e| e.name == "future"));
    let keys = root
        .children()
        .find(|e| e.name == "keyset-settings")
        .unwrap()
        .children()
        .find(|e| e.name == "keys")
        .unwrap();
    assert!(keys.children().any(|e| e.name == "future-key"));
    let reopened = Store::open(&data.0, &[0]).unwrap().unwrap();
    assert_eq!(reopened.state(), store.state());
    assert!(
        !store
            .commit_removed_boot_metadata("missing")
            .unwrap_err()
            .committed
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn native_library_commit_preserves_other_owners_and_clears_legacy_abi_fallback() {
    let data = Data::new();
    data.settings();
    let path = data.0.join("system/packages.xml");
    let original = b"<packages future='keep'><package name='example.app' codePath='/data/app/example' userId='10100' requiredCpuAbi='armeabi-v7a' nativeLibraryPath='/data/app-lib/old' pageSizeCompat='8'><future value='retained'/></package><updated-package name='example.app' codePath='/system/app/example.apk' userId='10100' requiredCpuAbi='armeabi-v7a'/><future-owner value='unchanged'/></packages>";
    fs::write(&path, original).unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let previous = store.state().clone();
    let mut desired = previous.settings.clone();
    let package = &mut desired.packages[0];
    package.legacy_native_library_path = Some("/data/app/example/lib".into());
    package.primary_cpu_abi = Some("arm64-v8a".into());
    package.secondary_cpu_abi = Some("armeabi-v7a".into());
    package.cpu_abi_override = Some("arm64-v8a".into());
    package.page_size_compat = 32;
    desired.disabled_system_packages[0].primary_cpu_abi = None;
    store.commit_native_library_metadata(&desired).unwrap();
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.starts_with(abx::MAGIC));
    assert_eq!(bytes, fs::read(sibling(&path, ".reservecopy")).unwrap());
    assert!(!data.0.join("system/packages-backup.xml").exists());
    let root = aim_android_xml::read(&bytes).unwrap();
    assert_eq!(root.string("future").as_deref(), Some("keep"));
    assert!(root.children().any(|e| e.name == "future-owner"));
    let package = root.children().find(|e| e.name == "package").unwrap();
    assert!(package.children().any(|e| e.name == "future"));
    assert_eq!(package.int("pageSizeCompat").unwrap(), Some(32));
    assert!(
        root.children()
            .filter(|e| matches!(e.name.as_str(), "package" | "updated-package"))
            .all(|e| e.string("requiredCpuAbi").is_none())
    );
    let reread = State::read(&data.0, &[0]).unwrap().unwrap();
    assert_eq!(reread.settings, desired);
    assert_eq!(store.state(), &reread);
    assert_eq!(reread.users, previous.users);
    desired.packages[0].legacy_native_library_path = None;
    desired.packages[0].primary_cpu_abi = None;
    desired.packages[0].secondary_cpu_abi = None;
    desired.packages[0].cpu_abi_override = None;
    desired.packages[0].page_size_compat = 0;
    store.commit_native_library_metadata(&desired).unwrap();
    let root = aim_android_xml::read(&fs::read(path).unwrap()).unwrap();
    let package = root.children().find(|e| e.name == "package").unwrap();
    for name in [
        "nativeLibraryPath",
        "primaryCpuAbi",
        "secondaryCpuAbi",
        "cpuAbiOverride",
        "requiredCpuAbi",
        "pageSizeCompat",
    ] {
        assert!(!package.attrs.iter().any(|(key, _)| key == name));
    }
    assert_eq!(
        State::read(&data.0, &[0]).unwrap().unwrap().settings,
        desired
    );
}

#[test]
fn native_library_commit_rejects_identity_signer_and_unrelated_changes_before_writes() {
    let data = Data::new();
    data.settings();
    let path = data.0.join("system/packages.xml");
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let previous = store.state().clone();
    let bytes = fs::read(&path).unwrap();
    for change in 0..8 {
        let mut desired = previous.settings.clone();
        desired.packages[0].primary_cpu_abi = Some("arm64-v8a".into());
        match change {
            0 => desired.packages[0].app_id += 1,
            1 => desired.packages[0].code_path.push_str("/other"),
            2 => desired.packages[0].version_code += 1,
            3 => desired.packages[0].signatures = Some(Default::default()),
            4 => desired.packages[0].page_size_compat = -1,
            5 => desired.packages[0].page_size_compat = 128,
            6 => desired.packages.clear(),
            _ => desired.packages.push(desired.packages[0].clone()),
        }
        assert!(
            !store
                .commit_native_library_metadata(&desired)
                .unwrap_err()
                .committed
        );
        assert_eq!(store.state(), &previous);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(!data.0.join("system/packages-backup.xml").exists());
        assert!(!sibling(&path, ".reservecopy").exists());
    }
    fs::write(&path, b"<packages/>").unwrap();
    let mut desired = previous.settings.clone();
    desired.packages[0].primary_cpu_abi = Some("arm64-v8a".into());
    assert!(
        !store
            .commit_native_library_metadata(&desired)
            .unwrap_err()
            .committed
    );
    assert_eq!(fs::read(path).unwrap(), b"<packages/>");
    assert_eq!(store.state(), &previous);
}

#[test]
fn writes_enabled_state_and_preserves_unmodelled_fields() {
    let data = Data::new();
    let path = data.settings();
    fs::write(&path, RESTRICTIONS).unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let enabled = Enabled {
        enabled: 2,
        last_disable_app_caller: Some("shell:2000".into()),
        enabled_components: ["example.app.Enabled".into()].into(),
        disabled_components: ["example.app.Disabled".into()].into(),
    };
    store.commit_enabled("example.app", 0, &enabled).unwrap();
    let reread = State::read(&data.0, &[0]).unwrap().unwrap();
    assert_eq!(store.state(), &reread);
    let user = &reread.users[0].1.restrictions.packages[0].1;
    assert!(user.stopped && user.installed);
    assert_eq!(user.first_install_time, 0x12);
    assert_eq!(user.enabled, 2);
    assert_eq!(
        user.disabled_components.as_deref(),
        Some(["example.app.Disabled".to_string()].as_slice())
    );
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.starts_with(abx::MAGIC));
    assert_eq!(bytes, fs::read(sibling(&path, ".reservecopy")).unwrap());
    let root = aim_android_xml::read(&bytes).unwrap();
    assert_eq!(
        root.children()
            .find(|e| e.name == "crossProfile-intent-filters"),
        aim_android_xml::read(RESTRICTIONS)
            .unwrap()
            .children()
            .find(|e| e.name == "crossProfile-intent-filters")
    );
    let package = root.children().find(|e| e.name == "pkg").unwrap();
    assert!(matches!(package.attr("enabled"), Some(Value::Int(2))));
    assert!(package.children().any(|e| e.name == "suspend-params"));
    let inode = guest_inode::read(&path).unwrap().unwrap();
    assert_eq!(
        inode,
        GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o660)
        }
    );
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o660
    );
    store
        .commit_enabled("example.app", 0, &Enabled::default())
        .unwrap();
    let root = aim_android_xml::read(&fs::read(path).unwrap()).unwrap();
    let package = root.children().find(|e| e.name == "pkg").unwrap();
    assert!(package.attr("enabled").is_none());
    assert!(package.attr("enabledCaller").is_none());
    assert!(!package.children().any(|e| e.name == "disabled-components"));
    let reread = State::read(&data.0, &[0]).unwrap().unwrap();
    assert_eq!(store.state(), &reread);
    let user = &reread.users[0].1.restrictions.packages[0].1;
    assert_eq!(user.enabled_components.as_deref(), Some([].as_slice()));
    assert_eq!(user.disabled_components.as_deref(), Some([].as_slice()));
}

#[test]
fn failed_main_write_preserves_the_old_backup() {
    let data = Data::new();
    let path = data.0.join("settings.xml");
    let backup = data.0.join("settings-backup.xml");
    fs::write(&path, b"old").unwrap();
    let error = write_with(&path, &backup, |file| {
        file.write_all(b"partial")?;
        Err(io::Error::other("interrupted"))
    })
    .unwrap_err();
    assert!(!error.committed);
    assert!(!path.exists());
    assert_eq!(fs::read(&backup).unwrap(), b"old");
    fs::write(&path, b"untrusted").unwrap();
    write_resilient(&path, &backup, b"new").unwrap();
    assert!(!backup.exists());
    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert_eq!(fs::read(sibling(&path, ".reservecopy")).unwrap(), b"new");
}

#[test]
fn reserve_failure_reports_that_main_committed() {
    let data = Data::new();
    let path = data.0.join("settings.xml");
    let backup = data.0.join("settings-backup.xml");
    fs::write(&path, b"old").unwrap();
    let error = write_with(&path, &backup, |file| {
        file.write_all(b"new")?;
        fs::remove_file(sibling(&path, ".reservecopy"))
    })
    .unwrap_err();
    assert!(error.committed);
    assert_eq!(fs::read(&path).unwrap(), b"new");
    assert!(!backup.exists());
}

#[test]
fn preserves_a_recovered_reserve_across_a_failed_write() {
    let data = Data::new();
    let path = data.settings();
    let backup = path.with_file_name("package-restrictions-backup.xml");
    fs::write(&path, b"bad xml").unwrap();
    fs::write(sibling(&path, ".reservecopy"), RESTRICTIONS).unwrap();
    let store = Store::open(&data.0, &[0]).unwrap().unwrap();
    prepare(&path, &backup, &store.restrictions[&0]).unwrap();
    let error = write_with(&path, &backup, |_| Err(io::Error::other("interrupted"))).unwrap_err();
    assert!(!error.committed);
    assert_eq!(fs::read(backup).unwrap(), RESTRICTIONS);
    assert_eq!(State::read(&data.0, &[0]).unwrap().unwrap(), *store.state());
}

#[test]
fn refuses_an_external_writer_and_unknown_targets() {
    let data = Data::new();
    let path = data.settings();
    fs::write(&path, RESTRICTIONS).unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    assert!(
        store
            .commit_enabled("missing", 0, &Enabled::default())
            .is_err()
    );
    assert!(
        store
            .commit_enabled("example.app", 10, &Enabled::default())
            .is_err()
    );
    fs::write(&path, b"<package-restrictions />").unwrap();
    let error = store
        .commit_enabled("example.app", 0, &Enabled::default())
        .unwrap_err();
    assert!(!error.committed);
    assert!(error.message.contains("outside the native owner"));
    assert_eq!(fs::read(path).unwrap(), b"<package-restrictions />");
}

#[test]
fn signature_commit_reindexes_certificates_and_retains_unrelated_documents() {
    let data = Data::new();
    let restrictions = data.settings();
    fs::write(&restrictions, RESTRICTIONS).unwrap();
    let path = data.0.join("system/packages.xml");
    fs::write(&path, b"<packages><package name='example.app' codePath='/data/app/example' userId='10100' custom='keep'><keep value='nested'/><sigs count='1' schemeVersion='3'><cert index='7' key='aa'/></sigs></package><shared-user name='group' userId='1000'><sigs count='1' schemeVersion='3'><cert index='7'/></sigs></shared-user><unknown attr='retain'/></packages>").unwrap();
    let old_root = aim_android_xml::read(&fs::read(&path).unwrap()).unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let mut desired = store.state.settings.clone();
    desired.packages[0].signatures = Some(super::super::settings::Signatures {
        scheme_version: 3,
        signatures: vec![vec![0xaa], vec![0xbb]],
        past_signatures: Some(vec![(vec![0xcc], 3), (vec![0xaa], 1), (vec![0xbb], 0)]),
        ..Default::default()
    });
    desired.shared_users[0].signatures = Some(super::super::settings::Signatures {
        scheme_version: 3,
        signatures: vec![vec![0xbb]],
        past_signatures: Some(vec![(vec![0xcc], 3), (vec![0xbb], 0)]),
        ..Default::default()
    });
    store.commit_signatures(&desired).unwrap();
    assert_eq!(store.state.settings, desired);
    assert_eq!(store.state(), &State::read(&data.0, &[0]).unwrap().unwrap());
    assert_eq!(fs::read(&restrictions).unwrap(), RESTRICTIONS);
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.starts_with(abx::MAGIC));
    assert_eq!(bytes, fs::read(sibling(&path, ".reservecopy")).unwrap());
    let root = aim_android_xml::read(&bytes).unwrap();
    assert_eq!(
        root.children().find(|e| e.name == "unknown"),
        old_root.children().find(|e| e.name == "unknown")
    );
    let package = root.children().find(|e| e.name == "package").unwrap();
    assert_eq!(package.string("custom").as_deref(), Some("keep"));
    assert!(package.children().any(|e| e.name == "keep"));
    let sigs = package.children().find(|e| e.name == "sigs").unwrap();
    let certs: Vec<_> = sigs.children().filter(|e| e.name == "cert").collect();
    assert_eq!(certs[0].int("index").unwrap(), Some(0));
    assert_eq!(certs[1].int("index").unwrap(), Some(1));
    let group = root.children().find(|e| e.name == "shared-user").unwrap();
    let cert = group
        .children()
        .find(|e| e.name == "sigs")
        .unwrap()
        .children()
        .next()
        .unwrap();
    assert_eq!(cert.int("index").unwrap(), Some(1));
    assert!(cert.attr("key").is_none());
    let persisted = fs::read(&path).unwrap();
    let mut cleared = desired.clone();
    cleared.packages[0].signatures = None;
    assert!(!store.commit_signatures(&cleared).unwrap_err().committed);
    assert_eq!(fs::read(&path).unwrap(), persisted);
    assert_eq!(store.state.settings, desired);
}

#[test]
fn signature_commit_refuses_metadata_changes_and_concurrent_writers() {
    let data = Data::new();
    data.settings();
    let path = data.0.join("system/packages.xml");
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let before = fs::read(&path).unwrap();
    let mut desired = store.state.settings.clone();
    desired.packages[0].app_id += 1;
    assert!(!store.commit_signatures(&desired).unwrap_err().committed);
    assert_eq!(fs::read(&path).unwrap(), before);
    desired = store.state.settings.clone();
    fs::write(
        &path,
        b"<packages><package name='other' codePath='/data/app/other' userId='10100'/></packages>",
    )
    .unwrap();
    assert!(!store.commit_signatures(&desired).unwrap_err().committed);
    assert_eq!(store.state.settings, desired);
    assert!(!sibling(&path, ".reservecopy").exists());
}

const SHARED_MIGRATION: &[u8] = b"<packages><shared-user name='leaving' userId='10100'><sigs count='1' schemeVersion='3'><cert index='7' key='aa'/></sigs></shared-user><package name='example.app' codePath='/data/app/example' sharedUserId='10100' custom='keep'><keep value='nested'/><sigs count='1' schemeVersion='3'><cert index='7'/></sigs></package><updated-package name='example.app' codePath='/system/app/example' sharedUserId='10100' custom='old'/><package name='other' codePath='/data/app/other' sharedUserId='10101'><sigs count='1' schemeVersion='3'><cert index='8' key='bb'/></sigs></package><shared-user name='retained' userId='10101'><sigs count='1' schemeVersion='3'><cert index='8'/></sigs></shared-user><unknown attr='retain'/></packages>";

#[test]
fn shared_uid_persistence_preserves_ids_metadata_and_rebuilds_removed_certificate_definitions() {
    let data = Data::new();
    let restrictions = data.settings();
    fs::write(&restrictions, RESTRICTIONS).unwrap();
    let path = data.0.join("system/packages.xml");
    fs::write(&path, SHARED_MIGRATION).unwrap();
    let original = aim_android_xml::read(SHARED_MIGRATION).unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let mut desired = store.state.settings.clone();
    desired.packages[0].shared_user = false;
    desired.disabled_system_packages[0].shared_user = false;
    desired.shared_users.remove(0);
    let snapshot = store.state.clone();
    assert!(!store.commit_signatures(&desired).unwrap_err().committed);
    store.commit_shared_uid_migrations(&desired).unwrap();
    assert_eq!(store.state.settings, desired);
    assert_eq!(store.state(), &State::read(&data.0, &[0]).unwrap().unwrap());
    assert!(snapshot.settings.packages[0].shared_user);
    assert_eq!(fs::read(&restrictions).unwrap(), RESTRICTIONS);
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.starts_with(abx::MAGIC));
    assert_eq!(bytes, fs::read(sibling(&path, ".reservecopy")).unwrap());
    assert!(!data.0.join("system/packages-backup.xml").exists());
    let root = aim_android_xml::read(&bytes).unwrap();
    assert_eq!(
        root.children().find(|e| e.name == "unknown"),
        original.children().find(|e| e.name == "unknown")
    );
    for kind in ["package", "updated-package"] {
        let package = root
            .children()
            .find(|e| e.name == kind && e.string("name").as_deref() == Some("example.app"))
            .unwrap();
        assert!(package.attr("sharedUserId").is_none());
        assert_eq!(package.int("userId").unwrap(), Some(10100));
        assert_eq!(
            package.string("custom"),
            original
                .children()
                .find(|e| e.name == kind)
                .unwrap()
                .string("custom")
        );
    }
    let package = root.children().find(|e| e.name == "package").unwrap();
    assert!(package.children().any(|e| e.name == "keep"));
    let cert = package
        .children()
        .find(|e| e.name == "sigs")
        .unwrap()
        .children()
        .next()
        .unwrap();
    assert_eq!(cert.bytes_hex("key").unwrap(), Some(vec![0xaa]));
    assert_eq!(cert.int("index").unwrap(), Some(0));
    assert_eq!(guest_inode::read(&path).unwrap().unwrap().uid, Some(1000));
}

#[test]
fn shared_uid_persistence_rejects_partial_migration_remapping_and_cleared_signers_atomically() {
    let data = Data::new();
    data.settings();
    let path = data.0.join("system/packages.xml");
    fs::write(&path, SHARED_MIGRATION).unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let before = store.state.clone();
    let mut valid = before.settings.clone();
    valid.packages[0].shared_user = false;
    valid.disabled_system_packages[0].shared_user = false;
    valid.shared_users.remove(0);
    let mut variants = vec![];
    let mut bad = valid.clone();
    bad.packages[0].app_id += 1;
    variants.push(bad);
    let mut bad = valid.clone();
    bad.disabled_system_packages[0].shared_user = true;
    variants.push(bad);
    let mut bad = valid.clone();
    bad.packages[0].shared_user = true;
    variants.push(bad);
    let mut bad = valid.clone();
    bad.packages[0].signatures = None;
    variants.push(bad);
    let mut bad = valid.clone();
    bad.shared_users[0].signatures = None;
    variants.push(bad);
    let mut bad = valid.clone();
    bad.packages[0].code_path.push_str("/changed");
    variants.push(bad);
    for desired in variants {
        assert!(
            !store
                .commit_shared_uid_migrations(&desired)
                .unwrap_err()
                .committed
        );
        assert_eq!(store.state(), &before);
        assert_eq!(fs::read(&path).unwrap(), SHARED_MIGRATION);
        assert!(!sibling(&path, ".reservecopy").exists());
    }
    let mut multiple = before.settings.clone();
    let mut extra = multiple.packages[0].clone();
    extra.name = "second.member".into();
    multiple.packages.push(extra);
    // The candidate cannot introduce a new member while deleting its group.
    multiple.shared_users.remove(0);
    assert!(
        !store
            .commit_shared_uid_migrations(&multiple)
            .unwrap_err()
            .committed
    );
    assert_eq!(store.state(), &before);
}

#[test]
fn shared_uid_writer_rejects_multi_member_and_empty_group_deletion() {
    for added in [
        "<package name='second' codePath='/data/app/second' sharedUserId='10100'/>",
        "<updated-package name='second' codePath='/system/app/second' sharedUserId='10100'/>",
        "<shared-user name='empty' userId='10102'/>",
    ] {
        let data = Data::new();
        data.settings();
        let path = data.0.join("system/packages.xml");
        let document = String::from_utf8(SHARED_MIGRATION.to_vec())
            .unwrap()
            .replace("</packages>", &format!("{added}</packages>"));
        fs::write(&path, &document).unwrap();
        let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
        let snapshot = store.state.clone();
        let mut desired = snapshot.settings.clone();
        let group = if added.contains("name='empty'") {
            "empty"
        } else {
            "leaving"
        };
        if group == "leaving" {
            for p in desired
                .packages
                .iter_mut()
                .chain(&mut desired.disabled_system_packages)
                .filter(|p| p.app_id == 10100)
            {
                p.shared_user = false;
            }
        }
        desired.shared_users.retain(|g| g.name != group);
        assert!(
            !store
                .commit_shared_uid_migrations(&desired)
                .unwrap_err()
                .committed
        );
        assert_eq!(store.state(), &snapshot);
        assert_eq!(fs::read(&path).unwrap(), document.as_bytes());
    }
}
