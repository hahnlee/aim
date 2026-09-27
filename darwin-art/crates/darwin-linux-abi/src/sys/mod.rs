//! Linux arm64 syscall dispatch.
//!
//! The syscall number is in x8, arguments in x0-x5; the result (or -errno,
//! with Linux errno values) goes back in x0. Each subsystem owns its calls.

mod binder;
pub mod cred;
mod events;
mod exec;
mod fork;
mod fs;
mod futex;
mod mem;
mod misc;
pub mod names;
mod process;
mod procfs;
mod pstate;
mod selinuxfs;
mod signal;
mod thread;
mod unix_socket;
mod wait;

use std::sync::atomic::{AtomicBool, Ordering};

use crate::context::GuestContext;
use crate::errno::ENOSYS;

pub use binder::init as init_binder;
pub use exec::{ExecState, init as init_exec};
pub use mem::init_brk;
pub use mem::run_deferred_unmaps;
pub use process::{host_tid, set_exe};
pub use pstate::kernel_release;

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
        eprintln!("[linux-abi] panic in syscall handler; aborting");
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
            None if nr == darwin_hostcall::SYSCALL_NR => "hostcall",
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
            eprintln!("{line}");
        }
    }
    let r = handle(ctx, nr, a);
    if tracing() {
        if (-4095..0).contains(&r) {
            eprintln!("{line} = -{} ({})", -r, names::errno_name(-r as i32));
        } else if !(0..=0xffff).contains(&r) {
            eprintln!("{line} = {r:#x}");
        } else {
            eprintln!("{line} = {r}");
        }
    }
    ctx.x[0] = r as u64;
}

fn handle(ctx: &mut GuestContext, nr: u64, a: [u64; 6]) -> i64 {
    match nr {
        // files
        17 => fs::getcwd(a),
        23 => fs::dup(a),
        24 => fs::dup3(a),
        25 => fs::fcntl(a),
        29 => fs::ioctl(a),
        43 => fs::statfs(a),
        44 => fs::fstatfs(a),
        48 => fs::faccessat(a[0], a[1], a[2], 0),
        439 => fs::faccessat(a[0], a[1], a[2], a[3]),
        49 => fs::chdir(a),
        56 => fs::openat(a),
        57 => fs::close(a),
        59 => fs::pipe2(a),
        62 => fs::lseek(a),
        63 => fs::read(a),
        64 => fs::write(a),
        65 => fs::readv(a),
        66 => fs::writev(a),
        67 => fs::pread64(a),
        68 => fs::pwrite64(a),
        78 => fs::readlinkat(a),
        79 => fs::newfstatat(a),
        80 => fs::fstat(a),
        // readiness
        19 => events::eventfd2(a),
        20 => events::epoll_create1(a),
        21 => events::epoll_ctl(a),
        22 => events::epoll_pwait(a),
        73 => events::ppoll(a),
        85 => events::timerfd_create(a),
        86 => events::timerfd_settime(a),
        87 => events::timerfd_gettime(a),
        // memory
        214 => mem::brk(a),
        215 => mem::munmap(ctx, a),
        216 => mem::mremap(a),
        222 => mem::mmap(a),
        226 => mem::mprotect(a),
        233 => mem::madvise(a),
        // process
        93 => thread::exit(a),
        220 if fork::is_fork(a[0]) => fork::clone(ctx, a),
        220 => thread::clone(ctx, a),
        94 => process::exit_group(a),
        96 => process::set_tid_address(a),
        98 => futex::futex(a),
        99 => 0, // set_robust_list: robust futex lists matter once threads exist
        163 | 164 | 261 => cred::prlimit(nr, a),
        172 => process::getpid(),
        173 => process::getppid(),
        178 => process::gettid(),
        129..=131 => process::kill(nr, a),
        // signals
        132 => signal::sigaltstack(a),
        134 => signal::rt_sigaction(a),
        135 => signal::rt_sigprocmask(a),
        // sockets
        198 => unix_socket::socket(a),
        203 => unix_socket::connect(a),
        206 => unix_socket::sendto(a),
        207 => unix_socket::recvfrom(a),
        // misc
        101 => misc::nanosleep(a),
        113 => misc::clock_gettime(a),
        114 => misc::clock_getres(a),
        118..=121 => process::sched_policy(nr, a),
        123 => process::sched_getaffinity(a),
        124 => misc::sched_yield(),
        167 => misc::prctl(a),
        169 => misc::gettimeofday(a),
        278 => misc::getrandom(a),
        darwin_hostcall::SYSCALL_NR => crate::hostcall::call(a[0], a[1], a[2], a[3]),
        // process lifecycle
        435 if fork::is_fork(fork::clone3_flags(a)) => fork::clone3(ctx, a),
        221 => exec::execve(a),
        281 => exec::execveat(a),
        95 => wait::waitid(a),
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
        _ => {
            let args: Vec<String> = a[..names::arg_count(nr)]
                .iter()
                .map(|v| format!("{v:#x}"))
                .collect();
            let pc = ctx.resume_pc() - 4;
            eprintln!(
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
