//! Original PMS reads native signing persistence on a disposable data image.
use aim_services::package::{State, owner::Store};
use std::fs;
use std::time::{Duration, Instant};

mod common {
    pub mod runtime;
}
use common::runtime::{Boot, Data, run};

fn start(boot: &Boot) {
    run(boot.start_command().args(["start", "--windows"]));
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let result = boot
            .command()
            .args(["shell", "getprop", "sys.boot_completed"])
            .output()
            .unwrap();
        if result.status.success() && String::from_utf8_lossy(&result.stdout).trim() == "1" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "original PMS boot did not complete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[test]
#[ignore = "requires aimctl and the pinned derived image; run explicitly"]
fn original_pms_boots_with_native_keyset_registration() {
    use aim_services::package::owner::key_sets;
    let dir = std::env::temp_dir().join(format!("aim-keyr-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let data = Data(dir);
    let boot = Boot::new(aim_paths::root().join("target/release/aimctl"), data.0.join("guest"));
    start(&boot);
    run(boot.command().arg("stop"));
    let volume = aim_storage::data::DataImage::attach(&boot.data, None).unwrap();
    let mut store = Store::open(&boot.data.join("data"), &[0]).unwrap().unwrap();
    let before = store.state().settings.clone();
    assert_eq!(before.packages.len(), 243);
    let target = before
        .packages
        .iter()
        .find(|p| {
            let id = p.key_set_data.proper_signing_key_set;
            id > 0
                && p.key_set_data.defined_key_sets.is_empty()
                && before
                    .packages
                    .iter()
                    .filter(|other| {
                        other.key_set_data.proper_signing_key_set == id
                            || other
                                .key_set_data
                                .defined_key_sets
                                .iter()
                                .any(|(_, alias)| *alias == id)
                    })
                    .count()
                    == 1
        })
        .expect("image has no singly-owned signing keyset");
    let signing: Vec<_> = before
        .key_sets
        .key_sets
        .iter()
        .find(|(id, _)| *id == target.key_set_data.proper_signing_key_set)
        .unwrap()
        .1
        .iter()
        .map(|id| {
            before
                .key_sets
                .public_keys
                .iter()
                .find(|(key, _)| key == id)
                .unwrap()
                .1
                .clone()
        })
        .collect();
    let mut desired = before.clone();
    key_sets::clear_package(&mut desired, &target.name).unwrap();
    key_sets::register(&mut desired, &target.name, &signing, Some(&[]), &[]).unwrap();
    let changed = desired
        .packages
        .iter()
        .find(|p| p.name == target.name)
        .unwrap();
    assert_eq!(
        changed.key_set_data.proper_signing_key_set,
        before.key_sets.last_issued_key_set_id + 1
    );
    assert_ne!(changed.key_set_data, target.key_set_data);
    store.commit_key_sets(&desired).unwrap();
    let path = boot.data.join("data/system/packages.xml");
    let written = fs::read(&path).unwrap();
    assert!(written.starts_with(aim_android_xml::abx::MAGIC));
    assert_eq!(
        written,
        fs::read(path.with_file_name("packages.xml.reservecopy")).unwrap()
    );
    assert!(!path.with_file_name("packages-backup.xml").exists());
    assert_eq!(
        Store::open(&boot.data.join("data"), &[0])
            .unwrap()
            .unwrap()
            .state()
            .settings,
        desired
    );
    drop(store);
    volume.detach().unwrap();
    start(&boot);
    let reread = State::read(&boot.data.join("data"), &[0]).unwrap().unwrap();
    assert_eq!(reread.settings.key_sets, desired.key_sets);
    assert_eq!(reread.settings.packages.len(), desired.packages.len());
    for saved in &desired.packages {
        let actual = reread
            .settings
            .packages
            .iter()
            .find(|p| p.name == saved.name)
            .unwrap();
        assert_eq!(actual.key_set_data, saved.key_set_data, "{}", saved.name);
        assert_eq!(
            (
                actual.app_id,
                actual.shared_user,
                &actual.code_path,
                &actual.signatures
            ),
            (
                saved.app_id,
                saved.shared_user,
                &saved.code_path,
                &saved.signatures
            ),
            "{}",
            saved.name
        );
    }
    assert_eq!(reread.settings.shared_users, before.shared_users);
    let settings = run(boot.command().args([
        "shell",
        "am",
        "start",
        "-W",
        "-n",
        "com.android.settings/.Settings",
    ]));
    assert!(String::from_utf8_lossy(&settings.stdout).contains("Status: ok"));
    println!(
        "original PMS retained native-reallocated signing keyset IDs and global keys/counters; Settings launched"
    );
}

#[test]
#[ignore = "requires aimctl and the pinned derived image; run explicitly"]
fn original_pms_boots_with_native_library_metadata_persistence() {
    let dir = std::env::temp_dir().join(format!("aim-libr-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let data = Data(dir);
    let boot = Boot::new(aim_paths::root().join("target/release/aimctl"), data.0.join("guest"));
    start(&boot);
    run(boot.command().arg("stop"));
    let volume = aim_storage::data::DataImage::attach(&boot.data, None).unwrap();
    let mut store = Store::open(&boot.data.join("data"), &[0]).unwrap().unwrap();
    let before = store.state().settings.clone();
    let mut desired = before.clone();
    let package = desired
        .packages
        .iter_mut()
        .find(|p| {
            p.primary_cpu_abi.as_deref() == Some("arm64-v8a")
                && p.legacy_native_library_path.is_some()
        })
        .unwrap();
    package.cpu_abi_override = Some("arm64-v8a".into());
    package.set_page_size_compat(8).unwrap();
    let expected = package.clone();
    store.commit_native_library_metadata(&desired).unwrap();
    assert_eq!(store.state().settings, desired);
    let path = boot.data.join("data/system/packages.xml");
    let written = fs::read(&path).unwrap();
    assert!(written.starts_with(aim_android_xml::abx::MAGIC));
    assert_eq!(
        written,
        fs::read(path.with_file_name("packages.xml.reservecopy")).unwrap()
    );
    assert!(!path.with_file_name("packages-backup.xml").exists());
    drop(store);
    volume.detach().unwrap();
    start(&boot);
    let reread = State::read(&boot.data.join("data"), &[0]).unwrap().unwrap();
    let actual = reread
        .settings
        .packages
        .iter()
        .find(|p| p.name == expected.name)
        .unwrap();
    assert_eq!(
        actual.legacy_native_library_path,
        expected.legacy_native_library_path
    );
    assert_eq!(actual.primary_cpu_abi, expected.primary_cpu_abi);
    assert_eq!(actual.secondary_cpu_abi, expected.secondary_cpu_abi);
    // ScanPackageUtils sets this from the scan request, which supplies no
    // install-only override during an ordinary boot.
    assert_eq!(actual.cpu_abi_override, None);
    assert_eq!(actual.page_size_compat, expected.page_size_compat);
    for saved in &before.packages {
        let package = reread
            .settings
            .packages
            .iter()
            .find(|p| p.name == saved.name)
            .unwrap();
        assert_eq!(
            (
                package.app_id,
                package.shared_user,
                &package.code_path,
                &package.signatures
            ),
            (
                saved.app_id,
                saved.shared_user,
                &saved.code_path,
                &saved.signatures
            ),
            "{}",
            saved.name
        );
    }
    assert_eq!(reread.settings.shared_users, before.shared_users);
    assert_eq!(reread.settings.key_sets, before.key_sets);
    let settings = run(boot.command().args([
        "shell",
        "am",
        "start",
        "-W",
        "-n",
        "com.android.settings/.Settings",
    ]));
    assert!(String::from_utf8_lossy(&settings.stdout).contains("Status: ok"));
    println!(
        "original PMS reboot retained native ABI/path/page-size settings and cleared the install-only override; Settings launched"
    );
}

#[test]
#[ignore = "requires aimctl and the pinned derived image; run explicitly"]
fn original_pms_boots_with_native_signature_persistence() {
    let dir = std::env::temp_dir().join(format!("aim-sigr-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let data = Data(dir);
    let boot = Boot::new(aim_paths::root().join("target/release/aimctl"), data.0.join("guest"));
    start(&boot);
    let original = State::read(&boot.data.join("data"), &[0]).unwrap().unwrap();
    assert_eq!(original.settings.packages.len(), 243);
    assert_eq!(original.settings.shared_users.len(), 16);
    run(boot.command().arg("stop"));
    // Reattach only this test's stopped data image. No original PMS is
    // running while the native store owns the package document.
    let volume = aim_storage::data::DataImage::attach(&boot.data, None).unwrap();
    let mut store = Store::open(&boot.data.join("data"), &[0]).unwrap().unwrap();
    let settings = store.state().settings.clone();
    store.commit_signatures(&settings).unwrap();
    let path = boot.data.join("data/system/packages.xml");
    assert!(
        fs::read(&path)
            .unwrap()
            .starts_with(aim_android_xml::abx::MAGIC)
    );
    assert_eq!(
        fs::read(&path).unwrap(),
        fs::read(path.with_file_name("packages.xml.reservecopy")).unwrap()
    );
    assert!(!path.with_file_name("packages-backup.xml").exists());
    let expected = store.state().settings.clone();
    drop(store);
    volume.detach().unwrap();
    start(&boot);
    let reread = State::read(&boot.data.join("data"), &[0]).unwrap().unwrap();
    assert_eq!(reread.settings.packages.len(), expected.packages.len());
    for saved in &expected.packages {
        let package = reread
            .settings
            .packages
            .iter()
            .find(|p| p.name == saved.name)
            .unwrap();
        assert_eq!(
            package.signatures, saved.signatures,
            "original PMS signing state: {}",
            saved.name
        );
        assert_eq!(
            (package.app_id, package.shared_user, &package.code_path),
            (saved.app_id, saved.shared_user, &saved.code_path)
        );
    }
    assert_eq!(reread.settings.shared_users, expected.shared_users);
    let settings = run(boot.command().args([
        "shell",
        "am",
        "start",
        "-W",
        "-n",
        "com.android.settings/.Settings",
    ]));
    assert!(String::from_utf8_lossy(&settings.stdout).contains("Status: ok"));
    let log = fs::read_to_string(std::path::PathBuf::from(format!(
        "{}.aimctl/log",
        boot.data.display()
    )))
    .unwrap();
    for failure in [
        "Error in package manager settings: <sigs>",
        "Error in package manager settings: <cert>",
    ] {
        assert!(
            !log.contains(failure),
            "original PMS signature reader reported {failure}"
        );
    }
    println!(
        "original PMS reboot retained {} package and {} shared UID signing records; Settings launched",
        expected.packages.len(),
        expected.shared_users.len()
    );
}

fn migration_apk(dir: &std::path::Path, version: i32, leaving: bool) -> std::path::PathBuf {
    use std::process::Command;
    let java = aim_paths::fetched().join("java");
    let jdk = java.join("temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let tools = java.join("build-tools-36.0.0/android-16");
    let key = dir.join("fixture.keystore");
    if !key.exists() {
        run(Command::new(jdk.join("bin/keytool"))
            .args([
                "-genkeypair",
                "-alias",
                "fixture",
                "-keyalg",
                "RSA",
                "-keysize",
                "2048",
                "-validity",
                "3650",
                "-dname",
                "CN=AIM disposable migration fixture",
                "-storepass",
                "fixture-only",
                "-keypass",
                "fixture-only",
                "-keystore",
            ])
            .arg(&key));
    }
    let manifest = dir.join(format!("migration-{version}.xml"));
    let attribute = if leaving {
        "android:sharedUserMaxSdkVersion=\"35\""
    } else {
        ""
    };
    fs::write(
        &manifest,
        format!(
            r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android"
        package="org.example.aimmigration" android:versionCode="{version}"
        android:sharedUserId="org.example.aimmigration.uid" {attribute}>
        <uses-sdk android:minSdkVersion="23" android:targetSdkVersion="35" />
        <uses-permission android:name="android.permission.READ_CONTACTS" />
        <uses-permission android:name="android.permission.READ_CALENDAR" />
        <application android:hasCode="false" android:label="AIM migration fixture" />
        </manifest>"#
        ),
    )
    .unwrap();
    let unsigned = dir.join(format!("unsigned-{version}.apk"));
    let signed = dir.join(format!("migration-{version}.apk"));
    run(Command::new(tools.join("aapt2"))
        .args(["link", "--manifest"])
        .arg(&manifest)
        .arg("-I")
        .arg(common::runtime::cohort::original_image().join("system/framework/framework-res.apk"))
        .arg("-o")
        .arg(&unsigned));
    run(Command::new(jdk.join("bin/java"))
        .arg("-jar")
        .arg(tools.join("lib/apksigner.jar"))
        .args(["sign", "--ks"])
        .arg(&key)
        .args([
            "--ks-pass",
            "pass:fixture-only",
            "--key-pass",
            "pass:fixture-only",
            "--out",
        ])
        .arg(&signed)
        .arg(&unsigned));
    signed
}

#[test]
#[ignore = "requires aimctl, pinned image, JDK and Android build tools; run explicitly"]
fn original_pms_reboots_with_native_update_owner_clearing() {
    const NAME: &str = "org.example.aimmigration";
    let dir = std::env::temp_dir().join(format!("aim-ownr-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let data = Data(dir);
    let apk = migration_apk(&data.0, 1, false);
    let boot = Boot::new(aim_paths::root().join("target/release/aimctl"), data.0.join("guest"));
    start(&boot);
    let guest = boot.data.join("data/local/tmp/update-owner.apk");
    fs::copy(apk, &guest).unwrap();
    let result = run(boot.command().args([
        "shell",
        "pm",
        "install",
        "--update-ownership",
        "-i",
        "com.android.shell",
        "/data/local/tmp/update-owner.apk",
    ]));
    assert!(String::from_utf8_lossy(&result.stdout).contains("Success"));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let state = State::read(&boot.data.join("data"), &[0]).unwrap().unwrap();
        if state.settings.packages.iter().any(|p| {
            p.name == NAME && p.install_source.update_owner.as_deref() == Some("com.android.shell")
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "original install did not retain requested update owner"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    let before_dump = run(boot.command().args(["shell", "dumpsys", "package", NAME]));
    assert!(
        String::from_utf8_lossy(&before_dump.stdout)
            .contains("updateOwnerPackageName=com.android.shell")
    );
    run(boot.command().arg("stop"));
    let volume = aim_storage::data::DataImage::attach(&boot.data, None).unwrap();
    let mut store = Store::open(&boot.data.join("data"), &[0]).unwrap().unwrap();
    let mut desired = store.state().settings.clone();
    let target = desired
        .packages
        .iter_mut()
        .find(|p| p.name == NAME)
        .unwrap();
    assert_eq!(
        target.install_source.update_owner.as_deref(),
        Some("com.android.shell")
    );
    target.install_source.update_owner = None;
    store.commit_update_owner_clearings(&desired).unwrap();
    let path = boot.data.join("data/system/packages.xml");
    let written = fs::read(&path).unwrap();
    assert!(written.starts_with(aim_android_xml::abx::MAGIC));
    assert_eq!(
        written,
        fs::read(path.with_file_name("packages.xml.reservecopy")).unwrap()
    );
    assert_eq!(
        Store::open(&boot.data.join("data"), &[0])
            .unwrap()
            .unwrap()
            .state()
            .settings,
        desired
    );
    drop(store);
    volume.detach().unwrap();
    start(&boot);
    let actual = State::read(&boot.data.join("data"), &[0]).unwrap().unwrap();
    let target = actual
        .settings
        .packages
        .iter()
        .find(|p| p.name == NAME)
        .unwrap();
    assert_eq!(target.install_source.update_owner, None);
    assert_eq!(
        target.install_source.installer.as_deref(),
        Some("com.android.shell")
    );
    let expected = desired.packages.iter().find(|p| p.name == NAME).unwrap();
    assert_eq!(target.install_source, expected.install_source);
    assert_eq!(target.app_id, expected.app_id);
    assert_eq!(target.shared_user, expected.shared_user);
    assert_eq!(target.signatures, expected.signatures);
    assert_eq!(target.key_set_data, expected.key_set_data);
    assert_eq!(actual.settings.key_sets, desired.key_sets);
    assert_eq!(actual.settings.shared_users, desired.shared_users);
    let result = run(boot.command().args(["shell", "dumpsys", "package", NAME]));
    let dump = String::from_utf8(result.stdout).unwrap();
    assert!(dump.contains("installerPackageName=com.android.shell"));
    // Settings.dumpPackageLPr omits this field when its value is null.
    assert!(!dump.contains("updateOwnerPackageName="));
    println!(
        "original PMS retained native-cleared update owner and original installer/signing/UID state"
    );
}

#[test]
#[ignore = "requires aimctl, the pinned image, JDK and Android build tools; run explicitly"]
fn original_pms_reboots_after_native_shared_uid_migration() {
    use aim_services::package::{
        parse::Platform,
        pkg::booleans,
        scan::{Inputs, SharedUidMigration, SigningScan},
        write::Apks,
    };
    const NAME: &str = "org.example.aimmigration";
    const GROUP: &str = "org.example.aimmigration.uid";
    let dir = std::env::temp_dir().join(format!("aim-uidr-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let data = Data(dir);
    let old = migration_apk(&data.0, 1, false);
    let leaving = migration_apk(&data.0, 2, true);
    let boot = Boot::new(aim_paths::root().join("target/release/aimctl"), data.0.join("guest"));
    start(&boot);
    for apk in [old, leaving] {
        let output = run(boot.command().arg("install").arg(apk));
        assert!(String::from_utf8_lossy(&output.stdout).contains("Success"));
    }
    for args in [
        vec![
            "shell",
            "pm",
            "grant",
            NAME,
            "android.permission.READ_CONTACTS",
        ],
        vec![
            "shell",
            "pm",
            "set-permission-flags",
            NAME,
            "android.permission.READ_CONTACTS",
            "user-set",
        ],
        vec![
            "shell",
            "pm",
            "revoke",
            NAME,
            "android.permission.READ_CALENDAR",
        ],
        vec![
            "shell",
            "pm",
            "set-permission-flags",
            NAME,
            "android.permission.READ_CALENDAR",
            "user-set",
            "user-fixed",
        ],
        vec![
            "shell",
            "cmd",
            "appops",
            "set",
            "--uid",
            NAME,
            "RUN_IN_BACKGROUND",
            "ignore",
        ],
    ] {
        run(boot.command().args(args));
    }
    check_migrated_permissions(&boot, NAME);
    // AccessPersistence writes asynchronously. Wait for these specific states
    // on this owned image before stopping the original permission owner.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let state = State::read(&boot.data.join("data"), &[0]).unwrap().unwrap();
        let id = state
            .settings
            .packages
            .iter()
            .find(|p| p.name == NAME)
            .unwrap()
            .app_id;
        let access = state.users[0].1.access.as_ref().unwrap();
        let permissions = access
            .app_id_permissions
            .iter()
            .find(|(app_id, _)| *app_id == id);
        let modes = access
            .app_id_app_ops
            .iter()
            .find(|(app_id, _)| *app_id == id);
        // Pinned PermissionFlags: RUNTIME_GRANTED=16, USER_SET=32, USER_FIXED=64.
        if permissions.is_some_and(|(_, flags)| {
            flags
                .iter()
                .any(|(name, value)| name == "android.permission.READ_CONTACTS" && value & 48 == 48)
                && flags.iter().any(|(name, value)| {
                    name == "android.permission.READ_CALENDAR" && value & 112 == 96
                })
        }) && modes.is_some_and(|(_, modes)| {
            modes
                .iter()
                .any(|(name, mode)| name == "android:run_in_background" && *mode == 1)
        }) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "original permission owner did not persist fixture grants/mode"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
    run(boot.command().arg("stop"));
    let volume = aim_storage::data::DataImage::attach(&boot.data, None).unwrap();
    let mut store = Store::open(&boot.data.join("data"), &[0]).unwrap().unwrap();
    let saved = store
        .state()
        .settings
        .packages
        .iter()
        .find(|p| p.name == NAME)
        .unwrap()
        .clone();
    assert!(
        saved.shared_user,
        "original NEW_INSTALL_ONLY must retain an existing shared member"
    );
    let permission_state = store.state().users[0].1.access.clone().unwrap();
    let permission_file = boot
        .data
        .join("data/misc_de/0/apexdata/com.android.permission/access.abx");
    let permission_bytes = fs::read(&permission_file).unwrap();
    let mut input = store.state().clone();
    input.settings.packages.retain(|p| p.name == NAME);
    input.settings.disabled_system_packages.clear();
    let guest = boot.data.clone();
    let apks = Apks {
        signing_overrides: None,
        files: Box::new(move |p| Some(guest.join(p.trim_start_matches('/')))),
        platform: Platform::load(&common::runtime::cohort::original_image(), Default::default()).unwrap(),
    };
    let inputs = Inputs::load_verified_code(&input, &apks).unwrap();
    assert!(inputs.active[NAME].parsed.is(booleans::LEAVING_SHARED_UID));
    let mut scan = SigningScan::new(&Default::default(), &store.state().settings, 36).unwrap();
    scan.apply(&inputs.active[NAME]).unwrap();
    assert!(
        scan.migrate_single_shared_user(GROUP, SharedUidMigration::BestEffort, &inputs.disabled)
            .unwrap()
    );
    store.commit_shared_uid_migrations(&scan.settings).unwrap();
    assert_eq!(fs::read(&permission_file).unwrap(), permission_bytes);
    assert_eq!(
        store.state().users[0].1.access.as_ref(),
        Some(&permission_state)
    );
    let expected = store.state().settings.clone();
    let path = boot.data.join("data/system/packages.xml");
    assert_eq!(
        fs::read(&path).unwrap(),
        fs::read(path.with_file_name("packages.xml.reservecopy")).unwrap()
    );
    assert!(!expected.shared_users.iter().any(|g| g.name == GROUP));
    assert_eq!(
        expected
            .packages
            .iter()
            .find(|p| p.name == NAME)
            .unwrap()
            .app_id,
        saved.app_id
    );
    drop(store);
    volume.detach().unwrap();
    start(&boot);
    let after = State::read(&boot.data.join("data"), &[0]).unwrap().unwrap();
    let access = after.users[0].1.access.as_ref().unwrap();
    assert_eq!(
        access
            .app_id_permissions
            .iter()
            .find(|(id, _)| *id == saved.app_id),
        permission_state
            .app_id_permissions
            .iter()
            .find(|(id, _)| *id == saved.app_id)
    );
    assert_eq!(
        access
            .app_id_app_ops
            .iter()
            .find(|(id, _)| *id == saved.app_id),
        permission_state
            .app_id_app_ops
            .iter()
            .find(|(id, _)| *id == saved.app_id)
    );
    check_migrated_permissions(&boot, NAME);
    let migrated = after
        .settings
        .packages
        .iter()
        .find(|p| p.name == NAME)
        .unwrap();
    assert_eq!(
        (migrated.app_id, migrated.shared_user),
        (saved.app_id, false)
    );
    assert_eq!(
        migrated.signatures,
        expected
            .packages
            .iter()
            .find(|p| p.name == NAME)
            .unwrap()
            .signatures
    );
    assert!(!after.settings.shared_users.iter().any(|g| g.name == GROUP));
    for package in &expected.packages {
        let retained = after
            .settings
            .packages
            .iter()
            .find(|p| p.name == package.name)
            .unwrap();
        assert_eq!(
            (retained.app_id, retained.shared_user, &retained.signatures),
            (package.app_id, package.shared_user, &package.signatures)
        );
    }
    assert_eq!(after.settings.shared_users, expected.shared_users);
    let launch = run(boot.command().args([
        "shell",
        "am",
        "start",
        "-W",
        "-n",
        "com.android.settings/.Settings",
    ]));
    assert!(String::from_utf8_lossy(&launch.stdout).contains("Status: ok"));
    println!(
        "original PMS reboot retained migrated UID {}, grants/denials/flags/AppOps and {} package records; Settings launched",
        saved.app_id,
        expected.packages.len()
    );
}

fn check_migrated_permissions(boot: &Boot, package: &str) {
    let dump =
        run(boot
            .command()
            .args(["shell", "dumpsys", "permissionmgr", "--package", package]));
    let text = String::from_utf8(dump.stdout).unwrap();
    let granted = text
        .lines()
        .find(|line| line.contains("android.permission.READ_CONTACTS: granted="))
        .expect("fixture permission owner has no contacts grant record");
    assert!(
        granted.contains("granted=true") && granted.contains("USER_SET"),
        "{granted}"
    );
    let denied = text
        .lines()
        .find(|line| line.contains("android.permission.READ_CALENDAR: granted="))
        .expect("fixture permission owner has no calendar denial record");
    assert!(
        denied.contains("granted=false")
            && denied.contains("USER_SET")
            && denied.contains("USER_FIXED"),
        "{denied}"
    );
    let modes = run(boot.command().args([
        "shell",
        "cmd",
        "appops",
        "get",
        "--uid",
        package,
        "RUN_IN_BACKGROUND",
    ]));
    let text = String::from_utf8(modes.stdout).unwrap();
    assert!(text.contains("RUN_IN_BACKGROUND: ignore"), "{text}");
}
