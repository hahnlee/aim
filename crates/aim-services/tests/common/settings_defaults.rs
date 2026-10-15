//! Inputs/projections for actual original Settings default-value reads.
use aim_android_xml::{Element, Node, Value};
use aim_services::package::settings::{Package, Settings};

const PACKAGE: &str = "name='p' codePath='/system/p' userId='10001' publicFlags='0' privateFlags='0' installer='installer' installInitiator='initiator' domainSetId='00000000-0000-0000-0000-000000000001'";

pub fn inputs() -> Vec<Vec<u8>> {
    let common = [
        "version",
        "targetSdkVersion",
        "restrictUpdateHash",
        "scannedAsStoppedSystemApp",
        "ft",
        "ts",
        "ut",
        "userId",
        "sharedUserId",
        "appMetadataSource",
    ];
    let active = [
        "isSdkLibrary",
        "publicFlags",
        "privateFlags",
        "it",
        "installerUid",
        "packageSource",
        "isOrphaned",
        "installInitiatorUninstalled",
        "categoryHint",
        "updateAvailable",
        "forceQueryable",
        "pendingRestore",
        "debuggable",
        "baseRevisionCode",
        "pageSizeCompat",
        "loadingProgress",
        "loadingCompletedTime",
    ];
    let mut out = Vec::new();
    for (tag, attrs, fields) in [
        (
            "package",
            PACKAGE,
            common
                .iter()
                .chain(active.iter())
                .copied()
                .collect::<Vec<_>>(),
        ),
        (
            "updated-package",
            "name='p' codePath='/system/priv-app/p' userId='10001'",
            common.to_vec(),
        ),
        (
            "shared-user",
            "name='g' userId='10042'",
            vec!["userId", "system"],
        ),
    ] {
        for field in fields {
            let mut root =
                aim_android_xml::read(format!("<packages><{tag} {attrs}/></packages>").as_bytes())
                    .unwrap();
            let element = first(&mut root);
            element.attrs.retain(|(name, _)| name != field);
            element.attrs.push((
                field.into(),
                Value::String(
                    if field == "restrictUpdateHash" {
                        "A"
                    } else {
                        "bad"
                    }
                    .into(),
                ),
            ));
            let attrs = element
                .attrs
                .iter()
                .map(|(name, value)| format!("{name}='{}'", value.string().unwrap()))
                .collect::<Vec<_>>()
                .join(" ");
            out.push(format!("<packages><{tag} {attrs}/></packages>").into_bytes());
            out.push(aim_android_xml::abx::write(&root).unwrap());
            let element = first(&mut root);
            element.attrs.last_mut().unwrap().1 = wrong_type(field);
            out.push(aim_android_xml::abx::write(&root).unwrap());
        }
    }
    for (tag, field, attrs) in [
        ("uses-static-lib", "version", "name='l' version='bad'"),
        ("uses-sdk-lib", "version", "name='l' version='bad'"),
        (
            "uses-sdk-lib",
            "optional",
            "name='l' version='1' optional='bad'",
        ),
        ("split-version", "version", "name='split' version='bad'"),
    ] {
        let xml = format!("<packages><package {PACKAGE}><{tag} {attrs}/></package></packages>");
        let mut root = aim_android_xml::read(xml.as_bytes()).unwrap();
        out.push(xml.into_bytes());
        out.push(aim_android_xml::abx::write(&root).unwrap());
        let child = first(first(&mut root));
        child
            .attrs
            .iter_mut()
            .find(|(name, _)| name == field)
            .unwrap()
            .1 = wrong_type(field);
        out.push(aim_android_xml::abx::write(&root).unwrap());
    }
    let xml = "<packages><package name='p' codePath='/system/p' userId='10001' publicFlags='1' privateFlags='8' installer='installer' installInitiator='initiator' domainSetId='00000000-0000-0000-0000-000000000001' version='5' targetSdkVersion='35' restrictUpdateHash='AQID' scannedAsStoppedSystemApp='true' ft='2' ut='3' installerUid='42' packageSource='1' isOrphaned='true' installInitiatorUninstalled='true' categoryHint='2' updateAvailable='true' forceQueryable='true' pendingRestore='true' debuggable='true' baseRevisionCode='2' pageSizeCompat='8' loadingProgress='0.5' loadingCompletedTime='10'><uses-static-lib name='static' version='4'/><uses-sdk-lib name='sdk' version='2' optional='false'/><split-version name='split' version='3'/></package></packages>";
    out.push(xml.as_bytes().to_vec());
    out.push(aim_android_xml::abx::write(&aim_android_xml::read(xml.as_bytes()).unwrap()).unwrap());
    let mut flags =
        aim_android_xml::read(format!("<packages><package {PACKAGE}/></packages>").as_bytes())
            .unwrap();
    first(&mut flags)
        .attrs
        .iter_mut()
        .find(|(name, _)| name == "publicFlags")
        .unwrap()
        .1 = Value::IntHex(0x10);
    out.push(aim_android_xml::abx::write(&flags).unwrap());
    for attrs in [
        "",
        "system='true'",
        "system='TrUe'",
        "system='false'",
        "system='bad'",
        "system='1'",
        "flags='0' system='true'",
        "flags='bad' system='true'",
        "flags='134217729'",
        "flags='268435457'",
        "flags='1073741825'",
        "flags='1476395009' privateFlags='16'",
        "flags='-1'",
        "publicFlags='2' privateFlags='16' flags='1476395009' system='true'",
    ] {
        let xml = format!(
            "<packages><package name='p' codePath='/system/p' userId='10001' domainSetId='00000000-0000-0000-0000-000000000001' {attrs}/></packages>"
        );
        let mut root = aim_android_xml::read(xml.as_bytes()).unwrap();
        out.push(xml.into_bytes());
        out.push(aim_android_xml::abx::write(&root).unwrap());
        for (name, value) in &mut first(&mut root).attrs {
            if name == "flags" {
                *value = Value::IntHex(0x58000001);
            } else if name == "system" {
                *value = Value::Bool(true);
            }
        }
        out.push(aim_android_xml::abx::write(&root).unwrap());
    }
    out
}

fn first(element: &mut Element) -> &mut Element {
    element
        .content
        .iter_mut()
        .find_map(|node| match node {
            Node::Element(element) => Some(element),
            _ => None,
        })
        .unwrap()
}

fn wrong_type(field: &str) -> Value {
    if matches!(
        field,
        "system"
            | "optional"
            | "isSdkLibrary"
            | "isOrphaned"
            | "installInitiatorUninstalled"
            | "scannedAsStoppedSystemApp"
            | "updateAvailable"
            | "forceQueryable"
            | "pendingRestore"
            | "debuggable"
    ) {
        Value::Int(23)
    } else {
        Value::Bool(true)
    }
}

pub fn trace(settings: &Settings) -> String {
    let mut active: Vec<_> = settings
        .packages
        .iter()
        .map(|package| setting(package, true))
        .collect();
    let mut disabled: Vec<_> = settings
        .disabled_system_packages
        .iter()
        .map(|package| setting(package, false))
        .collect();
    let mut shared: Vec<_> = settings
        .shared_users
        .iter()
        .map(|group| format!("{}:{}:{}", group.name, group.app_id, group.flags))
        .collect();
    active.sort();
    disabled.sort();
    shared.sort();
    format!(
        "false|{}|{}|{}|true,true",
        active.join(";"),
        disabled.join(";"),
        shared.join(";")
    )
}

fn setting(p: &Package, active: bool) -> String {
    let hash = p
        .restrict_update_hash
        .as_ref()
        .map(|bytes| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
        .unwrap_or_else(|| "null".into());
    let mut fields = vec![
        p.name.clone(),
        p.app_id.to_string(),
        p.version_code.to_string(),
        p.target_sdk_version.to_string(),
        p.flags.to_string(),
        p.private_flags.to_string(),
        hash,
        p.scanned_as_stopped_system_app.to_string(),
        p.last_modified_time.to_string(),
        p.last_update_time.to_string(),
        p.app_metadata_source.to_string(),
    ];
    if active {
        fields.extend([
            p.install_source.installer_uid.to_string(),
            p.install_source.package_source.to_string(),
            p.install_source.is_orphaned.to_string(),
            p.install_source.initiating_package_uninstalled.to_string(),
            p.category_hint.to_string(),
            p.update_available.to_string(),
            p.force_queryable.to_string(),
            p.pending_restore.to_string(),
            p.debuggable.to_string(),
            p.base_revision_code.to_string(),
            p.page_size_compat.to_string(),
            format!("{:08x}", p.loading_progress.to_bits()),
            p.loading_completed_time.to_string(),
        ]);
    }
    fields.push(
        p.uses_static_libraries
            .iter()
            .map(|(name, version)| format!("{name}:{version}"))
            .collect::<Vec<_>>()
            .join("/"),
    );
    fields.push(
        p.uses_sdk_libraries
            .iter()
            .map(|lib| format!("{}:{}:{}", lib.name, lib.version_major, lib.optional))
            .collect::<Vec<_>>()
            .join("/"),
    );
    if active {
        fields.push(
            p.split_versions
                .iter()
                .map(|(name, revision)| format!("{name}:{revision}"))
                .collect::<Vec<_>>()
                .join("/"),
        );
    }
    fields.join(",")
}
