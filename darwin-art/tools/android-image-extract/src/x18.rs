//! Disassembly-free x18 usage scan of arm64 ELF files.
//!
//! Every 4-byte-aligned word of every executable section is classified:
//! - `str x30, [x18], #8` (0xf800865e) is a shadow-call-stack push;
//! - `ldr x30, [x18, #-8]!` (0xf85f8e5e) is a shadow-call-stack pop;
//! - any other word is decoded far enough to find general-purpose register
//!   fields equal to 18 in the base A64 load/store, data-processing,
//!   branch/system and GPR<->SIMD transfer encodings.
//!
//! Limits: there are no mapping symbols in stripped files, so data embedded
//! in text (literal pools, strings, OAT method headers) is decoded as if it
//! were code; a few obviously unallocated encodings are filtered out, but not
//! all. SVE/SME and rare encodings (RMIF, MOPS, LSE128, RCpc3) are not
//! decoded. x18 as SP/ZR cannot occur (register 31 is SP/ZR). Native
//! libraries inside APKs are scanned; ones inside other archives are not.
use crate::source::FileSource;
use crate::zip::Archive;
use crate::{Result, invalid, le16, le32, le64};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const SCS_PUSH: u32 = 0xf800_865e;
pub const SCS_POP: u32 = 0xf85f_8e5e;
const EM_AARCH64: u16 = 183;
const SHF_EXECINSTR: u64 = 0x4;
const SHT_NOBITS: u32 = 8;
const PT_LOAD: u32 = 1;
const PF_X: u32 = 1;
const MAX_ELF: u64 = 2 * 1024 * 1024 * 1024;
const SAMPLES: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    ScsPush,
    ScsPop,
    Write,
    Read,
}

fn is18(value: u32) -> bool {
    value & 31 == 18
}

/// Classify one instruction word; `None` if it does not name x18/w18.
pub fn classify(w: u32) -> Option<Use> {
    match w {
        SCS_PUSH => return Some(Use::ScsPush),
        SCS_POP => return Some(Use::ScsPop),
        _ => {}
    }
    let rd = w & 31;
    let rn = w >> 5;
    let rm = w >> 16;
    let ra = w >> 10;
    let bit = |n: u32| (w >> n) & 1 == 1;
    let (mut read, mut write) = (false, false);
    let op0 = (w >> 25) & 0xf;
    if op0 & 0b1110 == 0b1000 {
        // Data processing, immediate.
        match (w >> 23) & 7 {
            0b000 | 0b001 => write = is18(rd), // ADR, ADRP
            0b101 => {
                // MOVN/MOVZ write; MOVK also reads.
                write = is18(rd);
                read = is18(rd) && (w >> 29) & 3 == 3;
            }
            0b111 => {
                write = is18(rd);
                read = is18(rn) || is18(rm);
            }
            // Unallocated: 32-bit logical immediates with N=1, and
            // add/sub-with-tags other than 64-bit ADDG/SUBG.
            0b100 if !bit(31) && bit(22) => {}
            0b011 if !bit(31) || bit(29) || bit(22) => {}
            _ => {
                // Add/sub (with tags), logical, bitfield (BFM reads Rd).
                write = is18(rd);
                read = is18(rn) || ((w >> 23) & 7 == 0b110 && (w >> 29) & 3 == 1 && is18(rd));
            }
        }
    } else if op0 & 0b1110 == 0b1010 {
        // Branches, exception generation and system instructions.
        if w >> 22 == 0b11_0101_0100 {
            // MRS/SYSL write Rt; MSR/SYS read it. Hints and barriers use 31.
            if is18(rd) {
                if bit(21) {
                    write = true;
                } else {
                    read = true;
                }
            }
        } else if w >> 25 == 0b110_1011 {
            // BR/BLR/RET and their PAC forms (modifier in bits 4:0).
            read = is18(rn) || (bit(11) && is18(rd));
        } else if (w >> 25) & 0x3f == 0b01_1010 || (w >> 25) & 0x3f == 0b01_1011 {
            read = is18(rd); // CBZ/CBNZ, TBZ/TBNZ
        }
    } else if op0 & 0b0101 == 0b0100 {
        // Loads and stores.
        let simd = bit(26);
        let class = (w >> 27) & 7;
        let load = bit(22);
        if class == 0b011 && !bit(24) {
            // Load literal: no base register. PRFM (opc 11) names no register.
            write = !simd && w >> 30 != 3 && is18(rd);
        } else if class == 0b101 {
            // Register pairs; pre/post-index also write the base.
            let (rt, rt2) = (is18(rd), is18(ra));
            if !simd {
                write = load && (rt || rt2);
                read = !load && (rt || rt2);
            }
            read |= is18(rn);
            write |= is18(rn) && (w >> 23) & 1 == 1;
        } else if class == 0b001 && (w >> 24) & 7 == 0b000 && !simd {
            // Exclusive, ordered and compare-and-swap: Rs, Rt2, Rt, Rn.
            let cas = bit(23) && bit(21);
            read = is18(rn) || (is18(rd) && (!load || cas)) || (cas && is18(rm));
            write = (load && is18(rd)) || (!bit(23) && !load && is18(rm)) || (cas && is18(rm));
            read |= is18(ra) && !load;
            write |= is18(ra) && load;
        } else if class == 0b001 && (w >> 24) & 6 == 0b100 && simd {
            // SIMD structure loads/stores: base and post-index register.
            read = is18(rn) || (bit(23) && is18(rm));
            write = is18(rn) && bit(23);
        } else if class == 0b111 {
            let opc = (w >> 22) & 3;
            let prfm = !simd && w >> 30 == 3 && opc == 2;
            let rt = !simd && !prfm && is18(rd);
            let loads = opc != 0;
            read = is18(rn) || (rt && !loads);
            write = rt && loads;
            if !bit(24) && bit(21) {
                match (w >> 10) & 3 {
                    // Atomic memory operations: Rs is read, Rt written.
                    0b00 if !simd => {
                        read = is18(rn) || is18(rm);
                        write = is18(rd);
                    }
                    0b10 => read |= is18(rm),          // register offset
                    _ => write |= is18(rn) && bit(11), // PAC loads, writeback
                }
            } else if !bit(24) && (w >> 10) & 1 == 1 {
                write |= is18(rn); // pre/post-index writeback
            }
        } else {
            // Other load/store forms (tags, LDAPR/STLUR): Rt and Rn.
            read = is18(rn) || (!simd && is18(rd));
        }
    } else if op0 & 0b0111 == 0b0101 {
        // Data processing, register.
        write = is18(rd);
        read = is18(rn);
        let group = (w >> 21) & 0xff;
        if !bit(28) {
            read |= is18(rm); // logical / add-sub, shifted or extended
        } else if group == 0b1101_0010 {
            read |= !bit(11) && is18(rm); // conditional compare, register
        } else if group == 0b1101_0110 {
            // 2-source (Rm) and 1-source (opcode2 0 or 1, no Rm); with S=1
            // only SUBPS exists.
            let one_source = bit(30);
            let allocated = if one_source {
                !bit(29) && (w >> 16) & 31 <= 1
            } else {
                !bit(29) || (w >> 10) & 63 == 0
            };
            if !allocated {
                return None;
            }
            read |= !one_source && is18(rm);
        } else if group & 0b1111_1000 == 0b1101_1000 {
            read |= is18(rm) || is18(ra); // 3-source
        } else {
            read |= is18(rm); // add-sub with carry, conditional select
        }
    } else if op0 & 0b0111 == 0b0111 {
        // SIMD/FP: only the transfers between GPRs and vector registers.
        if w & 0x5f20_fc00 == 0x1e20_0000 {
            // FP <-> integer conversions: SCVTF/UCVTF/FMOV-from-GPR read Rn.
            match (w >> 16) & 7 {
                0b010 | 0b011 | 0b111 => read = is18(rn),
                _ => write = is18(rd),
            }
        } else if w & 0xbfe0_8400 == 0x0e00_0400 && !bit(29) {
            match (w >> 11) & 0xf {
                0b0001 | 0b0011 => read = is18(rn),  // DUP/INS (general)
                0b0101 | 0b0111 => write = is18(rd), // SMOV/UMOV
                _ => {}
            }
        }
    }
    if write {
        Some(Use::Write)
    } else if read {
        Some(Use::Read)
    } else {
        None
    }
}

#[derive(Clone, Debug, Default)]
pub struct Counts {
    pub words: u64,
    pub push: u64,
    pub pop: u64,
    pub writes: u64,
    pub reads: u64,
    /// (virtual address, word) of the first non-SCS uses.
    pub samples: Vec<(u64, u32)>,
    /// Every non-SCS word naming x18, with its count.
    pub words_used: BTreeMap<u32, u64>,
}

impl Counts {
    pub fn uses(&self) -> u64 {
        self.push + self.pop + self.writes + self.reads
    }
    fn add(&mut self, other: &Counts) {
        self.words += other.words;
        self.push += other.push;
        self.pop += other.pop;
        self.writes += other.writes;
        self.reads += other.reads;
    }
}

/// Executable byte ranges as (virtual address, bytes).
type TextRanges<'a> = Vec<(u64, &'a [u8])>;

/// Executable ranges of an ELF64 AArch64 file; `None` for any other file.
fn text_ranges(elf: &[u8]) -> Result<Option<TextRanges<'_>>> {
    if elf.len() < 64 || &elf[..4] != b"\x7fELF" || elf[4] != 2 || elf[5] != 1 {
        return Ok(None);
    }
    if le16(elf, 18)? != EM_AARCH64 {
        return Ok(None);
    }
    let slice = |offset: u64, size: u64| -> Result<&[u8]> {
        usize::try_from(offset)
            .ok()
            .zip(usize::try_from(size).ok())
            .and_then(|(o, s)| elf.get(o..o.checked_add(s)?))
            .ok_or_else(|| invalid("ELF range lies outside the file"))
    };
    let mut ranges = Vec::new();
    let shoff = le64(elf, 0x28)?;
    let shentsize = u64::from(le16(elf, 0x3a)?);
    let shnum = u64::from(le16(elf, 0x3c)?);
    if shoff != 0 && shnum != 0 && shentsize >= 64 {
        for i in 0..shnum {
            let sh = slice(shoff + i * shentsize, 64)?;
            let flags = le64(sh, 8)?;
            if flags & SHF_EXECINSTR != 0 && le32(sh, 4)? != SHT_NOBITS {
                ranges.push((le64(sh, 0x10)?, slice(le64(sh, 0x18)?, le64(sh, 0x20)?)?));
            }
        }
    }
    if ranges.is_empty() {
        let phoff = le64(elf, 0x20)?;
        let phentsize = u64::from(le16(elf, 0x36)?);
        for i in 0..u64::from(le16(elf, 0x38)?) {
            let ph = slice(phoff + i * phentsize, 56)?;
            if le32(ph, 0)? == PT_LOAD && le32(ph, 4)? & PF_X != 0 {
                ranges.push((le64(ph, 0x10)?, slice(le64(ph, 8)?, le64(ph, 0x20)?)?));
            }
        }
    }
    Ok(Some(ranges))
}

/// Scan one file's bytes; `None` if it is not an arm64 ELF64.
pub fn scan_elf(elf: &[u8]) -> Result<Option<Counts>> {
    let Some(ranges) = text_ranges(elf)? else {
        return Ok(None);
    };
    let mut counts = Counts::default();
    for (vaddr, bytes) in ranges {
        // Words are aligned relative to the section's address.
        let skip = ((4 - vaddr % 4) % 4) as usize;
        for (i, chunk) in bytes.get(skip..).unwrap_or(&[]).chunks_exact(4).enumerate() {
            let word = u32::from_le_bytes(chunk.try_into().unwrap());
            counts.words += 1;
            match classify(word) {
                Some(Use::ScsPush) => counts.push += 1,
                Some(Use::ScsPop) => counts.pop += 1,
                Some(kind) => {
                    if kind == Use::Write {
                        counts.writes += 1;
                    } else {
                        counts.reads += 1;
                    }
                    *counts.words_used.entry(word).or_default() += 1;
                    if counts.samples.len() < SAMPLES {
                        counts.samples.push((vaddr + (skip + i * 4) as u64, word));
                    }
                }
                None => {}
            }
        }
    }
    Ok(Some(counts))
}

#[derive(Default)]
pub struct Scan {
    /// Non-SCS instruction words naming x18 -> (occurrences, files).
    pub histogram: BTreeMap<u32, (u64, u64)>,
    pub elf_files: u64,
    pub other_elf: u64,
    pub apk_libraries: u64,
    pub unreadable: Vec<(PathBuf, String)>,
    /// Relative path (with `!/entry` for APK members) -> counts.
    pub files: BTreeMap<String, Counts>,
}

fn walk(directory: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(&entry.path(), out)?;
        } else if kind.is_file() {
            out.push(entry.path());
        }
    }
    Ok(())
}

fn scan_path(path: &Path) -> Result<Vec<(String, Option<Counts>)>> {
    let mut magic = [0u8; 4];
    let size = fs::metadata(path)?.len();
    if size < 64 {
        return Ok(Vec::new());
    }
    std::io::Read::read_exact(&mut fs::File::open(path)?, &mut magic)?;
    if &magic == b"\x7fELF" {
        if size > MAX_ELF {
            return Err(invalid("ELF file is too large"));
        }
        return Ok(vec![(String::new(), scan_elf(&fs::read(path)?)?)]);
    }
    let name = path.to_string_lossy();
    if &magic == b"PK\x03\x04" && (name.ends_with(".apk") || name.ends_with(".jar")) {
        // Native libraries loaded directly from (or extracted out of) APKs.
        let source = FileSource::open(path)?;
        let archive = Archive::open(&source)?;
        let mut out = Vec::new();
        for entry in &archive.entries {
            let member = String::from_utf8_lossy(&entry.name).into_owned();
            if member.starts_with("lib/") && member.ends_with(".so") {
                let bytes = archive.read(entry, MAX_ELF)?;
                if bytes.starts_with(b"\x7fELF") {
                    out.push((format!("!/{member}"), scan_elf(&bytes)?));
                }
            }
        }
        return Ok(out);
    }
    Ok(Vec::new())
}

/// Scan every ELF (and APK-embedded library) under `root`, in parallel.
pub fn scan_tree(root: &Path) -> Result<Scan> {
    let mut paths = Vec::new();
    walk(root, &mut paths)?;
    paths.sort();
    let next = Mutex::new(0usize);
    let scan = Mutex::new(Scan::default());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                loop {
                    let index = {
                        let mut next = next.lock().unwrap();
                        *next += 1;
                        *next - 1
                    };
                    let Some(path) = paths.get(index) else { break };
                    let relative = path
                        .strip_prefix(root)
                        .unwrap_or(path)
                        .display()
                        .to_string();
                    let result = scan_path(path);
                    let mut scan = scan.lock().unwrap();
                    match result {
                        Ok(found) => {
                            for (member, counts) in found {
                                if !member.is_empty() {
                                    scan.apk_libraries += 1;
                                }
                                match counts {
                                    Some(counts) => {
                                        scan.elf_files += 1;
                                        for (word, count) in &counts.words_used {
                                            let entry = scan.histogram.entry(*word).or_default();
                                            entry.0 += count;
                                            entry.1 += 1;
                                        }
                                        scan.files.insert(format!("{relative}{member}"), counts);
                                    }
                                    None => scan.other_elf += 1,
                                }
                            }
                        }
                        Err(error) => scan.unreadable.push((path.clone(), error.to_string())),
                    }
                }
            });
        }
    });
    Ok(scan.into_inner().unwrap())
}

/// The partition (mount) or APEX a path relative to OUTDIR belongs to.
pub fn partition_of(relative: &str) -> String {
    let mut parts = relative.split('/');
    match parts.next().unwrap_or_default() {
        "apex" => format!("apex/{}", parts.next().unwrap_or_default()),
        top @ ("system_ext" | "product" | "vendor" | "odm" | "system_dlkm" | "vendor_dlkm"
        | "odm_dlkm" | "ramdisk") => top.to_owned(),
        _ => "system".to_owned(),
    }
}

/// Files on the path ADR 0012 runs first, relative to OUTDIR.
pub const CRITICAL: &[(&str, &str)] = &[
    ("linker64", "apex/com.android.runtime/bin/linker64"),
    (
        "bionic libc",
        "apex/com.android.runtime/lib64/bionic/libc.so",
    ),
    (
        "bionic libm",
        "apex/com.android.runtime/lib64/bionic/libm.so",
    ),
    (
        "bionic libdl",
        "apex/com.android.runtime/lib64/bionic/libdl.so",
    ),
    ("libart", "apex/com.android.art/lib64/libart.so"),
    ("libartbase", "apex/com.android.art/lib64/libartbase.so"),
    (
        "libnativeloader",
        "apex/com.android.art/lib64/libnativeloader.so",
    ),
    ("libc++ (system)", "system/lib64/libc++.so"),
    ("libandroid_runtime", "system/lib64/libandroid_runtime.so"),
    ("libbinder", "system/lib64/libbinder.so"),
    ("libbinder_ndk", "system/lib64/libbinder_ndk.so"),
    ("libutils", "system/lib64/libutils.so"),
    ("libhwui", "system/lib64/libhwui.so"),
    ("libgui", "system/lib64/libgui.so"),
    ("libui", "system/lib64/libui.so"),
    ("libandroid_servers", "system/lib64/libandroid_servers.so"),
    ("servicemanager", "system/bin/servicemanager"),
    ("surfaceflinger", "system/bin/surfaceflinger"),
    ("app_process64", "system/bin/app_process64"),
    (
        "boot.oat (ART boot image)",
        "system/framework/arm64/boot.oat",
    ),
    ("services.odex", "system/framework/oat/arm64/services.odex"),
];

pub fn print(scan: &Scan, out: &mut dyn std::io::Write) -> std::io::Result<()> {
    let mut groups: BTreeMap<String, (u64, Counts, u64)> = BTreeMap::new();
    for (path, counts) in &scan.files {
        let group = groups.entry(partition_of(path)).or_default();
        group.0 += 1;
        group.1.add(counts);
        if counts.uses() > 0 {
            group.2 += 1;
        }
    }
    writeln!(
        out,
        "scanned arm64 ELF files: {} (of which APK members: {}); non-arm64 ELF: {}; unreadable: {}",
        scan.elf_files,
        scan.apk_libraries,
        scan.other_elf,
        scan.unreadable.len()
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "{:<44} {:>7} {:>13} {:>8} {:>8} {:>8} {:>8} {:>10}",
        "partition", "files", "words", "scs_push", "scs_pop", "writes", "reads", "files_x18"
    )?;
    for (name, (files, c, users)) in &groups {
        writeln!(
            out,
            "{:<44} {:>7} {:>13} {:>8} {:>8} {:>8} {:>8} {:>10}",
            name, files, c.words, c.push, c.pop, c.writes, c.reads, users
        )?;
    }
    writeln!(out)?;
    writeln!(
        out,
        "files naming x18 (partition, path, scs_push, scs_pop, writes, reads, samples):"
    )?;
    for (path, c) in scan.files.iter().filter(|(_, c)| c.uses() > 0) {
        let samples: Vec<String> = c
            .samples
            .iter()
            .map(|(address, word)| format!("{address:#x}:{word:08x}"))
            .collect();
        writeln!(
            out,
            "  {:<16} {} push={} pop={} writes={} reads={} [{}]",
            partition_of(path),
            path,
            c.push,
            c.pop,
            c.writes,
            c.reads,
            samples.join(" ")
        )?;
    }
    writeln!(out)?;
    let mut common: Vec<_> = scan.histogram.iter().collect();
    common.sort_by_key(|(word, (count, files))| {
        (std::cmp::Reverse(*files), std::cmp::Reverse(*count), **word)
    });
    writeln!(
        out,
        "most widespread non-SCS x18 words (word, files, occurrences):"
    )?;
    for (word, (count, files)) in common.iter().take(25) {
        writeln!(out, "  {word:08x} files={files} occurrences={count}")?;
    }
    writeln!(out)?;
    writeln!(out, "critical path (ADR 0012):")?;
    for (label, path) in CRITICAL {
        match scan.files.get(*path) {
            Some(c) => writeln!(
                out,
                "  {:<30} {:<58} words={} push={} pop={} writes={} reads={}",
                label, path, c.words, c.push, c.pop, c.writes, c.reads
            )?,
            None => writeln!(out, "  {label:<30} {path:<58} NOT FOUND")?,
        }
    }
    for (path, error) in &scan.unreadable {
        writeln!(out, "unreadable: {}: {error}", path.display())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Encodings produced by the NDK r28 assembler (llvm-objdump verified).
    const WRITES: &[u32] = &[
        0xaa0003f2, // mov x18, x0
        0xd28000b2, // mov x18, #5
        0xf2a00032, // movk x18, #1, lsl #16
        0x90000012, // adrp x18, 0
        0x910043f2, // add x18, sp, #16
        0xf9400012, // ldr x18, [x0]
        0xa9404ff2, // ldp x18, x19, [sp]
        0xa9c14be0, // ldp x0, x18, [sp, #16]!
        0xd53bd052, // mrs x18, tpidr_el0
        0x58000012, // ldr x18, <literal>
        0x9e660012, // fmov x18, d0
        0x0e0c3c12, // umov w18, v0.s[1]
        0x9b010812, // madd x18, x0, x1, x2
        0x9a810012, // csel x18, x0, x1, eq
        0xf8200032, // ldadd x0, x18, [x1]
        0xd3442c12, // ubfx x18, x0, #4, #8
        0xc85f7c12, // ldxr x18, [x0]
        0xc8127c20, // stxr w18, x0, [x1] (status)
        0xc8b27c20, // cas x18, x0, [x1]
    ];
    const READS: &[u32] = &[
        0xd61f0240, // br x18
        0xd63f0240, // blr x18
        0xb4000012, // cbz x18
        0x37180012, // tbnz w18, #3
        0xf9000012, // str x18, [x0]
        0xf9400640, // ldr x0, [x18, #8]
        0xf8726820, // ldr x0, [x1, x18]
        0xa9bf03f2, // stp x18, x0, [sp, #-16]!
        0x8b010240, // add x0, x18, x1
        0x9b024820, // madd x0, x1, x2, x18
        0xd51bd052, // msr tpidr_el0, x18
        0x4e040e40, // dup v0.4s, w18
        0x9e620240, // scvtf d0, x18
        0xfa430a40, // ccmp x18, #3, #0, eq
    ];
    const NONE: &[u32] = &[
        0xf9404820, // ldr x0, [x1, #0x90]  (imm12 bits look like 18)
        0xf8412020, // ldur x0, [x1, #18]
        0xfa520800, // ccmp x0, #18, #0, eq (imm5 = 18)
        0x4ea18412, // add v18.4s, v0.4s, v1.4s
        0x1e612812, // fadd d18, d0, d1
        0x3dc00012, // ldr q18, [x0]
        0xad404800, // ldp q0, q18, [x0]
        0xd2800240, // mov x0, #18
        0x14000000, // b .
        0xd503201f, // nop
        0xd65f03c0, // ret
        0x5c000012, // ldr d18, <literal>
        0x52800240, // mov w0, #18
        0x52412072, // unallocated logical immediate (sf=0, N=1): ASCII in text
        0xfad967b2, // unallocated 1-source (S=1)
        0x71c71c72, // unallocated add/sub with tags (sf=0)
    ];

    #[test]
    fn classifies_known_encodings() {
        assert_eq!(classify(SCS_PUSH), Some(Use::ScsPush));
        assert_eq!(classify(SCS_POP), Some(Use::ScsPop));
        for word in WRITES {
            assert_eq!(classify(*word), Some(Use::Write), "{word:08x}");
        }
        for word in READS {
            assert_eq!(classify(*word), Some(Use::Read), "{word:08x}");
        }
        for word in NONE {
            assert_eq!(classify(*word), None, "{word:08x}");
        }
    }

    /// A minimal ELF64 AArch64 file with one executable section.
    fn elf(words: &[u32], machine: u16) -> Vec<u8> {
        let text: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut out = vec![0u8; 64];
        out[..4].copy_from_slice(b"\x7fELF");
        out[4] = 2;
        out[5] = 1;
        out[18..20].copy_from_slice(&machine.to_le_bytes());
        let text_at = out.len() as u64;
        out.extend_from_slice(&text);
        let shoff = out.len() as u64;
        out.extend_from_slice(&[0u8; 64]); // null section
        let mut sh = [0u8; 64];
        sh[4..8].copy_from_slice(&1u32.to_le_bytes());
        sh[8..16].copy_from_slice(&(SHF_EXECINSTR | 2).to_le_bytes());
        sh[0x10..0x18].copy_from_slice(&0x1000u64.to_le_bytes());
        sh[0x18..0x20].copy_from_slice(&text_at.to_le_bytes());
        sh[0x20..0x28].copy_from_slice(&(text.len() as u64).to_le_bytes());
        out.extend_from_slice(&sh);
        out[0x28..0x30].copy_from_slice(&shoff.to_le_bytes());
        out[0x3a..0x3c].copy_from_slice(&64u16.to_le_bytes());
        out[0x3c..0x3e].copy_from_slice(&2u16.to_le_bytes());
        out
    }

    #[test]
    fn scans_executable_sections() {
        let file = elf(
            &[SCS_PUSH, 0xd503201f, 0xaa0003f2, SCS_POP, 0xd65f03c0],
            EM_AARCH64,
        );
        let counts = scan_elf(&file).unwrap().unwrap();
        assert_eq!(
            (
                counts.words,
                counts.push,
                counts.pop,
                counts.writes,
                counts.reads
            ),
            (5, 1, 1, 1, 0)
        );
        assert_eq!(counts.samples, vec![(0x1008, 0xaa0003f2)]);
        assert!(scan_elf(&elf(&[SCS_PUSH], 40)).unwrap().is_none());
        assert!(
            scan_elf(b"not an elf file at all, just some bytes padding it out to 64..")
                .unwrap()
                .is_none()
        );
        let mut truncated = file.clone();
        truncated.truncate(file.len() - 10);
        assert!(scan_elf(&truncated).is_err());
    }

    #[test]
    fn groups_paths_by_partition() {
        assert_eq!(
            partition_of("apex/com.android.art/lib64/libart.so"),
            "apex/com.android.art"
        );
        assert_eq!(partition_of("system/lib64/libc.so"), "system");
        assert_eq!(partition_of("vendor/lib64/libx.so"), "vendor");
        assert_eq!(partition_of("ramdisk/init"), "ramdisk");
    }
}
