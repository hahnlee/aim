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

// Construct disposable malformed ZIPs from inputs, without editing any APK.
fn resource_apk(manifest: &[u8], resource: Option<(u16, u32, &[u8])>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    let mut entries = vec![(
        "AndroidManifest.xml",
        0,
        manifest.len() as u32,
        crc32fast::hash(manifest),
        manifest,
    )];
    if let Some((method, size, payload)) = resource {
        entries.push((
            "resources.arsc",
            method,
            size,
            crc32fast::hash(payload) ^ 1,
            payload,
        ));
    }
    let count = entries.len() as u16;
    for (name, method, size, crc, payload) in entries {
        let offset = out.len() as u32;
        let padding = (4 - ((out.len() + 30 + name.len() + 4) % 4)) % 4;
        out.extend_from_slice(&0x04034b50u32.to_le_bytes());
        for value in [20u16, 0, method, 0, 0] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in [crc, payload.len() as u32, size] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in [name.len() as u16, padding as u16 + 4] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&0xffffu16.to_le_bytes());
        out.extend_from_slice(&(padding as u16).to_le_bytes());
        out.extend(std::iter::repeat_n(0, padding));
        out.extend_from_slice(payload);
        central.extend_from_slice(&0x02014b50u32.to_le_bytes());
        for value in [20u16, 20, 0, method, 0, 0] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        for value in [crc, payload.len() as u32, size] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        for value in [name.len() as u16, 0, 0, 0, 0] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        for value in [0u32, offset] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        central.extend_from_slice(name.as_bytes());
    }
    let offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x06054b50u32.to_le_bytes());
    for value in [0u16, 0, count, count] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for value in [central.len() as u32, offset] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[test]
#[ignore = "requires pinned image, aimctl, JDK, aapt2 and d8; run explicitly"]
fn compiled_update_ownership_xml_reads_selected_asset_and_raw_events() {
    use aim_services::package::{
        owner::update_ownership::read_denylist,
        parse::resources::{Config, Resources, Table},
    };
    use common::runtime::{Boot, run};
    use std::fmt::Write;
    use std::time::{Duration, Instant};
    let data = Data::new();
    let res = data.0.join("res");
    for directory in ["xml", "xml-en", "values"] {
        fs::create_dir_all(res.join(directory)).unwrap();
    }
    fs::write(
        res.join("xml/denylist.xml"),
        r#"<list>
        <deny-ownership> one </deny-ownership>
        <deny-ownership>one</deny-ownership><deny-ownership>one</deny-ownership>
        <deny-ownership>BB</deny-ownership><deny-ownership>Aa</deny-ownership>
        <deny-ownership> </deny-ownership><deny-ownership/>
        <deny-ownership> </deny-ownership><deny-ownership> </deny-ownership>
        <deny-ownership> </deny-ownership><deny-ownership>　</deny-ownership>
        <deny-ownership><deny-ownership>skipped</deny-ownership></deny-ownership>
        <other><deny-ownership>nested</deny-ownership></other>
        <deny-ownership>first<other/>last</deny-ownership>
    </list>"#,
    )
    .unwrap();
    fs::copy(res.join("xml/denylist.xml"), res.join("xml/mixed.xml")).unwrap();
    fs::write(
        res.join("xml-en/denylist.xml"),
        "<list><deny-ownership>english</deny-ownership></list>",
    )
    .unwrap();
    let mut long = String::from("<list>");
    for i in 0..502 {
        // Duplicates do not advance the limit.
        long.push_str(&format!(
            "<deny-ownership>p{i}</deny-ownership><deny-ownership>p{i}</deny-ownership>"
        ));
    }
    long.push_str("</list>");
    fs::write(res.join("xml/long.xml"), long).unwrap();
    fs::write(
        res.join("values/aliases.xml"),
        "<resources><item type=\"xml\" name=\"alias\">@xml/mixed</item></resources>",
    )
    .unwrap();
    let aapt = aim_paths::fetched().join("java/build-tools-36.0.0/android-16/aapt2");
    let compiled = data.0.join("compiled.zip");
    let result = Command::new(&aapt)
        .arg("compile")
        .arg("--dir")
        .arg(&res)
        .arg("-o")
        .arg(&compiled)
        .output()
        .expect("pinned aapt2");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let manifest = data.0.join("manifest.xml");
    let framework_path = aim_paths::derived_image().join("system/framework/framework-res.apk");
    let mut apks = Vec::new();
    let cluster = data.0.join("split-cluster");
    fs::create_dir(&cluster).unwrap();
    for name in ["mixed", "alias", "long", "denylist"] {
        fs::write(&manifest, format!(r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android"
            package="org.example.denylist"><uses-sdk android:minSdkVersion="23" android:targetSdkVersion="35"/>
            <uses-permission android:name="android.permission.INSTALL_PACKAGES"/>
            <application android:hasCode="false"><property android:name="android.app.PROPERTY_LEGACY_UPDATE_OWNERSHIP_DENYLIST"
            android:resource="@xml/{name}"/></application></manifest>"#)).unwrap();
        let apk_path = if name == "denylist" {
            cluster.join("base.apk")
        } else {
            data.0.join(format!("{name}.apk"))
        };
        let mut command = Command::new(&aapt);
        command
            .arg("link")
            .arg("--manifest")
            .arg(&manifest)
            .arg("-I")
            .arg(&framework_path)
            .arg("-o")
            .arg(&apk_path)
            .arg(&compiled);
        if name == "denylist" {
            command
                .arg("--split")
                .arg(format!("{}:en", cluster.join("config.en.apk").display()));
        }
        let result = command.output().expect("pinned aapt2");
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        apks.push(if name == "denylist" {
            cluster.clone()
        } else {
            apk_path
        });
    }
    link(
        &cluster,
        "config.code",
        r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android"
        package="org.example.denylist" split="config.code"><application android:hasCode="false"/></manifest>"#,
    );
    let apk = aim_apps::apk::Apk::open(&apks[0]).unwrap();
    let framework = aim_apps::apk::Apk::open(&framework_path).unwrap();
    let table = Table::parse(&apk.file("resources.arsc").unwrap()).unwrap();
    let framework = Table::parse(&framework.file("resources.arsc").unwrap()).unwrap();
    let mut resources = Resources {
        tables: vec![&framework, &table],
        overlays: &[],
        config: Config::default(),
    };
    let file = |cookie, path: &str| {
        assert_eq!(cookie, 1, "XML must come from the selected app asset");
        apk.file(path)
    };
    let id = table.id("xml", "denylist").unwrap();
    let expected = aim_services::package::info::array_order(
        [" one ", "one", "BB", "Aa", " ", " ", " ", "nested", "first"]
            .map(str::to_owned)
            .to_vec(),
        |s| s,
    );
    assert_eq!(read_denylist(&resources, id, file).unwrap(), expected);
    assert_eq!(
        read_denylist(&resources, table.id("xml", "alias").unwrap(), file).unwrap(),
        expected
    );
    let list = read_denylist(&resources, table.id("xml", "long").unwrap(), file).unwrap();
    assert_eq!(list.len(), 501);
    assert!(list.contains(&"p500".into()));
    assert!(!list.contains(&"p501".into()));
    use aim_services::package::{
        owner::update_ownership::UpdateOwnership,
        pkg::{Property, PropertyValue, UsesPermission},
        settings::{FLAG_SYSTEM, Package, Settings},
        system_config::SystemConfig,
    };
    let provider = Package {
        name: "provider".into(),
        flags: FLAG_SYSTEM,
        ..Package::default()
    };
    let parsed = AndroidPackage {
        properties: Some(vec![(
            "android.app.PROPERTY_LEGACY_UPDATE_OWNERSHIP_DENYLIST".into(),
            Property {
                name: None,
                package_name: None,
                class_name: None,
                value: PropertyValue::Resource(id as i32),
            },
        )]),
        uses_permissions: vec![UsesPermission {
            name: Some("android.permission.INSTALL_PACKAGES".into()),
            flags: 0,
        }],
        ..AndroidPackage::default()
    };
    let mut owner = UpdateOwnership::default();
    owner.queue(&provider, &parsed);
    let mut target = Package {
        name: "one".into(),
        ..Package::default()
    };
    target.install_source.update_owner = Some("saved.installer".into());
    let mut settings = Settings {
        packages: vec![provider, target],
        ..Settings::default()
    };
    let config = SystemConfig::default();
    let before = (owner.clone(), settings.clone());
    assert!(
        owner
            .complete_resource_read(
                "provider",
                &mut settings,
                &config,
                &resources,
                id,
                |_, _| Err(aim_apps::res::bad("missing provider asset"))
            )
            .is_err()
    );
    assert_eq!((owner.clone(), settings.clone()), before);
    assert_eq!(
        owner
            .complete_resource_read("provider", &mut settings, &config, &resources, id, file)
            .unwrap(),
        ["one"]
    );
    assert_eq!(settings.packages[1].install_source.update_owner, None);
    assert_eq!(owner.is_provider(Some("provider")), Ok(true));
    assert_eq!(owner.is_denylisted(" one "), Ok(true));
    let before = (owner.clone(), settings.clone());
    assert!(
        owner
            .complete_resource_read(
                "removed",
                &mut settings,
                &config,
                &resources,
                id,
                |_, _| panic!("unqueued provider must not open resources")
            )
            .is_err()
    );
    assert_eq!((owner, settings), before);
    // Drive the guest path reader with a parsed split cluster, including a
    // code-only APK whose absent table must not shift the XML source cookie.
    use aim_services::package::write::Apks;
    let cluster_root = cluster.clone();
    let image_root = aim_paths::derived_image();
    let apks_reader = Apks {
        files: Box::new(move |path| {
            if path == "/data/app/provider" {
                return Some(cluster_root.clone());
            }
            if let Some(name) = path.strip_prefix("/data/app/provider/") {
                return Some(cluster_root.join(name));
            }
            Some(image_root.join(path.trim_start_matches('/')))
        }),
        platform: Platform::load(&aim_paths::derived_image(), Default::default()).unwrap(),
    };
    let parsed = apks_reader.parsed_path("/data/app/provider", 0).unwrap();
    let provider = Package {
        name: parsed.package_name.clone(),
        flags: FLAG_SYSTEM,
        ..Package::default()
    };
    let mut owner = UpdateOwnership::default();
    owner.queue(&provider, &parsed);
    owner.queue(&provider, &parsed);
    let mut settings = Settings {
        packages: vec![provider],
        ..Settings::default()
    };
    let mut bad_reader = Apks {
        files: Box::new(|_| None),
        platform: Platform::load(&aim_paths::derived_image(), Default::default()).unwrap(),
    };
    let before = (owner.clone(), settings.clone());
    assert!(
        owner
            .complete_next_apk_read(&mut settings, &config, &bad_reader, Config::default())
            .is_err()
    );
    assert_eq!((owner.clone(), settings.clone()), before);
    let corrupt = data.0.join("corrupt-split.apk");
    fs::write(&corrupt, b"not a ZIP archive").unwrap();
    let cluster_root = cluster.clone();
    bad_reader.files = Box::new(move |path| {
        if path.ends_with("/config.code.apk") {
            Some(corrupt.clone())
        } else {
            path.strip_prefix("/data/app/provider/")
                .map(|name| cluster_root.join(name))
        }
    });
    assert!(
        owner
            .complete_next_apk_read(&mut settings, &config, &bad_reader, Config::default())
            .is_err()
    );
    assert_eq!((owner.clone(), settings.clone()), before);
    let mut null_split = parsed.clone();
    null_split.split_code_paths = Some(vec![None]);
    let mut null_owner = UpdateOwnership::default();
    null_owner.queue(&settings.packages[0], &null_split);
    let null_before = (null_owner.clone(), settings.clone());
    assert!(
        null_owner
            .complete_next_apk_read(&mut settings, &config, &apks_reader, Config::default())
            .is_err()
    );
    assert_eq!((null_owner, settings.clone()), null_before);
    assert_eq!((owner.clone(), settings.clone()), before);
    let resource_config = Config {
        language: *b"en",
        sdk_version: apks_reader.platform.sdk as u16,
        ..Config::default()
    };
    assert!(
        owner
            .complete_next_apk_read(&mut settings, &config, &apks_reader, resource_config)
            .unwrap()
            .is_empty()
    );
    assert_eq!(owner.next_provider(), Some(parsed.package_name.as_str()));
    assert!(owner.is_denylisted("english").is_err());
    assert!(
        owner
            .complete_next_apk_read(&mut settings, &config, &apks_reader, Config::default())
            .unwrap()
            .is_empty()
    );
    assert_eq!(owner.next_provider(), None);
    assert_eq!(owner.is_denylisted("english"), Ok(true));
    assert_eq!(owner.is_denylisted("one"), Ok(true));
    resources.config.language = *b"en";
    assert_eq!(read_denylist(&resources, id, file).unwrap(), ["english"]);
    assert!(read_denylist(&resources, 0, file).is_err());
    assert!(
        read_denylist(&resources, id, |_, _| Err(aim_apps::res::bad(
            "missing asset"
        )))
        .is_err()
    );
    let bytes = apk.file("res/xml-en/denylist.xml").unwrap();
    assert!(read_denylist(&resources, id, |_, _| Ok(bytes[..bytes.len() - 1].to_vec())).is_err());
    let mut unbalanced = bytes.clone();
    let last = aim_apps::res::chunks(&bytes).next().unwrap().unwrap();
    let child_chunks: Vec<_> = aim_apps::res::chunks(&last.data[last.header..])
        .map(Result::unwrap)
        .collect();
    let end_size = child_chunks.last().unwrap().data.len();
    unbalanced.truncate(unbalanced.len() - end_size);
    let size = unbalanced.len() as u32;
    unbalanced[4..8].copy_from_slice(&size.to_le_bytes());
    assert!(read_denylist(&resources, id, |_, _| Ok(unbalanced)).is_err());

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
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/UpdateOwnershipResourceOracle.java"),
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
        .args([
            classes.join("com/android/server/pm/UpdateOwnershipResourceOracle.class"),
            classes.join("com/android/server/pm/UpdateOwnershipResourceOracle$1.class"),
        ]));
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
    let guest = boot.data.join("data/local/tmp/update-ownership");
    fs::create_dir(&guest).unwrap();
    let policy_root = data.0.join("policy");
    let policy_dir = policy_root.join("system/etc/permissions");
    fs::create_dir_all(&policy_dir).unwrap();
    fs::write(
        policy_dir.join("ownership.xml"),
        r#"<permissions>
        <update-ownership package="app" installer="first"/>
        <update-ownership package="app" installer="second"/>
        <update-ownership package="app" installer=""/>
        <update-ownership package="missing"/>
        <update-ownership installer="missing"/>
        <update-ownership package="" installer="missing"/>
        <update-ownership package=" " installer=" "/>
        <other><update-ownership package="nested" installer="ignored"/></other>
    </permissions>"#,
    )
    .unwrap();
    let policy = SystemConfig::read(&policy_root, &|_| None);
    fs::create_dir(guest.join("policy")).unwrap();
    fs::copy(
        policy_dir.join("ownership.xml"),
        guest.join("policy/ownership.xml"),
    )
    .unwrap();
    fs::copy(dex.join("classes.dex"), guest.join("oracle.dex")).unwrap();
    let mut command = boot.command();
    command.args(["shell", "/system/bin/app_process", "-Djava.class.path=/data/local/tmp/update-ownership/oracle.dex:/system/framework/services.jar", "/system/bin", "com.android.server.pm.UpdateOwnershipResourceOracle"]);
    command.arg("/data/local/tmp/update-ownership/policy");
    let mut native = String::from("POLICY");
    for name in ["app", "missing", " ", "nested"] {
        native.push(' ');
        if let Some(installer) = policy.system_app_update_owners.get(name) {
            for b in installer.as_bytes() {
                write!(&mut native, "{b:02x}").unwrap();
            }
        } else {
            native.push_str("null");
        }
    }
    native.push('\n');
    let english = vec!["english".to_owned()];
    for (apk, contents) in apks.iter().zip([&expected, &expected, &list, &english]) {
        let name = apk.file_name().unwrap().to_str().unwrap();
        if apk.is_dir() {
            fs::create_dir(guest.join(name)).unwrap();
            for entry in fs::read_dir(apk).unwrap() {
                let entry = entry.unwrap();
                fs::copy(entry.path(), guest.join(name).join(entry.file_name())).unwrap();
            }
        } else {
            fs::copy(apk, guest.join(name)).unwrap();
        }
        command.arg(format!("/data/local/tmp/update-ownership/{name}"));
        write!(&mut native, "{name}").unwrap();
        for name in contents {
            native.push(' ');
            for b in name.as_bytes() {
                write!(&mut native, "{b:02x}").unwrap();
            }
        }
        native.push('\n');
    }
    assert_eq!(String::from_utf8(run(&mut command).stdout).unwrap(), native);
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
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/KeySetOwnerOracle.java"))
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/UpdateOwnershipOracle.java"),
        )
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/PreferredClearingOracle.java"),
        )
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ImplicitAccessOracle.java"),
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
        .args([
            classes.join("ManifestKeySetsOracle.class"),
            classes.join("ManifestKeySetsOracle$1.class"),
            classes.join("com/android/server/pm/KeySetOwnerOracle.class"),
            classes.join("com/android/server/pm/KeySetOwnerOracle$1.class"),
            classes.join("com/android/server/pm/UpdateOwnershipOracle.class"),
            classes.join("com/android/server/pm/PreferredClearingOracle.class"),
            classes.join("com/android/server/pm/ImplicitAccessOracle.class"),
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
    let seed = link(
        &data.0.join("apks"),
        "resource-seed",
        r#"<manifest xmlns:android="http://schemas.android.com/apk/res/android"
        package="org.example.noresources"><uses-sdk android:minSdkVersion="23" android:targetSdkVersion="35"/>
        <application android:hasCode="false"/></manifest>"#,
    );
    let manifest_bytes = aim_apps::apk::Apk::open(&seed)
        .unwrap()
        .file("AndroidManifest.xml")
        .unwrap();
    let code_only = data.0.join("apks/code-only.apk");
    fs::write(&code_only, resource_apk(&manifest_bytes, None)).unwrap();
    let code_only_apk = aim_apps::apk::Apk::open(&code_only).unwrap();
    assert!(
        code_only_apk
            .file_if_present("resources.arsc")
            .unwrap()
            .is_none()
    );
    assert!(code_only_apk.file("resources.arsc").is_err());
    assert!(parse(&code_only, "/data/app/empty/base.apk", 0, &platform).is_ok());
    writeln!(&mut expected, "CASE code-only.apk OK").unwrap();
    apks.push(code_only);
    for (name, method, size, payload, diagnostic, original_result) in [
        (
            "bad-resource-crc",
            0,
            8,
            b"invalid!".as_slice(),
            "CRC",
            "ERROR",
        ),
        (
            "bad-resource-deflate",
            8,
            8,
            b"\x07".as_slice(),
            "deflate",
            "ERROR",
        ),
        (
            "oversize-resource",
            0,
            (512 << 20) + 1,
            b"invalid!".as_slice(),
            "limit",
            "OK",
        ),
    ] {
        let apk = data.0.join("apks").join(format!("{name}.apk"));
        fs::write(
            &apk,
            resource_apk(&manifest_bytes, Some((method, size, payload))),
        )
        .unwrap();
        let error = aim_apps::apk::Apk::open(&apk)
            .err()
            .expect("present unreadable table was treated as absent");
        assert!(error.to_string().contains(diagnostic), "{name}: {error}");
        assert!(matches!(
            parse(&apk, "/data/app/bad/base.apk", 0, &platform),
            Err(Error::Parse(_))
        ));
        // The original accepts this inconsistent stored entry size; native
        // reading reaches its existing entry bound. Compatibility is #828.
        writeln!(&mut expected, "CASE {name}.apk {original_result}").unwrap();
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
    use aim_services::package::owner::update_ownership::UpdateOwnership;
    let mut ownership = UpdateOwnership::default();
    let mut expected = String::new();
    let mut snapshot = |name: &str, owner: &UpdateOwnership| {
        write!(&mut expected, "{name}").unwrap();
        for target in ["one", "two", "shared", "missing"] {
            write!(&mut expected, " {}", owner.is_denylisted(target).unwrap()).unwrap();
        }
        for provider in [Some("a"), Some("b"), None] {
            write!(&mut expected, " {}", owner.is_provider(provider).unwrap()).unwrap();
        }
        expected.push('\n');
    };
    ownership.add("a", &["one".into(), "shared".into(), "shared".into()]);
    snapshot("first", &ownership);
    ownership.add("b", &["shared".into()]);
    snapshot("overlap", &ownership);
    ownership.add("a", &["two".into()]);
    snapshot("accumulate", &ownership);
    ownership.add("a", &[]);
    snapshot("empty", &ownership);
    ownership.remove("a");
    snapshot("remove-a", &ownership);
    ownership.remove("a");
    snapshot("repeat-remove", &ownership);
    ownership.remove("b");
    snapshot("remove-last", &ownership);
    let original = String::from_utf8(run(boot.command().args([
        "shell", "/system/bin/app_process", "-Djava.class.path=/data/local/tmp/manifest-keysets/oracle.dex:/system/framework/services.jar",
        "/system/bin", "com.android.server.pm.UpdateOwnershipOracle",
    ])).stdout).unwrap();
    assert_eq!(original, expected);
    let native_data = data.0.join("preferred");
    let restrictions = native_data.join("system/users/0/package-restrictions.xml");
    fs::create_dir_all(restrictions.parent().unwrap()).unwrap();
    fs::write(native_data.join("system/packages.xml"), b"<packages/>").unwrap();
    let input = include_bytes!("fixtures/preferred-clearings.xml");
    fs::write(&restrictions, input).unwrap();
    fs::write(guest.join("preferred.xml"), input).unwrap();
    let mut store = aim_services::package::owner::Store::open(&native_data, &[0])
        .unwrap()
        .unwrap();
    let mut expected = String::new();
    for package in [Some("removed"), Some("removed"), None] {
        let changed = store
            .clear_package_preferred_activities(0, package)
            .unwrap();
        let root = aim_android_xml::read(&fs::read(&restrictions).unwrap()).unwrap();
        let mut names = root
            .children()
            .find(|e| e.name == "preferred-activities")
            .unwrap()
            .children()
            .filter(|e| e.name == "item")
            .map(|e| e.string("name").unwrap().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        writeln!(&mut expected, "{changed} {}", names.join(",")).unwrap();
    }
    let original = String::from_utf8(run(boot.command().args([
        "shell", "/system/bin/app_process", "-Djava.class.path=/data/local/tmp/manifest-keysets/oracle.dex:/system/framework/services.jar",
        "/system/bin", "com.android.server.pm.PreferredClearingOracle", "/data/local/tmp/manifest-keysets/preferred.xml",
    ])).stdout).unwrap();
    assert_eq!(original, expected);
    use aim_services::package::{
        apps_filter::{AppsFilter, Config},
        model,
    };
    let mut state = model::State::default();
    for (name, app_id) in [("a", 10001), ("b", 10002)] {
        state.packages.insert(
            name.into(),
            model::PackageState {
                name: name.into(),
                app_id,
                target_sdk_version: 35,
                pkg: Some(std::sync::Arc::new(AndroidPackage {
                    package_name: name.into(),
                    ..Default::default()
                })),
                ..Default::default()
            },
        );
    }
    let filter = AppsFilter::new(&state, &Config::default());
    let mut expected = String::new();
    for (recipient, visible, retain) in [
        (10002, 10002, false),
        (10002, 10001, false),
        (10002, 10001, false),
        (10002, 10001, true),
        (10002, 10001, true),
        (20002, 10001, true),
        (20002, 10001, false),
        (1010002, 1010001, false),
    ] {
        let changed = state
            .system
            .implicit_access
            .grant(recipient, visible, retain);
        writeln!(
            &mut expected,
            "{changed} {} {} {}",
            !filter.should_filter(&state, 10002, &state.packages["a"], 0),
            !filter.should_filter(&state, 20002, &state.packages["a"], 0),
            !filter.should_filter(&state, 1010002, &state.packages["a"], 10)
        )
        .unwrap();
    }
    let output = boot.command().args([
        "shell", "/system/bin/app_process", "-Djava.class.path=/data/local/tmp/manifest-keysets/oracle.dex:/system/framework/services.jar",
        "/system/bin", "com.android.server.pm.ImplicitAccessOracle",
    ]).output().unwrap();
    if !output.status.success() {
        let logs = boot
            .command()
            .args(["shell", "logcat", "-d", "-s", "AndroidRuntime"])
            .output()
            .unwrap();
        for line in String::from_utf8_lossy(&logs.stdout).lines() {
            eprintln!("{}", line.chars().take(700).collect::<String>());
        }
    }
    assert!(
        output.status.success(),
        "implicit oracle: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let original = String::from_utf8(output.stdout).unwrap();
    assert_eq!(original, expected);
}
