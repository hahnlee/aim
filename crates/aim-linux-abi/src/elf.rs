//! Minimal ELF64 (AArch64, little-endian) header parsing for the program loader.

pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;
pub const EM_AARCH64: u16 = 183;
pub const PT_LOAD: u32 = 1;
pub const PT_INTERP: u32 = 3;
pub const PT_PHDR: u32 = 6;
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

#[derive(Debug, Clone)]
pub struct Header {
    pub e_type: u16,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_phentsize: u16,
    pub e_phnum: u16,
}

#[derive(Debug, Clone, Copy)]
pub struct Phdr {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

pub fn parse_header(b: &[u8]) -> Result<Header, String> {
    if b.len() < 64 || &b[..4] != b"\x7fELF" {
        return Err("not an ELF file".into());
    }
    if b[4] != 2 || b[5] != 1 {
        return Err("not a little-endian ELF64 file".into());
    }
    let h = Header {
        e_type: u16_at(b, 16),
        e_entry: u64_at(b, 24),
        e_phoff: u64_at(b, 32),
        e_phentsize: u16_at(b, 54),
        e_phnum: u16_at(b, 56),
    };
    if u16_at(b, 18) != EM_AARCH64 {
        return Err("not an AArch64 ELF file".into());
    }
    if h.e_type != ET_EXEC && h.e_type != ET_DYN {
        return Err(format!("unsupported e_type {}", h.e_type));
    }
    if h.e_phentsize != 56 {
        return Err("unexpected e_phentsize".into());
    }
    Ok(h)
}

pub fn parse_phdrs(b: &[u8], h: &Header) -> Result<Vec<Phdr>, String> {
    let start = h.e_phoff as usize;
    let end = start + h.e_phnum as usize * 56;
    if end > b.len() {
        return Err("program headers out of bounds".into());
    }
    Ok((0..h.e_phnum as usize)
        .map(|i| {
            let o = start + i * 56;
            Phdr {
                p_type: u32_at(b, o),
                p_flags: u32_at(b, o + 4),
                p_offset: u64_at(b, o + 8),
                p_vaddr: u64_at(b, o + 16),
                p_filesz: u64_at(b, o + 32),
                p_memsz: u64_at(b, o + 40),
                p_align: u64_at(b, o + 48),
            }
        })
        .collect())
}
