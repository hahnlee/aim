//! Real original collection and native override/cache interaction on disposable code.
use super::{
    certificate_collection::trace,
    runtime::{Boot, run},
};
use aim_services::package::{
    scan::Record,
    sign::{Overrides, SigningDetails},
    write::{Apks, CertificateCollection},
};
use std::fs;

pub fn verify(boot: &Boot, apks: &Apks, source: &Record, replacement: &Record) {
    let dir = boot.data.join("data/local/tmp/signing-overrides");
    fs::create_dir(&dir).unwrap();
    fs::copy(
        (apks.files)(source.parsed.base_apk_path.as_deref().unwrap()).unwrap(),
        dir.join("base.apk"),
    )
    .unwrap();
    let mut parsed = source.parsed.clone();
    parsed.path = Some("/data/local/tmp/signing-overrides/base.apk".into());
    parsed.base_apk_path = parsed.path.clone();
    parsed.split_code_paths = None;
    fs::write(
        dir.join("source.cache"),
        parsed.to_cache_entry().unwrap().bytes,
    )
    .unwrap();
    fs::write(
        dir.join("replacement.cache"),
        replacement.parsed.to_cache_entry().unwrap().bytes,
    )
    .unwrap();
    let mut missing = parsed.clone();
    missing.base_apk_path = Some("/data/local/tmp/signing-overrides/missing.apk".into());
    fs::write(
        dir.join("missing.cache"),
        missing.to_cache_entry().unwrap().bytes,
    )
    .unwrap();
    let time = apks.scan_file_time(&parsed).unwrap();
    let original = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/app-ids.dex:/system/framework/services.jar",
        "/system/bin",
        "com.android.server.pm.AppIdsOracle",
        "override-collection",
        "/data/local/tmp/signing-overrides/source.cache",
        "/data/local/tmp/signing-overrides/replacement.cache",
        "/data/local/tmp/signing-overrides/missing.cache",
        &time.to_string(),
    ]));
    let original = String::from_utf8(original.stdout).unwrap();
    let debug = match original.lines().next().unwrap() {
        "DEBUG true" => true,
        "DEBUG false" => false,
        other => panic!("unexpected build policy: {other}"),
    };
    let owner = Overrides::new(debug);
    let collect = |parsed, saved, force, skip| {
        apks.collect_signing_details_with_overrides(
            parsed,
            CertificateCollection {
                saved,
                database_version: 3,
                force_collect: force,
                skip_verify: skip,
                pre_n_mr1_upgrade: false,
            },
            &owner,
        )
        .unwrap()
    };
    let before = collect(&parsed, None, true, false);
    let desired =
        SigningDetails::from_saved(replacement.settings.signatures.as_ref().unwrap()).unwrap();
    assert_ne!(before.signatures, desired.signatures);
    let mut expected = format!("DEBUG {debug}\n{}\n", trace(&before));
    if debug {
        owner.add(before.clone(), desired.clone()).unwrap();
    } else {
        assert!(owner.add(before.clone(), desired.clone()).is_err());
    }
    let retained = owner.snapshot();
    expected.push_str(&format!(
        "{}\n{}\n",
        trace(&collect(&parsed, None, true, false)),
        trace(&collect(&parsed, None, true, true))
    ));
    let mut saved = source.settings.clone();
    saved.code_path = parsed.path.clone().unwrap();
    saved.last_modified_time = time;
    saved.signatures = Some(aim_services::package::settings::Signatures {
        scheme_version: before.scheme_version,
        signatures: before.signatures.clone(),
        current_flags: before.current_flags.clone(),
        past_signatures: before.past_signing_certificates.clone(),
        public_keys: None,
    });
    expected.push_str(&format!(
        "{}\n{}\n",
        trace(&collect(&parsed, Some(&saved), false, false)),
        trace(&collect(&parsed, Some(&saved), true, false))
    ));
    let error = apks
        .collect_signing_details_with_overrides(
            &missing,
            CertificateCollection {
                saved: None,
                database_version: 3,
                force_collect: true,
                skip_verify: false,
                pre_n_mr1_upgrade: false,
            },
            &owner,
        )
        .unwrap_err();
    assert!(
        error.ends_with("(-103)"),
        "unexpected missing-source failure: {error}"
    );
    expected.push_str("error -103\n");
    if debug {
        owner.remove(&before).unwrap();
    } else {
        assert!(owner.remove(&before).is_err());
    }
    expected.push_str(&format!(
        "{}\n",
        trace(&collect(&parsed, None, true, false))
    ));
    if debug {
        owner.add(before.clone(), desired.clone()).unwrap();
        owner.clear().unwrap();
    } else {
        assert!(owner.clear().is_err());
    }
    expected.push_str(&format!(
        "{}\n",
        trace(&collect(&parsed, None, true, false))
    ));
    assert_eq!(
        retained.apply(&before),
        if debug { desired } else { before }
    );
    assert_eq!(original, expected, "original signing override collection");
    // The pinned boot is a release build. Exercise the native debug owner
    // explicitly too; original debug-build application remains an external gate.
    let debug_owner = Overrides::new(true);
    let choice = CertificateCollection {
        saved: None,
        database_version: 3,
        force_collect: true,
        skip_verify: false,
        pre_n_mr1_upgrade: false,
    };
    let before = apks
        .collect_signing_details_with_overrides(&parsed, choice, &debug_owner)
        .unwrap();
    let desired =
        SigningDetails::from_saved(replacement.settings.signatures.as_ref().unwrap()).unwrap();
    debug_owner.add(before.clone(), desired.clone()).unwrap();
    assert_eq!(
        apks.collect_signing_details_with_overrides(&parsed, choice, &debug_owner)
            .unwrap(),
        desired
    );
    let mut split = parsed.clone();
    split.split_code_paths = Some(vec![replacement.parsed.base_apk_path.clone()]);
    assert!(apks.collect_signing_details(&split, choice).is_err());
    assert_eq!(
        apks.collect_signing_details_with_overrides(&split, choice, &debug_owner)
            .unwrap(),
        desired
    );
    debug_owner.remove(&before).unwrap();
    assert_eq!(
        apks.collect_signing_details_with_overrides(&parsed, choice, &debug_owner)
            .unwrap(),
        before
    );
    debug_owner.add(before.clone(), desired).unwrap();
    debug_owner.clear().unwrap();
    assert_eq!(
        apks.collect_signing_details_with_overrides(&parsed, choice, &debug_owner)
            .unwrap(),
        before
    );
}
