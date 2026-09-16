//! Bounded positional reads from admitted file descriptors. This module owns
//! neither namespace policy nor path lookup and never reopens a pathname.
use super::*;
use std::os::unix::fs::FileExt;

pub(super) fn read_admitted_file(
    file: &File,
    root_is_elf: Option<&mut bool>,
) -> Result<Vec<u8>, FfiFailure> {
    let metadata = file.metadata().map_err(|e| FfiFailure::Io(e.to_string()))?;
    if !metadata.is_file() {
        return Err(FfiFailure::Invalid("ELF input is not a regular file"));
    }
    let mut prefix = [0u8; 4];
    let mut count = 0;
    while count < prefix.len() {
        match file.read_at(&mut prefix[count..], count as u64) {
            Ok(0) => break,
            Ok(read) => count += read,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(FfiFailure::Io(e.to_string())),
        }
    }
    if let Some(output) = root_is_elf {
        *output = count == 4 && prefix == *b"\x7fELF";
    }
    let size = usize::try_from(metadata.len())
        .map_err(|_| FfiFailure::Bounds("ELF size overflow".into()))?;
    if size == 0 || size > MAX_DISCOVERY_FILE_SIZE {
        return Err(FfiFailure::Bounds(
            "ELF file exceeds discovery size bounds".into(),
        ));
    }
    // An extra byte detects growth without allocating according to a changed
    // file length. Positional reads never alter the caller's shared offset.
    let mut bytes = vec![0; size + 1];
    let mut count = 0;
    while count < bytes.len() {
        match file.read_at(&mut bytes[count..], count as u64) {
            Ok(0) => break,
            Ok(read) => count += read,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(FfiFailure::Io(e.to_string())),
        }
    }
    if count != size {
        return Err(FfiFailure::Io("ELF changed size during read".into()));
    }
    bytes.truncate(size);
    Ok(bytes)
}

/// # Safety
/// fd is a live host descriptor throughout the call; outputs follow the same
/// ownership/writability contract as inspect_bytes. No namespace grant implied.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_elf_inspect_fd(
    fd: i32,
    output: *mut *mut DarwinArtElfInspection,
    error: *mut DarwinArtElfErrorBuffer,
) -> DarwinArtElfStatus {
    ffi_call(error, || {
        if output.is_null() {
            return Err(FfiFailure::Invalid("inspection output is null"));
        }
        unsafe {
            *output = ptr::null_mut();
        }
        if fd < 0 {
            return Err(FfiFailure::Invalid("negative ELF fd"));
        }
        use std::os::fd::BorrowedFd;
        let owned = unsafe { BorrowedFd::borrow_raw(fd) }
            .try_clone_to_owned()
            .map_err(|e| FfiFailure::Io(e.to_string()))?;
        let bytes = read_admitted_file(&File::from(owned), None)?;
        let inspection = inspection_from_bytes(&bytes)?;
        unsafe {
            *output = Box::into_raw(Box::new(inspection));
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom};
    #[test]
    fn admitted_reader_preserves_offset_and_open_identity() {
        let path = std::env::temp_dir().join(format!(
            "darwin-elf-fd-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, b"\x7fELFcontents").unwrap();
        let mut file = File::open(&path).unwrap();
        file.seek(SeekFrom::Start(3)).unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        let mut magic = false;
        assert_eq!(
            read_admitted_file(&file, Some(&mut magic))
                .unwrap_or_else(|_| panic!("admitted read failed")),
            b"\x7fELFcontents"
        );
        assert!(magic);
        assert_eq!(file.stream_position().unwrap(), 3);
        assert!(read_admitted_file(&File::open(std::env::temp_dir()).unwrap(), None).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
