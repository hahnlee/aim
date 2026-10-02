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
