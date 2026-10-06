//! Incremental package-child effects against original Settings.readSettingsLPw.
use aim_android_xml::pull::{Event, Reader};
use aim_services::package::{
    owner::app_ids::AppIds,
    settings::{Package, PackageReadAttempt, Settings},
};

pub fn inputs() -> Vec<Vec<u8>> {
    let header = "<packages><package name='p' codePath='/p' userId='10001' loadingProgress='0.75' pageSizeCompat='16' domainSetId='00000000-0000-0000-0000-000000000001'>";
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
    for suffix in [
        "<package name='p' codePath='/other' userId='10001' pageSizeCompat='128' version='6' domainSetId='00000000-0000-0000-0000-000000000002'/>",
        "<package name='p' codePath='/other' realName='other' userId='10001' publicFlags='0' privateFlags='8' version='9' loadingProgress='0.5' pageSizeCompat='8' domainSetId='00000000-0000-0000-0000-000000000002'><uses-static-lib name='second' version='4'/></package>",
        "<package name='p' codePath='/other' userId='10002' domainSetId='00000000-0000-0000-0000-000000000002'><uses-static-lib name='hidden' version='4'/></package>",
        "<package name='q' codePath='/q' userId='10001' domainSetId='00000000-0000-0000-0000-000000000002'><uses-static-lib name='hidden' version='4'/></package>",
    ] {
        let bytes = format!(
            "{header}<uses-static-lib name='first' version='3'/></package>{suffix}</packages>"
        )
        .into_bytes();
        out.push(bytes.clone());
        out.push(aim_android_xml::abx::write(&aim_android_xml::read(&bytes).unwrap()).unwrap());
    }
    out
}

pub fn read(bytes: &[u8]) -> Package {
    let mut reader = Reader::new(bytes).unwrap();
    reader.next().unwrap();
    let mut settings = Settings::default();
    let mut ids = AppIds::default();
    let mut attempt = PackageReadAttempt::default();
    let result = (|| -> Result<(), String> {
        loop {
            match reader.next()? {
                Event::Start(start) if start.name == "package" => {
                    settings.read_package(
                        &mut reader,
                        &start,
                        &mut ids,
                        &mut attempt,
                        |_, _, _| Ok(false),
                    )?;
                }
                Event::Start(_) => panic!("unexpected top-level fixture owner"),
                Event::End(_) if reader.depth() == 1 => return Ok(()),
                Event::EndDocument => return Ok(()),
                _ => {}
            }
        }
    })();
    // Original failRead retries an empty reserve without clearing active settings.
    let _ = result;
    settings
        .packages
        .into_iter()
        .find(|p| p.name == "p")
        .unwrap()
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
        "{static_libs}|{sdk}|{splits}|{}|{aliases}|{upgrades}|{mime}|{}:{}:{}:{}:{}:{:08x}:{}",
        p.key_set_data.proper_signing_key_set,
        p.code_path,
        p.flags,
        p.private_flags,
        p.version_code,
        p.domain_set_id.as_deref().unwrap_or("null"),
        p.loading_progress.to_bits(),
        p.page_size_compat
    )
}
