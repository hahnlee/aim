//! Digests and signature verification, by the JCA names the verifiers ask
//! for: `SHA256withRSA`, `SHA256withRSA/PSS`, `SHA256withECDSA`,
//! `SHA256withDSA` and their kin, over a key in X.509
//! `SubjectPublicKeyInfo` form, as Conscrypt (BoringSSL) verifies them.

use p256::ecdsa::signature::hazmat::PrehashVerifier;
use rsa::pkcs1::der::Decode;
use rsa::pkcs8::DecodePublicKey;
use rsa::{BigUint, Pkcs1v15Sign, Pss, RsaPublicKey};
use sha2::digest::DynDigest;

use super::asn1::{self, BIT_STRING, OID, Reader, SEQUENCE};

/// The largest RSA modulus BoringSSL accepts
/// (`OPENSSL_RSA_MAX_MODULUS_BITS`).
const RSA_MAX_MODULUS_BITS: usize = 16384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hash {
    Md5,
    Sha1,
    Sha224,
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    pub fn hasher(self) -> Box<dyn DynDigest> {
        match self {
            Hash::Md5 => Box::new(md5::Md5::default()),
            Hash::Sha1 => Box::new(sha1::Sha1::default()),
            Hash::Sha224 => Box::new(sha2::Sha224::default()),
            Hash::Sha256 => Box::new(sha2::Sha256::default()),
            Hash::Sha384 => Box::new(sha2::Sha384::default()),
            Hash::Sha512 => Box::new(sha2::Sha512::default()),
        }
    }

    pub fn digest(self, data: &[u8]) -> Vec<u8> {
        let mut h = self.hasher();
        h.update(data);
        h.finalize().into_vec()
    }

    /// A `MessageDigest` name (`SHA-256`, and `SHA1` as the JAR manifest
    /// spells it).
    pub fn by_name(name: &str) -> Option<Hash> {
        Some(match name.to_ascii_uppercase().as_str() {
            "MD5" => Hash::Md5,
            "SHA1" | "SHA-1" | "SHA" => Hash::Sha1,
            "SHA-224" | "SHA224" => Hash::Sha224,
            "SHA-256" | "SHA256" => Hash::Sha256,
            "SHA-384" | "SHA384" => Hash::Sha384,
            "SHA-512" | "SHA512" => Hash::Sha512,
            _ => return None,
        })
    }

    /// A digest algorithm's object identifier (`AlgorithmId`'s names).
    pub fn by_oid(oid: &[u8]) -> Option<Hash> {
        const NIST: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02];
        Some(match oid {
            [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x02, 0x05] => Hash::Md5,
            [0x2b, 0x0e, 0x03, 0x02, 0x1a] => Hash::Sha1,
            [n @ .., last] if n == NIST => match last {
                1 => Hash::Sha256,
                2 => Hash::Sha384,
                3 => Hash::Sha512,
                4 => Hash::Sha224,
                _ => return None,
            },
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Key {
    Rsa,
    Ec,
    Dsa,
}

/// A signature algorithm: the key's type, the digest, and for RSA whether
/// it is PSS (with MGF1 over the same digest and a salt of its size).
#[derive(Clone, Copy, Debug)]
pub(super) struct Algorithm {
    pub key: Key,
    pub hash: Hash,
    pub pss: bool,
}

/// The type of the key `spki` holds.
pub(super) fn key_type(spki: &[u8]) -> Result<Key, String> {
    let mut s = Reader::new(Reader::new(spki).expect(SEQUENCE)?.value);
    let mut alg = Reader::new(s.expect(SEQUENCE)?.value);
    Ok(match alg.expect(OID)?.value {
        asn1::RSA_ENCRYPTION => Key::Rsa,
        asn1::EC_PUBLIC_KEY => Key::Ec,
        asn1::DSA => Key::Dsa,
        _ => return Err("unsupported public key algorithm".into()),
    })
}

/// Verifies `signature` over `data` with the key `spki`.
pub(super) fn verify(
    alg: Algorithm,
    spki: &[u8],
    data: &[u8],
    signature: &[u8],
) -> Result<(), String> {
    if key_type(spki)? != alg.key {
        return Err(format!("{alg:?} cannot use this key"));
    }
    let digest = alg.hash.digest(data);
    let ok = match alg.key {
        Key::Rsa => {
            let key = rsa_key(spki)?;
            match (alg.pss, alg.hash) {
                (true, Hash::Sha256) => key.verify(Pss::new::<sha2::Sha256>(), &digest, signature),
                (true, Hash::Sha512) => key.verify(Pss::new::<sha2::Sha512>(), &digest, signature),
                (true, h) => return Err(format!("PSS with {h:?} is not supported")),
                (false, h) => key.verify(pkcs1v15(h), &digest, signature),
            }
            .is_ok()
        }
        Key::Ec => ec_verify(spki, &digest, signature)?,
        Key::Dsa => {
            let key = dsa::VerifyingKey::from_public_key_der(spki).map_err(|e| e.to_string())?;
            let sig = dsa::Signature::try_from(signature).map_err(|e| e.to_string())?;
            key.verify_prehash(&digest, &sig).is_ok()
        }
    };
    if ok {
        Ok(())
    } else {
        Err("signature did not verify".into())
    }
}

/// An RSA key of up to BoringSSL's size.
fn rsa_key(spki: &[u8]) -> Result<RsaPublicKey, String> {
    let mut s = Reader::new(Reader::new(spki).expect(SEQUENCE)?.value);
    s.expect(SEQUENCE)?;
    let bits = s.expect(BIT_STRING)?.value;
    let key = rsa::pkcs1::RsaPublicKey::from_der(bits.get(1..).ok_or("empty key")?)
        .map_err(|e| e.to_string())?;
    RsaPublicKey::new_with_max_size(
        BigUint::from_bytes_be(key.modulus.as_bytes()),
        BigUint::from_bytes_be(key.public_exponent.as_bytes()),
        RSA_MAX_MODULUS_BITS,
    )
    .map_err(|e| e.to_string())
}

fn pkcs1v15(h: Hash) -> Pkcs1v15Sign {
    match h {
        Hash::Md5 => Pkcs1v15Sign::new::<md5::Md5>(),
        Hash::Sha1 => Pkcs1v15Sign::new::<sha1::Sha1>(),
        Hash::Sha224 => Pkcs1v15Sign::new::<sha2::Sha224>(),
        Hash::Sha256 => Pkcs1v15Sign::new::<sha2::Sha256>(),
        Hash::Sha384 => Pkcs1v15Sign::new::<sha2::Sha384>(),
        Hash::Sha512 => Pkcs1v15Sign::new::<sha2::Sha512>(),
    }
}

/// ECDSA over a named NIST curve. A digest shorter than the field is
/// taken as the number it is, a longer one is truncated (`ECDSA_verify`).
fn ec_verify(spki: &[u8], digest: &[u8], signature: &[u8]) -> Result<bool, String> {
    let mut s = Reader::new(Reader::new(spki).expect(SEQUENCE)?.value);
    let mut alg = Reader::new(s.expect(SEQUENCE)?.value);
    alg.expect(OID)?;
    let curve = alg
        .expect(OID)
        .map_err(|_| "EC key without a named curve")?
        .value;
    let point = s.expect(BIT_STRING)?.value.get(1..).ok_or("empty key")?;
    fn padded(digest: &[u8], size: usize) -> Vec<u8> {
        let mut v = vec![0; size.saturating_sub(digest.len())];
        v.extend_from_slice(digest);
        v
    }
    macro_rules! verify {
        ($c:ident, $size:expr) => {{
            let key = $c::ecdsa::VerifyingKey::from_sec1_bytes(point).map_err(|e| e.to_string())?;
            let sig = $c::ecdsa::Signature::from_der(signature).map_err(|e| e.to_string())?;
            key.verify_prehash(&padded(digest, $size), &sig).is_ok()
        }};
    }
    Ok(match curve {
        [0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07] => verify!(p256, 32),
        [0x2b, 0x81, 0x04, 0x00, 0x22] => verify!(p384, 48),
        [0x2b, 0x81, 0x04, 0x00, 0x23] => verify!(p521, 66),
        _ => return Err("unsupported curve".into()),
    })
}
