//! NativeLibraryHelper non-incremental package copy at android-16.0.0_r1 (#810).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{
    AbiPolicy, NativeLibraryCopy, NativeLibraryInstallError, NativeLibraryInstallPolicy,
    SupportedAbi, ZipNativeLibraries, instruction_set,
};
use crate::package::{
    pkg::{AndroidPackage, booleans},
    write::Apks,
};
use aim_storage::guest_inode::{GuestInode, record};
use android_image_extract::source::FileSource;
use std::os::unix::fs::PermissionsExt;
use std::{fs, path::Path, time::SystemTime};

/// The filesystem/clock owners supply a checked writable root and restorecon.
/// APK read mapping is never used to obtain a writable destination.
pub struct NativeLibraryDestination<'a> {
    pub root: &'a Path,
    pub owner: GuestInode,
    pub zip_time: &'a dyn Fn(u32) -> Result<SystemTime, String>,
    pub restorecon: &'a dyn Fn(&Path) -> Result<(), String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct NativeLibraryAbiCopy {
    pub abi_index: usize,
    pub abi: String,
    pub copies: Vec<(String, NativeLibraryCopy)>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct NativeLibraryPackageCopy {
    pub ignored_override: bool,
    pub abis: Vec<NativeLibraryAbiCopy>,
}

struct Inputs {
    inventory: ZipNativeLibraries,
    sources: Vec<(String, FileSource)>,
}

impl Inputs {
    fn read(apks: &Apks, pkg: &AndroidPackage) -> Result<Self, NativeLibraryInstallError> {
        let fail = |message| NativeLibraryInstallError::new(-2, message);
        let mut paths = vec![
            pkg.base_apk_path
                .as_deref()
                .ok_or_else(|| fail("missing base APK path".into()))?,
        ];
        if let Some(splits) = &pkg.split_code_paths {
            for split in splits {
                paths.push(
                    split
                        .as_deref()
                        .ok_or_else(|| fail("null split APK path".into()))?,
                );
            }
        }
        let mut inputs = Self {
            inventory: ZipNativeLibraries::default(),
            sources: Vec::new(),
        };
        for path in paths {
            let host = (apks.files)(path).ok_or_else(|| fail(format!("unmapped APK: {path}")))?;
            let source = FileSource::open(&host).map_err(|e| fail(format!("{path}: {e}")))?;
            inputs.inventory.merge(
                ZipNativeLibraries::read(&source).map_err(|e| fail(format!("{path}: {e}")))?,
            );
            inputs.sources.push((path.into(), source));
        }
        Ok(inputs)
    }

    fn copy(
        &self,
        supported: &[String],
        use_isa: bool,
        policy: NativeLibraryInstallPolicy,
        destination: &NativeLibraryDestination<'_>,
    ) -> Result<NativeLibraryAbiCopy, NativeLibraryInstallError> {
        let at = match self.inventory.find_supported_abi(supported) {
            SupportedAbi::Index(at) => at,
            SupportedAbi::None => {
                return Err(NativeLibraryInstallError::new(-114, "no native libraries"));
            }
            SupportedAbi::NoMatch => {
                return Err(NativeLibraryInstallError::new(
                    -113,
                    "no matching native library ABI",
                ));
            }
        };
        let abi = &supported[at];
        let isa = instruction_set(abi).map_err(|e| NativeLibraryInstallError::new(-110, e))?;
        create_directory(destination.root, destination)?;
        let subdir = if use_isa {
            destination.root.join(isa)
        } else {
            destination.root.to_owned()
        };
        if use_isa {
            create_directory(&subdir, destination)?;
        }
        let mut copies = Vec::new();
        for (path, source) in &self.sources {
            let copy = policy
                .copy_to(
                    source,
                    abi,
                    &subdir,
                    destination.owner,
                    destination.zip_time,
                )
                .map_err(|e| {
                    NativeLibraryInstallError::new(e.code, format!("{path}: {}", e.message))
                })?;
            copies.push((path.clone(), copy));
        }
        Ok(NativeLibraryAbiCopy {
            abi_index: at,
            abi: abi.clone(),
            copies,
        })
    }
}

impl Apks {
    /// copyNativeBinariesForSupportedAbi for ordinary filesystem storage.
    /// Incremental installs require the incremental filesystem owner (#798).
    pub fn copy_native_libraries_for_supported_abi(
        &self,
        pkg: &AndroidPackage,
        supported: &[String],
        use_isa: bool,
        policy: NativeLibraryInstallPolicy,
        destination: &NativeLibraryDestination<'_>,
    ) -> Result<NativeLibraryAbiCopy, NativeLibraryInstallError> {
        Inputs::read(self, pkg)?.copy(supported, use_isa, policy, destination)
    }

    /// copyNativeBinariesWithOverride: narrow then wide for multiarch; override
    /// and RenderScript selection for single-ABI packages. Reports an ignored
    /// override so the caller retains the original diagnostic.
    pub fn copy_native_libraries_with_override(
        &self,
        pkg: &AndroidPackage,
        abis: &AbiPolicy,
        override_abi: Option<&str>,
        policy: NativeLibraryInstallPolicy,
        destination: &NativeLibraryDestination<'_>,
    ) -> Result<NativeLibraryPackageCopy, NativeLibraryInstallError> {
        let inputs = Inputs::read(self, pkg)?;
        let override_abi = override_abi.filter(|a| *a != "-");
        let multi = pkg.is(booleans::MULTI_ARCH);
        let mut result = NativeLibraryPackageCopy {
            ignored_override: multi && override_abi.is_some(),
            abis: Vec::new(),
        };
        if multi {
            for supported in [&abis.bit32, &abis.bit64] {
                if supported.is_empty() {
                    continue;
                }
                match inputs.copy(supported, true, policy, destination) {
                    Ok(copy) => result.abis.push(copy),
                    Err(error) if error.code == -113 || error.code == -114 => {}
                    Err(error) => return Err(error),
                }
            }
        } else {
            let overridden = override_abi.map(|abi| vec![abi.to_owned()]);
            let supported = if overridden.is_none()
                && !abis.bit64.is_empty()
                && inputs.inventory.renderscript_bitcode
            {
                &abis.bit32
            } else {
                overridden.as_ref().unwrap_or(&abis.all)
            };
            match inputs.copy(supported, true, policy, destination) {
                Ok(copy) => result.abis.push(copy),
                Err(error) if error.code == -114 => {}
                Err(error) => return Err(error),
            }
        }
        Ok(result)
    }
}

fn create_directory(
    path: &Path,
    destination: &NativeLibraryDestination<'_>,
) -> Result<(), NativeLibraryInstallError> {
    let fail = |message| NativeLibraryInstallError::new(-110, message);
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => return (destination.restorecon)(path).map_err(fail),
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(fail(
                "native directory was not resolved by its filesystem owner".into(),
            ));
        }
        Ok(_) => fs::remove_file(path).map_err(|e| fail(e.to_string()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(fail(e.to_string())),
    }
    fs::create_dir(path).map_err(|e| fail(e.to_string()))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|e| fail(e.to_string()))?;
    record(
        path,
        GuestInode {
            mode: Some(0o755),
            ..destination.owner
        },
    )
    .map_err(|e| fail(e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::Cell,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    struct Data(PathBuf);
    impl Data {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "aim-native-package-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Data {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    #[test]
    fn directory_owner_creates_metadata_and_preserves_existing_mode() {
        let data = Data::new();
        let root = data.0.join("lib");
        let calls = Cell::new(0);
        let restore = |path: &Path| {
            assert_eq!(path, root);
            calls.set(calls.get() + 1);
            Ok(())
        };
        let clock = |_| Err("unexpected clock read".into());
        let destination = NativeLibraryDestination {
            root: &root,
            owner: GuestInode {
                uid: Some(1000),
                gid: Some(1000),
                mode: None,
            },
            zip_time: &clock,
            restorecon: &restore,
        };
        fs::write(&root, b"old non-directory").unwrap();
        create_directory(&root, &destination).unwrap();
        assert_eq!(calls.get(), 0);
        assert!(root.is_dir());
        assert_eq!(
            recorded(&root),
            GuestInode {
                uid: Some(1000),
                gid: Some(1000),
                mode: Some(0o755)
            }
        );
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        record(
            &root,
            GuestInode {
                mode: Some(0o700),
                ..GuestInode::default()
            },
        )
        .unwrap();
        create_directory(&root, &destination).unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(recorded(&root).mode, Some(0o700));
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    fn recorded(path: &Path) -> GuestInode {
        aim_storage::guest_inode::read(path).unwrap().unwrap()
    }
    #[test]
    fn unresolved_paths_and_restorecon_failures_are_explicit() {
        let data = Data::new();
        let root = data.0.join("lib");
        fs::create_dir(&root).unwrap();
        let restore = |_: &Path| Err("restorecon denied".into());
        let clock = |_| Err("unexpected clock read".into());
        let destination = NativeLibraryDestination {
            root: &root,
            owner: GuestInode::default(),
            zip_time: &clock,
            restorecon: &restore,
        };
        let before = fs::metadata(&root).unwrap().permissions().mode();
        let error = create_directory(&root, &destination).unwrap_err();
        assert_eq!(error.code, -110);
        assert_eq!(error.message, "restorecon denied");
        assert_eq!(fs::metadata(&root).unwrap().permissions().mode(), before);
        let link = data.0.join("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        assert_eq!(
            create_directory(&link, &destination).unwrap_err().code,
            -110
        );
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
