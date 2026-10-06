//! URI-group Bundle input. Keep lazy values until permission/name/domain checks.
use super::uri_parcel::Group;
use aim_binder_host::parcel::{BAD_VALUE, Reader, Result};
use aim_service_aidl::ReadParcelable;

#[derive(Debug)]
pub enum GroupError {
    Parcel(i32),
    BadParcelable(String),
    Unavailable,
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
                read_serializable(&mut value)?;
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
                read_serializable(&mut value)?;
                groups.push(None);
                continue;
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

fn read_serializable(reader: &mut Reader<'_>) -> std::result::Result<(), GroupError> {
    let Some(name) = reader.read_string16()? else {
        return Ok(());
    };
    let bytes = aim_service_aidl::read_byte_array(reader)?.ok_or(GroupError::Unavailable)?;
    // ObjectInputStream reads the stream header before resolving any class.
    if bytes.len() <= 4 || bytes[..4] != [0xac, 0xed, 0, 5] {
        return Err(GroupError::BadParcelable(format!(
            "Parcelable encountered IOException reading a Serializable object (name = {name})"
        )));
    }
    Err(GroupError::Unavailable)
}
