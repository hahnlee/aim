//! Programs aimctl starts, each responsible for its own macOS privacy
//! requests (`aim_process::spawn`): `std::process::Command` cannot
//! disclaim, and linux-run then executes itself again to (#3014).

use std::ffi::{CString, OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

/// A program, its arguments and its environment.
#[derive(Debug)]
pub struct Spec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// The whole environment; None: aimctl's own.
    pub env: Option<Vec<(OsString, OsString)>>,
}

impl Spec {
    pub fn new(program: impl AsRef<Path>) -> Spec {
        Spec {
            program: program.as_ref().to_path_buf(),
            args: Vec::new(),
            env: None,
        }
    }

    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Spec {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    pub fn args<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(&mut self, args: I) -> &mut Spec {
        for arg in args {
            self.arg(arg);
        }
        self
    }

    /// Set `key` in the environment.
    pub fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Spec {
        let env = self
            .env
            .get_or_insert_with(|| std::env::vars_os().collect());
        let key = key.as_ref().to_os_string();
        env.retain(|(k, _)| *k != key);
        env.push((key, value.as_ref().to_os_string()));
        self
    }
}

/// What a standard descriptor of the child is.
pub enum Stdio {
    Inherit,
    Null,
    /// A pipe whose other end the caller gets.
    Piped,
    /// A descriptor of the caller's.
    #[cfg(test)]
    Fd(OwnedFd),
}

/// A spawned child, reaped by [`Child::wait`] or [`Child::try_wait`].
pub struct Child {
    pid: libc::pid_t,
    status: Option<ExitStatus>,
}

/// The child and the parent's ends of its piped stdin, stdout and stderr.
pub type Spawned = (Child, [Option<File>; 3]);

fn cstring(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
}

fn pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: a two-element buffer; both ends are close-on-exec and owned
    // below.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let ends = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    for fd in [&ends.0, &ends.1] {
        // SAFETY: setting close-on-exec on our own descriptor.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(ends)
}

pub fn spawn(spec: &Spec, stdio: [Stdio; 3]) -> io::Result<Spawned> {
    let program = cstring(spec.program.as_os_str().as_bytes())?;
    let argv = std::iter::once(program.clone())
        .chain(
            spec.args
                .iter()
                .map(|arg| cstring(arg.as_bytes()))
                .collect::<io::Result<Vec<_>>>()?,
        )
        .collect::<Vec<_>>();
    let env = match &spec.env {
        Some(env) => env.clone(),
        None => std::env::vars_os().collect(),
    };
    let envp = env
        .iter()
        .map(|(k, v)| cstring(&[k.as_bytes(), b"=", v.as_bytes()].concat()))
        .collect::<io::Result<Vec<_>>>()?;
    // The child's ends stay open until the spawn returns.
    let mut child_ends: Vec<OwnedFd> = Vec::new();
    let mut parent_ends: [Option<File>; 3] = [None, None, None];
    let mut fds: [Option<RawFd>; 3] = [None; 3];
    for (n, io) in stdio.into_iter().enumerate() {
        let end = match io {
            Stdio::Inherit => continue,
            Stdio::Null => File::options()
                .read(n == 0)
                .write(n != 0)
                .open("/dev/null")?
                .into(),
            Stdio::Piped => {
                let (read, write) = pipe()?;
                let (child, parent) = if n == 0 { (read, write) } else { (write, read) };
                parent_ends[n] = Some(File::from(parent));
                child
            }
            #[cfg(test)]
            Stdio::Fd(fd) => fd,
        };
        fds[n] = Some(end.as_raw_fd());
        child_ends.push(end);
    }
    let pid = aim_process::spawn(&program, &argv, &envp, fds)?;
    Ok((Child { pid, status: None }, parent_ends))
}

impl Child {
    pub fn id(&self) -> u32 {
        self.pid as u32
    }

    fn reap(&mut self, options: libc::c_int) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            return Ok(Some(status));
        }
        let mut raw = 0;
        loop {
            // SAFETY: waiting for our own child.
            let n = unsafe { libc::waitpid(self.pid, &mut raw, options) };
            if n < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if n == 0 {
                return Ok(None);
            }
            self.status = Some(ExitStatus::from_raw(raw));
            return Ok(self.status);
        }
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.reap(libc::WNOHANG)
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.reap(0)
            .map(|status| status.expect("waitpid without WNOHANG"))
    }

    /// SIGKILL, unless the child has been reaped.
    pub fn kill(&mut self) -> io::Result<()> {
        if self.status.is_some() {
            return Ok(());
        }
        // SAFETY: signalling our own unreaped child.
        if unsafe { libc::kill(self.pid, libc::SIGKILL) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
