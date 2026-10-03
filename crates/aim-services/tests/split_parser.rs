//! Compiled APK inputs for the cluster parser (#720). Requires the
//! pinned image and build tools; a missing input is a failed test.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use aim_services::package::parse::{Error, Platform, parse};
use aim_services::package::pkg::AndroidPackage;

struct Data(PathBuf);
impl Data {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "aim-split-parser-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Data {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn link(dir: &Path, name: &str, manifest: &str) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let xml = dir.join(format!("{name}.xml"));
    fs::write(&xml, manifest).unwrap();
    let apk = dir.join(format!("{name}.apk"));
    let result =
        Command::new(aim_paths::fetched().join("java/build-tools-36.0.0/android-16/aapt2"))
            .args(["link", "--manifest"])
            .arg(xml)
            .arg("-I")
            .arg(aim_paths::derived_image().join("system/framework/framework-res.apk"))
            .arg("-o")
            .arg(&apk)
            .output()
            .expect("pinned aapt2");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    apk
}

fn base(isolated: bool) -> String {
    format!(
        r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android"
        package="org.example.splits" android:versionCode="7" android:isolatedSplits="{isolated}"
        android:requiredSplitTypes="base__abi, base__density">
        <uses-sdk android:minSdkVersion="23" android:targetSdkVersion="35" />
        <application android:hasCode="false"><meta-data android:name="base" android:value="kept" /></application>
        </manifest>"#
    )
}

const FEATURE: &str = r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    package="org.example.splits" split="feature" android:versionCode="7" android:revisionCode="3"
    android:isFeatureSplit="true"><application android:classLoader="dalvik.system.DexClassLoader">
    <activity android:name=".Feature" android:exported="false" />
    <uses-library android:name="example.library" android:required="false" />
    <meta-data android:name="split" android:value="merged" />
    </application></manifest>"#;

const CONFIG: &str = r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    package="org.example.splits" split="config.en" android:versionCode="7" android:revisionCode="4"
    android:splitTypes="base__abi, base__density">
    <application android:hasCode="false" /></manifest>"#;

#[test]
#[ignore = "requires the pinned image and aapt2; run explicitly"]
fn compiled_split_cluster_merges_and_validates_manifests() {
    let data = Data::new();
    let cluster = data.0.join("cluster");
    link(&cluster, "middle", &base(false));
    let feature = link(&cluster, "a-feature", FEATURE);
    link(&cluster, "z-config", CONFIG);
    let platform = Platform::load(&aim_paths::derived_image(), Default::default()).unwrap();
    let pkg = parse(&cluster, "/data/app/example", 0, &platform).unwrap();
    assert_eq!(pkg.split_names.as_ref().unwrap(), &["config.en", "feature"]);
    assert_eq!(
        pkg.split_code_paths.as_ref().unwrap(),
        &[
            "/data/app/example/z-config.apk",
            "/data/app/example/a-feature.apk"
        ]
    );
    assert_eq!(pkg.split_revision_codes.as_ref().unwrap(), &[4, 3]);
    assert_eq!(pkg.split_flags.as_ref().unwrap(), &[0, 4]);
    assert_eq!(
        pkg.split_class_loader_names.as_ref().unwrap()[1].as_deref(),
        Some("dalvik.system.DexClassLoader")
    );
    assert_eq!(pkg.uses_optional_libraries, ["example.library"]);
    let activity = pkg
        .activities
        .iter()
        .find(|a| a.main.component.name == "org.example.splits.Feature")
        .unwrap();
    assert_eq!(activity.main.split_name.as_deref(), Some("feature"));
    let read = AndroidPackage::read_cache_entry(&pkg.to_cache_entry().bytes).unwrap();
    assert_eq!(
        read.split_names,
        pkg.split_names
            .as_ref()
            .map(|names| names.iter().cloned().map(Some).collect())
    );
    assert_eq!(read.split_flags, pkg.split_flags);
    assert_eq!(read.split_class_loader_names, pkg.split_class_loader_names);

    fs::copy(&feature, cluster.join("duplicate.apk")).unwrap();
    assert!(matches!(
        parse(&cluster, "/data/app/example", 0, &platform),
        Err(Error::Parse(_))
    ));
    fs::remove_file(cluster.join("duplicate.apk")).unwrap();
    link(
        &cluster,
        "a-feature",
        &FEATURE.replace("versionCode=\"7\"", "versionCode=\"8\""),
    );
    assert!(matches!(
        parse(&cluster, "/data/app/example", 0, &platform),
        Err(Error::Parse(_))
    ));

    let isolated = data.0.join("isolated");
    link(&isolated, "base", &base(true));
    link(&isolated, "feature", FEATURE);
    link(
        &isolated,
        "config",
        &CONFIG.replace(
            "split=\"config.en\"",
            "split=\"config.en\" configForSplit=\"feature\"",
        ),
    );
    let pkg = parse(&isolated, "/data/app/example", 0, &platform).unwrap();
    assert_eq!(
        pkg.split_dependencies.as_ref().unwrap(),
        &std::collections::BTreeMap::from([(0, vec![-1]), (2, vec![0, 1])])
    );
    let read = AndroidPackage::read_cache_entry(&pkg.to_cache_entry().bytes).unwrap();
    assert_eq!(
        read.split_dependencies,
        Some(vec![(0, Some(vec![-1])), (2, Some(vec![0, 1]))])
    );
    link(
        &isolated,
        "child",
        &FEATURE
            .replace("split=\"feature\"", "split=\"zchild\"")
            .replace(
                "<application",
                "<uses-split android:name=\"feature\" /><application",
            )
            .replace(".Feature", ".Child"),
    );
    let pkg = parse(&isolated, "/data/app/example", 0, &platform).unwrap();
    assert_eq!(
        pkg.split_dependencies.as_ref().unwrap().get(&3),
        Some(&vec![2])
    );
    link(
        &isolated,
        "config",
        &CONFIG.replace(
            "split=\"config.en\"",
            "split=\"config.en\" configForSplit=\"config.en\"",
        ),
    );
    assert!(matches!(
        parse(&isolated, "/data/app/example", 0, &platform),
        Err(Error::Parse(_))
    ));
    link(
        &isolated,
        "config",
        &CONFIG.replace(
            "split=\"config.en\"",
            "split=\"config.en\" configForSplit=\"feature\"",
        ),
    );
    link(
        &isolated,
        "feature",
        &FEATURE.replace(
            "<application",
            "<uses-split android:name=\"absent\" /><application",
        ),
    );
    assert!(matches!(
        parse(&isolated, "/data/app/example", 0, &platform),
        Err(Error::Parse(_))
    ));
    link(
        &isolated,
        "feature",
        &FEATURE.replace(
            "<application",
            "<uses-split android:name=\"feature\" /><application",
        ),
    );
    assert!(matches!(
        parse(&isolated, "/data/app/example", 0, &platform),
        Err(Error::Parse(_))
    ));
    let missing = data.0.join("missing");
    link(&missing, "feature", FEATURE);
    assert!(matches!(
        parse(&missing, "/data/app/example", 0, &platform),
        Err(Error::Parse(_))
    ));
    let invalid = data.0.join("invalid-types");
    link(
        &invalid,
        "base",
        &base(false).replace("base__abi, base__density", ".."),
    );
    assert!(matches!(
        parse(&invalid, "/data/app/example", 0, &platform),
        Err(Error::Parse(_))
    ));
}

#[test]
#[ignore = "requires the pinned image and aapt2; run explicitly"]
fn compiled_manifest_keysets_parse_and_roundtrip() {
    use aim_services::package::sign::deserialize_public_key;
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    let data = Data::new();
    let platform = Platform::load(&aim_paths::derived_image(), Default::default()).unwrap();
    let mut der = vec![
        0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 2, 1, 0x06, 8, 0x2a,
        0x86, 0x48, 0xce, 0x3d, 3, 1, 7, 3, 0x42, 0,
    ];
    let secret = p256::SecretKey::from_slice(&[1; 32]).unwrap();
    der.extend_from_slice(secret.public_key().to_encoded_point(false).as_bytes());
    let value = aim_android_xml::Element {
        name: "key".into(),
        attrs: vec![(
            "value".into(),
            aim_android_xml::Value::BytesBase64(der.clone()),
        )],
        content: vec![],
    }
    .string("value")
    .unwrap()
    .into_owned();
    let manifest = |body: &str| {
        format!(
            r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="org.example.keys"><uses-sdk android:minSdkVersion="23" android:targetSdkVersion="35"/><key-sets>{body}</key-sets><application android:hasCode="false"/></manifest>"#
        )
    };
    let body = format!(
        r#"<key-set android:name="z"><public-key android:name="one" android:value="{value}"/></key-set><key-set android:name="a"><public-key android:name="one"/></key-set><upgrade-key-set android:name="a"/>"#
    );
    let apk = link(&data.0.join("valid"), "base", &manifest(&body));
    let parsed = parse(&apk, "/data/app/keys/base.apk", 0, &platform).unwrap();
    let read = AndroidPackage::read_cache_entry(&parsed.to_cache_entry().bytes).unwrap();
    assert_eq!(read.upgrade_key_sets, ["a"]);
    let mapping = read.key_set_mapping.unwrap();
    assert_eq!(
        mapping
            .iter()
            .map(|(name, _)| name.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["a", "z"]
    );
    for (_, keys) in mapping {
        assert_eq!(
            deserialize_public_key(keys.unwrap()[0].as_ref().unwrap()).unwrap(),
            der
        );
    }
    for (name, body) in [
        (
            "missing",
            r#"<key-set android:name="a"><public-key android:name="one"/></key-set>"#.to_owned(),
        ),
        (
            "upgrade",
            r#"<key-set android:name="a"/><upgrade-key-set android:name="a"/>"#.to_owned(),
        ),
        (
            "collision",
            format!(
                r#"<key-set android:name="one"><public-key android:name="one" android:value="{value}"/></key-set>"#
            ),
        ),
    ] {
        let apk = link(&data.0.join(name), "base", &manifest(&body));
        assert!(
            matches!(
                parse(&apk, "/data/app/keys/base.apk", 0, &platform),
                Err(Error::Parse(_))
            ),
            "{name}"
        );
    }
    let apk = link(
        &data.0.join("invalid-key"),
        "base",
        &manifest(
            r#"<key-set android:name="a"><public-key android:name="one" android:value="invalid"/></key-set>"#,
        ),
    );
    let parsed = parse(&apk, "/data/app/keys/base.apk", 0, &platform).unwrap();
    assert!(parsed.key_set_mapping.is_empty());
}

mod common {
    pub mod java;
    pub mod runtime;
}

#[test]
#[ignore = "requires pinned image, aimctl, JDK, aapt2 and d8; run explicitly"]
fn manifest_keysets_match_original_parser() {
    use common::runtime::{Boot, run};
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    use std::fmt::Write;
    use std::time::{Duration, Instant};
    let data = Data::new();
    let java = aim_paths::fetched().join("java");
    let jdk = java.join("temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let classes = data.0.join("classes");
    let stubs = data.0.join("stubs");
    let dex = data.0.join("dex");
    for dir in [&classes, &stubs, &dex] {
        fs::create_dir(dir).unwrap();
    }
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(common::java::sources(
            &aim_paths::root().join("java/device-services/stubs"),
        )));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ManifestKeySetsOracle.java"),
        )
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/KeySetOwnerOracle.java")));
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
        .args([
            classes.join("ManifestKeySetsOracle.class"),
            classes.join("ManifestKeySetsOracle$1.class"),
            classes.join("com/android/server/pm/KeySetOwnerOracle.class"),
            classes.join("com/android/server/pm/KeySetOwnerOracle$1.class"),
        ]));
    let key = |scalar| {
        let mut der = vec![
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 2, 1, 0x06, 8, 0x2a,
            0x86, 0x48, 0xce, 0x3d, 3, 1, 7, 3, 0x42, 0,
        ];
        let secret = p256::SecretKey::from_slice(&[scalar; 32]).unwrap();
        der.extend_from_slice(secret.public_key().to_encoded_point(false).as_bytes());
        let text = aim_android_xml::Element {
            name: "key".into(),
            attrs: vec![(
                "value".into(),
                aim_android_xml::Value::BytesBase64(der.clone()),
            )],
            content: vec![],
        }
        .string("value")
        .unwrap()
        .into_owned();
        (der, text)
    };
    let (one_der, one) = key(1);
    let (two_der, two) = key(2);
    let cases = [
        ("reuse", format!(r#"<key-set android:name="z"><public-key android:name="one" android:value="{one}"/></key-set><key-set android:name="a"><public-key android:name="one"/></key-set><upgrade-key-set android:name="a"/>"#)),
        ("nullable", format!(r#"<key-set android:name="a"><public-key android:value="{one}"/></key-set>"#)),
        ("conflict", format!(r#"<key-set android:name="a"><public-key android:name="one" android:value="{one}"/><public-key android:name="one" android:value="{two}"/></key-set>"#)),
        ("collision", format!(r#"<key-set android:name="one"><public-key android:name="one" android:value="{one}"/></key-set>"#)),
        ("invalid", r#"<key-set android:name="a"><public-key android:name="one" android:value="invalid"/></key-set>"#.into()),
        ("missing", r#"<key-set android:name="a"><public-key android:name="one"/></key-set>"#.into()),
        ("empty-upgrade", r#"<key-set android:name="a"/><upgrade-key-set android:name="a"/>"#.into()),
        ("repeat-set", format!(r#"<key-set android:name="a"><public-key android:name="one" android:value="{one}"/></key-set><key-set android:name="a"><public-key android:name="two" android:value="{two}"/></key-set>"#)),
        ("sections", format!(r#"<key-set android:name="a"><public-key android:name="one" android:value="{one}"/></key-set></key-sets><key-sets><key-set android:name="a"><public-key android:name="two" android:value="{two}"/></key-set><upgrade-key-set android:name="a"/>"#)),
        ("base64-skip", format!(r#"<key-set android:name="a"><public-key android:name="one" android:value=" !{one} !"/></key-set>"#)),
        ("multiple", format!(r#"<key-set android:name="a"><public-key android:name="two" android:value="{two}"/><public-key android:name="one" android:value="{one}"/></key-set><upgrade-key-set android:name="a"/>"#)),
    ];
    let platform = Platform::load(&aim_paths::derived_image(), Default::default()).unwrap();
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let mut expected = String::new();
    let mut apks = Vec::new();
    for (name, body) in cases {
        let xml = format!(
            r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android" package="org.example.keys"><uses-sdk android:minSdkVersion="23" android:targetSdkVersion="35"/><key-sets>{body}</key-sets><application android:hasCode="false"/></manifest>"#
        );
        let apk = link(&data.0.join("apks"), name, &xml);
        match parse(&apk, "/data/app/keys/base.apk", 0, &platform) {
            Ok(package) => {
                writeln!(&mut expected, "CASE {name}.apk OK").unwrap();
                for (alias, keys) in package.key_set_mapping.iter() {
                    for key in keys {
                        let der = aim_services::package::sign::deserialize_public_key(key).unwrap();
                        writeln!(
                            &mut expected,
                            "SET {alias} {} {} {}",
                            key.class,
                            hex(&der),
                            hex(&key.bytes)
                        )
                        .unwrap();
                    }
                }
                for upgrade in package.upgrade_key_sets.iter() {
                    writeln!(&mut expected, "UPGRADE {upgrade}").unwrap();
                }
            }
            Err(Error::Parse(_)) => {
                writeln!(&mut expected, "CASE {name}.apk ERROR").unwrap();
            }
            Err(error) => panic!("native parser gap for {name}: {error}"),
        }
        apks.push(apk);
    }
    let boot = Boot {
        ctl: aim_paths::root().join("target/release/aimctl"),
        data: data.0.join("guest"),
    };
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
            "disposable boot did not complete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    let guest = boot.data.join("data/local/tmp/manifest-keysets");
    fs::create_dir(&guest).unwrap();
    fs::copy(dex.join("classes.dex"), guest.join("oracle.dex")).unwrap();
    let mut command = boot.command();
    command.args(["shell", "/system/bin/app_process", "-Djava.class.path=/data/local/tmp/manifest-keysets/oracle.dex:/system/framework/services.jar", "/system/bin", "ManifestKeySetsOracle"]);
    for apk in apks {
        let name = apk.file_name().unwrap();
        fs::copy(&apk, guest.join(name)).unwrap();
        command.arg(format!(
            "/data/local/tmp/manifest-keysets/{}",
            name.to_str().unwrap()
        ));
    }
    let original = String::from_utf8(run(&mut command).stdout).unwrap();
    assert_eq!(original, expected);
    // Compare the original global owner independently of the parser contract.
    use aim_services::package::{
        owner::key_sets,
        settings::{Package, Settings},
    };
    let mut settings = Settings {
        packages: vec![
            Package {
                name: "a".into(),
                ..Package::default()
            },
            Package {
                name: "b".into(),
                ..Package::default()
            },
        ],
        ..Settings::default()
    };
    let mut states = Vec::new();
    key_sets::register(
        &mut settings,
        "a",
        &[one_der.clone()],
        Some(&[("next".into(), vec![two_der.clone()])]),
        &["next".into()],
    )
    .unwrap();
    states.push(("first", settings.clone()));
    key_sets::register(&mut settings, "b", &[one_der.clone()], Some(&[]), &[]).unwrap();
    states.push(("shared", settings.clone()));
    key_sets::register(&mut settings, "a", &[two_der], Some(&[]), &[]).unwrap();
    states.push(("rotate", settings.clone()));
    key_sets::clear_package(&mut settings, "b").unwrap();
    states.push(("remove-shared", settings.clone()));
    key_sets::clear_package(&mut settings, "a").unwrap();
    states.push(("remove-last", settings.clone()));
    key_sets::register(&mut settings, "a", &[one_der], None, &[]).unwrap();
    states.push(("reallocate", settings.clone()));
    settings
        .key_sets
        .public_keys
        .extend([(4, key(2).0), (5, key(1).0)]);
    settings.key_sets.key_sets.push((4, vec![3, 4]));
    settings.key_sets.last_issued_key_id = 20;
    settings.key_sets.last_issued_key_set_id = 30;
    key_sets::restore(&mut settings).unwrap();
    assert_eq!(
        settings
            .key_sets
            .public_keys
            .iter()
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        [3, 5]
    );
    assert_eq!(settings.key_sets.key_sets, [(3, vec![3])]);
    states.push(("restore-orphan", settings));
    let output = String::from_utf8(run(boot.command().args([
        "shell", "/system/bin/app_process", "-Djava.class.path=/data/local/tmp/manifest-keysets/oracle.dex:/system/framework/services.jar",
        "/system/bin", "com.android.server.pm.KeySetOwnerOracle", "/data/local/tmp/manifest-keysets/reuse.apk", "/data/local/tmp/manifest-keysets/repeat-set.apk",
    ])).stdout).unwrap();
    let mut lines = output.lines();
    for (name, state) in states {
        assert_eq!(lines.next(), Some(format!("STATE {name}").as_str()));
        for package in &state.packages {
            assert_eq!(
                lines.next(),
                Some(
                    format!(
                        "PACKAGE {} {}",
                        package.name, package.key_set_data.proper_signing_key_set
                    )
                    .as_str()
                )
            );
            for (alias, id) in &package.key_set_data.defined_key_sets {
                assert_eq!(
                    lines.next(),
                    Some(format!("ALIAS {} {alias} {id}", package.name).as_str())
                );
            }
            for id in &package.key_set_data.upgrade_key_sets {
                assert_eq!(
                    lines.next(),
                    Some(format!("UPGRADE {} {id}", package.name).as_str())
                );
            }
        }
        let global = lines.next().unwrap().strip_prefix("GLOBAL ").unwrap();
        let bytes: Vec<_> = (0..global.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&global[i..i + 2], 16).unwrap())
            .collect();
        let original = Settings::parse(&aim_android_xml::read(&bytes).unwrap()).unwrap();
        assert_eq!(original.key_sets, state.key_sets, "{name}");
    }
    assert!(lines.next().is_none());
}
