//! URI-group Bundle input. Keep lazy values until permission/name/domain checks.
use super::uri_parcel::Group;
use aim_binder_host::parcel::{BAD_VALUE, Reader, Result};
use aim_service_aidl::ReadParcelable;

#[derive(Debug)]
pub enum GroupError {
    Parcel(i32),
    BadParcelable(String),
    Unavailable,
    Transport(i32),
}
impl From<i32> for GroupError {
    fn from(status: i32) -> Self {
        Self::Parcel(status)
    }
}
pub struct Bundle {
    bytes: Vec<u8>,
    objects: Vec<u64>,
}
pub struct Entry {
    pub key: Option<String>,
    pub kind: i32,
    pub bytes: Vec<u8>,
}
impl ReadParcelable for Bundle {
    fn read_from(reader: &mut Reader<'_>) -> Result<Self> {
        let start = reader.position();
        let length = reader.read_i32()?;
        if length > 0 {
            reader.read_i32()?;
            reader.skip(length as usize)?;
            reader.read_bool()?;
        }
        if length < 0 {
            return Err(BAD_VALUE);
        }
        let (bytes, objects) = reader.since(start);
        Ok(Self {
            bytes: bytes.to_vec(),
            objects,
        })
    }
}
impl Bundle {
    pub fn entries(&self) -> Result<Vec<Entry>> {
        let mut reader = Reader::new(&self.bytes, &self.objects);
        let length = reader.read_i32()?;
        if length == 0 {
            return Ok(vec![]);
        }
        if !matches!(
            reader.read_i32()?,
            crate::bundle::MAGIC | crate::bundle::MAGIC_NATIVE
        ) {
            return Err(BAD_VALUE);
        }
        let end = reader
            .position()
            .checked_add(length as usize)
            .ok_or(BAD_VALUE)?;
        let count = usize::try_from(reader.read_i32()?).map_err(|_| BAD_VALUE)?;
        if count > reader.remaining() / 8 {
            return Err(BAD_VALUE);
        }
        let mut entries: Vec<Entry> = Vec::new();
        for _ in 0..count {
            let key = reader.read_string16()?;
            let kind = reader.read_i32()?;
            let start = reader.position();
            skip_value(&mut reader, kind)?;
            let entry = Entry {
                key: key.clone(),
                kind,
                bytes: reader.since(start).0.to_vec(),
            };
            if let Some(at) = entries.iter().position(|old| old.key == key) {
                entries[at] = entry;
            } else {
                entries.push(entry);
            }
        }
        if reader.position() != end {
            return Err(BAD_VALUE);
        }
        reader.read_bool()?;
        entries.sort_by_key(|entry| {
            crate::package::info::java_hash(entry.key.as_deref().unwrap_or(""))
        });
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{Binder, Parcel};
    #[test]
    fn copied_bundle_rebases_binder_objects_and_retains_the_following_key() {
        let mut body = Parcel::new();
        body.write_i32(2);
        body.write_string16(Some("binder.example"));
        body.write_i32(15);
        body.write_binder(Some(Binder::Local(0x1234)));
        body.write_string16(Some("after.example"));
        body.write_i32(1);
        body.write_i32(42);
        let mut request = Parcel::new();
        request.write_i32(99);
        request.write_i32(1);
        request.write_i32(body.data().len() as i32);
        request.write_i32(crate::bundle::MAGIC);
        request.write_raw(body.data(), body.objects());
        request.write_bool(false);
        request.write_i32(88);
        let mut reader = Reader::new(request.data(), request.objects());
        reader.skip(8).unwrap();
        let bundle = Bundle::read_from(&mut reader).unwrap();
        assert_eq!(reader.read_i32().unwrap(), 88);
        assert_eq!(reader.remaining(), 0);
        let entries = bundle.entries().unwrap();
        assert_eq!(entries.len(), 2);
        for entry in entries {
            assert!(entry.groups().unwrap().is_none());
        }
    }
}
fn skip_value(reader: &mut Reader<'_>, kind: i32) -> Result<()> {
    match kind {
        -1 => (),
        0 => {
            reader.read_string16()?;
        }
        1 | 5 | 7 | 9 | 20 | 29 => {
            reader.read_i32()?;
        }
        6 | 8 | 26 | 27 => {
            reader.read_i64()?;
        }
        2 | 4 | 11 | 12 | 16 | 17 | 21 => {
            let length = usize::try_from(reader.read_i32()?).map_err(|_| BAD_VALUE)?;
            reader.skip(length)?;
        }
        13 | 19 | 18 | 23 | 28 | 30 | 31 | 32 => {
            let count = reader.read_i32()?;
            if count >= 0 {
                let width = if matches!(kind, 19 | 28) {
                    8
                } else if kind == 13 {
                    1
                } else {
                    4
                };
                reader.skip((count as usize).checked_mul(width).ok_or(BAD_VALUE)?)?;
            }
        }
        3 | 25 => {
            let length = reader.read_i32()?;
            if length > 0 {
                reader.read_i32()?;
                reader.skip(length as usize)?;
                reader.read_bool()?;
            }
        }
        10 => {
            crate::clip::char_sequence(reader)?;
        }
        24 => {
            let count = reader.read_i32()?;
            for _ in 0..count.max(0) {
                skip_value(reader, 10)?;
            }
        }
        15 => {
            reader.read_binder()?;
        }
        22 => {
            let count = reader.read_i32()?;
            if count >= 0 {
                reader.skip((count as usize).checked_mul(8).ok_or(BAD_VALUE)?)?;
            }
        }
        14 => {
            let count = reader.read_i32()?;
            for _ in 0..count.max(0) {
                reader.read_string16()?;
            }
        }
        _ => return Err(BAD_VALUE),
    }
    Ok(())
}
impl Entry {
    /// A wrong list/element type is the original typed getter's null result.
    pub fn groups(&self) -> Result<Option<Vec<Option<Group>>>> {
        self.groups_with_classes(None).map_err(|error| match error {
            GroupError::Parcel(status) => status,
            _ => BAD_VALUE,
        })
    }
    pub fn groups_with_classes(
        &self,
        classes: Option<&aim_android_image::linkage::Hierarchy>,
    ) -> std::result::Result<Option<Vec<Option<Group>>>, GroupError> {
        if matches!(self.kind, 4 | 21) {
            let mut outer = Reader::new(&self.bytes, &[]);
            let length =
                usize::try_from(outer.read_i32()?).map_err(|_| GroupError::Parcel(BAD_VALUE))?;
            outer.skip(length)?;
            let mut value = Reader::new(&self.bytes[4..4 + length], &[]);
            if self.kind == 21 {
                return match read_serializable(&mut value)? {
                    SerialValue::List {
                        count,
                        other: false,
                    } => Ok(Some((0..count).map(|_| None).collect())),
                    SerialValue::List { other: true, .. } => Err(GroupError::Transport(
                        aim_binder_host::parcel::UNKNOWN_TRANSACTION,
                    )),
                    _ => Ok(None),
                };
            } else if let Some(name) = value.read_string16()? {
                if class_matches(&name, "Ljava/util/ArrayList;", classes)? {
                    return Err(GroupError::Unavailable);
                }
            }
            return Ok(None);
        }
        if self.kind != 11 {
            return Ok(None);
        }
        let mut outer = Reader::new(&self.bytes, &[]);
        let length =
            usize::try_from(outer.read_i32()?).map_err(|_| GroupError::Parcel(BAD_VALUE))?;
        outer.skip(length)?;
        let mut reader = Reader::new(&self.bytes[4..4 + length], &[]);
        let count = reader.read_i32()?;
        if count < 0 {
            return Ok(None);
        }
        if count as usize > reader.remaining() / 4 {
            return Err(GroupError::Parcel(BAD_VALUE));
        }
        let mut groups = Vec::new();
        for _ in 0..count {
            let kind = reader.read_i32()?;
            if kind == -1 {
                groups.push(None);
                continue;
            }
            if kind == 21 {
                let length = usize::try_from(reader.read_i32()?)
                    .map_err(|_| GroupError::Parcel(BAD_VALUE))?;
                let start = reader.position();
                reader.skip(length)?;
                let bytes = reader.since(start).0;
                let mut value = Reader::new(&bytes[..length], &[]);
                match read_serializable(&mut value)? {
                    SerialValue::NullName => {groups.push(None); continue;}
                    _ => return Ok(None),
                }
            }
            if kind != 4 {
                return Ok(None);
            }
            let length =
                usize::try_from(reader.read_i32()?).map_err(|_| GroupError::Parcel(BAD_VALUE))?;
            let start = reader.position();
            reader.skip(length)?;
            let bytes = reader.since(start).0;
            let mut parcelable = Reader::new(&bytes[..length], &[]);
            match parcelable.read_string16()?.as_deref() {
                None => {
                    groups.push(None);
                    continue;
                }
                Some("android.content.UriRelativeFilterGroupParcel") => (),
                Some(name) => {
                    if class_matches(
                        name,
                        "Landroid/content/UriRelativeFilterGroupParcel;",
                        classes,
                    )? {
                        return Err(GroupError::Unavailable);
                    }
                    return Ok(None);
                }
            }
            groups.push(Some(Group::read_from(&mut parcelable)?));
        }
        Ok(Some(groups))
    }
}

fn class_matches(
    name: &str,
    required: &str,
    classes: Option<&aim_android_image::linkage::Hierarchy>,
) -> std::result::Result<bool, GroupError> {
    let classes = classes.ok_or(GroupError::Unavailable)?;
    let descriptor = if name.starts_with('[') {
        name.replace('.', "/")
    } else {
        format!("L{};", name.replace('.', "/"))
    };
    if name.contains('/') || !classes.contains(&descriptor) {
        return Err(GroupError::BadParcelable(format!(
            "ClassNotFoundException when unmarshalling: {name}"
        )));
    }
    if !classes.assignable(&descriptor, "Landroid/os/Parcelable;") {
        return Err(GroupError::BadParcelable(format!(
            "Parcelable protocol requires subclassing from Parcelable on class {name}"
        )));
    }
    Ok(classes.assignable(&descriptor, required))
}

enum SerialValue {
    NullName,
    Other,
    List { count: usize, other: bool },
}
fn read_serializable(reader: &mut Reader<'_>) -> std::result::Result<SerialValue, GroupError> {
    let Some(name) = reader.read_string16()? else {
        return Ok(SerialValue::NullName);
    };
    let bytes = aim_service_aidl::read_byte_array(reader)?.ok_or(GroupError::Unavailable)?;
    serial_stream(&bytes).map_err(|error| match error {
        SerialError::Io => GroupError::BadParcelable(format!(
            "Parcelable encountered IOException reading a Serializable object (name = {name})"
        )),
        SerialError::Unsupported => GroupError::Unavailable,
    })
}
#[derive(Debug)]
enum SerialError {
    Io,
    Unsupported,
}
struct SerialInput<'a> {
    bytes: &'a [u8],
}
impl<'a> SerialInput<'a> {
    fn take(&mut self, count: usize) -> std::result::Result<&'a [u8], SerialError> {
        let data = self.bytes.get(..count).ok_or(SerialError::Io)?;
        self.bytes = &self.bytes[count..];
        Ok(data)
    }
    fn string(&mut self, long: bool) -> std::result::Result<(), SerialError> {
        let count = if long {
            usize::try_from(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
                .map_err(|_| SerialError::Io)?
        } else {
            u16::from_be_bytes(self.take(2)?.try_into().unwrap()) as usize
        };
        let data = self.take(count)?;
        let mut at = 0;
        while at < data.len() {
            let first = data[at];
            at += 1;
            let following = match first {
                0..=0x7f => 0,
                0xc0..=0xdf => 1,
                0xe0..=0xef => 2,
                _ => return Err(SerialError::Io),
            };
            for _ in 0..following {
                if data.get(at).is_none_or(|value| value & 0xc0 != 0x80) {
                    return Err(SerialError::Io);
                }
                at += 1;
            }
        }
        Ok(())
    }
}
fn serial_stream(bytes: &[u8]) -> std::result::Result<SerialValue, SerialError> {
    let mut input = SerialInput { bytes };
    if input.take(4)? != [0xac, 0xed, 0, 5] {
        return Err(SerialError::Io);
    }
    match input.take(1)?[0] {
        0x74 => {
            input.string(false)?;
            Ok(SerialValue::Other)
        }
        0x7c => {
            input.string(true)?;
            Ok(SerialValue::Other)
        }
        0x73 => {
            // Pinned libcore ArrayList descriptor: serialVersionUID, size, writeObject.
            let schema = b"\x72\x00\x13java.util.ArrayList\x78\x81\xd2\x1d\x99\xc7\x61\x9d\x03\x00\x01I\x00\x04size\x78\x70";
            if input.bytes.len() < schema.len() {
                return Err(SerialError::Io);
            }
            if input.take(schema.len())? != schema {
                return Err(SerialError::Unsupported);
            }
            let count = usize::try_from(i32::from_be_bytes(input.take(4)?.try_into().unwrap()))
                .map_err(|_| SerialError::Io)?;
            if input.take(2)? != [0x77, 4] {
                return Err(SerialError::Unsupported);
            }
            input.take(4)?; // ArrayList's compatibility capacity, ignored by readObject.
            if count > input.bytes.len() {
                return Err(SerialError::Io);
            }
            let mut other = false;
            let mut next_handle = 0x7e0002u32;
            for _ in 0..count {
                match input.take(1)?[0] {
                    0x70 => (),
                    0x74 => {
                        input.string(false)?;
                        other = true;
                        next_handle += 1;
                    }
                    0x7c => {
                        input.string(true)?;
                        other = true;
                        next_handle += 1;
                    }
                    0x71 => {
                        let handle = u32::from_be_bytes(input.take(4)?.try_into().unwrap());
                        if !(0x7e0001..next_handle).contains(&handle) {
                            return Err(SerialError::Io);
                        }
                        other = true;
                    }
                    _ => return Err(SerialError::Unsupported),
                }
            }
            if input.take(1)?[0] != 0x78 {
                return Err(SerialError::Io);
            }
            Ok(SerialValue::List { count, other })
        }
        _ => Err(SerialError::Unsupported),
    }
}
