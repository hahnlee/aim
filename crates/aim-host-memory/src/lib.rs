//! Host-call module [`aim_hostcall::module::MEMORY`]: the Mac's memory
//! pressure, for the guest's lmkd (`daemons/lmkd`).
//!
//! The level is the kernel's own (`kern.memorystatus_vm_pressure_level`,
//! what `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE` reports), read at each
//! [`FN_READ`], with the free and file-backed memory of
//! `host_statistics64`. A dispatch memory-pressure source only wakes the
//! watchers: each level change writes a byte into every [`FN_WATCH`] pipe,
//! which lmkd polls. Host code never calls into the guest.

use std::ffi::{CStr, c_void};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Mutex, Once};

use aim_hostcall::memory::{FN_READ, FN_WATCH, Memory, VERSION, level};
use aim_hostcall::{HostModule, args_mut, errno, module};

pub static MODULE: HostModule = HostModule {
    id: module::MEMORY,
    name: "memory",
    version: VERSION,
    call,
};

unsafe fn call(func: u32, args: u64, len: u64) -> i64 {
    match func {
        // SAFETY: the registry passes the guest's argument block.
        FN_READ => match unsafe { args_mut::<Memory>(args, len) } {
            Ok(out) => {
                *out = read();
                0
            }
            Err(e) => e,
        },
        FN_WATCH => watch(),
        _ => -(errno::ENOSYS as i64),
    }
}

fn sysctl<T: Default>(name: &CStr) -> Option<T> {
    let mut value = T::default();
    let mut size = size_of::<T>();
    // SAFETY: a sysctl read into a local of the size given.
    let r = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&raw mut value).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (r == 0 && size == size_of::<T>()).then_some(value)
}

/// The Mac's pressure level and memory now.
pub fn read() -> Memory {
    let level = match sysctl::<i32>(c"kern.memorystatus_vm_pressure_level") {
        Some(2) => level::WARN,
        Some(4) => level::CRITICAL,
        _ => level::NORMAL,
    };
    // SAFETY: an all-zero vm_statistics64 is valid.
    let mut vm: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: HOST_VM_INFO64 into a vm_statistics64 of `count` words.
    #[allow(deprecated)]
    let ok = unsafe {
        libc::host_statistics64(
            libc::mach_host_self(),
            libc::HOST_VM_INFO64,
            (&raw mut vm).cast(),
            &mut count,
        )
    } == 0;
    // SAFETY: plain sysconf.
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as u64;
    let bytes = |pages: u32| pages as u64 * page;
    Memory {
        level,
        reserved: 0,
        total: sysctl::<u64>(c"hw.memsize").unwrap_or(0),
        free: if ok {
            bytes(vm.free_count) + bytes(vm.speculative_count)
        } else {
            0
        },
        file: if ok {
            bytes(vm.external_page_count) + bytes(vm.purgeable_count)
        } else {
            0
        },
    }
}

/// The write ends of the watchers' pipes.
static WATCHERS: Mutex<Vec<OwnedFd>> = Mutex::new(Vec::new());

/// Darwin's `F_SETNOSIGPIPE`: a write to a pipe whose reader has gone
/// fails with EPIPE instead of raising SIGPIPE.
const F_SETNOSIGPIPE: i32 = 73;

fn watch() -> i64 {
    let mut fds = [0i32; 2];
    // SAFETY: a pipe into a local array.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return -(errno::ENODEV as i64);
    }
    // SAFETY: the fds were just made and are ours.
    let (read_end, write_end) =
        unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    for fd in [&read_end, &write_end] {
        // SAFETY: flags on our new fds.
        unsafe {
            libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);
            libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK);
        }
    }
    // SAFETY: as above.
    unsafe { libc::fcntl(write_end.as_raw_fd(), F_SETNOSIGPIPE, 1) };
    WATCHERS.lock().unwrap().push(write_end);
    static SOURCE: Once = Once::new();
    SOURCE.call_once(start_source);
    // The guest owns the read end from here on.
    std::os::fd::IntoRawFd::into_raw_fd(read_end) as i64
}

/// Wake every watcher; forget those whose reader has gone.
pub fn notify() {
    WATCHERS.lock().unwrap().retain(|fd| {
        // SAFETY: one byte into our non-blocking pipe. A full pipe already
        // holds a wake-up.
        let n = unsafe { libc::write(fd.as_raw_fd(), [1u8].as_ptr().cast(), 1) };
        n == 1 || std::io::Error::last_os_error().raw_os_error() != Some(libc::EPIPE)
    });
}

const DISPATCH_MEMORYPRESSURE_NORMAL: usize = 0x01;
const DISPATCH_MEMORYPRESSURE_WARN: usize = 0x02;
const DISPATCH_MEMORYPRESSURE_CRITICAL: usize = 0x04;
const QOS_CLASS_UTILITY: isize = 0x11;

unsafe extern "C" {
    static _dispatch_source_type_memorypressure: c_void;
    fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *const c_void;
    fn dispatch_source_create(
        ty: *const c_void,
        handle: usize,
        mask: usize,
        q: *const c_void,
    ) -> *mut c_void;
    fn dispatch_source_set_event_handler_f(
        source: *mut c_void,
        handler: extern "C" fn(*mut c_void),
    );
    fn dispatch_resume(object: *mut c_void);
}

extern "C" fn pressure_changed(_: *mut c_void) {
    notify();
}

fn start_source() {
    // SAFETY: the source lives for the process; its handler only writes to
    // our pipes.
    unsafe {
        let source = dispatch_source_create(
            &raw const _dispatch_source_type_memorypressure,
            0,
            DISPATCH_MEMORYPRESSURE_NORMAL
                | DISPATCH_MEMORYPRESSURE_WARN
                | DISPATCH_MEMORYPRESSURE_CRITICAL,
            dispatch_get_global_queue(QOS_CLASS_UTILITY, 0),
        );
        if source.is_null() {
            return;
        }
        dispatch_source_set_event_handler_f(source, pressure_changed);
        dispatch_resume(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call_read(m: &mut Memory, len: u64) -> i64 {
        // SAFETY: `m` is a Memory block; `len` is what the guest claims.
        unsafe { call(FN_READ, m as *mut Memory as u64, len) }
    }

    #[test]
    fn reads_the_macs_memory() {
        let mut m = Memory::default();
        assert_eq!(call_read(&mut m, size_of::<Memory>() as u64), 0);
        assert!([level::NORMAL, level::WARN, level::CRITICAL].contains(&m.level));
        assert_eq!(Some(m.total), sysctl::<u64>(c"hw.memsize"));
        assert!(m.free > 0 && m.free < m.total, "{m:?}");
        assert!(m.file < m.total, "{m:?}");
        assert_eq!(
            call_read(&mut m, 8),
            -(errno::EINVAL as i64),
            "a block of the wrong size"
        );
        // SAFETY: an unknown function touches nothing.
        assert_eq!(unsafe { call(99, 0, 0) }, -(errno::ENOSYS as i64));
    }

    #[test]
    fn watchers_wake_on_a_level_change() {
        // SAFETY: FN_WATCH takes no argument block.
        let a = unsafe { call(FN_WATCH, 0, 0) };
        let b = unsafe { call(FN_WATCH, 0, 0) };
        assert!(a >= 0 && b >= 0 && a != b);
        // SAFETY: the read ends are ours now.
        let (a, b) = unsafe {
            (
                OwnedFd::from_raw_fd(a as i32),
                OwnedFd::from_raw_fd(b as i32),
            )
        };
        let pending = |fd: &OwnedFd| {
            let mut buf = [0u8; 16];
            // SAFETY: a non-blocking read into a local buffer.
            unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) }
        };
        assert_eq!(pending(&a), -1, "nothing before a change");
        // What the dispatch source's handler does on a change.
        notify();
        assert_eq!(pending(&a), 1);
        assert_eq!(pending(&b), 1);
        // A watcher that has gone is dropped without SIGPIPE.
        drop(b);
        notify();
        assert_eq!(pending(&a), 1);
        assert_eq!(WATCHERS.lock().unwrap().len(), 1);
    }
}
