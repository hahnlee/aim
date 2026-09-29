//! Fault reports that name the guest module containing the faulting pc.
//! The host signal handlers themselves are `sys::signal`'s.
//!
//! The layer's own messages ([`diag!`](crate::diag)) go to the diagnostics
//! descriptor: stderr, or with `--stdio-null` a hidden copy of it, so that
//! the guest's stdio can be `/dev/null` as init gives services.

use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering::Relaxed};

static LOG_FD: AtomicI32 = AtomicI32::new(2);

/// Send the layer's messages to `fd` from now on.
pub fn log_to(fd: i32) {
    LOG_FD.store(fd, Relaxed);
}

pub fn log_fd() -> i32 {
    LOG_FD.load(Relaxed)
}

/// `--diag-fd`: `fd` is the diagnostics descriptor a previous program of
/// this process kept across exec.
pub fn keep_log_fd(fd: i32) {
    crate::sys::fdtab::keep_hidden(fd);
    log_to(fd);
}

/// `--stdio-null`: keep stderr as the (hidden, exec-surviving)
/// diagnostics descriptor and give the guest `/dev/null` as stdin, stdout
/// and stderr, as init gives its services. Returns the descriptor.
pub fn stdio_null() -> std::io::Result<i32> {
    // SAFETY: duplicating and replacing this process's own descriptors.
    unsafe {
        // High up, like the layer's other descriptors (`fdtab::hide`).
        let base = crate::sys::fdtab::hidden_base();
        let fd = libc::fcntl(2, libc::F_DUPFD, base);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        if null < 0 {
            return Err(std::io::Error::last_os_error());
        }
        for std in 0..3 {
            libc::dup2(null, std);
        }
        if null > 2 {
            libc::close(null);
        }
        keep_log_fd(fd);
        Ok(fd)
    }
}

/// One line on the diagnostics descriptor, written with a single `write`.
pub fn write_line(args: std::fmt::Arguments) {
    let mut line = args.to_string();
    line.push('\n');
    let mut rest = line.as_bytes();
    while !rest.is_empty() {
        // SAFETY: writing a local buffer.
        let n = unsafe { libc::write(log_fd(), rest.as_ptr().cast(), rest.len()) };
        if n <= 0 {
            break;
        }
        rest = &rest[n as usize..];
    }
}

/// `eprintln!` for the layer's own messages (see the module docs).
#[macro_export]
macro_rules! diag {
    ($($arg:tt)*) => {
        $crate::diag::write_line(format_args!($($arg)*))
    };
}

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
        crate::diag!(
            "[linux-abi] fatal signal {} at pc {:#x} ({}), fault addr {:#x}, esr {:#x}",
            sig,
            ss.pc,
            describe(ss.pc),
            (*info).si_addr as u64,
            (*mc).es.esr
        );
        crate::diag!(
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
            crate::diag!("{line}");
        }
        crate::diag!("[linux-abi]   guest tp {:#x}", crate::context::guest_tp());
    }
}

/// Install the host signal handlers (faults, `brk` sites, guest signals).
/// Each guest thread gets its host alternate stack in
/// `context::init_thread`.
pub fn install_signal_handlers() {
    crate::sys::install_host_handlers();
}

/// execve in place: the old image's mappings are gone.
pub(crate) fn exec_reset() {
    MODULES.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

/// Fork: the named executable mappings, for diagnostics.
pub(crate) fn fork_save(w: &mut crate::sys::fork_state::Writer) {
    let m = MODULES.lock().unwrap_or_else(|e| e.into_inner());
    w.seq(m.iter(), |w, m| {
        w.u64(m.start);
        w.u64(m.end);
        w.str(&m.name);
    });
}

pub(crate) fn fork_restore(r: &mut crate::sys::fork_state::Reader) {
    let v = r.seq(|r| Module {
        start: r.u64(),
        end: r.u64(),
        name: r.str(),
    });
    *MODULES.lock().unwrap_or_else(|e| e.into_inner()) = v;
}
