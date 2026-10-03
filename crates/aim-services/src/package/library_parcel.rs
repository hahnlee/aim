//! Pinned SharedLibraryInfo parcels emitted by the original owner.
use aim_binder_host::parcel::{BAD_VALUE, Reader, Result};
use aim_service_aidl::read_byte_array;

use super::model::SharedLibrary;

const VERSIONED_PACKAGE: &str = "android.content.pm.VersionedPackage";

impl SharedLibrary {
    /// Encode retained raw owners with the pinned original wire format.
    pub fn write_parcel(&self) -> Vec<u8> {
        let mut parcel = aim_binder_host::parcel::Parcel::new();
        super::info::write_library(&mut parcel, self);
        parcel.data().to_vec()
    }

    /// Import one complete original owner; malformed or unsupported owners reject.
    pub fn read_parcel(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader::new(bytes, &[]);
        let library = read(&mut reader)?;
        if reader.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        Ok(library)
    }
}

fn count(r: &mut Reader<'_>) -> Result<Option<usize>> {
    let count = r.read_i32()?;
    match count {
        -1 => Ok(None),
        n if n >= 0 && n as usize <= r.remaining() / 4 => Ok(Some(n as usize)),
        _ => Err(BAD_VALUE),
    }
}

fn versioned(r: &mut Reader<'_>) -> Result<Option<(String, i64)>> {
    match r.read_string16()?.as_deref() {
        None => Ok(None),
        Some(VERSIONED_PACKAGE) => Ok(Some((r.read_string8()?.ok_or(BAD_VALUE)?, r.read_i64()?))),
        _ => Err(BAD_VALUE),
    }
}

fn read(r: &mut Reader<'_>) -> Result<SharedLibrary> {
    let path = r.read_string8()?;
    let package_name = r.read_string8()?;
    let code_paths = match r.read_i32()? {
        0 => None,
        1 => {
            let n = count(r)?.ok_or(BAD_VALUE)?;
            Some(
                (0..n)
                    .map(|_| r.read_string8()?.ok_or(BAD_VALUE))
                    .collect::<Result<_>>()?,
            )
        }
        _ => return Err(BAD_VALUE),
    };
    let mut library = SharedLibrary {
        path,
        package_name,
        code_paths,
        name: r.read_string8()?,
        version: r.read_i64()?,
        kind: r.read_i32()?,
        ..Default::default()
    };
    match versioned(r)? {
        Some(declaring) => library.declaring = declaring,
        None => library.declaring_absent = true,
    }
    if let Some(n) = count(r)? {
        library.dependents_initialized = true;
        for _ in 0..n {
            if r.read_i32()? != 4 {
                return Err(BAD_VALUE);
            }
            let length = r.read_i32()?;
            if length < 0 || length as usize > r.remaining() {
                return Err(BAD_VALUE);
            }
            let start = r.position();
            library.dependents.push(versioned(r)?.ok_or(BAD_VALUE)?);
            if r.position() - start != length as usize {
                return Err(BAD_VALUE);
            }
        }
    }
    if let Some(n) = count(r)? {
        library.dependencies_initialized = true;
        for _ in 0..n {
            if r.read_i32()? != 1 {
                return Err(BAD_VALUE);
            }
            library.dependencies.push(read(r)?);
        }
    }
    library.native = r.read_bool()?;
    library.optional_dependents = count(r)?
        .map(|n| (0..n).map(|_| versioned(r)).collect::<Result<_>>())
        .transpose()?;
    library.cert_digests = count(r)?
        .map(|n| (0..n).map(|_| r.read_string16()).collect::<Result<_>>())
        .transpose()?;
    Ok(library)
}

pub(super) fn read_feed(r: &mut Reader<'_>) -> Result<SharedLibrary> {
    let bytes = read_byte_array(r)?.ok_or(BAD_VALUE)?;
    SharedLibrary::read_parcel(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_optional_counts_and_oversized_counts_reject() {
        let bytes = SharedLibrary::default().write_parcel();
        for offset in [bytes.len() - 8, bytes.len() - 4] {
            for count in [-2_i32, i32::MAX] {
                let mut malformed = bytes.clone();
                malformed[offset..offset + 4].copy_from_slice(&count.to_le_bytes());
                assert!(SharedLibrary::read_parcel(&malformed).is_err());
            }
        }
    }
}
