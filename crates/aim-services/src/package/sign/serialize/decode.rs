//! Decode only the pinned public-key serialization schemas. Descriptors and
//! handles are checked against the same schemas as the native writer; no Java
//! objects are instantiated and trailing or foreign data is rejected.
use super::*;

struct Input<'a> {
    remaining: &'a [u8],
    schema: Stream,
}

impl<'a> Input<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], String> {
        if length > self.remaining.len() {
            return Err("truncated serialized public key".into());
        }
        let (value, rest) = self.remaining.split_at(length);
        self.remaining = rest;
        Ok(value)
    }

    fn expect(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.take(bytes.len())? != bytes {
            return Err("invalid serialized public-key schema".into());
        }
        Ok(())
    }

    fn schema(&mut self) -> Result<(), String> {
        let bytes = std::mem::take(&mut self.schema.bytes);
        self.expect(&bytes)
    }

    fn object(&mut self, class: &'static Class) -> Result<(), String> {
        self.schema.object(class);
        self.schema()
    }

    fn array(&mut self) -> Result<Vec<u8>, String> {
        self.schema.bytes.push(0x75);
        self.schema.class(&BYTES);
        self.schema.handle();
        self.schema()?;
        let length = i32::from_be_bytes(self.take(4)?.try_into().unwrap());
        let length = usize::try_from(length).map_err(|_| "negative public-key array length")?;
        Ok(self.take(length)?.to_vec())
    }

    fn integer(&mut self) -> Result<Vec<u8>, String> {
        self.object(&INTEGER_CLASS)?;
        self.expect(&[0; 16])?;
        let signum = i32::from_be_bytes(self.take(4)?.try_into().unwrap());
        let magnitude = self.array()?;
        if signum != i32::from(!magnitude.is_empty()) || magnitude.first() == Some(&0) {
            return Err("invalid serialized public-key integer".into());
        }
        self.expect(&[0x78])?;
        let mut value = Vec::new();
        if magnitude.is_empty() || magnitude[0] & 0x80 != 0 {
            value.push(0);
        }
        value.extend_from_slice(&magnitude);
        Ok(value)
    }
}

/// Decode a pinned Android public-key stream into canonical X.509 SPKI.
/// Foreign schemas and noncanonical streams fail before any owner mutation.
pub fn public_key(value: &Serialized) -> Result<Vec<u8>, String> {
    let mut input = Input {
        remaining: &value.bytes,
        schema: Stream::new(),
    };
    input.schema()?;
    let encoded = if value.class == EC.name {
        input.object(&EC)?;
        input.array()?
    } else if value.class == RSA.name {
        input.object(&RSA)?;
        let modulus = input.integer()?;
        let exponent = input.integer()?;
        let mut algorithm = der(OID, asn1::RSA_ENCRYPTION);
        algorithm.extend_from_slice(&der(0x05, &[]));
        let mut key = encoded_integer(&modulus)?;
        key.extend_from_slice(&encoded_integer(&exponent)?);
        spki(algorithm, der(SEQUENCE, &key))
    } else if value.class == DSA.name {
        input.object(&DSA)?;
        let y = input.integer()?;
        let mut parameters = Vec::new();
        for _ in 0..3 {
            parameters.extend_from_slice(&encoded_integer(&input.integer()?)?);
        }
        let mut algorithm = der(OID, asn1::DSA);
        algorithm.extend_from_slice(&der(SEQUENCE, &parameters));
        spki(algorithm, encoded_integer(&y)?)
    } else {
        return Err("unsupported serialized public-key class".into());
    };
    input.expect(&[0x78])?;
    if !input.remaining.is_empty() {
        return Err("noncanonical serialized public key".into());
    }
    let (serialized, _, canonical) = key(&encoded)?;
    if serialized != *value {
        return Err("noncanonical serialized public key".into());
    }
    Ok(canonical)
}

fn spki(algorithm: Vec<u8>, key: Vec<u8>) -> Vec<u8> {
    let mut encoded = der(SEQUENCE, &algorithm);
    let mut bits = vec![0];
    bits.extend_from_slice(&key);
    encoded.extend_from_slice(&der(BIT_STRING, &bits));
    der(SEQUENCE, &encoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::elliptic_curve::sec1::ToEncodedPoint;

    #[test]
    fn decodes_pinned_schemas_and_rejects_foreign_or_truncated_streams() {
        let rsa = spki(
            {
                let mut a = der(OID, asn1::RSA_ENCRYPTION);
                a.extend_from_slice(&der(5, &[]));
                a
            },
            der(
                SEQUENCE,
                &[der(INTEGER, &[0, 0x80, 1]), der(INTEGER, &[1, 0, 1])].concat(),
            ),
        );
        let dsa = spki(
            [
                der(OID, asn1::DSA),
                der(
                    SEQUENCE,
                    &[der(INTEGER, &[23]), der(INTEGER, &[11]), der(INTEGER, &[2])].concat(),
                ),
            ]
            .concat(),
            der(INTEGER, &[8]),
        );
        let secret = p256::SecretKey::from_slice(&[1; 32]).unwrap();
        let ec = spki(
            [
                der(OID, asn1::EC_PUBLIC_KEY),
                der(OID, &[0x2a, 0x86, 0x48, 0xce, 0x3d, 3, 1, 7]),
            ]
            .concat(),
            secret
                .public_key()
                .to_encoded_point(false)
                .as_bytes()
                .to_vec(),
        );
        for encoded in [rsa, dsa, ec] {
            let serialized = key(&encoded).unwrap().0;
            assert_eq!(public_key(&serialized).unwrap(), encoded);
            for length in 0..serialized.bytes.len() {
                let mut corrupt = serialized.clone();
                corrupt.bytes.truncate(length);
                assert!(public_key(&corrupt).is_err(), "length {length}");
            }
            let mut corrupt = serialized.clone();
            corrupt.bytes.push(0);
            assert!(public_key(&corrupt).is_err());
            corrupt = serialized.clone();
            corrupt.class = "java.lang.Runtime".into();
            assert!(public_key(&corrupt).is_err());
            corrupt = serialized;
            corrupt.bytes[4] = 0x71;
            assert!(public_key(&corrupt).is_err());
        }
    }
}
