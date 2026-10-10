//! Native PTY pair ownership and endpoint leases (#1259).
mod carrier;
mod session;
pub mod status;
pub mod transport;
use crate::private_fd::PrivateFd;
use std::{
    ffi::{CStr, CString},
    fs, io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
    path::Path,
    sync::Mutex,
};

fn failure(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code)
}
fn readable(fd: i32) -> io::Result<bool> {
    let mut poll = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    if unsafe { libc::poll(&mut poll, 1, 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if poll.revents & libc::POLLNVAL != 0 {
        return Err(failure(libc::EBADF));
    }
    Ok(poll.revents & libc::POLLIN != 0)
}
fn descriptor(create: impl FnOnce() -> i32) -> io::Result<PrivateFd> {
    PrivateFd::allocate(|| {
        let fd = create();
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        }
    })
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Master,
    Slave,
}
impl Side {
    fn name(self) -> &'static str {
        match self {
            Self::Master => "master",
            Self::Slave => "slave",
        }
    }
}
struct Directory {
    parent: PrivateFd,
    fd: PrivateFd,
    name: CString,
}
impl Drop for Directory {
    fn drop(&mut self) {
        for name in [c"master", c"slave"] {
            if unsafe { libc::unlinkat(self.fd.as_raw_fd(), name.as_ptr(), 0) } < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ENOENT) {
                    eprintln!("PTY lease cleanup: {error}");
                }
            }
        }
        if unsafe {
            libc::unlinkat(
                self.parent.as_raw_fd(),
                self.name.as_ptr(),
                libc::AT_REMOVEDIR,
            )
        } < 0
        {
            eprintln!(
                "PTY owner directory cleanup: {}",
                io::Error::last_os_error()
            );
        }
    }
}
/// The backing closes before its shared open-description lease.
pub struct Endpoint {
    backing: PrivateFd,
    lease: PrivateFd,
    pub side: Side,
}
impl Endpoint {
    pub fn descriptor(&self) -> BorrowedFd<'_> {
        self.backing.as_fd()
    }
    pub fn lease(&self) -> BorrowedFd<'_> {
        self.lease.as_fd()
    }
    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self {
            backing: self.backing.try_clone()?,
            lease: self.lease.try_clone()?,
            side: self.side,
        })
    }
}
struct State {
    master: Option<PrivateFd>,
    slave: Option<PrivateFd>,
    master_opened: bool,
    slave_opened: bool,
    slave_closed: bool,
    termios: libc::termios,
    slave_locked:bool,
}
pub struct Pair {
    state: Mutex<State>,
    slave_name: Vec<u8>,
    directory: Directory,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Readiness {
    pub readable: bool,
    pub hangup: bool,
}
impl Pair {
    pub fn slave_path(&self)->&[u8]{&self.slave_name[..self.slave_name.len()-1]}
    pub fn allocate(runtime: &Path) -> io::Result<Self> {
        let mut nonce = [0u8; 16];
        if unsafe { libc::getentropy(nonce.as_mut_ptr().cast(), nonce.len()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if !runtime.is_absolute() || fs::canonicalize(runtime)? != runtime {
            return Err(failure(libc::EINVAL));
        }
        use std::os::unix::ffi::OsStrExt;
        let path =
            CString::new(runtime.as_os_str().as_bytes()).map_err(|_| failure(libc::EINVAL))?;
        let parent = descriptor(|| unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        })?;
        let mut metadata: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(parent.as_raw_fd(), &mut metadata) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if metadata.st_mode & 0o077 != 0 || metadata.st_uid != unsafe { libc::geteuid() } {
            return Err(failure(libc::EPERM));
        }
        let name = CString::new(
            nonce
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        )
        .unwrap();
        if unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = match descriptor(|| unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        }) {
            Ok(fd) => fd,
            Err(error) => {
                if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) }
                    < 0
                {
                    eprintln!("PTY allocation cleanup: {}", io::Error::last_os_error());
                }
                return Err(error);
            }
        };
        let directory = Directory { parent, fd, name };
        for side in [Side::Master, Side::Slave] {
            let name = CString::new(side.name()).unwrap();
            let _ = descriptor(|| unsafe {
                libc::openat(
                    directory.fd.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDWR
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            })?;
        }
        let master = descriptor(|| unsafe {
            libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC)
        })?;
        if unsafe { libc::grantpt(master.as_raw_fd()) } < 0
            || unsafe { libc::unlockpt(master.as_raw_fd()) } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut name = [0i8; 128];
        if unsafe {
            libc::ioctl(
                master.as_raw_fd(),
                0x40807453u64 as libc::c_ulong,
                name.as_mut_ptr(),
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let slave_name = unsafe { CStr::from_ptr(name.as_ptr()) }
            .to_bytes_with_nul()
            .to_vec();
        let slave = descriptor(|| unsafe {
            libc::open(
                slave_name.as_ptr().cast(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        })?;
        let mut termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut termios) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            directory,
            slave_name,
            state: Mutex::new(State {
                master: Some(master),
                slave: Some(slave),
                master_opened: false,
                slave_opened: false,
                slave_closed: false,
                termios,
                slave_locked:false,
            }),
        })
    }
    fn lease_file(&self, side: Side) -> io::Result<PrivateFd> {
        let name = CString::new(side.name()).unwrap();
        descriptor(|| unsafe {
            libc::openat(
                self.directory.fd.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        })
    }
    fn no_endpoints(&self, side: Side) -> io::Result<bool> {
        let probe = self.lease_file(side)?;
        if unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Ok(false)
        } else {
            Err(error)
        }
    }
    pub fn acquire(&self, side: Side) -> io::Result<Endpoint> {
        let mut state = self.state.lock().unwrap();
        if state.master.is_none() {
            return Err(failure(libc::EIO));
        }
        if side==Side::Slave&&state.slave_locked{return Err(failure(libc::EIO));}
        let lease = self.lease_file(side)?;
        if unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let backing = match side {
            Side::Master => {
                state.master_opened = true;
                state.master.as_ref().unwrap().try_clone()?
            }
            Side::Slave => {
                if state.slave.is_none() {
                    let slave = descriptor(|| unsafe {
                        libc::open(
                            self.slave_name.as_ptr().cast(),
                            libc::O_RDWR | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC,
                        )
                    })?;
                    if unsafe { libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &state.termios) }
                        < 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                    state.slave = Some(slave);
                }
                state.slave_opened = true;
                state.slave_closed = false;
                state.slave.as_ref().unwrap().try_clone()?
            }
        };
        Ok(Endpoint {
            backing,
            lease,
            side,
        })
    }
    fn refresh_locked(&self, state: &mut State) -> io::Result<()> {
        if state.master_opened && self.no_endpoints(Side::Master)? {
            state.master.take();
            state.slave.take();
            state.slave_closed = true;
            return Ok(());
        }
        if state.slave_opened && self.no_endpoints(Side::Slave)? {
            state.slave_closed = true;
            if let Some(slave) = &state.slave {
                let mut queued = 0;
                if unsafe { libc::ioctl(slave.as_raw_fd(), libc::TIOCOUTQ, &mut queued) } < 0 {
                    return Err(io::Error::last_os_error());
                }
                let mut available = 0;
                let master = state.master.as_ref().ok_or_else(|| failure(libc::EIO))?;
                if unsafe { libc::ioctl(master.as_raw_fd(), libc::FIONREAD, &mut available) } < 0 {
                    return Err(io::Error::last_os_error());
                }

                if queued == 0 && available == 0 && !readable(master.as_raw_fd())? {
                    if unsafe { libc::tcgetattr(slave.as_raw_fd(), &mut state.termios) } < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    state.slave.take();
                }
            }
        }
        Ok(())
    }
    pub fn refresh(&self) -> io::Result<()> {
        self.refresh_locked(&mut self.state.lock().unwrap())
    }
    pub fn peer_closed(&self, side: Side) -> io::Result<bool> {
        let mut state = self.state.lock().unwrap();
        self.refresh_locked(&mut state)?;
        Ok(match side {
            Side::Master => state.slave_closed,
            Side::Slave => state.master.is_none(),
        })
    }
    pub fn slave_lock(&self)->bool{self.state.lock().unwrap().slave_locked}
    pub fn set_slave_lock(&self,locked:bool){self.state.lock().unwrap().slave_locked=locked;}
    pub fn master_readiness(&self) -> io::Result<Readiness> {
        let mut state = self.state.lock().unwrap();
        self.refresh_locked(&mut state)?;
        let master = state.master.as_ref().ok_or_else(|| failure(libc::EIO))?;
        Ok(Readiness {
            readable: state.slave.is_some() && readable(master.as_raw_fd())?,
            hangup: state.slave_closed,
        })
    }
    pub fn read_master(&self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let mut state = self.state.lock().unwrap();
        self.refresh_locked(&mut state)?;
        let master = state.master.as_ref().ok_or_else(|| failure(libc::EIO))?;
        let mut queued = 0;
        if unsafe { libc::ioctl(master.as_raw_fd(), libc::FIONREAD, &mut queued) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if state.slave.is_none() && state.slave_closed {
            return Err(failure(libc::EIO));
        }
        let count =
            unsafe { libc::read(master.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        self.refresh_locked(&mut state)?;
        Ok(count as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::DirBuilderExt, path::PathBuf};
    pub(super) fn isolated(name: &str) -> bool {
        if std::env::args().any(|arg| arg == "pty-owner-kernel-fixture") {
            return false;
        }
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                if self.0.try_wait().ok().flatten().is_none() {
                    let _ = self.0.kill();
                }
                let _ = self.0.wait();
            }
        }
        let mut child = Child(
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    name,
                    "--nocapture",
                    "--skip",
                    "pty-owner-kernel-fixture",
                ])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "PTY kernel fixture timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        use std::io::Read;
        let mut output = String::new();
        child
            .0
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        assert!(output.contains("1 passed"), "{output}");
        true
    }
    static NEXT_RUNTIME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    fn runtime() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "aim-pty-owner-{}-{}",
            std::process::id(),
            NEXT_RUNTIME.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
        fs::canonicalize(path).unwrap()
    }
    #[test]
    fn kernel_root_rejects_symlink_and_public_directory() {
        if isolated("pty_owner::tests::kernel_root_rejects_symlink_and_public_directory") {
            return;
        }
        let root = runtime();
        let link = root.with_extension("alias");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        assert!(
            matches!(Pair::allocate(&link),Err(error)if error.raw_os_error()==Some(libc::EINVAL))
        );
        fs::remove_file(link).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            matches!(Pair::allocate(&root),Err(error)if error.raw_os_error()==Some(libc::EPERM))
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        fs::remove_dir(root).unwrap();
    }
    #[test]
    fn completed_output_survives_last_slave_close_with_hangup_and_eio() {
        if isolated(
            "pty_owner::tests::completed_output_survives_last_slave_close_with_hangup_and_eio",
        ) {
            return;
        }
        let runtime = runtime();
        let pair = Pair::allocate(&runtime).unwrap();
        let master = pair.acquire(Side::Master).unwrap();
        let slave = pair.acquire(Side::Slave).unwrap();
        let duplicate = slave.try_clone().unwrap();
        let bytes = [b'x'; 268];
        assert_eq!(
            unsafe {
                libc::write(
                    slave.descriptor().as_raw_fd(),
                    bytes.as_ptr().cast(),
                    bytes.len(),
                )
            },
            268
        );
        drop(slave);
        assert!(
            !pair.master_readiness().unwrap().hangup,
            "real duplicate retains slave open description"
        );
        drop(duplicate);
        assert_eq!(
            pair.master_readiness().unwrap(),
            Readiness {
                readable: true,
                hangup: true
            }
        );
        let mut received = [0u8; 268];
        assert_eq!(pair.read_master(&mut received).unwrap(), 268);
        assert_eq!(received, bytes);
        assert_eq!(
            pair.master_readiness().unwrap(),
            Readiness {
                readable: false,
                hangup: true
            }
        );
        assert_eq!(
            pair.read_master(&mut received).unwrap_err().raw_os_error(),
            Some(libc::EIO)
        );
        let reopened = pair.acquire(Side::Slave).unwrap();
        assert!(!pair.master_readiness().unwrap().hangup);
        drop(reopened);
        drop(master);
        pair.refresh().unwrap();
        assert_eq!(
            pair.master_readiness().unwrap_err().raw_os_error(),
            Some(libc::EIO)
        );
        drop(pair);
        assert_eq!(fs::read_dir(&runtime).unwrap().count(), 0);
        fs::remove_dir(runtime).unwrap();
    }
    #[test]
    fn backpressured_output_is_retained_until_both_kernel_queues_are_empty() {
        if isolated(
            "pty_owner::tests::backpressured_output_is_retained_until_both_kernel_queues_are_empty",
        ) {
            return;
        }
        let runtime = runtime();
        let pair = Pair::allocate(&runtime).unwrap();
        let master = pair.acquire(Side::Master).unwrap();
        let slave = pair.acquire(Side::Slave).unwrap();
        let bytes = [b'x'; 4096];
        let mut written = 0;
        while written < 1 << 20 {
            let count = unsafe {
                libc::write(
                    slave.descriptor().as_raw_fd(),
                    bytes.as_ptr().cast(),
                    bytes.len(),
                )
            };
            if count < 0 {
                assert_eq!(
                    io::Error::last_os_error().raw_os_error(),
                    Some(libc::EWOULDBLOCK)
                );
                break;
            }
            assert!(count > 0);
            written += count as usize;
        }
        assert!(
            written >= 268 && written < 1 << 20,
            "actual PTY must produce backpressure"
        );
        drop(slave);
        assert!(pair.master_readiness().unwrap().hangup);
        let mut received = Vec::new();
        while received.len() < written {
            let mut chunk = [0; 4096];
            let count = pair.read_master(&mut chunk).unwrap();
            assert!(count > 0);
            received.extend_from_slice(&chunk[..count]);
        }
        assert_eq!(received.len(), written);
        assert!(received.iter().all(|byte| *byte == b'x'));
        assert_eq!(
            pair.read_master(&mut [0; 1]).unwrap_err().raw_os_error(),
            Some(libc::EIO)
        );
        drop(master);
        drop(pair);
        fs::remove_dir(runtime).unwrap();
    }
    #[test]
    #[ignore = "real child exercised by endpoint_lease_survives_fork_exec_until_final_close"]
    fn inherited_endpoint_child() {
        use std::io::{BufRead, Write};
        let mut input = io::stdin().lock();
        let mut line = String::new();
        input.read_line(&mut line).unwrap();
        let fds = line
            .trim()
            .split(',')
            .map(str::parse::<i32>)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(fds.len(), 2);
        for fd in &fds {
            assert!(unsafe { libc::fcntl(*fd, libc::F_GETFD) } >= 0);
        }
        println!("PTY_ENDPOINT_INHERITED");
        io::stdout().flush().unwrap();
        line.clear();
        input.read_line(&mut line).unwrap();
        let output = [b'x'; 268];
        assert_eq!(
            unsafe { libc::write(fds[0], output.as_ptr().cast(), output.len()) },
            268
        );
        for fd in fds {
            assert_eq!(unsafe { libc::close(fd) }, 0);
        }
    }
    #[test]
    fn endpoint_lease_survives_fork_exec_until_final_close() {
        if isolated("pty_owner::tests::endpoint_lease_survives_fork_exec_until_final_close") {
            return;
        }
        use std::{
            io::{BufRead, Write},
            os::unix::process::CommandExt,
            process::{Command, Stdio},
        };
        let runtime = runtime();
        let pair = Pair::allocate(&runtime).unwrap();
        let master = pair.acquire(Side::Master).unwrap();
        let slave = pair.acquire(Side::Slave).unwrap();
        let backing = slave.descriptor().as_raw_fd();
        let lease = slave.lease().as_raw_fd();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "pty_owner::tests::inherited_endpoint_child",
                "--ignored",
                "--nocapture",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        unsafe {
            command.pre_exec(move || {
                for fd in [backing, lease] {
                    if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                if self.0.try_wait().ok().flatten().is_none() {
                    let _ = self.0.kill();
                }
                let _ = self.0.wait();
            }
        }
        let mut child = Child(command.spawn().unwrap());
        let mut input = child.0.stdin.take().unwrap();
        writeln!(input, "{backing},{lease}").unwrap();
        let mut output = io::BufReader::new(child.0.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert!(output.read_line(&mut line).unwrap() > 0);
            if line.trim() == "PTY_ENDPOINT_INHERITED" {
                break;
            }
        }
        drop(slave);
        assert!(
            !pair.master_readiness().unwrap().hangup,
            "actual inherited lease keeps peer open after parent closes"
        );
        writeln!(input, "close").unwrap();
        drop(input);
        assert!(child.0.wait().unwrap().success());
        assert!(pair.master_readiness().unwrap().hangup);
        let mut received = [0; 268];
        assert_eq!(pair.read_master(&mut received).unwrap(), 268);
        assert!(received.iter().all(|byte| *byte == b'x'));
        assert_eq!(
            pair.read_master(&mut received).unwrap_err().raw_os_error(),
            Some(libc::EIO)
        );
        drop(master);
        drop(pair);
        fs::remove_dir(runtime).unwrap();
    }
}
