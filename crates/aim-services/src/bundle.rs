//! `Bundle` in its parcel form (`BaseBundle.writeToParcelInner`,
//! `Parcel.writeArrayMapInternal` and `writeValue` at the pinned tag), for
//! the few values native services exchange with the original ones: strings,
//! string arrays and lists, ints, int and long arrays and booleans, a
//! parcelable or a serializable written by the caller and a
//! `MemoryIntArray`; a length-prefixed value read (a parcelable, a map, a
//! serializable) is kept as its bytes for the caller to read.

use std::collections::HashMap;
use std::sync::Arc;

use aim_binder_host::parcel::{BAD_VALUE, Parcel, Reader, Result};

/// `BaseBundle.BUNDLE_MAGIC` and `BUNDLE_MAGIC_NATIVE`.
pub const MAGIC: i32 = 0x4C44_4E42;
pub const MAGIC_NATIVE: i32 = 0x4C44_4E44;

/// `Parcel.VAL_*`.
const VAL_NULL: i32 = -1;
const VAL_STRING: i32 = 0;
pub(crate) const VAL_INTEGER: i32 = 1;
const VAL_MAP: i32 = 2;
const VAL_BUNDLE: i32 = 3;
pub(crate) const VAL_PARCELABLE: i32 = 4;
const VAL_LONG: i32 = 6;
const VAL_FLOAT: i32 = 7;
const VAL_BOOLEAN: i32 = 9;
pub(crate) const VAL_LIST: i32 = 11;
const VAL_SPARSEARRAY: i32 = 12;
const VAL_STRINGARRAY: i32 = 14;
const VAL_PARCELABLEARRAY: i32 = 16;
const VAL_OBJECTARRAY: i32 = 17;
const VAL_INTARRAY: i32 = 18;
const VAL_LONGARRAY: i32 = 19;
const VAL_SERIALIZABLE: i32 = 21;
const VAL_PERSISTABLEBUNDLE: i32 = 25;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    String(Option<String>),
    Int(i32),
    Long(i64),
    Float(f32),
    Bool(bool),
    /// A `String[]` (`writeStringArray`).
    StringArray(Vec<Option<String>>),
    /// An `int[]` (`writeIntArray`).
    IntArray(Vec<i32>),
    /// A `long[]` (`writeLongArray`).
    LongArray(Vec<i64>),
    /// A parcelable of the class `class`, written to `data`
    /// (`writeParcelable`, length-prefixed).
    Parcelable {
        class: String,
        data: Vec<u8>,
    },
    /// A `Parcelable[]` (`putParcelableArray`, `writeParcelableArray`),
    /// length-prefixed: each item's class and fields.
    ParcelableArray(Vec<(String, Vec<u8>)>),
    /// An `ArrayList<String>` (`putStringArrayList`, `writeList`),
    /// length-prefixed as `Parcel.writeValue` writes lists.
    StringList(Vec<String>),
    /// An `ArrayList<Integer>` (`putIntegerArrayList`, `writeList`),
    /// length-prefixed as `Parcel.writeValue` writes lists.
    IntList(Vec<i32>),
    /// A `Map<String, String>` (`writeMap`), length-prefixed as
    /// `Parcel.writeValue` writes maps.
    StringMap(Vec<(String, Option<String>)>),
    /// A `Serializable` that is neither a `Map` nor a `List`
    /// (`writeSerializable`), length-prefixed: its class's name and its
    /// Object Serialization stream.
    Serializable {
        class: String,
        data: Vec<u8>,
    },
    /// A `PersistableBundle` (`writePersistableBundle`) of entries.
    PersistableBundle(Vec<(String, Value)>),
    /// A `Bundle` (`putBundle`, `writeBundle`) of entries.
    Bundle(Vec<(String, Value)>),
    /// A `MemoryIntArray` (`writeParcelable`): its shared memory, as the
    /// `ParcelFileDescriptor` its `writeToParcel` writes.
    MemoryIntArray(SharedFile),
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
            Value::Float(v) => {
                p.write_i32(VAL_FLOAT);
                p.write_f32(*v);
            }
            Value::Bool(v) => {
                p.write_i32(VAL_BOOLEAN);
                p.write_bool(*v);
            }
            Value::StringArray(items) => {
                p.write_i32(VAL_STRINGARRAY);
                p.write_i32(items.len() as i32);
                for item in items {
                    p.write_string16(item.as_deref());
                }
            }
            Value::IntArray(items) => {
                p.write_i32(VAL_INTARRAY);
                p.write_i32(items.len() as i32);
                for item in items {
                    p.write_i32(*item);
                }
            }
            Value::LongArray(items) => {
                p.write_i32(VAL_LONGARRAY);
                p.write_i32(items.len() as i32);
                for item in items {
                    p.write_i64(*item);
                }
            }
            Value::Parcelable { class, data } => length_prefixed(p, VAL_PARCELABLE, |p| {
                p.write_string16(Some(class));
                p.write_raw(data, &[]);
            }),
            Value::ParcelableArray(items) => length_prefixed(p, VAL_PARCELABLEARRAY, |p| {
                p.write_i32(items.len() as i32);
                for (class, data) in items {
                    p.write_string16(Some(class));
                    p.write_raw(data, &[]);
                }
            }),
            Value::StringList(items) => length_prefixed(p, VAL_LIST, |p| {
                p.write_i32(items.len() as i32);
                for item in items {
                    p.write_i32(VAL_STRING);
                    p.write_string16(Some(item));
                }
            }),
            Value::IntList(items) => length_prefixed(p, VAL_LIST, |p| {
                p.write_i32(items.len() as i32);
                for item in items {
                    p.write_i32(VAL_INTEGER);
                    p.write_i32(*item);
                }
            }),
            Value::StringMap(entries) => length_prefixed(p, VAL_MAP, |p| {
                p.write_i32(entries.len() as i32);
                for (key, value) in entries {
                    p.write_i32(VAL_STRING);
                    p.write_string16(Some(key));
                    match value {
                        Some(value) => {
                            p.write_i32(VAL_STRING);
                            p.write_string16(Some(value));
                        }
                        None => p.write_i32(VAL_NULL),
                    }
                }
            }),
            Value::MemoryIntArray(file) => length_prefixed(p, VAL_PARCELABLE, |p| {
                p.write_string16(Some("android.util.MemoryIntArray"));
                p.write_string16(Some("android.os.ParcelFileDescriptor"));
                p.write_i32(0); // no comm fd
                p.write_file(file.0.clone());
            }),
            Value::Serializable { class, data } => length_prefixed(p, VAL_SERIALIZABLE, |p| {
                p.write_string16(Some(class));
                aim_service_aidl::write_byte_array(p, Some(data));
            }),
            Value::PersistableBundle(entries) | Value::Bundle(entries) => {
                p.write_i32(if matches!(self, Value::Bundle(_)) {
                    VAL_BUNDLE
                } else {
                    VAL_PERSISTABLEBUNDLE
                });
                let entries: Vec<(&str, Value)> = entries
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.clone()))
                    .collect();
                write(p, &entries);
            }
            Value::Lazy { .. } => unreachable!("lazy values are only read"),
        }
    }
}

/// `writeValue` of a length-prefixed value of `kind`.
fn length_prefixed(p: &mut Parcel, kind: i32, value: impl FnOnce(&mut Parcel)) {
    p.write_i32(kind);
    let length_at = p.position();
    p.write_i32(-1);
    let start = p.position();
    value(p);
    p.set_i32_at(length_at, (p.position() - start) as i32);
}

/// A file a value carries: values are equal when they carry the same one.
#[derive(Clone)]
pub struct SharedFile(pub aim_binder_driver::File);

impl PartialEq for SharedFile {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::addr_eq(Arc::as_ptr(&self.0), Arc::as_ptr(&other.0))
    }
}

impl std::fmt::Debug for SharedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SharedFile({:p})", Arc::as_ptr(&self.0))
    }
}
/// `writeBundle` of a bundle with `entries`, in the order its `ArrayMap`
/// keeps them: by key hash, then as put.
pub fn write(p: &mut Parcel, entries: &[(&str, Value)]) {
    if entries.is_empty() {
        return p.write_i32(0);
    }
    let length_at = p.position();
    p.write_i32(-1);
    p.write_i32(MAGIC);
    let start = p.position();
    p.write_i32(entries.len() as i32);
    for (key, value) in crate::package::info::array_order(entries.iter().collect(), |(k, _)| k) {
        p.write_string16(Some(key));
        value.write(p);
    }
    let length = (p.position() - start) as i32;
    p.set_i32_at(length_at, length);
    // Whether it holds an intent (`Bundle.putParcelable` of one).
    p.write_bool(entries.iter().any(
        |(_, v)| matches!(v, Value::Parcelable { class, .. } if class == "android.content.Intent"),
    ));
}

/// A `Map<String, String>` value of a bundle read from `parcel`
/// (`readHashMap`): its entries in the order written, or none for a value
/// that is no map. A key or value that is not a string is a bad value.
pub fn string_map(
    value: &Value,
    parcel: &Reader<'_>,
) -> Result<Option<Vec<(String, Option<String>)>>> {
    let Value::Lazy {
        kind: VAL_MAP,
        start,
        len,
    } = *value
    else {
        return Ok(None);
    };
    let mut r = parcel.clone();
    r.set_position(start);
    let mut entries = Vec::new();
    for _ in 0..r.read_i32()?.max(0) {
        if r.read_i32()? != VAL_STRING {
            return Err(BAD_VALUE);
        }
        let key = r.read_string16()?.ok_or(BAD_VALUE)?;
        let value = match r.read_i32()? {
            VAL_STRING => r.read_string16()?,
            VAL_NULL => None,
            _ => return Err(BAD_VALUE),
        };
        entries.push((key, value));
    }
    if r.position() != start + len {
        return Err(BAD_VALUE);
    }
    Ok(Some(entries))
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
            VAL_FLOAT => Value::Float(r.read_f32()?),
            VAL_BOOLEAN => Value::Bool(r.read_bool()?),
            VAL_STRINGARRAY => {
                let n = r.read_i32()?;
                Value::StringArray(
                    (0..n.max(0))
                        .map(|_| r.read_string16())
                        .collect::<Result<_>>()?,
                )
            }
            VAL_INTARRAY => {
                let n = r.read_i32()?;
                Value::IntArray((0..n.max(0)).map(|_| r.read_i32()).collect::<Result<_>>()?)
            }
            VAL_LONGARRAY => {
                let n = r.read_i32()?;
                Value::LongArray((0..n.max(0)).map(|_| r.read_i64()).collect::<Result<_>>()?)
            }
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
            ("ratio", Value::Float(5.5)),
            ("names", Value::StringArray(vec![Some("a".into()), None])),
            ("uids", Value::IntArray(vec![10100, 10101])),
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

    /// `ArrayMap.append` logs and re-sorts keys a parcel holds out of hash
    /// order, whatever order they were put in.
    #[test]
    fn keys_are_written_in_array_map_order() {
        let mut p = Parcel::new();
        write(
            &mut p,
            &[
                ("android.os.extra.CYCLE_COUNT", Value::Int(2)),
                ("seq", Value::Int(1)),
            ],
        );
        let mut r = Reader::new(p.data(), &[]);
        r.read_i32().unwrap();
        r.read_i32().unwrap();
        assert_eq!(r.read_i32().unwrap(), 2);
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("seq"));
        r.read_i32().unwrap();
        r.read_i32().unwrap();
        assert_eq!(
            r.read_string16().unwrap().as_deref(),
            Some("android.os.extra.CYCLE_COUNT")
        );
    }

    /// `putStringArrayList`: a length-prefixed list of string values.
    #[test]
    fn writes_string_lists() {
        let mut p = Parcel::new();
        write(&mut p, &[("l", Value::StringList(vec!["a=1".into()]))]);
        let mut r = Reader::new(p.data(), &[]);
        let Value::Lazy { kind, start, len } = read(&mut r).unwrap().unwrap()["l"] else {
            panic!("not lazy");
        };
        assert_eq!(kind, VAL_LIST);
        r.set_position(start);
        assert_eq!((r.read_i32(), r.read_i32()), (Ok(1), Ok(VAL_STRING)));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("a=1"));
        assert_eq!(r.position(), start + len);
    }

    /// A `HashMap<String, String>` put as a serializable is written as a
    /// map (`getValueType` finds a `Map` first) and read back as one.
    #[test]
    fn string_maps_read_as_written() {
        let entries = vec![
            ("a/b".to_string(), Some("1".to_string())),
            ("a/c".to_string(), None),
        ];
        let mut p = Parcel::new();
        write(
            &mut p,
            &[
                ("_flags", Value::StringMap(entries.clone())),
                ("v", Value::Int(1)),
            ],
        );
        let parcel = Reader::new(p.data(), &[]);
        let read = read(&mut parcel.clone()).unwrap().unwrap();
        assert_eq!(string_map(&read["_flags"], &parcel), Ok(Some(entries)));
        assert_eq!(string_map(&read["v"], &parcel), Ok(None));
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
