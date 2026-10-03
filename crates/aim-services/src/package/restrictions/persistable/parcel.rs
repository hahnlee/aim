//! Original `BaseBundle.writeToParcelInner` / `Parcel.writeValue` ABI.
use super::{Bundle, Value};
use aim_binder_host::parcel::Parcel;
use std::collections::BTreeSet;

impl Bundle {
    /// Owned bytes accepted by the original PersistableBundle CREATOR. A
    /// malformed caller-built graph fails before any bytes are returned.
    pub fn parcel(&self) -> Result<Parcel, String> {
        let mut parcel = Parcel::new();
        write_bundle(&mut parcel, self)?;
        Ok(parcel)
    }
}

fn length(p: &mut Parcel, len: usize) -> Result<(), String> {
    p.write_i32(i32::try_from(len).map_err(|_| "persistable count exceeds Parcel range")?);
    Ok(())
}

fn string(p: &mut Parcel, value: Option<&str>) -> Result<(), String> {
    if value.is_some_and(|s| s.encode_utf16().count() > i32::MAX as usize) {
        return Err("persistable string exceeds Parcel range".into());
    }
    p.write_string16(value);
    Ok(())
}

fn write_bundle(p: &mut Parcel, bundle: &Bundle) -> Result<(), String> {
    if bundle.entries.is_empty() {
        p.write_i32(0);
        return Ok(());
    }
    let mut keys = BTreeSet::new();
    let mut entries = Vec::new();
    for (key, value) in &bundle.entries {
        if !keys.insert(key.as_deref()) {
            return Err("duplicate persistable key".into());
        }
        entries.push((key, value));
    }
    // ArrayMap's signed String.hashCode order, with stable insertion order
    // for hash collisions, including null and the empty string (hash zero).
    entries.sort_by_key(|(key, _)| hash(key.as_deref()));
    let length_at = p.position();
    p.write_i32(-1);
    p.write_i32(crate::bundle::MAGIC);
    let start = p.position();
    length(p, entries.len())?;
    for (key, value) in entries {
        string(p, key.as_deref())?;
        write_value(p, value)?;
    }
    let bytes = i32::try_from(p.position() - start)
        .map_err(|_| "persistable payload exceeds Parcel range")?;
    p.set_i32_at(length_at, bytes);
    p.write_bool(false); // PersistableBundle cannot contain an Intent.
    Ok(())
}

fn hash(key: Option<&str>) -> i32 {
    key.map_or(0, |s| {
        s.encode_utf16()
            .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)))
    })
}

fn array<T>(
    p: &mut Parcel,
    values: &[T],
    write: impl Fn(&mut Parcel, &T) -> Result<(), String>,
) -> Result<(), String> {
    length(p, values.len())?;
    for value in values {
        write(p, value)?;
    }
    Ok(())
}

fn write_value(p: &mut Parcel, value: &Value) -> Result<(), String> {
    let kind = match value {
        Value::Null => -1,
        Value::String(_) => 0,
        Value::Int(_) => 1,
        Value::Long(_) => 6,
        Value::Double(_) => 8,
        Value::Bool(_) => 9,
        Value::Strings(_) => 14,
        Value::Ints(_) => 18,
        Value::Longs(_) => 19,
        Value::Bools(_) => 23,
        Value::Bundle(_) => 25,
        Value::Doubles(_) => 28,
    };
    p.write_i32(kind);
    match value {
        Value::Null => {}
        Value::String(value) => string(p, Some(value))?,
        Value::Int(value) => p.write_i32(*value),
        Value::Long(value) => p.write_i64(*value),
        Value::Double(value) => p.write_i64(value.bits() as i64),
        Value::Bool(value) => p.write_bool(*value),
        Value::Ints(values) => array(p, values, |p, v| {
            p.write_i32(*v);
            Ok(())
        })?,
        Value::Longs(values) => array(p, values, |p, v| {
            p.write_i64(*v);
            Ok(())
        })?,
        Value::Doubles(values) => array(p, values, |p, v| {
            p.write_i64(v.bits() as i64);
            Ok(())
        })?,
        Value::Bools(values) => array(p, values, |p, v| {
            p.write_bool(*v);
            Ok(())
        })?,
        Value::Strings(values) => array(p, values, |p, v| string(p, v.as_deref()))?,
        Value::Bundle(value) => write_bundle(p, value)?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_duplicate_keys_and_retains_java_hash_collision_order() {
        let duplicated = Bundle {
            entries: vec![(None, Value::Int(1)), (None, Value::Int(2))],
        };
        assert!(duplicated.parcel().unwrap_err().contains("duplicate"));
        assert_eq!(hash(Some("Aa")), hash(Some("BB")));
        assert_eq!(hash(None), hash(Some("")));
        assert_eq!(Bundle::default().parcel().unwrap().data(), &[0, 0, 0, 0]);
    }
}
