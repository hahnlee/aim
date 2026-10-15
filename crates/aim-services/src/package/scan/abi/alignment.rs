//! NativeLibraryHelper.checkAlignment/getLoadSegmentPhdrs at android-16.0.0_r1 (#810).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{
    NativeLibraryInstallPolicy, NativeLibraryPaths, SupportedAbi, ZipNativeLibraries,
    instruction_set, zip::library_abi,
};
use android_image_extract::{
    source::{FileSource, ReadAt},
    zip::Archive,
};
use std::path::Path;

impl crate::package::write::Apks {
    /// NativeLibraryHelper.checkAlignmentForCompatMode: choose the first image
    /// 64-bit ABI across all APKs, then combine flags. The original ignores its
    /// ABI override argument here; install ABI policy is a separate phase.
    pub fn native_library_alignment(
        &self,
        pkg: &crate::package::pkg::AndroidPackage,
        supported_64: &[String],
        paths: &NativeLibraryPaths,
        policy: NativeLibraryInstallPolicy,
    ) -> Result<u32, String> {
        let base = pkg
            .base_apk_path
            .as_deref()
            .ok_or("missing base APK path")?;
        let mut apk_paths = vec![base];
        if let Some(splits) = &pkg.split_code_paths {
            for split in splits {
                apk_paths.push(split.as_deref().ok_or("null split APK path")?);
            }
        }
        let mut inventory = ZipNativeLibraries::default();
        let mut sources = Vec::new();
        for path in apk_paths {
            let host = (self.files)(path).ok_or_else(|| format!("unmapped APK: {path}"))?;
            let source = FileSource::open(&host).map_err(|e| format!("{path}: {e}"))?;
            inventory.merge(ZipNativeLibraries::read(&source).map_err(|e| format!("{path}: {e}"))?);
            sources.push(source);
        }
        let SupportedAbi::Index(at) = inventory.find_supported_abi(supported_64) else {
            return Err("no supported 64-bit native library ABI".into());
        };
        let abi = &supported_64[at];
        let directory = if paths.requires_isa {
            format!("{}/{}", paths.root, instruction_set(abi)?)
        } else {
            paths.root.clone()
        };
        let host = if policy.extract {
            (self.files)(&directory)
                .ok_or_else(|| format!("unmapped native library directory: {directory}"))?
        } else {
            directory.into()
        };
        let mut flags = 0;
        for source in sources {
            flags |= policy.check_alignment(&source, abi, &host)?;
        }
        Ok(flags)
    }
}

impl NativeLibraryInstallPolicy {
    /// Returns ApplicationInfo page-size compatibility flags (ZIP: 2, ELF: 4).
    /// Errors correspond to the original PAGE_SIZE_APP_COMPAT_FLAG_ERROR (-1).
    /// This diagnostic is separate from install admission and never writes files.
    pub fn check_alignment(
        self,
        source: &dyn ReadAt,
        abi: &str,
        native_directory: &Path,
    ) -> Result<u32, String> {
        if self.page_size < 4096 || !self.page_size.is_power_of_two() {
            return Err("invalid guest page size".into());
        }
        let archive = Archive::open(source).map_err(|e| e.to_string())?;
        let mut flags = 0;
        for entry in &archive.entries {
            if entry.name.contains(&0) {
                return Err("APK ZIP entry name contains NUL".into());
            }
            if entry.name.len() >= 4096 || library_abi(&entry.name) != Some(abi.as_bytes()) {
                continue;
            }
            if self.extract {
                let file = std::str::from_utf8(entry.name.rsplit(|b| *b == b'/').next().unwrap())
                    .map_err(|e| e.to_string())?;
                let native =
                    FileSource::open(&native_directory.join(file)).map_err(|e| e.to_string())?;
                flags |= load_alignment(&native, 0)?;
            } else {
                if entry.method != 0 {
                    return Err("native library is compressed with extractNativeLibs=false".into());
                }
                let offset = archive.data_offset(entry).map_err(|e| e.to_string())?;
                if offset % self.page_size != 0 {
                    flags |= 2;
                }
                flags |= load_alignment(source, offset)?;
            }
        }
        Ok(flags)
    }
}

fn load_alignment(source: &dyn ReadAt, offset: u64) -> Result<u32, String> {
    let header = source.read_vec(offset, 64).map_err(|e| e.to_string())?;
    // The pinned helper checks EI_CLASS only; ELF validity belongs to the loader.
    if header[4] != 2 {
        return Ok(0);
    }
    let program_offset = u64::from_le_bytes(header[32..40].try_into().unwrap());
    let count = u16::from_le_bytes(header[56..58].try_into().unwrap());
    let mut position = offset
        .checked_add(program_offset)
        .ok_or("ELF program-header offset overflow")?;
    let mut flags = 0;
    for _ in 0..count {
        let program = source.read_vec(position, 56).map_err(|e| e.to_string())?;
        if u32::from_le_bytes(program[..4].try_into().unwrap()) == 1
            && u64::from_le_bytes(program[48..56].try_into().unwrap()) == 4096
        {
            flags |= 4;
        }
        position = position
            .checked_add(56)
            .ok_or("ELF program-header offset overflow")?;
    }
    Ok(flags)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn elf(class: u8, alignments: &[(u32, u64)]) -> Vec<u8> {
        let mut elf = vec![0; 64];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = class;
        elf[32..40].copy_from_slice(&64u64.to_le_bytes());
        elf[56..58].copy_from_slice(&(alignments.len() as u16).to_le_bytes());
        for (kind, alignment) in alignments {
            let mut program = vec![0; 56];
            program[..4].copy_from_slice(&kind.to_le_bytes());
            program[48..56].copy_from_slice(&alignment.to_le_bytes());
            elf.extend_from_slice(&program);
        }
        elf
    }
    #[test]
    fn only_4k_load_segments_request_elf_compatibility() {
        for (class, programs, expected) in [
            (2, vec![(1, 4096)], 4),
            (2, vec![(1, 16384)], 0),
            (2, vec![(2, 4096)], 0),
            (2, vec![(1, 65536), (1, 4096)], 4),
            (1, vec![(1, 4096)], 0),
            (0, vec![(1, 4096)], 0),
        ] {
            assert_eq!(load_alignment(&elf(class, &programs), 0).unwrap(), expected);
        }
        let mut apk = vec![0; 4096];
        apk.extend_from_slice(&elf(2, &[(1, 4096)]));
        assert_eq!(load_alignment(&apk, 4096).unwrap(), 4);
    }
    #[test]
    fn truncated_headers_and_offset_overflow_override_partial_flags() {
        assert!(load_alignment(&vec![0; 63], 0).is_err());
        let mut truncated = elf(2, &[(1, 4096)]);
        truncated[56..58].copy_from_slice(&2u16.to_le_bytes());
        assert!(load_alignment(&truncated, 0).is_err());
        let mut overflow = vec![0; 100];
        let mut header = elf(2, &[]);
        header[32..40].copy_from_slice(&u64::MAX.to_le_bytes());
        overflow.extend_from_slice(&header);
        assert!(load_alignment(&overflow, 100).is_err());
    }
}
