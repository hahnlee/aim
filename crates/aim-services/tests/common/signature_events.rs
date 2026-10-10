//! Original PackageSignatures event publication and shared-table effects.
use aim_services::package::settings::{SignatureReader, Signatures};
use sha2::{Digest, Sha256};

pub fn seed(cert: &[u8]) -> Vec<u8> {
    format!(
        "<sigs count='1' schemeVersion='2'><cert index='0' key='{}'/></sigs>",
        hex(cert)
    )
    .into_bytes()
}

pub const RETRY: &[u8] = b"<sigs count='1' schemeVersion='3'><cert index='1'/><pastSigs count='1'><cert index='0' flags='7'/></pastSigs></sigs>";

pub fn inputs(cert: &[u8]) -> Vec<Vec<u8>> {
    let key = hex(cert);
    let mut out = Vec::new();
    for xml in [
        "<sigs/>".into(),
        "<sigs count='bad'><cert index='1' key='AA'/></sigs>".into(),
        "<sigs count='1' schemeVersion='3'><cert index='1' key='AA'/></sigs>".into(),
        format!("<sigs count='1' schemeVersion='3'><cert index='1' key='{key}'/></sigs>"),
        format!(
            "<sigs count='1' schemeVersion='3'><cert index='0'/><pastSigs count='1'><cert index='1' key='{key}' flags='5'/></pastSigs><pastSigs count='1'><cert index='1'/></pastSigs></sigs>"
        ),
        format!(
            "<sigs count='1'><unknown><cert index='1' key='{key}'/></unknown><cert index='0'/><pastSigs count='1'><pastSigs><cert index='1' key='{key}'/></pastSigs><cert index='0' flags='8'/></pastSigs></sigs>"
        ),
        "<sigs count='-2' schemeVersion='3'><cert index='0'/></sigs>".into(),
    ] {
        out.push(xml.as_bytes().to_vec());
        out.push(
            aim_android_xml::abx::write(&aim_android_xml::read(xml.as_bytes()).unwrap()).unwrap(),
        );
    }
    for tail in ["<", "<cert index='2' key=", "<cert index='2' key='AA'><"] {
        out.push(
            format!("<sigs count='2' schemeVersion='3'><cert index='1' key='{key}'/>{tail}")
                .into_bytes(),
        );
    }
    let xml = format!(
        "<sigs count='1' schemeVersion='3'><cert index='0'/><pastSigs count='1'><cert index='1' key='{key}' flags='5'/></pastSigs></sigs>"
    );
    let abx = aim_android_xml::abx::write(&aim_android_xml::read(xml.as_bytes()).unwrap()).unwrap();
    for end in 0..=abx.len() {
        out.push(abx[..end].to_vec());
    }
    out
}

pub fn read(
    bytes: &[u8],
    owner: &mut SignatureReader,
    target: &mut Option<Signatures>,
    flags: &mut Vec<i32>,
) -> String {
    use aim_android_xml::pull::{Event, Reader};
    let result = (|| {
        let mut reader = Reader::new(bytes)?;
        let start = loop {
            match reader.next()? {
                Event::Start(start) => break start,
                Event::EndDocument => return Ok("absent"),
                _ => {}
            }
        };
        if let Some(current) = owner.read(&mut reader, &start, target)? {
            *flags = current;
        }
        Ok::<_, String>("ok")
    })();
    let status = result.unwrap_or("error");
    if let Some(signing) = target {
        let stored: Vec<_> = (0..signing.signatures.len())
            .map(|i| signing.current_flags.get(i).copied().unwrap_or(0))
            .collect();
        assert_eq!(&stored, flags);
    }
    let table = owner
        .certificates()
        .iter()
        .map(|entry| match entry {
            Some((cert, flags)) => format!("{}:{flags}", hex(&Sha256::digest(cert))),
            None => "null".into(),
        })
        .collect::<Vec<_>>()
        .join(",");
    let current = if target.is_some() {
        flags
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(",")
    } else {
        "null".into()
    };
    let keys = target
        .as_ref()
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
        .unwrap_or_else(|| "null".into());
    format!(
        "{status}|{}|{current}|{table}|{keys}",
        super::settings_owner_defaults::signature(target.as_ref())
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
