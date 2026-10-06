//! Incremental package-child effects against original Settings.readSettingsLPw.
use aim_android_xml::{
    Element, Node,
    pull::{Event, Reader},
};
use aim_services::package::settings::{Package, Settings, SignatureReader};
use std::collections::BTreeMap;

pub fn inputs() -> Vec<Vec<u8>> {
    let header = "<packages><package name='p' codePath='/p' userId='10001' domainSetId='00000000-0000-0000-0000-000000000001'>";
    let mut out = Vec::new();
    for child in [
        "<uses-static-lib name='a' version='7'>",
        "<uses-sdk-lib name='a' version='7' optional='false'>",
        "<split-version name='a' version='7'>",
        "<proper-signing-keyset identifier='7'>",
        "<defined-keyset identifier='7' alias='a'>",
        "<upgrade-keyset identifier='7'>",
    ] {
        out.push(format!("{header}{child}<").into_bytes());
    }
    for children in [
        "<uses-static-lib name='a' version='1'><uses-static-lib name='hidden' version='2'/></uses-static-lib><uses-static-lib name='a' version='3'/><uses-sdk-lib name='b' version='4' optional='false'/><split-version name='x' version='1'/><split-version name='x' version='5'/>",
        "<proper-signing-keyset identifier='8'><defined-keyset identifier='8' alias='alias'/></proper-signing-keyset><upgrade-keyset identifier='9'/><upgrade-keyset identifier='9'/>",
        "<proper-signing-keyset identifier='2'/><defined-keyset identifier='bad'/>",
        "<defined-keyset identifier='2' alias='a'/><defined-keyset identifier='3' alias='a'/><defined-keyset identifier='4'/>",
        "<signing-keyset><upgrade-keyset identifier='3'/></signing-keyset><unknown><upgrade-keyset identifier='5'/></unknown>",
        "<mime-group name='g'><mime-type value='b'/><mime-type value='a'><mime-type value='c'/></mime-type><unknown><mime-type value='hidden'/></unknown></mime-group><mime-group name='g'><mime-type value='a'/><mime-type value='d'/></mime-group>",
        "<mime-group><mime-type value='hidden'/></mime-group><mime-group name=''><mime-type value=''/><mime-type/></mime-group>",
    ] {
        let bytes = format!("{header}{children}</package></packages>").into_bytes();
        out.push(bytes.clone());
        let binary = aim_android_xml::abx::write(&aim_android_xml::read(&bytes).unwrap()).unwrap();
        out.push(binary.clone());
        // Every truncation after the package header has become observable.
        for end in 1..binary.len() {
            let Ok(mut probe) = Reader::new(&binary[..end]) else {
                continue;
            };
            if matches!(probe.next(), Ok(Event::Start(_)))
                && matches!(probe.next(), Ok(Event::Start(_)))
            {
                out.push(binary[..end].to_vec());
            }
        }
    }
    out
}

pub fn read(bytes: &[u8]) -> Package {
    let mut reader = Reader::new(bytes).unwrap();
    reader.next().unwrap();
    let Event::Start(start) = reader.next().unwrap() else {
        panic!()
    };
    let root = Element {
        name: "packages".into(),
        attrs: vec![],
        content: vec![Node::Element(start)],
    };
    let mut package = Settings::parse(&root).unwrap().packages.remove(0);
    let mut signatures = SignatureReader::default();
    let mut refs = BTreeMap::new();
    let result =
        package.read_children(&mut reader, &mut signatures, &mut refs, |_, _, _| Ok(false));
    // Original failRead retries an empty reserve without clearing active settings.
    let _ = result;
    package
}

pub fn trace(p: &Package) -> String {
    let static_libs = p
        .uses_static_libraries
        .iter()
        .map(|(n, v)| format!("{n}:{v}"))
        .collect::<Vec<_>>()
        .join(",");
    let sdk = p
        .uses_sdk_libraries
        .iter()
        .map(|l| format!("{}:{}:{}", l.name, l.version_major, l.optional))
        .collect::<Vec<_>>()
        .join(",");
    let splits = p
        .split_versions
        .iter()
        .map(|(n, v)| format!("{n}:{v}"))
        .collect::<Vec<_>>()
        .join(",");
    let aliases = p
        .key_set_data
        .defined_key_sets
        .iter()
        .map(|(n, id)| format!("{}:{id}", n.as_deref().unwrap_or("null")))
        .collect::<Vec<_>>()
        .join(",");
    let upgrades = p
        .key_set_data
        .upgrade_key_sets
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let mime = p
        .mime_groups
        .iter()
        .map(|(name, types)| {
            format!(
                "{}:{}",
                name.as_deref().unwrap_or("null"),
                types
                    .iter()
                    .map(|value| value.as_deref().unwrap_or("null"))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        })
        .collect::<Vec<_>>()
        .join(";");
    format!(
        "{static_libs}|{sdk}|{splits}|{}|{aliases}|{upgrades}|{mime}",
        p.key_set_data.proper_signing_key_set
    )
}
