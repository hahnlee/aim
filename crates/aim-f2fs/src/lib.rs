//! `F2fsUtils.releaseCompressedBlocks` (`android-16.0.0_r1`, with its JNI
//! `nativeReleaseCompressedBlocks`): the compressed blocks of an installed
//! package's files given back to the free pool, where the kernel and
//! `/data` support f2fs compression, read here in the guest where the
//! original reads them.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::fs;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// `sKernelFeatures`, `sUserDataFeatures`, `sDataDirectory`.
const KERNEL_FEATURES: &str = "/sys/fs/f2fs/features";
const USER_DATA_FEATURES: &str = "/dev/sys/fs/by-name/userdata/features";
const DATA_DIRECTORY: &str = "/data";
const COMPRESSION_FEATURE: &str = "compression";
/// `FS_IOC_GETFLAGS` (`_IOR('f', 1, long)`), `FS_COMPR_FL`,
/// `F2FS_IOC_RELEASE_COMPRESS_BLOCKS` (`_IOR(0xf5, 18, __u64)`).
const FS_IOC_GETFLAGS: libc::c_ulong = 0x8008_6601;
const FS_COMPR_FL: libc::c_long = 0x4;
const F2FS_IOC_RELEASE_COMPRESS_BLOCKS: libc::c_ulong = 0x8008_f512;
/// incfs's `INCFS_MAGIC_NUMBER` (`IncFs_IsIncFsPath`).
const INCFS_MAGIC_NUMBER: i64 = 0x5346_414e;

/// `releaseCompressedBlocks(resolver, file)` for each path, last first as
/// the boot lifecycle leaf calls it; `release` is
/// `Secure.RELEASE_COMPRESS_BLOCKS_ON_INSTALL` (default 1) as the host
/// read it.
pub fn release(paths: &[String], release: bool) {
    if !kernel_compression(Path::new(KERNEL_FEATURES))
        || !user_data_compression(Path::new(USER_DATA_FEATURES))
        || !release
    {
        return;
    }
    for path in paths.iter().rev() {
        let path = Path::new(path);
        if !compression_allowed(path) {
            continue;
        }
        let Some(files) = files_to_release(path) else {
            continue;
        };
        for file in files.iter().rev() {
            release_file(file);
        }
    }
}

/// `isCompressionEnabledInKernel`: a feature named `compression`.
fn kernel_compression(features: &Path) -> bool {
    fs::read_dir(features).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|e| e.file_name() == COMPRESSION_FEATURE)
    })
}

/// `isCompressionEnabledOnUserData`: a single line of comma-separated
/// features naming `compression`.
fn user_data_compression(features: &Path) -> bool {
    if !features.is_file() {
        return false;
    }
    fs::read_to_string(features).is_ok_and(|text| features_name_compression(&text))
}

/// `Files.readAllLines`'s lines: exactly one, not empty, naming
/// `compression`.
fn features_name_compression(text: &str) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() != 1 || lines[0].is_empty() {
        return false;
    }
    lines[0]
        .split(',')
        .any(|feature| feature.trim() == COMPRESSION_FEATURE)
}

/// `isCompressionAllowed`: a canonical path under `/data`, not on incfs.
fn compression_allowed(file: &Path) -> bool {
    let Ok(path) = fs::canonicalize(file) else {
        return false;
    };
    if incremental(&path) {
        return false;
    }
    let Ok(data) = fs::canonicalize(DATA_DIRECTORY) else {
        return false;
    };
    is_child(&data, &path)
}

/// `isChild`: `base` is the path or one of its parents.
fn is_child(base: &Path, path: &Path) -> bool {
    path.ancestors().any(|parent| parent == base)
}

/// `IncrementalManager.isIncrementalPath`.
fn incremental(path: &Path) -> bool {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
        return false;
    };
    // SAFETY: statfs into a zeroed local of its type.
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    unsafe { libc::statfs(c.as_ptr(), &mut stat) == 0 && stat.f_type as i64 == INCFS_MAGIC_NUMBER }
}

/// `getFilesToRelease`: the files under a directory at any depth, or the
/// path itself when it is a file; none for an empty directory.
fn files_to_release(path: &Path) -> Option<Vec<PathBuf>> {
    match files_recursive(path) {
        None if path.is_file() => Some(vec![path.to_path_buf()]),
        None => None,
        Some(files) if files.is_empty() => None,
        Some(files) => Some(files),
    }
}

/// `getFilesRecursive`: none when the path cannot be listed.
fn files_recursive(path: &Path) -> Option<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(path).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(files_recursive(&path).unwrap_or_default());
        } else if path.is_file() {
            files.push(path);
        }
    }
    Some(files)
}

/// `nativeReleaseCompressedBlocks`: a compressed file's blocks released.
fn release_file(path: &Path) {
    let Ok(file) = fs::File::open(path) else {
        log::warn!("Failed to open file: {}", path.display());
        return;
    };
    let mut flags: libc::c_long = 0;
    // SAFETY: ioctls on an open descriptor with locals of their types.
    unsafe {
        if libc::ioctl(file.as_raw_fd(), FS_IOC_GETFLAGS as _, &mut flags) < 0 {
            log::warn!("Failed to get flags for file: {}", path.display());
            return;
        }
        if flags & FS_COMPR_FL == 0 {
            return;
        }
        let mut blocks: u64 = 0;
        libc::ioctl(
            file.as_raw_fd(),
            F2FS_IOC_RELEASE_COMPRESS_BLOCKS as _,
            &mut blocks,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_data_features_are_one_line_naming_compression() {
        assert!(features_name_compression(
            "extra_attr, compression,verity\n"
        ));
        assert!(!features_name_compression("extra_attr,verity\n"));
        assert!(!features_name_compression("compression\nverity\n"));
        assert!(!features_name_compression("\n"));
    }

    #[test]
    fn a_child_is_the_base_or_under_it() {
        assert!(is_child(
            Path::new("/data"),
            Path::new("/data/app/x/base.apk")
        ));
        assert!(is_child(Path::new("/data"), Path::new("/data")));
        assert!(!is_child(
            Path::new("/data"),
            Path::new("/system/app/x.apk")
        ));
        assert!(!is_child(Path::new("/data"), Path::new("/database")));
    }

    #[test]
    fn a_kernel_without_the_feature_releases_nothing() {
        let dir = std::env::temp_dir().join(format!("f2fs-{}", std::process::id()));
        fs::create_dir_all(dir.join("verity")).unwrap();
        assert!(!kernel_compression(&dir));
        fs::create_dir_all(dir.join(COMPRESSION_FEATURE)).unwrap();
        assert!(kernel_compression(&dir));
        assert!(!kernel_compression(&dir.join("absent")));
        fs::remove_dir_all(&dir).unwrap();
    }
}
