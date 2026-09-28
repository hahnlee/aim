//! Linux arm64 signal frames, built from and restored into guest register
//! state, and the translation of Darwin fault signals into Linux siginfo.
//!
//! The frame follows `arch/arm64/kernel/signal.c`: a `struct rt_sigframe`
//! (siginfo, then ucontext whose `uc_mcontext.__reserved` holds an
//! `fpsimd_context`, an optional `esr_context` and a terminator), with a
//! frame record `{fp, lr}` above it that x29 points at.

use crate::context::GuestContext;

pub const SIGILL: i32 = 4;
pub const SIGTRAP: i32 = 5;
pub const SIGBUS: i32 = 7;
pub const SIGFPE: i32 = 8;
pub const SIGSEGV: i32 = 11;

pub const SA_SIGINFO: u64 = 0x4;
pub const SA_RESTORER: u64 = 0x0400_0000;
pub const SA_ONSTACK: u64 = 0x0800_0000;

pub const SS_ONSTACK: i32 = 1;
pub const SS_DISABLE: i32 = 2;
pub const SS_AUTODISARM: i32 = i32::MIN;
/// arm64 `MINSIGSTKSZ`.
pub const MINSIGSTKSZ: u64 = 5120;

const FPSIMD_MAGIC: u32 = 0x4650_8001;
const FPSIMD_SIZE: u32 = 528;
const ESR_MAGIC: u32 = 0x4553_5201;
const ESR_SIZE: u32 = 16;

// Offsets inside `struct rt_sigframe`.
const UC: u64 = 128;
const UC_STACK: u64 = UC + 16;
const UC_SIGMASK: u64 = UC + 40;
const MCTX: u64 = UC + 176;
const MC_FAULT: u64 = MCTX;
const MC_REGS: u64 = MCTX + 8;
const MC_SP: u64 = MCTX + 256;
const MC_PC: u64 = MCTX + 264;
const MC_PSTATE: u64 = MCTX + 272;
const MC_RESERVED: u64 = MCTX + 288;
const RESERVED_SIZE: u64 = 4096;
/// `sizeof(struct rt_sigframe)`, 16-byte aligned.
const FRAME_SIZE: u64 = MC_RESERVED + RESERVED_SIZE;
const _: () = assert!(FRAME_SIZE == 4688 && FRAME_SIZE % 16 == 0);

/// Linux `siginfo_t` (128 bytes). The union starts at byte 16.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Siginfo {
    pub signo: i32,
    pub errno: i32,
    pub code: i32,
    _pad: i32,
    pub fields: [u64; 14],
}
const _: () = assert!(std::mem::size_of::<Siginfo>() == 128);

pub const SI_USER: i32 = 0;
pub const SI_TKILL: i32 = -6;

impl Siginfo {
    pub fn new(signo: i32, code: i32) -> Self {
        Siginfo {
            signo,
            errno: 0,
            code,
            _pad: 0,
            fields: [0; 14],
        }
    }

    /// `_kill` / `_rt`: sender pid and uid.
    pub fn from_sender(signo: i32, code: i32, pid: i32, uid: u32) -> Self {
        let mut i = Siginfo::new(signo, code);
        i.fields[0] = pid as u32 as u64 | (uid as u64) << 32;
        i
    }

    /// `_sigfault`: the faulting address.
    pub fn fault(signo: i32, code: i32, addr: u64) -> Self {
        let mut i = Siginfo::new(signo, code);
        i.fields[0] = addr;
        i
    }

    /// Read a guest siginfo.
    ///
    /// # Safety
    /// `p` must point to 128 readable bytes.
    pub unsafe fn read(p: u64) -> Self {
        // SAFETY: caller contract.
        unsafe { (p as *const Siginfo).read_unaligned() }
    }

    /// # Safety
    /// `p` must point to 128 writable bytes.
    pub unsafe fn write(&self, p: u64) {
        // SAFETY: caller contract.
        unsafe { (p as *mut Siginfo).write_unaligned(*self) }
    }
}

/// Linux arm64 kernel `struct sigaction`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct KSigaction {
    pub handler: u64,
    pub flags: u64,
    pub restorer: u64,
    pub mask: u64,
}

pub const SIG_DFL: u64 = 0;
pub const SIG_IGN: u64 = 1;

/// A guest alternate signal stack as `sigaltstack` records it. `flags` is
/// what Linux keeps in `sas_ss_flags` (SS_DISABLE or SS_AUTODISARM).
#[derive(Clone, Copy)]
pub struct AltStack {
    pub sp: u64,
    pub size: u64,
    pub flags: i32,
}

impl AltStack {
    pub const DISABLED: AltStack = AltStack {
        sp: 0,
        size: 0,
        flags: SS_DISABLE,
    };

    pub fn enabled(&self) -> bool {
        self.size != 0 && self.flags & SS_DISABLE == 0
    }

    /// Linux `on_sig_stack`.
    pub fn contains(&self, sp: u64) -> bool {
        self.enabled() && sp > self.sp && sp - self.sp <= self.size
    }
}

/// The Darwin arm64 `__darwin_mcontext64`.
#[repr(C)]
pub struct DarwinMcontext {
    pub far: u64,
    pub esr: u32,
    pub exception: u32,
    pub x: [u64; 29],
    pub fp: u64,
    pub lr: u64,
    pub sp: u64,
    pub pc: u64,
    pub cpsr: u32,
    pub flags: u32,
    pub v: [u128; 32],
    pub fpsr: u32,
    pub fpcr: u32,
}
const _: () = assert!(std::mem::offset_of!(DarwinMcontext, v) == 288);

/// # Safety
/// `uc` must be the ucontext a Darwin signal handler received.
pub unsafe fn mcontext<'a>(uc: *mut libc::c_void) -> &'a mut DarwinMcontext {
    // SAFETY: caller contract; uc_mcontext points at the saved state.
    unsafe { &mut *((*(uc as *mut libc::ucontext_t)).uc_mcontext as *mut DarwinMcontext) }
}

const NZCV: u64 = 0xf000_0000;

/// Complete guest register state at a signal boundary.
#[derive(Clone)]
pub struct Cpu {
    pub x: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub pstate: u64,
    pub v: [u128; 32],
    pub fpsr: u32,
    pub fpcr: u32,
}

impl Cpu {
    /// State of a thread stopped at a syscall boundary: resuming it means
    /// continuing after the `svc`.
    pub fn from_ctx(c: &GuestContext) -> Cpu {
        Cpu {
            x: c.x,
            sp: c.sp,
            pc: c.resume_pc(),
            pstate: c.nzcv & NZCV,
            v: c.v,
            fpsr: c.fpsr as u32,
            fpcr: c.fpcr as u32,
        }
    }

    /// Load into `c` so the thread resumes exactly here (`stub_ret` = 0).
    pub fn to_ctx(&self, c: &mut GuestContext) {
        c.x = self.x;
        c.sp = self.sp;
        c.pc = self.pc;
        c.nzcv = self.pstate & NZCV;
        c.v = self.v;
        c.fpsr = self.fpsr as u64;
        c.fpcr = self.fpcr as u64;
        c.stub_ret = 0;
    }

    pub fn from_mc(m: &DarwinMcontext) -> Cpu {
        let mut x = [0u64; 31];
        x[..29].copy_from_slice(&m.x);
        x[29] = m.fp;
        x[30] = m.lr;
        Cpu {
            x,
            sp: m.sp,
            pc: m.pc,
            pstate: m.cpsr as u64 & NZCV,
            v: m.v,
            fpsr: m.fpsr,
            fpcr: m.fpcr,
        }
    }

    pub fn to_mc(&self, m: &mut DarwinMcontext) {
        m.x.copy_from_slice(&self.x[..29]);
        m.fp = self.x[29];
        m.lr = self.x[30];
        m.sp = self.sp;
        m.pc = self.pc;
        m.cpsr = (m.cpsr & !(NZCV as u32)) | (self.pstate & NZCV) as u32;
        m.v = self.v;
        m.fpsr = self.fpsr;
        m.fpcr = self.fpcr;
    }
}

/// Registers a guest handler starts with.
pub struct Entry {
    pub pc: u64,
    pub sp: u64,
    pub x0: u64,
    pub x1: u64,
    pub x2: u64,
    pub fp: u64,
    pub lr: u64,
}

impl Entry {
    pub fn to_ctx(&self, c: &mut GuestContext) {
        c.pc = self.pc;
        c.sp = self.sp;
        c.x[0] = self.x0;
        c.x[1] = self.x1;
        c.x[2] = self.x2;
        c.x[29] = self.fp;
        c.x[30] = self.lr;
        c.stub_ret = 0;
    }

    pub fn to_mc(&self, m: &mut DarwinMcontext) {
        m.pc = self.pc;
        m.sp = self.sp;
        m.x[0] = self.x0;
        m.x[1] = self.x1;
        m.x[2] = self.x2;
        m.fp = self.fp;
        m.lr = self.lr;
    }
}

/// What a frame carries besides the interrupted registers.
pub struct Delivery<'a> {
    pub info: &'a Siginfo,
    pub act: &'a KSigaction,
    /// The mask `rt_sigreturn` restores.
    pub uc_mask: u64,
    /// ESR for memory faults (adds an `esr_context`), else 0.
    pub esr: u64,
    /// `sigcontext.fault_address`.
    pub fault_address: u64,
    /// Used when the handler has no SA_RESTORER.
    pub default_restorer: u64,
}

unsafe fn put<T>(p: u64, v: T) {
    // SAFETY: caller writes inside a frame it owns.
    unsafe { (p as *mut T).write_unaligned(v) }
}

unsafe fn get<T>(p: u64) -> T {
    // SAFETY: caller reads inside a frame.
    unsafe { (p as *const T).read_unaligned() }
}

/// Build the frame for `d` below the interrupted `cpu` (or on the guest
/// alternate stack) and return the handler's entry registers. `alt` is
/// updated when SS_AUTODISARM disarms it.
///
/// # Safety
/// The chosen guest stack must be writable.
pub unsafe fn setup(cpu: &Cpu, d: &Delivery, alt: &mut AltStack) -> Entry {
    let saved_alt = *alt;
    let mut sp = cpu.sp;
    if d.act.flags & SA_ONSTACK != 0 && alt.enabled() && !alt.contains(sp) {
        sp = alt.sp + alt.size;
        if alt.flags & SS_AUTODISARM != 0 {
            *alt = AltStack::DISABLED;
        }
    }
    let record = (sp - 16) & !15;
    let f = (record - FRAME_SIZE) & !15;
    // SAFETY: caller contract; everything written lies in [f, record + 16).
    unsafe {
        std::ptr::write_bytes(
            f as *mut u8,
            0,
            (MC_RESERVED + 2 * FPSIMD_SIZE as u64) as usize,
        );
        d.info.write(f);
        put(UC_STACK + f, saved_alt.sp);
        put(UC_STACK + f + 8, saved_alt.flags);
        put(UC_STACK + f + 16, saved_alt.size);
        put(UC_SIGMASK + f, d.uc_mask);
        put(MC_FAULT + f, d.fault_address);
        for (i, r) in cpu.x.iter().enumerate() {
            put(MC_REGS + f + i as u64 * 8, *r);
        }
        put(MC_SP + f, cpu.sp);
        put(MC_PC + f, cpu.pc);
        put(MC_PSTATE + f, cpu.pstate);
        let mut p = MC_RESERVED + f;
        put(p, FPSIMD_MAGIC);
        put(p + 4, FPSIMD_SIZE);
        put(p + 8, cpu.fpsr);
        put(p + 12, cpu.fpcr);
        std::ptr::copy_nonoverlapping(cpu.v.as_ptr() as *const u8, (p + 16) as *mut u8, 512);
        p += FPSIMD_SIZE as u64;
        if d.esr != 0 {
            put(p, ESR_MAGIC);
            put(p + 4, ESR_SIZE);
            put(p + 8, d.esr);
            p += ESR_SIZE as u64;
        }
        put(p, 0u64); // terminator
        put(record, cpu.x[29]);
        put(record + 8, cpu.x[30]);
    }
    let siginfo = d.act.flags & SA_SIGINFO != 0;
    Entry {
        pc: d.act.handler,
        sp: f,
        x0: d.info.signo as u64,
        x1: if siginfo { f } else { cpu.x[1] },
        x2: if siginfo { f + UC } else { cpu.x[2] },
        fp: record,
        lr: if d.act.flags & SA_RESTORER != 0 {
            d.act.restorer
        } else {
            d.default_restorer
        },
    }
}

/// Read the frame `rt_sigreturn` found at the guest sp: the registers to
/// resume, the signal mask and the saved alternate stack. None for a
/// misaligned frame or one without an `fpsimd_context`.
///
/// # Safety
/// `f` must point to a readable frame.
pub unsafe fn restore(f: u64) -> Option<(Cpu, u64, AltStack)> {
    if f & 15 != 0 {
        return None;
    }
    // SAFETY: caller contract.
    unsafe {
        let mut x = [0u64; 31];
        for (i, r) in x.iter_mut().enumerate() {
            *r = get(MC_REGS + f + i as u64 * 8);
        }
        let mut cpu = Cpu {
            x,
            sp: get(MC_SP + f),
            pc: get(MC_PC + f),
            pstate: get::<u64>(MC_PSTATE + f) & NZCV,
            v: [0; 32],
            fpsr: 0,
            fpcr: 0,
        };
        let mut p = MC_RESERVED + f;
        let end = p + RESERVED_SIZE;
        let mut fpsimd = false;
        while p + 8 <= end {
            let (magic, size) = (get::<u32>(p), get::<u32>(p + 4));
            if magic == 0 {
                break;
            }
            if size < 16 || size % 16 != 0 {
                return None;
            }
            if magic == FPSIMD_MAGIC && size == FPSIMD_SIZE {
                cpu.fpsr = get(p + 8);
                cpu.fpcr = get(p + 12);
                std::ptr::copy_nonoverlapping(
                    (p + 16) as *const u8,
                    cpu.v.as_mut_ptr() as *mut u8,
                    512,
                );
                fpsimd = true;
            }
            p += size as u64;
        }
        if !fpsimd {
            return None;
        }
        let alt = AltStack {
            sp: get(UC_STACK + f),
            flags: get(UC_STACK + f + 8),
            size: get(UC_STACK + f + 16),
        };
        Some((cpu, get(UC_SIGMASK + f), alt))
    }
}

const SEGV_MAPERR: i32 = 1;
const SEGV_ACCERR: i32 = 2;
const BUS_ADRALN: i32 = 1;
const BUS_ADRERR: i32 = 2;
const ILL_ILLOPC: i32 = 1;
const TRAP_BRKPT: i32 = 1;

/// Darwin maps nothing below 4 GiB but `__PAGEZERO`, which Linux does not
/// have: faults there are SEGV_MAPERR, as on an unmapped Linux page.
const PAGEZERO_END: u64 = 1 << 32;

/// A synchronous fault as Linux reports it.
pub struct Fault {
    pub info: Siginfo,
    pub esr: u64,
    pub fault_address: u64,
}

/// Translate a Darwin fault signal into the Linux signal, si_code and
/// address (ADR 0012, "Platform probes").
///
/// Darwin raises SIGBUS for a PROT_NONE page, a write to a read-only page
/// and a read past the end of a mapped file alike, with the same ESR, so
/// memory faults are classified through the VM map: nothing mapped is
/// SEGV_MAPERR, a protection that forbids the access is SEGV_ACCERR, and
/// anything else (the pager failed) is BUS_ADRERR.
pub fn translate_fault(hsig: i32, host_code: i32, m: &DarwinMcontext) -> Fault {
    let esr = m.esr as u64;
    let ec = esr >> 26;
    let data_abort = ec == 0x24 || ec == 0x25;
    let insn_abort = ec == 0x20 || ec == 0x21;
    let (signo, code, addr) = match hsig {
        libc::SIGSEGV | libc::SIGBUS => {
            let addr = if data_abort || insn_abort {
                m.far
            } else {
                m.pc
            };
            if data_abort && esr & 0x3f == 0x21 {
                (SIGBUS, BUS_ADRALN, addr)
            } else if (super::window::BASE..super::window::BASE + 0x1000).contains(&addr) {
                // The first page of ART's heap window is the null page of
                // heap references (offset 0 is null): report the access
                // as the null-page fault an implicit null check expects.
                (SIGSEGV, SEGV_MAPERR, addr - super::window::BASE)
            } else {
                let need = if insn_abort {
                    libc::PROT_EXEC
                } else if data_abort && esr & (1 << 6) != 0 {
                    libc::PROT_WRITE
                } else {
                    libc::PROT_READ
                };
                match crate::patch::vm::region(addr) {
                    Some((lo, _, prot, _)) if lo <= addr && addr >= PAGEZERO_END => {
                        if prot & need != need {
                            (SIGSEGV, SEGV_ACCERR, addr)
                        } else {
                            (SIGBUS, BUS_ADRERR, addr)
                        }
                    }
                    _ => (SIGSEGV, SEGV_MAPERR, addr),
                }
            }
        }
        libc::SIGILL => (SIGILL, ILL_ILLOPC, m.pc),
        libc::SIGTRAP => (SIGTRAP, TRAP_BRKPT, m.pc),
        _ => {
            // Darwin FPE_* numbering to Linux's.
            let code = match host_code {
                7 => 1, // INTDIV
                8 => 2, // INTOVF
                1..=6 => host_code + 2,
                _ => 0,
            };
            (SIGFPE, code, m.pc)
        }
    };
    let mem = signo == SIGSEGV || signo == SIGBUS;
    Fault {
        info: Siginfo::fault(signo, code, addr),
        esr: if mem { esr } else { 0 },
        fault_address: if mem { addr } else { 0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_round_trips_registers_mask_and_altstack() {
        let mut stack = vec![0u8; 64 << 10];
        let top = (stack.as_mut_ptr() as u64 + stack.len() as u64) & !15;
        let mut cpu = Cpu {
            x: [0; 31],
            sp: top - 8,
            pc: 0x1234_5678,
            pstate: 0x6000_0000,
            v: [0; 32],
            fpsr: 0x10,
            fpcr: 0x0300_0000,
        };
        for (i, r) in cpu.x.iter_mut().enumerate() {
            *r = 0x1000 + i as u64;
        }
        cpu.v[8] = 0xcafe;
        let info = Siginfo::fault(SIGSEGV, SEGV_MAPERR, 8);
        let act = KSigaction {
            handler: 0x4000,
            flags: SA_SIGINFO | SA_RESTORER,
            restorer: 0x5000,
            mask: 0,
        };
        let mut alt = AltStack::DISABLED;
        let d = Delivery {
            info: &info,
            act: &act,
            uc_mask: 0x55,
            esr: 0x9200_0006,
            fault_address: 8,
            default_restorer: 0,
        };
        // SAFETY: the frame goes into `stack`.
        let e = unsafe { setup(&cpu, &d, &mut alt) };
        assert_eq!(e.sp % 16, 0);
        assert!(e.sp + FRAME_SIZE <= e.fp && e.fp + 16 <= top);
        assert_eq!(
            (e.pc, e.x0, e.x1, e.x2, e.lr),
            (0x4000, 11, e.sp, e.sp + UC, 0x5000)
        );
        // SAFETY: reading the frame just built.
        unsafe {
            assert_eq!(get::<u64>(e.sp + 16), 8, "si_addr");
            assert_eq!(get::<u64>(MC_FAULT + e.sp), 8, "fault_address");
            let esr = MC_RESERVED + e.sp + FPSIMD_SIZE as u64;
            assert_eq!(
                (get::<u32>(esr), get::<u64>(esr + 8)),
                (ESR_MAGIC, 0x9200_0006)
            );
            put(MC_REGS + e.sp + 19 * 8, 0x1919u64);
            let (back, mask, _) = restore(e.sp).unwrap();
            assert_eq!(mask, 0x55);
            assert_eq!(back.x[19], 0x1919);
            assert_eq!(back.x[30], cpu.x[30]);
            assert_eq!(
                (back.sp, back.pc, back.pstate),
                (cpu.sp, cpu.pc, cpu.pstate)
            );
            assert_eq!(
                (back.v[8], back.fpsr, back.fpcr),
                (0xcafe, 0x10, 0x0300_0000)
            );
        }
    }

    #[test]
    fn sa_onstack_uses_and_autodisarms_the_alternate_stack() {
        let mut main = vec![0u8; 16 << 10];
        let mut altmem = vec![0u8; 16 << 10];
        let alt_base = altmem.as_mut_ptr() as u64;
        let cpu = Cpu {
            x: [0; 31],
            sp: (main.as_mut_ptr() as u64 + main.len() as u64) & !15,
            pc: 0,
            pstate: 0,
            v: [0; 32],
            fpsr: 0,
            fpcr: 0,
        };
        let info = Siginfo::new(10, SI_TKILL);
        let act = KSigaction {
            handler: 1 << 20,
            flags: SA_ONSTACK,
            restorer: 0,
            mask: 0,
        };
        let mut alt = AltStack {
            sp: alt_base,
            size: altmem.len() as u64,
            flags: SS_AUTODISARM,
        };
        let d = Delivery {
            info: &info,
            act: &act,
            uc_mask: 0,
            esr: 0,
            fault_address: 0,
            default_restorer: 0x77,
        };
        // SAFETY: the frame goes into `altmem`.
        let e = unsafe { setup(&cpu, &d, &mut alt) };
        assert!(e.sp > alt_base && e.sp < alt_base + altmem.len() as u64);
        assert_eq!(e.lr, 0x77, "no SA_RESTORER: the layer's restorer");
        assert!(!alt.enabled(), "SS_AUTODISARM");
        // SAFETY: reading the frame just built.
        let (_, _, saved) = unsafe { restore(e.sp).unwrap() };
        assert_eq!((saved.sp, saved.flags), (alt_base, SS_AUTODISARM));
    }
}
