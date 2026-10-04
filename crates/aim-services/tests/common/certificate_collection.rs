//! Original ScanPackageUtils collection choice on disposable, signed code.
use super::runtime::{Boot, run};
use aim_services::package::{
    scan::Record,
    settings::Signatures,
    sign,
    write::{Apks, CertificateCollection},
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, FileTimes},
    time::{Duration, UNIX_EPOCH},
};

pub fn verify(boot: &Boot, apks: &Apks, record: &Record) {
    let root = boot.data.join("data/local/tmp/collection");
    fs::create_dir(&root).unwrap();
    let source = (apks.files)(record.parsed.base_apk_path.as_deref().unwrap()).unwrap();
    for (name, time) in [("base.apk", 10_000), ("split.apk", 20_000)] {
        let path = root.join(name);
        fs::copy(&source, &path).unwrap();
        File::open(path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_millis(time)))
            .unwrap();
    }
    File::open(&root)
        .unwrap()
        .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_millis(40_000)))
        .unwrap();
    let mut rows = String::new();
    let mut expected = String::new();
    let mut index = 0;
    for shape in ["mono", "cluster", "split", "missing"] {
        let mut parsed = record.parsed.clone();
        parsed.path = Some(format!(
            "/data/local/tmp/collection{}",
            match shape {
                "mono" => "/base.apk",
                "missing" => "/missing",
                _ => "",
            }
        ));
        parsed.base_apk_path = Some("/data/local/tmp/collection/base.apk".into());
        parsed.split_code_paths =
            (shape == "split").then(|| vec![Some("/data/local/tmp/collection/split.apk".into())]);
        let cache = format!("/data/local/tmp/collection-{shape}.cache");
        fs::write(
            (apks.files)(&cache).unwrap(),
            parsed.to_cache_entry().unwrap().bytes,
        )
        .unwrap();
        for (kind, database, force, skip, legacy, delta) in [
            ("valid", 3, false, false, false, 0),
            ("valid", 4, false, false, false, 0),
            ("valid", 2, false, false, false, 0),
            ("valid", 1, false, false, false, 0),
            ("valid", 0, false, false, false, 0),
            ("valid", -1, false, false, false, 0),
            ("valid", 3, true, false, false, 0),
            ("valid", 3, true, true, false, 0),
            ("absent", 3, false, false, false, 0),
            ("empty", 3, false, false, false, 0),
            ("unknown", 3, false, false, false, 0),
            ("path", 3, false, false, false, 0),
            ("valid", 3, false, false, false, 1),
            ("valid", 3, false, false, true, 0),
            ("valid", 3, false, true, true, 0),
        ] {
            let modern = match shape {
                "missing" => 0,
                "split" => 20_000,
                _ => 10_000,
            };
            // A legacy cluster compares the directory, not its APK timestamp.
            let modified = if legacy && matches!(shape, "cluster" | "split") {
                40_000
            } else {
                modern
            };
            let mut saved = record.settings.clone();
            saved.code_path = if kind == "path" {
                "/data/local/tmp/other-code".into()
            } else {
                parsed.path.clone().unwrap()
            };
            saved.last_modified_time = modified + delta;
            let mut signing = Signatures {
                signatures: record.signing.signatures.clone(),
                scheme_version: record.signing.scheme_version,
                current_flags: vec![37; record.signing.signatures.len()],
                past_signatures: record
                    .signing
                    .past_signing_certificates
                    .as_ref()
                    .map(|past| past.iter().map(|(cert, _)| (cert.clone(), 0)).collect()),
                ..Default::default()
            };
            if kind == "empty" {
                signing.signatures.clear();
                signing.current_flags.clear();
                signing.past_signatures = None;
            }
            if kind == "unknown" {
                signing.scheme_version = 0;
            }
            saved.signatures = Some(signing);
            let collection = CertificateCollection {
                saved: (kind != "absent").then_some(&saved),
                database_version: database,
                force_collect: force,
                skip_verify: skip,
                pre_n_mr1_upgrade: legacy,
            };
            let result = apks.collect_signing_details(&parsed, collection).unwrap();
            expected.push_str(&format!("{index} {}\n", trace(&result)));
            rows.push_str(&format!(
                "{cache}\t{kind}\t{database}\t{force}\t{skip}\t{legacy}\t{}\n",
                saved.last_modified_time
            ));
            index += 1;
        }
    }
    fs::write(boot.data.join("data/local/tmp/collection-cases"), rows).unwrap();
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/app-ids.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.AppIdsOracle",
        "collect-certificates",
        "/data/local/tmp/collection-cases",
    ]));
    assert_eq!(
        String::from_utf8(original.stdout).unwrap(),
        expected,
        "original certificate collection"
    );
    eprintln!("original certificate collection cases: {index}");
}

fn trace(signing: &sign::SigningDetails) -> String {
    let digest = |bytes: &[u8]| {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };
    let current = signing
        .signatures
        .iter()
        .enumerate()
        .map(|(i, c)| {
            format!(
                "{}:{}",
                digest(c),
                signing.current_flags.get(i).copied().unwrap_or(0)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    let past = signing
        .past_signing_certificates
        .as_ref()
        .map(|past| {
            past.iter()
                .map(|(c, flags)| format!("{}:{flags}", digest(c)))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_else(|| "null".into());
    let keys = sign::serialize_public_keys(&signing.public_keys)
        .unwrap()
        .iter()
        .map(|key| format!("{}:{}", key.class, digest(&key.bytes)))
        .collect::<Vec<_>>()
        .join(",");
    format!("{} {current} {past} {keys}", signing.scheme_version)
}
