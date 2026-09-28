//! Host-call module [`aim_hostcall::module::MEMORY`]: the Mac's memory
//! pressure, for the guest's lmkd (`daemons/lmkd`).
//!
//! The level is the kernel's own (`kern.memorystatus_vm_pressure_level`,
//! what `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE` reports), read at each
//! [`FN_READ`], with the free and file-backed memory of
//! `host_statistics64`. A host thread watches the level and writes a byte
//! into every [`FN_WATCH`] pipe when it changes; lmkd polls the pipe. Host
//! code never calls into the guest.
//!
//! The thread polls the level ([`POLL`]) rather than use a dispatch
//! memory-pressure source: the kernel notifies only a few processes of a
//! change, the largest first, so a small daemon's source never fires
//! (measured under `memory_pressure -l warn`: the level read 2 for a
//! minute, and neither lmkd nor a plain host process got an event).

use std::ffi::CStr;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Mutex, Once};
use std::time::Duration;

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

/// How often the watcher thread reads the level.
pub const POLL: Duration = Duration::from_millis(250);

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

/// One of [`level`].
fn pressure_level() -> u32 {
    match sysctl::<i32>(c"kern.memorystatus_vm_pressure_level") {
        Some(2) => level::WARN,
        Some(4) => level::CRITICAL,
        _ => level::NORMAL,
    }
}

/// The Mac's pressure level and memory now.
pub fn read() -> Memory {
    let level = pressure_level();
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
    static WATCHER: Once = Once::new();
    WATCHER.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("aim-memory-pressure".into())
            .spawn(watch_level);
    });
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

/// The watcher thread: wake the watchers on each change of the level.
fn watch_level() {
    // SAFETY: plain call on this thread, which takes no guest signals.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
        let mut all: libc::sigset_t = 0;
        libc::sigfillset(&mut all);
        libc::pthread_sigmask(libc::SIG_BLOCK, &all, std::ptr::null_mut());
    }
    let mut last = pressure_level();
    loop {
        std::thread::sleep(POLL);
        let now = pressure_level();
        if now != last {
            last = now;
            notify();
        }
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
        // What the watcher thread does on a change.
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
