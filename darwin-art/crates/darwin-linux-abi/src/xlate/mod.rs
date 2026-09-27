//! Ahead-of-time translation of original Android arm64 ELF files (ADR 0012
//! decision 8, after Rosetta 2).
//!
//! [`translate`] is a pure function from the original file's bytes to the
//! translated file's bytes plus a [`Report`]. It rewrites the few
//! instructions that depend on state Darwin owns:
//!
//! | original                 | becomes                                        |
//! |--------------------------|------------------------------------------------|
//! | `svc #0`                 | `b` to a stub that enters the syscall layer     |
//! | `mrs xN, tpidr_el0`      | `b` to a stub loading the guest TP TSD slot     |
//! | `msr tpidr_el0, xN`      | `b` to a stub storing it                        |
//! | `str x30, [x18], #8`     | `b` to a stub pushing x30 on the shadow stack whose pointer lives in a TSD slot |
//! | `ldr x30, [x18, #-8]!`   | `b` to a stub popping it                        |
//! | `mrs xN, ctr_el0`        | `b` to a stub loading the guest CTR_EL0 value   |
//!
//! # Stub placement
//!
//! The stubs, and a copy of the program header table, go into one extra
//! `R+X` PT_LOAD appended after the image's last segment. The original
//! `linker64` reads the translated file's program headers, so it reserves
//! the stub segment as part of the library's span and maps it itself; the
//! syscall layer only has to hand it the translated file when it opens the
//! original. Nothing lives outside the span the linker owns, so `dlclose`,
//! `dl_iterate_phdr` and address-space reuse keep working unchanged.
//!
//! The stub segment's `p_vaddr - p_offset` equals that of the first
//! PT_LOAD, so `load_base + e_phoff` (how `linker64` finds its own program
//! headers, and the linker's fallback when there is no PT_PHDR) lands on the
//! moved table. Original bytes keep their offsets; only rewritten words
//! change. A site further than ±128 MiB from the stub segment (a span larger
//! than 128 MiB) becomes a `brk` that the SIGTRAP handler emulates.
//!
//! # Code and data
//!
//! Only words inside SHF_EXECINSTR PROGBITS sections that lie in PF_X
//! segments are candidates. Inside them, STT_OBJECT symbols and `$d`
//! mapping-symbol ranges are data and are never rewritten. A rewritten site
//! not covered by an STT_FUNC symbol or a `$x` range is counted as
//! ambiguous. Files without section headers fall back to whole PF_X
//! segments, and every site there is ambiguous.
//!
//! # Integrity hashes
//!
//! A file carrying a BoringSSL FIPS module gets its module hash recomputed
//! over the rewritten bytes, as BoringSSL's build does after linking
//! ([`fips`]). The stub segment lies after every original segment, outside
//! the hashed ranges.

pub mod ehframe;
pub mod elf;
pub mod fips;

use sha2::{Digest, Sha256};

use crate::a64::{self, Kind};
use elf::{Elf, PF_R, PF_X, PHDR_SIZE, PT_LOAD, PT_PHDR, Phdr};

/// Bump when the translated output for the same input changes: encodings,
/// stub layout, the TSD slot numbers in `a64::slot`, or identification.
pub const VERSION: u32 = 2;

pub const PAGE: u64 = 16384;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// The value rewritten `mrs xN, ctr_el0` sites produce.
    pub ctr_el0: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            ctr_el0: a64::host_ctr_el0(),
        }
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Cache key of an original file with this translator.
pub fn cache_key(original: &[u8]) -> String {
    key_for_digest(&sha256_hex(original))
}

pub fn key_for_digest(sha256: &str) -> String {
    format!("{sha256}-v{VERSION}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// Section headers plus code evidence: `.eh_frame` FDEs, STT_FUNC
    /// symbols and `$x` mapping symbols (and data: STT_OBJECT, `$d`).
    SectionsAndEvidence,
    /// Section headers, nothing to confirm code.
    Sections,
    /// No section headers: whole PF_X segments.
    Segments,
}

impl Method {
    pub fn name(self) -> &'static str {
        match self {
            Method::SectionsAndEvidence => "sections+evidence",
            Method::Sections => "sections",
            Method::Segments => "segments",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Site {
    pub vaddr: u64,
    pub offset: u64,
    pub kind: Kind,
    pub rt: u32,
    /// Not covered by any STT_FUNC symbol or `$x` range.
    pub ambiguous: bool,
}

#[derive(Clone, Debug)]
pub struct Analysis {
    pub sites: Vec<Site>,
    pub method: Method,
    /// Pattern words skipped because symbols mark them as data.
    pub data_excluded: usize,
    /// Pattern words in PF_X segments outside executable sections, skipped.
    pub outside_code: usize,
    /// The file is an OAT/ODEX (has an `oatdata` dynamic symbol).
    pub is_oat: bool,
    /// Smallest PT_LOAD alignment.
    pub min_load_align: u64,
    /// A BoringSSL FIPS module whose hash must follow rewritten bytes.
    pub fips: Option<fips::Module>,
}

impl Analysis {
    pub fn ambiguous(&self) -> usize {
        self.sites.iter().filter(|s| s.ambiguous).count()
    }
}

/// Sorted, merged half-open intervals.
#[derive(Default)]
struct Intervals(Vec<(u64, u64)>);

impl Intervals {
    fn push(&mut self, a: u64, b: u64) {
        if b > a {
            self.0.push((a, b));
        }
    }
    fn normalize(&mut self) {
        self.0.sort_unstable();
        let mut out: Vec<(u64, u64)> = Vec::with_capacity(self.0.len());
        for &(a, b) in &self.0 {
            match out.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => out.push((a, b)),
            }
        }
        self.0 = out;
    }
    fn contains(&self, v: u64) -> bool {
        let i = self.0.partition_point(|&(a, _)| a <= v);
        i > 0 && v < self.0[i - 1].1
    }
}

fn is_mapping(name: &[u8], c: u8) -> bool {
    name.len() >= 2 && name[0] == b'$' && name[1] == c && (name.len() == 2 || name[2] == b'.')
}

/// Identify code and find every site to rewrite.
pub fn analyze(elf: &Elf) -> Analysis {
    let b = elf.bytes;
    let loads: Vec<Phdr> = elf.loads().copied().collect();
    let min_load_align = loads.iter().map(|p| p.p_align.max(1)).min().unwrap_or(1);
    // Executable file images: [vaddr, vaddr + filesz).
    let exec: Vec<Phdr> = loads
        .iter()
        .filter(|p| p.p_flags & PF_X != 0)
        .copied()
        .collect();
    let in_exec = |a: u64, e: u64| -> Vec<(u64, u64)> {
        exec.iter()
            .filter_map(|p| {
                let lo = a.max(p.p_vaddr);
                let hi = e.min(p.p_vaddr.saturating_add(p.p_filesz));
                (hi > lo).then_some((lo, hi))
            })
            .collect()
    };

    let mut code = Intervals::default();
    let method_has_sections = elf.shdrs.iter().any(|s| s.sh_flags & elf::SHF_ALLOC != 0);
    let exec_sections: Vec<usize> = elf
        .shdrs
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.sh_type == elf::SHT_PROGBITS
                && s.sh_flags & (elf::SHF_ALLOC | elf::SHF_EXECINSTR)
                    == elf::SHF_ALLOC | elf::SHF_EXECINSTR
        })
        .map(|(i, _)| i)
        .collect();
    if method_has_sections {
        for &i in &exec_sections {
            let s = &elf.shdrs[i];
            for (lo, hi) in in_exec(s.sh_addr, s.sh_addr.saturating_add(s.sh_size)) {
                code.push(lo, hi);
            }
        }
    } else {
        // No sections: whole PF_X file images, minus the ELF and program
        // headers when a PF_X segment maps file offset 0.
        let hdr_end = elf.ehdr.e_phoff + elf.ehdr.e_phnum as u64 * PHDR_SIZE as u64;
        for p in &exec {
            let mut lo = p.p_vaddr;
            if p.p_offset == 0 {
                lo += hdr_end.max(elf::EHDR_SIZE as u64);
            }
            code.push(lo, p.p_vaddr.saturating_add(p.p_filesz));
        }
    }
    code.normalize();

    // Symbols: data carve-outs and code evidence.
    let mut data = Intervals::default();
    let mut evidence = Intervals::default();
    let mut is_oat = false;
    let syms = elf.symbols();
    let mut mapping: Vec<(u16, u64, bool)> = Vec::new();
    for s in &syms {
        if !s.full && s.name == b"oatdata" {
            is_oat = true;
        }
        // Only symbols inside an allocated section that exists count. Stripped
        // files keep symbols of removed sections (Rust dylibs export
        // `rust_metadata_*` at value 0 in the dropped `.rustc` section), and
        // those say nothing about the loaded image.
        let Some(sec) = elf
            .shdrs
            .get(s.shndx as usize)
            .filter(|_| s.shndx != elf::SHN_UNDEF && s.shndx < elf::SHN_LORESERVE)
            .filter(|sec| sec.sh_flags & elf::SHF_ALLOC != 0)
        else {
            continue;
        };
        let (sec_lo, sec_hi) = (sec.sh_addr, sec.sh_addr.saturating_add(sec.sh_size));
        if s.value < sec_lo || s.value > sec_hi || s.value.saturating_add(s.size) > sec_hi {
            continue;
        }
        if s.full && (is_mapping(s.name, b'd') || is_mapping(s.name, b'x')) {
            if exec_sections.contains(&(s.shndx as usize)) {
                mapping.push((s.shndx, s.value, s.name[1] == b'd'));
            }
            continue;
        }
        match s.kind {
            elf::STT_OBJECT if s.size > 0 => data.push(s.value, s.value.saturating_add(s.size)),
            elf::STT_FUNC if s.size > 0 => evidence.push(s.value, s.value.saturating_add(s.size)),
            _ => {}
        }
    }
    for s in &elf.shdrs {
        if s.sh_flags & elf::SHF_ALLOC != 0
            && elf.section_name(s) == b".eh_frame"
            && let Some(data) = elf.section_data(s)
        {
            for (lo, hi) in ehframe::fde_ranges(data, s.sh_addr) {
                evidence.push(lo, hi);
            }
        }
    }
    mapping.sort_unstable();
    for (i, &(sec, v, is_data)) in mapping.iter().enumerate() {
        let s = &elf.shdrs[sec as usize];
        let end = match mapping.get(i + 1) {
            Some(&(nsec, nv, _)) if nsec == sec => nv,
            _ => s.sh_addr.saturating_add(s.sh_size),
        };
        if is_data {
            data.push(v, end);
        } else {
            evidence.push(v, end);
        }
    }
    data.normalize();
    evidence.normalize();
    let has_symbols = !evidence.0.is_empty() || !data.0.is_empty();
    let method = if !method_has_sections {
        Method::Segments
    } else if has_symbols {
        Method::SectionsAndEvidence
    } else {
        Method::Sections
    };

    let mut sites = Vec::new();
    let mut data_excluded = 0;
    let mut outside_code = 0;
    for p in &exec {
        let start = (p.p_vaddr + 3) & !3;
        let end = p.p_vaddr.saturating_add(p.p_filesz);
        let mut v = start;
        while v + 4 <= end {
            let off = v - p.p_vaddr + p.p_offset;
            let Some(w) = elf::u32_at(b, off as usize) else {
                break;
            };
            if let Some((kind, rt)) = a64::classify(w) {
                if !code.contains(v) {
                    outside_code += 1;
                } else if data.contains(v) {
                    data_excluded += 1;
                } else {
                    sites.push(Site {
                        vaddr: v,
                        offset: off,
                        kind,
                        rt,
                        ambiguous: method != Method::SectionsAndEvidence || !evidence.contains(v),
                    });
                }
            }
            v += 4;
        }
    }
    sites.sort_unstable_by_key(|s| s.vaddr);
    sites.dedup_by_key(|s| s.vaddr);
    Analysis {
        sites,
        method,
        data_excluded,
        outside_code,
        is_oat,
        min_load_align,
        fips: fips::find(elf),
    }
}

/// A translated file: the original bytes with rewritten words, then (after
/// a hole) the stub segment at `stub_offset`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub patched: Vec<u8>,
    pub stub_offset: u64,
    pub stub_segment: Vec<u8>,
}

impl Output {
    pub fn len(&self) -> u64 {
        self.stub_offset + self.stub_segment.len() as u64
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = self.patched.clone();
        v.resize(self.stub_offset as usize, 0);
        v.extend_from_slice(&self.stub_segment);
        v
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to rewrite: map the original.
    Identity,
    Translated(Output),
    /// Cannot be translated ahead of time; the load-time path handles it.
    Unsupported(String),
}

impl Outcome {
    pub fn name(&self) -> &'static str {
        match self {
            Outcome::Identity => "identity",
            Outcome::Translated(_) => "translated",
            Outcome::Unsupported(_) => "unsupported",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub sites: [usize; 6],
    pub brk_fallback: usize,
    pub ambiguous: usize,
    pub data_excluded: usize,
    pub outside_code: usize,
    pub method: &'static str,
    pub is_oat: bool,
    /// The file's BoringSSL FIPS module hash was re-injected.
    pub fips_rehashed: bool,
    pub stub_vaddr: u64,
    pub stub_size: u64,
    pub ctr_el0: u32,
}

impl Report {
    pub fn total_sites(&self) -> usize {
        self.sites.iter().sum()
    }
}

pub struct Translation {
    pub outcome: Outcome,
    pub report: Report,
}

fn page_up(v: u64) -> u64 {
    v.div_ceil(PAGE) * PAGE
}

/// Translate an original ELF file. Errors only for input that is not an
/// AArch64 ELF64 file this layer could load at all.
pub fn translate(bytes: &[u8], opts: &Options) -> Result<Translation, String> {
    let elf = elf::parse(bytes)?;
    let a = analyze(&elf);
    let mut report = Report {
        ambiguous: a.ambiguous(),
        data_excluded: a.data_excluded,
        outside_code: a.outside_code,
        method: a.method.name(),
        is_oat: a.is_oat,
        ctr_el0: opts.ctr_el0,
        ..Default::default()
    };
    for s in &a.sites {
        report.sites[s.kind as usize] += 1;
    }
    let done = |outcome, report| Ok(Translation { outcome, report });
    if a.sites.is_empty() {
        return done(Outcome::Identity, report);
    }
    // 4 KiB-aligned ELFs go through linker64's 16 KiB compat loader, which
    // copies segments into anonymous memory; the load-time path covers them.
    if a.min_load_align < PAGE {
        return done(
            Outcome::Unsupported(format!(
                "PT_LOAD alignment {:#x} < 16 KiB (linker64 compat loading)",
                a.min_load_align
            )),
            report,
        );
    }
    let loads: Vec<(usize, Phdr)> = elf
        .phdrs
        .iter()
        .enumerate()
        .filter(|(_, p)| p.p_type == PT_LOAD)
        .map(|(i, p)| (i, *p))
        .collect();
    let first = loads
        .iter()
        .map(|(_, p)| p)
        .min_by_key(|p| p.p_vaddr)
        .unwrap();
    if first.p_vaddr < first.p_offset || (first.p_vaddr - first.p_offset) % PAGE != 0 {
        return done(
            Outcome::Unsupported("first PT_LOAD p_vaddr - p_offset is not page aligned".into()),
            report,
        );
    }
    let delta = first.p_vaddr - first.p_offset;
    let vaddr_end = page_up(
        loads
            .iter()
            .map(|(_, p)| p.p_vaddr.saturating_add(p.p_memsz))
            .max()
            .unwrap(),
    );
    let stub_offset = page_up((bytes.len() as u64).max(vaddr_end.saturating_sub(delta)));
    let stub_vaddr = stub_offset + delta;
    let phnum = elf.phdrs.len() + 1;
    if phnum >= 0xff00 {
        return done(
            Outcome::Unsupported("too many program headers".into()),
            report,
        );
    }
    let table_len = phnum * PHDR_SIZE;
    let stubs_start = table_len.next_multiple_of(16);

    // Place stubs in site order; patch the sites.
    let mut patched = bytes.to_vec();
    let mut stubs: Vec<u32> = Vec::new();
    for s in &a.sites {
        let at = stub_vaddr + (stubs_start + stubs.len() * 4) as u64;
        let word = match a64::stub_for(s.kind, s.rt, opts.ctr_el0, at, s.vaddr) {
            Some(words) => {
                stubs.extend_from_slice(&words);
                a64::encode_b(s.vaddr, at).unwrap()
            }
            None => {
                report.brk_fallback += 1;
                a64::brk_fallback(s.kind, s.rt)
            }
        };
        patched[s.offset as usize..s.offset as usize + 4].copy_from_slice(&word.to_le_bytes());
    }
    let seg_len = stubs_start + stubs.len() * 4;

    // New program header table: PT_PHDR moved, the stub PT_LOAD inserted
    // after the last PT_LOAD so PT_LOADs stay sorted by p_vaddr.
    let last_load_index = loads.last().unwrap().0;
    let stub_phdr = Phdr {
        p_type: PT_LOAD,
        p_flags: PF_R | PF_X,
        p_offset: stub_offset,
        p_vaddr: stub_vaddr,
        p_paddr: stub_vaddr,
        p_filesz: seg_len as u64,
        p_memsz: seg_len as u64,
        p_align: PAGE,
    };
    let mut table = Vec::with_capacity(phnum);
    for (i, p) in elf.phdrs.iter().enumerate() {
        let mut p = *p;
        if p.p_type == PT_PHDR {
            p.p_offset = stub_offset;
            p.p_vaddr = stub_vaddr;
            p.p_paddr = stub_vaddr;
            p.p_filesz = table_len as u64;
            p.p_memsz = table_len as u64;
        }
        table.push(p);
        if i == last_load_index {
            table.push(stub_phdr);
        }
    }
    let mut seg = Vec::with_capacity(seg_len);
    for p in &table {
        seg.extend_from_slice(&p.to_bytes());
    }
    seg.resize(stubs_start, 0);
    for w in &stubs {
        seg.extend_from_slice(&w.to_le_bytes());
    }
    patched[32..40].copy_from_slice(&stub_offset.to_le_bytes());
    patched[56..58].copy_from_slice(&(phnum as u16).to_le_bytes());
    if let Some(m) = &a.fips {
        m.reinject(&mut patched);
        report.fips_rehashed = true;
    }

    report.stub_vaddr = stub_vaddr;
    report.stub_size = seg_len as u64;
    done(
        Outcome::Translated(Output {
            patched,
            stub_offset,
            stub_segment: seg,
        }),
        report,
    )
}

#[cfg(test)]
mod exec_tests;
#[cfg(test)]
mod tests;
