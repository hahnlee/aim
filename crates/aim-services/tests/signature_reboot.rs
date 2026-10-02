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
