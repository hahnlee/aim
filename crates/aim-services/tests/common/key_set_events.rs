//! Keyset record/retry projections; public-key factory coverage is separate.
use aim_services::package::{
    owner::{app_ids::AppIds, key_sets},
    settings::{PackageReadAttempt, ReadError, Settings},
};

pub fn inputs() -> Vec<Vec<u8>> {
    let prefix = "<packages><package name='p' codePath='/p' userId='10001' domainSetId='00000000-0000-0000-0000-000000000001'><proper-signing-keyset identifier='2'/></package>";
    let mut out = Vec::new();
    for body in [
        "<keyset-settings version='1'><keysets><keyset identifier='2'/></keysets><lastIssuedKeyId value='9'/><lastIssuedKeySetId value='7'/></keyset-settings>",
        "<keyset-settings version='anything'><unknown><keysets><keyset identifier='3'/><keyset identifier='2'/><keyset identifier='2'/></keysets></unknown></keyset-settings>",
        "<keyset-settings version='1'><lastIssuedKeyId value='9'/><keysets><keyset identifier='2'/></keysets><lastIssuedKeySetId value='bad'/></keyset-settings>",
        "<keyset-settings version='1'><lastIssuedKeyId value='9'/><keysets><key-id identifier='1'/></keysets></keyset-settings>",
        "<keyset-settings><keysets><keyset identifier='2'/></keysets></keyset-settings>",
        "<keyset-settings><",
        "<keyset-settings version='1'><keysets><keyset identifier='2'/></keysets></keyset-settings><keyset-settings/>",
    ] {
        let bytes = format!("{prefix}{body}</packages>").into_bytes();
        out.push(bytes.clone());
        if let Ok(root) = aim_android_xml::read(&bytes) {
            out.push(aim_android_xml::abx::write(&root).unwrap());
        }
    }
    let binary = aim_android_xml::abx::write(&aim_android_xml::read(format!("{prefix}<keyset-settings version='1'><lastIssuedKeyId value='9'/><keysets><keyset identifier='2'/></keysets><lastIssuedKeySetId value='7'/></keyset-settings></packages>").as_bytes()).unwrap()).unwrap();
    for end in 1..binary.len() {
        if let Ok(mut reader) = aim_android_xml::pull::Reader::new(&binary[..end]) {
            if reader.next().is_ok() && reader.next().is_ok() {
                // Restrict truncation to after the complete package container.
                loop {
                    match reader.next() {
                        Ok(aim_android_xml::pull::Event::End(name)) if name == "package" => {
                            out.push(binary[..end].to_vec());
                            break;
                        }
                        Ok(aim_android_xml::pull::Event::EndDocument) | Err(_) => break,
                        _ => {}
                    }
                }
            }
        }
    }
    out
}

pub fn read(bytes: &[u8]) -> (Settings, &'static str) {
    let mut state = Settings::default();
    let mut ids = AppIds::default();
    let mut attempt = PackageReadAttempt::default();
    let result = state.read_document(bytes, |state, reader, start| {
        match start.name.as_str() {
            "package" => {
                state.read_package(reader, start, &mut ids, &mut attempt, |_, _, _| Ok(false))?;
            }
            "keyset-settings" => state.read_key_sets(
                reader,
                start,
                &attempt.key_set_refs,
                |_| panic!("public key outside this projection"),
                |state, refs| {
                    let counts = state
                        .key_sets
                        .reference_counts
                        .get_or_insert_with(Default::default);
                    for (id, count) in refs {
                        if let Some(value) = counts.get_mut(id) {
                            *value = *count;
                        }
                    }
                    key_sets::restore(state).map_err(ReadError::Owner)
                },
            )?,
            _ => return Ok(false),
        }
        Ok(true)
    });
    let status = if matches!(result, Err(ReadError::FatalInput(_))) {
        "fatal"
    } else {
        "ok"
    };
    assert!(
        !matches!(result, Err(ReadError::Owner(_))),
        "unexpected native owner error: {result:?}"
    );
    // File-error retry reads an empty reserve, retaining registered pool effects.
    (state, status)
}

pub fn trace(state: &Settings, status: &str) -> String {
    let proper = state
        .packages
        .iter()
        .find(|p| p.name == "p")
        .map(|p| p.key_set_data.proper_signing_key_set)
        .unwrap_or(-1);
    let refs = if state.key_sets.key_sets.iter().any(|(id, _)| *id == proper) {
        state
            .key_sets
            .reference_counts
            .as_ref()
            .and_then(|counts| counts.get(&proper))
            .copied()
            .unwrap_or(0)
            .to_string()
    } else {
        "absent".into()
    };
    let mut sets = state
        .key_sets
        .key_sets
        .iter()
        .map(|(id, keys)| {
            format!(
                "{id}:{}",
                keys.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect::<Vec<_>>();
    sets.sort();
    format!(
        "{status}|{proper}|{refs}|{},{}|{}",
        state.key_sets.last_issued_key_id,
        state.key_sets.last_issued_key_set_id,
        sets.join(";")
    )
}
