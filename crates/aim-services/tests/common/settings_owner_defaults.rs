//! Real-certificate/defaulted-owner cases for original Settings.readSettingsLPw.
use aim_android_xml::{Element, Node, Value};
use aim_services::package::settings::{Settings, Signatures};
use sha2::{Digest, Sha256};

pub fn inputs(cert: &[u8]) -> Vec<Vec<u8>> {
    let cert = hex(cert);
    let header = "name='p' codePath='/system/p' userId='10001' domainSetId='00000000-0000-0000-0000-000000000001'";
    let mut documents = Vec::new();
    for (count, scheme, index, past_count, flags, past_key) in [
        ("bad", "3", "0", "1", "7", ""),
        ("1", "bad", "0", "1", "7", ""),
        ("1", "3", "bad", "1", "7", ""),
        ("1", "3", "0", "bad", "7", ""),
        ("1", "3", "0", "1", "bad", ""),
        ("1", "3", "0", "1", "7", "key='bad'"),
        ("1", "3", "-2", "1", "7", ""),
    ] {
        documents.push(format!("<packages><package {header}><sigs count='{count}' schemeVersion='{scheme}'><cert index='{index}' key='{cert}'/><pastSigs count='{past_count}'><cert index='0' flags='{flags}' {past_key}/></pastSigs></sigs></package></packages>"));
    }
    for body in [
        format!(
            "<sigs count='1' schemeVersion='3'><cert index='0' key='{cert}'/></sigs><sigs count='bad'/>"
        ),
        format!(
            "<sigs count='1' schemeVersion='3'><cert index='0' key='{cert}'/><pastSigs count='1'><cert index='0' key='{cert}' flags='7'/></pastSigs><pastSigs count='1'><cert index='1'/></pastSigs></sigs>"
        ),
        format!(
            "<sigs count='1' schemeVersion='3'><cert index='0' key='{cert}'/><pastSigs count='1'><cert index='0' flags='7'/></pastSigs><pastSigs count='bad'/></sigs>"
        ),
        format!(
            "<sigs count='2' schemeVersion='3'><cert index='0' key='{cert}'/><cert index='0' key='{cert}'/><pastSigs count='1'><cert index='1' flags='7'/></pastSigs></sigs>"
        ),
    ] {
        documents.push(format!(
            "<packages><package {header}>{body}</package></packages>"
        ));
    }
    documents.push(format!("<packages><package {header}><sigs count='1' schemeVersion='3'><cert index='0' key='{cert}'/><pastSigs count='1'><cert index='0' key='{cert}' flags='7'/></pastSigs></sigs></package><package name='q' codePath='/system/q' userId='10002' domainSetId='00000000-0000-0000-0000-000000000002'><sigs count='1' schemeVersion='3'><cert index='1'/></sigs></package></packages>"));
    for body in [
        format!(
            "<sigs count='1' schemeVersion='3'><cert index='0' key='{cert}'/></sigs><sigs count='1'><cert index='1' key='AA'/></sigs>"
        ),
        format!(
            "<sigs count='1' schemeVersion='3'><cert index='0' key='{cert}'/></sigs><sigs count='1'><cert index='1' key='AA'/><pastSigs count='1'><cert index='2' key='{cert}' flags='9'/></pastSigs></sigs>"
        ),
        format!(
            "<sigs count='2' schemeVersion='3'><cert index='0' key='{cert}'/><cert index='1' key='AA'/></sigs>"
        ),
        format!(
            "<sigs count='1' schemeVersion='3'><cert index='0' key='{cert}'/><pastSigs count='1'><cert index='1' key='AA' flags='7'/></pastSigs></sigs>"
        ),
    ] {
        documents.push(format!("<packages><package {header}>{body}</package><package name='q' codePath='/system/q' userId='10002' domainSetId='00000000-0000-0000-0000-000000000002'><sigs count='1' schemeVersion='3'><cert index='2'/></sigs></package></packages>"));
        documents.push(format!(
            "<packages><shared-user name='g' userId='10003'>{body}</shared-user></packages>"
        ));
        documents.push(format!(
            "<packages><package {header} installInitiator='installer'>{}</package></packages>",
            body.replace("sigs", "install-initiator-sigs")
        ));
    }
    for tag in ["permissions", "permission-trees"] {
        for attrs in ["protection='bad' icon='9'", "protection='2' icon='bad'"] {
            documents.push(format!("<packages><{tag}><item name='perm' package='p' type='dynamic' label='label' {attrs}/></{tag}></packages>"));
        }
    }
    let key = Value::BytesBase64(include_bytes!("../fixtures/settings-public-key-1.der").to_vec())
        .string()
        .unwrap()
        .into_owned();
    for version in ["", "version='bad'", "version='1'"] {
        documents.push(format!("<packages><keyset-settings {version}><keys><public-key identifier='1' value='{key}'/></keys><lastIssuedKeyId value='9'/><lastIssuedKeySetId value='11'/></keyset-settings></packages>"));
    }
    for attrs in [
        "identifier='1' value='A'",
        "identifier='bad' value='A'",
        "value='A'",
    ] {
        documents.push(format!("<packages><keyset-settings version='1'><keys><public-key {attrs}/></keys></keyset-settings></packages>"));
    }
    documents.push("<packages><keyset-settings version='1'><lastIssuedKeyId value='bad'/></keyset-settings></packages>".into());
    let mut inputs = Vec::new();
    for xml in documents {
        let mut root = aim_android_xml::read(xml.as_bytes()).unwrap();
        inputs.push(xml.into_bytes());
        inputs.push(aim_android_xml::abx::write(&root).unwrap());
        if wrong(&mut root) {
            inputs.push(aim_android_xml::abx::write(&root).unwrap());
        }
    }
    inputs
}

fn wrong(element: &mut Element) -> bool {
    for (_, value) in &mut element.attrs {
        if *value == Value::String("bad".into()) {
            *value = Value::Bool(true);
            return true;
        }
    }
    element.content.iter_mut().any(|node| match node {
        Node::Element(element) => wrong(element),
        _ => false,
    })
}

pub fn permission_inputs() -> Vec<Vec<u8>> {
    let mut inputs = Vec::new();
    for body in [
        "<permissions><item name='a' package='p'/></permissions><permissions><item name='b' package='q'/></permissions>",
        "<permissions><item name='a' package='p' type='dynamic' icon='9' label='old'/><item name='a' package='q' protection='2'/></permissions>",
        "<permission-trees><item name='a' package='p'/></permission-trees><permission-trees><item name='b' package='q'/></permission-trees>",
        "<permissions><unknown><item name='hidden' package='p'/></unknown><item name='a' package='p'><item name='hidden' package='p'/></item><item name='a'/></permissions>",
        "<permissions><item name='a' package='p' protection='bad' type='dynamic' icon='bad'/></permissions><permission-trees><item name='t' package='p'/></permission-trees>",
    ] {
        let xml = format!("<packages>{body}</packages>");
        inputs.push(xml.as_bytes().to_vec());
        inputs.push(
            aim_android_xml::abx::write(&aim_android_xml::read(xml.as_bytes()).unwrap()).unwrap(),
        );
    }
    for tail in [
        "<",
        "<item name='b' package='q'><",
        "<item name='b' package='q' broken=",
        "</permissions><version volumeUuid='v' sdkVersion='bad'/>",
    ] {
        inputs.push(format!("<packages><permissions><item name='a' package='p' type='dynamic' icon='9' label='kept'/>{tail}").into_bytes());
    }
    let xml = b"<packages><permissions><item name='a' package='p' type='dynamic' icon='9' label='kept'/><item name='b' package='q'/></permissions></packages>";
    let abx = aim_android_xml::abx::write(&aim_android_xml::read(xml).unwrap()).unwrap();
    for end in 0..=abx.len() {
        inputs.push(abx[..end].to_vec());
    }
    inputs
}

pub fn configured_permission_inputs() -> Vec<Vec<u8>> {
    let mut inputs = permission_inputs();
    for body in [
        "<permissions><item name='a' package='changed' protection='2'/></permissions>",
        "<permissions><item name='a' package='changed' type='dynamic' protection='bad' icon='7' label='new'/></permissions>",
        "<permissions><item name='a' package='changed' type='dynamic' icon='7' label='new'/><item name='a' package='changed-again' protection='2'/></permissions>",
        "<permission-trees><item name='a' package='changed' type='dynamic' icon='bad'/></permission-trees>",
    ] {
        let xml = format!("<packages>{body}</packages>");
        inputs.push(xml.as_bytes().to_vec());
        inputs.push(
            aim_android_xml::abx::write(&aim_android_xml::read(xml.as_bytes()).unwrap()).unwrap(),
        );
    }
    inputs
}

pub fn seed_permissions(settings: &mut Settings) {
    use aim_services::package::settings::{Permission, PermissionOwner};
    for out in [&mut settings.permissions, &mut settings.permission_trees] {
        out.push(Permission {
            name: "a".into(),
            package: "configured".into(),
            owner: PermissionOwner::Config {
                uid: 1234,
                gids: vec![1001, 1002],
            },
            protection_level: 2,
            dynamic: Some((42, Some("configured-label".into()))),
        });
    }
}

pub fn assert_configured_owners(settings: &Settings) {
    use aim_services::package::settings::PermissionOwner;
    for entries in [&settings.permissions, &settings.permission_trees] {
        let p = entries.iter().find(|p| p.name == "a").unwrap();
        assert_eq!(p.package, "configured");
        assert_eq!(
            p.owner,
            PermissionOwner::Config {
                uid: 1234,
                gids: vec![1001, 1002]
            }
        );
    }
}

pub fn trace(settings: &Settings, first: bool, main: bool, reserve: bool) -> String {
    let mut packages: Vec<_> = settings
        .packages
        .iter()
        .map(|p| {
            let flags = p
                .signatures
                .as_ref()
                .map(|s| {
                    (0..s.signatures.len())
                        .map(|i| s.current_flags.get(i).copied().unwrap_or(0).to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                })
                .unwrap_or_else(|| "null".into());
            let mut trace = format!(
                "{}:{}:{flags}:{}",
                p.name,
                signature(p.signatures.as_ref()),
                public_keys(p.signatures.as_ref())
            );
            if p.install_source.initiating_package.is_some() {
                let initiator = p.install_source.initiating_package_signatures.as_ref();
                trace.push_str(&format!(
                    ":initiator={}:{}",
                    signature(initiator),
                    public_keys(initiator)
                ));
            }
            trace
        })
        .collect();
    for group in &settings.shared_users {
        let signing = group.signatures.as_ref();
        let flags = signing
            .map(|s| {
                (0..s.signatures.len())
                    .map(|i| s.current_flags.get(i).copied().unwrap_or(0).to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_else(|| "null".into());
        packages.push(format!(
            "shared:{}:{}:{flags}:{}",
            group.name,
            signature(signing),
            public_keys(signing)
        ));
    }
    packages.sort();
    let permissions = |entries: &[aim_services::package::settings::Permission]| {
        let mut entries: Vec<_> = entries
            .iter()
            .map(|p| {
                use aim_services::package::settings::PermissionOwner;
                let kind = match p.owner {
                    PermissionOwner::Manifest => 0,
                    PermissionOwner::Config { .. } => 1,
                    PermissionOwner::Dynamic => 2,
                };
                let (icon, label) = p
                    .dynamic
                    .as_ref()
                    .map(|(icon, label)| (*icon, label.as_deref().unwrap_or("null")))
                    .unwrap_or((0, "null"));
                format!(
                    "{}:{}:{}:{icon}:{label}:{kind}",
                    p.name, p.package, p.protection_level
                )
            })
            .collect();
        entries.sort();
        entries.join(";")
    };
    format!(
        "{first}|{}|{}|{}|{main},{reserve}",
        packages.join(";"),
        permissions(&settings.permissions),
        permissions(&settings.permission_trees)
    )
}

pub fn signature(s: Option<&Signatures>) -> String {
    let Some(s) = s else {
        return "0:null:null".into();
    };
    let current = s
        .signatures
        .iter()
        .map(|c| hex(&Sha256::digest(c)))
        .collect::<Vec<_>>()
        .join(",");
    let past = s
        .past_signatures
        .as_ref()
        .map(|certs| {
            certs
                .iter()
                .map(|(c, flags)| format!("{}:{flags}", hex(&Sha256::digest(c))))
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_else(|| "null".into());
    format!("{}:{current}:{past}", s.scheme_version)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn public_keys(signing: Option<&Signatures>) -> String {
    signing
        .and_then(|s| s.public_keys.as_ref())
        .map(|keys| {
            keys.iter()
                .map(|key| {
                    let key = key.as_ref().unwrap();
                    format!("{}:{}", key.class, hex(&Sha256::digest(&key.bytes)))
                })
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_else(|| "null".into())
}
