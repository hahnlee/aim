//! Program loading as Linux's `binfmt_elf` does it: reserve the image span,
//! place PT_LOAD segments at a load bias honouring their alignment, load the
//! PT_INTERP interpreter the same way, and build the initial stack
//! (argc, argv, envp, auxv).

use std::collections::BTreeMap;
use std::ffi::CStr;
use std::os::unix::fs::FileExt;

use crate::elf::{self, PF_R, PF_W, PF_X, PT_INTERP, PT_LOAD, PT_PHDR};
use crate::patch;

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

/// Map an ELF file from the host path `host`; `name` labels it in diagnostics.
pub fn load_elf(host: &CStr, name: &str) -> Result<Image, String> {
    let path = host.to_str().map_err(|e| e.to_string())?;
    let file = std::fs::File::open(path).map_err(|e| format!("{name}: {e}"))?;
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
    let span = hi - lo;

    // Reserve the whole span (plus alignment slack for ET_DYN), like the
    // kernel's total_mapping_size reservation.
    let bias = if hdr.e_type == elf::ET_DYN {
        // SAFETY: fresh PROT_NONE reservation.
        let r = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                (span + align) as usize,
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
        let start = (r + align - 1) & !(align - 1);
        // SAFETY: trimming our own reservation.
        unsafe {
            if start > r {
                libc::munmap(r as *mut _, (start - r) as usize);
            }
            let tail = r + span + align - (start + span);
            if tail > 0 {
                libc::munmap((start + span) as *mut _, tail as usize);
            }
        }
        start - lo
    } else {
        // SAFETY: ET_EXEC must live at its link address.
        let r = unsafe {
            libc::mmap(
                lo as *mut _,
                span as usize,
                libc::PROT_NONE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if r as u64 != lo {
            return Err(format!("{name}: ET_EXEC range {lo:#x} is not available"));
        }
        0
    };

    // Populate: every page writable while file contents are copied in.
    // SAFETY: remapping inside our reservation.
    let r = unsafe {
        libc::mmap(
            (bias + lo) as *mut _,
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
    for p in &loads {
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
    let mut stats = patch::PatchStats::default();
    for p in loads.iter().filter(|p| p.p_flags & PF_X != 0) {
        let s = patch::rewrite_region(bias + p.p_vaddr, p.p_filesz);
        stats.svc += s.svc;
        stats.mrs_tp += s.mrs_tp;
        stats.msr_tp += s.msr_tp;
        stats.brk_fallback += s.brk_fallback;
    }
    let mut pg = bias + lo;
    while pg < bias + hi {
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
    crate::diag::register_module(bias + lo, bias + hi, name.to_string());

    let phdr = match phdrs.iter().find(|p| p.p_type == PT_PHDR) {
        Some(p) => bias + p.p_vaddr,
        None => {
            // The phdrs must be inside the first PT_LOAD's file image.
            let first = loads[0];
            if hdr.e_phoff < first.p_offset || hdr.e_phoff >= first.p_offset + first.p_filesz {
                return Err(format!("{name}: program headers are not loaded"));
            }
            bias + first.p_vaddr + (hdr.e_phoff - first.p_offset)
        }
    };
    let interp = match phdrs.iter().find(|p| p.p_type == PT_INTERP) {
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
        entry: bias + hdr.e_entry,
        phdr,
        phnum: hdr.e_phnum as u64,
        end: bias + hi,
        interp,
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
    pub argv: &'a [String],
    pub envp: &'a [String],
    pub execfn: &'a str,
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
    let cstr = |s: &str| {
        let mut v = s.as_bytes().to_vec();
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
    // SAFETY: trivial identity queries.
    let (uid, euid, gid, egid) = unsafe {
        (
            libc::getuid() as u64,
            libc::geteuid() as u64,
            libc::getgid() as u64,
            libc::getegid() as u64,
        )
    };
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
