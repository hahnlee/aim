//! Program loading as Linux's `binfmt_elf` does it: reserve the image span,
//! place PT_LOAD segments at a load bias honouring their alignment, load the
//! PT_INTERP interpreter the same way, and build the initial stack
//! (argc, argv, envp, auxv).
//!
//! A file with a translation-cache entry (or one with nothing to rewrite) is
//! mapped file-backed: read-only segments shared, writable ones
//! copy-on-write. Anything else is copied and rewritten before it becomes
//! executable (see `xrt`), and a BoringSSL FIPS module in it gets its
//! integrity hash recomputed (see `xlate::fips`).

use std::collections::BTreeMap;
use std::ffi::CStr;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use std::path::Path;

use crate::elf::{self, PF_R, PF_W, PF_X, PT_INTERP, PT_LOAD, PT_PHDR};
use crate::{patch, xlate, xrt};

pub const PAGE: u64 = 16384;

fn page_down(v: u64) -> u64 {
    v & !(PAGE - 1)
}
fn page_up(v: u64) -> u64 {
    (v + PAGE - 1) & !(PAGE - 1)
}

#[derive(Debug)]
pub struct Image {
    pub bias: u64,
    pub entry: u64,
    pub phdr: u64,
    pub phnum: u64,
    pub end: u64,
    pub interp: Option<String>,
    /// Where the code came from (translation cache, original, load time).
    pub source: String,
    /// Sites rewritten at load time (zero for file-backed images).
    pub stats: patch::PatchStats,
}

fn prot_of(flags: u32) -> i32 {
    let mut p = 0;
    if flags & PF_R != 0 {
        p |= libc::PROT_READ;
    }
    if flags & PF_W != 0 {
        p |= libc::PROT_WRITE;
    }
    if flags & PF_X != 0 {
        p |= libc::PROT_EXEC;
    }
    p
}

struct Headers {
    hdr: elf::Header,
    phdrs: Vec<elf::Phdr>,
    loads: Vec<elf::Phdr>,
    lo: u64,
    hi: u64,
    align: u64,
}

fn read_headers(file: &std::fs::File, name: &str) -> Result<Headers, String> {
    let mut head = vec![0u8; 64];
    file.read_exact_at(&mut head, 0)
        .map_err(|e| format!("{name}: {e}"))?;
    let hdr = elf::parse_header(&head).map_err(|e| format!("{name}: {e}"))?;
    let ph_end = hdr.e_phoff as usize + hdr.e_phnum as usize * 56;
    let mut buf = vec![0u8; ph_end];
    file.read_exact_at(&mut buf, 0)
        .map_err(|e| format!("{name}: {e}"))?;
    let phdrs = elf::parse_phdrs(&buf, &hdr)?;
    let loads: Vec<_> = phdrs
        .iter()
        .filter(|p| p.p_type == PT_LOAD)
        .copied()
        .collect();
    if loads.is_empty() {
        return Err(format!("{name}: no PT_LOAD segments"));
    }
    let lo = page_down(loads.iter().map(|p| p.p_vaddr).min().unwrap());
    let hi = page_up(loads.iter().map(|p| p.p_vaddr + p.p_memsz).max().unwrap());
    let align = loads.iter().map(|p| p.p_align).max().unwrap().max(PAGE);
    if !align.is_power_of_two() {
        return Err(format!(
            "{name}: PT_LOAD alignment {align:#x} is not a power of two"
        ));
    }
    Ok(Headers {
        hdr,
        phdrs,
        loads,
        lo,
        hi,
        align,
    })
}

/// Reserve the whole span (plus alignment slack for ET_DYN), like the
/// kernel's total_mapping_size reservation. Returns the load bias.
fn reserve(h: &Headers, name: &str) -> Result<u64, String> {
    let span = h.hi - h.lo;
    if h.hdr.e_type == elf::ET_DYN {
        // SAFETY: fresh PROT_NONE reservation.
        let r = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                (span + h.align) as usize,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if r == libc::MAP_FAILED {
            return Err(format!("{name}: cannot reserve {span:#x} bytes"));
        }
        let r = r as u64;
        let start = (r + h.align - 1) & !(h.align - 1);
        // SAFETY: trimming our own reservation.
        unsafe {
            if start > r {
                libc::munmap(r as *mut _, (start - r) as usize);
            }
            let tail = r + span + h.align - (start + span);
            if tail > 0 {
                libc::munmap((start + span) as *mut _, tail as usize);
            }
        }
        Ok(start - h.lo)
    } else {
        // SAFETY: ET_EXEC must live at its link address.
        let r = unsafe {
            libc::mmap(
                h.lo as *mut _,
                span as usize,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if r as u64 != h.lo {
            return Err(format!(
                "{name}: ET_EXEC range {:#x} is not available",
                h.lo
            ));
        }
        Ok(0)
    }
}

/// Whether every PT_LOAD can be mapped from the file page by page.
fn file_mappable(h: &Headers) -> bool {
    h.loads
        .iter()
        .all(|p| p.p_align >= PAGE && p.p_vaddr % PAGE == p.p_offset % PAGE)
        && h.loads
            .windows(2)
            .all(|w| page_up(w[0].p_vaddr + w[0].p_memsz) <= page_down(w[1].p_vaddr))
}

/// Map an ELF file from the host path `host`; `name` labels it in diagnostics.
pub fn load_elf(host: &CStr, name: &str) -> Result<Image, String> {
    let path = host.to_str().map_err(|e| e.to_string())?;
    if let xrt::LoaderSource::File(mapped, what) = xrt::loader_source(Path::new(path)) {
        let file = std::fs::File::open(&mapped).map_err(|e| format!("{name}: {e}"))?;
        let h = read_headers(&file, name)?;
        if file_mappable(&h) {
            let bias = reserve(&h, name)?;
            map_file_backed(&file, &h, bias, name)?;
            let source = format!("{what}: {}", mapped.display());
            return finish(&file, h, bias, name, source, patch::PatchStats::default());
        }
    }
    let file = std::fs::File::open(path).map_err(|e| format!("{name}: {e}"))?;
    let h = read_headers(&file, name)?;
    let bias = reserve(&h, name)?;
    let stats = map_copied(&file, &h, bias, name)?;
    finish(&file, h, bias, name, "load-time rewrite".into(), stats)
}

fn map_file_backed(file: &std::fs::File, h: &Headers, bias: u64, name: &str) -> Result<(), String> {
    let fd = file.as_raw_fd();
    for p in &h.loads {
        let start = page_down(bias + p.p_vaddr);
        let file_end = bias + p.p_vaddr + p.p_filesz;
        let mem_end = page_up(bias + p.p_vaddr + p.p_memsz);
        let writable = p.p_flags & PF_W != 0;
        if p.p_filesz > 0 {
            let len = page_up(file_end) - start;
            let (initial, flags) = if writable {
                (libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE)
            } else {
                // Read-only first: Darwin refuses PROT_EXEC on an unsigned
                // file mapping but allows mprotect to it (experiments/p0/07).
                (libc::PROT_READ, libc::MAP_SHARED)
            };
            // SAFETY: mapping the file inside our reservation.
            let r = unsafe {
                libc::mmap(
                    start as *mut _,
                    len as usize,
                    initial,
                    flags | libc::MAP_FIXED,
                    fd,
                    page_down(p.p_offset) as i64,
                )
            };
            if r == libc::MAP_FAILED {
                return Err(format!(
                    "{name}: cannot map segment at {start:#x}: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if writable && page_up(file_end) > file_end {
                // SAFETY: zeroing the start of bss in the private page.
                unsafe {
                    std::ptr::write_bytes(
                        file_end as *mut u8,
                        0,
                        (page_up(file_end) - file_end) as usize,
                    )
                };
            }
        }
        let anon_start = if p.p_filesz > 0 {
            page_up(file_end)
        } else {
            start
        };
        if mem_end > anon_start {
            // SAFETY: anonymous bss inside our reservation.
            let r = unsafe {
                libc::mmap(
                    anon_start as *mut _,
                    (mem_end - anon_start) as usize,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_FIXED,
                    -1,
                    0,
                )
            };
            if r == libc::MAP_FAILED {
                return Err(format!("{name}: cannot map bss at {anon_start:#x}"));
            }
        }
        let prot = prot_of(p.p_flags);
        // SAFETY: final protection of the segment we mapped.
        if unsafe { libc::mprotect(start as *mut _, (mem_end - start) as usize, prot) } != 0 {
            return Err(format!(
                "{name}: mprotect {start:#x} failed: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

/// Copy the segments into anonymous memory and rewrite the code before it
/// becomes executable (the no-cache path).
fn map_copied(
    file: &std::fs::File,
    h: &Headers,
    bias: u64,
    name: &str,
) -> Result<patch::PatchStats, String> {
    let span = h.hi - h.lo;
    // Populate: every page writable while file contents are copied in.
    // SAFETY: remapping inside our reservation.
    let r = unsafe {
        libc::mmap(
            (bias + h.lo) as *mut _,
            span as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_FIXED,
            -1,
            0,
        )
    };
    if r == libc::MAP_FAILED {
        return Err(format!("{name}: cannot map image"));
    }
    // Pages between segments stay inaccessible, as with the kernel loader.
    let mut page_prot: BTreeMap<u64, i32> = BTreeMap::new();
    for p in &h.loads {
        let dst = bias + p.p_vaddr;
        // SAFETY: dst..dst+memsz lies within the RW mapping above.
        let seg = unsafe { std::slice::from_raw_parts_mut(dst as *mut u8, p.p_filesz as usize) };
        file.read_exact_at(seg, p.p_offset)
            .map_err(|e| format!("{name}: {e}"))?;
        let mut pg = page_down(dst);
        while pg < dst + p.p_memsz {
            *page_prot.entry(pg).or_insert(0) |= prot_of(p.p_flags);
            pg += PAGE;
        }
    }
    // Sites from the translator's code/data identification; a blind scan of
    // the executable segments only if the file cannot be analyzed.
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut whole = vec![0u8; len as usize];
    let analysis = file
        .read_exact_at(&mut whole, 0)
        .ok()
        .and_then(|_| xlate::elf::parse(&whole).ok())
        .map(|e| xlate::analyze(&e));
    let stats = match analysis {
        Some(a) => {
            let sites: Vec<_> = a
                .sites
                .iter()
                .map(|s| (bias + s.vaddr, s.kind, s.rt))
                .collect();
            let stats = patch::rewrite_sites(&sites, bias + h.lo, bias + h.hi, true);
            if let Some(m) = &a.fips {
                // Every segment is still mapped read-write.
                // SAFETY: the module and its hash lie in the loaded segments.
                unsafe {
                    let mem = |(lo, hi): (u64, u64)| {
                        std::slice::from_raw_parts((bias + lo) as *const u8, (hi - lo) as usize)
                    };
                    let hash = xlate::fips::digest(mem(m.text), m.rodata.map(mem));
                    std::ptr::copy_nonoverlapping(
                        hash.as_ptr(),
                        (bias + m.hash_vaddr) as *mut u8,
                        32,
                    );
                }
            }
            stats
        }
        None => {
            let mut stats = patch::PatchStats::default();
            for p in h.loads.iter().filter(|p| p.p_flags & PF_X != 0) {
                stats.add(&patch::rewrite_region(bias + p.p_vaddr, p.p_filesz));
            }
            stats
        }
    };
    let mut pg = bias + h.lo;
    while pg < bias + h.hi {
        let prot = page_prot.get(&pg).copied().unwrap_or(libc::PROT_NONE);
        // SAFETY: applying final protections inside our mapping.
        if unsafe { libc::mprotect(pg as *mut _, PAGE as usize, prot) } != 0 {
            return Err(format!(
                "{name}: mprotect {pg:#x} failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        pg += PAGE;
    }
    Ok(stats)
}

fn finish(
    file: &std::fs::File,
    h: Headers,
    bias: u64,
    name: &str,
    source: String,
    stats: patch::PatchStats,
) -> Result<Image, String> {
    crate::diag::register_module(bias + h.lo, bias + h.hi, name.to_string());
    let phdr = match h.phdrs.iter().find(|p| p.p_type == PT_PHDR) {
        Some(p) => bias + p.p_vaddr,
        None => {
            // The phdrs must be inside the first PT_LOAD's file image.
            let first = h.loads[0];
            if h.hdr.e_phoff < first.p_offset || h.hdr.e_phoff >= first.p_offset + first.p_filesz {
                return Err(format!("{name}: program headers are not loaded"));
            }
            bias + first.p_vaddr + (h.hdr.e_phoff - first.p_offset)
        }
    };
    let interp = match h.phdrs.iter().find(|p| p.p_type == PT_INTERP) {
        Some(p) => {
            let mut s = vec![0u8; p.p_filesz as usize];
            file.read_exact_at(&mut s, p.p_offset)
                .map_err(|e| format!("{name}: {e}"))?;
            let end = s.iter().position(|&c| c == 0).unwrap_or(s.len());
            Some(String::from_utf8_lossy(&s[..end]).into_owned())
        }
        None => None,
    };
    Ok(Image {
        bias,
        entry: bias + h.hdr.e_entry,
        phdr,
        phnum: h.hdr.e_phnum as u64,
        end: bias + h.hi,
        interp,
        source,
        stats,
    })
}

// auxv tags
const AT_NULL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_PAGESZ: u64 = 6;
const AT_BASE: u64 = 7;
const AT_FLAGS: u64 = 8;
const AT_ENTRY: u64 = 9;
const AT_UID: u64 = 11;
const AT_EUID: u64 = 12;
const AT_GID: u64 = 13;
const AT_EGID: u64 = 14;
const AT_PLATFORM: u64 = 15;
const AT_HWCAP: u64 = 16;
const AT_CLKTCK: u64 = 17;
const AT_SECURE: u64 = 23;
const AT_RANDOM: u64 = 25;
const AT_HWCAP2: u64 = 26;
const AT_EXECFN: u64 = 31;

const STACK_SIZE: u64 = 8 << 20;

pub struct StackInputs<'a> {
    pub argv: &'a [Vec<u8>],
    pub envp: &'a [Vec<u8>],
    pub execfn: &'a [u8],
    pub program: &'a Image,
    pub interp_base: u64,
}

/// Build the Linux initial process stack and return the initial sp.
pub fn build_stack(inp: &StackInputs) -> Result<u64, String> {
    // SAFETY: fresh anonymous stack mapping.
    let base = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            STACK_SIZE as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        )
    };
    if base == libc::MAP_FAILED {
        return Err("cannot allocate the guest stack".into());
    }
    let base = base as u64;
    // Guard page at the bottom.
    // SAFETY: our own mapping.
    unsafe { libc::mprotect(base as *mut _, PAGE as usize, libc::PROT_NONE) };
    let mut top = base + STACK_SIZE;

    let mut push_bytes = |bytes: &[u8]| -> u64 {
        top -= bytes.len() as u64;
        // SAFETY: inside the stack mapping.
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), top as *mut u8, bytes.len()) };
        top
    };
    let cstr = |s: &[u8]| {
        let mut v = s.to_vec();
        v.push(0);
        v
    };
    let execfn = push_bytes(&cstr(inp.execfn));
    let envs: Vec<u64> = inp
        .envp
        .iter()
        .rev()
        .map(|s| push_bytes(&cstr(s)))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let args: Vec<u64> = inp
        .argv
        .iter()
        .rev()
        .map(|s| push_bytes(&cstr(s)))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let platform = push_bytes(b"aarch64\0");
    let mut rnd = [0u8; 16];
    // SAFETY: local buffer.
    unsafe { libc::getentropy(rnd.as_mut_ptr().cast(), rnd.len()) };
    let random = push_bytes(&rnd);

    let (hwcap, hwcap2) = crate::hwcap::host_hwcaps();
    let id = crate::sys::cred::current();
    let (uid, euid, gid, egid) = (
        id.uid[0] as u64,
        id.uid[1] as u64,
        id.gid[0] as u64,
        id.gid[1] as u64,
    );
    let p = inp.program;
    let auxv: Vec<(u64, u64)> = vec![
        (AT_HWCAP, hwcap),
        (AT_PAGESZ, PAGE),
        (AT_CLKTCK, 100),
        (AT_PHDR, p.phdr),
        (AT_PHENT, 56),
        (AT_PHNUM, p.phnum),
        (AT_BASE, inp.interp_base),
        (AT_FLAGS, 0),
        (AT_ENTRY, p.entry),
        (AT_UID, uid),
        (AT_EUID, euid),
        (AT_GID, gid),
        (AT_EGID, egid),
        (AT_SECURE, 0),
        (AT_RANDOM, random),
        (AT_HWCAP2, hwcap2),
        (AT_EXECFN, execfn),
        (AT_PLATFORM, platform),
        (AT_NULL, 0),
    ];

    let words = 1 + args.len() + 1 + envs.len() + 1 + auxv.len() * 2;
    let mut sp = (top - words as u64 * 8) & !15;
    let start = sp;
    let mut put = |v: u64| {
        // SAFETY: inside the stack mapping.
        unsafe { (sp as *mut u64).write(v) };
        sp += 8;
    };
    put(args.len() as u64);
    args.iter().for_each(|&a| put(a));
    put(0);
    envs.iter().for_each(|&e| put(e));
    put(0);
    for (k, v) in auxv {
        put(k);
        put(v);
    }
    Ok(start)
}
