//! Load-time rewriting, for code that has no translation-cache entry:
//! anonymous executable memory created at run time (a JIT, or an app's own
//! mappings) and original files that were never translated.
//!
//! Sites are rewritten with the same stubs as the ahead-of-time translator
//! (`a64`), placed in trampoline islands within ±128 MiB of each site. A site
//! with no island in range becomes `brk #(0xA000 | kind << 5 | rt)`, which
//! the SIGTRAP handler emulates.
//!
//! Other threads may be running while this happens, so live code never
//! loses execute permission:
//! - islands are dual-mapped: stubs are written through a private RW view
//!   while the RX view stays executable;
//! - a site word on a page that is currently executable is written through a
//!   temporary RW alias of that page (`mach_vm_remap`), as one aligned 32-bit
//!   store, after its stub is complete;
//! - a page that is not executable yet is written in place.

use std::sync::Mutex;

use crate::a64::{self, Kind};
use crate::context::GuestContext;

const ISLAND_SIZE: u64 = 256 << 10;
const PAGE: u64 = 16384;

unsafe extern "C" {
    fn sys_icache_invalidate(start: *mut libc::c_void, len: usize);
}

pub mod vm {
    //! The few Mach VM calls the layer needs.
    unsafe extern "C" {
        pub static mach_task_self_: libc::mach_port_t;
        pub fn mach_vm_remap(
            target: libc::mach_port_t,
            address: *mut u64,
            size: u64,
            mask: u64,
            flags: i32,
            src_task: libc::mach_port_t,
            src_address: u64,
            copy: i32,
            cur: *mut i32,
            max: *mut i32,
            inheritance: u32,
        ) -> i32;
        pub fn mach_vm_protect(
            task: libc::mach_port_t,
            address: u64,
            size: u64,
            set_maximum: i32,
            prot: i32,
        ) -> i32;
        pub fn mach_vm_deallocate(task: libc::mach_port_t, address: u64, size: u64) -> i32;
        pub fn mach_vm_map(
            task: libc::mach_port_t,
            address: *mut u64,
            size: u64,
            mask: u64,
            flags: i32,
            object: libc::mach_port_t,
            offset: u64,
            copy: i32,
            cur: i32,
            max: i32,
            inheritance: u32,
        ) -> i32;
        pub fn mach_vm_region(
            task: libc::mach_port_t,
            address: *mut u64,
            size: *mut u64,
            flavor: i32,
            info: *mut i32,
            count: *mut u32,
            object_name: *mut libc::mach_port_t,
        ) -> i32;
    }
    pub const VM_FLAGS_FIXED: i32 = 0;
    pub const VM_FLAGS_ANYWHERE: i32 = 1;
    pub const VM_FLAGS_OVERWRITE: i32 = 0x4000;
    pub const VM_INHERIT_COPY: u32 = 1;
    pub const VM_REGION_BASIC_INFO_64: i32 = 9;

    pub fn task() -> libc::mach_port_t {
        // SAFETY: reading the task port global.
        unsafe { mach_task_self_ }
    }

    /// Protection, max protection and "shared" of the region containing (or
    /// following) `addr`, with its bounds.
    pub fn region(addr: u64) -> Option<(u64, u64, i32, bool)> {
        let mut a = addr;
        let mut size = 0u64;
        let mut info = [0i32; 9];
        let mut count = 9u32;
        let mut obj = 0;
        // SAFETY: VM_REGION_BASIC_INFO_64 into a 9-int buffer.
        let kr = unsafe {
            mach_vm_region(
                task(),
                &mut a,
                &mut size,
                VM_REGION_BASIC_INFO_64,
                info.as_mut_ptr(),
                &mut count,
                &mut obj,
            )
        };
        (kr == 0).then_some((a, a + size, info[0], info[3] != 0))
    }

    /// Map a second, shared view of `[src, src+len)` and give it `prot`.
    /// With `at`, the view replaces whatever is mapped there.
    pub fn alias(src: u64, len: u64, at: Option<u64>, prot: i32) -> Option<u64> {
        let mut addr = at.unwrap_or(0);
        let flags = match at {
            Some(_) => VM_FLAGS_FIXED | VM_FLAGS_OVERWRITE,
            None => VM_FLAGS_ANYWHERE,
        };
        let (mut cur, mut max) = (0, 0);
        // SAFETY: remapping our own memory into a fresh or owned range.
        let kr = unsafe {
            mach_vm_remap(
                task(),
                &mut addr,
                len,
                0,
                flags,
                task(),
                src,
                0,
                &mut cur,
                &mut max,
                VM_INHERIT_COPY,
            )
        };
        if kr != 0 {
            return None;
        }
        // SAFETY: protecting the view we just created.
        if unsafe { mach_vm_protect(task(), addr, len, 0, prot) } != 0 {
            // SAFETY: removing the view we just created.
            unsafe { mach_vm_deallocate(task(), addr, len) };
            return None;
        }
        Some(addr)
    }
}

fn page_down(v: u64) -> u64 {
    v & !(PAGE - 1)
}

/// Write aligned 32-bit words into code, never removing execute permission
/// from a page that has it.
fn write_words(words: &[(u64, u32)]) {
    let mut i = 0;
    while i < words.len() {
        let page = page_down(words[i].0);
        let mut j = i;
        while j < words.len() && page_down(words[j].0) == page {
            j += 1;
        }
        let (_, _, prot, _) = vm::region(page).unwrap_or((0, 0, 0, false));
        let store = |base: u64| {
            for &(addr, w) in &words[i..j] {
                // SAFETY: `base` maps `page` writable; one aligned store per word.
                unsafe {
                    ((base + (addr - page)) as *mut u32).write_volatile(w);
                }
            }
        };
        if prot & libc::PROT_WRITE != 0 {
            store(page);
        } else if prot & libc::PROT_EXEC == 0 {
            // Not executable: nobody can be running it.
            // SAFETY: temporarily opening a non-executable page of guest code.
            unsafe {
                libc::mprotect(page as *mut _, PAGE as usize, prot | libc::PROT_WRITE);
                store(page);
                libc::mprotect(page as *mut _, PAGE as usize, prot);
            }
        } else if let Some(rw) = vm::alias(page, PAGE, None, libc::PROT_READ | libc::PROT_WRITE) {
            store(rw);
            // SAFETY: removing the temporary alias.
            unsafe { vm::mach_vm_deallocate(vm::task(), rw, PAGE) };
        } else {
            crate::diag!("[linux-abi] cannot write live code at {page:#x}: no RW alias");
        }
        for &(addr, _) in &words[i..j] {
            // SAFETY: flushing the word we wrote.
            unsafe { sys_icache_invalidate(addr as *mut _, 4) };
        }
        i = j;
    }
}

// ---- islands --------------------------------------------------------------

struct Island {
    /// Executable view: where stubs run and branches point.
    rx: u64,
    /// Writable view of the same pages.
    rw: u64,
    used: u64,
}

static ISLANDS: Mutex<Vec<Island>> = Mutex::new(Vec::new());

fn in_range(a: u64, b: u64) -> bool {
    let d = a.wrapping_sub(b) as i64;
    d.abs() < a64::BRANCH_RANGE - ISLAND_SIZE as i64
}

fn new_island(hint: u64) -> Option<Island> {
    // SAFETY: fresh anonymous mappings; the hint only steers placement.
    unsafe {
        let rw = libc::mmap(
            std::ptr::null_mut(),
            ISLAND_SIZE as usize,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        );
        if rw == libc::MAP_FAILED {
            return None;
        }
        let spot = libc::mmap(
            hint as *mut _,
            ISLAND_SIZE as usize,
            libc::PROT_NONE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        );
        if spot == libc::MAP_FAILED {
            libc::munmap(rw, ISLAND_SIZE as usize);
            return None;
        }
        match vm::alias(
            rw as u64,
            ISLAND_SIZE,
            Some(spot as u64),
            libc::PROT_READ | libc::PROT_EXEC,
        ) {
            Some(rx) => Some(Island {
                rx,
                rw: rw as u64,
                used: 0,
            }),
            None => {
                libc::munmap(spot, ISLAND_SIZE as usize);
                libc::munmap(rw, ISLAND_SIZE as usize);
                None
            }
        }
    }
}

/// execve in place: the islands went with the old image's memory; their
/// writable views are host memory, released here.
pub(crate) fn exec_reset() {
    for isl in ISLANDS.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
        // SAFETY: the island's own mappings, which nothing uses any more.
        unsafe {
            libc::munmap(isl.rw as *mut _, ISLAND_SIZE as usize);
            libc::munmap(isl.rx as *mut _, ISLAND_SIZE as usize);
        }
    }
}

/// Fork: the islands. The child gets a copy of each executable view with
/// the guest's memory; its writable view is host memory, which it makes
/// anew from that copy.
pub(crate) fn fork_save(w: &mut crate::sys::fork_state::Writer) {
    let islands = ISLANDS.lock().unwrap_or_else(|e| e.into_inner());
    w.seq(islands.iter(), |w, isl| {
        w.u64(isl.rx);
        w.u64(isl.used);
    });
}

pub(crate) fn fork_restore(r: &mut crate::sys::fork_state::Reader) {
    let saved = r.seq(|r| (r.u64(), r.u64()));
    let mut islands = ISLANDS.lock().unwrap_or_else(|e| e.into_inner());
    for (rx, used) in saved {
        // SAFETY: a fresh host mapping, filled from the island's copy.
        let rw = unsafe {
            let rw = libc::mmap(
                std::ptr::null_mut(),
                ISLAND_SIZE as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            );
            if rw == libc::MAP_FAILED {
                None
            } else {
                std::ptr::copy_nonoverlapping(rx as *const u8, rw as *mut u8, ISLAND_SIZE as usize);
                Some(rw as u64)
            }
        };
        let aliased = rw.and_then(|rw| {
            vm::alias(rw, ISLAND_SIZE, Some(rx), libc::PROT_READ | libc::PROT_EXEC).map(|_| rw)
        });
        match aliased {
            Some(rw) => islands.push(Island { rx, rw, used }),
            None => crate::diag!("[linux-abi] cannot restore the stub island at {rx:#x}"),
        }
    }
}

/// Reserve `bytes` of stub space reachable from `site`; `[lo, hi)` is the
/// region being patched and guides where a new island goes.
fn alloc_stub(islands: &mut Vec<Island>, site: u64, lo: u64, hi: u64, bytes: u64) -> Option<usize> {
    if let Some(i) = islands
        .iter()
        .position(|isl| isl.used + bytes <= ISLAND_SIZE && in_range(isl.rx + isl.used, site))
    {
        return Some(i);
    }
    let hi_hint = (hi + PAGE - 1) & !(PAGE - 1);
    let lo_hint = page_down(lo).saturating_sub(4 * ISLAND_SIZE);
    for hint in [hi_hint, lo_hint, site] {
        if let Some(isl) = new_island(hint) {
            if in_range(isl.rx, site) {
                islands.push(isl);
                return Some(islands.len() - 1);
            }
            // SAFETY: dropping both views of the island we just created.
            unsafe {
                libc::munmap(isl.rx as *mut _, ISLAND_SIZE as usize);
                libc::munmap(isl.rw as *mut _, ISLAND_SIZE as usize);
            }
        }
    }
    None
}

#[derive(Default, Debug, Clone, Copy)]
pub struct PatchStats {
    pub svc: usize,
    pub mrs_tp: usize,
    pub msr_tp: usize,
    pub scs: usize,
    pub ctr: usize,
    pub brk_fallback: usize,
}

impl PatchStats {
    pub fn add(&mut self, o: &PatchStats) {
        self.svc += o.svc;
        self.mrs_tp += o.mrs_tp;
        self.msr_tp += o.msr_tp;
        self.scs += o.scs;
        self.ctr += o.ctr;
        self.brk_fallback += o.brk_fallback;
    }

    pub fn total(&self) -> usize {
        self.svc + self.mrs_tp + self.msr_tp + self.scs + self.ctr
    }

    fn count(&mut self, kind: Kind) {
        match kind {
            Kind::Svc => self.svc += 1,
            Kind::MrsTp => self.mrs_tp += 1,
            Kind::MsrTp => self.msr_tp += 1,
            Kind::ScsPush | Kind::ScsPop => self.scs += 1,
            Kind::MrsCtr => self.ctr += 1,
        }
    }
}

/// Rewrite the given sites (absolute addresses) of code in `[lo, hi)`.
pub fn rewrite_sites(sites:&[(u64,Kind,u32)],lo:u64,hi:u64,use_islands:bool)->PatchStats {
    let(words,stats)=prepare_rewrite_sites(sites,lo,hi,use_islands);
    write_words(&words);stats
}

/// Complete immutable executable stubs before a demand page publishes its
/// branch words. This does not write into any guest source page.
pub fn prepare_rewrite_sites(
    sites: &[(u64, Kind, u32)],
    lo: u64,
    hi: u64,
    use_islands: bool,
) -> (Vec<(u64,u32)>,PatchStats) {
    let mut stats = PatchStats::default();
    let ctr = crate::a64::host_ctr_el0();
    let mut islands = ISLANDS.lock().unwrap_or_else(|e| e.into_inner());
    let mut site_words = Vec::with_capacity(sites.len());
    for &(site, kind, rt) in sites {
        let bytes = a64::stub_len(kind) as u64 * 4;
        let placed = if use_islands {
            alloc_stub(&mut islands, site, lo, hi, bytes)
        } else {
            None
        };
        let word = placed.and_then(|i| {
            let isl = &mut islands[i];
            let at = isl.rx + isl.used;
            let words = a64::stub_for(kind, rt, ctr, at, site)?;
            for (k, w) in words.iter().enumerate() {
                // SAFETY: unused stub space in the island's RW view.
                unsafe { ((isl.rw + isl.used) as *mut u32).add(k).write(*w) };
            }
            // SAFETY: the RX view aliases what we just wrote.
            unsafe { sys_icache_invalidate(at as *mut _, words.len() * 4) };
            isl.used += bytes;
            a64::encode_b(site, at)
        });
        let word = word.unwrap_or_else(|| {
            stats.brk_fallback += 1;
            a64::brk_fallback(kind, rt)
        });
        stats.count(kind);
        site_words.push((site, word));
    }
    drop(islands);
    site_words.sort_unstable_by_key(|&(a, _)| a);
    (site_words,stats)
}

/// Find and rewrite every candidate site in `[addr, addr+len)` by scanning
/// all words: code with no metadata (run-time generated code).
/// Validate an inherited word against actual immutable island ownership and
/// the authenticated original instruction recipe, never a serialized claim.
pub(crate) fn validate_prepared_word(site:u64,kind:Kind,rt:u32,word:u32)->bool {
    if word==a64::brk_fallback(kind,rt){return true;}
    if word&0xfc00_0000!=0x1400_0000{return false;}
    let displacement=(((word&0x03ff_ffff)as i32)<<6>>4)as i64;
    let target=site.wrapping_add_signed(displacement);
    let islands=ISLANDS.lock().unwrap_or_else(|error|error.into_inner());
    let Some(words)=a64::stub_for(kind,rt,a64::host_ctr_el0(),target,site)else{return false;};
    let bytes=(words.len()*4)as u64;
    if !islands.iter().any(|island|target>=island.rx&&target.checked_add(bytes).is_some_and(|end|end<=island.rx+island.used)){return false;}
    unsafe{std::slice::from_raw_parts(target as*const u32,words.len())==words.as_slice()}
}

pub fn rewrite_region(addr: u64, len: u64) -> PatchStats {
    rewrite_region_with(addr, len, true)
}

/// As [`rewrite_region`], but with `use_islands == false` every site takes the
/// `brk` fallback (used to measure that path).
pub fn rewrite_region_with(addr: u64, len: u64, use_islands: bool) -> PatchStats {
    let start = (addr + 3) & !3;
    let mut sites = Vec::new();
    let mut site = start;
    while site + 4 <= addr + len {
        // SAFETY: the caller guarantees the region is mapped and readable.
        let insn = unsafe { (site as *const u32).read() };
        if let Some((kind, rt)) = a64::classify(insn) {
            sites.push((site, kind, rt));
        }
        site += 4;
    }
    rewrite_sites(&sites, addr, addr + len, use_islands)
}

// ---- brk fallback ---------------------------------------------------------

#[repr(C)]
struct ThreadState64 {
    x: [u64; 29],
    fp: u64,
    lr: u64,
    sp: u64,
    pc: u64,
    cpsr: u32,
    pad: u32,
}

#[repr(C)]
struct NeonState64 {
    v: [u128; 32],
    fpsr: u32,
    fpcr: u32,
}

#[repr(C)]
struct MContext64 {
    es: [u64; 2],
    ss: ThreadState64,
    ns: NeonState64,
}

/// Emulate a fallback `brk` site. Returns false if the brk is not ours.
///
/// # Safety
/// `uc` must be the ucontext passed to a SIGTRAP handler.
pub unsafe fn handle_brk(uc: *mut libc::ucontext_t) -> bool {
    // SAFETY: Darwin arm64 ucontext layout.
    unsafe {
        let mc = (*uc).uc_mcontext as *mut MContext64;
        let ss = &mut (*mc).ss;
        let insn = (ss.pc as *const u32).read();
        if insn & 0xffe0_001f != 0xd420_0000 {
            return false;
        }
        let imm = (insn >> 5) & 0xffff;
        if imm & 0xff00 != a64::BRK_TAG {
            return false;
        }
        let Some(kind) = Kind::from_index((imm >> 5) & 0x7) else {
            return false;
        };
        let rt = (imm & 0x1f) as usize;
        let tsd: u64;
        std::arch::asm!("mrs {}, tpidrro_el0", out(reg) tsd);
        let slot = |off: u32| (tsd + off as u64) as *mut u64;
        let get = |ss: &ThreadState64, r: usize| match r {
            0..=28 => ss.x[r],
            29 => ss.fp,
            30 => ss.lr,
            _ => 0,
        };
        let set = |ss: &mut ThreadState64, r: usize, v: u64| match r {
            0..=28 => ss.x[r] = v,
            29 => ss.fp = v,
            30 => ss.lr = v,
            _ => {}
        };
        match kind {
            Kind::MrsTp => set(ss, rt, slot(a64::slot::TP).read()),
            Kind::MsrTp => slot(a64::slot::TP).write(get(ss, rt)),
            Kind::MrsCtr => set(ss, rt, a64::host_ctr_el0() as u64),
            Kind::ScsPush => {
                let p = slot(a64::slot::SCS).read();
                (p as *mut u64).write(ss.lr);
                slot(a64::slot::SCS).write(p + 8);
            }
            Kind::ScsPop => {
                let p = slot(a64::slot::SCS).read() - 8;
                ss.lr = (p as *const u64).read();
                slot(a64::slot::SCS).write(p);
            }
            Kind::Svc => {
                let mut ctx: GuestContext = std::mem::zeroed();
                ctx.x[..29].copy_from_slice(&ss.x);
                ctx.x[29] = ss.fp;
                ctx.x[30] = ss.lr;
                ctx.sp = ss.sp;
                ctx.pc = ss.pc + 4;
                ctx.nzcv = ss.cpsr as u64;
                ctx.v = (*mc).ns.v;
                crate::sys::dispatch(&mut ctx);
                crate::sys::repoke_self();
                ss.x.copy_from_slice(&ctx.x[..29]);
                ss.fp = ctx.x[29];
                ss.lr = ctx.x[30];
                ss.sp = ctx.sp;
                ss.pc = ctx.pc - 4;
                // rt_sigreturn restores these too.
                ss.cpsr = (ss.cpsr & !0xf000_0000) | (ctx.nzcv as u32 & 0xf000_0000);
                (*mc).ns.v = ctx.v;
            }
        }
        ss.pc += 4;
        true
    }
}
