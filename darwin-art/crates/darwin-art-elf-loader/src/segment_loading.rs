//! Materialize PT_LOAD bytes into an owned Darwin reservation.
//! Android's 16KiB compat mapper reads from the 4KiB-aligned file offset,
//! including the leading partial page (Bionic linker_phdr_16kib_compat.cpp).
//! Policy selection and final protection ownership remain in the parsed plan.
use super::*;

const COMPAT_PAGE_SIZE: usize = 4096;

pub(super) fn populate(
    mapping: &Mapping,
    plan: &ParsedImage,
    bytes: &[u8],
) -> Result<(), LoadError> {
    for (index, protection) in plan.page_protections.iter().copied().enumerate() {
        if protection != PROT_NONE {
            mapping.protect(
                index * plan.page_size,
                plan.page_size,
                PROT_READ | PROT_WRITE,
            )?;
        }
    }
    for load in &plan.loads {
        let destination = plan
            .image_offset
            .checked_add(difference_to_usize(
                load.virtual_address,
                plan.minimum_page,
            )?)
            .ok_or(LoadError::Bounds("PT_LOAD shifted destination overflow"))?;
        let file_offset = to_usize(load.offset, "PT_LOAD file offset")?;
        let file_size = to_usize(load.file_size, "PT_LOAD file size")?;
        let memory_size = to_usize(load.memory_size, "PT_LOAD memory size")?;
        let prefix = if plan.compat_layout {
            file_offset % COMPAT_PAGE_SIZE
        } else {
            0
        };
        if plan.compat_layout && destination % COMPAT_PAGE_SIZE != prefix {
            return Err(LoadError::Format(
                "compat PT_LOAD offset/address incongruence",
            ));
        }
        let start = destination.checked_sub(prefix).ok_or(LoadError::Bounds(
            "compat PT_LOAD prefix outside reservation",
        ))?;
        let copy_size = prefix
            .checked_add(file_size)
            .ok_or(LoadError::Bounds("PT_LOAD copy size overflow"))?;
        let end = destination
            .checked_add(memory_size)
            .ok_or(LoadError::Bounds("PT_LOAD memory end overflow"))?;
        if file_size > memory_size || end > mapping.length() || start > end {
            return Err(LoadError::Bounds("PT_LOAD copy outside reservation"));
        }
        if start < end {
            let pages = plan
                .page_protections
                .get(start / plan.page_size..=(end - 1) / plan.page_size)
                .ok_or(LoadError::Bounds("PT_LOAD copy outside page plan"))?;
            if pages.contains(&PROT_NONE) {
                return Err(LoadError::Protection("PT_LOAD copy touches unmapped page"));
            }
        }
        let source = checked_slice(bytes, file_offset - prefix, copy_size, "PT_LOAD file range")?;
        // SAFETY: checked source, destination bounds and writable page plan above.
        // The owned anonymous reservation cannot alias the input file bytes.
        unsafe {
            ptr::copy_nonoverlapping(
                source.as_ptr(),
                mapping.pointer().as_ptr().add(start),
                copy_size,
            );
            ptr::write_bytes(
                mapping.pointer().as_ptr().add(destination + file_size),
                0,
                memory_size - file_size,
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compat_elf() -> Vec<u8> {
        let mut bytes = vec![0x5a; 0x2300];
        bytes[..64].fill(0);
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[16..18].copy_from_slice(&ET_DYN.to_le_bytes());
        bytes[18..20].copy_from_slice(&EM_AARCH64.to_le_bytes());
        bytes[20..24].copy_from_slice(&EV_CURRENT.to_le_bytes());
        bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
        bytes[52..54].copy_from_slice(&64u16.to_le_bytes());
        bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&3u16.to_le_bytes());
        let headers = [
            (PT_LOAD, PF_R | PF_X, 0u64, 0x1800u64, 0x1800u64, 4096u64),
            (PT_LOAD, PF_R | PF_W, 0x2123, 0x100, 0x200, 4096),
            (PT_DYNAMIC, PF_R | PF_W, 0x2140, 96, 96, 8),
        ];
        for (i, (kind, flags, address, file_size, memory_size, alignment)) in
            headers.into_iter().enumerate()
        {
            let start = 64 + 56 * i;
            let ph = &mut bytes[start..start + 56];
            ph.fill(0);
            ph[..4].copy_from_slice(&kind.to_le_bytes());
            ph[4..8].copy_from_slice(&flags.to_le_bytes());
            ph[8..16].copy_from_slice(&address.to_le_bytes());
            ph[16..24].copy_from_slice(&address.to_le_bytes());
            ph[32..40].copy_from_slice(&file_size.to_le_bytes());
            ph[40..48].copy_from_slice(&memory_size.to_le_bytes());
            ph[48..56].copy_from_slice(&alignment.to_le_bytes());
        }
        for (i, (tag, value)) in [
            (DT_HASH, 0x400u64),
            (DT_STRTAB, 0x500),
            (DT_STRSZ, 1),
            (DT_SYMTAB, 0x600),
            (DT_SYMENT, 24),
            (DT_NULL, 0),
        ]
        .into_iter()
        .enumerate()
        {
            let start = 0x2140 + 16 * i;
            bytes[start..start + 8].copy_from_slice(&tag.to_le_bytes());
            bytes[start + 8..start + 16].copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn actual_elf_stage_requires_original_rw_header_adjacency() {
        assert_eq!(unsafe { getpagesize() }, 16384);
        let mut bytes = compat_elf();
        bytes.resize(0x3200, 0x5a);
        bytes[56..58].copy_from_slice(&4u16.to_le_bytes());
        let mut extra = bytes[120..176].to_vec();
        extra[8..16].copy_from_slice(&0x3123u64.to_le_bytes());
        extra[16..24].copy_from_slice(&0x3123u64.to_le_bytes());
        extra[32..40].copy_from_slice(&16u64.to_le_bytes());
        extra[40..48].copy_from_slice(&32u64.to_le_bytes());
        bytes[232..288].copy_from_slice(&extra);
        // RX, RW, DYNAMIC, RW: filtering out DYNAMIC must not admit this.
        assert!(matches!(
            LoadedElf::stage(&bytes),
            Err(LoadError::Protection(
                "Android 16 compat requires adjacent RW program headers"
            ))
        ));
        // RX, RW, RW, DYNAMIC: same virtual layout, valid table adjacency.
        let dynamic = bytes[176..232].to_vec();
        bytes[176..232].copy_from_slice(&extra);
        bytes[232..288].copy_from_slice(&dynamic);
        let staged = LoadedElf::stage(&bytes).unwrap();
        let actual = unsafe { std::slice::from_raw_parts(staged.image.mapping.as_ptr(), 0x3200) };
        assert_eq!(&actual[0x3123..0x3133], &bytes[0x3123..0x3133]);
    }

    #[test]
    fn actual_elf_stage_copies_compat_prefix_into_shifted_mapping() {
        assert_eq!(unsafe { getpagesize() }, 16384);
        let bytes = compat_elf();
        let staged = LoadedElf::stage(&bytes).unwrap();
        let shift =
            staged.image.mapping.as_ptr() as usize - staged.image.reservation.as_ptr() as usize;
        assert_eq!(shift, 0x2000);
        let actual = unsafe { std::slice::from_raw_parts(staged.image.mapping.as_ptr(), 0x2400) };
        assert_eq!(&actual[0x2000..0x2123], &bytes[0x2000..0x2123]);
        assert_eq!(&actual[0x2123..0x2223], &bytes[0x2123..0x2223]);
        assert!(actual[0x2223..0x2323].iter().all(|&v| v == 0));
        assert!(
            staged
                .page_protections
                .iter()
                .all(|p| p & (PROT_WRITE | PROT_EXEC) != (PROT_WRITE | PROT_EXEC))
        );
    }

    #[test]
    fn explicit_16kb_policy_rejects_disabled_and_forces_enabled_compat() {
        assert_eq!(unsafe { getpagesize() }, 16384);
        let bytes = compat_elf();
        assert!(matches!(
            LoadedElf::stage_with_appcompat(&bytes, Some(false)),
            Err(LoadError::Protection(
                "program alignment is smaller than system page size without effective 16KiB app compat"
            ))
        ));
        let staged = LoadedElf::stage_with_appcompat(&bytes, Some(true)).unwrap();
        assert_eq!(
            staged.image.mapping.as_ptr() as usize - staged.image.reservation.as_ptr() as usize,
            0x2000
        );
    }

    #[test]
    fn explicit_compat_applies_even_without_permission_overlap() {
        assert_eq!(unsafe { getpagesize() }, 16384);
        let mut bytes = compat_elf();
        // Move the writable LOAD and its dynamic table to another host page,
        // preserving their file offsets and 4KB alignment congruence.
        bytes[136..144].copy_from_slice(&0x8123u64.to_le_bytes());
        bytes[192..200].copy_from_slice(&0x8140u64.to_le_bytes());
        assert!(!parse_image(&bytes).unwrap().compat_layout);
        assert!(parse_image_with_appcompat(&bytes, Some(false)).is_err());
        assert!(
            parse_image_with_appcompat(&bytes, Some(true))
                .unwrap()
                .compat_layout
        );
    }

    fn plan(compat_layout: bool) -> ParsedImage {
        let header = ProgramHeader {
            kind: PT_LOAD,
            flags: PF_R | PF_W,
            offset: 0x1123,
            virtual_address: 0x2123,
            file_size: 16,
            memory_size: 48,
            alignment: 4096,
        };
        ParsedImage {
            loads: vec![header],
            dynamic: header,
            tls: None,
            minimum_page: 0,
            image_size: 16384,
            reservation_size: 16384,
            image_offset: 0,
            compat_layout,
            stack_guard_offset: 0,
            direct_syscall_shim_offset: 0,
            direct_syscall_shim_size: 0,
            page_size: 16384,
            page_protections: vec![PROT_READ | PROT_WRITE],
        }
    }

    #[test]
    fn compat_preserves_leading_partial_page_and_zeros_bss() {
        let bytes = vec![0x5a; 0x2000];
        let mapping = Mapping::reserve(16384).unwrap();
        populate(&mapping, &plan(true), &bytes).unwrap();
        let actual = unsafe { std::slice::from_raw_parts(mapping.pointer().as_ptr(), 16384) };
        assert!(actual[0x2000..0x2133].iter().all(|&v| v == 0x5a));
        assert!(actual[0x2133..0x2153].iter().all(|&v| v == 0));
        assert_eq!(actual[0x1fff], 0);
    }

    #[test]
    fn noncompat_retains_payload_only_copy() {
        let mapping = Mapping::reserve(16384).unwrap();
        populate(&mapping, &plan(false), &vec![0x5a; 0x2000]).unwrap();
        let actual = unsafe { std::slice::from_raw_parts(mapping.pointer().as_ptr(), 16384) };
        assert_eq!(actual[0x2122], 0);
        assert_eq!(actual[0x2123], 0x5a);
    }

    #[test]
    fn rejects_misaligned_or_unmapped_prefix() {
        let mapping = Mapping::reserve(16384).unwrap();
        let mut p = plan(true);
        p.image_offset = 1;
        assert!(populate(&mapping, &p, &vec![0; 0x2000]).is_err());
        p.image_offset = 0;
        p.page_protections[0] = PROT_NONE;
        assert!(populate(&mapping, &p, &vec![0; 0x2000]).is_err());
    }
}
