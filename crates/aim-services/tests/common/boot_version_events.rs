//! Original boot VersionInfo initialization projections with normalized current values.
use aim_services::package::{
    owner::recovery,
    settings::{Settings, Version},
};
use std::{fs, path::Path};

pub fn cases() -> Vec<(Option<&'static [u8]>, bool)> {
    vec![(None,false),(Some(b" "),false),(Some(b"<packages/>"),false),
        (Some(b"<packages><version sdkVersion='31' databaseVersion='1' buildFingerprint='saved' fingerprint='saved-partitions'/></packages>"),false),
        (Some(b"<packages><version volumeUuid='primary_physical' sdkVersion='32' databaseVersion='1'/></packages>"),false),
        (Some(b"<packages><version sdkVersion='33' databaseVersion='bad'/></packages>"),false),
        (Some(b"<packages><version sdkVersion='33' databaseVersion='bad'/></packages>"),true),
        (Some(b"<packages><verifier/></packages>"),false)]
}

pub fn export(directory: &Path) -> Vec<String> {
    let current = Version {
        sdk_version: 36,
        database_version: 3,
        build_fingerprint: Some("fixture-build".into()),
        fingerprint: Some("fixture-partitions".into()),
        ..Default::default()
    };
    cases()
        .into_iter()
        .enumerate()
        .map(|(index, (input, reserve))| {
            if let Some(bytes) = input {
                fs::write(directory.join(format!("boot-version-input-{index}")), bytes).unwrap();
            }
            if reserve {
                fs::write(
                    directory.join(format!("boot-version-reserve-{index}")),
                    b"<packages/>",
                )
                .unwrap();
            }
            let data = directory.join(format!("boot-version-native-{index}"));
            fs::create_dir_all(data.join("system")).unwrap();
            if let Some(bytes) = input {
                fs::write(data.join("system/packages.xml"), bytes).unwrap();
            }
            if reserve {
                fs::write(data.join("system/packages.xml.reservecopy"), b"<packages/>").unwrap();
            }
            let mut state = Settings::default();
            let mut internal = current.clone();
            internal.sdk_version = 30;
            internal.database_version = 1;
            internal.build_fingerprint = Some("old".into());
            internal.fingerprint = Some("old-partitions".into());
            state.versions.push(internal);
            let result = recovery::Plan::inspect(&data).unwrap().recover_boot(
                &[],
                &mut state,
                &current,
                |bytes, state| state.read_document(bytes, |_, _, _| Ok(false)),
            );
            let mut rows = vec![match result {
                Ok((_, report)) => format!("{}", report.first_boot),
                Err(_) => "fatal".into(),
            }];
            for uuid in [None, Some("primary_physical")] {
                let v = state
                    .versions
                    .iter()
                    .find(|v| v.volume_uuid.as_deref() == uuid)
                    .unwrap();
                rows.push(
                    if v.sdk_version == current.sdk_version
                        && v.database_version == current.database_version
                        && v.build_fingerprint == current.build_fingerprint
                        && v.fingerprint == current.fingerprint
                    {
                        "current".into()
                    } else {
                        format!(
                            "{},{},{},{}",
                            v.sdk_version,
                            v.database_version,
                            v.build_fingerprint.as_deref().unwrap_or("null"),
                            v.fingerprint.as_deref().unwrap_or("null")
                        )
                    },
                );
            }
            rows.join("|")
        })
        .collect()
}
