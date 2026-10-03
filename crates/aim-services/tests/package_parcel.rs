//! Original PackageImpl read/write oracle for native scan parcel output.
use aim_services::package::pkg::AndroidPackage;
use std::{
    fs,
    process::Command,
    time::{Duration, Instant},
};
mod common {
    pub mod java;
    pub mod runtime;
}
use common::java::sources;
use common::runtime::{Boot, Data, run};
#[test]
#[ignore = "requires pinned image, aimctl, JDK and d8; run explicitly"]
fn native_package_parcels_match_original_read_write() {
    let dir = std::env::temp_dir().join(format!("aim-package-parcels-{}", std::process::id()));
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
                .join("tests/fixtures/PackageRoundTripOracle.java"),
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
        .arg(classes.join("PackageRoundTripOracle.class")));
    common::java::check_linkage(
        &dex.join("classes.dex"),
        &["/system/framework/services.jar"],
    )
    .unwrap();
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

    let directory = boot.data.join("data/local/tmp/package-parcels");
    fs::create_dir(&directory).unwrap();
    fs::copy(dex.join("classes.dex"), directory.join("oracle.dex")).unwrap();
    let mut pending = vec![boot.data.join("data/system/package_cache")];
    let mut files = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    assert_eq!(
        files.len(),
        285,
        "pinned template parser cache inventory changed"
    );
    let state = aim_services::package::State::read(&boot.data.join("data"), &[0])
        .unwrap()
        .unwrap();
    let saved = state
        .settings
        .packages
        .iter()
        .find(|p| p.name == "android")
        .unwrap()
        .signatures
        .as_ref()
        .unwrap();
    let signer = aim_services::package::sign::SigningDetails::from_saved(saved).unwrap();
    let keys = aim_services::package::sign::serialize_public_keys(&signer.public_keys).unwrap();
    let mut expected = Vec::new();
    for (index, file) in files.iter().enumerate() {
        let pkg = AndroidPackage::read_cache_entry(&fs::read(file).unwrap()).unwrap();
        for enriched in [false, true] {
            let mut pkg = pkg.clone();
            if enriched {
                pkg.uid = 19001;
                pkg.primary_cpu_abi = Some("arm64-v8a".into());
                pkg.native_library_root_dir = Some("/data/app/fixture/lib".into());
                pkg.native_library_dir = Some("/data/app/fixture/lib/arm64".into());
                pkg.native_library_root_requires_isa = true;
                pkg.version_name = Some("native-owner".into());
                pkg.page_size_app_compat_flags = 8;
                pkg.signing_details = Some(aim_services::package::pkg::SigningDetails {
                    signatures: Some(signer.signatures.clone()),
                    scheme_version: signer.scheme_version,
                    public_keys: Some(keys.iter().cloned().map(Some).collect()),
                    past_signing_certificates: signer
                        .past_signing_certificates
                        .as_ref()
                        .map(|lineage| lineage.iter().map(|(cert, _)| cert.clone()).collect()),
                });
            }
            let entry = pkg.to_cache_entry().unwrap();
            let decoded = AndroidPackage::read_cache_entry(&entry.bytes).unwrap();
            assert_eq!(
                decoded == pkg,
                true,
                "native read/write changed {}",
                pkg.package_name
            );
            let name = format!("{index}-{enriched}.native");
            fs::write(directory.join(&name), &entry.bytes).unwrap();
            expected.push((name, pkg.package_name, entry));
        }
    }
    let original = run(boot.command().args([
        "shell", "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/package-parcels/oracle.dex:/system/framework/services.jar",
        "/system/bin", "PackageRoundTripOracle", "/data/local/tmp/package-parcels",
    ]));
    assert_eq!(
        String::from_utf8(original.stdout).unwrap(),
        format!("PARCELS {}\n", expected.len())
    );
    for (name, package, entry) in expected {
        let original = fs::read(directory.join(format!("{name}.original"))).unwrap();
        let mut native = AndroidPackage::read_cache_entry(&entry.bytes).unwrap();
        let mut original = AndroidPackage::read_cache_entry(&original).unwrap();
        normalize_maps(&mut native);
        normalize_maps(&mut original);
        assert!(
            native == original,
            "{name} {package}: original PackageImpl changed a decoded field"
        );
    }
}

// Map iteration order is not part of the Parcel map's contract: readHashMap
// constructs a HashMap even when the producer used an ArrayMap. Compare all
// entries and their values while retaining order for lists and arrays.
fn normalize_maps(pkg: &mut AndroidPackage) {
    use aim_services::package::pkg::{Component, Property};
    let props = |values: &mut Option<Vec<(String, Property)>>| {
        if let Some(values) = values {
            values.sort_by(|a, b| a.0.cmp(&b.0));
        }
    };
    let component = |value: &mut Component| {
        props(&mut value.properties);
    };
    props(&mut pkg.properties);
    if let Some(values) = &mut pkg.overlayables {
        values.sort_by(|a, b| a.0.cmp(&b.0));
    }
    if let Some(values) = &mut pkg.key_set_mapping {
        values.sort_by(|a, b| a.0.cmp(&b.0));
    }
    if let Some(values) = &mut pkg.processes {
        values.sort_by(|a, b| a.map_key.cmp(&b.map_key));
        for value in values {
            value
                .app_class_names_by_package
                .sort_by(|a, b| a.0.cmp(&b.0));
        }
    }
    for value in pkg.activities.iter_mut().chain(&mut pkg.receivers) {
        component(&mut value.main.component);
    }
    for value in &mut pkg.services {
        component(&mut value.main.component);
    }
    for value in &mut pkg.providers {
        component(&mut value.main.component);
    }
    for value in &mut pkg.permissions {
        component(&mut value.component);
        if let Some(group) = &mut value.parsed_permission_group {
            component(&mut group.component);
        }
    }
    for value in &mut pkg.permission_groups {
        component(&mut value.component);
    }
    for value in &mut pkg.instrumentations {
        component(&mut value.component);
    }
}
