//! NativeLibraryHelper.findSupportedAbi/hasRenderscriptBitcode and
//! ApkParsing.ValidLibraryPathLastSlash at android-16.0.0_r1 (#810).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::NativeLibraryInstallError;
use android_image_extract::{source::ReadAt, zip::Archive};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ZipNativeLibraries {
    pub abis: BTreeSet<Vec<u8>>,
    pub renderscript_bitcode: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupportedAbi {
    None,
    NoMatch,
    Index(usize),
}

/// Guest page size and the original installation owner's policy inputs.
#[derive(Clone, Copy, Debug)]
pub struct NativeLibraryInstallPolicy {
    pub page_size: u64,
    pub extract: bool,
    pub debuggable: bool,
    pub compat_16kb_disabled: bool,
    pub manifest_compat_disabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeLibraryEntry {
    pub index: usize,
    pub name: Vec<u8>,
    pub offset: u64,
    pub extract: bool,
}

impl NativeLibraryInstallPolicy {
    /// NativeLibraryHelper.copyFileIfChanged's admission and extraction rules.
    /// This reads the APK only; a plan is not proof of completed extraction.
    pub fn inspect(
        self,
        source: &dyn ReadAt,
        abi: &str,
    ) -> Result<Vec<NativeLibraryEntry>, NativeLibraryInstallError> {
        let invalid = |message| NativeLibraryInstallError::new(-2, message);
        if self.page_size < 4096 || !self.page_size.is_power_of_two() {
            return Err(NativeLibraryInstallError::new(
                -110,
                "invalid guest page size",
            ));
        }
        let archive = Archive::open(source).map_err(|e| invalid(e.to_string()))?;
        let mut entries = Vec::new();
        for (index, entry) in archive.entries.iter().enumerate() {
            if entry.name.contains(&0) {
                return Err(invalid("APK ZIP entry name contains NUL".into()));
            }
            if entry.name.len() >= 4096 || library_abi(&entry.name) != Some(abi.as_bytes()) {
                continue;
            }
            let file = entry.name.rsplit(|b| *b == b'/').next().unwrap();
            let offset = archive
                .data_offset(entry)
                .map_err(|e| invalid(e.to_string()))?;
            let mut extract = self.extract || (self.debuggable && file == b"wrap.sh");
            if !extract {
                if entry.method != 0 {
                    return Err(invalid(
                        "native library is compressed with extractNativeLibs=false".into(),
                    ));
                }
                // Also reject inconsistent stored lengths before direct mapping.
                archive.stored(entry).map_err(|e| invalid(e.to_string()))?;
                if offset % self.page_size != 0 {
                    extract = self.page_size == 16384
                        && !self.compat_16kb_disabled
                        && !self.manifest_compat_disabled
                        && offset % 4096 == 0;
                    if !extract {
                        return Err(invalid("native library is not guest-page-aligned".into()));
                    }
                }
            }
            entries.push(NativeLibraryEntry {
                index,
                name: entry.name.clone(),
                offset,
                extract,
            });
        }
        Ok(entries)
    }
}

impl ZipNativeLibraries {
    /// Reads only ZIP inventory, without extracting or changing an APK.
    pub fn read(source: &dyn ReadAt) -> Result<Self, String> {
        let archive = Archive::open(source).map_err(|e| e.to_string())?;
        let mut inventory = Self::default();
        for entry in &archive.entries {
            let name = &entry.name;
            if name.contains(&0) {
                return Err("APK ZIP entry name contains NUL".into());
            }
            // Original getEntryFileName uses a PATH_MAX-sized C buffer.
            if name.len() >= 4096 {
                continue;
            }
            if let Some(abi) = library_abi(name) {
                inventory.abis.insert(abi.to_vec());
            }
            if name.ends_with(b".bc") {
                // The pinned JNI uses fileName + 1 when any slash is present.
                let base = if name.contains(&b'/') {
                    &name[1..]
                } else {
                    name
                };
                inventory.renderscript_bitcode |= filename_safe(base);
            }
        }
        Ok(inventory)
    }

    pub fn merge(&mut self, other: Self) {
        self.abis.extend(other.abis);
        self.renderscript_bitcode |= other.renderscript_bitcode;
    }

    /// The handle's base/split union uses the lowest supported-list index;
    /// an unmatched split does not discard another split's matching ABI.
    pub fn find_supported_abi(&self, supported: &[String]) -> SupportedAbi {
        if self.abis.is_empty() {
            SupportedAbi::None
        } else if let Some(at) = supported
            .iter()
            .position(|a| self.abis.contains(a.as_bytes()))
        {
            SupportedAbi::Index(at)
        } else {
            SupportedAbi::NoMatch
        }
    }
}

impl crate::package::write::Apks {
    /// Validate every base/split before the installation owner changes files.
    pub fn native_library_install_plan(
        &self,
        pkg: &crate::package::pkg::AndroidPackage,
        abi: &str,
        policy: NativeLibraryInstallPolicy,
    ) -> Result<Vec<(String, Vec<NativeLibraryEntry>)>, NativeLibraryInstallError> {
        let base = pkg
            .base_apk_path
            .as_deref()
            .ok_or_else(|| NativeLibraryInstallError::new(-110, "missing base APK path"))?;
        let mut paths = vec![base];
        if let Some(splits) = &pkg.split_code_paths {
            for split in splits {
                paths.push(
                    split.as_deref().ok_or_else(|| {
                        NativeLibraryInstallError::new(-110, "null split APK path")
                    })?,
                );
            }
        }
        paths
            .into_iter()
            .map(|path| {
                let host = (self.files)(path).ok_or_else(|| {
                    NativeLibraryInstallError::new(-2, format!("unmapped APK: {path}"))
                })?;
                let source = android_image_extract::source::FileSource::open(&host)
                    .map_err(|e| NativeLibraryInstallError::new(-2, format!("{path}: {e}")))?;
                let entries = policy.inspect(&source, abi).map_err(|e| {
                    NativeLibraryInstallError::new(e.code, format!("{path}: {}", e.message))
                })?;
                Ok((path.to_owned(), entries))
            })
            .collect()
    }

    pub fn zip_native_libraries(
        &self,
        pkg: &crate::package::pkg::AndroidPackage,
    ) -> Result<ZipNativeLibraries, String> {
        let base = pkg
            .base_apk_path
            .as_deref()
            .ok_or("missing base APK path")?;
        let mut paths = vec![base];
        if let Some(splits) = &pkg.split_code_paths {
            for split in splits {
                paths.push(split.as_deref().ok_or("null split APK path")?);
            }
        }
        let mut inventory = ZipNativeLibraries::default();
        for path in paths {
            let host = (self.files)(path).ok_or_else(|| format!("unmapped APK: {path}"))?;
            let source = android_image_extract::source::FileSource::open(&host)
                .map_err(|e| format!("{path}: {e}"))?;
            inventory.merge(ZipNativeLibraries::read(&source).map_err(|e| format!("{path}: {e}"))?);
        }
        Ok(inventory)
    }
}

pub(super) fn library_abi(name: &[u8]) -> Option<&[u8]> {
    if name.len() < 13 {
        return None;
    }
    let tail = name.strip_prefix(b"lib/")?;
    let slash = tail.iter().position(|b| *b == b'/')?;
    let file = &tail[slash + 1..];
    if file.is_empty() || file.contains(&b'/') || !filename_safe(file) {
        None
    } else {
        Some(&tail[..slash])
    }
}

fn filename_safe(name: &[u8]) -> bool {
    name.iter()
        .all(|b| b.is_ascii_alphanumeric() || b"+,-./=_".contains(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Empty ZIP payloads isolate ZIP admission from ELF/content extraction.
    fn archive(name: &[u8], offset: usize, method: u16) -> Vec<u8> {
        let extra = offset - 30 - name.len();
        let mut bytes = vec![0; 30];
        bytes[..4].copy_from_slice(&0x04034b50u32.to_le_bytes());
        bytes[8..10].copy_from_slice(&method.to_le_bytes());
        bytes[26..28].copy_from_slice(&(name.len() as u16).to_le_bytes());
        bytes[28..30].copy_from_slice(&(extra as u16).to_le_bytes());
        bytes.extend_from_slice(name);
        bytes.resize(offset, 0);
        let mut central = vec![0; 46];
        central[..4].copy_from_slice(&0x02014b50u32.to_le_bytes());
        central[10..12].copy_from_slice(&method.to_le_bytes());
        central[28..30].copy_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(&central);
        bytes.extend_from_slice(name);
        let mut end = vec![0; 22];
        end[..4].copy_from_slice(&0x06054b50u32.to_le_bytes());
        end[8..10].copy_from_slice(&1u16.to_le_bytes());
        end[10..12].copy_from_slice(&1u16.to_le_bytes());
        end[12..16].copy_from_slice(&((46 + name.len()) as u32).to_le_bytes());
        end[16..20].copy_from_slice(&(offset as u32).to_le_bytes());
        bytes.extend_from_slice(&end);
        bytes
    }

    fn install_policy(page_size: u64) -> NativeLibraryInstallPolicy {
        NativeLibraryInstallPolicy {
            page_size,
            extract: false,
            debuggable: false,
            compat_16kb_disabled: false,
            manifest_compat_disabled: false,
        }
    }

    #[test]
    fn direct_apk_mapping_requires_stored_guest_page_aligned_entries() {
        for page in [4096, 16384, 65536] {
            let policy = install_policy(page);
            let apk = archive(b"lib/arm64-v8a/libx.so", page as usize, 0);
            let before = apk.clone();
            assert!(!policy.inspect(&apk, "arm64-v8a").unwrap()[0].extract);
            assert_eq!(apk, before);
            assert!(
                policy
                    .inspect(
                        &archive(b"lib/arm64-v8a/libx.so", page as usize, 8),
                        "arm64-v8a"
                    )
                    .is_err()
            );
            assert!(
                policy
                    .inspect(
                        &archive(b"lib/arm64-v8a/libx.so", page as usize + 1, 0),
                        "arm64-v8a"
                    )
                    .is_err()
            );
        }
        assert!(install_policy(0).inspect(&vec![], "arm64-v8a").is_err());
    }

    #[test]
    fn compatibility_and_debug_wrap_extraction_follow_original_policy() {
        let apk = archive(b"lib/arm64-v8a/libx.so", 4096, 0);
        let mut policy = install_policy(16384);
        assert!(policy.inspect(&apk, "arm64-v8a").unwrap()[0].extract);
        policy.manifest_compat_disabled = true;
        assert!(policy.inspect(&apk, "arm64-v8a").is_err());
        policy.manifest_compat_disabled = false;
        policy.compat_16kb_disabled = true;
        assert!(policy.inspect(&apk, "arm64-v8a").is_err());
        policy.debuggable = true;
        let wrap = archive(b"lib/arm64-v8a/wrap.sh", 100, 8);
        assert!(policy.inspect(&wrap, "arm64-v8a").unwrap()[0].extract);
        assert!(policy.inspect(&wrap, "x86").unwrap().is_empty());
        policy.debuggable = false;
        assert!(policy.inspect(&wrap, "arm64-v8a").is_err());
        policy.extract = true;
        assert!(policy.inspect(&wrap, "arm64-v8a").unwrap()[0].extract);
        let mut corrupt = apk;
        corrupt[0] = 0;
        assert!(policy.inspect(&corrupt, "arm64-v8a").is_err());
        assert!(
            policy
                .inspect(&archive(b"lib/arm64-v8a/nu\0l.so", 100, 0), "x86")
                .is_err()
        );
    }
    #[test]
    fn original_name_rules_include_non_so_files_and_reject_nested_or_unsafe_files() {
        for name in [
            b"lib/x86/gdbserver".as_slice(),
            b"lib/x86/anything",
            b"lib/x86/libx.so",
        ] {
            assert_eq!(library_abi(name), Some(b"x86".as_slice()));
        }
        for name in [
            b"lib/x86/".as_slice(),
            b"lib/x86/sub/libx.so",
            b"lib/x86/lib x.so",
            b"assets/x86/libx.so",
            b"lib/a/x",
        ] {
            assert_eq!(library_abi(name), None);
        }
    }
    #[test]
    fn split_union_retains_best_preference_and_distinguishes_absent_from_unmatched() {
        let supported = vec!["arm64-v8a".into(), "x86".into()];
        let mut inventory = ZipNativeLibraries::default();
        assert_eq!(inventory.find_supported_abi(&supported), SupportedAbi::None);
        inventory.abis.insert(b"unknown".to_vec());
        assert_eq!(
            inventory.find_supported_abi(&supported),
            SupportedAbi::NoMatch
        );
        inventory.merge(ZipNativeLibraries {
            abis: BTreeSet::from([b"x86".to_vec()]),
            renderscript_bitcode: true,
        });
        assert_eq!(
            inventory.find_supported_abi(&supported),
            SupportedAbi::Index(1)
        );
        inventory.merge(ZipNativeLibraries {
            abis: BTreeSet::from([b"arm64-v8a".to_vec()]),
            ..Default::default()
        });
        assert_eq!(
            inventory.find_supported_abi(&supported),
            SupportedAbi::Index(0)
        );
        assert_eq!(inventory.find_supported_abi(&[]), SupportedAbi::NoMatch);
        assert!(inventory.renderscript_bitcode);
        assert!(ZipNativeLibraries::read(&vec![0; 40]).is_err());
    }
}
