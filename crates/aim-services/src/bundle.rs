//! `Bundle` in its parcel form (`BaseBundle.writeToParcelInner`,
//! `Parcel.writeArrayMapInternal` and `writeValue` at the pinned tag), for
//! the few values native services exchange with the original ones: strings,
//! ints and booleans; a length-prefixed value (a parcelable, a map, a
//! serializable) is kept as its bytes for the caller to read.

use std::collections::HashMap;

use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader, Result};

/// `BaseBundle.BUNDLE_MAGIC` and `BUNDLE_MAGIC_NATIVE`.
pub const MAGIC: i32 = 0x4C44_4E42;
pub const MAGIC_NATIVE: i32 = 0x4C44_4E44;

/// `Parcel.VAL_*`.
const VAL_NULL: i32 = -1;
const VAL_STRING: i32 = 0;
const VAL_INTEGER: i32 = 1;
const VAL_MAP: i32 = 2;
const VAL_PARCELABLE: i32 = 4;
const VAL_LONG: i32 = 6;
const VAL_BOOLEAN: i32 = 9;
const VAL_LIST: i32 = 11;
const VAL_SPARSEARRAY: i32 = 12;
const VAL_PARCELABLEARRAY: i32 = 16;
const VAL_OBJECTARRAY: i32 = 17;
const VAL_SERIALIZABLE: i32 = 21;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    String(Option<String>),
    Int(i32),
    Long(i64),
    Bool(bool),
    /// A length-prefixed value (`Parcel.isLengthPrefixed`): its type and
    /// where its bytes are in the parcel read.
    Lazy {
        kind: i32,
        start: usize,
        len: usize,
    },
}

impl Value {
    fn write(&self, p: &mut Parcel) {
        match self {
            Value::Null => p.write_i32(VAL_NULL),
            Value::String(s) => {
                p.write_i32(VAL_STRING);
                p.write_string16(s.as_deref());
            }
            Value::Int(v) => {
                p.write_i32(VAL_INTEGER);
                p.write_i32(*v);
            }
            Value::Long(v) => {
                p.write_i32(VAL_LONG);
                p.write_i64(*v);
            }
            Value::Bool(v) => {
                p.write_i32(VAL_BOOLEAN);
                p.write_bool(*v);
            }
            Value::Lazy { .. } => unreachable!("lazy values are only read"),
        }
    }
}

/// `writeBundle` of a bundle with `entries`, in their order.
pub fn write(p: &mut Parcel, entries: &[(&str, Value)]) {
    if entries.is_empty() {
        return p.write_i32(0);
    }
    let length_at = p.position();
    p.write_i32(-1);
    p.write_i32(MAGIC);
    let start = p.position();
    p.write_i32(entries.len() as i32);
    for (key, value) in entries {
        p.write_string16(Some(key));
        value.write(p);
    }
    let length = (p.position() - start) as i32;
    p.set_i32_at(length_at, length);
    p.write_bool(false); // has an intent
}

/// `readBundle`: its entries, or none for a null bundle.
pub fn read(r: &mut Reader<'_>) -> Result<Option<HashMap<String, Value>>> {
    let length = r.read_i32()?;
    if length < 0 {
        return Ok(None);
    }
    let mut entries = HashMap::new();
    if length == 0 {
        return Ok(Some(entries));
    }
    if !matches!(r.read_i32()?, MAGIC | MAGIC_NATIVE) {
        return Err(BAD_VALUE);
    }
    let end = r.position() + length as usize;
    for _ in 0..r.read_i32()?.max(0) {
        let key = r.read_string16()?.ok_or(BAD_VALUE)?;
        let value = match r.read_i32()? {
            VAL_NULL => Value::Null,
            VAL_STRING => Value::String(r.read_string16()?),
            VAL_INTEGER => Value::Int(r.read_i32()?),
            VAL_LONG => Value::Long(r.read_i64()?),
            VAL_BOOLEAN => Value::Bool(r.read_bool()?),
            kind @ (VAL_MAP | VAL_PARCELABLE | VAL_LIST | VAL_SPARSEARRAY | VAL_PARCELABLEARRAY
            | VAL_OBJECTARRAY | VAL_SERIALIZABLE) => {
                let len = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
                let start = r.position();
                r.skip(len)?;
                Value::Lazy { kind, start, len }
            }
            _ => return Err(BAD_VALUE),
        };
        entries.insert(key, value);
    }
    if r.position() != end {
        return Err(BAD_VALUE);
    }
    r.read_bool()?; // has an intent
    Ok(Some(entries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let entries = [
            ("value", Value::String(Some("1".into()))),
            ("_user", Value::Int(10)),
            ("_track_generation", Value::Null),
            ("flag", Value::Bool(true)),
        ];
        let mut p = Parcel::new();
        write(&mut p, &entries);
        let mut r = Reader::new(p.data(), &[]);
        let read = read(&mut r).unwrap().unwrap();
        assert_eq!(r.remaining(), 0);
        for (key, value) in entries {
            assert_eq!(read[key], value);
        }
    }

    #[test]
    fn keeps_lazy_values_as_bytes() {
        let mut p = Parcel::new();
        p.write_i32(0); // length, below
        p.write_i32(MAGIC);
        p.write_i32(1);
        p.write_string16(Some("value"));
        p.write_i32(VAL_SERIALIZABLE);
        p.write_i32(8);
        p.write_i64(7);
        let length = (p.position() - 8) as i32;
        p.set_i32_at(0, length);
        p.write_bool(false);
        let mut r = Reader::new(p.data(), &[]);
        let read = read(&mut r).unwrap().unwrap();
        let Value::Lazy { kind, start, len } = read["value"] else {
            panic!("not lazy");
        };
        assert_eq!((kind, len), (VAL_SERIALIZABLE, 8));
        r.set_position(start);
        assert_eq!(r.read_i64(), Ok(7));
    }
}
