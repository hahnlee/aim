//! Minimal fixtures for android-16.0.0_r1 ApkLiteParseUtils and SplitDependencyLoader.
use super::*;
use aim_apps::res::{Attr, Value};
#[path = "../../../tests/common/zip.rs"]
mod zip;

fn status<T>(result: Result<T>, code: i32, text: &str) {
    match result {
        Err(Error::Status(actual, message)) => assert_eq!((actual, message.as_str()), (code, text)),
        _ => panic!("expected {code}: {text}"),
    }
}
struct Directory(std::path::PathBuf);
impl Drop for Directory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn attr(name: &str, value: &str) -> Attr {
    Attr {
        ns: ANDROID.into(),
        name: name.into(),
        id: 0,
        value: Value::String(value.into()),
        kind: if value == "true" {
            TYPE_INT_BOOLEAN
        } else {
            TYPE_STRING
        },
        data: 1,
    }
}
fn part(split: Option<&str>, uses: Option<&str>) -> cluster::Part {
    let mut manifest = Element {
        name: "manifest".into(),
        ..Default::default()
    };
    manifest.attrs = ["isolatedSplits", "isFeatureSplit"]
        .map(|n| attr(n, "true"))
        .to_vec();
    if let Some(name) = uses {
        manifest.children.push(Element {
            name: "uses-split".into(),
            attrs: vec![attr("name", name)],
            ..Default::default()
        });
    }
    cluster::Part {
        path: "fixture.apk".into(),
        manifest,
        table: None,
        split: split.map(str::to_owned),
        revision: 0,
    }
}
// Binary XML fixture: <manifest package="org.example.archive" split="" android:versionCode="1"/>.
// The pool also holds "feature" (index 7); each attribute occupies 20 bytes.
fn manifest(package: &str, split: &str, version: u32) -> Vec<u8> {
    let hex = concat!(
        "030008003801000001001c00b80000000800000000000000000100003c00000000000000000000000b00000015000000",
        "1d0000002b000000580000006e0000007100000008086d616e69666573740007077061636b61676500050573706c6974",
        "000b0b76657273696f6e436f6465002a2a687474703a2f2f736368656d61732e616e64726f69642e636f6d2f61706b2f",
        "7265732f616e64726f69640013136f72672e6578616d706c652e61726368697665000000000707666561747572650000",
        "020110006000000001000000ffffffffffffffff00000000140014000300000000000000ffffffff0100000005000000",
        "0800000305000000ffffffff020000000600000008000003060000000400000003000000ffffffff0800001001000000",
        "030110001800000001000000ffffffffffffffff00000000",
    );
    let mut xml = hex_bytes(hex);
    xml[158..177].copy_from_slice(package.as_bytes());
    for at in [256, 264] {
        xml[at..at + 4].copy_from_slice(&(if split.is_empty() { 6u32 } else { 7 }).to_le_bytes());
    }
    xml[284..288].copy_from_slice(&version.to_le_bytes());
    xml
}
#[test]
fn archive_resource_and_manifest_boundaries() {
    let dir = Directory(aim_paths::scratch_dir("parse-archive"));
    let path = dir.0.join("base.apk");
    let environment = lite::Environment {
        sdk: 36,
        codenames: vec![],
        properties: Default::default(),
    };
    let xml = manifest("org.example.archive", "", 1);
    for (bytes, code) in [
        (b"not a zip".to_vec(), -100),
        (zip::resource_apk(b"unreadable XML", None), -102),
        (zip::resource_apk(&xml, Some((0, 8, b"invalid!"))), -100),
    ] {
        std::fs::write(&path, bytes).unwrap();
        let message = format!("Failed to parse {}", path.display());
        status(lite::package_name(&path, &environment), code, &message);
        status(lite::split_name(&path, &environment), code, &message);
        let failure = lite::install_lite(&path, &environment).err().unwrap();
        assert_eq!(
            (failure.legacy_status, failure.message.as_str()),
            (code, message.as_str())
        );
        status(
            cluster::load(&path, "/code/base.apk"),
            code,
            "Failed to parse /code/base.apk",
        );
    }
    std::fs::write(&path, zip::resource_apk(&xml, None)).unwrap();
    assert!(
        cluster::load(&path, "/code/base.apk").unwrap()[0]
            .table
            .is_none()
    );
    assert_eq!(
        lite::package_name(&path, &environment).unwrap(),
        "org.example.archive"
    );
    assert!(lite::install_lite(&path, &environment).is_ok());
}
#[test]
fn cluster_identity_and_base_selection() {
    let dir = Directory(aim_paths::scratch_dir("parse-archive"));
    status(
        cluster::load(&dir.0, "/code"),
        -100,
        "No packages found in split",
    );
    std::fs::write(dir.0.join("other"), b"ignored").unwrap();
    status(
        cluster::load(&dir.0, "/code"),
        -101,
        "Missing base APK in /code",
    );
    let first = dir.0.join("first.apk");
    let base = zip::resource_apk(&manifest("org.example.archive", "", 1), None);
    let second = dir.0.join("second.apk");
    for (package, version) in [("org.example.changed", 1), ("org.example.archive", 2)] {
        std::fs::write(&first, &base).unwrap();
        std::fs::write(
            &second,
            zip::resource_apk(&manifest(package, "feature", version), None),
        )
        .unwrap();
        let expected = if version == 1 {
            [
                "Inconsistent package org.example.changed in /code/second.apk; expected org.example.archive",
                "Inconsistent package org.example.archive in /code/first.apk; expected org.example.changed",
            ]
        } else {
            [
                "Inconsistent version 2 in /code/second.apk; expected 1",
                "Inconsistent version 1 in /code/first.apk; expected 2",
            ]
        };
        let Err(Error::Status(-101, message)) = cluster::load(&dir.0, "/code") else {
            panic!("cluster accepted")
        };
        assert!(expected.contains(&message.as_str()), "{message}");
    }
    assert!(cluster::load(&second, "/code/feature.apk").is_ok());
    std::fs::remove_file(first).unwrap();
    status(
        cluster::load(&dir.0, "/code"),
        -101,
        "Missing base APK in /code",
    );
}
#[test]
fn illegal_split_dependencies() {
    for (name, message) in [
        (
            "absent",
            "Split 'feature' requires split 'absent', which is missing.",
        ),
        ("feature", "Cycle detected in split dependencies."),
    ] {
        status(
            cluster::dependencies(&[part(None, None), part(Some("feature"), Some(name))]),
            -101,
            message,
        );
    }
}
