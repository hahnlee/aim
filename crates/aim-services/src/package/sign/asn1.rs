//! The ASN.1 the verifiers read: PKCS #7 `SignedData` (a v1 signature
//! block) and the fields of an X.509 certificate they check. Encodings are
//! read as libcore's `DerInputStream` reads them: BER's indefinite lengths
//! are first made definite (`DerIndefLenConverter`), and a certificate
//! keeps its encoding verbatim (`VerbatimX509Certificate`).

/// An element: its tag, contents and whole encoding.
#[derive(Clone, Copy)]
pub(super) struct Tlv<'a> {
    pub tag: u8,
    pub value: &'a [u8],
    pub raw: &'a [u8],
}

pub(super) const INTEGER: u8 = 0x02;
pub(super) const BIT_STRING: u8 = 0x03;
pub(super) const OCTET_STRING: u8 = 0x04;
pub(super) const OID: u8 = 0x06;
pub(super) const SEQUENCE: u8 = 0x30;
pub(super) const SET: u8 = 0x31;
const BOOLEAN: u8 = 0x01;

/// The elements of a definite-length encoding, in order.
pub(super) struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Reader<'a> {
        Reader(data)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn peek_tag(&self) -> Option<u8> {
        self.0.first().copied()
    }

    pub fn next(&mut self) -> Result<Tlv<'a>, String> {
        let (tag, header, len) = header(self.0)?;
        let len = len.ok_or("indefinite length")?;
        let end = header
            .checked_add(len)
            .filter(|&e| e <= self.0.len())
            .ok_or("element longer than its container")?;
        let tlv = Tlv {
            tag,
            value: &self.0[header..end],
            raw: &self.0[..end],
        };
        self.0 = &self.0[end..];
        Ok(tlv)
    }

    /// The next element, which must have `tag`.
    pub fn expect(&mut self, tag: u8) -> Result<Tlv<'a>, String> {
        let t = self.next()?;
        if t.tag != tag {
            return Err(format!("expected tag {tag:#x}, found {:#x}", t.tag));
        }
        Ok(t)
    }

    /// The next element if it has `tag`.
    pub fn optional(&mut self, tag: u8) -> Result<Option<Tlv<'a>>, String> {
        if self.peek_tag() == Some(tag) {
            return self.next().map(Some);
        }
        Ok(None)
    }
}

/// A header: the tag, the header's size, and the contents' length (none
/// for an indefinite length).
fn header(b: &[u8]) -> Result<(u8, usize, Option<usize>), String> {
    let (&tag, rest) = b.split_first().ok_or("truncated element")?;
    if tag & 0x1f == 0x1f {
        return Err("high tag numbers are not supported".into());
    }
    let (&first, rest) = rest.split_first().ok_or("truncated length")?;
    if first < 0x80 {
        return Ok((tag, 2, Some(first as usize)));
    }
    let n = (first & 0x7f) as usize;
    if n == 0 {
        if tag & 0x20 == 0 {
            return Err("indefinite length of a primitive element".into());
        }
        return Ok((tag, 2, None));
    }
    if n > 4 || rest.len() < n {
        return Err("bad length".into());
    }
    let len = rest[..n].iter().fold(0usize, |l, &b| (l << 8) | b as usize);
    Ok((tag, 2 + n, Some(len)))
}

/// `b` with every indefinite length made definite, as
/// `DerIndefLenConverter` does before libcore parses a PKCS #7 block.
pub(super) fn definite(b: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(b.len());
    let mut rest = b;
    while !rest.is_empty() {
        rest = convert(rest, &mut out)?;
    }
    Ok(out)
}

/// Writes the first element of `b` in definite form; returns what follows
/// it.
fn convert<'a>(b: &'a [u8], out: &mut Vec<u8>) -> Result<&'a [u8], String> {
    let (tag, header, len) = header(b)?;
    let mut contents = Vec::new();
    let rest = match len {
        Some(len) => {
            let body = b
                .get(header..header + len)
                .ok_or("element longer than its container")?;
            if tag & 0x20 != 0 {
                contents = definite(body)?;
            } else {
                contents.extend_from_slice(body);
            }
            &b[header + len..]
        }
        None => {
            let mut rest = &b[header..];
            loop {
                if rest.starts_with(&[0, 0]) {
                    break &rest[2..];
                }
                if rest.is_empty() {
                    return Err("unterminated indefinite length".into());
                }
                rest = convert(rest, &mut contents)?;
            }
        }
    };
    out.push(tag);
    let len = contents.len();
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes = len.to_be_bytes();
        let skip = bytes.iter().take_while(|&&b| b == 0).count();
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
    out.extend_from_slice(&contents);
    Ok(rest)
}

/// Whether `b` uses an indefinite length anywhere.
pub(super) fn has_indefinite(b: &[u8]) -> bool {
    let mut rest = b;
    while !rest.is_empty() {
        let Ok((tag, header, len)) = header(rest) else {
            return false;
        };
        let Some(len) = len else {
            return true;
        };
        let Some(body) = rest.get(header..header + len) else {
            return false;
        };
        if tag & 0x20 != 0 && has_indefinite(body) {
            return true;
        }
        rest = &rest[header + len..];
    }
    false
}

/// An INTEGER's value with its redundant leading octets removed, so that
/// equal numbers compare equal (`BigInteger.equals`).
pub(super) fn integer(value: &[u8]) -> &[u8] {
    let mut v = value;
    while v.len() > 1 && ((v[0] == 0 && v[1] & 0x80 == 0) || (v[0] == 0xff && v[1] & 0x80 != 0)) {
        v = &v[1..];
    }
    v
}

// Object identifiers' contents.
pub(super) const RSA_ENCRYPTION: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
pub(super) const EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
pub(super) const DSA: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x38, 0x04, 0x01];
pub(super) const SIGNED_DATA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02];
pub(super) const CONTENT_TYPE: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x03];
pub(super) const MESSAGE_DIGEST: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x04];
/// `id-ce-keyUsage` (2.5.29.15).
const KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x0f];

/// The extensions BoringSSL handles (`X509_supported_extension`), by
/// `id-ce` (2.5.29.x) or Netscape number: a certificate with another
/// critical extension has an unsupported one.
const SUPPORTED_EXTENSIONS: [&[u8]; 10] = [
    &[0x60, 0x86, 0x48, 0x01, 0x86, 0xf8, 0x42, 0x01, 0x01], // nsCertType
    &[0x55, 0x1d, 0x0f],                                     // keyUsage
    &[0x55, 0x1d, 0x11],                                     // subjectAltName
    &[0x55, 0x1d, 0x13],                                     // basicConstraints
    &[0x55, 0x1d, 0x20],                                     // certificatePolicies
    &[0x55, 0x1d, 0x25],                                     // extKeyUsage
    &[0x55, 0x1d, 0x24],                                     // policyConstraints
    &[0x55, 0x1d, 0x1e],                                     // nameConstraints
    &[0x55, 0x1d, 0x21],                                     // policyMappings
    &[0x55, 0x1d, 0x36],                                     // inhibitAnyPolicy
];

/// What the verifiers read of an X.509 certificate.
pub(super) struct Certificate<'a> {
    /// The certificate's encoding, verbatim.
    pub encoded: &'a [u8],
    pub serial: &'a [u8],
    pub issuer: &'a [u8],
    /// `SubjectPublicKeyInfo`, as encoded.
    pub public_key: &'a [u8],
    /// The `keyUsage` extension's `digitalSignature` and `nonRepudiation`
    /// bits, if it has one.
    pub key_usage: Option<(bool, bool)>,
    pub unsupported_critical_extension: bool,
}

impl<'a> Certificate<'a> {
    /// The certificate `b` encodes, all of it.
    pub fn parse(b: &'a [u8]) -> Result<Certificate<'a>, String> {
        let mut outer = Reader::new(b);
        let cert = outer.expect(SEQUENCE)?;
        if !outer.is_empty() {
            return Err("data after the certificate".into());
        }
        let mut c = Reader::new(cert.value);
        let tbs = c.expect(SEQUENCE)?;
        c.expect(SEQUENCE)?;
        c.expect(BIT_STRING)?;
        let mut t = Reader::new(tbs.value);
        t.optional(0xa0)?;
        let serial = integer(t.expect(INTEGER)?.value);
        t.expect(SEQUENCE)?;
        let issuer = t.expect(SEQUENCE)?.raw;
        t.expect(SEQUENCE)?;
        t.expect(SEQUENCE)?;
        let public_key = t.expect(SEQUENCE)?.raw;
        t.optional(0x81)?;
        t.optional(0x82)?;
        let mut key_usage = None;
        let mut unsupported_critical_extension = false;
        if let Some(ext) = t.optional(0xa3)? {
            let mut list = Reader::new(Reader::new(ext.value).expect(SEQUENCE)?.value);
            while !list.is_empty() {
                let mut e = Reader::new(list.expect(SEQUENCE)?.value);
                let oid = e.expect(OID)?.value;
                let critical = e
                    .optional(BOOLEAN)?
                    .is_some_and(|b| b.value.first().is_some_and(|&v| v != 0));
                let value = e.expect(OCTET_STRING)?.value;
                if critical && !SUPPORTED_EXTENSIONS.contains(&oid) {
                    unsupported_critical_extension = true;
                }
                if oid == KEY_USAGE {
                    let bits = Reader::new(value).expect(BIT_STRING)?.value;
                    let first = bits.get(1).copied().unwrap_or(0);
                    key_usage = Some((first & 0x80 != 0, first & 0x40 != 0));
                }
            }
        }
        Ok(Certificate {
            encoded: b,
            serial,
            issuer,
            public_key,
            key_usage,
            unsupported_critical_extension,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_indefinite_lengths_definite() {
        // SEQUENCE (indefinite) { INTEGER 1, SEQUENCE (indefinite) { NULL } }
        let ber = [
            0x30, 0x80, 0x02, 0x01, 0x01, 0x30, 0x80, 0x05, 0x00, 0, 0, 0, 0,
        ];
        assert!(has_indefinite(&ber));
        let der = definite(&ber).unwrap();
        assert_eq!(der, [0x30, 0x07, 0x02, 0x01, 0x01, 0x30, 0x02, 0x05, 0x00]);
        assert!(!has_indefinite(&der));
    }

    #[test]
    fn integers_compare_by_value() {
        assert_eq!(integer(&[0x00, 0x00, 0x7f]), &[0x7f]);
        assert_eq!(integer(&[0x00, 0x80]), &[0x00, 0x80]);
        assert_eq!(integer(&[0xff, 0xff, 0x80]), &[0x80]);
        assert_eq!(integer(&[0xff, 0x7f]), &[0xff, 0x7f]);
    }
}
