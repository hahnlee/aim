//! Public-key Object Serialization streams for the pinned Android runtime
//! (#738). Layouts follow android-16.0.0_r1 Conscrypt OpenSSLRSAPublicKey,
//! OpenSSLECPublicKey and Bouncy Castle BCDSAPublicKey, including libcore's
//! BigInteger persistent fields. This writes the serialization protocol;
//! it does not execute Java or retain a serialized key from the guest.

use super::asn1::{self, BIT_STRING, INTEGER, OID, Reader, SEQUENCE};
use crate::package::pkg::Serialized;
use std::collections::BTreeMap;
mod decode;
pub use decode::public_key as decode_public_key;

struct Class {
    name: &'static str,
    uid: i64,
    flags: u8,
    fields: &'static [(u8, &'static str, Option<&'static str>)],
    parent: Option<&'static Class>,
}

const NUMBER: Class = Class {
    name: "java.lang.Number",
    uid: -8742448824652078965,
    flags: 2,
    fields: &[],
    parent: None,
};
const INTEGER_CLASS: Class = Class {
    name: "java.math.BigInteger",
    uid: -8287574255936472291,
    flags: 3,
    fields: &[
        (b'I', "bitCount", None),
        (b'I', "bitLength", None),
        (b'I', "firstNonzeroByteNum", None),
        (b'I', "lowestSetBit", None),
        (b'I', "signum", None),
        (b'[', "magnitude", Some("[B")),
    ],
    parent: Some(&NUMBER),
};
const BYTES: Class = Class {
    name: "[B",
    uid: -5984413125824719648,
    flags: 2,
    fields: &[],
    parent: None,
};
const RSA: Class = Class {
    name: "com.android.org.conscrypt.OpenSSLRSAPublicKey",
    uid: 123125005824688292,
    flags: 3,
    fields: &[
        (b'L', "modulus", Some("Ljava/math/BigInteger;")),
        (b'L', "publicExponent", Some("Ljava/math/BigInteger;")),
    ],
    parent: None,
};
const EC: Class = Class {
    name: "com.android.org.conscrypt.OpenSSLECPublicKey",
    uid: 3215842926808298020,
    flags: 3,
    fields: &[],
    parent: None,
};
const DSA: Class = Class {
    name: "com.android.org.bouncycastle.jcajce.provider.asymmetric.dsa.BCDSAPublicKey",
    uid: 1752452449903495175,
    flags: 3,
    fields: &[(b'L', "y", Some("Ljava/math/BigInteger;"))],
    parent: None,
};

struct Stream {
    bytes: Vec<u8>,
    next: u32,
    classes: BTreeMap<&'static str, u32>,
    strings: BTreeMap<&'static str, u32>,
}

impl Stream {
    fn new() -> Self {
        Self {
            bytes: vec![0xac, 0xed, 0, 5],
            next: 0x7e0000,
            classes: BTreeMap::new(),
            strings: BTreeMap::new(),
        }
    }

    fn handle(&mut self) -> u32 {
        let value = self.next;
        self.next += 1;
        value
    }

    fn reference(&mut self, handle: u32) {
        self.bytes.push(0x71);
        self.bytes.extend_from_slice(&handle.to_be_bytes());
    }

    fn utf(&mut self, text: &str) {
        // All descriptors and field names here are ASCII constants.
        self.bytes
            .extend_from_slice(&(text.len() as u16).to_be_bytes());
        self.bytes.extend_from_slice(text.as_bytes());
    }

    fn string(&mut self, text: &'static str) {
        if let Some(&handle) = self.strings.get(text) {
            self.reference(handle);
        } else {
            self.bytes.push(0x74);
            self.utf(text);
            let handle = self.handle();
            self.strings.insert(text, handle);
        }
    }

    fn class(&mut self, class: &'static Class) {
        if let Some(&handle) = self.classes.get(class.name) {
            self.reference(handle);
            return;
        }
        self.bytes.push(0x72);
        self.utf(class.name);
        self.bytes.extend_from_slice(&class.uid.to_be_bytes());
        let handle = self.handle();
        self.classes.insert(class.name, handle);
        self.bytes.push(class.flags);
        self.bytes
            .extend_from_slice(&(class.fields.len() as u16).to_be_bytes());
        for &(kind, name, signature) in class.fields {
            self.bytes.push(kind);
            self.utf(name);
            if let Some(signature) = signature {
                self.string(signature);
            }
        }
        self.bytes.push(0x78);
        match class.parent {
            Some(parent) => self.class(parent),
            None => self.bytes.push(0x70),
        }
    }

    fn object(&mut self, class: &'static Class) {
        self.bytes.push(0x73);
        self.class(class);
        self.handle();
    }

    fn array(&mut self, value: &[u8]) -> Result<(), String> {
        let length = i32::try_from(value.len()).map_err(|_| "public key too long")?;
        self.bytes.push(0x75);
        self.class(&BYTES);
        self.handle();
        self.bytes.extend_from_slice(&length.to_be_bytes());
        self.bytes.extend_from_slice(value);
        Ok(())
    }

    fn integer(&mut self, value: &[u8]) -> Result<(), String> {
        let magnitude = positive(value)?;
        self.object(&INTEGER_CLASS);
        // Android's BigInteger PutField leaves its four cached fields at
        // zero, unlike the host JDK. signum and magnitude carry the value.
        self.bytes.extend_from_slice(&[0; 16]);
        self.bytes
            .extend_from_slice(&i32::from(!magnitude.is_empty()).to_be_bytes());
        self.array(magnitude)?;
        self.bytes.push(0x78);
        Ok(())
    }
}

fn positive(value: &[u8]) -> Result<&[u8], String> {
    if value.is_empty() || value[0] & 0x80 != 0 {
        return Err("invalid public-key integer".into());
    }
    Ok(&value[value.iter().take_while(|&&v| v == 0).count()..])
}

fn integer_hash(value: &[u8]) -> Result<i32, String> {
    let magnitude = positive(value)?;
    let mut hash = 0u32;
    let mut at = 0;
    while at < magnitude.len() {
        let size = if at == 0 && magnitude.len() % 4 != 0 {
            magnitude.len() % 4
        } else {
            4
        };
        let word = magnitude[at..at + size]
            .iter()
            .fold(0u32, |v, b| (v << 8) | u32::from(*b));
        hash = hash.wrapping_mul(31).wrapping_add(word);
        at += size;
    }
    Ok(hash as i32)
}

fn key(spki: &[u8]) -> Result<(Serialized, i32, Vec<u8>), String> {
    let mut outer = Reader::new(spki);
    let mut body = Reader::new(outer.expect(SEQUENCE)?.value);
    if !outer.is_empty() {
        return Err("data after public key".into());
    }
    let mut algorithm = Reader::new(body.expect(SEQUENCE)?.value);
    let oid = algorithm.expect(OID)?.value;
    let bits = body.expect(BIT_STRING)?.value;
    if !body.is_empty() || bits.first() != Some(&0) {
        return Err("invalid public-key bits".into());
    }
    let mut stream = Stream::new();
    let (class, hash, canonical) = match oid {
        asn1::RSA_ENCRYPTION => {
            let mut encoded = Reader::new(&bits[1..]);
            let mut rsa = Reader::new(encoded.expect(SEQUENCE)?.value);
            let modulus = rsa.expect(INTEGER)?.value;
            let exponent = rsa.expect(INTEGER)?.value;
            if !encoded.is_empty() || !rsa.is_empty() {
                return Err("data after RSA key".into());
            }
            stream.object(&RSA);
            stream.integer(modulus)?;
            stream.integer(exponent)?;
            let mut algorithm = der(OID, oid);
            algorithm.extend_from_slice(&der(0x05, &[]));
            let mut value = encoded_integer(modulus)?;
            value.extend_from_slice(&encoded_integer(exponent)?);
            let mut bits = vec![0];
            bits.extend_from_slice(&der(SEQUENCE, &value));
            let mut encoded = der(SEQUENCE, &algorithm);
            encoded.extend_from_slice(&der(BIT_STRING, &bits));
            (
                &RSA,
                integer_hash(modulus)? ^ integer_hash(exponent)?,
                der(SEQUENCE, &encoded),
            )
        }
        asn1::EC_PUBLIC_KEY => {
            let curve = algorithm.expect(OID)?;
            if !algorithm.is_empty() {
                return Err("data after EC algorithm".into());
            }
            let point = match curve.value {
                [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07] => {
                    p256::ecdsa::VerifyingKey::from_sec1_bytes(&bits[1..])
                        .map_err(|e| e.to_string())?
                        .to_encoded_point(false)
                        .as_bytes()
                        .to_vec()
                }
                [0x2b, 0x81, 0x04, 0x00, 0x22] => {
                    p384::ecdsa::VerifyingKey::from_sec1_bytes(&bits[1..])
                        .map_err(|e| e.to_string())?
                        .to_encoded_point(false)
                        .as_bytes()
                        .to_vec()
                }
                [0x2b, 0x81, 0x04, 0x00, 0x23] => {
                    p521::ecdsa::VerifyingKey::from_sec1_bytes(&bits[1..])
                        .map_err(|e| e.to_string())?
                        .to_encoded_point(false)
                        .as_bytes()
                        .to_vec()
                }
                _ => return Err("unsupported public-key curve".into()),
            };
            let mut alg = der(OID, oid);
            alg.extend_from_slice(curve.raw);
            let mut encoded = der(SEQUENCE, &alg);
            let mut bits = vec![0];
            bits.extend_from_slice(&point);
            encoded.extend_from_slice(&der(BIT_STRING, &bits));
            let encoded = der(SEQUENCE, &encoded);
            let hash = encoded.iter().fold(1i32, |v, b| {
                v.wrapping_mul(31).wrapping_add(i32::from(*b as i8))
            });
            stream.object(&EC);
            stream.array(&encoded)?;
            (&EC, hash, encoded)
        }
        asn1::DSA => {
            let mut parameters = Reader::new(algorithm.expect(SEQUENCE)?.value);
            let p = parameters.expect(INTEGER)?.value;
            let q = parameters.expect(INTEGER)?.value;
            let g = parameters.expect(INTEGER)?.value;
            let mut encoded = Reader::new(&bits[1..]);
            let y = encoded.expect(INTEGER)?.value;
            if !parameters.is_empty() || !algorithm.is_empty() || !encoded.is_empty() {
                return Err("data after DSA key".into());
            }
            stream.object(&DSA);
            for value in [y, p, q, g] {
                stream.integer(value)?;
            }
            let mut parameters = encoded_integer(p)?;
            parameters.extend_from_slice(&encoded_integer(q)?);
            parameters.extend_from_slice(&encoded_integer(g)?);
            let mut algorithm = der(OID, oid);
            algorithm.extend_from_slice(&der(SEQUENCE, &parameters));
            let mut bits = vec![0];
            bits.extend_from_slice(&encoded_integer(y)?);
            let mut encoded = der(SEQUENCE, &algorithm);
            encoded.extend_from_slice(&der(BIT_STRING, &bits));
            (
                &DSA,
                integer_hash(y)? ^ integer_hash(p)? ^ integer_hash(q)? ^ integer_hash(g)?,
                der(SEQUENCE, &encoded),
            )
        }
        _ => return Err("unsupported public-key serialization algorithm".into()),
    };
    stream.bytes.push(0x78);
    Ok((
        Serialized {
            class: class.name.into(),
            bytes: stream.bytes,
        },
        hash,
        canonical,
    ))
}

fn encoded_integer(value: &[u8]) -> Result<Vec<u8>, String> {
    let magnitude = positive(value)?;
    let mut value = Vec::new();
    if magnitude.is_empty() || magnitude[0] & 0x80 != 0 {
        value.push(0);
    }
    value.extend_from_slice(magnitude);
    Ok(der(INTEGER, &value))
}

fn der(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    if value.len() < 128 {
        out.push(value.len() as u8);
    } else {
        let bytes = value.len().to_be_bytes();
        let start = bytes.iter().take_while(|&&v| v == 0).count();
        out.push(0x80 | (bytes.len() - start) as u8);
        out.extend_from_slice(&bytes[start..]);
    }
    out.extend_from_slice(value);
    out
}

/// SigningDetails's ArraySet iterates by signed key hash, retaining the
/// insertion order for collisions. Equal public keys appear once.
pub fn public_keys(keys: &[Vec<u8>]) -> Result<Vec<Serialized>, String> {
    Ok(ordered(keys)?
        .into_iter()
        .map(|(value, _, _)| value)
        .collect())
}

/// PublicKey.getEncoded and the same ArraySet ordering used in parcels.
pub(crate) fn canonical_public_keys(keys: &[Vec<u8>]) -> Result<Vec<Vec<u8>>, String> {
    Ok(ordered(keys)?
        .into_iter()
        .map(|(_, _, encoded)| encoded)
        .collect())
}

fn ordered(keys: &[Vec<u8>]) -> Result<Vec<(Serialized, i32, Vec<u8>)>, String> {
    let mut values = Vec::new();
    for spki in keys {
        let value = key(spki)?;
        if !values
            .iter()
            .any(|(serialized, hash, _)| serialized == &value.0 && hash == &value.1)
        {
            values.push(value);
        }
    }
    values.sort_by_key(|(_, hash, _)| *hash);
    Ok(values)
}
