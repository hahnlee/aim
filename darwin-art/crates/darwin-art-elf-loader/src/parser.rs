//! ELF header/program-header/dynamic-table parsing policy.
//!
//! Relocation and mapped-image lifetime remain in the parent loader module;
//! this module only validates bytes and produces the parsed load plan.

use super::*;

pub(super) fn parse_image(bytes: &[u8]) -> Result<ParsedImage, LoadError> {
    parse_image_with_appcompat(bytes, None)
}

pub(super) fn parse_image_with_appcompat(
    bytes: &[u8],
    appcompat_16kb: Option<bool>,
) -> Result<ParsedImage, LoadError> {
    parse_image_with_policy_and_appcompat(bytes, false, appcompat_16kb)
}

pub(super) fn parse_image_with_policy(
    bytes: &[u8],
    metadata_only: bool,
) -> Result<ParsedImage, LoadError> {
    parse_image_with_policy_and_appcompat(bytes, metadata_only, None)
}

fn parse_image_with_policy_and_appcompat(
    bytes: &[u8],
    metadata_only: bool,
    appcompat_16kb: Option<bool>,
) -> Result<ParsedImage, LoadError> {
    if bytes.len() < ELF64_EHDR_SIZE || bytes.len() < EI_NIDENT {
        return Err(LoadError::Format("truncated ELF header"));
    }
    if &bytes[..4] != b"\x7fELF" {
        return Err(LoadError::Format("bad magic"));
    }
    if bytes[4] != 2 || bytes[5] != 1 || bytes[6] != 1 {
        return Err(LoadError::Format("requires ELF64 little-endian v1"));
    }
    if read_u16(bytes, 16)? != ET_DYN {
        return Err(LoadError::Format("requires ET_DYN"));
    }
    if read_u16(bytes, 18)? != EM_AARCH64 {
        return Err(LoadError::Format("requires EM_AARCH64"));
    }
    if read_u32(bytes, 20)? != EV_CURRENT {
        return Err(LoadError::Format("bad ELF version"));
    }
    if read_u16(bytes, 52)? as usize != ELF64_EHDR_SIZE
        || read_u16(bytes, 54)? as usize != ELF64_PHDR_SIZE
    {
        return Err(LoadError::Format("unexpected ELF/program header size"));
    }
    let program_offset = to_usize(read_u64(bytes, 32)?, "program header offset")?;
    let program_count = read_u16(bytes, 56)? as usize;
    if program_count == 0 {
        return Err(LoadError::Format("no program headers"));
    }
    let table_size = program_count
        .checked_mul(ELF64_PHDR_SIZE)
        .ok_or(LoadError::Bounds("program header table overflow"))?;
    checked_slice(bytes, program_offset, table_size, "program header table")?;

    let mut loads = Vec::new();
    let mut dynamic = None;
    let mut relro = None;
    let mut tls = None;
    // AOSP tests RW adjacency in the original program-header table, not in
    // the filtered/sorted PT_LOAD vector. Even an intervening PT_DYNAMIC
    // breaks that adjacency.
    let mut previous_rw_index = None;
    let mut compat_rw_headers_adjacent = true;
    for index in 0..program_count {
        let offset = program_offset + index * ELF64_PHDR_SIZE;
        let header = ProgramHeader {
            kind: read_u32(bytes, offset)?,
            flags: read_u32(bytes, offset + 4)?,
            offset: read_u64(bytes, offset + 8)?,
            virtual_address: read_u64(bytes, offset + 16)?,
            file_size: read_u64(bytes, offset + 32)?,
            memory_size: read_u64(bytes, offset + 40)?,
            alignment: read_u64(bytes, offset + 48)?,
        };
        match header.kind {
            PT_LOAD => {
                validate_load(bytes, &header)?;
                if header.flags & (PF_R | PF_W) == (PF_R | PF_W) {
                    if previous_rw_index.is_some_and(|previous| previous + 1 != index) {
                        compat_rw_headers_adjacent = false;
                    }
                    previous_rw_index = Some(index);
                }
                loads.push(header);
            }
            PT_DYNAMIC => {
                if dynamic.replace(header).is_some() {
                    return Err(LoadError::Format("multiple PT_DYNAMIC segments"));
                }
            }
            PT_TLS => {
                if tls.replace(header).is_some() {
                    return Err(LoadError::Format("multiple PT_TLS segments"));
                }
            }
            PT_GNU_RELRO if !metadata_only => {
                if relro.replace(header).is_some() {
                    return Err(LoadError::Format("multiple PT_GNU_RELRO segments"));
                }
            }
            _ => {}
        }
    }
    if loads.is_empty() {
        return Err(LoadError::Format("no PT_LOAD segments"));
    }
    loads.sort_by_key(|load| load.virtual_address);
    for pair in loads.windows(2) {
        let previous_end = pair[0]
            .virtual_address
            .checked_add(pair[0].memory_size)
            .ok_or(LoadError::Bounds("PT_LOAD end overflow"))?;
        if previous_end > pair[1].virtual_address {
            return Err(LoadError::Format("overlapping PT_LOAD memory ranges"));
        }
    }

    if let Some(header) = tls {
        validate_tls_segment(bytes, &header, &loads)?;
    }

    let dynamic = dynamic.ok_or(LoadError::Format("missing PT_DYNAMIC"))?;
    validate_dynamic_segment(bytes, &dynamic, &loads)?;

    let page_size = usize::try_from(unsafe { getpagesize() })
        .ok()
        .filter(|size| size.is_power_of_two())
        .ok_or(LoadError::System {
            operation: "getpagesize",
            code: 0,
        })?;
    let minimum_alignment = loads
        .iter()
        .filter_map(|load| (load.alignment > 1).then_some(load.alignment))
        .min()
        .map(|alignment| alignment.min(page_size as u64))
        .unwrap_or(page_size as u64);
    let effective_compat = appcompat_16kb == Some(true)
        && page_size == 16 * 1024
        && minimum_alignment == ANDROID_COMPAT_PAGE_SIZE as u64;
    if appcompat_16kb.is_some()
        && page_size >= 16 * 1024
        && minimum_alignment < page_size as u64
        && !effective_compat
    {
        return Err(LoadError::Protection(
            "program alignment is smaller than system page size without effective 16KiB app compat",
        ));
    }
    let page_mask = page_size as u64 - 1;
    let minimum = loads.first().unwrap().virtual_address & !page_mask;
    let maximum_unaligned = loads
        .iter()
        .map(|load| load.virtual_address.checked_add(load.memory_size))
        .collect::<Option<Vec<_>>>()
        .ok_or(LoadError::Bounds("PT_LOAD maximum overflow"))?
        .into_iter()
        .max()
        .unwrap();
    let maximum = maximum_unaligned
        .checked_add(page_mask)
        .ok_or(LoadError::Bounds("image page rounding overflow"))?
        & !page_mask;
    let image_size = difference_to_usize(maximum, minimum)?;
    if image_size == 0 || image_size > MAX_IMAGE_SIZE {
        return Err(LoadError::Bounds("image reservation size"));
    }
    let mut protections = vec![PROT_NONE; image_size / page_size];
    let mut protection_overlap = false;
    for load in &loads {
        let start = difference_to_usize(load.virtual_address & !page_mask, minimum)? / page_size;
        let end_address = load
            .virtual_address
            .checked_add(load.memory_size)
            .and_then(|value| value.checked_add(page_mask))
            .ok_or(LoadError::Bounds("PT_LOAD page span overflow"))?
            & !page_mask;
        let end = difference_to_usize(end_address, minimum)? / page_size;
        let protection = elf_flags_to_protection(load.flags);
        for page in protections
            .get_mut(start..end)
            .ok_or(LoadError::Bounds("PT_LOAD page plan"))?
        {
            *page |= protection;
            if *page & PROT_WRITE != 0 && *page & PROT_EXEC != 0 {
                protection_overlap = true;
            }
        }
    }

    if appcompat_16kb.is_some() && protection_overlap && !effective_compat {
        return Err(LoadError::Protection(
            "host page permission overlap requires effective 16KiB app compat",
        ));
    }

    let direct_syscall_count = loads
        .iter()
        .filter(|load| load.flags & PF_X != 0)
        .map(|load| {
            let offset = to_usize(load.offset, "direct-syscall load offset")?;
            let size = to_usize(load.file_size, "direct-syscall load size")?;
            let code = checked_slice(bytes, offset, size, "direct-syscall load range")?;
            Ok::<usize, LoadError>(super::direct_syscall::count_linux_svc(code))
        })
        .try_fold(0usize, |total, count| {
            total
                .checked_add(count?)
                .ok_or(LoadError::Bounds("direct-syscall count overflow"))
        })?;
    let direct_syscall_pages =
        super::direct_syscall::veneer_page_count(direct_syscall_count, page_size)?;

    let compat_requested = match appcompat_16kb {
        Some(true) => effective_compat,
        Some(false) => false,
        None => protection_overlap,
    };
    let (image_offset, reservation_size, compat_boundary) = if compat_requested {
        if !compat_rw_headers_adjacent {
            return Err(LoadError::Protection(
                "Android 16 compat requires adjacent RW program headers",
            ));
        }
        let boundary = rx_rw_compat_boundary(&loads, relro.as_ref(), page_size)?.ok_or(
            LoadError::Protection(
                "host page permission overlap is not eligible for Android 16 RX|RW compat",
            ),
        )?;
        let boundary_from_minimum = difference_to_usize(boundary, minimum)?;
        let boundary_modulo = boundary_from_minimum % page_size;
        let image_offset = if boundary_modulo == 0 {
            0
        } else {
            page_size - boundary_modulo
        };
        let shifted_boundary = image_offset
            .checked_add(boundary_from_minimum)
            .ok_or(LoadError::Bounds("compat permission boundary overflow"))?;
        if shifted_boundary % page_size != 0 {
            return Err(LoadError::Bounds(
                "compat permission boundary is not host-page aligned",
            ));
        }
        let reservation_size = image_offset
            .checked_add(image_size)
            .and_then(|size| size.checked_add(page_mask as usize))
            .map(|size| size & !(page_size - 1))
            .ok_or(LoadError::Bounds("compat reservation size overflow"))?;
        protections = vec![PROT_READ | PROT_EXEC; reservation_size / page_size];
        for protection in protections.iter_mut().skip(shifted_boundary / page_size) {
            *protection = PROT_READ | PROT_WRITE;
        }
        (image_offset, reservation_size, Some(boundary))
    } else {
        (0, image_size, None)
    };
    let stack_guard_offset = reservation_size;
    let direct_syscall_shim_offset = reservation_size
        .checked_add(page_size)
        .ok_or(LoadError::Bounds("direct-syscall shim offset overflow"))?;
    let direct_syscall_bytes =
        page_size
            .checked_mul(direct_syscall_pages.max(1))
            .ok_or(LoadError::Bounds(
                "direct-syscall page reservation overflow",
            ))?;
    let reservation_size = reservation_size
        .checked_add(
            page_size
                .checked_add(direct_syscall_bytes)
                .ok_or(LoadError::Bounds(
                    "compatibility page reservation size overflow",
                ))?,
        )
        .ok_or(LoadError::Bounds(
            "compatibility page reservation size overflow",
        ))?;
    protections.push(PROT_READ);
    protections.extend(std::iter::repeat_n(
        PROT_READ | PROT_EXEC,
        direct_syscall_pages.max(1),
    ));

    if let Some(relro) = relro {
        if relro.flags != PF_R || relro.memory_size == 0 || relro.file_size > relro.memory_size {
            return Err(LoadError::Format("invalid PT_GNU_RELRO segment"));
        }
        let relro_end = relro
            .virtual_address
            .checked_add(relro.memory_size)
            .ok_or(LoadError::Bounds("PT_GNU_RELRO end overflow"))?;
        // Android's 16 KiB linker may emit a RELRO range that crosses the
        // boundary between two adjacent RW PT_LOADs.  It is still fully
        // backed by loadable memory and must be protected as one page range;
        // requiring one containing segment rejects otherwise valid NDK DSOs
        // such as Unity's Burst/sqlcipher libraries.
        let covered_by_loads = relro_covered_by_load_pages(
            &loads,
            relro.virtual_address,
            relro_end,
            page_size as u64,
        )?;
        if !covered_by_loads && compat_boundary.is_none() {
            return Err(LoadError::Format("PT_GNU_RELRO outside PT_LOAD"));
        }

        if compat_boundary.is_some() {
            // Android 16's safe 4K-on-16K mode folds the RELRO prefix into the
            // RX half of its single RX|RW boundary. There is no independently
            // protectable host page at the guest RELRO edge.
            return Ok(ParsedImage {
                loads,
                dynamic,
                tls,
                minimum_page: minimum,
                image_size,
                reservation_size,
                image_offset,
                compat_layout: compat_boundary.is_some(),
                stack_guard_offset,
                direct_syscall_shim_offset,
                direct_syscall_shim_size: direct_syscall_bytes,
                page_size,
                page_protections: protections,
            });
        }

        let start = difference_to_usize(relro.virtual_address & !page_mask, minimum)? / page_size;
        let end_address = relro_end
            .checked_add(page_mask)
            .ok_or(LoadError::Bounds("PT_GNU_RELRO page rounding overflow"))?
            & !page_mask;
        let end = difference_to_usize(end_address, minimum)? / page_size;
        let relro_pages = protections
            .get_mut(start..end)
            .ok_or(LoadError::Bounds("PT_GNU_RELRO page plan"))?;
        if relro_pages.is_empty()
            || relro_pages
                .iter()
                .any(|protection| protection & PROT_READ == 0 || protection & PROT_EXEC != 0)
        {
            return Err(LoadError::Protection(
                "PT_GNU_RELRO does not cover readable non-executable pages",
            ));
        }
        for protection in relro_pages {
            *protection &= !PROT_WRITE;
        }
    }

    Ok(ParsedImage {
        loads,
        dynamic,
        tls,
        minimum_page: minimum,
        image_size,
        reservation_size,
        image_offset,
        compat_layout: compat_boundary.is_some(),
        stack_guard_offset,
        direct_syscall_shim_offset,
        direct_syscall_shim_size: direct_syscall_bytes,
        page_size,
        page_protections: protections,
    })
}

fn relro_covered_by_load_pages(
    loads: &[ProgramHeader],
    relro_start: u64,
    relro_end: u64,
    page_size: u64,
) -> Result<bool, LoadError> {
    debug_assert!(page_size.is_power_of_two());
    let page_mask = page_size - 1;
    let required_start = relro_start & !page_mask;
    let required_end = relro_end
        .checked_add(page_mask)
        .ok_or(LoadError::Bounds("PT_GNU_RELRO page rounding overflow"))?
        & !page_mask;
    let mut covered_end = required_start;
    let mut ranges = loads
        .iter()
        .map(|load| {
            let start = load.virtual_address & !page_mask;
            let end = load
                .virtual_address
                .checked_add(load.memory_size)
                .and_then(|value| value.checked_add(page_mask))
                .ok_or(LoadError::Bounds("PT_LOAD page span overflow"))?
                & !page_mask;
            Ok((start, end))
        })
        .collect::<Result<Vec<_>, LoadError>>()?;
    ranges.sort_unstable_by_key(|(start, _)| *start);
    for (start, end) in ranges {
        if end <= covered_end {
            continue;
        }
        if start > covered_end {
            break;
        }
        covered_end = covered_end.max(end);
        if covered_end >= required_end {
            return Ok(true);
        }
    }
    Ok(false)
}

const ANDROID_COMPAT_PAGE_SIZE: usize = 4096;

/// Return the single guest RX|RW boundary accepted by Android 16's safe
/// 4K-on-16K compatibility layout. Ineligible layouts remain fail-closed; we
/// deliberately do not implement Android's legacy RWX fallback.
fn rx_rw_compat_boundary(
    loads: &[ProgramHeader],
    relro: Option<&ProgramHeader>,
    host_page_size: usize,
) -> Result<Option<u64>, LoadError> {
    if host_page_size != 16 * 1024
        || loads
            .iter()
            .all(|load| load.alignment >= host_page_size as u64)
    {
        return Ok(None);
    }
    let mut first_rw = None;
    let mut saw_rw = false;
    for load in loads {
        match load.flags {
            flags if !saw_rw && (flags == PF_R || flags == (PF_R | PF_X)) => {}
            flags if flags == PF_R | PF_W => {
                saw_rw = true;
                first_rw.get_or_insert(load);
            }
            _ => return Ok(None),
        }
    }
    let Some(first_rw) = first_rw else {
        return Ok(None);
    };
    let boundary = if let Some(relro) = relro {
        // A RELRO segment is eligible only when it is the exact prefix of the
        // first RW segment.  Comparing page-rounded addresses would also admit
        // a RELRO that starts later in the same guest page, which is not the
        // linker contract and can expose bytes before the actual prefix.
        if relro.virtual_address != first_rw.virtual_address {
            return Ok(None);
        }
        // The compat layout is assembled using the guest 4 KiB granularity,
        // regardless of a first RW segment's p_align (which may be 16 KiB).
        let alignment = ANDROID_COMPAT_PAGE_SIZE as u64;
        let load_start = first_rw.virtual_address & !(alignment - 1);
        let load_end = first_rw
            .virtual_address
            .checked_add(first_rw.memory_size)
            .and_then(|end| end.checked_add(alignment - 1))
            .map(|end| end & !(alignment - 1))
            .ok_or(LoadError::Bounds("compat RW segment end overflow"))?;
        let relro_start = relro.virtual_address & !(alignment - 1);
        let relro_end = relro
            .virtual_address
            .checked_add(relro.memory_size)
            .and_then(|end| end.checked_add(alignment - 1))
            .map(|end| end & !(alignment - 1))
            .ok_or(LoadError::Bounds("compat RELRO end overflow"))?;
        if relro_start != load_start || relro_end > load_end {
            return Ok(None);
        }
        relro_end
    } else {
        first_rw.virtual_address & !((ANDROID_COMPAT_PAGE_SIZE as u64) - 1)
    };
    Ok(Some(boundary))
}

fn validate_load(bytes: &[u8], load: &ProgramHeader) -> Result<(), LoadError> {
    if load.memory_size == 0 {
        return Err(LoadError::Format("zero-sized PT_LOAD"));
    }
    if load.file_size > load.memory_size {
        return Err(LoadError::Format("PT_LOAD p_filesz exceeds p_memsz"));
    }
    if load.flags & !(PF_R | PF_W | PF_X) != 0 {
        return Err(LoadError::Format("unknown PT_LOAD flags"));
    }
    if load.flags & PF_W != 0 && load.flags & PF_X != 0 {
        return Err(LoadError::Protection("PT_LOAD requests W+X"));
    }
    if load.alignment > 1
        && (!load.alignment.is_power_of_two()
            || load.offset % load.alignment != load.virtual_address % load.alignment)
    {
        return Err(LoadError::Format("invalid PT_LOAD alignment"));
    }
    let offset = to_usize(load.offset, "PT_LOAD file offset")?;
    let size = to_usize(load.file_size, "PT_LOAD file size")?;
    checked_slice(bytes, offset, size, "PT_LOAD file range")?;
    load.virtual_address
        .checked_add(load.memory_size)
        .ok_or(LoadError::Bounds("PT_LOAD virtual range overflow"))?;
    Ok(())
}

fn validate_tls_segment(
    bytes: &[u8],
    tls: &ProgramHeader,
    loads: &[ProgramHeader],
) -> Result<(), LoadError> {
    if tls.memory_size == 0 || tls.file_size > tls.memory_size {
        return Err(LoadError::Format("invalid PT_TLS size"));
    }
    if tls.memory_size > MAX_TLS_SIZE as u64 {
        return Err(LoadError::Bounds("PT_TLS memory size cap"));
    }
    if tls.flags != PF_R {
        return Err(LoadError::Format("PT_TLS must be read-only metadata"));
    }
    if tls.alignment == 0
        || !tls.alignment.is_power_of_two()
        || tls.alignment > MAX_TLS_ALIGNMENT as u64
        || tls.offset % tls.alignment != tls.virtual_address % tls.alignment
    {
        return Err(LoadError::Format("invalid PT_TLS alignment"));
    }
    checked_slice(
        bytes,
        to_usize(tls.offset, "PT_TLS file offset")?,
        to_usize(tls.file_size, "PT_TLS file size")?,
        "PT_TLS file range",
    )?;
    let memory_end = tls
        .virtual_address
        .checked_add(tls.memory_size)
        .ok_or(LoadError::Bounds("PT_TLS memory range overflow"))?;
    let file_end = tls
        .virtual_address
        .checked_add(tls.file_size)
        .ok_or(LoadError::Bounds("PT_TLS file range overflow"))?;
    // A zero-initialized TLS block has no template bytes to map. Android's
    // ps16k system libraries legitimately place that metadata in the reserved
    // virtual gap between PT_LOAD segments. Keep it bounded by the complete
    // image reservation, but do not invent a containing file mapping.
    if tls.file_size == 0 {
        let image_start = loads
            .first()
            .ok_or(LoadError::Bounds("PT_TLS without PT_LOAD"))?
            .virtual_address;
        let image_end = loads
            .iter()
            .map(|load| load.virtual_address.checked_add(load.memory_size))
            .collect::<Option<Vec<_>>>()
            .ok_or(LoadError::Bounds("PT_LOAD range overflow"))?
            .into_iter()
            .max()
            .ok_or(LoadError::Bounds("PT_TLS without PT_LOAD"))?;
        if tls.virtual_address < image_start || memory_end > image_end {
            return Err(LoadError::Bounds(
                "zero-fill PT_TLS is outside image reservation",
            ));
        }
        return Ok(());
    }
    let containing = loads.iter().find(|load| {
        load.virtual_address <= tls.virtual_address
            && load
                .virtual_address
                .checked_add(load.memory_size)
                .is_some_and(|load_end| memory_end <= load_end)
    });
    let Some(load) = containing else {
        return Err(LoadError::Bounds("PT_TLS is not inside PT_LOAD"));
    };
    let file_delta = tls
        .offset
        .checked_sub(load.offset)
        .ok_or(LoadError::Bounds("PT_TLS file mapping"))?;
    let virtual_delta = tls
        .virtual_address
        .checked_sub(load.virtual_address)
        .ok_or(LoadError::Bounds("PT_TLS virtual mapping"))?;
    if file_delta != virtual_delta || file_end > load.virtual_address.saturating_add(load.file_size)
    {
        return Err(LoadError::Format("PT_TLS file/virtual mapping mismatch"));
    }
    Ok(())
}

fn validate_dynamic_segment(
    bytes: &[u8],
    dynamic: &ProgramHeader,
    loads: &[ProgramHeader],
) -> Result<(), LoadError> {
    if dynamic.file_size == 0
        || dynamic.file_size > dynamic.memory_size
        || dynamic.file_size % ELF64_DYN_SIZE as u64 != 0
    {
        return Err(LoadError::Format("invalid PT_DYNAMIC size"));
    }
    checked_slice(
        bytes,
        to_usize(dynamic.offset, "PT_DYNAMIC offset")?,
        to_usize(dynamic.file_size, "PT_DYNAMIC size")?,
        "PT_DYNAMIC file range",
    )?;
    let end = dynamic
        .virtual_address
        .checked_add(dynamic.memory_size)
        .ok_or(LoadError::Bounds("PT_DYNAMIC range overflow"))?;
    let containing_load = loads.iter().find(|load| {
        load.virtual_address <= dynamic.virtual_address
            && load
                .virtual_address
                .checked_add(load.memory_size)
                .is_some_and(|load_end| end <= load_end)
    });
    let Some(containing_load) = containing_load else {
        return Err(LoadError::Bounds("PT_DYNAMIC is not inside PT_LOAD"));
    };
    let file_delta = dynamic
        .offset
        .checked_sub(containing_load.offset)
        .ok_or(LoadError::Bounds("PT_DYNAMIC file mapping"))?;
    let virtual_delta = dynamic
        .virtual_address
        .checked_sub(containing_load.virtual_address)
        .ok_or(LoadError::Bounds("PT_DYNAMIC virtual mapping"))?;
    if file_delta != virtual_delta
        || file_delta
            .checked_add(dynamic.file_size)
            .is_none_or(|value| value > containing_load.file_size)
    {
        return Err(LoadError::Format(
            "PT_DYNAMIC file/virtual mapping mismatch",
        ));
    }
    Ok(())
}

pub(super) fn parse_dynamic(
    bytes: &[u8],
    dynamic: &ProgramHeader,
) -> Result<DynamicInfo, LoadError> {
    parse_dynamic_with_policy(bytes, dynamic, false)
}

pub(super) fn parse_dynamic_with_policy(
    bytes: &[u8],
    dynamic: &ProgramHeader,
    metadata_only: bool,
) -> Result<DynamicInfo, LoadError> {
    let data = checked_slice(
        bytes,
        to_usize(dynamic.offset, "PT_DYNAMIC offset")?,
        to_usize(dynamic.file_size, "PT_DYNAMIC size")?,
        "PT_DYNAMIC",
    )?;
    let mut info = DynamicInfo::default();
    let mut terminated = false;
    for entry in data.chunks_exact(ELF64_DYN_SIZE) {
        let tag = read_i64(entry, 0)?;
        let value = read_u64(entry, 8)?;
        if tag == DT_NULL {
            terminated = true;
            break;
        }
        match tag {
            DT_NEEDED => info.needed_offsets.push(value),
            DT_SONAME => set_once(&mut info.soname_offset, value, "duplicate DT_SONAME")?,
            DT_HASH => set_once(&mut info.hash, value, "duplicate DT_HASH")?,
            DT_GNU_HASH => set_once(&mut info.gnu_hash, value, "duplicate DT_GNU_HASH")?,
            DT_STRTAB => set_once(&mut info.string_table, value, "duplicate DT_STRTAB")?,
            DT_STRSZ => set_once(&mut info.string_size, value, "duplicate DT_STRSZ")?,
            DT_SYMTAB => set_once(&mut info.symbol_table, value, "duplicate DT_SYMTAB")?,
            DT_SYMENT => set_once(&mut info.symbol_entry_size, value, "duplicate DT_SYMENT")?,
            DT_RELA => set_once(&mut info.rela, value, "duplicate DT_RELA")?,
            DT_RELASZ => set_once(&mut info.rela_size, value, "duplicate DT_RELASZ")?,
            DT_RELAENT => set_once(&mut info.rela_entry_size, value, "duplicate DT_RELAENT")?,
            DT_RELACOUNT => set_once(
                &mut info.relative_relocation_count,
                value,
                "duplicate DT_RELACOUNT",
            )?,
            DT_ANDROID_RELA => {
                set_once(&mut info.android_rela, value, "duplicate DT_ANDROID_RELA")?
            }
            DT_ANDROID_RELASZ => set_once(
                &mut info.android_rela_size,
                value,
                "duplicate DT_ANDROID_RELASZ",
            )?,
            DT_RELR | DT_ANDROID_RELR => set_once(&mut info.relr, value, "duplicate DT_RELR")?,
            DT_RELRSZ | DT_ANDROID_RELRSZ => {
                set_once(&mut info.relr_size, value, "duplicate DT_RELRSZ")?
            }
            DT_RELRENT | DT_ANDROID_RELRENT => {
                set_once(&mut info.relr_entry_size, value, "duplicate DT_RELRENT")?
            }
            DT_ANDROID_RELRCOUNT => set_once(
                &mut info.relr_count,
                value,
                "duplicate DT_ANDROID_RELRCOUNT",
            )?,
            DT_JMPREL => set_once(&mut info.plt_rela, value, "duplicate DT_JMPREL")?,
            DT_PLTRELSZ => set_once(&mut info.plt_rela_size, value, "duplicate DT_PLTRELSZ")?,
            DT_PLTREL => set_once(&mut info.plt_relocation_kind, value, "duplicate DT_PLTREL")?,
            DT_BIND_NOW => info.bind_now_tag = true,
            DT_FLAGS => set_once(&mut info.flags, value, "duplicate DT_FLAGS")?,
            DT_FLAGS_1 => set_once(&mut info.flags_1, value, "duplicate DT_FLAGS_1")?,
            DT_VERSYM => set_once(&mut info.versym, value, "duplicate DT_VERSYM")?,
            DT_VERNEED => set_once(&mut info.verneed, value, "duplicate DT_VERNEED")?,
            DT_VERNEEDNUM => set_once(&mut info.verneed_count, value, "duplicate DT_VERNEEDNUM")?,
            DT_VERDEF => set_once(&mut info.verdef, value, "duplicate DT_VERDEF")?,
            DT_VERDEFNUM => set_once(&mut info.verdef_count, value, "duplicate DT_VERDEFNUM")?,
            DT_INIT_ARRAY => set_once(&mut info.init_array, value, "duplicate DT_INIT_ARRAY")?,
            DT_INIT_ARRAYSZ => set_once(
                &mut info.init_array_size,
                value,
                "duplicate DT_INIT_ARRAYSZ",
            )?,
            DT_FINI => set_once(&mut info.fini, value, "duplicate DT_FINI")?,
            DT_FINI_ARRAY => set_once(&mut info.fini_array, value, "duplicate DT_FINI_ARRAY")?,
            DT_FINI_ARRAYSZ => set_once(
                &mut info.fini_array_size,
                value,
                "duplicate DT_FINI_ARRAYSZ",
            )?,
            DT_AARCH64_BTI_PLT => set_once(
                &mut info.aarch64_bti_plt,
                value,
                "duplicate DT_AARCH64_BTI_PLT",
            )?,
            DT_PLTGOT | DT_DEBUG => {}
            DT_REL | DT_RELSZ | DT_RELENT | DT_ANDROID_REL | DT_ANDROID_RELSZ => {
                if value != 0 && !metadata_only {
                    return Err(LoadError::Capability(Capability::RelRelocations));
                }
            }
            DT_INIT => {
                if value != 0 && !metadata_only {
                    return Err(LoadError::Capability(Capability::DynamicInitializer));
                }
            }
            DT_PREINIT_ARRAY | DT_PREINIT_ARRAYSZ => {
                if value != 0 && !metadata_only {
                    return Err(LoadError::Capability(Capability::PreinitArray));
                }
            }
            DT_TEXTREL if !metadata_only => {
                return Err(LoadError::Capability(Capability::TextRelocations));
            }
            DT_RUNPATH if metadata_only => {
                set_once(&mut info.runpath_offset, value, "duplicate DT_RUNPATH")?;
            }
            DT_RPATH | DT_RUNPATH if !metadata_only => {
                return Err(LoadError::Capability(Capability::Rpath));
            }
            DT_SYMBOLIC if !metadata_only => {
                return Err(LoadError::Capability(Capability::SymbolicLookup));
            }
            _ if metadata_only => {}
            _ => return Err(LoadError::Capability(Capability::UnknownDynamicTag(tag))),
        }
    }
    if !terminated {
        return Err(LoadError::Format("PT_DYNAMIC lacks DT_NULL"));
    }
    Ok(info)
}

pub(super) fn validate_dynamic_capabilities(info: &DynamicInfo) -> Result<(), LoadError> {
    if let Some(value) = info.aarch64_bti_plt.filter(|value| *value != 0) {
        return Err(LoadError::Capability(Capability::DynamicFlags {
            tag: DT_AARCH64_BTI_PLT,
            value,
        }));
    }
    if info.hash.is_none() && info.gnu_hash.is_none() {
        return Err(LoadError::Capability(Capability::MissingSysvHash));
    }
    if info.string_table.is_none()
        || info.string_size.is_none()
        || info.symbol_table.is_none()
        || info.symbol_entry_size.is_none()
    {
        return Err(LoadError::Format("incomplete dynamic symbol metadata"));
    }
    if let Some(relative_count) = info.relative_relocation_count {
        let relocation_count = info.rela_size.unwrap_or(0) / ELF64_RELA_SIZE as u64;
        if relative_count > relocation_count {
            return Err(LoadError::Format("DT_RELACOUNT exceeds DT_RELASZ"));
        }
    }
    let android_rela_address = info.android_rela.unwrap_or(0);
    let android_rela_size = info.android_rela_size.unwrap_or(0);
    if (android_rela_address == 0) != (android_rela_size == 0) {
        return Err(LoadError::Format(
            "incomplete DT_ANDROID_RELA/DT_ANDROID_RELASZ",
        ));
    }
    let relr_address = info.relr.unwrap_or(0);
    let relr_size = info.relr_size.unwrap_or(0);
    if (relr_address == 0) != (relr_size == 0) {
        return Err(LoadError::Format("incomplete DT_RELR/DT_RELRSZ"));
    }
    if relr_size != 0 && (info.relr_entry_size.unwrap_or(8) != 8 || relr_size % 8 != 0) {
        return Err(LoadError::Format("invalid DT_RELRENT/DT_RELRSZ"));
    }
    if info.relr_count.is_some_and(|count| count > relr_size / 8) {
        return Err(LoadError::Format("DT_ANDROID_RELRCOUNT exceeds DT_RELRSZ"));
    }
    let plt_address = info.plt_rela.unwrap_or(0);
    let plt_size = info.plt_rela_size.unwrap_or(0);
    if (plt_address == 0) != (plt_size == 0) {
        return Err(LoadError::Format("incomplete DT_JMPREL/DT_PLTRELSZ"));
    }
    if plt_size != 0 {
        if info.plt_relocation_kind != Some(DT_RELA as u64) {
            return Err(LoadError::Format("DT_PLTREL is not DT_RELA"));
        }
        let now = info.bind_now_tag
            || info.flags.is_some_and(|flags| flags & DF_BIND_NOW != 0)
            || info.flags_1.is_some_and(|flags| flags & DF_1_NOW != 0);
        if !now {
            return Err(LoadError::Capability(Capability::LazyBinding));
        }
    } else if info.plt_relocation_kind.is_some()
        && (info.plt_rela.is_none() || info.plt_rela_size.is_none())
    {
        return Err(LoadError::Format("DT_PLTREL without DT_JMPREL"));
    }
    if let Some(flags) = info.flags
        && flags & !(DF_BIND_NOW | DF_SYMBOLIC) != 0
    {
        return Err(LoadError::Capability(Capability::DynamicFlags {
            tag: DT_FLAGS,
            value: flags,
        }));
    }
    if let Some(flags) = info.flags_1
        && flags & !(DF_1_NOW | DF_1_GLOBAL) != 0
    {
        return Err(LoadError::Capability(Capability::DynamicFlags {
            tag: DT_FLAGS_1,
            value: flags,
        }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_bounded_zero_fill_tls_between_loads() {
        let loads = [
            ProgramHeader {
                kind: PT_LOAD,
                flags: PF_R | PF_X,
                offset: 0,
                virtual_address: 0,
                file_size: 0x1800,
                memory_size: 0x1800,
                alignment: 0x4000,
            },
            ProgramHeader {
                kind: PT_LOAD,
                flags: PF_R | PF_W,
                offset: 0x4000,
                virtual_address: 0x4000,
                file_size: 0x1000,
                memory_size: 0x2000,
                alignment: 0x4000,
            },
        ];
        let tls = ProgramHeader {
            kind: PT_TLS,
            flags: PF_R,
            offset: 0x4000,
            virtual_address: 0x1800,
            file_size: 0,
            memory_size: 16,
            alignment: 8,
        };
        let bytes = vec![0; 0x4000];
        assert!(validate_tls_segment(&bytes, &tls, &loads).is_ok());

        let outside = ProgramHeader {
            virtual_address: 0x6000,
            ..tls
        };
        assert!(validate_tls_segment(&bytes, &outside, &loads).is_err());
    }

    #[test]
    fn android16_rx_rw_compat_accepts_snapseed_segment_geometry() {
        let loads = vec![
            load(0, 0x10c1b40, PF_R | PF_X),
            load(0x10c2000, 0x77640, PF_R | PF_W),
            load(0x113a640, 0xdb730, PF_R | PF_W),
        ];
        let relro = ProgramHeader {
            kind: PT_GNU_RELRO,
            flags: PF_R,
            offset: 0x10c2000,
            virtual_address: 0x10c2000,
            file_size: 0x77640,
            memory_size: 0x78000,
            alignment: 1,
        };
        assert_eq!(
            rx_rw_compat_boundary(&loads, Some(&relro), 16 * 1024).unwrap(),
            Some(0x113a000)
        );
    }

    #[test]
    fn android16_rx_rw_compat_rejects_relro_not_at_first_rw_vaddr() {
        let loads = vec![
            load(0, 0x2000, PF_R | PF_X),
            load(0x2000, 0x4000, PF_R | PF_W),
        ];
        let relro = ProgramHeader {
            kind: PT_GNU_RELRO,
            flags: PF_R,
            offset: 0x2001,
            // This is in the same guest 4 KiB page as first_rw, but is not its
            // exact prefix. Page-rounded equality must not make it eligible.
            virtual_address: 0x2001,
            file_size: 0x100,
            memory_size: 0x100,
            alignment: 1,
        };
        assert_eq!(
            rx_rw_compat_boundary(&loads, Some(&relro), 16 * 1024).unwrap(),
            None
        );
    }

    #[test]
    fn android16_rx_rw_compat_rounds_relro_end_to_guest_page_with_16k_p_align() {
        let mut first_rw = load(0x20000, 0x6000, PF_R | PF_W);
        first_rw.alignment = 16 * 1024;
        let loads = vec![load(0, 0x2000, PF_R | PF_X), first_rw];
        let relro = ProgramHeader {
            kind: PT_GNU_RELRO,
            flags: PF_R,
            offset: 0x20000,
            virtual_address: 0x20000,
            file_size: 0x3000,
            memory_size: 0x3000,
            alignment: 1,
        };
        // 0x23000 is 4 KiB aligned but not 16 KiB aligned. The compat
        // boundary follows the guest page size, not first_rw.p_align.
        assert_eq!(
            rx_rw_compat_boundary(&loads, Some(&relro), 16 * 1024).unwrap(),
            Some(0x23000)
        );
    }

    #[test]
    fn android16_rx_rw_compat_rejects_code_after_writable_data() {
        let loads = vec![
            load(0, 0x1000, PF_R | PF_X),
            load(0x1000, 0x1000, PF_R | PF_W),
            load(0x2000, 0x1000, PF_R | PF_X),
        ];
        assert_eq!(
            rx_rw_compat_boundary(&loads, None, 16 * 1024).unwrap(),
            None
        );
    }

    #[test]
    fn relro_accepts_android_lld_page_padding_across_rw_loads() {
        let loads = vec![
            load(0x2c000, 0x988, PF_R | PF_W),
            load(0x30000, 0x9268, PF_R | PF_W),
        ];
        assert!(relro_covered_by_load_pages(&loads, 0x2c000, 0x34000, 0x4000,).unwrap());
    }

    #[test]
    fn relro_rejects_an_unmapped_page_gap() {
        let loads = vec![
            load(0x2c000, 0x988, PF_R | PF_W),
            load(0x34000, 0x1000, PF_R | PF_W),
        ];
        assert!(!relro_covered_by_load_pages(&loads, 0x2c000, 0x38000, 0x4000,).unwrap());
    }

    fn load(virtual_address: u64, memory_size: u64, flags: u32) -> ProgramHeader {
        ProgramHeader {
            kind: PT_LOAD,
            flags,
            offset: virtual_address,
            virtual_address,
            file_size: memory_size,
            memory_size,
            alignment: ANDROID_COMPAT_PAGE_SIZE as u64,
        }
    }
}
