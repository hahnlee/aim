//! NativeLibraryHelper.extractNativeLibFromApk at android-16.0.0_r1 (#810).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::NativeLibraryInstallPolicy;
use aim_storage::guest_inode::{GuestInode, record};
use android_image_extract::{source::ReadAt, zip::Archive};
use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, PartialEq, Eq)]
pub struct NativeLibraryInstallError {
    pub code: i32,
    pub message: String,
}

impl NativeLibraryInstallError {
    pub(super) fn new(code: i32, message: impl ToString) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

impl std::fmt::Display for NativeLibraryInstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (install code {})", self.message, self.code)
    }
}
impl std::error::Error for NativeLibraryInstallError {}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct NativeLibraryCopy {
    pub extracted: Vec<Vec<u8>>,
    pub reused: Vec<Vec<u8>>,
    pub direct: Vec<Vec<u8>>,
}

impl NativeLibraryInstallPolicy {
    /// Copy into a checked native directory owned by the installation caller.
    /// The clock owner converts DOS local time using the guest timezone; never
    /// use the host's local timezone for Android file metadata.
    pub fn copy_to(
        self,
        source: &dyn ReadAt,
        abi: &str,
        directory: &Path,
        owner: GuestInode,
        zip_time: &dyn Fn(u32) -> Result<SystemTime, String>,
    ) -> Result<NativeLibraryCopy, NativeLibraryInstallError> {
        let fail = NativeLibraryInstallError::new;
        let planned = self.inspect(source, abi)?;
        let archive = Archive::open(source).map_err(|e| fail(-2, e.to_string()))?;
        let mut result = NativeLibraryCopy::default();
        for planned in planned {
            if !planned.extract {
                result.direct.push(planned.name);
                continue;
            }
            let dir = fs::symlink_metadata(directory).map_err(|e| fail(-18, e.to_string()))?;
            if !dir.is_dir() {
                return Err(fail(
                    -18,
                    "native library destination is not a directory".into(),
                ));
            }
            let entry = &archive.entries[planned.index];
            let name = std::str::from_utf8(entry.name.rsplit(|b| *b == b'/').next().unwrap())
                .map_err(|e| fail(-2, e.to_string()))?;
            let destination = directory.join(name);
            let modified = zip_time(entry.dos_time).map_err(|e| fail(-110, e))?;
            let existing = match fs::symlink_metadata(&destination) {
                Ok(metadata) => Some(metadata),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(fail(-18, e.to_string())),
            };
            if let Some(metadata) = &existing {
                if metadata.is_file()
                    && metadata.len() == entry.size
                    && metadata
                        .modified()
                        .map_err(|e| fail(-18, e.to_string()))?
                        .duration_since(UNIX_EPOCH)
                        .map_err(|e| fail(-18, e.to_string()))?
                        .as_secs()
                        == modified
                            .duration_since(UNIX_EPOCH)
                            .map_err(|e| fail(-110, e.to_string()))?
                            .as_secs()
                    && file_crc(&destination).map_err(|e| fail(-18, e.to_string()))? == entry.crc32
                {
                    result.reused.push(planned.name);
                    continue;
                }
            }
            let (temporary, file) =
                temporary_file(directory).map_err(|e| fail(-18, e.to_string()))?;
            let copied: Result<(), NativeLibraryInstallError> = (|| {
                archive
                    .copy_to(entry, &mut &file)
                    .map_err(|e| fail(-18, e.to_string()))?;
                file.sync_all().map_err(|e| fail(-110, e.to_string()))?;
                let accessed = existing
                    .as_ref()
                    .map(|m| {
                        let seconds = m.atime();
                        let duration = std::time::Duration::from_secs(seconds.unsigned_abs());
                        if seconds < 0 {
                            UNIX_EPOCH - duration
                        } else {
                            UNIX_EPOCH + duration
                        }
                    })
                    .unwrap_or(UNIX_EPOCH);
                file.set_times(
                    FileTimes::new()
                        .set_accessed(accessed)
                        .set_modified(modified),
                )
                .map_err(|e| fail(-18, e.to_string()))?;
                file.set_permissions(fs::Permissions::from_mode(0o755))
                    .map_err(|e| fail(-18, e.to_string()))?;
                record(
                    &temporary,
                    GuestInode {
                        mode: Some(0o755),
                        ..owner
                    },
                )
                .map_err(|e| fail(-18, e.to_string()))?;
                fs::rename(&temporary, &destination).map_err(|e| fail(-18, e.to_string()))?;
                Ok(())
            })();
            if let Err(mut error) = copied {
                if let Err(cleanup) = fs::remove_file(&temporary) {
                    error.message =
                        format!("{}; temporary cleanup failed: {cleanup}", error.message);
                }
                return Err(error);
            }
            result.extracted.push(planned.name);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "aim-native-copy-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn archive(method: u16, payload: &[u8]) -> Vec<u8> {
        let name = b"lib/arm64-v8a/libx.so";
        let compressed = if method == 8 {
            let mut deflater =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            deflater.write_all(payload).unwrap();
            deflater.finish().unwrap()
        } else {
            payload.to_vec()
        };
        let mut local = vec![0; 30];
        local[..4].copy_from_slice(&0x04034b50u32.to_le_bytes());
        local[8..10].copy_from_slice(&method.to_le_bytes());
        local[14..18].copy_from_slice(&crc32fast::hash(payload).to_le_bytes());
        local[18..22].copy_from_slice(&(compressed.len() as u32).to_le_bytes());
        local[22..26].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        local[26..28].copy_from_slice(&(name.len() as u16).to_le_bytes());
        let mut central = vec![0; 46];
        central[..4].copy_from_slice(&0x02014b50u32.to_le_bytes());
        central[10..12].copy_from_slice(&method.to_le_bytes());
        central[16..28].copy_from_slice(&local[14..26]);
        central[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
        local.extend_from_slice(name);
        local.extend_from_slice(&compressed);
        let offset = local.len();
        local.extend_from_slice(&central);
        local.extend_from_slice(name);
        let mut end = vec![0; 22];
        end[..4].copy_from_slice(&0x06054b50u32.to_le_bytes());
        end[8..10].copy_from_slice(&1u16.to_le_bytes());
        end[10..12].copy_from_slice(&1u16.to_le_bytes());
        end[12..16].copy_from_slice(&((46 + name.len()) as u32).to_le_bytes());
        end[16..20].copy_from_slice(&(offset as u32).to_le_bytes());
        local.extend_from_slice(&end);
        local
    }
    fn policy() -> NativeLibraryInstallPolicy {
        NativeLibraryInstallPolicy {
            page_size: 16384,
            extract: true,
            debuggable: false,
            compat_16kb_disabled: false,
            manifest_compat_disabled: false,
        }
    }
    fn owner() -> GuestInode {
        GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: None,
        }
    }
    fn timestamp(_: u32) -> Result<SystemTime, String> {
        Ok(UNIX_EPOCH + std::time::Duration::from_secs(1735787046))
    }

    #[test]
    fn extraction_preserves_content_metadata_and_reuses_only_matching_files() {
        for method in [0, 8] {
            let dir = Directory::new();
            let apk = archive(method, b"native payload");
            let before = apk.clone();
            let output = dir.0.join("libx.so");
            let result = policy()
                .copy_to(&apk, "arm64-v8a", &dir.0, owner(), &timestamp)
                .unwrap();
            assert_eq!(result.extracted.len(), 1);
            assert_eq!(fs::read(&output).unwrap(), b"native payload");
            assert_eq!(
                fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                0o755
            );
            assert_eq!(
                fs::metadata(&output).unwrap().modified().unwrap(),
                timestamp(0).unwrap()
            );
            assert_eq!(
                aim_storage::guest_inode::read(&output).unwrap(),
                Some(GuestInode {
                    mode: Some(0o755),
                    ..owner()
                })
            );
            let inode = fs::metadata(&output).unwrap().ino();
            assert_eq!(
                policy()
                    .copy_to(&apk, "arm64-v8a", &dir.0, owner(), &timestamp)
                    .unwrap()
                    .reused
                    .len(),
                1
            );
            assert_eq!(fs::metadata(&output).unwrap().ino(), inode);
            fs::write(&output, b"broken payload").unwrap();
            File::open(&output)
                .unwrap()
                .set_times(FileTimes::new().set_modified(timestamp(0).unwrap()))
                .unwrap();
            assert_eq!(
                policy()
                    .copy_to(&apk, "arm64-v8a", &dir.0, owner(), &timestamp)
                    .unwrap()
                    .extracted
                    .len(),
                1
            );
            assert_eq!(fs::read(&output).unwrap(), b"native payload");
            assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
            assert_eq!(apk, before);
        }
    }

    #[test]
    fn extraction_failure_preserves_existing_file_and_removes_temporary_output() {
        let dir = Directory::new();
        let output = dir.0.join("libx.so");
        fs::write(&output, b"existing").unwrap();
        let mut apk = archive(8, b"new native payload");
        let archive = Archive::open(&apk).unwrap();
        let payload_offset = archive.data_offset(&archive.entries[0]).unwrap() as usize;
        apk[payload_offset] = 255;
        assert_eq!(
            policy()
                .copy_to(&apk, "arm64-v8a", &dir.0, owner(), &timestamp)
                .unwrap_err()
                .code,
            -18
        );
        assert_eq!(fs::read(&output).unwrap(), b"existing");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
        assert_eq!(
            policy()
                .copy_to(&apk, "arm64-v8a", &dir.0, owner(), &|_| Err(
                    "guest clock unavailable".into()
                ))
                .unwrap_err()
                .code,
            -110
        );
        let mut direct = policy();
        direct.extract = false;
        assert_eq!(
            direct
                .copy_to(&apk, "arm64-v8a", &dir.0, owner(), &timestamp)
                .unwrap_err()
                .code,
            -2
        );
        let link = dir.0.join("link");
        std::os::unix::fs::symlink(&dir.0, &link).unwrap();
        assert_eq!(
            policy()
                .copy_to(&apk, "arm64-v8a", &link, owner(), &timestamp)
                .unwrap_err()
                .code,
            -18
        );
    }
}

fn file_crc(path: &Path) -> io::Result<u32> {
    let mut file = File::open(path)?;
    let mut crc = crc32fast::Hasher::new();
    let mut buffer = [0; 16384];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(crc.finalize());
        }
        crc.update(&buffer[..read]);
    }
}

fn temporary_file(directory: &Path) -> io::Result<(PathBuf, File)> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    loop {
        let path = directory.join(format!(
            ".tmp{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}
