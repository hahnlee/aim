//! The recoverable keystore's certificate files (`recoverablekeystore/
//! certificate` at `android-16.0.0_r1`): a recovery service's list of
//! endpoint certificates (`CertXml`) and the signature over it (`SigXml`),
//! read as `CertUtils` reads them, and validated as its PKIX
//! `CertPathBuilder` and `CertPathValidator` do: a path from the
//! certificate through the file's intermediates to a trusted root, each
//! certificate signed by the next, within its validity at the validation
//! date, the issuers CAs (basic constraints, path length, key usage), no
//! unrecognized critical extension, no revocation check. Policy and name
//! constraint extensions are not processed; a certificate path that
//! carries one is refused as invalid (#1888).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_android_xml::{CDSECT, ENTITY_REF, Element, Node, TEXT};

use crate::package::sign::asn1::{
    self, BIT_STRING, BOOLEAN, INTEGER, OCTET_STRING, OID, Reader, SEQUENCE,
};
use crate::package::sign::crypto::{self, Algorithm, Hash, Key};

/// `TrustedRootCertificates.GOOGLE_CLOUD_KEY_VAULT_SERVICE_V1_ALIAS`.
pub const GOOGLE_CLOUD_KEY_VAULT_SERVICE_V1_ALIAS: &str = "GoogleCloudKeyVaultServiceV1";
/// `TrustedRootCertificates.TEST_ONLY_INSECURE_CERTIFICATE_ALIAS`.
pub const TEST_ONLY_INSECURE_CERTIFICATE_ALIAS: &str = "TEST_ONLY_INSECURE_CERTIFICATE_ALIAS";

/// `TrustedRootCertificates.GOOGLE_CLOUD_KEY_VAULT_SERVICE_V1_BASE64`.
const GOOGLE_CLOUD_KEY_VAULT_SERVICE_V1: &str = concat!(
    "MIIFDzCCAvegAwIBAgIQbNdueU2o0vM9gGq4N6bhjzANBgkqhkiG9w0BAQsFADAx",
    "MS8wLQYDVQQDEyZHb29nbGUgQ2xvdWQgS2V5IFZhdWx0IFNlcnZpY2UgUm9vdCBD",
    "QTAeFw0xODA1MDcxODI0MDJaFw0zODA1MDgxOTI0MDJaMDExLzAtBgNVBAMTJkdv",
    "b2dsZSBDbG91ZCBLZXkgVmF1bHQgU2VydmljZSBSb290IENBMIICIjANBgkqhkiG",
    "9w0BAQEFAAOCAg8AMIICCgKCAgEArUgzu+4o9yl22eql1BiGBq3gWXooh2ql3J+v",
    "Vuzf/ThjzdIg0xkkkw/NAFxYFi49Eo1fa/hf8wCIoAqCEs1lD6tE3cCD3T3+EQPq",
    "uh6CB2KmZDJ6mPnXvVUlUuFr0O2MwZkwylqBETzK0x5NCHgL/p47vkjhHx6LqVao",
    "bigKlHxszvVi4fkt/qq7KW3YTVxhwdLGEab+OqSfwMxdBLhMfE0K0dvFt8bs8yJA",
    "F04DJsMbRChFFBpT17Z0u53iIAAu5qVQhKrQXiIAwgboZqd+JkHLXU1fJeVT5WJO",
    "JgoJFWHkdWkHta4mSYlS72J1Q927JD1JdET1kFtH+EDtYAtx7x7F9xAAbb2tMITw",
    "s/wwd2rAzZTX/kxRbDlXVLToU05LFYPr+dFV1wvXmi0jlkIxnhdaVBqWC93p528U",
    "iUcLpib+HVzMWGdYI3G1NOa/lTp0c8LcbJjapiiVneRQJ3cIqDPOSEnEq40hyZd1",
    "jx3JnOxJMwHs8v4s9GIlb3BcOmDvA/Mu09xEMKwpHBm4TFDKXeGHOWha7ccWEECb",
    "yO5ncu6XuN2iyz9S+TuMyjZBE552p6Pu5gEC2xk+qab0NGDTHdLKLbyWn3IxdmBH",
    "yTr7iPCqmpyHngkC/pbGfvGusc5BpBugsBtlz67m4RWLJ72yAeVPO/ly/8w4orNs",
    "GWjn3s0CAwEAAaMjMCEwDgYDVR0PAQH/BAQDAgGGMA8GA1UdEwEB/wQFMAMBAf8w",
    "DQYJKoZIhvcNAQELBQADggIBAGiWlu+4qyxgPb6RsA0mwR7V21UJ9rEpYhSN+ARp",
    "TWGiI22RCJSGK0ZrPGeFQzE2BpnVRdmLTV5jf9JUStjHoPvNYFnwLTJ0E2e9Olj8",
    "MrHrAucAUFLhl4woWz0kU/X0EB1j6Y2SXrAaZPiMMpq8BKj3mH1MbV4stZ0kiHUp",
    "Zu6PEmrojYG7FKKN30na2xXfiOfl2JusVsyHDqmUn/HjTh6zASKqE6hxE+FJRl2V",
    "Q4dcr4SviHtdbimMy2LghLnZ4FE4XhJgRnw9TeRV5C9Sn7pmnAA5X0C8ZXhXvfvr",
    "dx4fL3UKlk1Lqlb5skxoK1R9wwr+aNIO+cuR8JA5DmEDWFw5Budh/uWWZlBTyVW2",
    "ybbTB6tkmOc8c08XOgxBaKrsXALmJcluabjmN1jp81ae1epeN31jJ4N5IE5aq7Xb",
    "TFmKkwpgTTvJmqCR2XzWujlvdbdjfiABliWsnLzLQCP8eZwcM4LA5UK3f1ktHolr",
    "1OI9etSOkebE2py8LPYBJWlX36tRAagZhU/NoyOtvhRzq9rb3rbf96APEHKUFsXG",
    "9nBEd2BUKZghLKPf+JNCU/2pOGx0jdMcf+K+a1DeG0YzGYMRkFvpN3hvHYrJdByL",
    "3kSP3UtD0H2g8Ps7gRLELG2HODxbSn8PV3XtuSvxVanA6uyaaS3AZ6SxeVLvmw50",
    "7aYI",
);

/// `TrustedRootCertificates.TEST_ONLY_INSECURE_CERTIFICATE_BASE64`.
const TEST_ONLY_INSECURE_CERTIFICATE: &str = concat!(
    "MIIFMDCCAxigAwIBAgIJAIZ9/G8KQie9MA0GCSqGSIb3DQEBDQUAMCUxIzAhBgNV",
    "BAMMGlRlc3QgT25seSBVbnNlY3VyZSBSb290IENBMB4XDTE4MDMyODAwMzIyM1oX",
    "DTM4MDMyMzAwMzIyM1owJTEjMCEGA1UEAwwaVGVzdCBPbmx5IFVuc2VjdXJlIFJv",
    "b3QgQ0EwggIiMA0GCSqGSIb3DQEBAQUAA4ICDwAwggIKAoICAQDGxFNzAEyzSPmw",
    "E5gfuBXdXq++bl9Ep62V7Xn1UiejvmS+pRHT39pf/M7sl4Zr9ezanJTrFvf9+B85",
    "VGehdsD32TgfEjThcqaoQCI6pKkHYsUo7FZ5n+G3eE8oabWRZJMVo3QDjnnFYp7z",
    "20vnpjDofI2oQyxHcb/1yep+ca1+4lIvbUp/ybhNFqhRXAMcDXo7pyH38eUQ1JdK",
    "Q/QlBbShpFEqx1Y6KilKfTDf7Wenqr67LkaEim//yLZjlHzn/BpuRTrpo+XmJZx1",
    "P9CX9LGOXTtmsaCcYgD4yijOvV8aEsIJaf1kCIO558oH0oQc+0JG5aXeLN7BDlyZ",
    "vH0RdSx5nQLS9kj2I6nthOw/q00/L+S6A0m5jyNZOAl1SY78p+wO0d9eHbqQzJwf",
    "EsSq3qGAqlgQyyjp6oxHBqT9hZtN4rxw+iq0K1S4kmTLNF1FvmIB1BE+lNvvoGdY",
    "5G0b6Pe4R5JFn9LV3C3PEmSYnae7iG0IQlKmRADIuvfJ7apWAVanJPJAAWh2Akfp",
    "8Uxr02cHoY6o7vsEhJJOeMkipaBHThESm/XeFVubQzNfZ9gjQnB9ZX2v+lyj+WYZ",
    "SAz3RuXx6TlLrmWccMpQDR1ibcgyyjLUtX3kwZl2OxmJXitjuD7xlxvAXYob15N+",
    "K4xKHgxUDrbt2zU/tY0vgepAUg/xbwIDAQABo2MwYTAdBgNVHQ4EFgQUwyeNpYgs",
    "XXYvh9z0/lFrja7sV+swHwYDVR0jBBgwFoAUwyeNpYgsXXYvh9z0/lFrja7sV+sw",
    "DwYDVR0TAQH/BAUwAwEB/zAOBgNVHQ8BAf8EBAMCAYYwDQYJKoZIhvcNAQENBQAD",
    "ggIBAGuOsvMN5SD3RIQnMJtBpcHNrxun+QFjPZFlYCLfIPrUkHpn5O1iIIq8tVLd",
    "2V+12VKnToUEANsYBD3MP8XjP+6GZ7ZQ2rwLGvUABKSX4YXvmjEEXZUZp0y3tIV4",
    "kUDlbACzguPneZDp5Qo7YWH4orgqzHkn0sD/ikO5XrAqmzc245ewJlrf+V11mjcu",
    "ELfDrEejpPhi7Hk/ZNR0ftP737Hs/dNoCLCIaVNgYzBZhgo4kd220TeJu2ttW0XZ",
    "ldyShtpcOmyWKBgVseixR6L/3sspPHyAPXkSuRo0Eh1xvzDKCg9ttb0qoacTlXMF",
    "GkBpNzmVq67NWFGGa9UElift1mv6RfktPCAGZ+Ai8xUiKAUB0Eookpt/8gX9Senq",
    "yP/jMxkxXmHWxUu8+KnLvj6WLrfftuuD7u3cfc7j5kkrheDz3O4h4477GnqL5wdo",
    "9DuEsNc4FxJVz8Iy8RS6cJuW4pihYpM1Tyn7uopLnImpYzEY+R5aQqqr+q/A1diq",
    "ogbEKPH6oUiqJUwq3nD70gPBUKJmIzS4vLwLouqUHEm1k/MgHV/BkEU0uVHszPFa",
    "XUMMCHb0iT9P8LuZ7Ajer3SR/0TRVApCrk/6OV68e+6k/OFpM5kcZnNMD5ANyBri",
    "Tsz3NrDwSw4i4+Dsfh6A9dB/cEghw4skLaBxnQLQIgVeqCzK",
);

/// `TestOnlyInsecureCertificateHelper.getValidationDate` of the test-only
/// root, which has expired: `new Date(2019 - 1900, 1, 30)`, March 2, 2019
/// at midnight (UTC here, the device's zone there).
const TEST_ONLY_VALIDATION_DATE: i64 = 1_551_484_800;

/// A failure `CertUtils` reports: the files could not be read
/// (`CertParsingException`), or a certificate or signature is not valid
/// (`CertValidationException`).
#[derive(Debug, PartialEq)]
pub enum CertError {
    Parsing(String),
    Validation(String),
}

/// The root certificate of `alias` (`TestOnlyInsecureCertificateHelper
/// .getRootCertificate`), none for an alias that names none.
pub fn root_certificate(alias: &str) -> Option<Vec<u8>> {
    let base64 = match alias {
        GOOGLE_CLOUD_KEY_VAULT_SERVICE_V1_ALIAS => GOOGLE_CLOUD_KEY_VAULT_SERVICE_V1,
        TEST_ONLY_INSECURE_CERTIFICATE_ALIAS => TEST_ONLY_INSECURE_CERTIFICATE,
        _ => return None,
    };
    aim_android_xml::base64::decode(base64)
}

/// `getValidationDate`: the test-only root's fixed date, else now.
pub fn validation_date(alias: &str) -> i64 {
    if alias == TEST_ONLY_INSECURE_CERTIFICATE_ALIAS {
        return TEST_ONLY_VALIDATION_DATE;
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// `CertXml`: the file's serial, intermediates and endpoints.
pub struct CertXml {
    pub serial: i64,
    pub intermediates: Vec<Vec<u8>>,
    pub endpoints: Vec<Vec<u8>>,
}

/// `SigXml`: the signer, its intermediates and the signature.
pub struct SigXml {
    pub intermediates: Vec<Vec<u8>>,
    pub signer: Vec<u8>,
    pub signature: Vec<u8>,
}

/// How many nodes a path must reach (`CertUtils.MUST_EXIST_*`).
#[derive(Clone, Copy, PartialEq)]
enum MustExist {
    Unenforced,
    ExactlyOne,
    AtLeastOne,
}

/// `getXmlNodeContents`: the text of the nodes at `tags` below `root`,
/// without whitespace.
fn node_contents(
    must_exist: MustExist,
    root: &Element,
    tags: &[&str],
) -> Result<Vec<String>, CertError> {
    let (leaf, path) = tags
        .split_last()
        .ok_or_else(|| CertError::Parsing("The tag list must not be empty".into()))?;
    let mut parent = root;
    for tag in path {
        let children: Vec<&Element> = parent.children().filter(|e| e.name == *tag).collect();
        if (children.is_empty() && must_exist != MustExist::Unenforced) || children.len() > 1 {
            return Err(CertError::Parsing(format!(
                "The XML file must contain exactly one path with the tag {tag}"
            )));
        }
        let Some(child) = children.first() else {
            return Ok(Vec::new());
        };
        parent = child;
    }
    let leafs: Vec<&Element> = parent.children().filter(|e| e.name == *leaf).collect();
    if must_exist == MustExist::ExactlyOne && leafs.len() != 1 {
        return Err(CertError::Parsing(format!(
            "The XML file must contain exactly one node with the path {}",
            tags.join("/")
        )));
    }
    if must_exist == MustExist::AtLeastOne && leafs.is_empty() {
        return Err(CertError::Parsing(format!(
            "The XML file must contain at least one node with the path {}",
            tags.join("/")
        )));
    }
    Ok(leafs
        .into_iter()
        .map(|e| {
            let mut text = String::new();
            text_content(e, &mut text);
            text.retain(|c| !c.is_whitespace());
            text
        })
        .collect())
}

/// `Node.getTextContent`: the text of all the element's descendants.
fn text_content(e: &Element, out: &mut String) {
    for node in &e.content {
        match node {
            Node::Element(child) => text_content(child, out),
            Node::Token(TEXT | CDSECT | ENTITY_REF, Some(text)) => out.push_str(text),
            Node::Token(..) => {}
        }
    }
}

/// `CertUtils.decodeBase64`.
fn decode_base64(s: &str) -> Result<Vec<u8>, CertError> {
    aim_android_xml::base64::decode(s)
        .ok_or_else(|| CertError::Parsing("Illegal base64 character".into()))
}

/// `decodeCert` of each base64 content: a certificate it parses.
fn certificates(contents: Vec<String>) -> Result<Vec<Vec<u8>>, CertError> {
    contents
        .iter()
        .map(|c| {
            let der = decode_base64(c)?;
            Certificate::parse(&der).map_err(CertError::Parsing)?;
            Ok(der)
        })
        .collect()
}

/// `getXmlRootNode`.
fn root(bytes: &[u8]) -> Result<Element, CertError> {
    aim_android_xml::text::read(bytes).map_err(CertError::Parsing)
}

impl CertXml {
    /// `CertXml.parse`. A serial that is no number fails as
    /// `Long.parseLong` does (`NumberFormatException`), which is no
    /// `CertParsingException`.
    pub fn parse(bytes: &[u8]) -> Result<Result<CertXml, String>, CertError> {
        let root = root(bytes)?;
        let serial = node_contents(MustExist::ExactlyOne, &root, &["metadata", "serial"])?;
        let intermediates = certificates(node_contents(
            MustExist::Unenforced,
            &root,
            &["intermediates", "cert"],
        )?)?;
        let endpoints = certificates(node_contents(
            MustExist::AtLeastOne,
            &root,
            &["endpoints", "cert"],
        )?)?;
        Ok(match serial[0].parse() {
            Ok(serial) => Ok(CertXml {
                serial,
                intermediates,
                endpoints,
            }),
            Err(_) => Err(format!("For input string: \"{}\"", serial[0])),
        })
    }

    /// `getEndpointCert` of `index`: its validated path.
    pub fn endpoint_path(
        &self,
        index: usize,
        root: &[u8],
        date: i64,
    ) -> Result<Vec<Vec<u8>>, CertError> {
        validate(date, root, &self.intermediates, &self.endpoints[index])
    }
}

impl SigXml {
    /// `SigXml.parse`.
    pub fn parse(bytes: &[u8]) -> Result<SigXml, CertError> {
        let root = root(bytes)?;
        let intermediates = certificates(node_contents(
            MustExist::Unenforced,
            &root,
            &["intermediates", "cert"],
        )?)?;
        let signer = certificates(node_contents(
            MustExist::ExactlyOne,
            &root,
            &["certificate"],
        )?)?
        .remove(0);
        let signature =
            decode_base64(&node_contents(MustExist::ExactlyOne, &root, &["value"])?[0])?;
        Ok(SigXml {
            intermediates,
            signer,
            signature,
        })
    }

    /// `verifyFileSignature`: the signer's path to `root`, then its RSA
    /// SHA-256 signature over `signed` (`verifyRsaSha256Signature`).
    pub fn verify_file_signature(
        &self,
        root: &[u8],
        signed: &[u8],
        date: i64,
    ) -> Result<(), CertError> {
        validate(date, root, &self.intermediates, &self.signer)?;
        let signer = Certificate::parse(&self.signer).map_err(CertError::Validation)?;
        let alg = Algorithm {
            key: Key::Rsa,
            hash: Hash::Sha256,
            pss: false,
        };
        crypto::verify(alg, signer.public_key, signed, &self.signature)
            .map_err(|_| CertError::Validation("The signature is invalid".into()))
    }
}

/// `CertPath.getEncoded("PkiPath")` of a path, leaf first: a SEQUENCE of
/// its certificates, the one the root issued first.
pub fn pki_path(path: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = path.iter().rev().flatten().copied().collect();
    let mut out = vec![SEQUENCE];
    der_length(body.len(), &mut out);
    out.extend_from_slice(&body);
    out
}

fn der_length(len: usize, out: &mut Vec<u8>) {
    if len < 0x80 {
        out.push(len as u8);
        return;
    }
    let bytes: Vec<u8> = len
        .to_be_bytes()
        .into_iter()
        .skip_while(|&b| b == 0)
        .collect();
    out.push(0x80 | bytes.len() as u8);
    out.extend_from_slice(&bytes);
}

/// The fields of an X.509 certificate path validation reads.
struct Certificate<'a> {
    tbs: &'a [u8],
    signature_algorithm: &'a [u8],
    signature: &'a [u8],
    v3: bool,
    issuer: &'a [u8],
    subject: &'a [u8],
    not_before: i64,
    not_after: i64,
    public_key: &'a [u8],
    /// `basicConstraints`: whether a CA, and its path length.
    basic_constraints: Option<(bool, Option<u64>)>,
    /// `keyUsage`'s `keyCertSign` bit, if it has the extension.
    key_cert_sign: Option<bool>,
    /// A critical extension path validation does not process.
    unsupported_critical: Option<Vec<u8>>,
}

const BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1d, 0x13];
const KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x0f];
const SUBJECT_ALT_NAME: &[u8] = &[0x55, 0x1d, 0x11];

impl<'a> Certificate<'a> {
    fn parse(b: &'a [u8]) -> Result<Certificate<'a>, String> {
        let mut outer = Reader::new(b);
        let cert = outer.expect(SEQUENCE)?;
        if !outer.is_empty() {
            return Err("data after the certificate".into());
        }
        let mut c = Reader::new(cert.value);
        let tbs = c.expect(SEQUENCE)?;
        let mut sig_alg = Reader::new(c.expect(SEQUENCE)?.value);
        let signature_algorithm = sig_alg.expect(OID)?.value;
        let signature = c
            .expect(BIT_STRING)?
            .value
            .get(1..)
            .ok_or("empty signature")?;
        let mut t = Reader::new(tbs.value);
        let v3 = match t.optional(0xa0)? {
            Some(version) => Reader::new(version.value).expect(INTEGER)?.value == [2],
            None => false,
        };
        t.expect(INTEGER)?;
        t.expect(SEQUENCE)?;
        let issuer = t.expect(SEQUENCE)?.raw;
        let mut validity = Reader::new(t.expect(SEQUENCE)?.value);
        let not_before = time(validity.next()?)?;
        let not_after = time(validity.next()?)?;
        let subject = t.expect(SEQUENCE)?.raw;
        let public_key = t.expect(SEQUENCE)?.raw;
        t.optional(0x81)?;
        t.optional(0x82)?;
        let mut basic_constraints = None;
        let mut key_cert_sign = None;
        let mut unsupported_critical = None;
        if let Some(ext) = t.optional(0xa3)? {
            let mut list = Reader::new(Reader::new(ext.value).expect(SEQUENCE)?.value);
            while !list.is_empty() {
                let mut e = Reader::new(list.expect(SEQUENCE)?.value);
                let oid = e.expect(OID)?.value;
                let critical = e
                    .optional(BOOLEAN)?
                    .is_some_and(|b| b.value.first().is_some_and(|&v| v != 0));
                let value = e.expect(OCTET_STRING)?.value;
                match oid {
                    BASIC_CONSTRAINTS => {
                        let mut s = Reader::new(Reader::new(value).expect(SEQUENCE)?.value);
                        let ca = s
                            .optional(BOOLEAN)?
                            .is_some_and(|b| b.value.first().is_some_and(|&v| v != 0));
                        let len = s.optional(INTEGER)?.map(|i| {
                            asn1::integer(i.value)
                                .iter()
                                .fold(0u64, |n, &b| n << 8 | b as u64)
                        });
                        basic_constraints = Some((ca, len));
                    }
                    KEY_USAGE => {
                        let bits = Reader::new(value).expect(BIT_STRING)?.value;
                        key_cert_sign = Some(bits.get(1).is_some_and(|b| b & 0x04 != 0));
                    }
                    SUBJECT_ALT_NAME => {}
                    _ if critical => unsupported_critical = Some(oid.to_vec()),
                    _ => {}
                }
            }
        }
        Ok(Certificate {
            tbs: tbs.raw,
            signature_algorithm,
            signature,
            v3,
            issuer,
            subject,
            not_before,
            not_after,
            public_key,
            basic_constraints,
            key_cert_sign,
            unsupported_critical,
        })
    }

    /// Whether `issuer`'s key verifies this certificate's signature.
    fn signed_by(&self, issuer: &Certificate<'_>) -> Result<(), String> {
        let alg = signature_algorithm(self.signature_algorithm)?;
        crypto::verify(alg, issuer.public_key, self.tbs, self.signature)
    }
}

/// The signature algorithms a certificate may be signed with.
fn signature_algorithm(oid: &[u8]) -> Result<Algorithm, String> {
    const PKCS1: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01];
    const ECDSA: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03];
    let (key, hash) = match oid.split_last() {
        Some((n, prefix)) if prefix == PKCS1 => (
            Key::Rsa,
            match n {
                0x05 => Hash::Sha1,
                0x0b => Hash::Sha256,
                0x0c => Hash::Sha384,
                0x0d => Hash::Sha512,
                0x0e => Hash::Sha224,
                _ => return Err("unsupported signature algorithm".into()),
            },
        ),
        Some((n, prefix)) if prefix == ECDSA => (
            Key::Ec,
            match n {
                0x01 => Hash::Sha224,
                0x02 => Hash::Sha256,
                0x03 => Hash::Sha384,
                0x04 => Hash::Sha512,
                _ => return Err("unsupported signature algorithm".into()),
            },
        ),
        _ => return Err("unsupported signature algorithm".into()),
    };
    Ok(Algorithm {
        key,
        hash,
        pss: false,
    })
}

/// An X.509 `Time` (`UTCTime`, `GeneralizedTime`) in seconds since 1970.
fn time(t: asn1::Tlv<'_>) -> Result<i64, String> {
    let s = std::str::from_utf8(t.value).map_err(|_| "bad time")?;
    let s = s.strip_suffix('Z').ok_or("a time not in UTC")?;
    let s = s.split('.').next().unwrap_or_default();
    let digits = |r: std::ops::Range<usize>| -> Result<i64, String> {
        s.get(r)
            .and_then(|d| d.parse().ok())
            .ok_or_else(|| "bad time".to_string())
    };
    let (year, rest) = match t.tag {
        0x17 => {
            let yy = digits(0..2)?;
            (if yy < 50 { 2000 + yy } else { 1900 + yy }, 2)
        }
        0x18 => (digits(0..4)?, 4),
        _ => return Err("bad time".into()),
    };
    let (month, day) = (digits(rest..rest + 2)?, digits(rest + 2..rest + 4)?);
    let (hour, minute) = (digits(rest + 4..rest + 6)?, digits(rest + 6..rest + 8)?);
    let second = if s.len() > rest + 8 {
        digits(rest + 8..rest + 10)?
    } else {
        0
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Ok(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// `CertUtils.validateCert`: the path from `leaf` through `intermediates`
/// to `root` that `CertPathBuilder` builds, validated at `date`; the path
/// leaf first, the root left out.
pub fn validate(
    date: i64,
    root: &[u8],
    intermediates: &[Vec<u8>],
    leaf: &[u8],
) -> Result<Vec<Vec<u8>>, CertError> {
    let invalid = |e: String| CertError::Validation(e);
    let root = Certificate::parse(root).map_err(invalid)?;
    let mut store = vec![(leaf, Certificate::parse(leaf).map_err(invalid)?)];
    for i in intermediates {
        store.push((i, Certificate::parse(i).map_err(invalid)?));
    }
    let mut path = vec![0usize];
    if !build(&root, &store, &mut path) {
        return Err(invalid("Unable to find certificate chain.".into()));
    }
    // Basic path validation, from the root's subject down to the leaf.
    let n = path.len();
    let mut max_path_length = n as u64;
    let mut working = &root;
    for (step, &at) in path.iter().rev().enumerate() {
        let cert = &store[at].1;
        let last = step + 1 == n;
        cert.signed_by(working)
            .map_err(|e| invalid(format!("Could not validate certificate signature: {e}")))?;
        if date < cert.not_before || date > cert.not_after {
            return Err(invalid(
                "Could not validate certificate: certificate expired or not yet valid".into(),
            ));
        }
        if cert.issuer != working.subject {
            return Err(invalid(
                "IssuerName does not match SubjectName of issuer".into(),
            ));
        }
        if let Some(oid) = &cert.unsupported_critical {
            return Err(invalid(format!(
                "Certificate has unsupported critical extension: {oid:02x?}"
            )));
        }
        if !last {
            let self_issued = cert.issuer == cert.subject;
            if !self_issued {
                if max_path_length == 0 {
                    return Err(invalid("Max path length not greater than zero".into()));
                }
                max_path_length -= 1;
            }
            match cert.basic_constraints {
                Some((true, len)) => {
                    if let Some(len) = len {
                        max_path_length = max_path_length.min(len);
                    }
                }
                _ if !cert.v3 => {
                    return Err(invalid(
                        "Version 1 certificates can't be used as CA ones.".into(),
                    ));
                }
                _ => return Err(invalid("Not a CA certificate".into())),
            }
            if cert.key_cert_sign == Some(false) {
                return Err(invalid("Issuer certificate keyusage extension is critical and does not permit key signing.".into()));
            }
        }
        working = cert;
    }
    Ok(path.iter().map(|&i| store[i].0.to_vec()).collect())
}

/// Extends `path` (indices into `store`, leaf first) to a certificate the
/// root issued, as a builder searching the store does.
fn build(
    root: &Certificate<'_>,
    store: &[(&[u8], Certificate<'_>)],
    path: &mut Vec<usize>,
) -> bool {
    let current = &store[*path.last().unwrap()].1;
    if current.issuer == root.subject && current.signed_by(root).is_ok() {
        return true;
    }
    for (i, (_, candidate)) in store.iter().enumerate() {
        if path.contains(&i)
            || candidate.subject != current.issuer
            || current.signed_by(candidate).is_err()
        {
            continue;
        }
        path.push(i);
        if build(root, store, path) {
            return true;
        }
        path.pop();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `TestData.ROOT_CA_TRUSTED` (`recoverablekeystore/certificate`'s
    /// tests): the root the test files are signed under.
    const ROOT_CA_TRUSTED: &str =
        include_str!("../../tests/fixtures/recoverable_keystore/root-ca-trusted.b64");
    const CERT_XML: &[u8] =
        include_bytes!("../../tests/fixtures/recoverable_keystore/valid-cert-file.xml");
    const SIG_XML: &[u8] =
        include_bytes!("../../tests/fixtures/recoverable_keystore/valid-sig-file.sig.xml");
    /// `TestData.DATE_ALL_CERTS_VALID`, `DATE_INTERMEDIATE_CA_2_EXPIRED`
    /// (to the next second).
    const DATE_ALL_CERTS_VALID: i64 = 1_516_406_400;
    const DATE_INTERMEDIATE_CA_2_EXPIRED: i64 = 1_673_397_484;

    fn root() -> Vec<u8> {
        let b64: String = ROOT_CA_TRUSTED
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        aim_android_xml::base64::decode(&b64).unwrap()
    }

    #[test]
    fn the_cert_file_is_read_as_cert_xml_reads_it() {
        let x = CertXml::parse(CERT_XML).unwrap().unwrap();
        assert_eq!(x.serial, 1000);
        assert_eq!((x.intermediates.len(), x.endpoints.len()), (2, 3));
    }

    #[test]
    fn the_signature_file_verifies_as_sig_xml_verifies_it() {
        let sig = SigXml::parse(SIG_XML).unwrap();
        sig.verify_file_signature(&root(), CERT_XML, DATE_ALL_CERTS_VALID)
            .unwrap();
        assert!(matches!(
            sig.verify_file_signature(&root(), CERT_XML, DATE_INTERMEDIATE_CA_2_EXPIRED),
            Err(CertError::Validation(_))
        ));
        let mut modified = CERT_XML.to_vec();
        modified[0] ^= 1;
        assert_eq!(
            sig.verify_file_signature(&root(), &modified, DATE_ALL_CERTS_VALID),
            Err(CertError::Validation("The signature is invalid".into()))
        );
        let google = root_certificate(GOOGLE_CLOUD_KEY_VAULT_SERVICE_V1_ALIAS).unwrap();
        assert!(matches!(
            sig.verify_file_signature(&google, CERT_XML, DATE_ALL_CERTS_VALID),
            Err(CertError::Validation(_))
        ));
    }

    #[test]
    fn an_endpoint_path_runs_to_the_root() {
        let x = CertXml::parse(CERT_XML).unwrap().unwrap();
        let path = x.endpoint_path(0, &root(), DATE_ALL_CERTS_VALID).unwrap();
        assert_eq!(path[0], x.endpoints[0]);
        assert!(path.len() >= 2);
        let encoded = pki_path(&path);
        assert_eq!(encoded[0], SEQUENCE);
        assert!(encoded.ends_with(&path[0]));
    }

    #[test]
    fn times_are_seconds_since_1970() {
        let utc = asn1::Tlv {
            tag: 0x17,
            value: b"180120000000Z",
            raw: &[],
        };
        assert_eq!(time(utc).unwrap(), DATE_ALL_CERTS_VALID);
        let generalized = asn1::Tlv {
            tag: 0x18,
            value: b"20180120000000Z",
            raw: &[],
        };
        assert_eq!(time(generalized).unwrap(), DATE_ALL_CERTS_VALID);
    }
}
