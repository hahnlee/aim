//! NativeLibraryHelper.findSupportedAbi/hasRenderscriptBitcode and
//! ApkParsing.ValidLibraryPathLastSlash at android-16.0.0_r1 (#810).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
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

fn library_abi(name: &[u8]) -> Option<&[u8]> {
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
