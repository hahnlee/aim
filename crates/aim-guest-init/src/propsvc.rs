//! The `property_service` sockets on the host.
//!
//! `/dev/socket/property_service` (0666) and
//! `/dev/socket/property_service_for_system` (0660) are Unix sockets at the
//! mapped guest path. One thread per socket accepts connections and reads
//! one request each with init's 5 s timeout (`handle_property_set_fd`);
//! decoded requests go to the boot loop, which owns the property areas and
//! applies them in order with the rules of
//! [`aim_android_init::props::protocol::serve`], exactly as init's
//! property service thread hands work to its main loop. Control messages
//! are answered when the main loop has handled them.

use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use aim_android_init::props::protocol::{
    Decoded, PROP_SERVICE_FOR_SYSTEM_NAME, PROP_SERVICE_NAME, Request, SOCKET_TIMEOUT_MS,
    decode_request, encode_reply, truncated_reply,
};

/// How often an idle socket thread looks at its stop flag.
const STOP_POLL_MS: i32 = 100;

/// One decoded request waiting for the main loop.
#[derive(Debug)]
pub struct SetRequest {
    pub request: Request,
    /// Kept open for the reply.
    pub stream: UnixStream,
    /// Host pid of the peer (`LOCAL_PEERPID`).
    pub peer_pid: i32,
    /// Host uid of the peer (`getpeereid`).
    pub peer_host_uid: u32,
    /// Which socket it came in on.
    pub socket: &'static str,
}

impl SetRequest {
    /// Sends the status word and closes (init's `SocketConnection::SendUint32`).
    pub fn reply(mut self, code: u32) {
        let _ = self.stream.write_all(&encode_reply(code));
    }
}

/// Events the socket threads and the child watcher send to the boot loop.
#[derive(Debug)]
pub enum PropertyEvent {
    Set(SetRequest),
    /// SIGCHLD: a child may have exited; the loop reaps it.
    ChildExited,
}

/// The listening sockets and their threads.
pub struct PropertySockets {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    paths: Vec<PathBuf>,
}

/// `CreateSocket(..., should_listen=true, ...)` with init's backlog of 8.
fn bind(dir: &Path, name: &str, mode: u32) -> io::Result<UnixListener> {
    use std::os::unix::fs::PermissionsExt;
    let fd = crate::unixsock::bind_at(dir, name, libc::SOCK_STREAM)?;
    std::fs::set_permissions(dir.join(name), std::fs::Permissions::from_mode(mode))?;
    // SAFETY: listen on our socket.
    if unsafe { libc::listen(fd.as_raw_fd(), 8) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(UnixListener::from(fd))
}

fn peer_pid(stream: &UnixStream) -> i32 {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: LOCAL_PEERPID writes a pid_t.
    let r = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut len,
        )
    };
    if r == 0 { pid } else { -1 }
}

fn peer_uid(stream: &UnixStream) -> u32 {
    let (mut uid, mut gid) = (0, 0);
    // SAFETY: getpeereid on a connected Unix socket.
    unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
    uid
}

/// Reads one request the way init does: until it decodes, the peer
/// closes, or 5 s pass. Returns the request, or answers and drops the
/// connection itself.
fn read_request(mut stream: UnixStream) -> Option<(Request, UnixStream)> {
    let deadline = Instant::now() + Duration::from_millis(SOCKET_TIMEOUT_MS as u64);
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match decode_request(&bytes) {
            Decoded::Complete(request, _) => return Some((request, stream)),
            Decoded::Reject(code) => {
                let _ = stream.write_all(&encode_reply(code));
                return None;
            }
            Decoded::Incomplete => {}
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() || stream.set_read_timeout(Some(remaining)).is_err() {
            break;
        }
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => bytes.extend_from_slice(&buffer[..n]),
        }
    }
    if let Some(code) = truncated_reply(&bytes) {
        let _ = stream.write_all(&encode_reply(code));
    }
    None
}

impl PropertySockets {
    /// Binds both sockets in `socket_dir` (the host directory behind
    /// `/dev/socket`) and starts their threads.
    pub fn start(socket_dir: &Path, events: Sender<PropertyEvent>) -> io::Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();
        let mut paths = Vec::new();
        for (name, mode) in [
            (PROP_SERVICE_NAME, 0o666),
            (PROP_SERVICE_FOR_SYSTEM_NAME, 0o660),
        ] {
            let path = socket_dir.join(name);
            let listener = bind(socket_dir, name, mode)?;
            listener.set_nonblocking(true)?;
            paths.push(path);
            let stop = stop.clone();
            let events = events.clone();
            threads.push(std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            let pid = peer_pid(&stream);
                            let uid = peer_uid(&stream);
                            let events = events.clone();
                            // A slow client must not block the next one.
                            std::thread::spawn(move || {
                                if let Some((request, stream)) = read_request(stream) {
                                    let _ = events.send(PropertyEvent::Set(SetRequest {
                                        request,
                                        stream,
                                        peer_pid: pid,
                                        peer_host_uid: uid,
                                        socket: name,
                                    }));
                                }
                            });
                        }
                        // Wait for the next connection, looking at `stop`
                        // now and then.
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            let mut p = libc::pollfd {
                                fd: listener.as_raw_fd(),
                                events: libc::POLLIN,
                                revents: 0,
                            };
                            // SAFETY: one pollfd on our stack.
                            unsafe { libc::poll(&mut p, 1, STOP_POLL_MS) };
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            }));
        }
        Ok(Self {
            stop,
            threads,
            paths,
        })
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }
}

impl Drop for PropertySockets {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// bionic's `__system_property_set` client, for tests and tools: sends one
/// `PROP_MSG_SETPROP2` and returns the status word.
pub fn client_set(socket: &Path, name: &str, value: &str) -> io::Result<u32> {
    let dir = socket.parent().ok_or(io::ErrorKind::InvalidInput)?;
    let file = socket
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(io::ErrorKind::InvalidInput)?;
    let mut stream = crate::unixsock::connect_at(dir, file)?;
    stream.write_all(&aim_android_init::props::protocol::encode_setprop2(
        name.as_bytes(),
        value.as_bytes(),
    ))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply)?;
    Ok(u32::from_ne_bytes(reply))
}
