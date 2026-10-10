//! Pinned DomainVerificationInfo and DomainVerificationUtils host-map parcels.
use std::io;
use std::os::fd::AsFd;

use aim_binder_host::parcel::Parcel;
use aim_service_aidl::WriteParcelable;

pub struct Info(Parcel);
pub struct Owner {
    pub name: String,
    pub overrideable: bool,
}
impl WriteParcelable for Owner {
    fn write_to(&self, out: &mut Parcel) {
        out.write_i32(if self.overrideable { 2 } else { 0 });
        out.write_string16(Some(&self.name));
    }
}
impl Info {
    /// Prepare before replying: creating a large-map region can fail.
    /// `prefix` includes the enclosing reply/typed-object headers.
    pub fn prepare(
        prefix: usize,
        id: &str,
        name: &str,
        states: &[(String, i32)],
    ) -> io::Result<Self> {
        let mut parcel = Parcel::new();
        parcel.write_string16(Some(&id.to_ascii_lowercase()));
        parcel.write_string16(Some(name));
        host_map(&mut parcel, prefix, states)?;
        Ok(Self(parcel))
    }
}
impl WriteParcelable for Info {
    fn write_to(&self, out: &mut Parcel) {
        out.write_raw_files(self.0.data(), self.0.objects(), self.0.files());
    }
}

pub struct UserState(Parcel);
pub struct UriGroups(Parcel);
impl UriGroups {
    pub fn prepare(
        entries: &[(
            Option<String>,
            Vec<crate::package::intent_filter::UriRelativeFilterGroup>,
        )],
    ) -> io::Result<Self> {
        let mut out = Parcel::new();
        if entries.is_empty() {
            out.write_i32(0);
            return Ok(Self(out));
        }
        let length = out.position();
        out.write_i32(-1);
        out.write_i32(crate::bundle::MAGIC);
        let start = out.position();
        out.write_i32(count(entries.len())?);
        for (host, groups) in entries {
            out.write_string16(host.as_deref());
            out.write_i32(11); // VAL_LIST, lazy value length
            let length = out.position();
            out.write_i32(-1);
            let start = out.position();
            out.write_i32(count(groups.len())?);
            for group in groups {
                out.write_i32(4); // VAL_PARCELABLE, lazy value length
                let length = out.position();
                out.write_i32(-1);
                let start = out.position();
                out.write_string16(Some("android.content.UriRelativeFilterGroupParcel"));
                let size = out.position();
                out.write_i32(-1);
                out.write_i32(group.action);
                out.write_i32(count(group.filters.len())?);
                for filter in &group.filters {
                    out.write_i32(1); // typed UriRelativeFilterParcel
                    let size = out.position();
                    out.write_i32(-1);
                    out.write_i32(filter.uri_part);
                    out.write_i32(filter.pattern_type);
                    out.write_string16(filter.filter.as_deref());
                    out.set_i32_at(size, count(out.position() - size)?);
                }
                out.set_i32_at(size, count(out.position() - size)?);
                out.set_i32_at(length, count(out.position() - start)?);
            }
            out.set_i32_at(length, count(out.position() - start)?);
        }
        out.set_i32_at(length, count(out.position() - start)?);
        out.write_bool(false); // BaseBundle.mHasIntent
        Ok(Self(out))
    }
}
fn count(value: usize) -> io::Result<i32> {
    i32::try_from(value).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "URI-group parcel size overflow",
        )
    })
}
impl WriteParcelable for UriGroups {
    fn write_to(&self, out: &mut Parcel) {
        out.write_raw(self.0.data(), &[]);
    }
}
impl UserState {
    pub fn prepare(
        prefix: usize,
        id: &str,
        name: &str,
        user: i32,
        allowed: bool,
        states: &[(String, i32)],
    ) -> io::Result<Self> {
        let mut parcel = Parcel::new();
        parcel.write_i32(if allowed { 8 } else { 0 }); // writeByte(flg), padded to int32
        parcel.write_string16(Some(&id.to_ascii_lowercase()));
        parcel.write_string16(Some(name));
        parcel.write_i32(1); // typed UserHandle
        parcel.write_i32(user);
        host_map(&mut parcel, prefix, states)?;
        Ok(Self(parcel))
    }
}
impl WriteParcelable for UserState {
    fn write_to(&self, out: &mut Parcel) {
        out.write_raw_files(self.0.data(), self.0.objects(), self.0.files());
    }
}

fn host_map(parcel: &mut Parcel, prefix: usize, states: &[(String, i32)]) -> io::Result<()> {
    let mut estimated = prefix + parcel.data().len();
    let mut blob = false;
    for (host, _) in states {
        estimated = estimated
            .checked_add(host.encode_utf16().count() * 2 + 12)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "host-map size overflow"))?;
        if estimated > 32768 {
            blob = true;
            break;
        }
    }
    let mut map = Parcel::new();
    map.write_i32(
        i32::try_from(states.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "host-map count overflow"))?,
    );
    for (host, state) in states {
        map.write_i32(0); // Parcel.VAL_STRING
        map.write_string16(Some(host));
        map.write_i32(1); // Parcel.VAL_INTEGER
        map.write_i32(*state);
    }
    parcel.write_bool(blob);
    if !blob {
        parcel.write_raw(map.data(), &[]);
    } else {
        parcel
            .write_i32(i32::try_from(map.data().len()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "blob length overflow")
            })?);
        if map.data().len() <= 16384 {
            parcel.write_i32(0); // BLOB_INPLACE
            parcel.write_raw(map.data(), &[]);
        } else {
            let region = aim_ashmem::immutable_blob(map.data())?;
            let file = aim_binder_host::server::file_from_fd(region.as_fd())
                .ok_or_else(|| io::Error::other("cannot retain native blob fileport"))?;
            parcel.write_i32(1); // BLOB_ASHMEM_IMMUTABLE
            parcel.write_file(file);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "00000000-0000-0000-0000-000000000abc";

    fn map_start(info: &Info) -> aim_binder_host::parcel::Reader<'_> {
        let mut reader = aim_binder_host::parcel::Reader::new(info.0.data(), info.0.objects());
        assert_eq!(reader.read_string16().unwrap().as_deref(), Some(ID));
        assert_eq!(reader.read_string16().unwrap().as_deref(), Some("fixture"));
        reader
    }

    #[test]
    fn uri_group_bundle_retains_null_filter_and_empty_filter_distinction() {
        use crate::package::intent_filter::UriRelativeFilterGroup;
        use crate::package::domain_verification::uri_parcel::Group;
        use aim_service_aidl::ReadParcelable;
        let mut group = UriRelativeFilterGroup::new(0);
        group.add_nullable(0, 0, None); group.add(0, 0, "");
        let value = UriGroups::prepare(&[(Some("x.example".into()), vec![group])]).unwrap();
        let mut reader = aim_binder_host::parcel::Reader::new(value.0.data(), &[]);
        reader.read_i32().unwrap(); assert_eq!(reader.read_i32().unwrap(), crate::bundle::MAGIC);
        assert_eq!(reader.read_i32().unwrap(), 1); reader.read_string16().unwrap();
        assert_eq!(reader.read_i32().unwrap(), 11); reader.read_i32().unwrap(); assert_eq!(reader.read_i32().unwrap(), 1);
        assert_eq!(reader.read_i32().unwrap(), 4); reader.read_i32().unwrap(); reader.read_string16().unwrap();
        let decoded = Group::read_from(&mut reader).unwrap();
        let filters = decoded.filters.unwrap(); assert_eq!(filters.len(), 2);
        assert_eq!(filters[0].as_ref().unwrap().filter, None); assert_eq!(filters[1].as_ref().unwrap().filter.as_deref(), Some(""));
        assert!(!reader.read_bool().unwrap()); assert_eq!(reader.remaining(), 0);
    }

    #[test]
    fn host_map_threshold_includes_headers_and_utf16_units() {
        let base = 8 + 80 + 20; // reply headers, UUID String16, package String16
        let units = (32768 - base - 12) / 2;
        let host = "😀".repeat(units / 2) + &"a".repeat(units % 2);
        let inline = Info::prepare(8, ID, "fixture", &[(host.clone(), 1024)]).unwrap();
        assert_eq!(map_start(&inline).read_i32().unwrap(), 0);
        assert!(inline.0.files().is_empty());
        let large = Info::prepare(8, ID, "fixture", &[(host + "a", 1024)]).unwrap();
        let mut reader = map_start(&large);
        assert_eq!(reader.read_i32().unwrap(), 1);
        assert!(reader.read_i32().unwrap() > 16384);
        assert_eq!(reader.read_i32().unwrap(), 1);
        assert_eq!(large.0.files().len(), 1);
        let file = aim_binder_host::server::file_fd(&large.0.files()[0].1).unwrap();
        let bytes = file.metadata().unwrap().len();
        assert!(bytes > 16384);
    }

    #[test]
    fn oversized_prefix_uses_inline_blob_for_a_small_map_and_empty_stays_inline() {
        let small = Info::prepare(40000, ID, "fixture", &[("a.example".into(), 1)]).unwrap();
        let mut reader = map_start(&small);
        assert_eq!(reader.read_i32().unwrap(), 1);
        let length = reader.read_i32().unwrap();
        assert_eq!(reader.read_i32().unwrap(), 0);
        assert_eq!(reader.remaining(), length as usize);
        assert!(small.0.files().is_empty());
        let empty = Info::prepare(40000, ID, "fixture", &[]).unwrap();
        assert_eq!(map_start(&empty).read_i32().unwrap(), 0);
    }
}
