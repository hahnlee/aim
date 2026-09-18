//! Immutable shared-memory Binder transaction data plane.
//!
//! The sender writes one anonymous object, drops every writable handle/mapping,
//! and transfers only an O_RDONLY descriptor. The receiver validates and maps
//! it without copying Parcel, offsets or already-captured scatter/gather bytes.

use crate::descriptor_manifest::{self, DescriptorAttributes, DescriptorBundle};
use crate::{objects, transaction_layout::TransactionLayout};
use std::{
    ffi::CString,
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
    ptr::NonNull,
};

const MAGIC: &[u8; 8] = b"DABTX003";
const VERSION: u32 = 4;
const HEADER_BYTES: usize = 56;
const MANIFEST_OBJECT_BYTES: usize = 32;
const MAX_TRANSFER_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    InvalidFormat,
    Objects(objects::Error),
    RemoteObjects(crate::remote_objects::Error),
    UnsupportedDescriptorAttributes,
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

struct ReadOnlyView {
    address: NonNull<u8>,
    length: usize,
}

struct WritableView {
    address: NonNull<u8>,
    length: usize,
}

// SAFETY: this mapping is created PROT_READ from an O_RDONLY descriptor after
// every writable descriptor and mapping has been destroyed. Safe APIs expose
// shared immutable slices only.
unsafe impl Send for ReadOnlyView {}
unsafe impl Sync for ReadOnlyView {}

impl Drop for ReadOnlyView {
    fn drop(&mut self) {
        // SAFETY: this object uniquely owns the successful mapping.
        unsafe {
            libc::munmap(self.address.as_ptr().cast(), self.length);
        }
    }
}

impl WritableView {
    fn map(descriptor: &OwnedFd, length: usize) -> io::Result<Self> {
        // SAFETY: kernel chooses the address; descriptor is live and sized.
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                descriptor.as_raw_fd(),
                0,
            )
        };
        if address == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        let Some(address) = NonNull::new(address.cast()) else {
            // SAFETY: a successful zero-address mapping still needs releasing.
            unsafe { libc::munmap(address, length) };
            return Err(io::Error::other("null transfer mapping"));
        };
        Ok(Self { address, length })
    }

    fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: exclusive owner of this live writable mapping.
        unsafe { std::slice::from_raw_parts_mut(self.address.as_ptr(), self.length) }
    }
}

impl Drop for WritableView {
    fn drop(&mut self) {
        // SAFETY: this object uniquely owns the successful mapping.
        unsafe {
            libc::munmap(self.address.as_ptr().cast(), self.length);
        }
    }
}

pub struct TransferImage {
    descriptor: OwnedFd,
    view: ReadOnlyView,
    layout: TransactionLayout,
    payload_offset: usize,
    objects: Vec<crate::remote_objects::ManifestObject>,
    files: Vec<TransferFd>,
}

pub struct TransferFd {
    offset: usize,
    ordinal: u64,
    attributes: DescriptorAttributes,
    descriptor: OwnedFd,
}

impl TransferFd {
    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn descriptor(&self) -> BorrowedFd<'_> {
        self.descriptor.as_fd()
    }

    pub fn ordinal(&self) -> u64 {
        self.ordinal
    }

    pub fn attributes(&self) -> &DescriptorAttributes {
        &self.attributes
    }

    pub fn metadata(&self) -> &[u8] {
        self.attributes.as_bytes()
    }
}

impl TransferImage {
    pub fn capture(
        snapshot: &crate::transaction_snapshot::TransactionSnapshot,
        extra: &[u8],
    ) -> Result<Self, Error> {
        Self::capture_with_objects(snapshot, extra, &[])
    }

    pub fn capture_with_objects(
        snapshot: &crate::transaction_snapshot::TransactionSnapshot,
        extra: &[u8],
        objects: &[crate::remote_objects::ManifestObject],
    ) -> Result<Self, Error> {
        Self::capture_with_objects_and_fds(snapshot, extra, objects, Vec::new())
    }

    pub fn capture_with_objects_and_fds(
        snapshot: &crate::transaction_snapshot::TransactionSnapshot,
        extra: &[u8],
        objects: &[crate::remote_objects::ManifestObject],
        files: Vec<(usize, OwnedFd)>,
    ) -> Result<Self, Error> {
        let descriptors = files
            .into_iter()
            .enumerate()
            .map(|(ordinal, (offset, descriptor))| {
                DescriptorBundle::new(
                    offset,
                    ordinal as u64,
                    descriptor,
                    DescriptorAttributes::empty(),
                )
            })
            .collect();
        Self::capture_with_objects_and_descriptors(snapshot, extra, objects, descriptors)
    }

    pub fn capture_with_objects_and_descriptors(
        snapshot: &crate::transaction_snapshot::TransactionSnapshot,
        extra: &[u8],
        objects: &[crate::remote_objects::ManifestObject],
        files: Vec<DescriptorBundle>,
    ) -> Result<Self, Error> {
        if extra.len() != snapshot.layout().extra().len() {
            return Err(Error::InvalidFormat);
        }
        let parsed = snapshot.objects().map_err(Error::Objects)?;
        crate::remote_objects::validate_manifest(&parsed, objects).map_err(Error::RemoteObjects)?;
        validate_files(&parsed, &files)?;
        let manifest_bytes = objects
            .len()
            .checked_mul(MANIFEST_OBJECT_BYTES)
            .ok_or(Error::InvalidFormat)?;
        let payload_bytes = snapshot.layout().total();
        let file_manifest_bytes = files.iter().try_fold(0_usize, |total, file| {
            descriptor_manifest::encoded_len(file.attributes().as_bytes().len())
                .map_err(|_| Error::InvalidFormat)
                .and_then(|length| total.checked_add(length).ok_or(Error::InvalidFormat))
        })?;
        let total = HEADER_BYTES
            .checked_add(manifest_bytes)
            .and_then(|total| total.checked_add(file_manifest_bytes))
            .and_then(|total| total.checked_add(payload_bytes))
            .filter(|total| *total <= MAX_TRANSFER_BYTES)
            .ok_or(Error::InvalidFormat)?;
        let (writable, readonly) = immutable_backing(total)?;
        let mut header = [0_u8; HEADER_BYTES];
        header[..8].copy_from_slice(MAGIC);
        header[8..12].copy_from_slice(&VERSION.to_le_bytes());
        header[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        header[16..24].copy_from_slice(&(snapshot.data().len() as u64).to_le_bytes());
        header[24..32].copy_from_slice(&(snapshot.offsets().len() as u64).to_le_bytes());
        header[32..40].copy_from_slice(&(extra.len() as u64).to_le_bytes());
        header[40..48].copy_from_slice(&(objects.len() as u64).to_le_bytes());
        header[48..56].copy_from_slice(&(files.len() as u64).to_le_bytes());
        let mut writer = WritableView::map(&writable, total)?;
        let bytes = writer.bytes();
        bytes.fill(0);
        bytes[..HEADER_BYTES].copy_from_slice(&header);
        let mut cursor = HEADER_BYTES;
        for object in objects {
            let entry = &mut bytes[cursor..cursor + MANIFEST_OBJECT_BYTES];
            entry[..8].copy_from_slice(&(object.offset as u64).to_le_bytes());
            entry[8..16].copy_from_slice(&object.node.owner().get().to_le_bytes());
            entry[16..24].copy_from_slice(&object.node.local().get().to_le_bytes());
            entry[24..28].copy_from_slice(&object.flags.to_le_bytes());
            entry[28] = match object.strength {
                crate::reference_table::Strength::Strong => 0,
                crate::reference_table::Strength::Weak => 1,
            };
            cursor += MANIFEST_OBJECT_BYTES;
        }
        for file in &files {
            let encoded = descriptor_manifest::encode(
                &mut bytes[cursor..],
                file.offset(),
                file.ordinal(),
                file.attributes(),
            )
            .map_err(|_| Error::InvalidFormat)?;
            cursor += encoded;
        }
        let payload = &mut bytes[cursor..];
        payload[snapshot.layout().data()].copy_from_slice(snapshot.data());
        payload[snapshot.layout().offsets()].copy_from_slice(snapshot.offsets());
        payload[snapshot.layout().extra()].copy_from_slice(extra);
        drop(writer);
        drop(writable);
        Self::import_with_fds(
            readonly,
            files.into_iter().map(|file| file.into_parts().2).collect(),
        )
    }

    pub fn import(descriptor: OwnedFd) -> Result<Self, Error> {
        Self::import_with_fds(descriptor, Vec::new())
    }

    pub fn import_with_fds(descriptor: OwnedFd, files: Vec<OwnedFd>) -> Result<Self, Error> {
        Self::validate_carrier(descriptor.as_fd())?;
        let length = descriptor_length(descriptor.as_fd())?;
        // SAFETY: checked positive length and live descriptor. O_RDONLY plus
        // PROT_READ prevents this process from introducing a writable alias.
        let address = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ,
                libc::MAP_SHARED,
                descriptor.as_raw_fd(),
                0,
            )
        };
        if address == libc::MAP_FAILED {
            return Err(io::Error::last_os_error().into());
        }
        let Some(address) = NonNull::new(address.cast()) else {
            // SAFETY: a successful zero-address mapping still needs releasing.
            unsafe { libc::munmap(address, length) };
            return Err(Error::InvalidFormat);
        };
        let view = ReadOnlyView { address, length };
        let bytes = view.bytes();
        if &bytes[..8] != MAGIC
            || u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != VERSION
            || u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize != HEADER_BYTES
        {
            return Err(Error::InvalidFormat);
        }
        let data = usize_field(bytes, 16)?;
        let offsets = usize_field(bytes, 24)?;
        let extra = usize_field(bytes, 32)?;
        let object_count = usize_field(bytes, 40)?;
        let file_count = usize_field(bytes, 48)?;
        if file_count != files.len() {
            return Err(Error::InvalidFormat);
        }
        let manifest_bytes = object_count
            .checked_mul(MANIFEST_OBJECT_BYTES)
            .ok_or(Error::InvalidFormat)?;
        if HEADER_BYTES
            .checked_add(manifest_bytes)
            .filter(|end| *end <= length)
            .is_none()
        {
            return Err(Error::InvalidFormat);
        }
        let mut payload_offset = HEADER_BYTES
            .checked_add(manifest_bytes)
            .ok_or(Error::InvalidFormat)?;
        let mut decoded_files = Vec::new();
        decoded_files
            .try_reserve_exact(file_count)
            .map_err(|_| Error::InvalidFormat)?;
        for ordinal in 0..file_count {
            let source = bytes.get(payload_offset..).ok_or(Error::InvalidFormat)?;
            let decoded = descriptor_manifest::decode(source).map_err(|_| Error::InvalidFormat)?;
            if decoded.ordinal != ordinal as u64 {
                return Err(Error::InvalidFormat);
            }
            let attributes = DescriptorAttributes::new(decoded.attributes.to_vec())
                .map_err(|_| Error::InvalidFormat)?;
            decoded_files.push((decoded.offset, decoded.ordinal, attributes));
            payload_offset = payload_offset
                .checked_add(decoded.encoded_len)
                .ok_or(Error::InvalidFormat)?;
        }
        let layout = TransactionLayout::new(data, offsets, extra).ok_or(Error::InvalidFormat)?;
        let logical_length = payload_offset
            .checked_add(layout.total())
            .ok_or(Error::InvalidFormat)?;
        // Darwin rounds POSIX shm objects up to its VM page size. Accept only
        // zero-filled kernel padding beyond the authenticated logical image.
        if logical_length > length || bytes[logical_length..].iter().any(|byte| *byte != 0) {
            return Err(Error::InvalidFormat);
        }
        let payload = &bytes[payload_offset..logical_length];
        let parsed = objects::validate(&payload[layout.data()], &payload[layout.offsets()])
            .map_err(Error::Objects)?;
        let mut manifest = Vec::new();
        manifest
            .try_reserve_exact(object_count)
            .map_err(|_| Error::InvalidFormat)?;
        for index in 0..object_count {
            let start = HEADER_BYTES + index * MANIFEST_OBJECT_BYTES;
            let entry = &bytes[start..start + MANIFEST_OBJECT_BYTES];
            if entry[29..32].iter().any(|byte| *byte != 0) {
                return Err(Error::InvalidFormat);
            }
            let owner = crate::authority_protocol::ConnectionToken::from_nonzero(
                u64::from_le_bytes(entry[8..16].try_into().unwrap()),
            )
            .ok_or(Error::InvalidFormat)?;
            let local = crate::authority_protocol::LocalNodeToken::from_nonzero(
                u64::from_le_bytes(entry[16..24].try_into().unwrap()),
            )
            .ok_or(Error::InvalidFormat)?;
            let strength = match entry[28] {
                0 => crate::reference_table::Strength::Strong,
                1 => crate::reference_table::Strength::Weak,
                _ => return Err(Error::InvalidFormat),
            };
            manifest.push(crate::remote_objects::ManifestObject {
                offset: usize::try_from(u64::from_le_bytes(entry[..8].try_into().unwrap()))
                    .map_err(|_| Error::InvalidFormat)?,
                node: crate::authority_protocol::NodeToken::new(owner, local),
                flags: u32::from_le_bytes(entry[24..28].try_into().unwrap()),
                strength,
            });
        }
        crate::remote_objects::validate_manifest(&parsed, &manifest)
            .map_err(Error::RemoteObjects)?;
        let mut imported_files = Vec::new();
        imported_files
            .try_reserve_exact(file_count)
            .map_err(|_| Error::InvalidFormat)?;
        for (index, descriptor) in files.into_iter().enumerate() {
            imported_files.push(TransferFd {
                offset: decoded_files[index].0,
                ordinal: decoded_files[index].1,
                attributes: decoded_files[index].2.clone(),
                descriptor,
            });
        }
        validate_imported_files(&parsed, &imported_files)?;
        Ok(Self {
            descriptor,
            view,
            layout,
            payload_offset,
            objects: manifest,
            files: imported_files,
        })
    }

    /// Cheap daemon-side carrier validation. It never maps or reads Parcel
    /// contents; the receiving endpoint performs complete image validation.
    pub fn validate_carrier(descriptor: BorrowedFd<'_>) -> Result<(), Error> {
        // SAFETY: query flags on a live descriptor without changing state.
        let flags = unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error().into());
        }
        if flags & libc::O_ACCMODE != libc::O_RDONLY {
            return Err(Error::InvalidFormat);
        }
        let length = descriptor_length(descriptor)?;
        if !(HEADER_BYTES..=MAX_TRANSFER_BYTES).contains(&length) {
            return Err(Error::InvalidFormat);
        }
        Ok(())
    }

    pub fn descriptor(&self) -> BorrowedFd<'_> {
        self.descriptor.as_fd()
    }

    pub fn try_clone_descriptor(&self) -> io::Result<OwnedFd> {
        self.descriptor.try_clone()
    }

    pub fn try_clone_files(&self) -> io::Result<Vec<OwnedFd>> {
        if self.files.iter().any(|file| !file.metadata().is_empty()) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "bare descriptor cloning would discard provider attributes",
            ));
        }
        self.files
            .iter()
            .map(|file| file.descriptor.try_clone())
            .collect()
    }

    pub fn try_clone_descriptors(&self) -> io::Result<Vec<DescriptorBundle>> {
        self.files
            .iter()
            .map(|file| {
                Ok(DescriptorBundle::new(
                    file.offset,
                    file.ordinal,
                    file.descriptor.try_clone()?,
                    file.attributes.clone(),
                ))
            })
            .collect()
    }

    pub fn data(&self) -> &[u8] {
        &self.payload()[self.layout.data()]
    }

    pub fn offsets(&self) -> &[u8] {
        &self.payload()[self.layout.offsets()]
    }

    pub fn extra(&self) -> &[u8] {
        &self.payload()[self.layout.extra()]
    }

    pub fn layout(&self) -> &TransactionLayout {
        &self.layout
    }

    pub fn objects_manifest(&self) -> &[crate::remote_objects::ManifestObject] {
        &self.objects
    }

    pub fn files(&self) -> &[TransferFd] {
        &self.files
    }

    pub fn take_files(&mut self) -> Result<Vec<(usize, OwnedFd)>, Error> {
        if self.files.iter().any(|file| !file.metadata().is_empty()) {
            return Err(Error::UnsupportedDescriptorAttributes);
        }
        Ok(self
            .take_descriptors()
            .into_iter()
            .map(|file| {
                let (offset, _, descriptor, _) = file.into_parts();
                (offset, descriptor)
            })
            .collect())
    }

    pub fn take_descriptors(&mut self) -> Vec<DescriptorBundle> {
        std::mem::take(&mut self.files)
            .into_iter()
            .map(|file| {
                DescriptorBundle::new(file.offset, file.ordinal, file.descriptor, file.attributes)
            })
            .collect()
    }

    fn payload(&self) -> &[u8] {
        &self.view.bytes()[self.payload_offset..self.payload_offset + self.layout.total()]
    }
}

fn validate_files(
    objects: &[objects::Object<'_>],
    files: &[DescriptorBundle],
) -> Result<(), Error> {
    let expected: Vec<_> = objects
        .iter()
        .filter(|object| object.kind() == objects::Kind::Fd)
        .map(|object| object.offset())
        .collect();
    if expected.len() != files.len()
        || expected
            .iter()
            .zip(files)
            .enumerate()
            .any(|(index, (expected, actual))| {
                *expected != actual.offset() || actual.ordinal() != index as u64
            })
    {
        return Err(Error::InvalidFormat);
    }
    Ok(())
}

fn validate_imported_files(
    objects: &[objects::Object<'_>],
    files: &[TransferFd],
) -> Result<(), Error> {
    let expected: Vec<_> = objects
        .iter()
        .filter(|object| object.kind() == objects::Kind::Fd)
        .map(|object| object.offset())
        .collect();
    if expected.len() != files.len()
        || expected
            .iter()
            .zip(files)
            .enumerate()
            .any(|(index, (expected, actual))| {
                *expected != actual.offset || actual.ordinal != index as u64
            })
    {
        return Err(Error::InvalidFormat);
    }
    Ok(())
}

impl crate::transaction_snapshot::TransactionPayload for TransferImage {
    fn data(&self) -> &[u8] {
        self.data()
    }

    fn offsets(&self) -> &[u8] {
        self.offsets()
    }

    fn layout(&self) -> &TransactionLayout {
        self.layout()
    }
}

impl ReadOnlyView {
    fn bytes(&self) -> &[u8] {
        // SAFETY: the live mapping is immutable and covers exactly length bytes.
        unsafe { std::slice::from_raw_parts(self.address.as_ptr(), self.length) }
    }
}

fn immutable_backing(length: usize) -> io::Result<(OwnedFd, OwnedFd)> {
    loop {
        let mut random = [0_u8; 8];
        // SAFETY: initialized writable buffer is below getentropy's limit.
        if unsafe { libc::getentropy(random.as_mut_ptr().cast(), random.len()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let name = CString::new(format!("/dart-btx-{:016x}", u64::from_ne_bytes(random))).unwrap();
        // SAFETY: private exclusive name and owner-only permissions.
        let writable = unsafe {
            libc::shm_open(
                name.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                0o600,
            )
        };
        if writable < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EEXIST) {
                continue;
            }
            return Err(error);
        }
        // SAFETY: successful shm_open transfers unique descriptor ownership.
        let writable = unsafe { OwnedFd::from_raw_fd(writable) };
        if unsafe { libc::ftruncate(writable.as_raw_fd(), length as libc::off_t) } != 0 {
            let _ = unsafe { libc::shm_unlink(name.as_ptr()) };
            return Err(io::Error::last_os_error());
        }
        // Open the transferable descriptor read-only before unlinking the name.
        let readonly = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) };
        let open_error = io::Error::last_os_error();
        let unlink_result = unsafe { libc::shm_unlink(name.as_ptr()) };
        if readonly < 0 {
            return Err(open_error);
        }
        // SAFETY: successful shm_open transfers descriptor ownership.
        let readonly = unsafe { OwnedFd::from_raw_fd(readonly) };
        if unlink_result != 0 {
            return Err(io::Error::last_os_error());
        }
        return Ok((writable, readonly));
    }
}

fn descriptor_length(descriptor: BorrowedFd<'_>) -> io::Result<usize> {
    let mut stat = std::mem::MaybeUninit::<libc::stat>::zeroed();
    // SAFETY: writable stat storage and live descriptor.
    if unsafe { libc::fstat(descriptor.as_raw_fd(), stat.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fstat initialized the complete structure.
    let stat = unsafe { stat.assume_init() };
    usize::try_from(stat.st_size).map_err(|_| io::Error::from_raw_os_error(libc::EOVERFLOW))
}

fn usize_field(bytes: &[u8], offset: usize) -> Result<usize, Error> {
    usize::try_from(u64::from_le_bytes(
        bytes[offset..offset + 8].try_into().unwrap(),
    ))
    .map_err(|_| Error::InvalidFormat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor_manifest::{DescriptorAttributes, DescriptorBundle};
    use crate::transaction_snapshot::TransactionSnapshot;
    use std::fs::File;

    #[test]
    fn read_only_duplicate_imports_without_copying_sections() {
        let mut source = b"parcel".to_vec();
        let snapshot = TransactionSnapshot::capture(&source, &[], 8).unwrap();
        let image = TransferImage::capture(&snapshot, b"extra123").unwrap();
        source.fill(0xff);
        let imported = TransferImage::import(image.try_clone_descriptor().unwrap()).unwrap();
        assert_eq!(imported.data(), b"parcel");
        assert!(imported.offsets().is_empty());
        assert_eq!(imported.extra(), b"extra123");
        assert_eq!(imported.layout(), snapshot.layout());

        let byte = [0_u8; 1];
        assert_eq!(
            unsafe {
                libc::pwrite(
                    imported.descriptor().as_raw_fd(),
                    byte.as_ptr().cast(),
                    1,
                    HEADER_BYTES as libc::off_t,
                )
            },
            -1
        );
    }

    #[test]
    fn malformed_header_size_and_object_offsets_fail_closed() {
        let snapshot = TransactionSnapshot::capture(b"parcel", &[], 0).unwrap();
        assert!(matches!(
            TransferImage::capture(&snapshot, b"x"),
            Err(Error::InvalidFormat)
        ));

        let (writable, readonly) = immutable_backing(HEADER_BYTES).unwrap();
        drop(writable);
        assert!(matches!(
            TransferImage::import(readonly),
            Err(Error::InvalidFormat)
        ));

        let (writable, _readonly) = immutable_backing(HEADER_BYTES).unwrap();
        assert!(matches!(
            TransferImage::import(writable),
            Err(Error::InvalidFormat)
        ));

        let mut data = [0_u8; 24];
        data[..4].copy_from_slice(&objects::Kind::Binder.tag().to_le_bytes());
        let snapshot = TransactionSnapshot::capture(&data, &0_u64.to_le_bytes(), 0).unwrap();
        assert!(matches!(
            TransferImage::capture(&snapshot, &[]),
            Err(Error::RemoteObjects(
                crate::remote_objects::Error::InvalidManifest
            ))
        ));
    }

    #[test]
    fn direct_fd_manifest_requires_one_ordered_sidecar_per_fd_object() {
        let mut data = vec![0_u8; 28];
        data[4..8].copy_from_slice(&objects::Kind::Fd.tag().to_le_bytes());
        data[12..16].copy_from_slice(&77_u32.to_le_bytes());
        let snapshot = TransactionSnapshot::capture(&data, &4_u64.to_le_bytes(), 0).unwrap();
        let file: OwnedFd = File::open("/dev/null").unwrap().into();
        let image =
            TransferImage::capture_with_objects_and_fds(&snapshot, &[], &[], vec![(4, file)])
                .unwrap();
        assert_eq!(image.files().len(), 1);
        assert_eq!(image.files()[0].offset(), 4);

        assert!(matches!(
            TransferImage::import(image.try_clone_descriptor().unwrap()),
            Err(Error::InvalidFormat)
        ));
        let imported = TransferImage::import_with_fds(
            image.try_clone_descriptor().unwrap(),
            image.try_clone_files().unwrap(),
        )
        .unwrap();
        assert_eq!(imported.files()[0].offset(), 4);
        assert_eq!(imported.data(), data);
    }

    #[test]
    fn typed_descriptor_attributes_survive_clone_import_and_take() {
        let mut data = vec![0_u8; 28];
        data[4..8].copy_from_slice(&objects::Kind::Fd.tag().to_le_bytes());
        data[12..16].copy_from_slice(&77_u32.to_le_bytes());
        let snapshot = TransactionSnapshot::capture(&data, &4_u64.to_le_bytes(), 0).unwrap();
        let file: OwnedFd = File::open("/dev/null").unwrap().into();
        let attrs = DescriptorAttributes::new(vec![0x12, 0x34, 0xa5]).unwrap();
        let image = TransferImage::capture_with_objects_and_descriptors(
            &snapshot,
            &[],
            &[],
            vec![DescriptorBundle::new(4, 0, file, attrs.clone())],
        )
        .unwrap();
        assert_eq!(image.files()[0].ordinal(), 0);
        assert_eq!(image.files()[0].metadata(), attrs.as_bytes());
        let cloned = image.try_clone_descriptors().unwrap();
        assert_eq!(cloned[0].ordinal(), 0);
        assert_eq!(cloned[0].attributes().as_bytes(), attrs.as_bytes());
        assert_eq!(
            image.try_clone_files().unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );

        let mut imported = TransferImage::import_with_fds(
            image.try_clone_descriptor().unwrap(),
            cloned.into_iter().map(|file| file.into_parts().2).collect(),
        )
        .unwrap();
        assert_eq!(imported.files()[0].metadata(), attrs.as_bytes());
        assert!(matches!(
            imported.take_files(),
            Err(Error::UnsupportedDescriptorAttributes)
        ));
        let taken = imported.take_descriptors();
        assert_eq!(taken[0].ordinal(), 0);
        assert_eq!(taken[0].attributes().as_bytes(), attrs.as_bytes());
    }

    #[test]
    fn descriptor_attributes_and_ordinals_are_bounded_before_capture() {
        assert!(
            DescriptorAttributes::new(vec![0; descriptor_manifest::MAX_ATTRIBUTES_BYTES + 1])
                .is_err()
        );
        let data = {
            let mut data = vec![0_u8; 28];
            data[4..8].copy_from_slice(&objects::Kind::Fd.tag().to_le_bytes());
            data[12..16].copy_from_slice(&77_u32.to_le_bytes());
            data
        };
        let snapshot = TransactionSnapshot::capture(&data, &4_u64.to_le_bytes(), 0).unwrap();
        let file: OwnedFd = File::open("/dev/null").unwrap().into();
        assert!(matches!(
            TransferImage::capture_with_objects_and_descriptors(
                &snapshot,
                &[],
                &[],
                vec![DescriptorBundle::new(
                    4,
                    1,
                    file,
                    DescriptorAttributes::empty(),
                )],
            ),
            Err(Error::InvalidFormat)
        ));
    }

    #[test]
    fn v4_import_rejects_nonzero_descriptor_reserved_field() {
        let payload_len = 28 + 8;
        let total = HEADER_BYTES + descriptor_manifest::ENTRY_HEADER_BYTES + payload_len;
        let (writable, readonly) = immutable_backing(total).unwrap();
        let mut writer = WritableView::map(&writable, total).unwrap();
        let bytes = writer.bytes();
        bytes.fill(0);
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes[16..24].copy_from_slice(&28_u64.to_le_bytes());
        bytes[24..32].copy_from_slice(&8_u64.to_le_bytes());
        bytes[40..48].copy_from_slice(&0_u64.to_le_bytes());
        bytes[48..56].copy_from_slice(&1_u64.to_le_bytes());
        let manifest = HEADER_BYTES;
        bytes[manifest..manifest + 8].copy_from_slice(&4_u64.to_le_bytes());
        bytes[manifest + 8..manifest + 16].copy_from_slice(&0_u64.to_le_bytes());
        bytes[manifest + 20] = 1;
        let payload = manifest + descriptor_manifest::ENTRY_HEADER_BYTES;
        bytes[payload + 4..payload + 8].copy_from_slice(&objects::Kind::Fd.tag().to_le_bytes());
        bytes[payload + 28..payload + 36].copy_from_slice(&4_u64.to_le_bytes());
        drop(writer);
        drop(writable);
        let file: OwnedFd = File::open("/dev/null").unwrap().into();
        assert!(matches!(
            TransferImage::import_with_fds(readonly, vec![file]),
            Err(Error::InvalidFormat)
        ));
    }
}
