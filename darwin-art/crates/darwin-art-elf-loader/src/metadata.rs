//! File-backed ELF metadata inspection, separate from mapping and relocation.
use super::*;
impl LoadedElf {
    /// Original flags retained with the mapping, not re-read from its pathname.
    pub fn dynamic_flags_1(&self) -> u64 {
        self.dynamic.flags_1.unwrap_or(0)
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ElfMetadata {
    pub soname: Option<Vec<u8>>,
    pub needed_libraries: Vec<Vec<u8>>,
    /// Raw DT_RUNPATH bytes; expansion and namespace search belong to the linker owner.
    pub runpath: Option<Vec<u8>>,
    /// Original DT_FLAGS_1 bits (zero when absent), not host dlopen flags.
    pub flags_1: u64,
}

/// Parses one Android arm64 ET_DYN image without relocating it or running guest code.
///
/// This path parses file bytes only: it does not reserve address space, relocate, or run guest code.
pub fn inspect_elf_metadata(bytes: &[u8]) -> Result<ElfMetadata, LoadError> {
    let parsed = parse_image_with_policy(bytes, true)?;
    let dynamic = parse_dynamic_with_policy(bytes, &parsed.dynamic, true)?;
    let needed_libraries = dynamic
        .needed_offsets
        .iter()
        .map(|offset| dynamic_string_bytes(bytes, &parsed.loads, &dynamic, *offset))
        .collect::<Result<Vec<_>, _>>()?;
    let soname = dynamic
        .soname_offset
        .map(|offset| dynamic_string_bytes(bytes, &parsed.loads, &dynamic, offset))
        .transpose()?;
    if needed_libraries.iter().any(Vec::is_empty) {
        return Err(LoadError::Format("empty DT_NEEDED string"));
    }
    if soname.as_ref().is_some_and(Vec::is_empty) {
        return Err(LoadError::Format("empty DT_SONAME string"));
    }
    let runpath = dynamic
        .runpath_offset
        .map(|offset| dynamic_string_bytes(bytes, &parsed.loads, &dynamic, offset))
        .transpose()?;
    Ok(ElfMetadata {
        soname,
        needed_libraries,
        runpath,
        flags_1: dynamic.flags_1.unwrap_or(0),
    })
}

pub(super) fn dynamic_string_bytes(
    bytes: &[u8],
    loads: &[ProgramHeader],
    dynamic: &DynamicInfo,
    offset: u64,
) -> Result<Vec<u8>, LoadError> {
    let table = dynamic
        .string_table
        .ok_or(LoadError::Format("missing DT_STRTAB"))?;
    let size = dynamic
        .string_size
        .ok_or(LoadError::Format("missing DT_STRSZ"))?;
    if offset >= size {
        return Err(LoadError::Bounds("dynamic string offset"));
    }
    let address = table
        .checked_add(offset)
        .ok_or(LoadError::Bounds("dynamic string address overflow"))?;
    let remaining = size - offset;
    let end = address
        .checked_add(remaining)
        .ok_or(LoadError::Bounds("dynamic string range overflow"))?;
    let load = loads
        .iter()
        .find(|load| {
            load.flags & PF_R != 0
                && address >= load.virtual_address
                && load
                    .virtual_address
                    .checked_add(load.file_size)
                    .is_some_and(|load_end| end <= load_end)
        })
        .ok_or(LoadError::Bounds(
            "dynamic string table is not file-backed readable data",
        ))?;
    let file_offset = load
        .offset
        .checked_add(
            address
                .checked_sub(load.virtual_address)
                .ok_or(LoadError::Bounds("dynamic string file mapping"))?,
        )
        .ok_or(LoadError::Bounds("dynamic string file offset overflow"))?;
    let data = checked_slice(
        bytes,
        to_usize(file_offset, "dynamic string file offset")?,
        to_usize(remaining, "dynamic string size")?,
        "dynamic string file range",
    )?;
    let terminator = data
        .iter()
        .position(|byte| *byte == 0)
        .ok_or(LoadError::Format("unterminated dynamic string"))?;
    Ok(data[..terminator].to_vec())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) fn image(runpath: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; 512];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        for (offset, value) in [(16, 3u16), (18, 183), (52, 64), (54, 56), (56, 2)] {
            bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
        }
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes());
        for (offset, value) in [
            (32, 64u64),
            (80, 0x1000),
            (96, 512),
            (104, 512),
            (112, 4096),
            (128, 176),
            (136, 0x10b0),
            (152, 64),
            (160, 64),
            (168, 8),
        ] {
            bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        for (offset, value) in [(64, 1u32), (68, 4), (120, 2), (124, 4)] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (index, (tag, value)) in [
            (DT_STRTAB, 0x1100u64),
            (DT_STRSZ, runpath.len() as u64),
            (DT_RUNPATH, 0),
            (DT_NULL, 0),
        ]
        .into_iter()
        .enumerate()
        {
            let offset = 176 + index * 16;
            bytes[offset..offset + 8].copy_from_slice(&tag.to_le_bytes());
            bytes[offset + 8..offset + 16].copy_from_slice(&value.to_le_bytes());
        }
        bytes[256..256 + runpath.len()].copy_from_slice(runpath);
        bytes
    }
    #[test]
    fn dynamic_flags_survive_inspection_without_enabling_load() {
        assert_eq!(inspect_elf_metadata(&image(b"\0")).unwrap().flags_1, 0);
        let mut bytes = image(b"\0");
        bytes[208..216].copy_from_slice(&DT_FLAGS_1.to_le_bytes());
        // DF_1_GLOBAL plus an unknown bit: inspection preserves, not interprets.
        let flags = 2u64 | (1u64 << 45);
        bytes[216..224].copy_from_slice(&flags.to_le_bytes());
        assert_eq!(inspect_elf_metadata(&bytes).unwrap().flags_1, flags);
        unsafe {
            let mut inspection = std::ptr::null_mut();
            assert_eq!(
                crate::ffi::darwin_art_elf_inspect_bytes(
                    bytes.as_ptr(),
                    bytes.len(),
                    &mut inspection,
                    std::ptr::null_mut()
                ),
                crate::ffi::DarwinArtElfStatus::Ok
            );
            let mut result = 77;
            assert_eq!(
                crate::ffi::darwin_art_elf_inspection_flags_1(
                    inspection,
                    &mut result,
                    std::ptr::null_mut()
                ),
                crate::ffi::DarwinArtElfStatus::Ok
            );
            assert_eq!(result, flags);
            assert_ne!(
                crate::ffi::darwin_art_elf_inspection_flags_1(
                    std::ptr::null(),
                    &mut result,
                    std::ptr::null_mut()
                ),
                crate::ffi::DarwinArtElfStatus::Ok
            );
            assert_eq!(result, flags);
            crate::ffi::darwin_art_elf_inspection_destroy(&mut inspection);
        }
        let info = DynamicInfo {
            hash: Some(1),
            string_table: Some(1),
            string_size: Some(1),
            symbol_table: Some(1),
            symbol_entry_size: Some(24),
            flags_1: Some(flags),
            ..Default::default()
        };
        assert!(matches!(
            validate_dynamic_capabilities(&info),
            Err(LoadError::Capability(Capability::DynamicFlags {
                tag: DT_FLAGS_1,
                ..
            }))
        ));
    }
    #[test]
    fn runpath_is_preserved_as_bytes_and_bounds_checked() {
        let bytes = image(b"$ORIGIN/../lib:\xff\0");
        let metadata = inspect_elf_metadata(&bytes).unwrap();
        assert_eq!(
            metadata.runpath.as_deref(),
            Some(&b"$ORIGIN/../lib:\xff"[..])
        );
        unsafe {
            let mut inspection = std::ptr::null_mut();
            assert_eq!(
                crate::ffi::darwin_art_elf_inspect_bytes(
                    bytes.as_ptr(),
                    bytes.len(),
                    &mut inspection,
                    std::ptr::null_mut()
                ),
                crate::ffi::DarwinArtElfStatus::Ok
            );
            let mut runpath = std::ptr::null();
            assert_eq!(
                crate::ffi::darwin_art_elf_inspection_runpath(
                    inspection,
                    &mut runpath,
                    std::ptr::null_mut()
                ),
                crate::ffi::DarwinArtElfStatus::Ok
            );
            assert_eq!(
                std::ffi::CStr::from_ptr(runpath).to_bytes(),
                b"$ORIGIN/../lib:\xff"
            );
            crate::ffi::darwin_art_elf_inspection_destroy(&mut inspection);
        }
        let parsed = parse_image_with_policy(&bytes, true).unwrap();
        assert!(matches!(
            parse_dynamic_with_policy(&bytes, &parsed.dynamic, false),
            Err(LoadError::Capability(Capability::Rpath))
        ));
        assert!(inspect_elf_metadata(&image(b"unterminated")).is_err());
        assert_eq!(
            inspect_elf_metadata(&image(b"\0")).unwrap().runpath,
            Some(Vec::new())
        );
        let mut invalid = bytes.clone();
        invalid[216..224].copy_from_slice(&999u64.to_le_bytes());
        assert!(inspect_elf_metadata(&invalid).is_err());
    }
}
