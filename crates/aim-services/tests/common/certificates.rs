//! Self-signed RSA-2048 X.509 test certificates generated with openssl req
//! -x509 -newkey rsa:2048 -outform DER and CN=AIM settings test certificate N.
//! No private keys are retained; certificate validity is not checked by readXml.
pub fn certificate(index: usize) -> Vec<u8> {
    [
        include_bytes!("../fixtures/settings-certificate-1.der").as_slice(),
        include_bytes!("../fixtures/settings-certificate-2.der").as_slice(),
        include_bytes!("../fixtures/settings-certificate-3.der").as_slice(),
    ][index]
        .to_vec()
}

pub fn xml(input: &[u8]) -> Vec<u8> {
    let mut xml = std::str::from_utf8(input).unwrap().to_owned();
    for (index, aliases) in [
        ["aa", "3082aa", "0102"].as_slice(),
        &["bb", "3082bb"],
        &["cc", "3082cc"],
    ]
    .into_iter()
    .enumerate()
    {
        let hex: String = certificate(index)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        for alias in aliases {
            for quote in ["'", "\""] {
                xml = xml.replace(
                    &format!("key={quote}{alias}{quote}"),
                    &format!("key={quote}{hex}{quote}"),
                );
            }
        }
    }
    xml.into_bytes()
}
