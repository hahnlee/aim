//! Canonical native preferred backup checked by the pinned original owner.
use std::{
    fs,
    process::Command,
    time::{Duration, Instant},
};
mod common {
    pub mod java;
    pub mod runtime;
}
use common::runtime::{Boot, Data, run};
#[test]
#[ignore = "requires original pinned image, aimctl, JDK and d8; run explicitly"]
fn preferred_backup_matches_original_owner() {
    let directory = std::env::temp_dir().join(format!("aim-prefs-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let data = Data(directory);
    let repo = aim_paths::root();
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
        .args(common::java::sources(
            &repo.join("java/device-services/stubs"),
        )));
    let extra_stubs = data.0.join("extra-stubs");
    fs::create_dir(&extra_stubs).unwrap();
    fs::write(extra_stubs.join("Xml.java"), r#"package android.util;
      public class Xml {
        public static com.android.modules.utils.TypedXmlSerializer newFastSerializer() { throw new RuntimeException(); }
        public static com.android.modules.utils.TypedXmlPullParser resolvePullParser(java.io.InputStream input) throws java.io.IOException { throw new RuntimeException(); }
      }"#).unwrap();
    fs::write(extra_stubs.join("PreferredActivity.java"), r#"package com.android.server.pm;
      public class PreferredActivity {
        public PreferredActivity(com.android.modules.utils.TypedXmlPullParser parser) throws java.io.IOException { throw new RuntimeException(); }
        public void writeToXml(com.android.modules.utils.TypedXmlSerializer serializer, boolean full) throws java.io.IOException { throw new RuntimeException(); }
      }"#).unwrap();
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .arg("-classpath")
        .arg(&stubs)
        .arg(extra_stubs.join("Xml.java"))
        .arg(extra_stubs.join("PreferredActivity.java")));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .arg(repo.join("crates/aim-services/tests/fixtures/PreferredSerializationOracle.java")));
    let mut class_files = Vec::new();
    let mut pending = vec![classes.clone()];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "class") {
                class_files.push(path);
            }
        }
    }
    class_files.sort();
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
        .args(class_files));
    common::java::check_linkage(
        &dex.join("classes.dex"),
        &["/system/framework/services.jar"],
    )
    .unwrap();
    let boot = Boot {
        ctl: repo.join("target/release/aimctl"),
        data: data.0.join("g"),
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
            "preferred oracle boot incomplete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    let guest = boot.data.join("data/local/tmp/preferred-owner");
    fs::create_dir(&guest).unwrap();
    aim_storage::guest_inode::record(
        &guest,
        aim_storage::guest_inode::GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o700),
            ..Default::default()
        },
    )
    .unwrap();
    fs::copy(dex.join("classes.dex"), guest.join("oracle.dex")).unwrap();
    let mut state = aim_services::package::preferred::Preferred::default();
    for name in ["one", "two"] {
        let mut filter = aim_services::package::intent_filter::IntentFilter::default();
        filter.add_action("android.intent.action.VIEW");
        filter.add_category("android.intent.category.DEFAULT");
        filter.add_data_scheme("https");
        filter.add_data_authority("*.example.com", Some("443"));
        filter.add_data_path(
            aim_services::package::intent_filter::PatternMatcher::new("/<&é", 1).unwrap(),
        );
        filter.add_data_type("text/*").unwrap();
        let component =
            aim_services::package::preferred::unflatten(&format!("{name}/.Main")).unwrap();
        state.add_preferred(
            aim_services::package::preferred::PreferredActivity::new(
                filter,
                0x208001,
                Some(vec![component.clone()]),
                component,
                name == "one",
            ),
            false,
        );
    }
    let mut expected = state.preferred_backup(&[1, 0]).unwrap();
    fs::write(guest.join("input-0.xml"), &expected).unwrap();
    let mut case_count = 1;
    let mut double_bits = vec![
        0,
        1u64 << 63,
        1,
        (1u64 << 63) | 1,
        0x0010000000000000,
        0x7fefffffffffffff,
        0x7ff0000000000000,
        0xfff0000000000000,
        0x7ff8000000000000,
        1.0f64.to_bits(),
        1e23f64.to_bits(),
        1e7f64.to_bits(),
        1e-3f64.to_bits(),
    ];
    let mut random = 0x42c43e654abc1123u64;
    for _ in 0..1024 {
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        double_bits.push(random);
    }
    for (index, bits) in double_bits.into_iter().enumerate() {
        let mut filter = aim_services::package::intent_filter::IntentFilter::default();
        filter.add_action("android.intent.action.VIEW");
        let bundle = aim_services::package::restrictions::persistable::Bundle {
            entries: vec![(
                Some("number".into()),
                aim_services::package::restrictions::persistable::Value::Double(
                    aim_services::package::restrictions::persistable::Double::new(f64::from_bits(
                        bits,
                    )),
                ),
            )],
        };
        filter.extras = Some(bundle.parcel().unwrap().data().to_vec());
        let component =
            aim_services::package::preferred::unflatten(&format!("double{index}/.Main")).unwrap();
        let mut single = aim_services::package::preferred::Preferred::default();
        single.add_preferred(
            aim_services::package::preferred::PreferredActivity::new(
                filter,
                0x208001,
                Some(vec![component.clone()]),
                component,
                true,
            ),
            false,
        );
        let bytes = single.preferred_backup(&[0]).unwrap();
        fs::write(guest.join(format!("input-{case_count}.xml")), &bytes).unwrap();
        expected.extend_from_slice(&bytes);
        case_count += 1;
    }

    let output = boot.client(1000).args([
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/preferred-owner/oracle.dex:/system/framework/services.jar",
        "/system/bin", "com.android.server.pm.PreferredSerializationOracle", "/data/local/tmp/preferred-owner", &case_count.to_string(),
    ]).output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("original preferred serializer checks passed")
    );
    let actual = fs::read(guest.join("output.xml")).unwrap();
    let mismatch = actual.iter().zip(&expected).position(|(a, b)| a != b);
    if let Some(index) = mismatch {
        let start = index.saturating_sub(80);
        panic!(
            "preferred XML mismatch at {index}: actual={} expected={}",
            String::from_utf8_lossy(&actual[start..actual.len().min(index + 80)]),
            String::from_utf8_lossy(&expected[start..expected.len().min(index + 80)])
        );
    }
    assert_eq!(actual.len(), expected.len());
    drop(boot);
    drop(data);
}
