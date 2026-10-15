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

impl aim_service_aidl::ReadParcelable for Bundle {
    fn read_from(reader: &mut aim_binder_host::parcel::Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        read_bundle(reader, 0)
    }
}
fn read_bundle(reader: &mut aim_binder_host::parcel::Reader<'_>, depth: usize) -> aim_binder_host::parcel::Result<Bundle> {
    use aim_binder_host::parcel::BAD_VALUE;

    let bytes = reader.read_i32()?;
    if bytes == 0 { return Ok(Bundle::default()); }
    if bytes < 0 || !matches!(reader.read_i32()?, crate::bundle::MAGIC | crate::bundle::MAGIC_NATIVE) { return Err(BAD_VALUE); }
    let end = reader.position().checked_add(bytes as usize).ok_or(BAD_VALUE)?;
    if bytes as usize > reader.remaining() { return Err(BAD_VALUE); }
    let count = reader.read_i32()?;
    if count < 0 || count as usize > bytes as usize / 8 { return Err(BAD_VALUE); }
    let mut entries = Vec::new();
    for _ in 0..count {
        let key = reader.read_string16()?;
        let value = read_value(reader, depth + 1)?;
        if reader.position() > end { return Err(BAD_VALUE); }
        if let Some((_, previous)) = entries.iter_mut().find(|(name, _)| *name == key) { *previous = value; }
        else { entries.push((key, value)); }
    }
    if reader.position() != end { return Err(BAD_VALUE); }
    reader.read_bool()?; // BaseBundle's hasIntent footer; no intent graph is accepted.
    entries.sort_by_key(|(key, _)| hash(key.as_deref()));
    Ok(Bundle { entries })
}
fn read_value(reader: &mut aim_binder_host::parcel::Reader<'_>, depth: usize) -> aim_binder_host::parcel::Result<Value> {
    use aim_binder_host::parcel::BAD_VALUE;
    let kind = reader.read_i32()?;
    Ok(match kind {
        -1 => Value::Null,
        0 => reader.read_string16()?.map(Value::String).unwrap_or(Value::Null),
        1 => Value::Int(reader.read_i32()?),
        6 => Value::Long(reader.read_i64()?),
        8 => Value::Double(super::Double::new(f64::from_bits(reader.read_i64()? as u64))),
        9 => Value::Bool(reader.read_bool()?),
        25 => Value::Bundle(read_bundle(reader, depth)?),
        14 | 18 | 19 | 23 | 28 => {
            let count = reader.read_i32()?;
            if count == -1 { return Ok(Value::Null); }
            if count < 0 || count as usize > reader.remaining() / 4 { return Err(BAD_VALUE); }
            match kind {
                14 => Value::Strings((0..count).map(|_| reader.read_string16()).collect::<Result<_, _>>()?),
                18 => Value::Ints((0..count).map(|_| reader.read_i32()).collect::<Result<_, _>>()?),
                19 => Value::Longs((0..count).map(|_| reader.read_i64()).collect::<Result<_, _>>()?),
                23 => Value::Bools((0..count).map(|_| reader.read_bool()).collect::<Result<_, _>>()?),
                28 => Value::Doubles((0..count).map(|_| reader.read_i64().map(|bits| super::Double::new(f64::from_bits(bits as u64)))).collect::<Result<_, _>>()?),
                _ => unreachable!(),
            }
        }
        _ => return Err(BAD_VALUE),
    })
}
