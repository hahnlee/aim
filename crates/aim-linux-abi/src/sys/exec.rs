//! execve and execveat.
//!
//! The target is classified as Linux does (`binfmt_script`, then
//! `binfmt_elf`): a `#!` script is rewritten into its interpreter's
//! argument vector, an arm64 ELF is run, anything else (a Mach-O among
//! them) is ENOEXEC. Running a program replaces the process image.
//!
//! A process whose only guest thread execs, and which holds no host
//! module state (GPU, audio, ...), replaces its image in place, as Linux's
//! `begin_new_exec` does: CLOEXEC fds are closed, the guest range is
//! unmapped with what the layer records about it, caught signals go back
//! to their defaults and the alternate stack goes, the thread loses its
//! clear_child_tid, robust list, `comm` and thread pointer, credentials
//! take the exec transform, and the new program and its interpreter are
//! loaded; the syscall returns into the interpreter's entry. What fails
//! before that (the program or interpreter cannot be read) is returned to
//! the caller; a load failure after it kills the process with SIGSEGV, as
//! on Linux. Everything else stays: fds, the signal mask and pending
//! signals, the working directory and mounts, the pid and credentials, and
//! the layer's caches (path map, translation cache, attributes). The
//! command line other processes read moves to the process's `by-pid`
//! entry, since the host's argv keeps naming the program the process
//! started with.
//!
//! Otherwise the layer re-executes `linux-run` itself with the program, the
//! guest's argv and envp, and the state Linux keeps across exec:
//!
//! - open fds without O_CLOEXEC: guest fds are host fds, and Darwin's exec
//!   keeps exactly those;
//! - the signal mask and the ignored dispositions ([`ExecState`]);
//! - credentials (after the exec capability transform), the working
//!   directory and the personality.
//!
//! Darwin keeps the pid, parent, session, process group, umask and rlimits
//! itself. `AT_EXECFN` is the filename as the caller named it; `argv[0]` is
//! whatever the caller passed.

use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::sync::OnceLock;

use crate::context::{self, GuestContext};
use crate::errno::{self, EACCES, EFAULT, EINVAL, ELOOP, ENOENT, ENOEXEC};
use crate::sys::guest_cstr;
use crate::vfs::{self, LINUX_AT_FDCWD};

const AT_SYMLINK_NOFOLLOW: u64 = 0x100;
const AT_EMPTY_PATH: u64 = 0x1000;
/// Linux `BINPRM_BUF_SIZE`: how much of a file `#!` parsing sees.
const BINPRM_BUF_SIZE: usize = 256;
/// Linux allows this many interpreter levels (`binfmt_script` nesting).
const MAX_INTERP_DEPTH: usize = 4;
const EM_AARCH64: u16 = 183;
const SIGSEGV: i32 = 11;

struct Launch {
    exe: CString,
    /// `linux-run` options describing the runtime, repeated on exec.
    args: Vec<CString>,
}

static LAUNCH: OnceLock<Launch> = OnceLock::new();

/// Record how this process was started, for re-executing it.
pub fn init(runtime_args: Vec<CString>) {
    if let Ok(exe) = std::env::current_exe() {
        let _ = LAUNCH.set(Launch {
            exe: arg(exe.as_os_str().as_bytes()),
            args: runtime_args,
        });
    }
}

/// The `linux-run` executable, as this process was started.
pub fn launch_exe() -> Option<&'static CString> {
    LAUNCH.get().map(|l| &l.exe)
}

/// The `linux-run` options describing this runtime.
pub fn launch_args() -> Option<&'static [CString]> {
    LAUNCH.get().map(|l| l.args.as_slice())
}

/// Process state carried over exec on `linux-run`'s command line.
#[derive(Default)]
pub struct ExecState {
    /// Guest working directory.
    pub cwd: Option<String>,
    pub sigmask: u64,
    /// Signals whose disposition is SIG_IGN (bit n-1 for signal n).
    pub sigign: u64,
    pub personality: u32,
    /// The process's own mounts (`vfs::own_mounts_text`).
    pub mounts: String,
}

const SIG_SETMASK: u64 = 2;
const SIG_IGN: u64 = 1;

impl ExecState {
    /// Install this state in the new process, before the guest runs.
    pub fn apply(&self) {
        if let Some(cwd) = &self.cwd {
            vfs::set_cwd(cwd.clone());
        }
        let mask = self.sigmask;
        super::signal::rt_sigprocmask([SIG_SETMASK, &mask as *const u64 as u64, 0, 8, 0, 0]);
        for sig in 1..=64u64 {
            if self.sigign & (1 << (sig - 1)) != 0 {
                let act = [SIG_IGN, 0, 0, 0];
                super::signal::rt_sigaction([sig, act.as_ptr() as u64, 0, 8, 0, 0]);
            }
        }
        super::pstate::set_personality(self.personality);
        vfs::load_own_mounts(&self.mounts);
    }

    fn current() -> ExecState {
        let mut mask = 0u64;
        super::signal::rt_sigprocmask([0, 0, &mut mask as *mut u64 as u64, 8, 0, 0]);
        let mut sigign = 0;
        for sig in 1..=64u64 {
            let mut act = [0u64; 4];
            if super::signal::rt_sigaction([sig, 0, act.as_mut_ptr() as u64, 8, 0, 0]) == 0
                && act[0] == SIG_IGN
            {
                sigign |= 1 << (sig - 1);
            }
        }
        ExecState {
            cwd: Some(vfs::cwd()),
            sigmask: mask,
            sigign,
            personality: super::pstate::personality_value(),
            mounts: vfs::own_mounts_text(),
        }
    }
}

/// A NULL-terminated guest array of strings (NULL itself is empty).
///
/// # Safety
/// `p` must be null or point at a readable guest array of string pointers.
unsafe fn guest_strv(p: u64) -> Vec<CString> {
    let mut v = Vec::new();
    if p == 0 {
        return v;
    }
    let mut i = 0;
    loop {
        // SAFETY: caller contract.
        let s = unsafe { ((p + i * 8) as *const u64).read_unaligned() };
        if s == 0 {
            return v;
        }
        // SAFETY: caller contract.
        v.push(unsafe { CStr::from_ptr(s as *const libc::c_char) }.to_owned());
        i += 1;
    }
}

enum Kind {
    Elf,
    /// Interpreter and its optional argument.
    Script(Vec<u8>, Option<Vec<u8>>),
}

fn is_spacetab(c: u8) -> bool {
    c == b' ' || c == b'\t'
}

/// `binfmt_script`'s parse of a `#!` line in the first `BINPRM_BUF_SIZE`
/// bytes (zero-padded).
fn parse_shebang(buf: &[u8; BINPRM_BUF_SIZE]) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
    if &buf[..2] != b"#!" {
        return None;
    }
    let is_term = |c: u8| is_spacetab(c) || c == 0;
    let buf_end = BINPRM_BUF_SIZE - 1;
    let mut end = match buf.iter().position(|&c| c == b'\n') {
        Some(e) => e,
        None => {
            let start = (2..buf_end).find(|&i| !is_spacetab(buf[i]))?;
            // No terminator after the name: the interpreter path is cut off.
            (start..buf_end).find(|&i| is_term(buf[i]))?;
            buf_end
        }
    };
    while end > 2 && is_spacetab(buf[end - 1]) {
        end -= 1;
    }
    let name = (2..end).find(|&i| !is_spacetab(buf[i]))?;
    let sep = (name..end).find(|&i| is_term(buf[i])).unwrap_or(end);
    let arg = if sep < end && buf[sep] != 0 {
        (sep..end)
            .find(|&i| !is_spacetab(buf[i]))
            .map(|a| buf[a..end].to_vec())
    } else {
        None
    };
    Some((buf[name..sep].to_vec(), arg))
}

/// Classify the file at `host` as `execve` would, after the permission
/// checks.
fn classify(host: &CStr) -> Result<Kind, i64> {
    // SAFETY: host path from the resolver; local buffers.
    unsafe {
        let mut st: libc::stat = std::mem::zeroed();
        if libc::stat(host.as_ptr(), &mut st) < 0 {
            return Err(-(errno::last() as i64));
        }
        if st.st_mode & libc::S_IFMT != libc::S_IFREG || libc::access(host.as_ptr(), libc::X_OK) < 0
        {
            return Err(-(EACCES as i64));
        }
        let fd = libc::open(host.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
        if fd < 0 {
            return Err(-(errno::last() as i64));
        }
        let mut buf = [0u8; BINPRM_BUF_SIZE];
        let n = libc::pread(fd, buf.as_mut_ptr().cast(), buf.len(), 0);
        libc::close(fd);
        if n < 0 {
            return Err(-(errno::last() as i64));
        }
        if let Some((interp, arg)) = parse_shebang(&buf) {
            return Ok(Kind::Script(interp, arg));
        }
        let elf = n >= 20
            && buf[..4] == *b"\x7fELF"
            && buf[4] == 2
            && buf[5] == 1
            && u16::from_le_bytes([buf[18], buf[19]]) == EM_AARCH64;
        if elf {
            Ok(Kind::Elf)
        } else {
            Err(-(ENOEXEC as i64))
        }
    }
}

pub fn execve(ctx: &mut GuestContext, a: [u64; 6]) -> i64 {
    exec(ctx, LINUX_AT_FDCWD, a[0], a[1], a[2], 0)
}

pub fn execveat(ctx: &mut GuestContext, a: [u64; 6]) -> i64 {
    exec(ctx, a[0] as i32, a[1], a[2], a[3], a[4])
}

fn exec(ctx: &mut GuestContext, dirfd: i32, path: u64, argv: u64, envp: u64, flags: u64) -> i64 {
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
        return -(EINVAL as i64);
    }
    if path == 0 {
        return -(EFAULT as i64);
    }
    // SAFETY: guest string and string arrays.
    let (path, mut argv, envp) = unsafe {
        (
            guest_cstr(path).to_vec(),
            guest_strv(argv),
            guest_strv(envp),
        )
    };
    // Since Linux 5.18 an empty argv gets an empty argv[0].
    if argv.is_empty() {
        argv.push(CString::default());
    }
    // The filename as Linux records it (bprm->filename): AT_EXECFN, and
    // what a script's interpreter is given.
    let filename: Vec<u8> = if path.is_empty() {
        if flags & AT_EMPTY_PATH == 0 {
            return -(ENOENT as i64);
        }
        format!("/dev/fd/{dirfd}").into_bytes()
    } else if path[0] == b'/' || dirfd == LINUX_AT_FDCWD {
        path.clone()
    } else {
        let mut f = format!("/dev/fd/{dirfd}/").into_bytes();
        f.extend_from_slice(&path);
        f
    };
    let target = match resolve(dirfd, &path, flags) {
        Ok(r) => r,
        Err(e) => return e,
    };
    match interpret(target, &filename, argv) {
        Ok((target, argv)) => in_place(ctx, &target, &argv, &envp, &filename)
            .unwrap_or_else(|| relaunch(&target.guest, &argv, &envp, &filename)),
        Err(e) => e,
    }
}

/// Replace the image in place (see the module docs); None when the process
/// cannot, and execs anew.
fn in_place(
    ctx: &mut GuestContext,
    target: &vfs::Resolved,
    argv: &[CString],
    envp: &[CString],
    execfn: &[u8],
) -> Option<i64> {
    if !context::is_live(ctx)
        || !super::thread::alone()
        || crate::hostcall::used()
        || super::memfd::has_exec_copies()
        || super::ptimer::any()
    {
        return None;
    }
    // What Linux checks before the point of no return: the interpreter.
    let interp = crate::loader::interpreter(&target.host).ok()?;
    if let Some(i) = interp {
        let r = match vfs::resolve(LINUX_AT_FDCWD, i.as_bytes(), true) {
            Ok(r) => r,
            Err(e) => return Some(-(e as i64)),
        };
        if let Err(e) = classify(&r.host) {
            return Some(e);
        }
    }
    let argv: Vec<Vec<u8>> = argv.iter().map(|a| a.as_bytes().to_vec()).collect();
    let envp: Vec<Vec<u8>> = envp.iter().map(|e| e.as_bytes().to_vec()).collect();
    super::fork::spawn::wait_handovers();
    super::fs::close_on_exec();
    super::fdtab::adopt_plain();
    super::binder::exec_reset();
    super::mem::exec_reset();
    crate::diag::exec_reset();
    crate::patch::exec_reset();
    super::signal::exec_reset();
    super::thread::exec_reset();
    super::misc::exec_reset();
    super::cred::exec();
    match crate::load_program(target, &argv, &envp, execfn) {
        Ok((entry, sp)) => {
            ctx.x = [0; 31];
            ctx.v = [0; 32];
            ctx.sp = sp;
            ctx.pc = entry;
            ctx.stub_ret = 0;
            ctx.nzcv = 0;
            ctx.fpcr = 0;
            ctx.fpsr = 0;
            Some(0)
        }
        Err(e) => {
            crate::diag!("[linux-abi] execve {}: {e}", target.guest);
            super::signal::die(SIGSEGV)
        }
    }
}

/// `binfmt_script`: follows `#!` lines from `target` to the ELF program
/// that runs, rewriting `argv` (never empty) as Linux does. `filename` is
/// the name the first file was executed by.
pub fn interpret(
    mut target: vfs::Resolved,
    filename: &[u8],
    mut argv: Vec<CString>,
) -> Result<(vfs::Resolved, Vec<CString>), i64> {
    let mut interp_of = filename.to_vec();
    let mut depth = 0;
    loop {
        match classify(&target.host)? {
            Kind::Elf => return Ok((target, argv)),
            Kind::Script(interp, arg) => {
                if depth == MAX_INTERP_DEPTH {
                    return Err(-(ELOOP as i64));
                }
                depth += 1;
                let mut next = vec![to_cstring(&interp)];
                next.extend(arg.map(|a| to_cstring(&a)));
                next.push(to_cstring(&interp_of));
                next.extend(argv.drain(1..));
                argv = next;
                target = vfs::resolve(LINUX_AT_FDCWD, &interp, true).map_err(|e| -(e as i64))?;
                interp_of = interp;
            }
        }
    }
}

fn to_cstring(b: &[u8]) -> CString {
    // Parsed from NUL-terminated data, so no interior NUL.
    CString::new(b).unwrap_or_default()
}

/// Resolve the program `execveat` names.
fn resolve(dirfd: i32, path: &[u8], flags: u64) -> Result<vfs::Resolved, i64> {
    if path.is_empty() {
        // The fd itself; a translated file stands for its original.
        let guest = super::procfs::fd_guest_path(dirfd).map_err(|e| -(e as i64))?;
        return vfs::resolve(LINUX_AT_FDCWD, guest.as_bytes(), true).map_err(|e| -(e as i64));
    }
    let nofollow = flags & AT_SYMLINK_NOFOLLOW != 0;
    let r = vfs::resolve(dirfd, path, !nofollow).map_err(|e| -(e as i64))?;
    if nofollow {
        // SAFETY: host path, local buffer.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::lstat(r.host.as_ptr(), &mut st) } == 0
            && st.st_mode & libc::S_IFMT == libc::S_IFLNK
        {
            return Err(-(ELOOP as i64));
        }
    }
    Ok(r)
}

fn arg(s: impl AsRef<[u8]>) -> CString {
    to_cstring(s.as_ref())
}

/// Replace this process with `linux-run` running `program`.
fn relaunch(program: &str, argv: &[CString], envp: &[CString], execfn: &[u8]) -> i64 {
    let Some(launch) = LAUNCH.get() else {
        return -(ENOEXEC as i64);
    };
    let state = ExecState::current();
    let mut id = super::cred::current();
    id.exec_transform();
    let mut host = vec![launch.exe.clone()];
    host.extend(launch.args.iter().cloned());
    host.extend([arg("--identity-text"), arg(id.to_text())]);
    if let Some(dir) = super::cred::by_pid_dir() {
        host.extend([arg("--by-pid"), arg(dir.as_os_str().as_bytes())]);
    }
    host.extend([
        arg("--cwd"),
        arg(state.cwd.unwrap_or_default()),
        arg("--sigmask"),
        arg(format!("{:x}", state.sigmask)),
        arg("--sigign"),
        arg(format!("{:x}", state.sigign)),
        arg("--personality"),
        arg(format!("{:x}", state.personality)),
    ]);
    if !state.mounts.is_empty() {
        host.extend([arg("--mounts"), arg(&state.mounts)]);
    }
    host.extend([
        arg("--inherit-env"),
        arg("--exec"),
        arg(execfn),
        arg(program),
    ]);
    host.extend(argv.iter().cloned());
    super::fork::spawn::wait_handovers();
    let mut hargv: Vec<*const libc::c_char> = host.iter().map(|s| s.as_ptr()).collect();
    hargv.push(std::ptr::null());
    let mut henv: Vec<*const libc::c_char> = envp.iter().map(|s| s.as_ptr()).collect();
    henv.push(std::ptr::null());
    // SAFETY: NULL-terminated arrays of C strings that outlive the call.
    unsafe { libc::execve(hargv[0], hargv.as_ptr(), henv.as_ptr()) };
    -(errno::last() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shebang(s: &[u8]) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
        let mut buf = [0u8; BINPRM_BUF_SIZE];
        let n = s.len().min(BINPRM_BUF_SIZE);
        buf[..n].copy_from_slice(&s[..n]);
        parse_shebang(&buf)
    }

    #[test]
    fn shebang_lines_parse_as_binfmt_script() {
        assert_eq!(
            shebang(b"#!/system/bin/sh\necho"),
            Some((b"/system/bin/sh".to_vec(), None))
        );
        assert_eq!(
            shebang(b"#!  /bin/env  -S a b \t\nx"),
            Some((b"/bin/env".to_vec(), Some(b"-S a b".to_vec())))
        );
        assert_eq!(shebang(b"#!/bin/sh"), Some((b"/bin/sh".to_vec(), None)));
        assert_eq!(shebang(b"#!   \n"), None);
        assert_eq!(shebang(b"\x7fELF"), None);
        // An interpreter path that fills the buffer is truncated: ENOEXEC.
        let mut long = b"#!/".to_vec();
        long.extend(std::iter::repeat_n(b'a', 300));
        assert_eq!(shebang(&long), None);
    }
}
