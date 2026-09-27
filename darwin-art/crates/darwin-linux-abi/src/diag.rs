//! Fault reports that name the guest module containing the faulting pc.
//! The host signal handlers themselves are `sys::signal`'s.

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
        .or_else(|| host_symbol(pc))
        .unwrap_or_else(|| "?".into())
}

/// A host pc (the layer or a system library) as `symbol+offset (image)`.
fn host_symbol(pc: u64) -> Option<String> {
    let mut info: libc::Dl_info = unsafe { std::mem::zeroed() };
    // SAFETY: dladdr fills `info` with pointers into loaded images.
    if unsafe { libc::dladdr(pc as *const libc::c_void, &mut info) } == 0 {
        return None;
    }
    let text = |p: *const libc::c_char| {
        // SAFETY: NUL-terminated strings owned by dyld.
        (!p.is_null()).then(|| unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy())
    };
    let image = text(info.dli_fname)?;
    let image = image.rsplit('/').next().unwrap_or(&image).to_string();
    Some(match text(info.dli_sname) {
        Some(symbol) => format!("{symbol}+{:#x} ({image})", pc - info.dli_saddr as u64),
        None => format!("{image}+{:#x}", pc - info.dli_fbase as u64),
    })
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

/// Report a fault no guest handler takes.
///
/// # Safety
/// Handler arguments from the kernel.
pub unsafe fn report(sig: i32, info: *mut libc::siginfo_t, uc: *mut libc::c_void) {
    let uc = uc as *mut libc::ucontext_t;
    // SAFETY: SA_SIGINFO handler arguments from the kernel.
    unsafe {
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
    }
}

/// Install the host signal handlers (faults, `brk` sites, guest signals).
/// Each guest thread gets its host alternate stack in
/// `context::init_thread`.
pub fn install_signal_handlers() {
    crate::sys::install_host_handlers();
}
