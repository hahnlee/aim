//! Linux arm64 syscall dispatch.
//!
//! The syscall number is in x8, arguments in x0-x5; the result (or -errno,
//! with Linux errno values) goes back in x0. Each subsystem owns its calls.

mod arena;
mod ashmem;
mod attrs;
mod binder;
mod bpf;
pub(crate) mod clock;
mod copies;
mod copy;
pub mod cred;
mod dhcp;
mod dir;
mod epoll;
mod evdev;
mod event;
mod exec;
pub(crate) mod fdtab;
mod fork;
mod fs;
mod fsops;
mod futex;
mod genfs;
mod inotify;
mod itimer;
mod jit;
mod knob;
mod mem;
mod memfd;
mod misc;
mod mount;
pub mod names;
mod net;
mod netif;
mod netlink;
mod packet;
mod park;
mod pidns;
mod poll;
mod process;
mod procfs;
mod procrec;
mod pstate;
mod ptimer;
mod random;
mod ptrace;
mod selinuxfs;
mod sharedfile;
mod sigframe;
mod signal;
pub(crate) mod space;
mod sync_file;
mod thread;
mod tmpfile;
mod uevent;
mod uplink;
mod vmmap;
mod wait;
pub mod window;
mod xattr;

use std::sync::atomic::{AtomicBool, Ordering};

use crate::context::GuestContext;
use crate::errno::ENOSYS;

pub use arena::map_anon as map_guest_anon;
pub use binder::init as init_binder;
pub use copies::note as note_copy;
pub use dir::synthesized_path as synthesized_dir_path;
pub use exec::{ExecState, init as init_exec, interpret as interpret_script};
pub use fdtab::adopt as adopt_fd;
pub use fdtab::init as init_fds;
pub use fork::spawn::{child_main as become_fork_child, reserve_fds as reserve_fork_fds};
pub(crate) use fork::state as fork_state;
pub use mem::init_brk;
pub use mem::run_deferred_unmaps;
pub use pidns::enter as enter_pid_namespace;
pub use pidns::new_table as new_pid_namespace;
pub use process::set_exe;
pub use procfs::{StackInfo, note_stack};
pub use pstate::kernel_release;
pub use ptrace::start as start_ptrace_agent;
pub(crate) use signal::{install_host_handlers, repoke_self};
pub(crate) use thread::{Thread, register_current};
pub use thread::{host_tid, name_program};
pub use window::init as init_heap_window;

/// Read by the trampoline: while set, every syscall takes the full path so
/// it is traced.
pub static TRACE: AtomicBool = AtomicBool::new(false);

pub fn set_trace(on: bool) {
    TRACE.store(on, Ordering::Relaxed);
}

pub fn tracing() -> bool {
    TRACE.load(Ordering::Relaxed)
}

/// Entry from `trampoline.S`: `ctx` is the calling thread's saved state.
#[unsafe(no_mangle)]
extern "C" fn linux_abi_dispatch(ctx: *mut GuestContext) {
    // SAFETY: the trampoline passes this thread's live context.
    let ctx = unsafe { &mut *ctx };
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| dispatch(ctx))).is_err() {
        crate::diag!("[linux-abi] panic in syscall handler; aborting");
        std::process::abort();
    }
}

/// Read a NUL-terminated guest string.
///
/// # Safety
/// `p` must point to readable guest memory.
pub(crate) unsafe fn guest_cstr<'a>(p: u64) -> &'a [u8] {
    if p == 0 {
        return b"";
    }
    // SAFETY: caller contract.
    unsafe { std::ffi::CStr::from_ptr(p as *const libc::c_char).to_bytes() }
}

/// Syscalls whose first or second argument is a path, for tracing.
fn path_arg(nr: u64) -> Option<usize> {
    match nr {
        34 | 35 | 48 | 56 | 78 | 79 | 439 | 291 | 53 | 54 | 88 => Some(1),
        17 | 49 | 221 => Some(0),
        _ => None,
    }
}

pub fn dispatch(ctx: &mut GuestContext) {
    let nr = ctx.x[8];
    let a = [ctx.x[0], ctx.x[1], ctx.x[2], ctx.x[3], ctx.x[4], ctx.x[5]];
    let mut line = String::new();
    if tracing() {
        let name = match names::name(nr) {
            Some(n) => n,
            None if nr == aim_hostcall::SYSCALL_NR => "hostcall",
            None => "?",
        };
        line = format!("[linux] {name}(");
        for (i, v) in a.iter().take(names::arg_count(nr)).enumerate() {
            if i > 0 {
                line.push_str(", ");
            }
            if path_arg(nr) == Some(i) && nr != 17 {
                // SAFETY: path arguments point at guest strings.
                line.push_str(&format!(
                    "\"{}\"",
                    String::from_utf8_lossy(unsafe { guest_cstr(*v) })
                ));
            } else if (*v as i64) < 0 && (*v as i64) > -4096 {
                line.push_str(&format!("{}", *v as i64));
            } else {
                line.push_str(&format!("{v:#x}"));
            }
        }
        line.push(')');
        if matches!(nr, 93 | 94) {
            crate::diag!("{line}");
        }
    }
    // A syscall a host signal interrupted is restarted when no guest
    // handler is to run for it.
    let r = loop {
        let r = handle(ctx, nr, a);
        if !signal::after_syscall(ctx, nr, &a, r) {
            break r;
        }
    };
    if tracing() {
        if (-4095..0).contains(&r) {
            crate::diag!("{line} = -{} ({})", -r, names::errno_name(-r as i32));
        } else if !(0..=0xffff).contains(&r) {
            crate::diag!("{line} = {r:#x}");
        } else {
            crate::diag!("{line} = {r}");
        }
    }
    ctx.x[0] = r as u64;
}

fn handle(ctx: &mut GuestContext, nr: u64, a: [u64; 6]) -> i64 {
    match nr {
        // files
        5..=7 => xattr::setxattr(nr, a),
        8..=10 => xattr::getxattr(nr, a),
        11..=13 => xattr::listxattr(nr, a),
        14..=16 => xattr::removexattr(nr, a),
        17 => fs::getcwd(a),
        23 => fs::dup(a),
        24 => fs::dup3(a),
        25 => fs::fcntl(a),
        29 => fs::ioctl(a),
        32 => fsops::flock(a),
        33 => fsops::mknodat(a),
        34 => fsops::mkdirat(a),
        35 => fsops::unlinkat(a),
        36 => fsops::symlinkat(a),
        37 => fsops::linkat(a),
        38 => fsops::renameat(a),
        43 => fs::statfs(a),
        44 => fs::fstatfs(a),
        45 => fsops::truncate(a),
        46 => fsops::ftruncate(a),
        47 => fsops::fallocate(a),
        48 => fs::faccessat(a[0], a[1], a[2], 0),
        439 => fs::faccessat(a[0], a[1], a[2], a[3]),
        49 => fs::chdir(a),
        50 => fs::fchdir(a),
        52 => fsops::fchmod(a),
        53 => fsops::fchmodat(a),
        54 => fsops::fchownat(a),
        55 => fsops::fchown(a),
        56 => fs::openat(a),
        57 => fs::close(a),
        59 => fs::pipe2(a),
        61 => dir::getdents64(a),
        62 => fs::lseek(a),
        63 => fs::read(a),
        64 => fs::write(a),
        65 => fs::readv(a),
        66 => fs::writev(a),
        67 => fs::pread64(a),
        68 => fs::pwrite64(a),
        69 | 286 => fs::preadv(false, a),
        70 | 287 => fs::preadv(true, a),
        71 => copy::sendfile(a),
        76 => copy::splice(a),
        78 => fs::readlinkat(a),
        79 => fs::newfstatat(a),
        80 => fs::fstat(a),
        81 => fsops::sync(),
        82 | 83 | 267 => fsops::fsync(a),
        84 => fsops::sync_file_range(a),
        88 => fsops::utimensat(a),
        213 | 223 => 0, // readahead, fadvise64: advice only
        276 => fsops::renameat2(a),
        279 => memfd::memfd_create(a),
        285 => copy::copy_file_range(a),
        291 => fs::statx(a),
        436 => fs::close_range(a),
        // events
        19 => event::eventfd2(a),
        20 => epoll::epoll_create1(a),
        21 => epoll::epoll_ctl(a),
        22 => epoll::epoll_pwait(a),
        441 => epoll::epoll_pwait2(a),
        26 => inotify::inotify_init1(a),
        27 => inotify::inotify_add_watch(a),
        28 => inotify::inotify_rm_watch(a),
        72 => poll::pselect6(a),
        73 => poll::ppoll(a),
        85 => event::timerfd_create(a),
        86 => event::timerfd_settime(a),
        87 => event::timerfd_gettime(a),
        102 => itimer::getitimer(a),
        103 => itimer::setitimer(a),
        107 => ptimer::timer_create(a),
        108 => ptimer::timer_gettime(a),
        109 => ptimer::timer_getoverrun(a),
        110 => ptimer::timer_settime(a),
        111 => ptimer::timer_delete(a),
        // sockets
        198 => net::socket(a),
        199 => net::socketpair(a),
        200..=212 | 242 | 243 | 269 if net::hidden_socket(a[0] as i32) => -net::ENOTSOCK,
        200 => net::bind(a),
        201 => net::listen(a),
        202 => net::accept4([a[0], a[1], a[2], 0, 0, 0]),
        242 => net::accept4(a),
        203 => net::connect(a),
        204 => net::getsockname(a),
        205 => net::getpeername(a),
        206 => net::sendto(a),
        207 => net::recvfrom(a),
        208 => net::setsockopt(a),
        209 => net::getsockopt(a),
        210 => net::shutdown(a),
        211 => net::sendmsg(a),
        212 => net::recvmsg(a),
        243 => net::recvmmsg(a),
        269 => net::sendmmsg(a),
        // memory
        214 => mem::brk(a),
        215 => mem::munmap(ctx, a),
        216 => mem::mremap(a),
        222 => mem::mmap(a),
        226 => mem::mprotect(a),
        227 => mem::msync(a),
        228 | 229 => mem::mlock(nr, a),
        230 | 231 => 0, // mlockall/munlockall: Darwin pages are not locked per process
        232 => mem::mincore(a),
        233 => mem::madvise(a),
        270 => mem::process_vm_rw(false, a),
        271 => mem::process_vm_rw(true, a),
        283 => mem::membarrier(a),
        284 => mem::mlock(228, a),
        // process and threads
        93 => thread::exit(a),
        220 if fork::is_fork(a[0]) => fork::clone(ctx, a),
        94 => thread::exit_group(a),
        96 => thread::set_tid_address(a),
        98 => futex::futex(a),
        99 => thread::set_robust_list(a),
        100 => thread::get_robust_list(a),
        220 => thread::clone(ctx, a),
        435 if fork::is_fork(fork::clone3_flags(a)) => fork::clone3(ctx, a),
        435 => thread::clone3(ctx, a),
        163 | 164 | 261 => cred::prlimit(nr, a),
        172 => process::getpid(),
        173 => process::getppid(),
        178 => thread::gettid(),
        // signals
        129 => signal::kill(a),
        130 | 131 => signal::tgkill(nr, a),
        132 => signal::sigaltstack(ctx, a),
        133 => signal::rt_sigsuspend(a),
        134 => signal::rt_sigaction(a),
        135 => signal::rt_sigprocmask(a),
        136 => signal::rt_sigpending(a),
        137 => signal::rt_sigtimedwait(a),
        138 => signal::rt_sigqueueinfo(a),
        139 => signal::rt_sigreturn(ctx),
        240 => signal::rt_tgsigqueueinfo(a),
        // misc
        101 => park::nanosleep(a),
        115 => park::clock_nanosleep(a),
        113 => clock::clock_gettime(a),
        114 => clock::clock_getres(a),
        118..=121 => process::sched_policy(nr, a),
        122 => process::sched_setaffinity(a),
        123 => process::sched_getaffinity(a),
        125 | 126 => process::sched_priority_range(nr, a),
        124 => misc::sched_yield(),
        167 => misc::prctl(a),
        169 => clock::gettimeofday(a),
        278 => misc::getrandom(a),
        aim_hostcall::SYSCALL_NR => crate::hostcall::call(a[0], a[1], a[2], a[3]),
        // process lifecycle
        221 => exec::execve(ctx, a),
        281 => exec::execveat(ctx, a),
        95 => wait::waitid(a),
        117 => ptrace::ptrace(a),
        260 => wait::wait4(a),
        424 => wait::pidfd_send_signal(a),
        434 => wait::pidfd_open(a),
        // identity
        174..=177 => cred::getuid(nr),
        143 | 145 => cred::setreid(nr, a),
        144 | 146 => cred::setid(nr, a),
        147 | 149 => cred::setresid(nr, a),
        148 | 150 => cred::getresid(nr, a),
        151 | 152 => cred::setfsid(nr, a),
        158 => cred::getgroups(a),
        159 => cred::setgroups(a),
        90 => cred::capget(a),
        91 => cred::capset(a),
        140 => cred::setpriority(a),
        141 => cred::getpriority(a),
        // process state
        92 => pstate::personality(a),
        153 => pstate::times(a),
        154 => pstate::setpgid(a),
        155 => pstate::getpgid(a),
        156 => pstate::getsid(a),
        157 => pstate::setsid(),
        160 => pstate::uname(a),
        165 => pstate::getrusage(a),
        166 => pstate::umask(a),
        168 => pstate::getcpu(a),
        179 => pstate::sysinfo(a),
        // eBPF: maps, and programs that never run
        280 => bpf::bpf(a),
        // mount namespaces
        97 => mount::unshare(a),
        39 => mount::umount2(a),
        40 => mount::mount(a),
        // userfaultfd: answered "unsupported" (ADR 0012); ART then uses
        // its concurrent-copying collector.
        282 => -(ENOSYS as i64),
        _ => {
            let args: Vec<String> = a[..names::arg_count(nr)]
                .iter()
                .map(|v| format!("{v:#x}"))
                .collect();
            let pc = ctx.resume_pc() - 4;
            crate::diag!(
                "[linux-abi] unimplemented syscall {} ({}) args [{}] at pc {:#x} ({})",
                nr,
                names::name(nr).unwrap_or("unknown"),
                args.join(", "),
                pc,
                crate::diag::describe(pc)
            );
            -(ENOSYS as i64)
        }
    }
}
