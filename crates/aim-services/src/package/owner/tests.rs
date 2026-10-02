use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

struct Data(PathBuf);

impl Data {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "aim-package-owner-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn settings(&self) -> PathBuf {
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
    assert_eq!(user.disabled_components, ["example.app.Disabled"]);
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
