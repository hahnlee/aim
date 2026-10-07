use super::super::test_certificates::{certificate, xml as certificate_xml};
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

#[test]
fn current_leaving_bits_are_not_persisted_or_invented_by_disk_readers() {
    let root = aim_android_xml::read(b"<packages><package name='fixture' codePath='/data/app/fixture' userId='10100' leavingSharedUser='true'/><updated-package name='fixture' codePath='/system/app/fixture' userId='10100'/></packages>").unwrap();
    let saved = crate::package::settings::Settings::parse(&root).unwrap();
    assert_eq!(saved.packages[0].leaving_shared_user, Some(false));
    assert_eq!(
        saved.disabled_system_packages[0].leaving_shared_user,
        Some(false)
    );
    let mut current = saved.clone();
    current.packages[0].leaving_shared_user = Some(true);
    current.disabled_system_packages[0].leaving_shared_user = None;
    let output = native_libraries::replace(&root, &current).unwrap();
    assert_eq!(
        crate::package::settings::Settings::parse(&output).unwrap(),
        saved
    );
    assert_eq!(current.packages[0].leaving_shared_user, Some(true));
    assert_eq!(
        current.disabled_system_packages[0].leaving_shared_user,
        None
    );
}
#[test]
fn duplicate_factory_import_opens_one_owner_without_rewriting_the_source() {
    let data = Data::new();
    data.settings();
    let path = data.0.join("system/packages.xml");
    let bytes = b"<packages><package name='example.app' codePath='/data/app/example' userId='10100'/><updated-package name='example.app' codePath='/system/priv-app/old' userId='10100' version='1'/><updated-package name='example.app' codePath='/system/app/new' userId='10100' version='2'/></packages>";
    std::fs::write(&path, bytes).unwrap();
    let store = Store::open(&data.0, &[]).unwrap().unwrap();
    let factories = &store.state().settings.disabled_system_packages;
    assert_eq!(factories.len(), 1);
    assert_eq!(factories[0].version_code, 2);
    assert_eq!(factories[0].code_path, "/system/app/new");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn boot_frontend_completion_precedes_related_files_and_preserves_input_on_failure() {
    use crate::package::{settings::{Settings, Version, PackageReadAttempt}, owner::app_ids::AppIds};
    use recovery::{ReadStage, ReadError, Event};
    let data = Data::new(); let restrictions = data.settings();
    let path = data.0.join("system/packages.xml");
    let bytes = b"<packages><package name='pending' codePath='/pending' sharedUserId='10100' domainSetId='00000000-0000-0000-0000-000000000001'/><shared-user name='group' userId='10100'/></packages>";
    fs::write(&path, bytes).unwrap(); fs::write(&restrictions, b"bad related input").unwrap();
    let mut settings = Settings::default(); let mut ids = AppIds::default(); let mut attempt = PackageReadAttempt::default();
    let current = Version { sdk_version: 36, database_version: 3, ..Default::default() };
    let error = recovery::Plan::inspect(&data.0).unwrap().recover_boot_frontend(&[0], &mut settings, &current, |stage, state| match stage {
        ReadStage::File(bytes) => state.read_document(bytes, |state, reader, start| {
            match start.name.as_str() {
                "package" => { state.read_package(reader,start,&mut ids,&mut attempt,|_,_,_|Ok(false))?; }
                "shared-user" => { state.read_shared_user(reader,start,&mut ids,&mut attempt,|_,_,_|Ok(false))?; }
                _ => return Ok(false),
            } Ok(true)
        }),
        ReadStage::Complete => {
            assert_eq!(state.versions.len(), 2);
            attempt.resolve_pending(state, &mut ids, |_,_,_,_| Err("binding owner unavailable".into())).map_err(ReadError::Owner)?;
            Ok(None)
        }
    }).err().unwrap();
    assert!(matches!(error.events.last(), Some(Event::CompletionFailed(message)) if message == "binding owner unavailable"));
    assert!(settings.packages.is_empty()); assert_eq!(attempt.pending.len(), 1);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let (store, _) = recovery::Plan::inspect(&data.0).unwrap().recover_boot_frontend(&[0], &mut settings, &current, |stage, state| match stage {
        ReadStage::File(bytes) => state.read_document(bytes, |_,reader,_| { crate::package::settings::skip(reader)?; Ok(true) }),
        ReadStage::Complete => {
            attempt.resolve_pending(state, &mut ids, |_,_,_,_| Ok(())).map_err(ReadError::Owner)?;
            fs::write(&restrictions, b"<package-restrictions><pkg name='pending' inst='false'/></package-restrictions>").unwrap();
            Ok(None)
        }
    }).unwrap();
    assert_eq!(store.state().settings.packages[0].name, "pending");
    assert!(!store.state().users[0].1.restrictions.packages[0].1.installed);
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[test]
fn false_boot_continuation_skips_package_user_files_but_retry_continues_them() {
    use crate::package::settings::{Settings, Version};
    let data = Data::new(); let restrictions = data.settings();
    let path = data.0.join("system/packages.xml");
    fs::write(&restrictions, b"malformed package user file").unwrap();
    let runtime = data.0.join("misc_de/0").join(crate::package::PERMISSION_DIR);
    fs::create_dir_all(&runtime).unwrap();
    fs::write(runtime.join("runtime-permissions.xml"), b"malformed runtime user file").unwrap();
    let current = Version { sdk_version: 36, database_version: 3, ..Default::default() };
    for input in [None, Some(b" ".as_slice())] {
        let mut state = Settings::default();
        if let Some(bytes) = input { fs::write(&path, bytes).unwrap(); } else { fs::remove_file(&path).ok(); }
        let (mut store, report) = recovery::Plan::inspect(&data.0).unwrap().recover_boot_frontend(&[0], &mut state, &current, |stage, state| match stage {
            recovery::ReadStage::File(bytes) => state.read_document(bytes, |_,_,_| Ok(false)),
            recovery::ReadStage::Complete => panic!("false continuation finalized"),
        }).unwrap();
        assert!(report.first_boot); assert_eq!(state.versions.len(), 2);
        assert!(store.state().users[0].1.runtime_permissions.is_none());
        assert!(store.state().users[0].1.restrictions.packages.is_empty());
        let error = store.clear_package_preferred_activities(0, None).unwrap_err();
        assert!(!error.committed); assert!(error.message.contains("not restored"));
        assert_eq!(fs::read(&restrictions).unwrap(), b"malformed package user file");
    }
    fs::write(&path, b"<packages><version sdkVersion='33' databaseVersion='bad'/></packages>").unwrap();
    let mut state = Settings::default(); let mut completed = false;
    let error = recovery::Plan::inspect(&data.0).unwrap().recover_boot_frontend(&[0], &mut state, &current, |stage, state| match stage {
        recovery::ReadStage::File(bytes) => state.read_document(bytes, |_,_,_| Ok(false)),
        recovery::ReadStage::Complete => { completed = true; Ok(None) }
    }).err().unwrap();
    assert!(completed); assert!(!path.exists());
    assert!(error.message.contains("XML") || error.message.contains("start") || error.message.contains("root"), "{}", error.message);
    assert_eq!(fs::read(&restrictions).unwrap(), b"malformed package user file");
}

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
        .suspensions
        .as_ref()
        .unwrap()[0]
        .clone();
    assert_eq!(
        suspension.params.as_ref().unwrap().app_extras,
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
        reread.users[0].1.restrictions.packages[0]
            .1
            .suspensions
            .as_ref()
            .unwrap()[0],
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
    let input = certificate_xml(b"<packages future='keep'><package name='store' codePath='/data/app/store' userId='10100'><sigs count='1' schemeVersion='3'><cert index='0' key='0102'/></sigs></package><package name='app' codePath='/data/app/app' userId='10101' installer='store' installerUid='10100' installInitiator='store' installOriginator='store' updateOwner='store' installerAttributionTag='tag'><sigs count='1' schemeVersion='3'><cert index='0'/><pastSigs count='1'><cert index='0' flags='7'/></pastSigs></sigs><install-initiator-sigs count='1' schemeVersion='3'><cert index='0'/></install-initiator-sigs><future-package/></package><updated-package name='app' codePath='/system/app/app' userId='10101' installer='store'/><future-owner/></packages>");
    fs::write(data.0.join("system/packages.xml"), &input).unwrap();
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
    let mut null_alias = desired.clone();
    null_alias.packages[0].key_set_data.defined_key_sets[0].0 = None;
    assert!(!store.commit_key_sets(&null_alias).unwrap_err().committed);
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(store.state().settings, desired);

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
      <keyset-settings version='1'><keys><public-key identifier='1' value='MFwwDQYJKoZIhvcNAQEBBQADSwAwSAJBAIBVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVQMCAwEAAQ=='/><public-key identifier='2' value='MFwwDQYJKoZIhvcNAQEBBQADSwAwSAJBAIBVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVQcCAwEAAQ=='/><future-key/></keys><keysets><keyset identifier='1'><key-id identifier='1'/></keyset><keyset identifier='2'><key-id identifier='1'/><key-id identifier='2'/></keyset></keysets><lastIssuedKeyId value='99'/><lastIssuedKeySetId value='100'/></keyset-settings>
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
    assert_eq!(
        state.settings.key_sets.public_keys,
        [(
            1,
            include_bytes!("../../../tests/fixtures/settings-public-key-1.der").to_vec()
        )]
    );
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
        enabled_components: ["B".into(), "Aa".into(), "BB".into()].into(),
        disabled_components: ["example.app.Disabled".into()].into(),
    };
    store.commit_enabled("example.app", 0, &enabled).unwrap();
    let reread = State::read(&data.0, &[0]).unwrap().unwrap();
    assert_eq!(store.state(), &reread);
    let user = &reread.users[0].1.restrictions.packages[0].1;
    assert!(user.stopped && user.installed);
    assert_eq!(
        user.enabled_components.as_deref(),
        Some(["B".to_string(), "Aa".to_string(), "BB".to_string()].as_slice())
    );
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
    fs::write(&path, certificate_xml(b"<packages><package name='example.app' codePath='/data/app/example' userId='10100' custom='keep'><keep value='nested'/><sigs count='1' schemeVersion='3'><cert index='7' key='aa'/></sigs></package><shared-user name='group' userId='1000'><sigs count='1' schemeVersion='3'><cert index='7'/></sigs></shared-user><unknown attr='retain'/></packages>")).unwrap();
    let old_root = aim_android_xml::read(&fs::read(&path).unwrap()).unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let mut desired = store.state.settings.clone();
    desired.packages[0].signatures = Some(super::super::settings::Signatures {
        scheme_version: 3,
        signatures: vec![certificate(0), certificate(1)],
        past_signatures: Some(vec![
            (certificate(2), 3),
            (certificate(0), 1),
            (certificate(1), 0),
        ]),
        ..Default::default()
    });
    desired.shared_users[0].signatures = Some(super::super::settings::Signatures {
        scheme_version: 3,
        signatures: vec![certificate(1)],
        past_signatures: Some(vec![(certificate(2), 3), (certificate(1), 0)]),
        ..Default::default()
    });
    derive_public_keys(&mut desired);
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
    fs::write(&path, certificate_xml(SHARED_MIGRATION)).unwrap();
    let original = aim_android_xml::read(&certificate_xml(SHARED_MIGRATION)).unwrap();
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
    assert_eq!(cert.bytes_hex("key").unwrap(), Some(certificate(0)));
    assert_eq!(cert.int("index").unwrap(), Some(0));
    assert_eq!(guest_inode::read(&path).unwrap().unwrap().uid, Some(1000));
}

#[test]
fn shared_uid_persistence_rejects_partial_migration_remapping_and_cleared_signers_atomically() {
    let data = Data::new();
    data.settings();
    let path = data.0.join("system/packages.xml");
    fs::write(&path, certificate_xml(SHARED_MIGRATION)).unwrap();
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
        assert_eq!(fs::read(&path).unwrap(), certificate_xml(SHARED_MIGRATION));
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
        let document = String::from_utf8(certificate_xml(SHARED_MIGRATION))
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

#[test]
fn imported_keyset_references_survive_commits_until_restart() {
    let data = Data::new();
    data.settings();
    let path = data.0.join("system/packages.xml");
    let input = b"<packages><package name='a' codePath='/data/app/a' userId='10100'><proper-signing-keyset identifier='1'/><proper-signing-keyset identifier='1'/><defined-keyset alias='same' identifier='1'/><defined-keyset alias='same' identifier='1'/><defined-keyset alias='replace' identifier='2'/><defined-keyset alias='replace' identifier='1'/></package><keyset-settings version='1'><keys><public-key identifier='1' value='MFwwDQYJKoZIhvcNAQEBBQADSwAwSAJBAIBVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVQMCAwEAAQ=='/><public-key identifier='2' value='MFwwDQYJKoZIhvcNAQEBBQADSwAwSAJBAIBVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVVQcCAwEAAQ=='/></keys><keysets><keyset identifier='1'><key-id identifier='1'/></keyset><keyset identifier='2'><key-id identifier='2'/></keyset></keysets><lastIssuedKeyId value='2'/><lastIssuedKeySetId value='2'/></keyset-settings></packages>";
    fs::write(&path, input).unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    assert_eq!(
        store.state.settings.key_sets.reference_counts,
        Some(BTreeMap::from([(1, 5), (2, 1)]))
    );
    store.commit_removed_boot_metadata("a").unwrap();
    let live = store.state.settings.clone();
    assert_eq!(
        live.key_sets.reference_counts,
        Some(BTreeMap::from([(1, 2), (2, 1)]))
    );
    assert_eq!(live.key_sets.key_sets.len(), 2);
    store.commit_update_owner_clearings(&live).unwrap();
    assert_eq!(store.state.settings, live);
    let mut reopened = Store::open(&data.0, &[0])
        .unwrap()
        .unwrap()
        .state
        .settings
        .clone();
    assert_eq!(
        reopened.key_sets.reference_counts,
        Some(BTreeMap::from([(1, 0), (2, 0)]))
    );
    key_sets::restore(&mut reopened).unwrap();
    assert!(reopened.key_sets.key_sets.is_empty());
    assert!(reopened.key_sets.public_keys.is_empty());
    assert_eq!(reopened.key_sets.last_issued_key_set_id, 2);
}

#[test]
fn initiating_signatures_share_the_reindexed_package_and_group_table() {
    let data = Data::new();
    data.settings();
    let path = data.0.join("system/packages.xml");
    fs::write(&path, certificate_xml(b"<packages><package name='example.app' codePath='/data/app/example' userId='10100' installInitiator='installer'><sigs count='1' schemeVersion='3'><cert index='0' key='aa'/></sigs><install-initiator-sigs count='1' schemeVersion='3'><cert index='0'/><pastSigs count='1'><cert index='1' key='bb' flags='7'/></pastSigs></install-initiator-sigs></package><shared-user name='group' userId='1000'><sigs count='1' schemeVersion='3'><cert index='1'/></sigs></shared-user></packages>")).unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let mut desired = store.state.settings.clone();
    desired.packages[0].signatures.as_mut().unwrap().signatures = vec![certificate(2)];
    derive_public_keys(&mut desired);
    store.commit_signatures(&desired).unwrap();
    assert_eq!(store.state.settings, desired);
    assert_eq!(
        Store::open(&data.0, &[0]).unwrap().unwrap().state.settings,
        desired
    );
    let root = aim_android_xml::read(&fs::read(path).unwrap()).unwrap();
    let package = root.children().find(|e| e.name == "package").unwrap();
    let initiator = package
        .children()
        .find(|e| e.name == "install-initiator-sigs")
        .unwrap();
    assert_eq!(
        initiator.children().next().unwrap().int("index").unwrap(),
        Some(1)
    );
    assert_eq!(
        initiator
            .children()
            .next()
            .unwrap()
            .bytes_hex("key")
            .unwrap(),
        Some(certificate(0))
    );
    let group = root.children().find(|e| e.name == "shared-user").unwrap();
    assert_eq!(
        group
            .children()
            .next()
            .unwrap()
            .children()
            .next()
            .unwrap()
            .int("index")
            .unwrap(),
        Some(2)
    );
}

#[test]
fn captured_scan_settings_replace_package_metadata_and_reject_uncommitted_global_owners() {
    let data = Data::new();
    data.settings();
    let path = data.0.join("system/packages.xml");
    fs::write(&path, b"<packages><package name='example.app' codePath='/data/app/old' userId='10100' domainSetId='00000000-0000-0000-0000-000000000001' requiredCpuAbi='old' custom='keep'><keep value='nested'/></package><keyset-settings version='1'><keys/><keysets/><lastIssuedKeyId value='0'/><lastIssuedKeySetId value='0'/></keyset-settings><extension value='global'/></packages>").unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let mut owner =
        super::super::scan::SigningScan::new(&Default::default(), &store.state.settings, 36)
            .unwrap();
    let setting = &mut owner.settings.packages[0];
    setting.code_path = "/data/app/new".into();
    setting.primary_cpu_abi = None;
    setting.cpu_abi_override = Some("arm64-v8a".into());
    setting.flags = 2;
    setting.private_flags = 8;
    setting.version_code = i64::MAX;
    setting.last_modified_time = -1;
    setting.last_update_time = i64::MAX;
    setting.target_sdk_version = 36;
    setting.domain_set_id = Some("00000000-0000-0000-0000-000000000002".into());
    setting.app_metadata_source = 1;
    setting.app_metadata_file_path = Some("/data/app/new/metadata.pb".into());
    setting.restrict_update_hash = Some(vec![1, 2, 3]);
    setting.install_source.installer = Some("installer".into());
    setting.install_source.installer_uid = 10123;
    setting.install_source.package_source = 3;
    setting.loading_progress = 0.5;
    setting.loading_completed_time = 19;
    setting.uses_sdk_libraries = vec![super::super::settings::UsesSdkLibrary {
        name: "sdk".into(),
        version_major: 17,
        optional: false,
    }];
    setting.uses_static_libraries = vec![("static".into(), 23)];
    setting.split_versions = vec![("archived-split".into(), 7)];
    setting.add_mime_types("types".into(), ["text/plain".into()]);
    owner.settings.find_or_create_version(None).database_version = 3;
    let capture = |owner| {
        super::super::scan_snapshot::Store::new(owner, super::usage::Usage::new(["example.app"]))
            .unwrap()
            .capture()
    };
    let snapshot = capture(owner.clone());
    store.commit_scan_settings(&snapshot).unwrap();
    let mut expected = owner.settings.clone();
    expected.shared_users = owner
        .identities
        .ordered_shared_users()
        .unwrap()
        .into_iter()
        .map(|(name, group)| super::super::settings::SharedUser {
            name: name.into(),
            app_id: group.app_id,
            flags: 0,
            signatures: group.signatures.clone(),
        })
        .collect();
    assert_eq!(expected.shared_users.len(), 9);
    assert_eq!(
        signing::persisted(store.state.settings.clone()),
        signing::persisted(expected)
    );
    let bytes = fs::read(&path).unwrap();
    let root = aim_android_xml::read(&bytes).unwrap();
    let package = root.children().find(|e| e.name == "package").unwrap();
    assert_eq!(package.string("custom").as_deref(), Some("keep"));
    assert!(package.attr("requiredCpuAbi").is_none());
    assert!(package.children().any(|e| e.name == "keep"));
    assert!(root.children().any(|e| e.name == "extension"));
    assert_eq!(
        Store::open(&data.0, &[0]).unwrap().unwrap().state.settings,
        store.state.settings
    );
    let before = store.state.settings.clone();
    let mut foreign_global = owner.clone();
    foreign_global.settings.verifier = Some("uncommitted".into());
    assert!(
        !store
            .commit_scan_settings(&capture(foreign_global))
            .unwrap_err()
            .committed
    );
    let mut null_mime = owner.clone();
    null_mime.settings.packages[0].add_nullable_mime_types(None, [Some("text/plain".into())]);
    assert!(
        !store
            .commit_scan_settings(&capture(null_mime))
            .unwrap_err()
            .committed
    );
    let mut no_domain = owner.clone();
    no_domain.settings.packages[0].domain_set_id = None;
    assert!(
        !store
            .commit_scan_settings(&capture(no_domain))
            .unwrap_err()
            .committed
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(store.state.settings, before);
    // A complete but foreign document is not adopted by the retained owner.
    fs::write(&path, b"<packages><package name='foreign' codePath='/data/app/foreign' userId='10199'/></packages>").unwrap();
    assert!(!store.commit_scan_settings(&snapshot).unwrap_err().committed);
    assert_eq!(store.state.settings, before);
}

#[test]
fn scan_settings_use_array_map_package_order_without_changing_captured_slots() {
    let data = Data::new();
    data.settings();
    let path = data.0.join("system/packages.xml");
    fs::write(&path, b"<packages/>").unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let names = ["z", "BB", "Aa", "negative.hash.owner"];
    let settings = super::super::settings::Settings {
        packages: names
            .iter()
            .enumerate()
            .map(|(index, name)| super::super::settings::Package {
                name: (*name).into(),
                code_path: format!("/data/app/{name}"),
                app_id: 10100 + index as i32,
                category_hint: -1,
                domain_set_id: Some("00000000-0000-0000-0000-000000000001".into()),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let mut owner =
        super::super::scan::SigningScan::new(&Default::default(), &settings, 36).unwrap();
    let capture = |owner| {
        super::super::scan_snapshot::Store::new(owner, super::usage::Usage::new(names))
            .unwrap()
            .capture()
    };
    let old = capture(owner.clone());
    store.commit_scan_settings(&old).unwrap();
    let ordered = |settings: &super::super::settings::Settings| {
        settings
            .packages
            .iter()
            .map(|package| package.name.clone())
            .collect::<Vec<_>>()
    };
    let mut expected = names.to_vec();
    expected.sort_by_key(|name| super::super::info::java_hash(name));
    assert_eq!(ordered(&store.state.settings), expected);
    assert_eq!(ordered(&old.owner().settings), names);
    owner.settings.packages[1].version_code = 2;
    store.commit_scan_settings(&capture(owner.clone())).unwrap();
    assert_eq!(ordered(&store.state.settings), expected);
    let removed = owner.settings.packages.remove(1);
    owner.settings.packages.push(removed);
    store.commit_scan_settings(&capture(owner)).unwrap();
    expected.swap(2, 3); // BB/Aa share a hash; recreation moves BB after Aa.
    assert_eq!(ordered(&store.state.settings), expected);
    let restored =
        super::super::scan::SigningScan::new(&Default::default(), &store.state.settings, 36)
            .unwrap();
    store.commit_scan_settings(&capture(restored)).unwrap();
    assert_eq!(ordered(&store.state.settings), expected);
    store.commit_scan_settings(&old).unwrap();
    assert_eq!(ordered(&old.owner().settings), names);
    assert_eq!(
        store
            .state
            .settings
            .packages
            .iter()
            .find(|p| p.name == "BB")
            .unwrap()
            .version_code,
        0
    );
}

#[test]
fn fresh_settings_claim_is_read_only_and_rejects_existing_or_foreign_files() {
    let data = Data::new();
    let mut first = Store::create(&data.0, &[0, 10]).unwrap();
    assert!(!data.0.join("system").exists());
    assert!(Store::open(&data.0, &[0]).unwrap().is_none());
    assert!(!first.settings_present);
    assert_eq!(
        first
            .state
            .users
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        [0, 10]
    );
    let mut second = Store::create(&data.0, &[0]).unwrap();
    let settings = super::super::settings::Settings {
        packages: vec![super::super::settings::Package {
            name: "first.app".into(),
            app_id: 10100,
            code_path: "/data/app/first".into(),
            category_hint: -1,
            domain_set_id: Some("00000000-0000-0000-0000-000000000001".into()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let owner = super::super::scan::SigningScan::new(&Default::default(), &settings, 36).unwrap();
    let capture = |owner| {
        super::super::scan_snapshot::Store::new(owner, super::usage::Usage::new(["first.app"]))
            .unwrap()
            .capture()
    };
    let mut invalid = owner.clone();
    invalid.settings.packages[0].domain_set_id = None;
    assert!(
        !first
            .commit_scan_settings(&capture(invalid))
            .unwrap_err()
            .committed
    );
    assert!(!data.0.join("system").exists());
    let snapshot = capture(owner);
    first.commit_scan_settings(&snapshot).unwrap();
    assert!(first.settings_present);
    assert!(Store::create(&data.0, &[0]).is_err());
    let path = data.0.join("system/packages.xml");
    let bytes = fs::read(&path).unwrap();
    assert!(
        !second
            .commit_scan_settings(&snapshot)
            .unwrap_err()
            .committed
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(
        signing::persisted(Store::open(&data.0, &[0]).unwrap().unwrap().state.settings),
        signing::persisted(first.state.settings.clone())
    );
    first.commit_scan_settings(&snapshot).unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    for name in [
        "packages.xml",
        "packages-backup.xml",
        "packages.xml.reservecopy",
    ] {
        let other = Data::new();
        fs::create_dir_all(other.0.join("system")).unwrap();
        let file = other.0.join("system").join(name);
        fs::write(&file, b"corrupt-owned-input").unwrap();
        assert!(Store::create(&other.0, &[0]).is_err());
        assert_eq!(fs::read(&file).unwrap(), b"corrupt-owned-input");
    }
}

#[test]
fn fresh_settings_claim_validates_related_user_files_and_preserves_documents() {
    let data = Data::new();
    let dir = data.0.join("system/users/0");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("package-restrictions.xml");
    fs::write(&path, b"malformed").unwrap();
    assert!(Store::create(&data.0, &[0]).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"malformed");
    fs::write(&path, RESTRICTIONS).unwrap();
    let owner = Store::create(&data.0, &[0]).unwrap();
    assert_eq!(
        owner.restrictions[&0],
        aim_android_xml::read(RESTRICTIONS).unwrap()
    );
    assert_eq!(fs::read(&path).unwrap(), RESTRICTIONS);
    assert!(!data.0.join("system/packages.xml").exists());
}

#[test]
fn first_write_failure_retains_only_owned_artifacts_and_retries() {
    let data = Data::new();
    let mut store = Store::create(&data.0, &[0]).unwrap();
    let root = aim_android_xml::read(b"<packages><package name='first.app' codePath='/data/app/first' userId='10100'/></packages>").unwrap();
    let prior = store.state.clone();
    let error = store
        .commit_package_document_using(root.clone(), None, |file, bytes| {
            file.write_all(&bytes[..bytes.len() / 2])?;
            Err(io::Error::other("controlled partial main failure"))
        })
        .unwrap_err();
    assert!(!error.committed);
    assert_eq!(store.state, prior);
    assert!(!store.settings_present);
    let path = data.0.join("system/packages.xml");
    let reserve = sibling(&path, ".reservecopy");
    assert!(!path.exists());
    assert_eq!(fs::read(&reserve).unwrap(), b"");
    assert_eq!(store.first_write_files.len(), 1);
    assert!(Store::create(&data.0, &[0]).is_err());
    fs::write(&reserve, b"foreign").unwrap();
    assert!(
        !store
            .commit_package_document_using(root.clone(), None, |file, bytes| file.write_all(bytes))
            .unwrap_err()
            .committed
    );
    assert_eq!(fs::read(&reserve).unwrap(), b"foreign");
    fs::write(&reserve, b"").unwrap();
    fs::remove_file(&reserve).unwrap();
    fs::write(&reserve, b"").unwrap();
    assert!(
        !store
            .commit_package_document_using(root.clone(), None, |file, bytes| file.write_all(bytes))
            .unwrap_err()
            .committed
    );
    assert!(
        store.first_write_files[0].file.metadata().unwrap().ino()
            != fs::metadata(&reserve).unwrap().ino()
    );
    fs::remove_file(&reserve).unwrap();
    store
        .commit_package_document_using(root, None, |file, bytes| file.write_all(bytes))
        .unwrap();
    assert!(store.settings_present);
    assert!(store.first_write_files.is_empty());
    assert_eq!(fs::read(&path).unwrap(), fs::read(&reserve).unwrap());
    assert_eq!(
        Store::open(&data.0, &[0]).unwrap().unwrap().state.settings,
        store.state.settings
    );
}

#[test]
fn first_write_start_failure_can_retry_its_owned_empty_main() {
    let data = Data::new();
    let mut store = Store::create(&data.0, &[0]).unwrap();
    let path = data.0.join("system/packages.xml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    // Same observer used by the Store; failure after opening main models
    // reserve descriptor acquisition failing before a writable stream returns.
    assert!(
        !write_with_observed(
            &path,
            &data.0.join("system/packages-backup.xml"),
            |_| Ok(()),
            |file, main| {
                store.first_write_files.push(OwnedFile {
                    file: file.try_clone()?,
                    payload: std::sync::Arc::from([]),
                });
                assert!(main);
                Err(io::Error::other(
                    "controlled descriptor acquisition failure",
                ))
            }
        )
        .unwrap_err()
        .committed
    );
    assert_eq!(fs::read(&path).unwrap(), b"");
    let root = element("packages");
    store
        .commit_package_document_using(root, None, |file, bytes| file.write_all(bytes))
        .unwrap();
    assert!(store.settings_present);
    assert!(store.first_write_files.is_empty());
    assert!(!data.0.join("system/packages-backup.xml").exists());
}

#[test]
fn first_write_reserve_failure_publishes_main_and_restores_reserve_on_retry() {
    let data = Data::new();
    let mut store = Store::create(&data.0, &[0]).unwrap();
    let path = data.0.join("system/packages.xml");
    let reserve = sibling(&path, ".reservecopy");
    let root = aim_android_xml::read(b"<packages><package name='first.app' codePath='/data/app/first' userId='10100'/></packages>").unwrap();
    let error = store
        .commit_package_document_using(root.clone(), None, |file, bytes| {
            file.write_all(bytes)?;
            // Invalidate the owned reserve pathname after descriptors opened.
            // Its finalize metadata operation then fails after main committed.
            fs::remove_file(&reserve)
        })
        .unwrap_err();
    assert!(error.committed);
    assert!(store.settings_present);
    assert!(store.first_write_files.is_empty());
    assert!(!reserve.exists());
    assert_eq!(
        Store::open(&data.0, &[0]).unwrap().unwrap().state.settings,
        store.state.settings
    );
    store
        .commit_package_document_using(root, None, |file, bytes| file.write_all(bytes))
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), fs::read(&reserve).unwrap());
}

#[test]
fn initial_write_retries_owned_reserve_without_retaining_removed_descriptors() {
    let data = Data::new();
    let mut store = Store::create(&data.0, &[0]).unwrap();
    let root = element("packages");
    let prior = store.state.clone();
    for _ in 0..5 {
        let error = store
            .commit_package_document_using(root.clone(), None, |file, bytes| {
                file.write_all(&bytes[..3])?;
                Err(io::Error::other("controlled repeated first-write failure"))
            })
            .unwrap_err();
        assert!(!error.committed);
        assert_eq!(store.state, prior);
        assert_eq!(store.first_write_files.len(), 1);
    }
    store
        .commit_package_document_using(root, None, |file, bytes| file.write_all(bytes))
        .unwrap();
    assert!(store.settings_present);
    assert!(store.first_write_files.is_empty());
    let path = data.0.join("system/packages.xml");
    assert_eq!(
        fs::read(&path).unwrap(),
        fs::read(sibling(&path, ".reservecopy")).unwrap()
    );
}

#[test]
fn restarted_settings_recovery_matches_resilient_file_precedence_and_no_root() {
    use recovery::{Event, Plan, Source};
    let read = |bytes: &[u8], settings: &mut super::super::settings::Settings| {
        let root = aim_android_xml::read_next_optional(bytes)?;
        if let Some(root) = &root {
            *settings = super::super::settings::Settings::parse(root)?;
        }
        Ok(root)
    };
    let cases: &[(Option<&[u8]>, Option<&[u8]>, Option<&[u8]>, bool, usize, Vec<Event>)] = &[
        (None, None, None, true, 0, vec![Event::Absent]),
        (None, None, Some(b""), true, 0, vec![Event::Selected(Source::Reserve), Event::NoStartTag(Source::Reserve)]),
        (Some(b""), None, Some(b"<packages/>"), true, 0, vec![Event::Selected(Source::Main), Event::NoStartTag(Source::Main)]),
        (Some(b"<packages/>"), Some(b"<packages><package name='backup' codePath='/data/app/backup' userId='10100'/></packages>"), Some(b"<packages/>"), false, 1,
            vec![Event::Selected(Source::Backup), Event::Removed(Source::Main), Event::Removed(Source::Reserve)]),
    ];
    for (main, backup, reserve, first_boot, packages, expected) in cases {
        let data = Data::new();
        fs::create_dir_all(data.0.join("system")).unwrap();
        for (path, value) in settings_paths(&data.0)
            .into_iter()
            .zip([main, backup, reserve])
        {
            if let Some(bytes) = value {
                fs::write(path, bytes).unwrap();
            }
        }
        let plan = Plan::inspect(&data.0).unwrap();
        let mut settings = Default::default();
        let (mut store, report) = plan.recover(&[0], &mut settings, read).unwrap();
        assert_eq!(report.first_boot, *first_boot);
        assert_eq!(report.events, *expected);
        assert_eq!(store.state.settings.packages.len(), *packages);
        store
            .commit_package_document_using(element("packages"), None, |file, bytes| {
                file.write_all(bytes)
            })
            .unwrap();
        assert!(Store::open(&data.0, &[0]).unwrap().is_some());
    }
    let data = Data::new();
    fs::create_dir_all(data.0.join("system")).unwrap();
    let paths = settings_paths(&data.0);
    fs::write(&paths[0], b"broken").unwrap();
    fs::write(&paths[2], b"<packages/>").unwrap();
    let (store, report) = Plan::inspect(&data.0)
        .unwrap()
        .recover(&[0], &mut Default::default(), read)
        .unwrap();
    assert!(!report.first_boot);
    assert!(matches!(
        report.events[1],
        Event::Failed {
            source: Source::Main,
            ..
        }
    ));
    assert_eq!(report.events[2], Event::Removed(Source::Main));
    assert_eq!(report.events[3], Event::Selected(Source::Reserve));
    assert!(store.settings_present);
    assert!(!paths[0].exists());
    let data = Data::new();
    fs::create_dir_all(data.0.join("system")).unwrap();
    let paths = settings_paths(&data.0);
    fs::write(&paths[0], b"<packages/>").unwrap();
    fs::write(&paths[1], b"broken").unwrap();
    fs::write(&paths[2], b"<packages/>").unwrap();
    let (_, report) = Plan::inspect(&data.0)
        .unwrap()
        .recover(&[0], &mut Default::default(), read)
        .unwrap();
    assert!(!report.first_boot); // The outer failed read returns true even if retry finds nothing.
    assert_eq!(report.events.last(), Some(&Event::Absent));
    assert!(paths.iter().all(|path| !path.exists()));
}

#[test]
fn recovery_rejects_foreign_changes_and_retains_frontend_effects_before_failure() {
    use recovery::{Event, Plan, Source};
    let data = Data::new();
    fs::create_dir_all(data.0.join("system")).unwrap();
    let path = data.0.join("system/packages.xml");
    fs::write(&path, b"<packages/>").unwrap();
    let plan = Plan::inspect(&data.0).unwrap();
    fs::write(&path, b"foreign").unwrap();
    let error = plan
        .recover(&[0], &mut Default::default(), |_, _| {
            panic!("foreign input reached frontend")
        })
        .err()
        .unwrap();
    assert!(error.events.is_empty());
    assert_eq!(fs::read(&path).unwrap(), b"foreign");
    fs::write(&path, b"broken").unwrap();
    let plan = Plan::inspect(&data.0).unwrap();
    let mut settings = super::super::settings::Settings::default();
    let (store, report) = plan
        .recover(&[0], &mut settings, |_, state| {
            state.verifier = Some("retained frontend effect".into());
            Err("controlled frontend failure".into())
        })
        .unwrap();
    assert_eq!(
        settings.verifier.as_deref(),
        Some("retained frontend effect")
    );
    assert_eq!(store.state.settings.verifier, settings.verifier);
    assert!(matches!(
        report.events[1],
        Event::Failed {
            source: Source::Main,
            ..
        }
    ));
    assert!(!report.first_boot);
    assert!(!path.exists());
}

#[test]
#[ignore = "invoked as the controlled first-write crash subprocess"]
fn recovery_first_write_crash_child() {
    use std::io::Read;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    let data = PathBuf::from(input);
    assert!(data.starts_with(std::env::temp_dir()));
    assert!(
        data.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("aim-package-owner-")
    );
    let mut store = Store::create(&data, &[0]).unwrap();
    store
        .commit_package_document_using(element("packages"), None, |file, bytes| {
            file.write_all(&bytes[..3])?;
            file.sync_all()?;
            std::process::exit(17); // No failWrite or Rust destructors run.
        })
        .unwrap();
    panic!("controlled crash returned");
}

#[test]
fn recovery_claim_survives_an_actual_first_write_process_exit() {
    use std::process::{Command, Stdio};
    let data = Data::new();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "package::owner::tests::recovery_first_write_crash_child",
            "--ignored",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let write = child
        .stdin
        .take()
        .unwrap()
        .write_all(data.0.to_str().unwrap().as_bytes());
    if let Err(error) = write {
        let _ = child.kill();
        let _ = child.wait();
        panic!("controlled child input: {error}");
    }
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(17),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(Store::open(&data.0, &[0]).is_err());
    assert!(Store::create(&data.0, &[0]).is_err());
    let mut state = super::super::settings::Settings::default();
    let (mut store, report) = recovery::Plan::inspect(&data.0)
        .unwrap()
        .recover(&[0], &mut state, |bytes, settings| {
            let root = aim_android_xml::read_next_optional(bytes)?;
            if let Some(root) = &root {
                *settings = super::super::settings::Settings::parse(root)?;
            }
            Ok(root)
        })
        .unwrap();
    assert!(!report.first_boot);
    assert!(report.events.iter().any(|event| matches!(
        event,
        recovery::Event::Failed {
            source: recovery::Source::Main,
            ..
        }
    )));
    assert_eq!(
        report.events.last(),
        Some(&recovery::Event::NoStartTag(recovery::Source::Reserve))
    );
    store
        .commit_package_document_using(element("packages"), None, |file, bytes| {
            file.write_all(bytes)
        })
        .unwrap();
    assert!(Store::open(&data.0, &[0]).unwrap().is_some());
    assert!(store.first_write_files.is_empty());
}

fn derive_public_keys(settings: &mut crate::package::settings::Settings) {
    for signing in settings
        .packages
        .iter_mut()
        .filter_map(|p| p.signatures.as_mut())
        .chain(
            settings
                .shared_users
                .iter_mut()
                .filter_map(|g| g.signatures.as_mut()),
        )
    {
        signing.public_keys = Some(
            super::super::sign::saved_certificate_keys(&signing.signatures)
                .unwrap()
                .unwrap()
                .into_iter()
                .map(Some)
                .collect(),
        );
    }
}

#[test]
fn domain_commits_preserve_foreign_data_and_reject_external_writers() {
    let data = Data::new();
    data.settings();
    let path = data.0.join("system/packages.xml");
    let xml = b"<packages future='root'><package name='example.app' codePath='/data/app/example' userId='10100'/><domain-verifications future='global'><active><package-state packageName='example.app' id='00000000-0000-0000-0000-000000000001' future='package'><state><domain name='example.com' state='0' future='host'/><future-state/></state><user-states><user-state userId='0' allowLinkHandling='true'><enabled-hosts><host name='example.com'/></enabled-hosts><future-user/></user-state></user-states></package-state><future-active/></active><restored/></domain-verifications><domain-verifications-legacy><user-states packageName='example.app'><user-state userId='0' state='2'/></user-states></domain-verifications-legacy><future-owner/></packages>";
    fs::write(&path, xml).unwrap();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let unchanged = store.state().settings.packages.clone();
    let mut desired = store.state().settings.domain_verification.clone();
    desired.active[0].domains[0].1 = 1;
    desired.active[0].users[0].allow_link_handling = false;
    store.commit_domains(&desired).unwrap();
    assert_eq!(store.state().settings.packages, unchanged);
    assert_eq!(store.state().settings.domain_verification, desired);
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.starts_with(abx::MAGIC));
    assert_eq!(fs::read(sibling(&path, ".reservecopy")).unwrap(), bytes);
    assert_eq!(
        Store::open(&data.0, &[0]).unwrap().unwrap().state(),
        store.state()
    );
    let root = aim_android_xml::read(&bytes).unwrap();
    assert!(root.children().any(|e| e.name == "future-owner"));
    let domains = root
        .children()
        .find(|e| e.name == "domain-verifications")
        .unwrap();
    assert_eq!(domains.string("future").as_deref(), Some("global"));
    let package = domains
        .children()
        .find(|e| e.name == "active")
        .unwrap()
        .children()
        .find(|e| e.name == "package-state")
        .unwrap();
    assert_eq!(package.string("future").as_deref(), Some("package"));
    assert!(
        package
            .children()
            .find(|e| e.name == "state")
            .unwrap()
            .children()
            .any(|e| e.name == "future-state")
    );
    desired.active[0].domains.clear();
    store.commit_domains(&desired).unwrap();
    let root = aim_android_xml::read(&fs::read(&path).unwrap()).unwrap();
    let package = root
        .children()
        .find(|e| e.name == "domain-verifications")
        .unwrap()
        .children()
        .find(|e| e.name == "active")
        .unwrap()
        .children()
        .find(|e| e.name == "package-state")
        .unwrap();
    assert!(
        package
            .children()
            .find(|e| e.name == "state")
            .unwrap()
            .children()
            .any(|e| e.name == "future-state")
    );
    let before = fs::read(&path).unwrap();
    let mut invalid = desired.clone();
    invalid.active[0].id = "invalid".into();
    assert!(!store.commit_domains(&invalid).unwrap_err().committed);
    assert_eq!(fs::read(&path).unwrap(), before);
    let mut invalid = desired.clone();
    invalid.active.push(invalid.active[0].clone());
    assert!(!store.commit_domains(&invalid).unwrap_err().committed);
    assert_eq!(fs::read(&path).unwrap(), before);
    let previous = store.state().clone();
    fs::write(&path, b"<packages future='external'/>").unwrap();
    let error = store.commit_domains(&desired).unwrap_err();
    assert!(!error.committed);
    assert_eq!(store.state(), &previous);
    assert_eq!(fs::read(&path).unwrap(), b"<packages future='external'/>");
}

#[test]
fn domain_write_failures_publish_only_committed_disk_state() {
    let data = Data::new();
    data.settings();
    let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
    let path = data.0.join("system/packages.xml");
    let desired_root = aim_android_xml::read(b"<domain-verifications><active><package-state packageName='example.app' id='00000000-0000-0000-0000-000000000001'><state><domain name='example.com' state='1'/></state></package-state></active><restored/></domain-verifications>").unwrap();
    let mut desired = crate::package::domain_verification::State::default();
    desired.read(&desired_root).unwrap();
    let root = domains::replace(&store.settings_document, &desired).unwrap();
    let before = store.state().clone();
    let error = store
        .commit_package_document_using(root.clone(), None, |file, bytes| {
            file.write_all(&bytes[..bytes.len() / 2])?;
            Err(io::Error::other("injected domain main-file failure"))
        })
        .unwrap_err();
    assert!(!error.committed);
    assert_eq!(store.state(), &before);
    assert_eq!(
        Store::open(&data.0, &[0]).unwrap().unwrap().state(),
        &before
    );
    let error = store
        .commit_package_document_using(root, None, |file, bytes| {
            file.write_all(bytes)?;
            fs::remove_file(sibling(&path, ".reservecopy"))
        })
        .unwrap_err();
    assert!(error.committed);
    assert_eq!(store.state().settings.domain_verification, desired);
    assert_eq!(
        Store::open(&data.0, &[0]).unwrap().unwrap().state(),
        store.state()
    );
}

#[test]
fn exclusive_recovery_matches_directory_open_selection_and_cleanup() {
    use recovery::{Event, Plan, Source};
    let parse = |bytes: &[u8], settings: &mut crate::package::settings::Settings| {
        let root = aim_android_xml::read(bytes)?; *settings = crate::package::settings::Settings::parse(&root)?; Ok(Some(root))
    };
    for nonempty in [false, true] {
        let data = Data::new(); fs::create_dir_all(data.0.join("system")).unwrap(); let paths = settings_paths(&data.0);
        fs::create_dir(&paths[0]).unwrap(); if nonempty {fs::write(paths[0].join("keep"), b"owned fixture").unwrap();}
        fs::write(&paths[1], b"<packages/>").unwrap(); fs::create_dir(&paths[2]).unwrap();
        let (_, report) = Plan::inspect(&data.0).unwrap().recover(&[0], &mut Default::default(), parse).unwrap();
        assert_eq!(report.events, vec![Event::Selected(Source::Backup), if nonempty {Event::RemoveFailed(Source::Main)} else {Event::Removed(Source::Main)}, Event::Removed(Source::Reserve)]);
        assert_eq!(paths[0].exists(), nonempty); assert!(paths[1].is_file()); assert!(!paths[2].exists());
    }
    let data = Data::new(); fs::create_dir_all(data.0.join("system")).unwrap(); let paths = settings_paths(&data.0);
    fs::write(&paths[0], b"<packages/>").unwrap(); fs::create_dir(&paths[1]).unwrap(); fs::create_dir(&paths[2]).unwrap();
    let (_, report) = Plan::inspect(&data.0).unwrap().recover(&[0], &mut Default::default(), parse).unwrap();
    assert_eq!(report.events, vec![Event::OpenFailed(Source::Backup), Event::Selected(Source::Main)]); assert!(paths[1].is_dir() && paths[2].is_dir());
    fs::remove_file(&paths[0]).unwrap(); fs::create_dir(&paths[0]).unwrap();
    let error = Plan::inspect(&data.0).unwrap().recover(&[0], &mut Default::default(), parse).err().unwrap();
    assert_eq!(error.events, vec![Event::OpenFailed(Source::Backup), Event::OpenFailed(Source::Main)]);
    let plan = Plan::inspect(&data.0).unwrap(); fs::write(paths[1].join("foreign"), b"changed").unwrap();
    assert!(plan.recover(&[0], &mut Default::default(), parse).is_err()); assert!(paths[1].join("foreign").is_file());
}

#[test]
fn backup_cleanup_permission_failure_is_reported_without_aborting_the_read() {
    use recovery::{Event, Plan, Source};
    let data = Data::new(); let parent = data.0.join("system"); fs::create_dir_all(&parent).unwrap();
    let paths = settings_paths(&data.0);
    for path in &paths {fs::write(path, b"<packages/>").unwrap();}
    let plan = Plan::inspect(&data.0).unwrap();
    struct RestorePermissions(std::path::PathBuf);
    impl Drop for RestorePermissions {fn drop(&mut self) {fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755)).unwrap();}}
    let restore = RestorePermissions(parent.clone()); fs::set_permissions(&parent, fs::Permissions::from_mode(0o555)).unwrap();
    let (_, report) = plan.recover(&[0], &mut Default::default(), |bytes, state| {
        let root = aim_android_xml::read(bytes)?; *state = crate::package::settings::Settings::parse(&root)?; Ok(Some(root))
    }).unwrap();
    assert_eq!(report.events, vec![Event::Selected(Source::Backup), Event::RemoveFailed(Source::Main), Event::RemoveFailed(Source::Reserve)]);
    assert!(paths.iter().all(|path| path.is_file())); drop(restore);
    // failRead still owns a required deletion; it must expose permission failure.
    fs::write(&paths[1], b"broken").unwrap(); let plan = Plan::inspect(&data.0).unwrap();
    let restore = RestorePermissions(parent.clone()); fs::set_permissions(&parent, fs::Permissions::from_mode(0o555)).unwrap();
    let error = plan.recover(&[0], &mut Default::default(), |_, _| Err("controlled parse failure".into())).err().unwrap();
    assert!(matches!(error.events.last(), Some(Event::Failed {source: Source::Backup, ..}))); assert!(paths.iter().all(|path| path.is_file())); drop(restore);
}


#[test]
fn native_frontend_owner_failure_preserves_selected_input_and_prior_effects() {
    use recovery::{Event, Plan, ReadError, Source};
    for backup in [false, true] {
        let data = Data::new();
        let paths = settings_paths(&data.0);
        fs::create_dir_all(data.0.join("system")).unwrap();
        let bytes = b"<packages><package name='p' codePath='/p' userId='10001' domainSetId='00000000-0000-0000-0000-000000000001'><perms/></package></packages>";
        for path in &paths { fs::write(path, bytes).unwrap(); }
        if !backup { fs::remove_file(&paths[1]).unwrap(); }
        let plan = Plan::inspect(&data.0).unwrap();
        let mut settings = crate::package::settings::Settings::default();
        let mut calls = 0;
        let mut ids = crate::package::owner::app_ids::AppIds::default();
        let mut attempt = crate::package::settings::PackageReadAttempt::default();
        let error = plan.recover_with_owner(&[0], &mut settings, |bytes, state| {
            use aim_android_xml::pull::Reader;
            calls += 1;
            state.find_or_create_version(None).sdk_version = 36;
            let mut reader = Reader::new(bytes).map_err(ReadError::File)?;
            state.read_events(&mut reader, |state, reader, start| {
                state.read_package(reader, start, &mut ids, &mut attempt, |_, _, _| {
                    Err(ReadError::Owner("package policy bridge unavailable".into()))
                })?;
                Ok(true)
            })?;
            Ok(Some(element("packages")))
        }).err().unwrap();
        let source = if backup { Source::Backup } else { Source::Main };
        assert_eq!(calls, 1);
        assert_eq!(settings.versions[0].sdk_version, 36);
        assert_eq!(settings.packages[0].name, "p");
        assert!(ids.get(10001).is_some());
        assert_eq!(error.message, "package policy bridge unavailable");
        assert_eq!(error.events.last(), Some(&Event::OwnerFailed { source, message: error.message.clone() }));
        assert!(!error.events.iter().any(|event| matches!(event, Event::Failed { .. })));
        assert_eq!(fs::read(&paths[if backup {1} else {0}]).unwrap(), bytes);
        if backup {
            // Backup openRead cleanup precedes the frontend, as in the original.
            assert!(!paths[0].exists()); assert!(!paths[2].exists());
        } else { assert!(paths[2].is_file()); }
    }
}

#[test]
fn classified_file_failure_still_removes_corrupt_input_and_retries_reserve() {
    use recovery::{Event, Plan, ReadError, Source};
    let data = Data::new(); let paths = settings_paths(&data.0);
    fs::create_dir_all(data.0.join("system")).unwrap();
    fs::write(&paths[0], b"<packages><").unwrap();
    fs::write(&paths[2], b"<packages/>").unwrap();
    let mut settings = crate::package::settings::Settings::default();
    let (_, report) = Plan::inspect(&data.0).unwrap().recover_with_owner(&[0], &mut settings, |bytes, state| {
        let root = aim_android_xml::read_next_optional(bytes).map_err(ReadError::File)?;
        if let Some(root) = &root { *state = crate::package::settings::Settings::parse(root).map_err(ReadError::File)?; }
        Ok(root)
    }).unwrap();
    assert!(!report.first_boot); assert!(!paths[0].exists()); assert!(paths[2].is_file());
    assert!(report.events.iter().any(|event| matches!(event, Event::Failed { source: Source::Main, .. })));
    assert_eq!(report.events.last(), Some(&Event::Selected(Source::Reserve)));
}

#[test]
fn captured_settings_document_reopens_after_owned_commit_with_extensions() {
    let data = Data::new();
    fs::create_dir_all(data.0.join("system")).unwrap();
    let path = data.0.join("system/packages.xml");
    let bytes = b"<packages custom='keep'><!--note--><version sdkVersion='36' databaseVersion='3'/><permissions><item name='perm' package='p' protection='2'/></permissions><?native capture?><extension value='keep'><nested/></extension></packages>";
    fs::write(&path, bytes).unwrap();
    let mut settings = crate::package::settings::Settings::default();
    let (mut store, report) = recovery::Plan::inspect(&data.0).unwrap()
        .recover_with_owner(&[0], &mut settings, |bytes, state| state.read_document(bytes, |_, _, _| Ok(false))).unwrap();
    assert!(!report.first_boot);
    let document = aim_android_xml::read_next(bytes).unwrap();
    assert_eq!(store.settings_document, document);
    store.commit_package_document_using(document.clone(), None, |file, bytes| file.write_all(bytes)).unwrap();
    assert_eq!(Store::open(&data.0, &[0]).unwrap().unwrap().settings_document, document);
    assert_eq!(aim_android_xml::read_next(&fs::read(path).unwrap()).unwrap(), document);
}

#[test]
fn uncaught_keyset_input_failure_preserves_the_selected_file() {
    use recovery::{Event, Plan, Source};
    let data = Data::new(); fs::create_dir_all(data.0.join("system")).unwrap();
    let path = data.0.join("system/packages.xml");
    let bytes = b"<packages><keyset-settings version='1'><lastIssuedKeyId value='9'/><keysets><key-id identifier='1'/></keysets></keyset-settings></packages>";
    fs::write(&path, bytes).unwrap();
    let mut settings = crate::package::settings::Settings::default();
    let error = Plan::inspect(&data.0).unwrap().recover_with_owner(&[0], &mut settings, |bytes, state| {
        state.read_document(bytes, |state, reader, start| {
            state.read_key_sets(reader, start, &Default::default(), |_| panic!("unexpected key"), |_,_| panic!("invalid mapping finalized"))?;
            Ok(true)
        })
    }).err().unwrap();
    assert_eq!(settings.key_sets.last_issued_key_id, 9);
    assert!(matches!(error.events.last(), Some(Event::FatalInput { source: Source::Main, .. })));
    assert!(!error.events.iter().any(|event| matches!(event, Event::Removed(_))));
    assert_eq!(fs::read(path).unwrap(), bytes);
}

#[test]
fn boot_versions_distinguish_absence_no_start_and_uncaught_failure() {
    use crate::package::settings::{Settings, Version};
    let current = Version {sdk_version:36,database_version:3,build_fingerprint:Some("build".into()),fingerprint:Some("partitions".into()),..Default::default()};
    for input in [None, Some(b" ".as_slice()), Some(b"<packages/>".as_slice())] {
        let data = Data::new(); fs::create_dir_all(data.0.join("system")).unwrap();
        if let Some(input) = input { fs::write(data.0.join("system/packages.xml"), input).unwrap(); }
        let mut settings = Settings::default(); settings.find_or_create_version(None).sdk_version = 30;
        let (store, report) = recovery::Plan::inspect(&data.0).unwrap().recover_boot(&[], &mut settings, &current, |bytes, state| state.read_document(bytes, |_,_,_| Ok(false))).unwrap();
        assert_eq!(settings.versions[0].sdk_version, if input.is_none() {36} else {30});
        assert_eq!(settings.versions[1].volume_uuid.as_deref(), Some("primary_physical"));
        assert_eq!(settings.versions[1].sdk_version,36);
        assert_eq!(store.state().settings, settings);
        assert_eq!(report.first_boot, input.is_none() || input == Some(b" ".as_slice()));
    }
    let data = Data::new(); fs::create_dir_all(data.0.join("system")).unwrap();
    let bytes = b"<packages><verifier/></packages>"; fs::write(data.0.join("system/packages.xml"), bytes).unwrap();
    let mut settings = Settings::default();
    assert!(recovery::Plan::inspect(&data.0).unwrap().recover_boot(&[], &mut settings, &current, |bytes, state| state.read_document(bytes, |_,_,_| Ok(false))).is_err());
    assert_eq!(settings.versions.len(),2); assert_eq!(settings.versions[0].sdk_version,36);
    assert_eq!(fs::read(data.0.join("system/packages.xml")).unwrap(),bytes);
}
