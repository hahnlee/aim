//! DomainSet / DomainVerificationUtils.readHostSet, android-16.0.0_r1 (AOSP).
use aim_binder_host::{
    local::LocalProcess,
    parcel::{BAD_VALUE, Reader, Result},
};
use aim_service_aidl::ReadParcelable;
use std::os::unix::fs::FileExt;

pub enum DomainSet {
    Hosts(Vec<Option<String>>),
    Blob { fd: u32, len: usize },
}

impl ReadParcelable for DomainSet {
    fn read_from(reader: &mut Reader<'_>) -> Result<Self> {
        if !reader.read_bool()? {
            return hosts(reader).map(Self::Hosts);
        }
        let len = usize::try_from(reader.read_i32()?).map_err(|_| BAD_VALUE)?;
        match reader.read_i32()? {
            0 => {
                let start = reader.position();
                reader.skip(len)?;
                let (bytes, _) = reader.since(start);
                let mut blob = Reader::new(&bytes[..len], &[]);
                let value = hosts(&mut blob)?;
                // Original unmarshalling permits unused bytes in the blob.
                Ok(Self::Hosts(value))
            }
            1 | 2 => Ok(Self::Blob {
                fd: reader.read_fd()?,
                len,
            }),
            _ => Err(BAD_VALUE),
        }
    }
}

impl DomainSet {
    pub fn resolve(self, process: &LocalProcess) -> Result<Vec<Option<String>>> {
        match self {
            Self::Hosts(hosts) => Ok(hosts),
            Self::Blob { fd, len } => {
                let file = process.file(fd).ok_or(BAD_VALUE)?;
                let fd = aim_binder_host::server::file_fd(&file).ok_or(BAD_VALUE)?;
                let file = fd;
                if file.metadata().map_err(|_| BAD_VALUE)?.len() < len as u64 {
                    return Err(BAD_VALUE);
                }
                let mut bytes = Vec::new();
                bytes.try_reserve_exact(len).map_err(|_| BAD_VALUE)?;
                bytes.resize(len, 0);
                file.read_exact_at(&mut bytes, 0).map_err(|_| BAD_VALUE)?;
                hosts(&mut Reader::new(&bytes, &[]))
            }
        }
    }
}

fn hosts(reader: &mut Reader<'_>) -> Result<Vec<Option<String>>> {
    let count = reader.read_i32()?;
    if count == -1 {
        return Ok(vec![]);
    }
    let count = usize::try_from(count).map_err(|_| BAD_VALUE)?;
    if count > reader.remaining() / 4 {
        return Err(BAD_VALUE);
    }
    let mut hosts = Vec::new();
    for _ in 0..count {
        let host = reader.read_string16()?;
        if !hosts.contains(&host) {
            hosts.push(host);
        }
    }
    hosts.sort_by_key(|host| crate::package::info::java_hash(host.as_deref().unwrap_or("")));
    Ok(hosts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::Parcel;
    #[test]
    fn malformed_counts_and_blob_frames_fail() {
        for words in [
            vec![0, -2],
            vec![0, i32::MAX],
            vec![1, -1, 0],
            vec![1, 8, 3],
            vec![1, 8, 0, 1],
        ] {
            let mut parcel = Parcel::new();
            for word in words {
                parcel.write_i32(word);
            }
            assert!(DomainSet::read_from(&mut Reader::new(parcel.data(), &[])).is_err());
        }
    }
    #[test]
    fn inline_set_keeps_null_empty_collision_order_and_deduplicates() {
        let mut parcel = Parcel::new();
        parcel.write_bool(false);
        parcel.write_i32(5);
        for host in [Some("BB"), None, Some("Aa"), Some(""), Some("BB")] {
            parcel.write_string16(host);
        }
        let DomainSet::Hosts(hosts) =
            DomainSet::read_from(&mut Reader::new(parcel.data(), &[])).unwrap()
        else {
            panic!("not inline")
        };
        assert_eq!(
            hosts,
            [None, Some("".into()), Some("BB".into()), Some("Aa".into())]
        );
    }
}
