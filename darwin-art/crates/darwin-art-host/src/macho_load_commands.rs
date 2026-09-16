//! Bounded, pure Mach-O load-command parsing.
//!
//! This parser performs no filesystem lookup or dependency resolution. It
//! accepts a thin little-endian arm64 Mach-O directly, or a bounded
//! big-endian FAT_MAGIC container with exactly one supported arm64 slice.

use std::io;

const MH_MAGIC_64: u32 = 0xfeed_facf;
const MH_CIGAM_64: u32 = 0xcffa_edfe;
const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_CIGAM: u32 = 0xbeba_feca;
const FAT_MAGIC_64: u32 = 0xcafe_babf;
const FAT_CIGAM_64: u32 = 0xbfba_feca;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;
const CPU_SUBTYPE_ARM64_ALL: u32 = 0;
const MH_EXECUTE: u32 = 2;
const MH_DYLIB: u32 = 6;
const MH_BUNDLE: u32 = 8;
const LC_LOAD_DYLIB: u32 = 0x0000_000c;
const LC_RPATH: u32 = 0x8000_001c;
const LC_LOAD_WEAK_DYLIB: u32 = 0x8000_0018;
const LC_REEXPORT_DYLIB: u32 = 0x8000_001f;
const LC_LOAD_UPWARD_DYLIB: u32 = 0x8000_0023;
const MAX_COMMAND_BYTES: usize = 16 * 1024 * 1024;
const MAX_COMMAND_COUNT: usize = 65_536;
const MAX_FAT_ARCHES: usize = 4_096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LoadCommands {
    pub dependencies: Vec<Dependency>,
    pub runpaths: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Dependency {
    pub path: String,
    pub weak: bool,
}

pub(crate) fn parse(bytes: &[u8]) -> io::Result<LoadCommands> {
    if bytes.len() < 4 {
        return Err(invalid("truncated Mach-O magic"));
    }
    let magic = u32::from_be_bytes(bytes[..4].try_into().unwrap());
    match magic {
        FAT_MAGIC => parse_thin(select_arm64_slice(bytes)?),
        FAT_CIGAM | FAT_MAGIC_64 | FAT_CIGAM_64 => {
            Err(invalid("unsupported Mach-O universal-binary byte format"))
        }
        MH_MAGIC_64 => Err(invalid("unsupported big-endian Mach-O")),
        MH_CIGAM_64 => parse_thin(bytes),
        _ => Err(invalid("unsupported Mach-O magic or architecture")),
    }
}

fn select_arm64_slice(bytes: &[u8]) -> io::Result<&[u8]> {
    if bytes.len() < 8 {
        return Err(invalid("truncated FAT Mach-O header"));
    }
    let count = u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
    if count == 0 || count > MAX_FAT_ARCHES {
        return Err(invalid("invalid FAT Mach-O architecture count"));
    }
    let table_bytes = count
        .checked_mul(20)
        .ok_or_else(|| invalid("FAT Mach-O architecture table overflow"))?;
    let table_end = 8_usize
        .checked_add(table_bytes)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| invalid("truncated FAT Mach-O architecture table"))?;
    let mut selected = None;
    let mut ranges = Vec::with_capacity(count);
    for index in 0..count {
        let offset = 8 + index * 20;
        let cputype = be32(bytes, offset)?;
        let cpusubtype = be32(bytes, offset + 4)?;
        let slice_offset = be32(bytes, offset + 8)? as usize;
        let slice_size = be32(bytes, offset + 12)? as usize;
        let align = be32(bytes, offset + 16)?;
        if align >= usize::BITS || (slice_offset & ((1_usize << align) - 1)) != 0 {
            return Err(invalid("invalid FAT Mach-O slice alignment"));
        }
        let slice_end = slice_offset
            .checked_add(slice_size)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| invalid("FAT Mach-O slice exceeds input"))?;
        if slice_offset < table_end || slice_size < 32 {
            return Err(invalid("invalid FAT Mach-O slice range"));
        }
        if ranges
            .iter()
            .any(|(start, end)| slice_offset < *end && *start < slice_end)
        {
            return Err(invalid("overlapping FAT Mach-O slices"));
        }
        ranges.push((slice_offset, slice_end));
        if cputype == CPU_TYPE_ARM64 && cpusubtype == CPU_SUBTYPE_ARM64_ALL {
            if selected.is_some() {
                return Err(invalid("duplicate supported arm64 FAT Mach-O slices"));
            }
            selected = Some(&bytes[slice_offset..slice_end]);
        }
    }
    selected.ok_or_else(|| invalid("FAT Mach-O has no supported arm64 slice"))
}

fn parse_thin(bytes: &[u8]) -> io::Result<LoadCommands> {
    if bytes.len() < 32 {
        return Err(invalid("truncated Mach-O 64-bit header"));
    }
    if le32(bytes, 0)? != MH_MAGIC_64 {
        return Err(invalid("Mach-O slice is not little-endian 64-bit"));
    }
    if le32(bytes, 4)? != CPU_TYPE_ARM64 || le32(bytes, 8)? != CPU_SUBTYPE_ARM64_ALL {
        return Err(invalid("Mach-O slice is not supported arm64"));
    }
    if !matches!(le32(bytes, 12)?, MH_EXECUTE | MH_DYLIB | MH_BUNDLE) {
        return Err(invalid(
            "Mach-O file type is not executable, dylib or bundle",
        ));
    }
    let command_count = le32(bytes, 16)? as usize;
    let command_bytes = le32(bytes, 20)? as usize;
    if command_count > MAX_COMMAND_COUNT || command_bytes > MAX_COMMAND_BYTES {
        return Err(invalid("Mach-O load-command table exceeds limit"));
    }
    let commands_end = 32_usize
        .checked_add(command_bytes)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| invalid("Mach-O load commands exceed input"))?;
    let mut cursor = 32_usize;
    let mut dependencies = Vec::new();
    let mut runpaths = Vec::new();
    for _ in 0..command_count {
        let command = le32(bytes, cursor)?;
        let command_size = le32(bytes, cursor + 4)? as usize;
        if command_size < 8 || command_size % 8 != 0 {
            return Err(invalid("invalid Mach-O load-command size"));
        }
        let end = cursor
            .checked_add(command_size)
            .filter(|end| *end <= commands_end)
            .ok_or_else(|| invalid("Mach-O load command exceeds command table"))?;
        match command {
            LC_LOAD_DYLIB | LC_LOAD_WEAK_DYLIB | LC_REEXPORT_DYLIB | LC_LOAD_UPWARD_DYLIB => {
                if command_size < 24 {
                    return Err(invalid("truncated Mach-O dylib command"));
                }
                let path = command_string(bytes, cursor, end, 8, 24)?;
                dependencies.push(Dependency {
                    path,
                    weak: command == LC_LOAD_WEAK_DYLIB,
                });
            }
            LC_RPATH => {
                if command_size < 16 {
                    return Err(invalid("truncated Mach-O rpath command"));
                }
                runpaths.push(command_string(bytes, cursor, end, 8, 12)?);
            }
            _ => {}
        }
        cursor = end;
    }
    if cursor != commands_end {
        return Err(invalid(
            "Mach-O command count does not consume command table",
        ));
    }
    Ok(LoadCommands {
        dependencies,
        runpaths,
    })
}

fn command_string(
    bytes: &[u8],
    command_start: usize,
    command_end: usize,
    offset_field: usize,
    minimum_offset: usize,
) -> io::Result<String> {
    let offset = le32(bytes, command_start + offset_field)? as usize;
    if offset < minimum_offset || command_start.checked_add(offset).is_none() {
        return Err(invalid("invalid Mach-O command string offset"));
    }
    let start = command_start + offset;
    if start >= command_end {
        return Err(invalid("Mach-O command string offset exceeds command"));
    }
    let string = &bytes[start..command_end];
    let nul = string
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| invalid("unterminated Mach-O command string"))?;
    if nul == 0 {
        return Err(invalid("empty Mach-O command string"));
    }
    std::str::from_utf8(&string[..nul])
        .map(str::to_owned)
        .map_err(|_| invalid("Mach-O command string is not UTF-8"))
}

fn le32(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| invalid("Mach-O offset overflow"))?;
    let bytes = bytes
        .get(offset..end)
        .ok_or_else(|| invalid("truncated Mach-O field"))?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()))
}

fn be32(bytes: &[u8], offset: usize) -> io::Result<u32> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| invalid("FAT Mach-O offset overflow"))?;
    let bytes = bytes
        .get(offset..end)
        .ok_or_else(|| invalid("truncated FAT Mach-O field"))?;
    Ok(u32::from_be_bytes(bytes.try_into().unwrap()))
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thin(file_type: u32, commands: &[u8]) -> Vec<u8> {
        assert_eq!(commands.len() % 8, 0);
        let mut bytes = vec![0_u8; 32 + commands.len()];
        bytes[..4].copy_from_slice(&MH_MAGIC_64.to_le_bytes());
        bytes[4..8].copy_from_slice(&CPU_TYPE_ARM64.to_le_bytes());
        bytes[8..12].copy_from_slice(&CPU_SUBTYPE_ARM64_ALL.to_le_bytes());
        bytes[12..16].copy_from_slice(&file_type.to_le_bytes());
        bytes[16..20].copy_from_slice(&count_commands(commands).to_le_bytes());
        bytes[20..24].copy_from_slice(&(commands.len() as u32).to_le_bytes());
        bytes[32..].copy_from_slice(commands);
        bytes
    }

    fn count_commands(commands: &[u8]) -> u32 {
        let mut count = 0;
        let mut offset = 0;
        while offset < commands.len() {
            count += 1;
            offset +=
                u32::from_le_bytes(commands[offset + 4..offset + 8].try_into().unwrap()) as usize;
        }
        count
    }

    fn dylib(command: u32, path: &[u8]) -> Vec<u8> {
        let size = (24 + path.len() + 1 + 7) & !7;
        let mut bytes = vec![0_u8; size];
        bytes[..4].copy_from_slice(&command.to_le_bytes());
        bytes[4..8].copy_from_slice(&(size as u32).to_le_bytes());
        bytes[8..12].copy_from_slice(&24_u32.to_le_bytes());
        bytes[24..24 + path.len()].copy_from_slice(path);
        bytes
    }

    fn rpath(path: &[u8]) -> Vec<u8> {
        let size = (12 + path.len() + 1 + 7) & !7;
        let mut bytes = vec![0_u8; size];
        bytes[..4].copy_from_slice(&LC_RPATH.to_le_bytes());
        bytes[4..8].copy_from_slice(&(size as u32).to_le_bytes());
        bytes[8..12].copy_from_slice(&12_u32.to_le_bytes());
        bytes[12..12 + path.len()].copy_from_slice(path);
        bytes
    }

    fn fat(slices: &[(u32, u32, &[u8])]) -> Vec<u8> {
        let table_end = 8 + slices.len() * 20;
        let mut bytes = vec![0_u8; table_end];
        bytes[..4].copy_from_slice(&FAT_MAGIC.to_be_bytes());
        bytes[4..8].copy_from_slice(&(slices.len() as u32).to_be_bytes());
        let mut offset = (table_end + 0x3fff) & !0x3fff;
        for (index, (cpu, subtype, slice)) in slices.iter().enumerate() {
            let entry = 8 + index * 20;
            bytes[entry..entry + 4].copy_from_slice(&cpu.to_be_bytes());
            bytes[entry + 4..entry + 8].copy_from_slice(&subtype.to_be_bytes());
            bytes[entry + 8..entry + 12].copy_from_slice(&(offset as u32).to_be_bytes());
            bytes[entry + 12..entry + 16].copy_from_slice(&(slice.len() as u32).to_be_bytes());
            bytes[entry + 16..entry + 20].copy_from_slice(&14_u32.to_be_bytes());
            bytes.resize(offset + slice.len(), 0);
            bytes[offset..offset + slice.len()].copy_from_slice(slice);
            offset = (offset + slice.len() + 0x3fff) & !0x3fff;
        }
        bytes
    }

    #[test]
    fn parses_dependencies_runpaths_and_weakness() {
        let mut commands = dylib(LC_LOAD_DYLIB, b"/usr/lib/libSystem.B.dylib");
        commands.extend(dylib(LC_LOAD_WEAK_DYLIB, b"@rpath/libweak.dylib"));
        commands.extend(dylib(LC_REEXPORT_DYLIB, b"@loader_path/libreexport.dylib"));
        commands.extend(dylib(LC_LOAD_UPWARD_DYLIB, b"@rpath/libupward.dylib"));
        commands.extend(rpath(b"@loader_path/Frameworks"));
        let parsed = parse(&thin(MH_EXECUTE, &commands)).unwrap();
        assert_eq!(parsed.runpaths, vec!["@loader_path/Frameworks"]);
        assert_eq!(parsed.dependencies.len(), 4);
        assert!(!parsed.dependencies[0].weak);
        assert!(parsed.dependencies[1].weak);
        assert!(!parsed.dependencies[2].weak);
        assert_eq!(parsed.dependencies[3].path, "@rpath/libupward.dylib");
    }

    #[test]
    fn parses_exactly_one_arm64_fat_slice() {
        let slice = thin(MH_DYLIB, &dylib(LC_LOAD_DYLIB, b"lib.dylib"));
        let input = fat(&[(0x0100_0007, 3, &slice), (CPU_TYPE_ARM64, 0, &slice)]);
        assert_eq!(parse(&input).unwrap().dependencies[0].path, "lib.dylib");
    }

    #[test]
    fn rejects_fat_duplicates_bad_ranges_and_alignment() {
        let slice = thin(MH_DYLIB, &[]);
        let duplicate = fat(&[(CPU_TYPE_ARM64, 0, &slice), (CPU_TYPE_ARM64, 0, &slice)]);
        assert!(parse(&duplicate).is_err());

        let mut overlap = fat(&[(0x0100_0007, 3, &slice), (CPU_TYPE_ARM64, 0, &slice)]);
        let first_offset = overlap[16..20].to_vec();
        overlap[36..40].copy_from_slice(&first_offset);
        assert!(parse(&overlap).is_err());

        let mut bad_range = fat(&[(CPU_TYPE_ARM64, 0, &slice)]);
        bad_range[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(parse(&bad_range).is_err());

        let mut bad_alignment = fat(&[(CPU_TYPE_ARM64, 0, &slice)]);
        bad_alignment[24..28].copy_from_slice(&64_u32.to_be_bytes());
        assert!(parse(&bad_alignment).is_err());
        let mut fat64 = bad_alignment.clone();
        fat64[..4].copy_from_slice(&FAT_MAGIC_64.to_be_bytes());
        assert!(parse(&fat64).is_err());
    }

    #[test]
    fn rejects_unsupported_thin_architecture_and_file_type() {
        let mut wrong_cpu = thin(MH_EXECUTE, &[]);
        wrong_cpu[4..8].copy_from_slice(&7_u32.to_le_bytes());
        assert!(parse(&wrong_cpu).is_err());
        let mut wrong_endian = thin(MH_EXECUTE, &[]);
        wrong_endian[..4].copy_from_slice(&MH_CIGAM_64.to_le_bytes());
        assert!(parse(&wrong_endian).is_err());
        assert!(parse(&thin(3, &[])).is_err());
        assert!(parse(&fat(&[(7, 3, &thin(MH_DYLIB, &[]))])).is_err());
    }

    #[test]
    fn rejects_truncated_offsets_unterminated_utf8_and_bad_commands() {
        let valid = thin(MH_DYLIB, &dylib(LC_LOAD_DYLIB, b"lib.dylib"));
        for length in 0..valid.len() {
            assert!(parse(&valid[..length]).is_err(), "length={length}");
        }

        let mut bad_offset = valid.clone();
        bad_offset[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&bad_offset).is_err());
        let bad_utf8 = thin(MH_DYLIB, &dylib(LC_LOAD_DYLIB, &[0xff]));
        assert!(parse(&bad_utf8).is_err());
        let mut bad_size = thin(MH_DYLIB, &[]);
        bad_size[16..20].copy_from_slice(&1_u32.to_le_bytes());
        bad_size[20..24].copy_from_slice(&8_u32.to_le_bytes());
        bad_size.resize(40, 0);
        assert!(parse(&bad_size).is_err());
    }

    #[test]
    fn rejects_command_count_and_storage_limits() {
        let mut too_many = thin(MH_DYLIB, &[]);
        too_many[16..20].copy_from_slice(&((MAX_COMMAND_COUNT + 1) as u32).to_le_bytes());
        assert!(parse(&too_many).is_err());
        let mut too_large = thin(MH_DYLIB, &[]);
        too_large[20..24].copy_from_slice(&((MAX_COMMAND_BYTES + 8) as u32).to_le_bytes());
        assert!(parse(&too_large).is_err());
    }
}
