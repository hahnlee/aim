//! A merge's metadata and input references belong to the socket, including
//! when its creator exits. A native owner serves holders by socket identity
//! and retires each entry when the writer observes its peer's last close.
use aim_storage::private_fd::PrivateFd;
use aim_sync_file::State;
use std::{
    collections::BTreeMap,
    io::{self, Read, Write},
    os::{
        fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
        unix::net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

const CREATE: u8 = 1;
const INFO: u8 = 2;
static OWNER: Mutex<Option<PathBuf>> = Mutex::new(None);

fn error(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code)
}

fn context(fd: BorrowedFd<'_>) -> io::Result<u64> {
    Ok(aim_storage::socket_inode::identity(fd.as_raw_fd())?.cookie)
}

fn locator(fd: BorrowedFd<'_>) -> io::Result<Option<PathBuf>> {
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let mut len = size_of_val(&address) as u32;
    if unsafe {
        libc::getsockname(
            fd.as_raw_fd(),
            (&mut address as *mut libc::sockaddr_un).cast(),
            &mut len,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let bytes = unsafe {
        std::slice::from_raw_parts(
            address.sun_path.as_ptr().cast::<u8>(),
            address.sun_path.len(),
        )
    };
    let bytes = &bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len())];
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(std::ffi::OsStr::from_bytes(bytes));
    Ok(
        (bytes.starts_with(b"/tmp/aim-sync-") && path.file_name().is_some_and(|n| n == "fence"))
            .then(|| path.with_file_name("owner")),
    )
}

/// A holder gets private references to the original, flattened fences.
pub(super) fn info(fd: BorrowedFd<'_>) -> io::Result<([u8; 32], Vec<PrivateFd>)> {
    let Some(path) = locator(fd)? else {
        let name = super::name32(format!("aim-aim{}-1", context(fd)?).as_bytes());
        return Ok((name, vec![PrivateFd::allocate(|| fd.try_clone_to_owned())?]));
    };
    let mut stream = connect(&path)?;
    stream.write_all(&[INFO])?;
    send_fd(stream.as_fd(), fd)?;
    read_result(&mut stream)?;
    let mut name = [0; 32];
    stream.read_exact(&mut name)?;
    let count = read_count(&mut stream)?;
    if count == 0 {
        return Err(error(libc::ENOENT));
    }
    let mut inputs = Vec::new();
    for _ in 0..count {
        inputs.push(PrivateFd::allocate(|| receive_fd(stream.as_fd()))?);
    }
    Ok((name, inputs))
}

pub(super) fn merge(a: BorrowedFd<'_>, b: BorrowedFd<'_>, name: [u8; 32]) -> io::Result<OwnedFd> {
    let mut inputs = info(a)?.1;
    inputs.extend(info(b)?.1);
    // Host producers allocate one context per socket (sequence number 1).
    // Aliases of a context therefore denote the very same fence.
    let mut contexts = BTreeMap::new();
    let mut timestamp = 0;
    for fd in inputs {
        match aim_sync_file::state(fd.as_fd()) {
            State::Active => {
                contexts.insert(context(fd.as_fd())?, fd);
            }
            State::Signaled { timestamp_ns, .. } => timestamp = timestamp.max(timestamp_ns),
        }
    }
    // Linux 6.12 dma_fence_unwrap_merge drops already signaled inputs; if
    // none remain it allocates a private successful fence at the last time.
    if contexts.is_empty() {
        let fd = PrivateFd::allocate(|| aim_sync_file::signaled(timestamp, 1))?;
        contexts.insert(context(fd.as_fd())?, fd);
    }
    let mut owner = OWNER.lock().unwrap();
    for attempt in 0..2 {
        if owner.is_none() {
            *owner = Some(start()?);
        }
        let result = create(owner.as_ref().unwrap(), name, &contexts);
        match result {
            // An idle owner can exit between connect and request. Nothing
            // has been published, so replay with the same retained inputs.
            Err(e)
                if attempt == 0
                    && matches!(
                        e.kind(),
                        io::ErrorKind::NotFound
                            | io::ErrorKind::ConnectionRefused
                            | io::ErrorKind::ConnectionReset
                            | io::ErrorKind::BrokenPipe
                            | io::ErrorKind::UnexpectedEof
                    ) =>
            {
                *owner = None;
            }
            result => return result,
        }
    }
    unreachable!()
}

fn create(path: &Path, name: [u8; 32], contexts: &BTreeMap<u64, PrivateFd>) -> io::Result<OwnedFd> {
    let mut stream = connect(path)?;
    stream.write_all(&[CREATE])?;
    stream.write_all(&name)?;
    stream.write_all(&(contexts.len() as u32).to_ne_bytes())?;
    for fd in contexts.values() {
        send_fd(stream.as_fd(), fd.as_fd())?;
    }
    read_result(&mut stream)?;
    let fd = receive_fd(stream.as_fd())?;
    // Keep the sender's native descriptor alive until the right has been
    // externalized: XNU may collect a socket held only in an in-flight message.
    stream.write_all(&[0])?;
    Ok(fd)
}

struct Stream(PrivateFd);
impl AsFd for Stream {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}
impl Read for Stream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let n = unsafe { libc::read(self.0.as_raw_fd(), bytes.as_mut_ptr().cast(), bytes.len()) };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }
}
impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let n = unsafe {
            libc::send(
                self.0.as_raw_fd(),
                bytes.as_ptr().cast(),
                bytes.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn connect(path: &Path) -> io::Result<Stream> {
    Ok(Stream(PrivateFd::allocate(|| {
        let stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        Ok(stream.into())
    })?))
}
fn read_count(stream: &mut impl Read) -> io::Result<u32> {
    let mut count = [0; 4];
    stream.read_exact(&mut count)?;
    Ok(u32::from_ne_bytes(count))
}
fn read_result(stream: &mut impl Read) -> io::Result<()> {
    let code = read_count(stream)? as i32;
    if code == 0 { Ok(()) } else { Err(error(code)) }
}
fn send_fd(socket: BorrowedFd<'_>, fd: BorrowedFd<'_>) -> io::Result<()> {
    let mut control = [0usize; 4];
    let mut byte = 0u8;
    let mut iov = libc::iovec {
        iov_base: (&mut byte as *mut u8).cast(),
        iov_len: 1,
    };
    unsafe {
        let mut message: libc::msghdr = std::mem::zeroed();
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = libc::CMSG_SPACE(4);
        let c = libc::CMSG_FIRSTHDR(&message);
        (*c).cmsg_level = libc::SOL_SOCKET;
        (*c).cmsg_type = libc::SCM_RIGHTS;
        (*c).cmsg_len = libc::CMSG_LEN(4);
        libc::CMSG_DATA(c)
            .cast::<i32>()
            .write_unaligned(fd.as_raw_fd());
        loop {
            if libc::sendmsg(socket.as_raw_fd(), &message, libc::MSG_NOSIGNAL) == 1 {
                return Ok(());
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }
}
fn receive_fd(socket: BorrowedFd<'_>) -> io::Result<OwnedFd> {
    let mut control = [0usize; 4];
    let mut byte = 0u8;
    let mut iov = libc::iovec {
        iov_base: (&mut byte as *mut u8).cast(),
        iov_len: 1,
    };
    unsafe {
        let mut message: libc::msghdr = std::mem::zeroed();
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = size_of_val(&control) as u32;
        loop {
            let n = libc::recvmsg(socket.as_raw_fd(), &mut message, 0);
            if n >= 0 {
                let mut descriptors = Vec::new();
                let mut c = libc::CMSG_FIRSTHDR(&message);
                while !c.is_null() {
                    if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                        let count = (*c).cmsg_len.saturating_sub(libc::CMSG_LEN(0)) / 4;
                        for i in 0..count {
                            descriptors.push(OwnedFd::from_raw_fd(
                                libc::CMSG_DATA(c)
                                    .add(i as usize * 4)
                                    .cast::<i32>()
                                    .read_unaligned(),
                            ));
                        }
                    }
                    c = libc::CMSG_NXTHDR(&message, c);
                }
                if n != 1 || descriptors.len() != 1 || message.msg_flags & libc::MSG_CTRUNC != 0 {
                    return Err(error(libc::EPROTO));
                }
                let fd = descriptors.pop().unwrap();
                if libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
                return Ok(fd);
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
    }
}

fn start() -> io::Result<PathBuf> {
    let mut template = b"/tmp/aim-sync-XXXXXX\0".to_vec();
    if unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) }.is_null() {
        return Err(io::Error::last_os_error());
    }
    let directory = PathBuf::from(std::str::from_utf8(&template[..template.len() - 1]).unwrap());
    let result = (|| {
        let (mut parent, child) = UnixStream::pair()?;
        parent.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut command = Command::new(std::env::current_exe()?);
        #[cfg(not(test))]
        command.arg("--sync-file-owner").arg(&directory);
        #[cfg(test)]
        command
            .args([
                "--exact",
                "sys::sync_file::owner::tests::owner_process",
                "--ignored",
                "--nocapture",
                "--skip",
            ])
            .arg(&directory);
        command
            .stdin(Stdio::from(OwnedFd::from(child)))
            .stdout(Stdio::null());
        let mut child = command.spawn()?;
        let mut ready = [0];
        if let Err(e) = parent.read_exact(&mut ready) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        std::thread::spawn(move || {
            if let Err(e) = child.wait() {
                eprintln!("sync_file owner reap: {e}");
            }
        });
        Ok(directory.join("owner"))
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(directory);
    }
    result
}

struct Entry {
    name: [u8; 32],
    inputs: Vec<OwnedFd>,
    writer: Option<aim_sync_file::Writer>,
    observer: OwnedFd,
}
impl Entry {
    fn live(&mut self) -> bool {
        let mut byte = 0u8;
        let n = unsafe {
            libc::recv(
                self.observer.as_raw_fd(),
                (&mut byte as *mut u8).cast(),
                1,
                libc::MSG_PEEK | libc::MSG_DONTWAIT,
            )
        };
        if n < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ECONNRESET) {
            return false;
        }
        if self.writer.is_some() {
            let mut timestamp = 0;
            let mut status = 1;
            for fd in &self.inputs {
                match aim_sync_file::state(fd.as_fd()) {
                    State::Active => return true,
                    State::Signaled {
                        timestamp_ns,
                        status: s,
                    } => {
                        timestamp = timestamp.max(timestamp_ns);
                        if status == 1 && s < 0 {
                            status = s;
                        }
                    }
                }
            }
            self.writer.take().unwrap().signal_at(timestamp, status);
        }
        true
    }
}

/// Internal native helper entry point, before guest/runtime initialization.
pub fn serve(directory: &Path) -> io::Result<()> {
    struct Cleanup<'a>(&'a Path);
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0);
        }
    }
    let _cleanup = Cleanup(directory);
    let listener = UnixListener::bind(directory.join("owner"))?;
    listener.set_nonblocking(true)?;
    if unsafe { libc::write(0, [0u8].as_ptr().cast(), 1) } != 1 {
        return Err(io::Error::last_os_error());
    }
    let mut entries: BTreeMap<u64, Entry> = BTreeMap::new();
    let mut idle = Instant::now();
    loop {
        entries.retain(|_, entry| entry.live());
        if !entries.is_empty() {
            idle = Instant::now();
        }
        if idle.elapsed() >= Duration::from_secs(1) {
            return Ok(());
        }
        let mut poll = vec![libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        for entry in entries.values().filter(|e| e.writer.is_some()) {
            for fd in &entry.inputs {
                if aim_sync_file::state(fd.as_fd()) == State::Active {
                    poll.push(libc::pollfd {
                        fd: fd.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    });
                }
            }
        }
        if unsafe { libc::poll(poll.as_mut_ptr(), poll.len() as _, 20) } < 0 {
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
        loop {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            idle = Instant::now();
            stream.set_nonblocking(false)?;
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            stream.set_write_timeout(Some(Duration::from_secs(5)))?;
            // An unsuccessful request closes its unpublished descriptors.
            let mut responded = false;
            if let Err(e) = request(directory, &mut stream, &mut entries, &mut responded) {
                if !responded {
                    if let Err(reply) =
                        stream.write_all(&e.raw_os_error().unwrap_or(libc::EIO).to_ne_bytes())
                    {
                        eprintln!("sync_file owner error reply: {reply}");
                    }
                }
                eprintln!("sync_file owner request: {e}");
            }
        }
    }
}

fn request(
    directory: &Path,
    stream: &mut UnixStream,
    entries: &mut BTreeMap<u64, Entry>,
    responded: &mut bool,
) -> io::Result<()> {
    let mut op = [0];
    stream.read_exact(&mut op)?;
    match op[0] {
        CREATE => {
            let mut name = [0; 32];
            stream.read_exact(&mut name)?;
            let count = read_count(stream)?;
            if count == 0 {
                return Err(error(libc::EINVAL));
            }
            let mut inputs = Vec::new();
            for _ in 0..count {
                let fd = receive_fd(stream.as_fd())?;
                if !aim_sync_file::is_sync_file(fd.as_fd()) {
                    return Err(error(libc::EINVAL));
                }
                inputs.push(fd);
            }
            let (file, writer) = aim_sync_file::pair()?;
            let path =
                std::ffi::CString::new(directory.join("fence").as_os_str().as_encoded_bytes())
                    .unwrap();
            let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
            address.sun_family = libc::AF_UNIX as u8;
            if path.as_bytes_with_nul().len() > address.sun_path.len() {
                return Err(error(libc::ENAMETOOLONG));
            }
            unsafe {
                std::ptr::copy_nonoverlapping(
                    path.as_ptr(),
                    address.sun_path.as_mut_ptr(),
                    path.as_bytes_with_nul().len(),
                );
                if libc::bind(
                    file.as_raw_fd(),
                    (&address as *const libc::sockaddr_un).cast(),
                    size_of_val(&address) as u32,
                ) < 0
                {
                    return Err(io::Error::last_os_error());
                }
                if libc::unlink(path.as_ptr()) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            let observer =
                unsafe { BorrowedFd::borrow_raw(writer.as_raw_fd()) }.try_clone_to_owned()?;
            let mut entry = Entry {
                name: super::name32(&name),
                inputs,
                writer: Some(writer),
                observer,
            };
            entry.live();
            *responded = true;
            stream.write_all(&0i32.to_ne_bytes())?;
            send_fd(stream.as_fd(), file.as_fd())?;
            let mut ack = [0];
            stream.read_exact(&mut ack)?;
            entries.insert(context(file.as_fd())?, entry);
        }
        INFO => {
            let file = receive_fd(stream.as_fd())?;
            if !aim_sync_file::is_sync_file(file.as_fd()) {
                return Err(error(libc::EINVAL));
            }
            let entry = entries
                .get(&context(file.as_fd())?)
                .ok_or_else(|| error(libc::ENOENT))?;
            let name = if entry.name[0] == 0 {
                let context = if entry.inputs.len() == 1 {
                    context(entry.inputs[0].as_fd())?
                } else {
                    context(file.as_fd())?
                };
                super::name32(format!("aim-aim{context}-1").as_bytes())
            } else {
                entry.name
            };
            *responded = true;
            stream.write_all(&0i32.to_ne_bytes())?;
            stream.write_all(&name)?;
            stream.write_all(&(entry.inputs.len() as u32).to_ne_bytes())?;
            for fd in &entry.inputs {
                send_fd(stream.as_fd(), fd.as_fd())?;
            }
        }
        _ => return Err(error(libc::EINVAL)),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_merge_lives_until_its_last_alias_closes() {
        let (a, wa) = aim_sync_file::pair().unwrap();
        let (b, _wb) = aim_sync_file::pair().unwrap();
        let m = merge(a.as_fd(), b.as_fd(), super::super::name32(b"lease")).unwrap();
        let path = locator(m.as_fd()).unwrap().unwrap();
        let alias = m.try_clone().unwrap();
        drop((a, b, m));
        assert_eq!(info(alias.as_fd()).unwrap().1.len(), 2);
        assert_eq!(aim_sync_file::state(alias.as_fd()), State::Active);
        let (c, _wc) = aim_sync_file::pair().unwrap();
        let (d, _wd) = aim_sync_file::pair().unwrap();
        let keeper = merge(c.as_fd(), d.as_fd(), super::super::name32(b"keeper")).unwrap();
        drop(alias);
        let deadline = Instant::now() + Duration::from_secs(5);
        let writer = unsafe { BorrowedFd::borrow_raw(wa.as_raw_fd()) };
        while aim_sync_file::state(writer) == State::Active && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            matches!(
                aim_sync_file::state(writer),
                State::Signaled { status: -32, .. }
            ),
            "last close must release inputs while other merges remain live"
        );
        assert_eq!(
            info(keeper.as_fd()).unwrap().0,
            super::super::name32(b"keeper")
        );
        drop((c, d, keeper));
        while path.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !path.exists(),
            "owner must release unsignaled inputs and exit"
        );
    }

    #[test]
    #[ignore]
    fn owner_process() {
        let args: Vec<_> = std::env::args_os().collect();
        let directory = args.last().unwrap();
        aim_storage::inherited::close_above_stdio();
        super::serve(std::path::Path::new(directory)).unwrap();
    }
}
