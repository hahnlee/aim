//! Service process launch: each service is a host process running
//! `linux-run --root <image> ... <program> <args>`.
//!
//! init's child-side setup (`Service::Start` after `fork`) splits in two:
//! descriptors (`socket`, `file`) are created here and passed at fixed fd
//! numbers with their `ANDROID_SOCKET_*` / `ANDROID_FILE_*` variables;
//! credentials, priority and limits go into the identity file the syscall
//! layer reports from.

use std::collections::BTreeSet;
use std::ffi::CString;
use std::fs;
use std::io::{self, Write as _};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use darwin_android_init::rc::{FileMode, SocketType};

use crate::identity::Identity;
use crate::paths::Layout;

/// The first fd a service's descriptors are passed at.
pub const FIRST_DESCRIPTOR_FD: i32 = 3;

/// One `socket` line, ready to create.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SocketSpec {
    pub name: String,
    pub socket_type: SocketType,
    pub passcred: bool,
    pub listen: bool,
    pub perm: u32,
    pub uid: u32,
    pub gid: u32,
    /// Host path of `/dev/socket/<name>`.
    pub host_path: PathBuf,
    /// `ANDROID_SOCKET_<name>` with non-alphanumerics as `_`.
    pub env_name: String,
    /// The fd number the child receives it at.
    pub fd: i32,
}

/// One `file` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSpec {
    pub guest: String,
    pub host: PathBuf,
    pub mode: FileMode,
    pub env_name: String,
    pub fd: i32,
}

/// Everything needed to start one service process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchSpec {
    pub service: String,
    /// Starts of this service so far, including this one.
    pub generation: u64,
    /// Guest argv after property expansion; `argv[0]` is the program.
    pub argv: Vec<String>,
    /// Guest environment: init's (`PATH` plus `export`s), the service's
    /// `setenv`s, then the descriptor variables.
    pub env: Vec<(String, String)>,
    pub sockets: Vec<SocketSpec>,
    pub files: Vec<FileSpec>,
    pub identity: Identity,
    pub identity_file: PathBuf,
    pub log_file: PathBuf,
}

/// `Descriptor::Publish`: the key plus the name, non-alphanumerics as `_`.
pub fn descriptor_env_name(prefix: &str, name: &str) -> String {
    format!("{prefix}{name}")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinuxRunOptions {
    pub path_map: bool,
    pub identity: bool,
    pub inherit_env: bool,
    pub binder: bool,
    pub seclabel: bool,
}

impl LinuxRunOptions {
    /// Everything `docs/guest-init-contract.md` specifies.
    pub const CONTRACT: Self = Self {
        path_map: true,
        identity: true,
        inherit_env: true,
        binder: true,
        seclabel: true,
    };

    /// Which contract options a `linux-run` binary accepts, from its usage
    /// text.
    pub fn detect(binary: &Path) -> Self {
        let Ok(output) = Command::new(binary).arg("--help").output() else {
            return Self::default();
        };
        let usage = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        Self {
            path_map: usage.contains("--path-map"),
            identity: usage.contains("--identity"),
            inherit_env: usage.contains("--inherit-env"),
            binder: usage.contains("--binder"),
            seclabel: usage.contains("--seclabel"),
        }
    }

    pub fn missing(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if !self.path_map {
            out.push("--path-map");
        }
        if !self.identity {
            out.push("--identity");
        }
        if !self.inherit_env {
            out.push("--inherit-env");
        }
        if !self.binder {
            out.push("--binder");
        }
        if !self.seclabel {
            out.push("--seclabel");
        }
        out
    }
}

/// How to invoke `linux-run`.
#[derive(Clone, Debug)]
pub struct LinuxRun {
    pub binary: PathBuf,
    pub image: PathBuf,
    pub path_map_file: PathBuf,
    /// The bootstrap name of the binder host serving the guest's binder
    /// devices (docs/guest-init-contract.md, section 1).
    pub binder: Option<String>,
    pub trace: bool,
    pub options: LinuxRunOptions,
}

impl LinuxRun {
    /// The host command line (argv of the host process).
    pub fn command_line(&self, spec: &LaunchSpec) -> Vec<String> {
        let mut out = vec![
            self.binary.display().to_string(),
            "--root".to_string(),
            self.image.display().to_string(),
        ];
        if self.options.path_map {
            out.push("--path-map".to_string());
            out.push(self.path_map_file.display().to_string());
        }
        if self.options.identity {
            out.push("--identity".to_string());
            out.push(spec.identity_file.display().to_string());
        }
        if self.options.inherit_env {
            out.push("--inherit-env".to_string());
        }
        if self.options.binder
            && let Some(name) = &self.binder
        {
            out.push("--binder".to_string());
            out.push(name.clone());
        }
        // An empty label is one init would compute from the executable's
        // file context, which needs the policy; the layer then reports none.
        if self.options.seclabel && !spec.identity.seclabel.is_empty() {
            out.push("--seclabel".to_string());
            out.push(spec.identity.seclabel.clone());
        }
        if self.trace {
            out.push("--trace".to_string());
        }
        out.extend(spec.argv.iter().cloned());
        out
    }
}

/// How a service process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    Signal(i32),
}

impl Exit {
    /// `si_code == CLD_EXITED && si_status == 0`.
    pub fn is_success(self) -> bool {
        self == Exit::Code(0)
    }

    pub fn from_wait_status(status: i32) -> Self {
        if libc::WIFSIGNALED(status) {
            Exit::Signal(libc::WTERMSIG(status))
        } else {
            Exit::Code(libc::WEXITSTATUS(status))
        }
    }
}

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Exit::Code(code) => write!(f, "exited with status {code}"),
            Exit::Signal(signal) => write!(f, "killed by signal {signal}"),
        }
    }
}

/// Starts and stops service processes.
pub trait Launcher {
    /// Starts the process; returns its pid.
    fn launch(&mut self, spec: &LaunchSpec) -> Result<u32, String>;
    /// `KillProcessGroup`.
    fn kill_group(&mut self, pid: u32, signal: i32);
    /// Processes that exited since the last call.
    fn reap(&mut self) -> Vec<(u32, Exit)>;
}

/// Records launches; pids are synthetic and nothing exits on its own.
#[derive(Debug)]
pub struct DryRunLauncher {
    pub linux_run: LinuxRun,
    pub launches: Vec<(u32, LaunchSpec)>,
    pub kills: Vec<(u32, i32)>,
    next_pid: u32,
    live: BTreeSet<u32>,
    exited: Vec<(u32, Exit)>,
}

impl DryRunLauncher {
    pub fn new(linux_run: LinuxRun) -> Self {
        Self {
            linux_run,
            launches: Vec::new(),
            kills: Vec::new(),
            next_pid: 1000,
            live: BTreeSet::new(),
            exited: Vec::new(),
        }
    }

    /// Makes `pid` exit at the next [`Launcher::reap`].
    pub fn exit(&mut self, pid: u32, exit: Exit) {
        if self.live.remove(&pid) {
            self.exited.push((pid, exit));
        }
    }

    /// The launch report printed by `guest-init --dry-run`.
    pub fn describe(&self, pid: u32, spec: &LaunchSpec) -> String {
        describe_launch(&self.linux_run, pid, spec)
    }
}

impl Launcher for DryRunLauncher {
    fn launch(&mut self, spec: &LaunchSpec) -> Result<u32, String> {
        self.next_pid += 1;
        self.live.insert(self.next_pid);
        self.launches.push((self.next_pid, spec.clone()));
        Ok(self.next_pid)
    }

    fn kill_group(&mut self, pid: u32, signal: i32) {
        self.kills.push((pid, signal));
        if signal == libc::SIGKILL || signal == libc::SIGTERM {
            self.exit(pid, Exit::Signal(signal));
        }
    }

    fn reap(&mut self) -> Vec<(u32, Exit)> {
        std::mem::take(&mut self.exited)
    }
}

/// Multi-line description of a launch.
pub fn describe_launch(linux_run: &LinuxRun, pid: u32, spec: &LaunchSpec) -> String {
    let mut out = format!(
        "launch {} [pid {pid}] uid={} gid={} groups={:?} caps(eff)={:#x}\n",
        spec.service,
        spec.identity.uid,
        spec.identity.gid,
        spec.identity.groups,
        spec.identity.capabilities.effective
    );
    out.push_str(&format!(
        "  exec: {}\n",
        shell_join(&linux_run.command_line(spec))
    ));
    let env: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    out.push_str(&format!("  env: {}\n", env.join(" ")));
    for socket in &spec.sockets {
        out.push_str(&format!(
            "  fd {}: socket /dev/socket/{} {:?}{}{} {:o} {}:{}\n",
            socket.fd,
            socket.name,
            socket.socket_type,
            if socket.passcred { "+passcred" } else { "" },
            if socket.listen { "+listen" } else { "" },
            socket.perm,
            socket.uid,
            socket.gid
        ));
    }
    for file in &spec.files {
        out.push_str(&format!(
            "  fd {}: file {} {:?}\n",
            file.fd, file.guest, file.mode
        ));
    }
    out
}

fn shell_join(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.is_empty() || a.contains(|c: char| c.is_whitespace() || "'\"$\\".contains(c)) {
                format!("'{}'", a.replace('\'', "'\\''"))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Launches real processes with `posix_spawn`.
pub struct HostLauncher {
    pub linux_run: LinuxRun,
    layout: Layout,
    children: BTreeSet<u32>,
}

impl HostLauncher {
    pub fn new(linux_run: LinuxRun, layout: Layout) -> Self {
        Self {
            linux_run,
            layout,
            children: BTreeSet::new(),
        }
    }

    pub fn children(&self) -> impl Iterator<Item = u32> + '_ {
        self.children.iter().copied()
    }
}

/// `CreateSocket` (system/core/init/util.cpp) on the host: an AF_UNIX
/// socket bound at the mapped `/dev/socket/<name>`, chmod'ed to `perm`,
/// listening when `+listen` was given. Darwin has no `SOCK_SEQPACKET` for
/// AF_UNIX; such sockets are created as `SOCK_STREAM` and the guest type is
/// recorded in the sockets table for the syscall layer.
pub fn create_socket(spec: &SocketSpec) -> Result<(OwnedFd, i32), String> {
    let host_type = match spec.socket_type {
        SocketType::Stream => libc::SOCK_STREAM,
        SocketType::Dgram => libc::SOCK_DGRAM,
        SocketType::Seqpacket => libc::SOCK_SEQPACKET,
    };
    let dir = spec
        .host_path
        .parent()
        .ok_or_else(|| format!("{}: no parent directory", spec.host_path.display()))?;
    let name = spec
        .host_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{}: bad socket name", spec.host_path.display()))?;
    let (fd, created_type) = match crate::unixsock::bind_at(dir, name, host_type) {
        Ok(fd) => (fd, host_type),
        Err(_) if host_type == libc::SOCK_SEQPACKET => (
            crate::unixsock::bind_at(dir, name, libc::SOCK_STREAM)
                .map_err(|e| format!("bind {}: {e}", spec.host_path.display()))?,
            libc::SOCK_STREAM,
        ),
        Err(e) => return Err(format!("bind {}: {e}", spec.host_path.display())),
    };
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&spec.host_path, fs::Permissions::from_mode(spec.perm))
        .map_err(|e| format!("chmod {}: {e}", spec.host_path.display()))?;
    // SAFETY: listen on our socket.
    if spec.listen && unsafe { libc::listen(fd.as_raw_fd(), libc::SOMAXCONN) } != 0 {
        return Err(format!("listen: {}", io::Error::last_os_error()));
    }
    Ok((fd, created_type))
}

fn open_file(spec: &FileSpec) -> Result<OwnedFd, String> {
    let flags = match spec.mode {
        FileMode::Read => libc::O_RDONLY,
        FileMode::Write => libc::O_WRONLY,
        FileMode::ReadWrite => libc::O_RDWR,
    };
    let c_path = CString::new(spec.host.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: open of a host path.
    let fd = unsafe { libc::open(c_path.as_ptr(), flags | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(format!(
            "{} ({}): {}",
            spec.guest,
            spec.host.display(),
            io::Error::last_os_error()
        ));
    }
    // SAFETY: fresh fd.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn socket_type_name(socket_type: SocketType) -> &'static str {
    match socket_type {
        SocketType::Stream => "stream",
        SocketType::Dgram => "dgram",
        SocketType::Seqpacket => "seqpacket",
    }
}

fn append(path: &Path, line: &str) {
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

impl Launcher for HostLauncher {
    fn launch(&mut self, spec: &LaunchSpec) -> Result<u32, String> {
        fs::write(&spec.identity_file, spec.identity.to_file_text())
            .map_err(|e| format!("{}: {e}", spec.identity_file.display()))?;
        let mut passed: Vec<(OwnedFd, i32)> = Vec::new();
        for socket in &spec.sockets {
            let (fd, host_type) = create_socket(socket)?;
            append(
                &self.layout.sockets_file(),
                &format!(
                    "{}\t/dev/socket/{}\t{}\t{}\t{}\t{}\n",
                    spec.service,
                    socket.name,
                    socket_type_name(socket.socket_type),
                    if host_type == libc::SOCK_STREAM {
                        "stream"
                    } else if host_type == libc::SOCK_DGRAM {
                        "dgram"
                    } else {
                        "seqpacket"
                    },
                    if socket.passcred { "passcred" } else { "-" },
                    if socket.listen { "listen" } else { "-" },
                ),
            );
            append(
                &self.layout.fs_attrs_file(),
                &format!(
                    "/dev/socket/{}\t{}\t{}\t{:o}\n",
                    socket.name, socket.uid, socket.gid, socket.perm
                ),
            );
            passed.push((fd, socket.fd));
        }
        // A file that cannot be opened is not published (init logs and
        // continues without its ANDROID_FILE_ variable).
        let mut env = spec.env.clone();
        for file in &spec.files {
            match open_file(file) {
                Ok(fd) => passed.push((fd, file.fd)),
                Err(error) => {
                    env.retain(|(key, _)| *key != file.env_name);
                    append(
                        &spec.log_file,
                        &format!("[guest-init] could not open file descriptor: {error}\n"),
                    );
                }
            }
        }
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&spec.log_file)
            .map_err(|e| format!("{}: {e}", spec.log_file.display()))?;
        let argv = self.linux_run.command_line(spec);
        let pid = spawn(&argv, &env, &passed, log.as_raw_fd())?;
        self.children.insert(pid);
        let _ = std::os::unix::fs::symlink(
            &spec.identity_file,
            self.layout
                .identity_dir()
                .join("by-pid")
                .join(pid.to_string()),
        );
        Ok(pid)
    }

    fn kill_group(&mut self, pid: u32, signal: i32) {
        // SAFETY: signalling our own child's process group (or the child).
        unsafe {
            if libc::kill(-(pid as i32), signal) != 0 {
                libc::kill(pid as i32, signal);
            }
        }
    }

    fn reap(&mut self) -> Vec<(u32, Exit)> {
        let mut out = Vec::new();
        for pid in self.children.clone() {
            let mut status = 0;
            // SAFETY: waitpid on our own child.
            let r = unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) };
            if r == pid as i32 {
                self.children.remove(&pid);
                let _ = fs::remove_file(
                    self.layout
                        .identity_dir()
                        .join("by-pid")
                        .join(pid.to_string()),
                );
                out.push((pid, Exit::from_wait_status(status)));
            } else if r < 0 {
                self.children.remove(&pid);
            }
        }
        out
    }
}

impl Drop for HostLauncher {
    /// Services do not outlive their init.
    fn drop(&mut self) {
        for pid in std::mem::take(&mut self.children) {
            self.kill_group(pid, libc::SIGKILL);
            let mut status = 0;
            // SAFETY: reaping our own child.
            unsafe { libc::waitpid(pid as i32, &mut status, 0) };
        }
    }
}

/// `posix_spawn` with only the listed descriptors inherited
/// (`POSIX_SPAWN_CLOEXEC_DEFAULT`), stdin from `/dev/null`, stdout and
/// stderr to `log_fd`, in a new process group.
fn spawn(
    argv: &[String],
    env: &[(String, String)],
    passed: &[(OwnedFd, i32)],
    log_fd: i32,
) -> Result<u32, String> {
    let c_argv: Vec<CString> = argv
        .iter()
        .map(|a| CString::new(a.as_str()).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let c_env: Vec<CString> = env
        .iter()
        .map(|(k, v)| CString::new(format!("{k}={v}")).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    let mut argv_ptrs: Vec<*mut libc::c_char> =
        c_argv.iter().map(|a| a.as_ptr() as *mut _).collect();
    argv_ptrs.push(std::ptr::null_mut());
    let mut env_ptrs: Vec<*mut libc::c_char> = c_env.iter().map(|a| a.as_ptr() as *mut _).collect();
    env_ptrs.push(std::ptr::null_mut());
    let dev_null = c"/dev/null";
    // SAFETY: posix_spawn with initialized attribute and file action objects.
    unsafe {
        let mut actions: libc::posix_spawn_file_actions_t = std::mem::zeroed();
        let mut attr: libc::posix_spawnattr_t = std::mem::zeroed();
        libc::posix_spawn_file_actions_init(&mut actions);
        libc::posix_spawnattr_init(&mut attr);
        libc::posix_spawnattr_setflags(
            &mut attr,
            (libc::POSIX_SPAWN_SETPGROUP | libc::POSIX_SPAWN_CLOEXEC_DEFAULT) as libc::c_short,
        );
        libc::posix_spawnattr_setpgroup(&mut attr, 0);
        libc::posix_spawn_file_actions_addopen(
            &mut actions,
            0,
            dev_null.as_ptr(),
            libc::O_RDONLY,
            0,
        );
        libc::posix_spawn_file_actions_adddup2(&mut actions, log_fd, 1);
        libc::posix_spawn_file_actions_adddup2(&mut actions, log_fd, 2);
        // Sources may collide with targets: move every source above the
        // target range first.
        let mut staged = Vec::new();
        for (fd, target) in passed {
            let high = libc::fcntl(fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 256);
            if high < 0 {
                return Err(format!("F_DUPFD: {}", io::Error::last_os_error()));
            }
            staged.push(OwnedFd::from_raw_fd(high));
            libc::posix_spawn_file_actions_adddup2(&mut actions, high, *target);
        }
        let mut pid: libc::pid_t = 0;
        let r = libc::posix_spawn(
            &mut pid,
            c_argv[0].as_ptr(),
            &actions,
            &attr,
            argv_ptrs.as_ptr(),
            env_ptrs.as_ptr(),
        );
        libc::posix_spawn_file_actions_destroy(&mut actions);
        libc::posix_spawnattr_destroy(&mut attr);
        drop(staged);
        if r != 0 {
            return Err(format!(
                "posix_spawn {}: {}",
                argv[0],
                io::Error::from_raw_os_error(r)
            ));
        }
        Ok(pid as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_names_follow_init() {
        assert_eq!(
            descriptor_env_name("ANDROID_SOCKET_", "ot-daemon"),
            "ANDROID_SOCKET_ot_daemon"
        );
        assert_eq!(
            descriptor_env_name("ANDROID_FILE_", "/dev/kmsg"),
            "ANDROID_FILE__dev_kmsg"
        );
    }
}
