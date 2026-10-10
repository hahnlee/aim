//! Guest process POSIX locks live in a native holder, never on private pins.
use crate::errno::{self, Errno};
use aim_storage::{
    inode_lease::Identity,
    posix_control::{self, Frame, Operation, Owner},
    process_namespace::ProcessIdentity,
};
use std::{
    collections::HashMap,
    os::fd::{AsRawFd, BorrowedFd},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
fn io(error: std::io::Error) -> Errno {
    errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LinuxFlock {
    pub kind: i16,
    pub whence: i16,
    pub pad: i32,
    pub start: i64,
    pub len: i64,
    pub pid: i32,
    pub pad2: i32,
}
pub struct Client {
    control: posix_control::Client,
    pub owner: Owner,
    controller: ProcessIdentity,
    tickets: Mutex<HashMap<([u8; 36], i32), u64>>,
    next: AtomicU64,
}
impl Client {
    pub fn new(
        control: posix_control::Client,
        owner: Owner,
        controller: ProcessIdentity,
    ) -> Result<Arc<Self>, Errno> {
        if !owner.process.is_live() || owner.guest_pid <= 0 || !controller.is_live() {
            return Err(errno::ESRCH);
        }
        Ok(Arc::new(Self {
            control,
            owner,
            controller,
            tickets: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
        }))
    }
    fn call(&self, frame: Frame, proof: Option<BorrowedFd<'_>>) -> Result<Frame, Errno> {
        Self::reply(
            self.control
                .begin(frame, proof)
                .map_err(io)?
                .wait_authenticated(Duration::from_secs(5), self.controller)
                .map_err(io)?,
        )
    }
    fn request(&self) -> Result<u64, Errno> {
        self.next
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| 75)
    }
    fn reply(reply: Frame) -> Result<Frame, Errno> {
        if reply.errno != 0 {
            return Err(errno::from_darwin(reply.errno));
        }
        Ok(reply)
    }
    fn ticket(&self, fd: BorrowedFd<'_>, key: Identity, access: i32) -> Result<u64, Errno> {
        let mut tickets = self.tickets.lock().unwrap();
        if let Some(ticket) = tickets.get(&(key.to_bytes(), access)) {
            return Ok(*ticket);
        }
        let mut frame = Frame::new(Operation::Register, self.request()?);
        frame.key = key.to_bytes();
        frame.access = access;
        let reply = self.call(frame, Some(fd))?;
        tickets.insert((frame.key, access), reply.ticket);
        Ok(reply.ticket)
    }
    pub fn close_inode(&self, key: Identity) -> Result<(), Errno> {
        let mut tickets = self.tickets.lock().unwrap();
        let mut frame = Frame::new(Operation::Close, self.request()?);
        frame.key = key.to_bytes();
        let pending = self.control.begin(frame, None).map_err(io)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(errno::from_darwin(libc::ETIMEDOUT));
            }
            match pending.wait_authenticated(remaining, self.controller) {
                Ok(response) => {
                    Self::reply(response)?;
                    break;
                }
                Err(error) if error.raw_os_error() == Some(libc::EINTR) => {
                    #[cfg(test)]
                    CLOSE_INTERRUPTED.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => return Err(io(error)),
            }
        }
        tickets.retain(|(identity, _), _| *identity != frame.key);
        Ok(())
    }
    pub fn capability(&self) -> Result<posix_control::SendRight, Errno> {
        self.control.capability().map_err(io)
    }
    pub fn lock(
        &self,
        fd: BorrowedFd<'_>,
        cmd: u64,
        input: LinuxFlock,
        interrupted: impl Fn() -> bool,
    ) -> Result<LinuxFlock, Errno> {
        if !matches!(cmd, 5 | 6 | 7) {
            return Err(errno::EINVAL);
        }
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } < 0 {
            return Err(errno::last());
        }
        let position = if input.whence == 1 {
            let position = unsafe { libc::lseek(fd.as_raw_fd(), 0, libc::SEEK_CUR) };
            if position < 0 {
                return Err(errno::last());
            }
            position
        } else {
            0
        };
        let range = posix_control::normalize(
            input.kind,
            input.whence,
            input.start,
            input.len,
            position,
            stat.st_size,
        )
        .map_err(io)?;
        let access = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
        if access < 0 {
            return Err(errno::last());
        }
        let access = access & libc::O_ACCMODE;
        if cmd != 5
            && (range.kind == 0 && access == libc::O_WRONLY
                || range.kind == 1 && access == libc::O_RDONLY)
        {
            return Err(errno::EBADF);
        }
        let key = Identity::from_fd(fd).map_err(io)?;
        let ticket = self.ticket(fd, key, access)?;
        let mut frame = Frame::new(
            match cmd {
                5 => Operation::Get,
                6 => Operation::Set,
                _ => Operation::Wait,
            },
            self.request()?,
        );
        frame.ticket = ticket;
        frame.range = range;
        let pending = self.control.begin(frame, None).map_err(io)?;
        let mut cancelled = false;
        let reply = loop {
            match pending.wait_authenticated(Duration::from_millis(25), self.controller) {
                Ok(reply) => break Self::reply(reply)?,
                Err(error)
                    if matches!(
                        error.raw_os_error(),
                        Some(libc::ETIMEDOUT) | Some(libc::EINTR)
                    ) =>
                {
                    if interrupted() && !cancelled {
                        let mut cancel = Frame::new(Operation::Cancel, self.request()?);
                        cancel.ticket = frame.request;
                        self.call(cancel, None)?;
                        cancelled = true;
                    }
                }
                Err(error) => return Err(io(error)),
            }
        };
        let mut output = input;
        if cmd == 5 {
            output.kind = reply.range.kind;
            if output.kind != 2 {
                output.whence = 0;
                output.start = reply.range.start;
                output.len = reply.range.len;
                if reply.guest_pid < 0 {
                    return Err(71);
                }
                output.pid = reply.guest_pid;
            }
        }
        Ok(output)
    }
}
#[cfg(test)]
static CLOSE_INTERRUPTED:AtomicU64=AtomicU64::new(0);
static CURRENT: Mutex<Option<Arc<Client>>> = Mutex::new(None);
/// Root configures an authenticated controller-produced owner at process start.
pub fn install(client: Arc<Client>) -> Result<(), Errno> {
    let actual = ProcessIdentity::running(unsafe { libc::getpid() }).map_err(io)?;
    if actual != client.owner.process {
        return Err(errno::EPERM);
    }
    *CURRENT.lock().unwrap() = Some(client);
    Ok(())
}
pub fn current() -> Result<Arc<Client>, Errno> {
    let client = CURRENT.lock().unwrap().clone().ok_or(37)?;
    if ProcessIdentity::running(unsafe { libc::getpid() }).map_err(io)? != client.owner.process {
        return Err(errno::ESRCH);
    }
    Ok(client)
}
/// Child fork state receives descriptors, but no parent POSIX lock ownership.
pub fn reset_fork() {
    *CURRENT.lock().unwrap() = None;
}
pub fn dispatch(fd: BorrowedFd<'_>, cmd: u64, arg: u64) -> i64 {
    let input = unsafe { (arg as *const LinuxFlock).read_unaligned() };
    let result = current().and_then(|owner| {
        owner.lock(fd, cmd, input, || {
            super::thread::current().is_some_and(super::signal::interrupted)
        })
    });
    match result {
        Ok(output) => {
            if cmd == 5 {
                unsafe {
                    (arg as *mut LinuxFlock).write_unaligned(output);
                }
            }
            0
        }
        Err(error) => -(error as i64),
    }
}

static FORK_LOCATOR: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);
pub fn set_fork_locator(locator: Option<std::path::PathBuf>) {
    *FORK_LOCATOR.lock().unwrap() = locator;
}
pub fn attach_fork() -> Result<(), Errno> {
    let locator = FORK_LOCATOR.lock().unwrap().clone();
    start(locator.as_deref())
}
/// Attach to the native-admitted owner. Namespace init and the Mach reply
/// authenticate the controller; a host PID alone is never a guest PID claim.
pub fn start(locator: Option<&std::path::Path>) -> Result<(), Errno> {
    attach(locator,Operation::Attach)
}
pub(crate) fn start_external(locator: Option<&std::path::Path>) -> Result<(), Errno> {
    attach(locator,Operation::ExternalAttach)
}
fn attach(locator: Option<&std::path::Path>,operation:Operation) -> Result<(), Errno> {
    let Some(locator) = locator else {
        return Ok(());
    };
    let table = super::cred::by_pid_dir().ok_or(errno::ESRCH)?;
    let init = aim_storage::process_namespace::InitRegistration::read(table).map_err(io)?;
    let config = aim_storage::posix_broker::OwnerConfig::read(locator, init.process).map_err(io)?;
    if config.process != init.process {
        return Err(errno::EPERM);
    }
    let process = ProcessIdentity::running(unsafe { libc::getpid() }).map_err(io)?;
    let control = posix_control::Client::lookup(&config.endpoint).map_err(io)?;
    let frame = Frame::new(operation, 1);
    let reply = Client::reply(
        control
            .begin(frame, None)
            .map_err(io)?
            .wait_authenticated(Duration::from_secs(5), config.process)
            .map_err(io)?,
    )?;
    if reply.guest_pid <= 0 {
        return Err(errno::ESRCH);
    }
    let client = Client::new(
        control,
        Owner {
            process,
            guest_pid: reply.guest_pid,
        },
        config.process,
    )?;
    // Attach consumed request1 in the same native owner incarnation.
    client.next.store(2, Ordering::Relaxed);
    install(client)
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    #[test]
    fn authenticated_bootstrap_and_fork_reset_require_actual_owner_evidence() {
        const MARKER: &str = "posix-bootstrap-owner-fixture";
        if !std::env::args().any(|arg| arg == MARKER) {
            let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","sys::posix_locks::lifecycle_tests::authenticated_bootstrap_and_fork_reset_require_actual_owner_evidence","--skip",MARKER,"--nocapture"]).output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                String::from_utf8_lossy(&output.stdout).contains("POSIX_BOOTSTRAP_OWNER_EXECUTED")
            );
            return;
        }
        let actual = ProcessIdentity::running(unsafe { libc::getpid() }).unwrap();
        let root = std::env::temp_dir().join(format!("aim-posix-bootstrap-{}", actual.host_pid));
        std::fs::create_dir(&root).unwrap();
        let table = root.join("by-pid");
        std::fs::create_dir(&table).unwrap();
        super::super::cred::init(super::super::cred::Identity::default(), Some(table.clone()));
        let executable = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("aim-lock-holder");
        let controller =
            aim_storage::posix_broker::Controller::start(aim_storage::posix_broker::Config {
                endpoint: format!("com.aim.posix-bootstrap.{}", actual.host_pid),
                holder: executable,
                startup_timeout: Duration::from_secs(5),
            })
            .unwrap();
        let locator = root.join("posix-control-owner");
        controller.owner_config().unwrap().write(&locator).unwrap();
        assert_eq!(start(Some(&locator)), Err(errno::ENOENT));
        assert!(current().is_err());
        let registration = aim_storage::process_namespace::InitRegistration::register(
            &table,
            actual,
            &format!("posix-fixture-{}", actual.host_pid),
        )
        .unwrap();
        assert_eq!(start(Some(&locator)), Err(errno::EPERM));
        assert!(
            current().is_err(),
            "no native admission is not a host PID fallback"
        );
        controller
            .register_guest(Owner {
                process: actual,
                guest_pid: 1,
            })
            .unwrap();
        start(Some(&locator)).unwrap();
        assert_eq!(
            current().unwrap().owner,
            Owner {
                process: actual,
                guest_pid: 1
            }
        );
        use std::os::fd::AsFd;
        let file_path = root.join("actual-lock");
        std::fs::write(&file_path, b"authenticated native owner").unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&file_path)
            .unwrap();
        let lock = LinuxFlock {
            kind: 1,
            whence: 0,
            pad: 0,
            start: 0,
            len: 8,
            pid: 0,
            pad2: 0,
        };
        current()
            .unwrap()
            .lock(file.as_fd(), 6, lock, || false)
            .unwrap();
        assert_eq!(
            current()
                .unwrap()
                .lock(file.as_fd(), 5, lock, || false)
                .unwrap()
                .kind,
            2
        );
        // The holder has a real file lock while a pipe's EOF reader closes its
        // guest descriptor. A successful host close must not fail postclose.
        for flags in [0,0o2000000]{
            let mut pipe=[-1i32;2];
            assert_eq!(super::super::fs::pipe2([pipe.as_mut_ptr()as u64,flags,0,0,0,0]),0);
            assert!(super::super::fdtab::visible(pipe[0])&&super::super::fdtab::visible(pipe[1]));
            let reader=pipe[0];let worker=std::thread::spawn(move||{
                let mut bytes=vec![];let mut buffer=[0u8;7];
                loop{let count=super::super::fs::read([reader as u64,buffer.as_mut_ptr()as u64,buffer.len()as u64,0,0,0]);assert!(count>=0,"active-holder pipe read {count}");if count==0{break;}bytes.extend_from_slice(&buffer[..count as usize]);}
                assert!(super::super::fdtab::visible(reader));let before=unsafe{libc::fcntl(reader,libc::F_GETFD)};assert!(before>=0);
                let closed=super::super::fs::close([reader as u64,0,0,0,0,0]);let after=unsafe{libc::fcntl(reader,libc::F_GETFD)};
                assert_eq!(closed,0,"active-holder EOF close fd={reader} before={before} after={after} visible={}",super::super::fdtab::visible(reader));
                assert_eq!(after,-1);assert!(!super::super::fdtab::visible(reader));bytes
            });
            let bytes=b"original buffered pipe output";
            assert_eq!(super::super::fs::write([pipe[1]as u64,bytes.as_ptr()as u64,bytes.len()as u64,0,0,0]),bytes.len()as i64);
            assert_eq!(super::super::fs::close([pipe[1]as u64,0,0,0,0,0]),0,"active-holder pipe writer close");
            assert_eq!(worker.join().unwrap(),bytes);
        }
        current()
            .unwrap()
            .close_inode(Identity::from_fd(file.as_fd()).unwrap())
            .unwrap();
        drop(file);
        reset_fork();
        assert!(current().is_err());
        start(Some(&locator)).unwrap();
        assert_eq!(
            current().unwrap().owner.guest_pid,
            1,
            "authenticated exec reattach preserves real owner"
        );
        struct Child(Option<std::process::Child>);
        impl Drop for Child {
            fn drop(&mut self) {
                if let Some(mut child) = self.0.take() {
                    if child.try_wait().unwrap().is_none() {
                        let _ = child.kill();
                    }
                    let _ = child.wait();
                }
            }
        }
        let mut other = Child(Some(
            std::process::Command::new("/bin/sleep")
                .arg("30")
                .spawn()
                .unwrap(),
        ));
        let process = ProcessIdentity::running(other.0.as_ref().unwrap().id() as i32).unwrap();
        let wrong = Client::new(
            posix_control::Client::lookup(&controller.owner_config().unwrap().endpoint).unwrap(),
            Owner {
                process,
                guest_pid: process.host_pid,
            },
            actual,
        )
        .unwrap();
        assert_eq!(install(wrong), Err(errno::EPERM));
        assert_eq!(current().unwrap().owner.process, actual);
        let wrong_reply = Client::new(
            posix_control::Client::lookup(&controller.owner_config().unwrap().endpoint).unwrap(),
            Owner {
                process: actual,
                guest_pid: 1,
            },
            process,
        )
        .unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&file_path)
            .unwrap();
        assert_eq!(
            wrong_reply.lock(file.as_fd(), 5, lock, || false).err(),
            Some(errno::EPERM)
        );
        drop(file);
        let mut child = other.0.take().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        let stale = Owner {
            process,
            guest_pid: process.host_pid,
        };
        assert!(
            Client::new(
                posix_control::Client::lookup(&controller.owner_config().unwrap().endpoint)
                    .unwrap(),
                stale,
                actual
            )
            .is_err()
        );
        reset_fork();
        controller.shutdown().unwrap();
        registration.remove(&table).unwrap();
        std::fs::remove_dir_all(root).unwrap();
        println!("POSIX_BOOTSTRAP_OWNER_EXECUTED");
    }
}

#[cfg(test)]
mod interrupted_close_tests {
    use super::*;
    use std::os::fd::{AsFd, AsRawFd, IntoRawFd};
    #[test]
    fn close_reply_preserves_real_noninterrupted_error() {
        if super::super::fdtab::isolated_kernel_test(
            "sys::posix_locks::interrupted_close_tests::close_reply_preserves_real_noninterrupted_error",
        ) {
            return;
        }
        let actual = ProcessIdentity::running(unsafe { libc::getpid() }).unwrap();
        let name = format!("dev.aim.close-error.{}", actual.host_pid);
        let endpoint = posix_control::Endpoint::register(&name).unwrap();
        let client = Client::new(
            posix_control::Client::lookup(&name).unwrap(),
            Owner {
                process: actual,
                guest_pid: actual.host_pid,
            },
            actual,
        )
        .unwrap();
        let file = std::fs::File::open("/dev/null").unwrap();
        let identity = Identity::from_fd(file.as_fd()).unwrap();
        let server = std::thread::spawn(move || {
            let request = endpoint.receive(Duration::from_secs(2)).unwrap();
            assert_eq!(request.actor, actual);
            assert_eq!(request.frame.operation, Operation::Close);
            let mut response = request.frame;
            response.errno = libc::EIO;
            request.reply.send(response).unwrap();
        });
        let before = CLOSE_INTERRUPTED.load(Ordering::Relaxed);
        assert_eq!(client.close_inode(identity), Err(errno::EIO));
        server.join().unwrap();
        assert_eq!(CLOSE_INTERRUPTED.load(Ordering::Relaxed), before);
    }
    extern "C" fn unrelated_signal(_: i32) {}
    #[test]
    fn interrupted_close_wait_preserves_one_request_and_reused_descriptor() {
        if super::super::fdtab::isolated_kernel_test(
            "sys::posix_locks::interrupted_close_tests::interrupted_close_wait_preserves_one_request_and_reused_descriptor",
        ) {
            return;
        }
        let (_view, directory) = crate::vfs::test_view();
        let actual = ProcessIdentity::running(unsafe { libc::getpid() }).unwrap();
        let holder = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("aim-lock-holder");
        assert!(holder.is_file());
        let endpoint = format!("dev.aim.interrupted-close.{}", actual.host_pid);
        let controller =
            aim_storage::posix_broker::Controller::start(aim_storage::posix_broker::Config {
                endpoint: endpoint.clone(),
                holder,
                startup_timeout: Duration::from_secs(5),
            })
            .unwrap();
        let owner = Owner {
            process: actual,
            guest_pid: actual.host_pid,
        };
        controller.register_guest(owner).unwrap();
        let native = posix_control::Client::lookup(&endpoint).unwrap();
        let locker = Client::new(native.try_clone().unwrap(), owner, actual).unwrap();
        let path = directory.join("original-lock");
        std::fs::write(&path, b"original").unwrap();
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let lock = LinuxFlock {
            kind: 1,
            whence: 0,
            pad: 0,
            start: 0,
            len: 1,
            pid: 0,
            pad2: 0,
        };
        locker.lock(file.as_fd(), 6, lock, || false).unwrap();
        let mut held = libc::flock {
            l_start: 0, l_len: 1, l_pid: 0,
            l_type: libc::F_WRLCK as i16, l_whence: libc::SEEK_SET as i16,
        };
        assert_eq!(unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut held) }, 0);
        assert_eq!(held.l_type, libc::F_WRLCK as i16);
        assert!(held.l_pid > 0 && held.l_pid != actual.host_pid);

        let original = file.into_raw_fd();
        super::super::fdtab::publish_guest(original).unwrap();
        let replacement = std::fs::File::open("/dev/null").unwrap();
        let replacement_fd = replacement.as_raw_fd();
        let relay_name = format!("dev.aim.close-relay.{}", actual.host_pid);
        let relay = posix_control::Endpoint::register(&relay_name).unwrap();
        let close_client = Client::new(
            posix_control::Client::lookup(&relay_name).unwrap(),
            owner,
            actual,
        )
        .unwrap();
        install(close_client).unwrap();
        let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = unrelated_signal as *const () as usize;
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
            assert_eq!(libc::sigaction(libc::SIGUSR2, &action, &mut previous), 0);
        }
        struct Handler(libc::sigaction);
        impl Drop for Handler {
            fn drop(&mut self) {
                unsafe {
                    libc::sigaction(libc::SIGUSR2, &self.0, std::ptr::null_mut());
                }
            }
        }
        let _handler = Handler(previous);
        let waiting = unsafe { libc::pthread_self() } as usize;
        let before = CLOSE_INTERRUPTED.load(Ordering::Relaxed);
        let relay_thread = std::thread::spawn(move || {
            let request = relay.receive(Duration::from_secs(2)).unwrap();
            assert_eq!(request.actor, actual);
            assert_eq!(request.frame.operation, Operation::Close);
            // The authentic RPC is held while a real unrelated host signal interrupts
            // the waiting guest syscall; then exactly one request reaches the broker.
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while CLOSE_INTERRUPTED.load(Ordering::Relaxed) == before {
                assert!(std::time::Instant::now() < deadline);
                unsafe {
                    assert_eq!(
                        libc::pthread_kill(waiting as libc::pthread_t, libc::SIGUSR2),
                        0
                    );
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            {
                let _guard = super::super::fdtab::lifecycle();
                assert_eq!(unsafe { libc::dup2(replacement_fd, original) }, original);
                super::super::fdtab::publish_guest(original).unwrap();
            }
            let response = native
                .begin(request.frame, None)
                .unwrap()
                .wait_authenticated(Duration::from_secs(2), actual)
                .unwrap();
            assert_eq!(response.errno, 0);
            request.reply.send(response).unwrap();
            assert!(
                matches!(relay.receive(Duration::from_millis(25)),Err(error)if error.raw_os_error()==Some(libc::ETIMEDOUT))
            );
        });
        assert_eq!(super::super::fs::close([original as u64, 0, 0, 0, 0, 0]), 0);
        relay_thread.join().unwrap();
        assert!(CLOSE_INTERRUPTED.load(Ordering::Relaxed) > before);
        assert!(super::super::fdtab::visible(original));
        assert!(unsafe { libc::fcntl(original, libc::F_GETFD) } >= 0);
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let mut query = libc::flock {
            l_start: 0,
            l_len: 1,
            l_pid: 0,
            l_type: libc::F_WRLCK as i16,
            l_whence: libc::SEEK_SET as i16,
        };
        assert_eq!(
            unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut query) },
            0
        );
        assert_eq!(query.l_type, libc::F_UNLCK as i16);
        reset_fork();
        super::super::fdtab::close_owned_guest(original, false).unwrap();
        drop(replacement);
        drop(controller);
    }
}
