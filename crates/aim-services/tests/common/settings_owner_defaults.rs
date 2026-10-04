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

pub fn trace(settings: &Settings, first: bool, main: bool, reserve: bool) -> String {
    let mut packages: Vec<_> = settings
        .packages
        .iter()
        .map(|p| format!("{}:{}", p.name, signature(p.signatures.as_ref())))
        .collect();
    packages.sort();
    let permissions = |entries: &[aim_services::package::settings::Permission]| {
        let mut entries: Vec<_> = entries
            .iter()
            .map(|p| {
                let (icon, label) = p
                    .dynamic
                    .as_ref()
                    .map(|(icon, label)| (*icon, label.as_deref().unwrap_or("null")))
                    .unwrap_or((0, "null"));
                format!(
                    "{}:{}:{}:{icon}:{label}",
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

fn signature(s: Option<&Signatures>) -> String {
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
