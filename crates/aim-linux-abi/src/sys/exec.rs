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
//! - the signal mask, the ignored dispositions and the interval timers
//!   ([`ExecState`]);
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
    /// The armed interval timers (`itimer::exec_text`).
    pub itimers: String,
    pub regular_fds:String,
    pub close_receipt:Option<i32>,
}

const SIG_SETMASK: u64 = 2;
const SIG_IGN: u64 = 1;

impl ExecState {
    pub fn apply_posix_closes(&self)->Result<(),String>{
        if let Some(fd)=self.close_receipt{super::exec_fd_receipt::consume(fd).map_err(|error|format!("exec descriptor receipt: errno {error}"))?;}Ok(())
    }
    /// Install this state in the new process, before the guest runs.
    pub fn apply(&self) -> Result<(),String> {
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
        vfs::load_own_mounts(&self.mounts).map_err(|error|format!("exec mount namespace: errno {error}"))?;
        super::itimer::exec_restore(&self.itimers);
        super::fdtab::restore_regular_exec(&self.regular_fds).map_err(|error|format!("regular exec inheritance: errno {error}"))?;
        Ok(())
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
            itimers: super::itimer::exec_text(),
            // Descriptor receipts are captured under the final exec admission.
            regular_fds:String::new(),
            close_receipt:None,
        }
    }
}

/// A NULL-terminated guest array of strings (NULL itself is empty).
///
/// # Safety
/// `p` must be null or point at a readable guest array of string pointers.
fn guest_strv(p: u64,budget:&mut usize) -> Result<Vec<CString>,errno::Errno> {
    let mut v = Vec::new();
    if p == 0 {
        return Ok(v);
    }
    let mut i = 0;
    loop {
        // SAFETY: caller contract.
        let at=p.checked_add(i*8).ok_or(EFAULT)?;
        let bytes=super::user_memory::read_exact(at,8)?;let s=u64::from_le_bytes(bytes.try_into().unwrap());
        if s == 0 {
            return Ok(v);
        }
        // SAFETY: caller contract.
        let bytes=super::user_memory::read_cstr(s,32*super::mem::PAGE as usize).map_err(|error|if error==36{errno::E2BIG}else{error})?;
        *budget=budget.checked_sub(bytes.len()+1+8).ok_or(errno::E2BIG)?;
        v.push(CString::new(bytes).map_err(|_|EFAULT)?);
        if v.len()>0x7fff_ffff{return Err(errno::E2BIG);}
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
        let source=super::verified_source::VerifiedSource::open(host,true).map_err(|error|-(error as i64))?;
        let mut buf = [0u8; BINPRM_BUF_SIZE];
        let n=source.read_at(&mut buf,0).map_err(|error|-(error as i64))? as i64;
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

fn trace_stage(stage: &str) {
    if super::tracing() {
        // SAFETY: getpid reads this process's host identity.
        let pid = unsafe { libc::getpid() };
        crate::diag!("[linux-exec] pid={pid} tid={} stage={stage}", super::host_tid());
    }
}

fn exec(ctx: &mut GuestContext, dirfd: i32, path: u64, argv: u64, envp: u64, flags: u64) -> i64 {
    trace_stage("arguments.enter");
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 {
        return -(EINVAL as i64);
    }
    if path == 0 {
        return -(EFAULT as i64);
    }
    // SAFETY: guest string and string arrays.
    let path=match guest_cstr(path){Ok(path)=>path,Err(error)=>return -(error as i64)};
    let mut stack:libc::rlimit=unsafe{std::mem::zeroed()};if unsafe{libc::getrlimit(libc::RLIMIT_STACK,&mut stack)}<0{return -(errno::last()as i64);}
    let mut budget=(stack.rlim_cur/4).min(6<<20).max(32*super::mem::PAGE)as usize;
    let mut argv=match guest_strv(argv,&mut budget){Ok(argv)=>argv,Err(error)=>return -(error as i64)};
    let envp=match guest_strv(envp,&mut budget){Ok(envp)=>envp,Err(error)=>return -(error as i64)};
    // Since Linux 5.18 an empty argv gets an empty argv[0].
    if argv.is_empty() {
        argv.push(CString::default());
    }
    trace_stage("arguments.leave");
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
    trace_stage("resolve.enter");
    let target = match resolve(dirfd, &path, flags) {
        Ok(r) => r,
        Err(e) => return e,
    };
    trace_stage("resolve.leave");
    trace_stage("interpret.enter");
    match interpret(target, &filename, argv) {
        Ok((target, argv)) => {
            trace_stage("interpret.leave");
            in_place(ctx, &target, &argv, &envp, &filename)
                .unwrap_or_else(|| relaunch(&target.guest, &argv, &envp, &filename))
        }
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
    trace_stage("in-place.eligibility");
    if !context::is_live(ctx)
        || !super::thread::alone()
        || crate::hostcall::used()
        || super::memfd::has_exec_copies()
        || super::ptimer::any()
    {
        return None;
    }
    trace_stage("in-place.interpreter");
    // What Linux checks before the point of no return: the interpreter.
    let interp=match crate::loader::interpreter_errno(&target.host){Ok(interp)=>interp,Err(error)=>return Some(-(error as i64))};
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
    trace_stage("in-place.handovers.enter");
    super::fork::spawn::wait_handovers();
    trace_stage("in-place.close-on-exec");
    if let Err(error)=super::fs::close_on_exec(){crate::diag!("[linux-abi] exec descriptor close: errno {error}");super::signal::die(SIGSEGV)}
    trace_stage("in-place.reset");
    super::fdtab::adopt_plain();
    super::binder::exec_reset();
    super::mem::exec_reset();
    crate::diag::exec_reset();
    crate::patch::exec_reset();
    super::signal::exec_reset();
    super::thread::exec_reset();
    super::misc::exec_reset();
    super::cred::exec();
    trace_stage("in-place.load");
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
    trace_stage("relaunch.state.enter");
    let state = ExecState::current();
    trace_stage("relaunch.state.leave");
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
    if !state.itimers.is_empty() {
        host.extend([arg("--itimers"), arg(&state.itimers)]);
    }
    host.extend([
        arg("--inherit-env"),
        arg("--exec"),
        arg(execfn),
        arg(program),
    ]);
    host.extend(argv.iter().cloned());
    trace_stage("relaunch.handovers.enter");
    super::fork::spawn::wait_handovers();
    trace_stage("relaunch.receipt.prepare");
    let mut receipt=match super::exec_fd_receipt::Receipt::prepare(){Ok(receipt)=>receipt,Err(error)=>return -(error as i64)};
    trace_stage("relaunch.lifecycle.enter");
    let guard=super::fdtab::lifecycle();
    trace_stage("relaunch.receipt.capture");
    if let Some(receipt)=&mut receipt{if let Err(error)=receipt.capture(){drop(guard);return -(error as i64);}}
    trace_stage("relaunch.descriptors");
    let mut descriptors=Vec::new();
    for fd in super::fd_visibility::visible(){let flags=unsafe{libc::fcntl(fd,libc::F_GETFD)};if flags<0{let error=errno::last();drop(guard);return -(error as i64);}if flags&libc::FD_CLOEXEC==0{descriptors.push(fd.to_string());}}
    let mut options=vec![arg("--guest-fds"),arg(descriptors.join(",")),arg("--socket-receipts"),arg(super::fdtab::socket_exec_text())];
    trace_stage("relaunch.regular-fds");
    let regular=match super::fdtab::regular_exec_text(){Ok(text)=>text,Err(error)=>{drop(guard);return -(error as i64);}};if !regular.is_empty(){options.extend([arg("--regular-fds"),arg(regular)]);}
    if let Some(receipt)=&receipt{options.extend([arg("--exec-close-receipt"),arg(receipt.fd().to_string())]);}
    host.splice(1..1,options);
    let mut hargv: Vec<*const libc::c_char> = host.iter().map(|s| s.as_ptr()).collect();
    hargv.push(std::ptr::null());
    let mut henv: Vec<*const libc::c_char> = envp.iter().map(|s| s.as_ptr()).collect();
    henv.push(std::ptr::null());
    // SAFETY: NULL-terminated arrays of C strings that outlive the call.
    trace_stage("relaunch.writer-flags");
    let writer_flags=match super::fdtab::prepare_regular_exec(){Ok(flags)=>flags,Err(error)=>{drop(guard);return -(error as i64)}};
    trace_stage("relaunch.host-exec");
    unsafe { libc::execve(hargv[0], hargv.as_ptr(), henv.as_ptr()) };
    let error=errno::last();drop(guard);drop(writer_flags);drop(receipt);-(error as i64)
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
