//! Host signal handling: the SIGTRAP path of the `brk` fallback, and fault
//! reports that name the guest module containing the faulting pc.

use std::sync::Mutex;

struct Module {
    start: u64,
    end: u64,
    name: String,
}

static MODULES: Mutex<Vec<Module>> = Mutex::new(Vec::new());

pub fn register_module(start: u64, end: u64, name: String) {
    MODULES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(Module { start, end, name });
}

/// Record an executable mapping created by the guest from an fd.
pub fn register_fd_module(start: u64, len: u64, fd: i32, off: u64) {
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes.
    let name = if let Some(p) = crate::xrt::original_guest_path(fd) {
        p
    } else if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } == 0 {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        let host = std::path::PathBuf::from(String::from_utf8_lossy(&buf[..len]).into_owned());
        crate::vfs::guest_path_of_host(&host).unwrap_or_else(|| host.display().to_string())
    } else {
        format!("fd {fd}")
    };
    register_module(start, start + len, format!("{name}+{off:#x}"));
}

pub fn describe(pc: u64) -> String {
    let m = MODULES.lock().unwrap_or_else(|e| e.into_inner());
    m.iter()
        .rev()
        .find(|m| (m.start..m.end).contains(&pc))
        .map(|m| format!("{}+{:#x}", m.name, pc - m.start))
        .unwrap_or_else(|| "?".into())
}

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
struct ExceptionState64 {
    far: u64,
    esr: u32,
    exception: u32,
}

#[repr(C)]
struct MContext64 {
    es: ExceptionState64,
    ss: ThreadState64,
}

extern "C" fn on_signal(sig: i32, info: *mut libc::siginfo_t, uc: *mut libc::c_void) {
    let uc = uc as *mut libc::ucontext_t;
    // SAFETY: SA_SIGINFO handler arguments from the kernel.
    unsafe {
        if sig == libc::SIGTRAP && crate::patch::handle_brk(uc) {
            return;
        }
        let mc = (*uc).uc_mcontext as *const MContext64;
        let ss = &(*mc).ss;
        eprintln!(
            "[linux-abi] fatal signal {} at pc {:#x} ({}), fault addr {:#x}, esr {:#x}",
            sig,
            ss.pc,
            describe(ss.pc),
            (*info).si_addr as u64,
            (*mc).es.esr
        );
        eprintln!(
            "[linux-abi]   lr {:#x} ({}) sp {:#x}",
            ss.lr,
            describe(ss.lr),
            ss.sp
        );
        for i in (0..29).step_by(4) {
            let mut line = String::from("[linux-abi]  ");
            for j in i..(i + 4).min(29) {
                line.push_str(&format!(" x{j:<2} {:#018x}", ss.x[j]));
            }
            eprintln!("{line}");
        }
        eprintln!("[linux-abi]   guest tp {:#x}", crate::context::guest_tp());
        // End the process with this signal, as the guest's death by it would
        // on Linux: init tells a crash from an exit by the wait status.
        libc::signal(sig, libc::SIG_DFL);
        let mut set: libc::sigset_t = 0;
        libc::sigaddset(&mut set, sig);
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
        libc::raise(sig);
        libc::_exit(128 + sig);
    }
}

const ALTSTACK_SIZE: usize = 1 << 20;

/// Install host handlers for faults and the `brk` fallback on this thread.
pub fn install_signal_handlers() {
    // SAFETY: installing process-wide handlers and a per-thread altstack.
    unsafe {
        let stack = libc::mmap(
            std::ptr::null_mut(),
            ALTSTACK_SIZE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANON,
            -1,
            0,
        );
        let ss = libc::stack_t {
            ss_sp: stack,
            ss_size: ALTSTACK_SIZE,
            ss_flags: 0,
        };
        libc::sigaltstack(&ss, std::ptr::null_mut());
        for sig in [
            libc::SIGTRAP,
            libc::SIGSEGV,
            libc::SIGBUS,
            libc::SIGILL,
            libc::SIGFPE,
        ] {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as usize;
            sa.sa_flags = libc::SA_SIGINFO | libc::SA_ONSTACK;
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}
