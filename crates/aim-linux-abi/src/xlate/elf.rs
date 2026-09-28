//! Bounds-checked ELF64 (AArch64, little-endian) parsing for the translator:
//! program headers, section headers and symbol tables. Malformed input is
//! an error, never a panic.

pub const ET_EXEC: u16 = 2;
pub const ET_DYN: u16 = 3;
pub const EM_AARCH64: u16 = 183;

pub const PT_LOAD: u32 = 1;
pub const PT_PHDR: u32 = 6;
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

pub const SHT_PROGBITS: u32 = 1;
pub const SHT_SYMTAB: u32 = 2;
pub const SHT_DYNSYM: u32 = 11;
pub const SHF_ALLOC: u64 = 2;
pub const SHF_EXECINSTR: u64 = 4;

pub const STT_OBJECT: u8 = 1;
pub const STT_FUNC: u8 = 2;
pub const SHN_UNDEF: u16 = 0;
pub const SHN_LORESERVE: u16 = 0xff00;

pub const EHDR_SIZE: usize = 64;
pub const PHDR_SIZE: usize = 56;
pub const SHDR_SIZE: usize = 64;

pub fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        b.get(o..o.checked_add(2)?)?.try_into().ok()?,
    ))
}
pub fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(o..o.checked_add(4)?)?.try_into().ok()?,
    ))
}
pub fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(
        b.get(o..o.checked_add(8)?)?.try_into().ok()?,
    ))
}

#[derive(Debug, Clone)]
pub struct Ehdr {
    pub e_type: u16,
    pub e_machine: u16,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Phdr {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

impl Phdr {
    pub fn to_bytes(&self) -> [u8; PHDR_SIZE] {
        let mut b = [0u8; PHDR_SIZE];
        b[0..4].copy_from_slice(&self.p_type.to_le_bytes());
        b[4..8].copy_from_slice(&self.p_flags.to_le_bytes());
        b[8..16].copy_from_slice(&self.p_offset.to_le_bytes());
        b[16..24].copy_from_slice(&self.p_vaddr.to_le_bytes());
        b[24..32].copy_from_slice(&self.p_paddr.to_le_bytes());
        b[32..40].copy_from_slice(&self.p_filesz.to_le_bytes());
        b[40..48].copy_from_slice(&self.p_memsz.to_le_bytes());
        b[48..56].copy_from_slice(&self.p_align.to_le_bytes());
        b
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Shdr {
    pub sh_name: u32,
    pub sh_type: u32,
    pub sh_flags: u64,
    pub sh_addr: u64,
    pub sh_offset: u64,
    pub sh_size: u64,
    pub sh_link: u32,
    pub sh_entsize: u64,
}

#[derive(Debug, Clone)]
pub struct Sym<'a> {
    pub name: &'a [u8],
    pub value: u64,
    pub size: u64,
    pub kind: u8,
    pub shndx: u16,
    /// From `.symtab` (true) or `.dynsym` (false).
    pub full: bool,
}

pub struct Elf<'a> {
    pub bytes: &'a [u8],
    pub ehdr: Ehdr,
    pub phdrs: Vec<Phdr>,
    pub shdrs: Vec<Shdr>,
}

fn err<T>(m: &str) -> Result<T, String> {
    Err(m.to_string())
}

/// Whether `b` starts like an AArch64 ELF64 little-endian file.
pub fn is_aarch64_elf(b: &[u8]) -> bool {
    b.len() >= 20
        && &b[..4] == b"\x7fELF"
        && b[4] == 2
        && b[5] == 1
        && u16_at(b, 18) == Some(EM_AARCH64)
}

pub fn parse(b: &[u8]) -> Result<Elf<'_>, String> {
    if b.len() < EHDR_SIZE || &b[..4] != b"\x7fELF" {
        return err("not an ELF file");
    }
    if b[4] != 2 || b[5] != 1 {
        return err("not a little-endian ELF64 file");
    }
    let r16 = |o| u16_at(b, o).ok_or("truncated ELF header");
    let r64 = |o| u64_at(b, o).ok_or("truncated ELF header");
    let ehdr = Ehdr {
        e_type: r16(16)?,
        e_machine: r16(18)?,
        e_entry: r64(24)?,
        e_phoff: r64(32)?,
        e_shoff: r64(40)?,
        e_phentsize: r16(54)?,
        e_phnum: r16(56)?,
        e_shentsize: r16(58)?,
        e_shnum: r16(60)?,
        e_shstrndx: r16(62)?,
    };
    if ehdr.e_machine != EM_AARCH64 {
        return err("not an AArch64 ELF file");
    }
    if ehdr.e_type != ET_EXEC && ehdr.e_type != ET_DYN {
        return Err(format!("unsupported e_type {}", ehdr.e_type));
    }
    if ehdr.e_phentsize as usize != PHDR_SIZE {
        return err("unexpected e_phentsize");
    }
    let mut phdrs = Vec::with_capacity(ehdr.e_phnum as usize);
    for i in 0..ehdr.e_phnum as usize {
        let o = (ehdr.e_phoff as usize)
            .checked_add(i * PHDR_SIZE)
            .ok_or("program headers out of bounds")?;
        let f = |k: usize| u64_at(b, o + k).ok_or("program headers out of bounds");
        phdrs.push(Phdr {
            p_type: u32_at(b, o).ok_or("program headers out of bounds")?,
            p_flags: u32_at(b, o + 4).ok_or("program headers out of bounds")?,
            p_offset: f(8)?,
            p_vaddr: f(16)?,
            p_paddr: f(24)?,
            p_filesz: f(32)?,
            p_memsz: f(40)?,
            p_align: f(48)?,
        });
    }
    let shdrs = parse_shdrs(b, &ehdr).unwrap_or_default();
    Ok(Elf {
        bytes: b,
        ehdr,
        phdrs,
        shdrs,
    })
}

/// Section headers, or None when absent or malformed (the translator then
/// falls back to segment granularity).
fn parse_shdrs(b: &[u8], h: &Ehdr) -> Option<Vec<Shdr>> {
    if h.e_shoff == 0 || h.e_shentsize as usize != SHDR_SIZE {
        return None;
    }
    let at = |i: usize| -> Option<Shdr> {
        let o = (h.e_shoff as usize).checked_add(i.checked_mul(SHDR_SIZE)?)?;
        Some(Shdr {
            sh_name: u32_at(b, o)?,
            sh_type: u32_at(b, o + 4)?,
            sh_flags: u64_at(b, o + 8)?,
            sh_addr: u64_at(b, o + 16)?,
            sh_offset: u64_at(b, o + 24)?,
            sh_size: u64_at(b, o + 32)?,
            sh_link: u32_at(b, o + 40)?,
            sh_entsize: u64_at(b, o + 56)?,
        })
    };
    // e_shnum == 0 with a table present: the count is in section 0's sh_size.
    let n = if h.e_shnum == 0 {
        at(0)?.sh_size as usize
    } else {
        h.e_shnum as usize
    };
    if n > 1 << 20 {
        return None;
    }
    (0..n).map(at).collect()
}

impl<'a> Elf<'a> {
    pub fn section_data(&self, s: &Shdr) -> Option<&'a [u8]> {
        let start = s.sh_offset as usize;
        self.bytes
            .get(start..start.checked_add(s.sh_size as usize)?)
    }

    pub fn section_name(&self, s: &Shdr) -> &'a [u8] {
        let Some(strtab) = self
            .shdrs
            .get(self.ehdr.e_shstrndx as usize)
            .and_then(|t| self.section_data(t))
        else {
            return b"";
        };
        cstr_at(strtab, s.sh_name as usize)
    }

    /// Symbols from `.symtab` and `.dynsym`.
    pub fn symbols(&self) -> Vec<Sym<'a>> {
        let mut out = Vec::new();
        for s in &self.shdrs {
            if s.sh_type != SHT_SYMTAB && s.sh_type != SHT_DYNSYM {
                continue;
            }
            let (Some(data), Some(strtab)) = (
                self.section_data(s),
                self.shdrs
                    .get(s.sh_link as usize)
                    .and_then(|t| self.section_data(t)),
            ) else {
                continue;
            };
            for e in data.chunks_exact(24) {
                let name = u32_at(e, 0).unwrap_or(0) as usize;
                out.push(Sym {
                    name: cstr_at(strtab, name),
                    kind: e[4] & 0xf,
                    shndx: u16_at(e, 6).unwrap_or(0),
                    value: u64_at(e, 8).unwrap_or(0),
                    size: u64_at(e, 16).unwrap_or(0),
                    full: s.sh_type == SHT_SYMTAB,
                });
            }
        }
        out
    }

    pub fn loads(&self) -> impl Iterator<Item = &Phdr> {
        self.phdrs.iter().filter(|p| p.p_type == PT_LOAD)
    }

    /// File offset of a virtual address inside some PT_LOAD's file image.
    pub fn vaddr_to_offset(&self, v: u64) -> Option<u64> {
        self.loads()
            .find(|p| v >= p.p_vaddr && v - p.p_vaddr < p.p_filesz)
            .map(|p| v - p.p_vaddr + p.p_offset)
    }
}

fn cstr_at(b: &[u8], o: usize) -> &[u8] {
    let Some(rest) = b.get(o..) else {
        return b"";
    };
    let end = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
    &rest[..end]
}
