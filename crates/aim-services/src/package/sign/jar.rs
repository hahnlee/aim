//! The JAR signature scheme (v1), as `StrictJarFile` and
//! `StrictJarVerifier` check it: each signature block (`.RSA`, `.DSA`,
//! `.EC`, PKCS #7) signs its `.SF` file, which holds digests of the
//! manifest or of its sections, which hold digests of the entries. An
//! entry's certificates are those of the signers whose `.SF` names it.

use std::collections::{HashMap, HashSet};
use std::io::Read;

use android_image_extract::source::{ReadAt, Reader};
use android_image_extract::zip::{Archive, DEFLATE, Entry, STORED};

use super::asn1::{self, Certificate, INTEGER, OCTET_STRING, OID, SEQUENCE, SET};
use super::crypto::{self, Algorithm, Hash, Key};
use super::{
    Error, INSTALL_PARSE_FAILED_BAD_MANIFEST, INSTALL_PARSE_FAILED_INCONSISTENT_CERTIFICATES,
    INSTALL_PARSE_FAILED_NO_CERTIFICATES, INSTALL_PARSE_FAILED_UNEXPECTED_EXCEPTION, v2, v3,
};

const MANIFEST_NAME: &str = "META-INF/MANIFEST.MF";
const ANDROID_MANIFEST_FILENAME: &str = "AndroidManifest.xml";
/// The `.SF` attribute naming the newer schemes that signed the APK too.
const SF_ATTRIBUTE_ANDROID_APK_SIGNED_NAME: &str = "X-Android-APK-Signed";
/// Digests by preference.
const DIGEST_ALGORITHMS: [&str; 4] = ["SHA-512", "SHA-384", "SHA-256", "SHA1"];
const MAX_JAR_SIGNERS: usize = 10;

/// `ApkSignatureVerifier.verifyV1Signature`: the signers of
/// `AndroidManifest.xml` and, when `full`, the same signers for every
/// other entry outside `META-INF/`; when `full`, a `.SF` naming a newer
/// scheme the APK lacks means that scheme's signature was stripped.
pub(super) fn verify(apk: &dyn ReadAt, path: &str, full: bool) -> Result<Vec<Vec<u8>>, Error> {
    let no_certs = |m: String| Error::new(INSTALL_PARSE_FAILED_NO_CERTIFICATES, m);
    let jar = JarFile::open(apk, full)
        .map_err(|e| no_certs(format!("failed to collect certificates from {path}: {e}")))?;
    let manifest = jar.find(ANDROID_MANIFEST_FILENAME).ok_or_else(|| {
        Error::new(
            INSTALL_PARSE_FAILED_BAD_MANIFEST,
            format!("package {path} has no manifest"),
        )
    })?;
    let certs = |entry: &Entry| -> Result<Vec<Vec<u8>>, Error> {
        let name = String::from_utf8_lossy(&entry.name);
        let certs = jar.certificates(entry).map_err(|e| {
            Error::new(
                INSTALL_PARSE_FAILED_UNEXPECTED_EXCEPTION,
                format!("failed reading {name} in {path}: {e}"),
            )
        })?;
        if certs.is_empty() {
            return Err(no_certs(format!(
                "package {path} has no certificates at entry {name}"
            )));
        }
        Ok(certs)
    };
    let signers = certs(manifest)?;
    if full {
        for entry in &jar.archive.entries {
            let name = String::from_utf8_lossy(&entry.name);
            if name.ends_with('/')
                || name.starts_with("META-INF/")
                || name == ANDROID_MANIFEST_FILENAME
            {
                continue;
            }
            if certs(entry)? != signers {
                return Err(Error::new(
                    INSTALL_PARSE_FAILED_INCONSISTENT_CERTIFICATES,
                    format!("package {path} has mismatched certificates at entry {name}"),
                ));
            }
        }
    }
    Ok(signers)
}

/// A `StrictJarFile` opened to verify: its manifest, and the signers whose
/// signature over their `.SF` and whose `.SF` over the manifest verified.
struct JarFile<'a> {
    apk: &'a dyn ReadAt,
    archive: Archive<'a>,
    manifest: Manifest,
    /// The verified signers' `.SF` names, in the order the original's
    /// `Hashtable` lists them, with the sections each signs and its
    /// signing certificate.
    signatures: Vec<(String, HashMap<String, Attributes>, Vec<u8>)>,
}

impl<'a> JarFile<'a> {
    fn open(apk: &'a dyn ReadAt, rollback_protection: bool) -> Result<JarFile<'a>, String> {
        let archive = Archive::open(apk).map_err(|e| e.to_string())?;
        let mut names = HashSet::new();
        if let Some(e) = archive.entries.iter().find(|e| !names.insert(&e.name)) {
            return Err(format!(
                "duplicate entry {}",
                String::from_utf8_lossy(&e.name)
            ));
        }
        let mut meta = Vec::new();
        for e in &archive.entries {
            if e.name.starts_with(b"META-INF/") {
                let mut data = Vec::new();
                read_entry(apk, &archive, e, &mut |b| data.extend_from_slice(b))?;
                meta.push((String::from_utf8_lossy(&e.name).into_owned(), data));
            }
        }
        let manifest_bytes = meta
            .iter()
            .find(|(n, _)| n == MANIFEST_NAME)
            .map(|(_, b)| b.as_slice())
            .ok_or("no manifest")?;
        let manifest = Manifest::parse(manifest_bytes).map_err(|e| e.0)?;
        for file in manifest.entries.keys() {
            if archive.find(file.as_bytes()).is_none() {
                return Err(format!("file {file} in manifest does not exist"));
            }
        }
        let mut jar = JarFile {
            apk,
            archive,
            manifest,
            signatures: Vec::new(),
        };
        jar.read_certificates(&meta, manifest_bytes, rollback_protection)?;
        Ok(jar)
    }

    fn find(&self, name: &str) -> Option<&Entry> {
        self.archive.find(name.as_bytes())
    }

    /// `StrictJarVerifier.readCertificates`: each signature block, in the
    /// order of the original's `HashMap` of `META-INF/` entries.
    fn read_certificates(
        &mut self,
        meta: &[(String, Vec<u8>)],
        manifest_bytes: &[u8],
        rollback_protection: bool,
    ) -> Result<(), String> {
        let names: Vec<&str> = meta.iter().map(|(n, _)| n.as_str()).collect();
        let mut done = HashSet::new();
        let mut verified = Vec::new();
        let mut signers = 0;
        for i in java::hash_map_order(&names) {
            let key = names[i];
            if !(key.ends_with(".DSA") || key.ends_with(".RSA") || key.ends_with(".EC")) {
                continue;
            }
            signers += 1;
            if signers > MAX_JAR_SIGNERS {
                return Err(format!(
                    "APK Signature Scheme v1 only supports a maximum of {MAX_JAR_SIGNERS} signers"
                ));
            }
            let sf_name = format!("{}.SF", &key[..key.rfind('.').unwrap()]);
            if done.contains(&sf_name) {
                continue;
            }
            let Some((_, sf)) = meta.iter().find(|(n, _)| *n == sf_name) else {
                continue;
            };
            let block = &meta[i].1;
            let cert = verify_bytes(block, sf)
                .map_err(|e| format!("{key} failed verification of {sf_name}: {e}"))?;
            if let Some(sections) =
                self.verify_sf(&sf_name, sf, manifest_bytes, rollback_protection)?
            {
                done.insert(sf_name.clone());
                verified.push((sf_name, sections, cert));
            }
        }
        let order = java::hashtable_order(verified.iter().map(|(n, _, _)| n.as_str()));
        let mut verified: Vec<_> = verified.into_iter().map(Some).collect();
        self.signatures = order
            .into_iter()
            .filter_map(|i| verified[i].take())
            .collect();
        Ok(())
    }

    /// The rest of `verifyCertificate`: the `.SF` against the manifest.
    /// None if the signer is ignored (an unreadable `.SF`, no
    /// `Signature-Version`, a section the manifest lacks).
    fn verify_sf(
        &self,
        sf_name: &str,
        sf: &[u8],
        manifest_bytes: &[u8],
        rollback_protection: bool,
    ) -> Result<Option<HashMap<String, Attributes>>, String> {
        let (main, entries) = match read_sf(sf) {
            Ok(r) => r,
            Err(ManifestError(e, true)) => return Err(e),
            Err(_) => return Ok(None),
        };
        if rollback_protection && let Some(ids) = main.get(SF_ATTRIBUTE_ANDROID_APK_SIGNED_NAME) {
            for id in ids.split(',').map(str::trim) {
                match id.parse::<i32>() {
                    Ok(v2::SF_ATTRIBUTE_ANDROID_APK_SIGNED_ID) => {
                        return Err(format!(
                            "{sf_name} indicates the APK is signed using APK Signature Scheme \
                             v2, but no such signature was found. Signature stripped?"
                        ));
                    }
                    Ok(v3::SF_ATTRIBUTE_ANDROID_APK_SIGNED_ID) => {
                        return Err(format!(
                            "{sf_name} indicates the APK is signed using APK Signature Scheme \
                             v3, but no such signature was found. Signature stripped?"
                        ));
                    }
                    _ => {}
                }
            }
        }
        if main.get("Signature-Version").is_none() {
            return Ok(None);
        }
        let signtool = main
            .get("Created-By")
            .is_some_and(|c| c.contains("signtool"));
        let main_end = self.manifest.main_end;
        if main_end > 0
            && !signtool
            && !verify_digest(
                &main,
                "-Digest-Manifest-Main-Attributes",
                &manifest_bytes[..main_end],
                true,
            )
        {
            return Err(format!("failed verification of {sf_name}"));
        }
        let whole = if signtool {
            "-Digest"
        } else {
            "-Digest-Manifest"
        };
        if !verify_digest(&main, whole, manifest_bytes, false) {
            for (name, attrs) in &entries {
                let Some(&(start, end)) = self.manifest.chunks.get(name) else {
                    return Ok(None);
                };
                let mut chunk = &manifest_bytes[start..end];
                if signtool && chunk.ends_with(b"\n\n") {
                    chunk = &chunk[..chunk.len() - 1];
                }
                if !verify_digest(attrs, "-Digest", chunk, false) {
                    return Err(format!("{sf_name} has invalid digest for {name}"));
                }
            }
        }
        Ok(Some(entries))
    }

    /// `loadCertificates`: reads the entry through the verifier, and the
    /// signing certificates of the signers that signed it, once its digest
    /// verified.
    fn certificates(&self, entry: &Entry) -> Result<Vec<Vec<u8>>, String> {
        let name = String::from_utf8_lossy(&entry.name);
        let signers: Vec<&Vec<u8>> = self
            .signatures
            .iter()
            .filter(|(_, sections, _)| sections.contains_key(name.as_ref()))
            .map(|(_, _, cert)| cert)
            .collect();
        let digest = self
            .manifest
            .entries
            .get(name.as_ref())
            .filter(|_| !signers.is_empty())
            .and_then(|attrs| {
                DIGEST_ALGORITHMS.iter().find_map(|alg| {
                    let hash = attrs.get(&format!("{alg}-Digest"))?;
                    Some((Hash::by_name(alg)?, hash.to_owned()))
                })
            });
        let Some((hash, expected)) = digest else {
            read_entry(self.apk, &self.archive, entry, &mut |_| {})?;
            return Ok(Vec::new());
        };
        let mut h = hash.hasher();
        read_entry(self.apk, &self.archive, entry, &mut |b| h.update(b))?;
        if !digest_matches(&h.finalize(), &expected) {
            return Err(format!("{MANIFEST_NAME} has invalid digest for {name}"));
        }
        Ok(signers.into_iter().cloned().collect())
    }
}

/// Streams an entry's contents: a stored entry's `size` bytes, or a
/// deflated entry inflated, which must come to `size` bytes.
fn read_entry(
    apk: &dyn ReadAt,
    archive: &Archive,
    entry: &Entry,
    sink: &mut dyn FnMut(&[u8]),
) -> Result<(), String> {
    let offset = archive.data_offset(entry).map_err(|e| e.to_string())?;
    let mut input: Box<dyn Read> = match entry.method {
        STORED => Box::new(Reader::new(apk, offset, entry.size).map_err(|e| e.to_string())?),
        DEFLATE => Box::new(flate2::read::DeflateDecoder::new(
            Reader::new(apk, offset, entry.compressed_size).map_err(|e| e.to_string())?,
        )),
        m => return Err(format!("unsupported compression method {m}")),
    };
    let mut buf = vec![0; 64 << 10];
    let mut total = 0;
    loop {
        let n = input.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        sink(&buf[..n]);
        total += n as u64;
    }
    if total != entry.size {
        return Err(format!(
            "size mismatch on inflated file: {total} vs {}",
            entry.size
        ));
    }
    Ok(())
}

/// `verify`: the first digest of `entry` the attributes name, checked
/// against `data`; `ignorable` if they name none.
fn verify_digest(attrs: &Attributes, entry: &str, data: &[u8], ignorable: bool) -> bool {
    for alg in DIGEST_ALGORITHMS {
        let Some(expected) = attrs.get(&format!("{alg}{entry}")) else {
            continue;
        };
        let Some(hash) = Hash::by_name(alg) else {
            continue;
        };
        return digest_matches(&hash.digest(data), expected);
    }
    ignorable
}

/// `verifyMessageDigest`: `encoded` is the Base64 of `digest`.
fn digest_matches(digest: &[u8], encoded: &str) -> bool {
    base64(encoded).is_some_and(|d| d == digest)
}

/// `java.util.Base64.getDecoder().decode`: the basic alphabet, padding
/// optional.
fn base64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0);
    let mut padding = 0;
    for c in s.bytes() {
        if c == b'=' {
            padding += 1;
            continue;
        }
        if padding > 0 {
            return None;
        }
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    // A last unit of two or three characters, padded or not.
    matches!((bits, padding), (0, 0) | (4, 0) | (4, 2) | (2, 0) | (2, 1)).then_some(out)
}

/// `StrictJarVerifier.verifyBytes`: the PKCS #7 block's first signer that
/// verifies over the `.SF`, and its certificate.
fn verify_bytes(block: &[u8], sf: &[u8]) -> Result<Vec<u8>, String> {
    let definite;
    let block = if asn1::has_indefinite(block) {
        definite = asn1::definite(block)?;
        &definite
    } else {
        block
    };
    let signed =
        SignedData::parse(block).map_err(|e| format!("IO exception verifying jar cert: {e}"))?;
    for info in &signed.signer_infos {
        if let Some(cert) = info.verify(&signed, sf)? {
            return Ok(cert.encoded.to_vec());
        }
    }
    Err("failed to verify signature: no verified SignerInfos".into())
}

/// PKCS #7 `SignedData`, as libcore's `PKCS7` parses it.
struct SignedData<'a> {
    content_type: &'a [u8],
    certificates: Vec<Certificate<'a>>,
    signer_infos: Vec<SignerInfo<'a>>,
}

struct SignerInfo<'a> {
    issuer: &'a [u8],
    serial: &'a [u8],
    digest_algorithm: &'a [u8],
    authenticated: Option<Authenticated<'a>>,
    encryption_algorithm: &'a [u8],
    encrypted_digest: &'a [u8],
}

impl<'a> SignedData<'a> {
    fn parse(b: &'a [u8]) -> Result<SignedData<'a>, String> {
        let mut ci = asn1::Reader::new(asn1::Reader::new(b).expect(SEQUENCE)?.value);
        if ci.expect(OID)?.value != asn1::SIGNED_DATA {
            return Err("content type not supported".into());
        }
        let content = ci.expect(0xa0)?;
        let mut sd = asn1::Reader::new(asn1::Reader::new(content.value).expect(SEQUENCE)?.value);
        sd.expect(INTEGER)?;
        sd.expect(SET)?;
        let content_type = asn1::Reader::new(sd.expect(SEQUENCE)?.value)
            .expect(OID)?
            .value;
        let mut certificates = Vec::new();
        if let Some(certs) = sd.optional(0xa0)? {
            let mut r = asn1::Reader::new(certs.value);
            while !r.is_empty() {
                let c = r.next()?;
                if c.tag == SEQUENCE {
                    certificates.push(Certificate::parse(c.raw)?);
                }
            }
        }
        sd.optional(0xa1)?;
        let mut infos = asn1::Reader::new(sd.expect(SET)?.value);
        let mut signer_infos = Vec::new();
        while !infos.is_empty() {
            signer_infos.push(SignerInfo::parse(infos.expect(SEQUENCE)?.value)?);
        }
        Ok(SignedData {
            content_type,
            certificates,
            signer_infos,
        })
    }
}

impl<'a> SignerInfo<'a> {
    fn parse(b: &'a [u8]) -> Result<SignerInfo<'a>, String> {
        let mut r = asn1::Reader::new(b);
        r.expect(INTEGER)?;
        let mut ias = asn1::Reader::new(r.expect(SEQUENCE)?.value);
        let issuer = ias.next()?.raw;
        let serial = asn1::integer(ias.expect(INTEGER)?.value);
        let digest_algorithm = algorithm(r.expect(SEQUENCE)?)?;
        let authenticated = match r.optional(0xa0)? {
            Some(attrs) => Some(authenticated_attributes(attrs)?),
            None => None,
        };
        let encryption_algorithm = algorithm(r.expect(SEQUENCE)?)?;
        let encrypted_digest = r.expect(OCTET_STRING)?.value;
        r.optional(0xa1)?;
        if !r.is_empty() {
            return Err("extra data at the end".into());
        }
        Ok(SignerInfo {
            issuer,
            serial,
            digest_algorithm,
            authenticated,
            encryption_algorithm,
            encrypted_digest,
        })
    }

    /// `SignerInfo.verify`: its certificate, if the signature over the
    /// `.SF` (or over the authenticated attributes, which hold the `.SF`'s
    /// digest) verifies. Errors where the original throws rather than
    /// skipping the signer.
    fn verify<'c>(
        &self,
        block: &'c SignedData<'a>,
        data: &[u8],
    ) -> Result<Option<&'c Certificate<'a>>, String> {
        let hash = Hash::by_oid(self.digest_algorithm).ok_or("unsupported digest algorithm");
        let signed: Vec<u8> = match &self.authenticated {
            None => data.to_vec(),
            Some(a) => {
                if a.content_type != Some(block.content_type) {
                    return Ok(None);
                }
                let Some(expected) = a.message_digest else {
                    return Ok(None);
                };
                if hash?.digest(data) != expected {
                    return Ok(None);
                }
                a.encoding.clone()
            }
        };
        let Some(cert) = block
            .certificates
            .iter()
            .find(|c| c.serial == self.serial && c.issuer == self.issuer)
        else {
            return Ok(None);
        };
        if cert.unsupported_critical_extension {
            return Err("certificate has unsupported critical extension(s)".into());
        }
        if cert.key_usage == Some((false, false)) {
            return Err("key usage restricted: cannot be used for digital signatures".into());
        }
        let alg = Algorithm {
            key: encryption_key(self.encryption_algorithm)?,
            hash: hash?,
            pss: false,
        };
        if crypto::key_type(cert.public_key)? != alg.key {
            return Err("InvalidKey: the certificate's key is not for the signature".into());
        }
        Ok(
            crypto::verify(alg, cert.public_key, &signed, self.encrypted_digest)
                .is_ok()
                .then_some(cert),
        )
    }
}

/// An `AlgorithmIdentifier`'s OID.
fn algorithm(t: asn1::Tlv<'_>) -> Result<&[u8], String> {
    Ok(asn1::Reader::new(t.value).expect(OID)?.value)
}

/// The key type a `digestEncryptionAlgorithm` names, as
/// `AlgorithmId.getEncAlgFromSigAlg` reads a signature algorithm.
fn encryption_key(oid: &[u8]) -> Result<Key, String> {
    const PKCS1: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01];
    const X9_57: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x38, 0x04];
    const NIST_SIG: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x03];
    const X9_62_SIG: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04];
    Ok(match oid {
        asn1::RSA_ENCRYPTION => Key::Rsa,
        [p @ .., 4 | 5 | 11 | 12 | 13 | 14] if p == PKCS1 => Key::Rsa,
        asn1::DSA => Key::Dsa,
        [p @ .., 3] if p == X9_57 => Key::Dsa,
        [p @ .., 1 | 2] if p == NIST_SIG => Key::Dsa,
        asn1::EC_PUBLIC_KEY => Key::Ec,
        [p @ .., 1] if p == X9_62_SIG => Key::Ec,
        [p @ .., 3, 1..=4] if p == X9_62_SIG => Key::Ec,
        _ => return Err("unsupported signature algorithm".into()),
    })
}

/// A signer's authenticated attributes (`PKCS9Attributes`): their
/// encoding as a SET, the content type and the message digest.
struct Authenticated<'a> {
    encoding: Vec<u8>,
    content_type: Option<&'a [u8]>,
    message_digest: Option<&'a [u8]>,
}

/// The authenticated attributes, each at most once.
fn authenticated_attributes(t: asn1::Tlv<'_>) -> Result<Authenticated<'_>, String> {
    let mut encoding = t.raw.to_vec();
    encoding[0] = SET;
    let (mut content_type, mut message_digest) = (None, None);
    let mut seen = HashSet::new();
    let mut r = asn1::Reader::new(t.value);
    while !r.is_empty() {
        let mut a = asn1::Reader::new(r.expect(SEQUENCE)?.value);
        let oid = a.expect(OID)?.value;
        if !seen.insert(oid) {
            return Err("duplicate PKCS9 attribute".into());
        }
        let mut values = asn1::Reader::new(a.expect(SET)?.value);
        match oid {
            asn1::CONTENT_TYPE => content_type = Some(values.expect(OID)?.value),
            asn1::MESSAGE_DIGEST => message_digest = Some(values.expect(OCTET_STRING)?.value),
            _ => {}
        }
    }
    Ok(Authenticated {
        encoding,
        content_type,
        message_digest,
    })
}

/// A manifest section's attributes: `Attributes.Name`s are compared
/// without case.
#[derive(Default)]
struct Attributes(HashMap<String, String>);

impl Attributes {
    fn get(&self, name: &str) -> Option<&str> {
        self.0.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    fn put(&mut self, name: &str, value: String) {
        self.0.insert(name.to_ascii_lowercase(), value);
    }
}

/// A manifest error, and whether the original throws it as an unchecked
/// exception (which a `.SF` does not survive either).
#[derive(Debug)]
struct ManifestError(String, bool);

/// `StrictJarManifest`: the main attributes and where they end, the named
/// sections, and each section's span.
struct Manifest {
    main_end: usize,
    entries: HashMap<String, Attributes>,
    chunks: HashMap<String, (usize, usize)>,
}

impl Manifest {
    fn parse(b: &[u8]) -> Result<Manifest, ManifestError> {
        let mut m = Manifest {
            main_end: 0,
            entries: HashMap::new(),
            chunks: HashMap::new(),
        };
        if b.is_empty() {
            return Ok(m);
        }
        let mut r = ManifestReader::new(b);
        r.read_main(&mut Attributes::default())?;
        m.main_end = r.pos;
        r.read_entries(&mut m.entries, Some(&mut m.chunks))?;
        Ok(m)
    }
}

/// A `.SF`'s main attributes and sections.
fn read_sf(b: &[u8]) -> Result<(Attributes, HashMap<String, Attributes>), ManifestError> {
    let mut r = ManifestReader::new(b);
    let mut main = Attributes::default();
    r.read_main(&mut main)?;
    let mut entries = HashMap::new();
    r.read_entries(&mut entries, None)?;
    Ok((main, entries))
}

/// `StrictJarManifestReader`.
struct ManifestReader<'a> {
    buf: &'a [u8],
    pos: usize,
    name: Option<String>,
    value: String,
    consecutive_line_breaks: u32,
}

impl<'a> ManifestReader<'a> {
    fn new(buf: &'a [u8]) -> ManifestReader<'a> {
        ManifestReader {
            buf,
            pos: 0,
            name: None,
            value: String::new(),
            consecutive_line_breaks: 0,
        }
    }

    fn read_main(&mut self, main: &mut Attributes) -> Result<(), ManifestError> {
        while self.read_header()? {
            main.put(self.name.as_deref().unwrap_or_default(), self.value.clone());
        }
        Ok(())
    }

    fn read_entries(
        &mut self,
        entries: &mut HashMap<String, Attributes>,
        mut chunks: Option<&mut HashMap<String, (usize, usize)>>,
    ) -> Result<(), ManifestError> {
        let mut mark = self.pos;
        while self.read_header()? {
            if !self
                .name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case("Name"))
            {
                return Err(ManifestError("entry is not named".into(), false));
            }
            let entry_name = self.value.clone();
            let mut attrs = entries.remove(&entry_name).unwrap_or_default();
            while self.read_header()? {
                attrs.put(self.name.as_deref().unwrap_or_default(), self.value.clone());
            }
            if let Some(chunks) = chunks.as_deref_mut() {
                if chunks.contains_key(&entry_name) {
                    return Err(ManifestError(
                        "a jar verifier does not support more than one entry with the same name"
                            .into(),
                        false,
                    ));
                }
                chunks.insert(entry_name.clone(), (mark, self.pos));
                mark = self.pos;
            }
            entries.insert(entry_name, attrs);
        }
        Ok(())
    }

    fn read_header(&mut self) -> Result<bool, ManifestError> {
        if self.consecutive_line_breaks > 1 {
            self.consecutive_line_breaks = 0;
            return Ok(false);
        }
        self.read_name()?;
        self.consecutive_line_breaks = 0;
        self.read_value()?;
        Ok(self.consecutive_line_breaks > 0)
    }

    fn read_name(&mut self) -> Result<(), ManifestError> {
        let mark = self.pos;
        while self.pos < self.buf.len() {
            let b = self.buf[self.pos];
            self.pos += 1;
            if b != b':' {
                continue;
            }
            let name: String = self.buf[mark..self.pos - 1]
                .iter()
                .map(|&c| if c < 0x80 { c as char } else { '\u{fffd}' })
                .collect();
            let Some(&next) = self.buf.get(self.pos) else {
                return Err(ManifestError("truncated attribute".into(), true));
            };
            self.pos += 1;
            if next != b' ' {
                return Err(ManifestError(
                    format!("invalid value for attribute '{name}'"),
                    false,
                ));
            }
            // `Attributes.Name`'s rule.
            if name.is_empty()
                || name.len() > 70
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(ManifestError(name, false));
            }
            self.name = Some(name);
            return Ok(());
        }
        Ok(())
    }

    fn read_value(&mut self) -> Result<(), ManifestError> {
        let mut last_cr = false;
        let mut mark = self.pos;
        let mut last = self.pos;
        let mut value = Vec::new();
        while self.pos < self.buf.len() {
            let next = self.buf[self.pos];
            self.pos += 1;
            match next {
                0 => return Err(ManifestError("NUL character in a manifest".into(), false)),
                b'\n' => {
                    if last_cr {
                        last_cr = false;
                    } else {
                        self.consecutive_line_breaks += 1;
                    }
                    continue;
                }
                b'\r' => {
                    last_cr = true;
                    self.consecutive_line_breaks += 1;
                    continue;
                }
                b' ' if self.consecutive_line_breaks == 1 => {
                    value.extend_from_slice(&self.buf[mark..last]);
                    mark = self.pos;
                    self.consecutive_line_breaks = 0;
                    continue;
                }
                _ => {}
            }
            if self.consecutive_line_breaks >= 1 {
                self.pos -= 1;
                break;
            }
            last = self.pos;
        }
        value.extend_from_slice(&self.buf[mark..last]);
        self.value = String::from_utf8_lossy(&value).into_owned();
        Ok(())
    }
}

/// The iteration orders of Java's hash tables, which decide the order of a
/// JAR's signers: libcore reads the signature blocks in its `HashMap` of
/// `META-INF/` entries' order and lists the signers in its `Hashtable`'s.
/// Entries hashing alike are taken in the archive's order.
mod java {
    /// `String.hashCode`.
    fn hash(s: &str) -> i32 {
        s.encode_utf16()
            .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
    }

    /// The order a `HashMap` iterates `keys`, put in order.
    pub fn hash_map_order(keys: &[&str]) -> Vec<usize> {
        let mut capacity = 16;
        while keys.len() > capacity * 3 / 4 {
            capacity *= 2;
        }
        let bucket = |k: &str| {
            let h = hash(k) as u32;
            ((h ^ (h >> 16)) as usize) & (capacity - 1)
        };
        let mut order: Vec<usize> = (0..keys.len()).collect();
        order.sort_by_key(|&i| bucket(keys[i]));
        order
    }

    /// The order a `Hashtable` of initial capacity 5 iterates `keys`, each
    /// put once, in order: its buckets from the last, each newest first, the table
    /// rehashed to twice its size plus one at three quarters full.
    pub fn hashtable_order<'a>(keys: impl Iterator<Item = &'a str>) -> Vec<usize> {
        let mut table: Vec<Vec<(usize, i32)>> = vec![Vec::new(); 5];
        let mut threshold = 3;
        let index = |h: i32, len: usize| (h & 0x7fffffff) as usize % len;
        for (i, k) in keys.enumerate() {
            if i >= threshold {
                let mut next = vec![Vec::new(); table.len() * 2 + 1];
                for chain in table.iter().rev() {
                    for &(j, h) in chain {
                        let len = next.len();
                        next[index(h, len)].insert(0, (j, h));
                    }
                }
                threshold = next.len() * 3 / 4;
                table = next;
            }
            let h = hash(k);
            let len = table.len();
            table[index(h, len)].insert(0, (i, h));
        }
        table.iter().rev().flatten().map(|&(i, _)| i).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_manifest_sections_and_continuations() {
        let m = b"Manifest-Version: 1.0\r\nCreated-By: x\r\n\r\nName: a/b\r\nSHA-256-Digest: abc\r\n de\r\n\r\nName: c\r\nSHA1-Digest: x\r\n\r\n";
        let manifest = Manifest::parse(m).unwrap();
        assert_eq!(manifest.main_end, 40);
        assert_eq!(manifest.entries["a/b"].get("sha-256-digest"), Some("abcde"));
        assert_eq!(manifest.chunks["a/b"], (40, 79));
        assert_eq!(manifest.chunks["c"], (79, 106));
    }

    #[test]
    fn decodes_base64_as_java_does() {
        assert_eq!(base64("aGk=").unwrap(), b"hi");
        assert_eq!(base64("aGk").unwrap(), b"hi");
        assert!(base64("a").is_none());
        assert!(base64("aGk==").is_none());
        assert!(base64("aG!=").is_none());
    }

    #[test]
    fn orders_like_java_tables() {
        // "Aa" and "BB" share a hash; a Hashtable lists the newer first.
        assert_eq!(java::hashtable_order(["Aa", "BB"].into_iter()), [1, 0]);
        assert_eq!(java::hash_map_order(&["BB", "Aa"]), [0, 1]);
    }
}
