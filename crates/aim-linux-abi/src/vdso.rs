//! The vDSO, as Linux maps it into every arm64 process: an ELF shared
//! object at `AT_SYSINFO_EHDR` exporting `__kernel_clock_gettime`,
//! `__kernel_gettimeofday`, `__kernel_clock_getres` and
//! `__kernel_rt_sigreturn`, with a read-only data page (`[vvar]`) before it.
//! bionic's libc and linker find it through the auxv.
//!
//! The clock functions read what the syscalls read (`sys/clock.rs`), with
//! the same arithmetic, without leaving guest code: MONOTONIC is the
//! counter plus Darwin's `mach_absolute_time` offset, BOOTTIME the counter
//! plus the continuous-time offset, REALTIME the kernel's wall-clock
//! timestamp advanced by the ticks since it, all from Darwin's commpage;
//! ticks become nanoseconds with the timebase in `[vvar]`. What they cannot
//! serve (CPU and alarm clocks, bad ids, a host without a user-readable
//! counter, a stale wall-clock timestamp) is the syscall, as on Linux.
//!
//! The code is assembled into this binary and copied into each process's
//! image already translated: its `svc` sites branch to syscall stubs
//! placed after it (`a64::stub_for`), as a translated file's do.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

use crate::a64;
use crate::sys::clock::commpage;

const PAGE: u64 = crate::loader::PAGE;
/// Where the code starts in the image; the metadata comes before it.
const TEXT_OFF: usize = 0x1000;

std::arch::global_asm!(
    // x9 = the commpage.
    ".macro COMMPAGE",
    "movz x9, #{cp0}",
    "movk x9, #{cp1}, lsl #16",
    "movk x9, #{cp2}, lsl #32",
    ".endm",
    // \dst = the counter, read as libsystem does for mode w10.
    ".macro COUNTER dst",
    "cmp w10, #2",
    "b.eq 7f",
    "cmp w10, #3",
    "b.eq 8f",
    "isb",
    "mrs \\dst, cntvct_el0",
    "b 9f",
    "7: mrs \\dst, S3_3_C14_C0_6", // CNTVCTSS_EL0
    "b 9f",
    "8: mrs \\dst, S3_4_C15_C10_6", // Apple's non-speculative counter
    "9:",
    ".endm",
    // \dst = mach_absolute_time.
    ".macro ABSTIME dst, t1, t2",
    "6: ldr \\t1, [x9, #{tb_off}]",
    "COUNTER \\dst",
    "ldr \\t2, [x9, #{tb_off}]",
    "cmp \\t1, \\t2",
    "b.ne 6b",
    "add \\dst, \\dst, \\t1",
    ".endm",
    ".macro NSEC_PER_SEC reg",
    "movz \\reg, #0xca00",
    "movk \\reg, #0x3b9a, lsl #16",
    ".endm",
    // x11 = seconds, x12 = nanoseconds of the wall clock
    // (`clock::commpage_realtime`); \fail when the commpage cannot serve it.
    ".macro REALTIME fail",
    "add x15, x9, #{tod}",
    "5: ldr x2, [x15]",
    "ABSTIME x3, x4, x5",
    "ldp x11, x12, [x15, #8]",
    "ldp x4, x5, [x15, #24]",
    "ldr x6, [x15]",
    "cmp x2, x6",
    "b.ne 5b",
    "cbz x2, \\fail",
    "sub x3, x3, x2",
    "cmp x3, x5",
    "b.hs \\fail",
    "tbnz x11, #63, \\fail",
    "umulh x6, x4, x3",
    "mul x4, x4, x3",
    "adds x12, x12, x4",
    "adc x11, x11, x6",
    "NSEC_PER_SEC x4",
    "umulh x12, x12, x4",
    ".endm",
    //
    ".text",
    ".p2align 2",
    ".globl _aim_vdso_start",
    "_aim_vdso_start:",
    "Lvdso_start:",
    // int __kernel_clock_gettime(clockid_t, struct timespec *)
    ".globl _aim_vdso_clock_gettime",
    ".alt_entry _aim_vdso_clock_gettime",
    "_aim_vdso_clock_gettime:",
    "COMMPAGE",
    "ldrb w10, [x9, #{user_tb}]",
    "cbz w10, Lvdso_gettime_sys",
    "cmp w0, #1", // MONOTONIC
    "b.eq Lvdso_mono",
    "cmp w0, #4", // MONOTONIC_RAW
    "b.eq Lvdso_mono",
    "cmp w0, #6", // MONOTONIC_COARSE
    "b.eq Lvdso_mono",
    "cmp w0, #7", // BOOTTIME
    "b.eq Lvdso_boot",
    "cbz w0, Lvdso_real", // REALTIME
    "cmp w0, #5", // REALTIME_COARSE
    "b.eq Lvdso_real",
    "cmp w0, #11", // TAI
    "b.eq Lvdso_real",
    "Lvdso_gettime_sys:",
    "mov x8, #113",
    "svc #0",
    "ret",
    "Lvdso_boot:",
    "ldrb w11, [x9, #{cont_hw}]",
    "cbz w11, Lvdso_gettime_sys",
    "COUNTER x12",
    "ldr x13, [x9, #{cont_tb}]",
    "add x12, x12, x13",
    "b Lvdso_ticks",
    "Lvdso_mono:",
    "ABSTIME x12, x13, x14",
    // x12 ticks to nanoseconds as `aim_hostcall::clock::ticks_to_ns`.
    "Lvdso_ticks:",
    "adr x13, Lvdso_start",
    "sub x13, x13, #{vvar_back}",
    "ldp w14, w15, [x13]",
    "udiv x2, x12, x15",
    "msub x3, x2, x15, x12",
    "mul x2, x2, x14",
    "mul x3, x3, x14",
    "udiv x3, x3, x15",
    "add x12, x2, x3",
    "NSEC_PER_SEC x14",
    "udiv x11, x12, x14",
    "msub x12, x11, x14, x12",
    "Lvdso_store:",
    "stp x11, x12, [x1]",
    "mov w0, #0",
    "ret",
    "Lvdso_real:",
    "REALTIME Lvdso_gettime_sys",
    "b Lvdso_store",
    // int __kernel_gettimeofday(struct timeval *, struct timezone *)
    ".globl _aim_vdso_gettimeofday",
    ".alt_entry _aim_vdso_gettimeofday",
    "_aim_vdso_gettimeofday:",
    "cbz x0, 1f",
    "COMMPAGE",
    "ldrb w10, [x9, #{user_tb}]",
    "cbz w10, Lvdso_gettimeofday_sys",
    "REALTIME Lvdso_gettimeofday_sys",
    "mov x13, #1000",
    "udiv x12, x12, x13",
    "stp x11, x12, [x0]",
    "1: cbz x1, 2f",
    "str xzr, [x1]", // UTC, no DST, as the syscall reports
    "2: mov w0, #0",
    "ret",
    "Lvdso_gettimeofday_sys:",
    "mov x8, #169",
    "svc #0",
    "ret",
    // int __kernel_clock_getres(clockid_t, struct timespec *): Linux's
    // high-resolution and coarse clocks.
    ".globl _aim_vdso_clock_getres",
    ".alt_entry _aim_vdso_clock_getres",
    "_aim_vdso_clock_getres:",
    "cmp w0, #11",
    "b.hi Lvdso_getres_sys",
    "mov w9, #0x893", // REALTIME, MONOTONIC, MONOTONIC_RAW, BOOTTIME, TAI
    "lsr w9, w9, w0",
    "tbnz w9, #0, 1f",
    "mov w9, #0x60", // REALTIME_COARSE, MONOTONIC_COARSE
    "lsr w9, w9, w0",
    "tbz w9, #0, Lvdso_getres_sys",
    "movz x9, #{tick_lo}",
    "movk x9, #{tick_hi}, lsl #16",
    "b 2f",
    "1: mov x9, #1",
    "2: cbz x1, 3f",
    "stp xzr, x9, [x1]",
    "3: mov w0, #0",
    "ret",
    "Lvdso_getres_sys:",
    "mov x8, #114",
    "svc #0",
    "ret",
    ".globl _aim_vdso_rt_sigreturn",
    ".alt_entry _aim_vdso_rt_sigreturn",
    "_aim_vdso_rt_sigreturn:",
    "mov x8, #139",
    "svc #0",
    ".globl _aim_vdso_end",
    ".alt_entry _aim_vdso_end",
    "_aim_vdso_end:",
    // Not copied: keeps the end label inside the block, where the linker
    // leaves it (a label at the end of an atom may be placed elsewhere).
    "udf #0",
    ".purgem COMMPAGE",
    ".purgem COUNTER",
    ".purgem ABSTIME",
    ".purgem NSEC_PER_SEC",
    ".purgem REALTIME",
    cp0 = const commpage::BASE & 0xffff,
    cp1 = const (commpage::BASE >> 16) & 0xffff,
    cp2 = const commpage::BASE >> 32,
    tb_off = const commpage::TIMEBASE_OFFSET,
    user_tb = const commpage::USER_TIMEBASE,
    cont_hw = const commpage::CONT_HWCLOCK,
    cont_tb = const commpage::CONT_HW_TIMEBASE,
    tod = const commpage::TIMEOFDAY,
    vvar_back = const TEXT_OFF as u64 + PAGE,
    tick_lo = const crate::sys::clock::TICK_NS & 0xffff,
    tick_hi = const crate::sys::clock::TICK_NS >> 16,
);

unsafe extern "C" {
    fn aim_vdso_start();
    fn aim_vdso_clock_gettime();
    fn aim_vdso_gettimeofday();
    fn aim_vdso_clock_getres();
    fn aim_vdso_rt_sigreturn();
    fn aim_vdso_end();
}

/// `[vvar]`: the host tick's length (`aim_hostcall::clock::timebase`).
#[repr(C)]
struct Vvar {
    numer: u32,
    denom: u32,
}

/// The vDSO's address in this process; 0 before it is mapped.
static BASE: AtomicU64 = AtomicU64::new(0);

const SONAME: &str = "linux-vdso.so.1";
const STT_FUNC_GLOBAL: u8 = 0x12;
const SHT_PROGBITS: u32 = 1;
const SHT_STRTAB: u32 = 3;
const SHT_HASH: u32 = 5;
const SHT_DYNAMIC: u32 = 6;
const SHT_DYNSYM: u32 = 11;
const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;
const DT_HASH: u64 = 4;
const DT_STRTAB: u64 = 5;
const DT_SYMTAB: u64 = 6;
const DT_STRSZ: u64 = 10;
const DT_SYMENT: u64 = 11;
const DT_SONAME: u64 = 14;

fn put(b: &mut Vec<u8>, bytes: &[u8]) -> usize {
    let at = b.len();
    b.extend_from_slice(bytes);
    at
}

fn words(v: &[u64]) -> Vec<u8> {
    v.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// The image: ELF header, program headers, `.hash`, `.dynsym`, `.dynstr`,
/// `.dynamic`, then the code at `TEXT_OFF` with its syscall stubs, then
/// `.shstrtab` and the section headers. One PT_LOAD maps it all at vaddr 0.
fn build() -> Vec<u8> {
    let addr = |f: unsafe extern "C" fn()| f as usize;
    let start = addr(aim_vdso_start);
    let len = addr(aim_vdso_end) - start;
    // SAFETY: the assembled block above, in this binary's text.
    let code = unsafe { std::slice::from_raw_parts(start as *const u8, len) };
    let entries = [
        ("__kernel_clock_gettime", addr(aim_vdso_clock_gettime)),
        ("__kernel_gettimeofday", addr(aim_vdso_gettimeofday)),
        ("__kernel_clock_getres", addr(aim_vdso_clock_getres)),
        ("__kernel_rt_sigreturn", addr(aim_vdso_rt_sigreturn)),
    ];

    let mut b = vec![0u8; 64 + 2 * 56];
    let nsyms = entries.len() + 1;
    // SysV hash with one bucket: every symbol on one chain.
    let mut hash = vec![1u32, nsyms as u32, 1, 0];
    hash.extend((2..nsyms as u32).chain([0]));
    let hash: Vec<u8> = hash.iter().flat_map(|w| w.to_le_bytes()).collect();
    let hash_off = put(&mut b, &hash);

    let mut dynstr = vec![0u8];
    let mut name = |s: &str| {
        let at = dynstr.len();
        dynstr.extend_from_slice(s.as_bytes());
        dynstr.push(0);
        at as u32
    };
    let soname = name(SONAME);
    let mut dynsym = vec![0u8; 24];
    for (i, &(sym, at)) in entries.iter().enumerate() {
        let end = entries.get(i + 1).map_or(start + len, |e| e.1);
        dynsym.extend(name(sym).to_le_bytes());
        dynsym.extend([STT_FUNC_GLOBAL, 0]);
        dynsym.extend(5u16.to_le_bytes()); // .text
        dynsym.extend(((TEXT_OFF + at - start) as u64).to_le_bytes());
        dynsym.extend(((end - at) as u64).to_le_bytes());
    }
    b.resize(b.len().next_multiple_of(8), 0);
    let dynsym_off = put(&mut b, &dynsym);
    let dynstr_off = put(&mut b, &dynstr);
    b.resize(b.len().next_multiple_of(8), 0);
    let dynamic = words(&[
        DT_HASH,
        hash_off as u64,
        DT_STRTAB,
        dynstr_off as u64,
        DT_SYMTAB,
        dynsym_off as u64,
        DT_STRSZ,
        dynstr.len() as u64,
        DT_SYMENT,
        24,
        DT_SONAME,
        soname as u64,
        0,
        0,
    ]);
    let dynamic_off = put(&mut b, &dynamic);
    assert!(b.len() <= TEXT_OFF, "vDSO metadata overlaps its code");

    // The code, its svc sites branching to stubs after it.
    b.resize(TEXT_OFF, 0);
    put(&mut b, code);
    let ctr = a64::host_ctr_el0();
    for site in (TEXT_OFF..TEXT_OFF + len).step_by(4) {
        let insn = u32::from_le_bytes(b[site..site + 4].try_into().unwrap());
        let Some((kind, rt)) = a64::classify(insn) else {
            continue;
        };
        let stub = b.len() as u64;
        let body = a64::stub_for(kind, rt, ctr, stub, site as u64).expect("stub in range");
        b.extend(body.iter().flat_map(|w| w.to_le_bytes()));
        let branch = a64::encode_b(site as u64, stub).expect("stub in range");
        b[site..site + 4].copy_from_slice(&branch.to_le_bytes());
    }
    let text_len = b.len() - TEXT_OFF;

    let shstrtab = b"\0.hash\0.dynsym\0.dynstr\0.dynamic\0.text\0.shstrtab\0";
    let shstrtab_off = put(&mut b, shstrtab);
    b.resize(b.len().next_multiple_of(8), 0);
    let shoff = b.len();
    let sections: [(u32, u32, usize, usize, u32, u32, u64); 7] = [
        // name, type, offset, size, link, flags (SHF_ALLOC 2, SHF_EXECINSTR 4), entsize
        (0, 0, 0, 0, 0, 0, 0),
        (1, SHT_HASH, hash_off, hash.len(), 2, 2, 4),
        (7, SHT_DYNSYM, dynsym_off, dynsym.len(), 3, 2, 24),
        (15, SHT_STRTAB, dynstr_off, dynstr.len(), 0, 2, 0),
        (23, SHT_DYNAMIC, dynamic_off, dynamic.len(), 3, 2, 16),
        (32, SHT_PROGBITS, TEXT_OFF, text_len, 0, 6, 0),
        (38, SHT_STRTAB, shstrtab_off, shstrtab.len(), 0, 0, 0),
    ];
    for (name, ty, off, size, link, flags, entsize) in sections {
        let addr = if flags != 0 { off as u64 } else { 0 };
        b.extend(name.to_le_bytes());
        b.extend(ty.to_le_bytes());
        b.extend(words(&[flags as u64, addr, off as u64, size as u64]));
        b.extend(link.to_le_bytes());
        // sh_info: .dynsym's first global symbol.
        b.extend(if ty == SHT_DYNSYM { 1u32 } else { 0 }.to_le_bytes());
        b.extend(words(&[if flags != 0 { 8 } else { 1 }, entsize]));
    }
    let size = b.len() as u64;
    assert!(size <= PAGE, "vDSO larger than a page");

    let mut h = Vec::with_capacity(64 + 2 * 56);
    h.extend(b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0");
    h.extend(3u16.to_le_bytes()); // ET_DYN
    h.extend(crate::elf::EM_AARCH64.to_le_bytes());
    h.extend(1u32.to_le_bytes());
    h.extend(words(&[0, 64, shoff as u64])); // entry, phoff, shoff
    h.extend(0u32.to_le_bytes());
    for half in [64u16, 56, 2, 64, sections.len() as u16, 6] {
        h.extend(half.to_le_bytes());
    }
    let r_x = crate::elf::PF_R | crate::elf::PF_X;
    for (ty, flags, off, len, align) in [
        (PT_LOAD, r_x, 0, size, PAGE),
        (
            PT_DYNAMIC,
            crate::elf::PF_R,
            dynamic_off as u64,
            dynamic.len() as u64,
            8,
        ),
    ] {
        h.extend(ty.to_le_bytes());
        h.extend(flags.to_le_bytes());
        h.extend(words(&[off, off, off, len, len, align]));
    }
    b[..h.len()].copy_from_slice(&h);
    b
}

unsafe extern "C" {
    fn sys_icache_invalidate(start: *mut libc::c_void, len: usize);
}

/// Map `[vvar]` and the vDSO for a new program; returns the vDSO's
/// address (`AT_SYSINFO_EHDR`).
pub fn map() -> Result<u64, String> {
    static IMAGE: OnceLock<Vec<u8>> = OnceLock::new();
    let image = IMAGE.get_or_init(build);
    let vvar = crate::sys::map_guest_anon(2 * PAGE, libc::PROT_READ | libc::PROT_WRITE)
        .map_err(|_| "cannot map the vDSO".to_string())?;
    let base = vvar + PAGE;
    let (numer, denom) = aim_hostcall::clock::timebase();
    // SAFETY: our new read-write mapping.
    unsafe {
        (vvar as *mut Vvar).write(Vvar { numer, denom });
        std::ptr::copy_nonoverlapping(image.as_ptr(), base as *mut u8, image.len());
        sys_icache_invalidate(base as *mut _, image.len());
        if libc::mprotect(vvar as *mut _, PAGE as usize, libc::PROT_READ) != 0
            || libc::mprotect(
                base as *mut _,
                PAGE as usize,
                libc::PROT_READ | libc::PROT_EXEC,
            ) != 0
        {
            return Err(format!(
                "cannot protect the vDSO: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    BASE.store(base, Relaxed);
    crate::diag::register_module(base, base + PAGE, "[vdso]".into());
    Ok(base)
}

/// What `/proc/<pid>/maps` names the mapping at `addr`: `[vvar]`, `[vdso]`.
pub fn name_of(addr: u64) -> Option<&'static str> {
    let base = BASE.load(Relaxed);
    match addr.checked_sub(base.checked_sub(PAGE)?)? {
        o if o < PAGE => Some("[vvar]"),
        o if o < 2 * PAGE => Some("[vdso]"),
        _ => None,
    }
}

/// Fork: the child's memory has the vDSO at the same address.
pub(crate) fn fork_save(w: &mut crate::sys::fork_state::Writer) {
    w.u64(BASE.load(Relaxed));
}

pub(crate) fn fork_restore(r: &mut crate::sys::fork_state::Reader) {
    BASE.store(r.u64(), Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    type GetTime = unsafe extern "C" fn(i32, *mut [i64; 2]) -> i32;

    fn sym(base: u64, image: &[u8], want: &str) -> u64 {
        // bionic's lookup (`__libc_init_vdso`): the SHT_DYNSYM section's
        // size, PT_DYNAMIC's DT_STRTAB and DT_SYMTAB.
        let u16_at = |o: usize| u16::from_le_bytes(image[o..o + 2].try_into().unwrap()) as usize;
        let u32_at = |o: usize| u32::from_le_bytes(image[o..o + 4].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(image[o..o + 8].try_into().unwrap());
        let (shoff, shnum) = (u64_at(40) as usize, u16_at(60));
        let count = (0..shnum)
            .map(|i| shoff + i * 64)
            .find(|&s| u32_at(s + 4) == SHT_DYNSYM)
            .map(|s| u64_at(s + 32) / 24)
            .unwrap();
        let phdr = (0..u16_at(56))
            .map(|i| 64 + i * 56)
            .find(|&p| u32_at(p) == PT_DYNAMIC)
            .unwrap();
        let (mut strtab, mut symtab) = (0, 0);
        let mut d = u64_at(phdr + 8) as usize;
        while u64_at(d) != 0 {
            match u64_at(d) {
                DT_STRTAB => strtab = u64_at(d + 8) as usize,
                DT_SYMTAB => symtab = u64_at(d + 8) as usize,
                _ => {}
            }
            d += 16;
        }
        (0..count as usize)
            .map(|i| symtab + i * 24)
            .find(|&s| {
                let n = strtab + u32_at(s) as usize;
                image[n..].split(|&c| c == 0).next() == Some(want.as_bytes())
            })
            .map(|s| base + u64_at(s + 8))
            .unwrap_or_else(|| panic!("{want} not exported"))
    }

    #[test]
    fn clocks_match_the_syscalls() {
        let base = map().unwrap();
        // SAFETY: the mapped image.
        let image = unsafe { std::slice::from_raw_parts(base as *const u8, PAGE as usize) };
        assert_eq!(name_of(base), Some("[vdso]"));
        assert_eq!(name_of(base - 1), Some("[vvar]"));
        assert_eq!(name_of(base + PAGE), None);
        // SAFETY: the vDSO's entry points, C ABI.
        let gettime: GetTime =
            unsafe { std::mem::transmute(sym(base, image, "__kernel_clock_gettime")) };
        let getres: GetTime =
            unsafe { std::mem::transmute(sym(base, image, "__kernel_clock_getres")) };
        let tod: unsafe extern "C" fn(*mut [i64; 2], *mut [i32; 2]) -> i32 =
            unsafe { std::mem::transmute(sym(base, image, "__kernel_gettimeofday")) };
        sym(base, image, "__kernel_rt_sigreturn");

        let ns = |t: [i64; 2]| t[0] * 1_000_000_000 + t[1];
        let clocks: [(i32, fn() -> i64); 6] = [
            (0, crate::sys::clock::realtime_ns),
            (1, aim_hostcall::clock::monotonic_ns),
            (4, aim_hostcall::clock::monotonic_ns),
            (6, aim_hostcall::clock::monotonic_ns),
            (7, aim_hostcall::clock::boottime_ns),
            (11, crate::sys::clock::realtime_ns),
        ];
        for (id, host) in clocks {
            let mut t = [0i64; 2];
            let before = host();
            // SAFETY: a served clock: no syscall.
            assert_eq!(unsafe { gettime(id, &mut t) }, 0);
            let after = host();
            assert!(t[1] < 1_000_000_000);
            assert!(
                before <= ns(t) && ns(t) <= after,
                "clock {id}: {before} {t:?} {after}"
            );
        }
        let (mut tv, mut tz) = ([0i64; 2], [7i32; 2]);
        let before = crate::sys::clock::realtime_ns() / 1000;
        // SAFETY: served without a syscall.
        assert_eq!(unsafe { tod(&mut tv, &mut tz) }, 0);
        let after = crate::sys::clock::realtime_ns() / 1000;
        let us = tv[0] * 1_000_000 + tv[1];
        assert!(before <= us && us <= after && tv[1] < 1_000_000);
        assert_eq!(tz, [0, 0]);
        for (id, res) in [
            (0, 1),
            (1, 1),
            (4, 1),
            (5, 4_000_000),
            (6, 4_000_000),
            (7, 1),
            (11, 1),
        ] {
            let mut t = [9i64; 2];
            // SAFETY: as above.
            assert_eq!(unsafe { getres(id, &mut t) }, 0);
            assert_eq!(t, [0, res], "clock {id}");
        }
    }

    #[test]
    fn syscalls_are_translated() {
        let image = build();
        let len = aim_vdso_end as usize - aim_vdso_start as usize;
        let text = &image[TEXT_OFF..TEXT_OFF + len];
        assert!(
            text.chunks(4)
                .all(|w| a64::classify(u32::from_le_bytes(w.try_into().unwrap())).is_none()),
            "an svc left in the vDSO"
        );
    }
}
