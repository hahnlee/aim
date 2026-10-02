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
