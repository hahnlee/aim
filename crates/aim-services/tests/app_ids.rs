//! Compare UID-slot operations with the image's original AppIdSettingMap.
use aim_services::package::owner::app_ids::{AppIds, Error, Owner};
use std::fs;
use std::process::Command;
use std::time::{Duration, Instant};

mod common {
    pub mod runtime;
}
use common::runtime::{Boot, Data, run, sources};

#[test]
#[ignore = "requires aimctl, the pinned derived image, JDK and d8; run explicitly"]
fn allocation_matches_the_original_runtime() {
    let dir = std::env::temp_dir().join(format!("aim-ids-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let data = Data(dir);
    let java = aim_paths::fetched().join("java");
    let jdk = java.join("temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let classes = data.0.join("classes");
    let stubs = data.0.join("stubs");
    let dex = data.0.join("dex");
    for path in [&classes, &stubs, &dex] {
        fs::create_dir(path).unwrap();
    }
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(sources(
            &aim_paths::root().join("java/device-services/stubs"),
        )));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/AppIdsOracle.java"),
        ));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(java.join("build-tools-36.0.0/android-16/lib/d8.jar"))
        .args([
            "com.android.tools.r8.D8",
            "--release",
            "--min-api",
            "36",
            "--lib",
        ])
        .arg(&jdk)
        .arg("--classpath")
        .arg(&stubs)
        .arg("--output")
        .arg(&dex)
        .arg(classes.join("com/android/server/pm/AppIdsOracle.class")));
    let boot = Boot {
        ctl: aim_paths::root().join("target/release/aimctl"),
        data: data.0.join("guest"),
    };
    run(boot.command().args(["start", "--windows"]));
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let output = boot
            .command()
            .args(["shell", "getprop", "sys.boot_completed"])
            .output()
            .unwrap();
        if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "1" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "disposable boot did not complete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    fs::copy(
        dex.join("classes.dex"),
        boot.data.join("data/local/tmp/app-ids.dex"),
    )
    .unwrap();
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/app-ids.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.AppIdsOracle",
    ]));

    let a = Owner::SharedUser("a".into());
    let b = Owner::SharedUser("b".into());
    let mut ids = AppIds::default();
    let mut lines = vec![ids.register_existing(10002, a.clone()).is_ok().to_string()];
    for _ in 0..3 {
        lines.push(ids.acquire(b.clone()).unwrap().to_string());
    }
    ids.remove(10001);
    lines.push(ids.acquire(b.clone()).unwrap().to_string());
    ids.replace(10001, a.clone()).unwrap();
    lines.push((ids.get(10001) == Some(&a)).to_string());
    ids.replace(1000, a.clone()).unwrap();
    lines.push((ids.get(1000) == Some(&a)).to_string());
    ids.remove(10010);
    lines.push(ids.acquire(b.clone()).unwrap().to_string());
    let mut restart = AppIds::default();
    restart.register_existing(10002, a.clone()).unwrap();
    lines.push(restart.acquire(b.clone()).unwrap().to_string());
    let mut full = AppIds::default();
    let mut last = -1;
    for _ in 0..10000 {
        last = full.acquire(a.clone()).unwrap();
    }
    lines.push(last.to_string());
    assert_eq!(full.acquire(b.clone()), Err(Error::Exhausted));
    lines.push("-1".into());
    full.remove(19999);
    assert_eq!(full.acquire(b), Err(Error::Exhausted));
    lines.push("-1".into());
    assert_eq!(
        String::from_utf8(original.stdout).unwrap(),
        lines.join("\n") + "\n"
    );

    // Original SystemConfig reads these disposable vendor/OEM fixtures
    // with zero partition permissions: the UID tag has no allow-bit gate.
    let root = boot.data.join("data/local/tmp/uid-image");
    let inputs = [
        (
            "vendor",
            r#"<permissions>
            <oem-defined-uid name="android.uid.Aa" uid="2900" />
            <oem-defined-uid name="android.uid.BB" uid="2900" />
            <oem-defined-uid name="android.uid.update" uid="2901" />
            <oem-defined-uid name="android.uid.signed" uid="+2902" />
            <oem-defined-uid name="android.uid.unicode" uid="٢٩٠٣" />
            <oem-defined-uid name="android.uid.fullwidth" uid="２９０４" />
            <oem-defined-uid name="android.uid.min" uid="-2147483648" />
            <oem-defined-uid name="android.uid.max" uid="2147483647" />
            <oem-defined-uid name="android.uid.space" uid=" 2905" />
            <oem-defined-uid name="android.uid.overflow" uid="2147483648" />
            <oem-defined-uid name="" uid="2906" />
            <oem-defined-uid name="android.uid.empty" uid="" />
            <oem-defined-uid name="android.uid.missing" />
            </permissions>"#,
        ),
        (
            "oem",
            r#"<config><oem-defined-uid name="android.uid.update" uid="2999" />
            <oem-defined-uid name="android.uid.invalid" uid="-1" /></config>"#,
        ),
    ];
    for (partition, xml) in inputs {
        let dir = root.join(partition).join("etc/permissions");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("uids.xml"), xml).unwrap();
    }
    let config = aim_services::package::system_config::SystemConfig::read(&root, &|_| None);
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/app-ids.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.AppIdsOracle",
        "oem",
        "/data/local/tmp/uid-image/vendor/etc/permissions",
        "/data/local/tmp/uid-image/oem/etc/permissions",
    ]));
    let expected = config
        .oem_defined_uids
        .iter()
        .map(|(name, id)| format!("{name} {id}\n"))
        .collect::<String>();
    assert_eq!(String::from_utf8(original.stdout).unwrap(), expected);
    assert_eq!(config.oem_defined_uids.len(), 9);
    assert_eq!(config.rejected_oem_uids.len(), 5);
    assert!(
        config
            .rejected_oem_uids
            .iter()
            .all(|r| r.path == "vendor/etc/permissions/uids.xml")
    );
    let bootstrap = aim_services::package::owner::shared_users::Bootstrap::new(&config);
    assert_eq!(bootstrap.shared_users.len(), 14);
    assert_eq!(bootstrap.rejected.len(), 4);

    let state = aim_services::package::State::read(&boot.data.join("data"), &[0])
        .unwrap()
        .unwrap();
    let before = state.clone();
    assert!(!state.settings.packages.is_empty());
    assert!(!state.settings.shared_users.is_empty());
    let restored = AppIds::restore(&state.settings).unwrap();
    for group in &state.settings.shared_users {
        assert_eq!(
            restored.get(group.app_id),
            Some(&Owner::SharedUser(group.name.clone()))
        );
    }
    let seeded = aim_services::package::owner::shared_users::Bootstrap::new(&Default::default());
    // PMS prunes unused seeded groups after scanning; the bootstrap
    // input must not recreate them in a restored published snapshot.
    let mut retained = 0;
    for (name, group) in seeded.shared_users {
        if let Some(saved) = state.settings.shared_users.iter().find(|g| g.name == name) {
            assert_eq!(saved.app_id, group.app_id);
            assert_eq!(restored.get(group.app_id), Some(&Owner::SharedUser(name)));
            retained += 1;
        }
    }
    assert!(retained > 0);
    let mut merged = aim_services::package::owner::shared_users::Bootstrap::restore(
        &Default::default(),
        &state.settings,
    )
    .unwrap();
    merged.prune_unused(&state.settings);
    assert_eq!(merged.shared_users.len(), state.settings.shared_users.len());
    for saved in &state.settings.shared_users {
        let group = &merged.shared_users[&saved.name];
        assert_eq!(group.app_id, saved.app_id);
        assert_eq!(group.signatures, saved.signatures);
        assert_eq!(merged.ids.get(saved.app_id), restored.get(saved.app_id));
    }
    for saved in &state.settings.packages {
        assert_eq!(merged.ids.get(saved.app_id), restored.get(saved.app_id));
    }
    let image = aim_paths::derived_image();
    let mut platform =
        aim_services::package::parse::Platform::load(&image, Default::default()).unwrap();
    let density =
        String::from_utf8(run(boot.command().args(["shell", "wm", "density"])).stdout).unwrap();
    let density = density
        .lines()
        .filter_map(|line| {
            line.strip_prefix("Physical density: ")
                .or_else(|| line.strip_prefix("Override density: "))
                .map(|n| n.parse().unwrap())
        })
        .last()
        .expect("original display density");
    platform.density_dpi = Some(density);
    let data_files = boot.data.join("data");
    let apks = aim_services::package::write::Apks {
        files: Box::new(move |path| {
            Some(if let Some(relative) = path.strip_prefix("/data/") {
                data_files.join(relative)
            } else {
                image.join(path.trim_start_matches('/'))
            })
        }),
        platform,
    };
    let inputs = aim_services::package::scan::Inputs::load(&state, &apks).unwrap();
    assert_eq!(inputs.active.len(), state.settings.packages.len());
    for (name, record) in &inputs.active {
        assert_eq!(&record.identity.internal_name, name);
        assert_eq!(
            merged.ids.get(record.settings.app_id),
            restored.get(record.settings.app_id)
        );
    }
    let static_libraries: Vec<_> = inputs
        .active
        .values()
        .filter(|r| r.parsed.static_shared_library_name.is_some())
        .collect();
    assert!(!static_libraries.is_empty());
    for record in static_libraries {
        assert_ne!(record.identity.internal_name, record.identity.manifest_name);
        assert_eq!(
            record.identity.internal_name,
            format!(
                "{}_{}",
                record.identity.manifest_name, record.parsed.static_shared_lib_version
            )
        );
    }
    for package in &state.settings.packages {
        if package.shared_user {
            assert!(matches!(
                restored.get(package.app_id),
                Some(Owner::SharedUser(_))
            ));
        } else {
            assert_eq!(
                restored.get(package.app_id),
                Some(&Owner::Package(package.name.clone()))
            );
        }
    }
    assert_eq!(state, before);
    println!(
        "restored {} active packages and {} shared UID groups",
        state.settings.packages.len(),
        state.settings.shared_users.len()
    );
}
