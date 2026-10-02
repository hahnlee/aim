//! Original PMS reads native signing persistence on a disposable data image.
use aim_services::package::{State, owner::Store};
use std::fs;
use std::time::{Duration, Instant};

mod common {
    pub mod runtime;
}
use common::runtime::{Boot, Data, run};

fn start(boot: &Boot) {
    run(boot.command().args(["start", "--windows"]));
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
fn original_pms_boots_with_native_signature_persistence() {
    let dir = std::env::temp_dir().join(format!("aim-sigr-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let data = Data(dir);
    let boot = Boot {
        ctl: aim_paths::root().join("target/release/aimctl"),
        data: data.0.join("guest"),
    };
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
        .arg(aim_paths::derived_image().join("system/framework/framework-res.apk"))
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
    let boot = Boot {
        ctl: aim_paths::root().join("target/release/aimctl"),
        data: data.0.join("guest"),
    };
    start(&boot);
    for apk in [old, leaving] {
        let output = run(boot.command().arg("install").arg(apk));
        assert!(String::from_utf8_lossy(&output.stdout).contains("Success"));
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
    let mut input = store.state().clone();
    input.settings.packages.retain(|p| p.name == NAME);
    input.settings.disabled_system_packages.clear();
    let guest = boot.data.clone();
    let apks = Apks {
        files: Box::new(move |p| Some(guest.join(p.trim_start_matches('/')))),
        platform: Platform::load(&aim_paths::derived_image(), Default::default()).unwrap(),
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
        "original PMS reboot retained migrated UID {} and {} package records; Settings launched",
        saved.app_id,
        expected.packages.len()
    );
}
