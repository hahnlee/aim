//! The app host's agent (docs/app-host.md): what the native services on
//! the host need done by a guest process of the system uid.
//!
//! - Zygote takes commands only from the system uid (`ZygoteConnection`),
//!   as system_server's `ZygoteProcess` sends them. init passes the agent
//!   the listening socket `aim_zygote_relay`; each command the host writes
//!   there, in zygote's own framing (the argument count, then each
//!   argument, one per line: `attemptZygoteSendArgsAndGetResult`), goes to
//!   zygote over one kept session, and zygote's reply (the pid, a
//!   big-endian `int`, and whether a wrapper started it, a `boolean`; an
//!   `int` alone for `--preload-app`) goes back unchanged. A connection
//!   whose first line is `@` and a name talks to the child zygote listening
//!   on that abstract socket instead (`ZygoteProcess.startChildZygote`'s,
//!   as `ChildZygoteProcess` does).
//! - A window's input channel is a guest socket pair, as
//!   `InputChannel::openInputChannelPair` makes it: the agent makes it,
//!   hands the client's end to the host (`dev.aim.IAppHostAgent`, which it
//!   attaches to `aim.app_host`) and publishes the window's input on the
//!   server's (`input`).
//! - It kills the process of a task the host removed, as system_server
//!   does (`ProcessRecord.killLocked`: `Process.killProcessQuiet`).
//! - It mounts a user's emulated storage through vold and serves its FUSE
//!   connection (`storage`).
//! - It plays the sound effects AudioService plays (`effects`).
//! - It reads a thread's `/proc/<tid>/status` for the host, as
//!   system_server reads it (`Process.readProcLines`).

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;

use binder::binder_impl::{
    AssociateClass, BorrowedParcel, IBinderInternal, Remotable, TransactionCode,
};
use binder::{
    BinderFeatures, Interface, ParcelFileDescriptor, Status, StatusCode, declare_binder_interface,
};

mod effects;
mod input;
mod storage;

/// init's socket for this service (`socket aim_zygote_relay` in its rc).
const SOCKET: &str = "ANDROID_SOCKET_aim_zygote_relay";
/// The primary zygote's command socket.
const ZYGOTE: &str = "/dev/socket/zygote";
/// `ZygoteProcess.ZYGOTE_CONNECT_TIMEOUT_MS`, `ZYGOTE_CONNECT_RETRY_DELAY_MS`:
/// zygote listens once it has preloaded.
const ZYGOTE_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const ZYGOTE_CONNECT_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(50);
/// The app host's control service, and its `attachAgent(IBinder)`.
const APP_HOST: &str = "aim.app_host";
const ATTACH_AGENT: u32 = 2;
/// `IAppHostAgent.openInputChannel(String name, int displayWidth, int
/// displayHeight, int left, int top, int right, int bottom)`: the
/// channel's index and the client's end; `focusInputChannel(int index)`;
/// `setInputChannelFrame(int index, int left, int top, int right, int
/// bottom)`.
const OPEN_INPUT_CHANNEL: u32 = 1;
const FOCUS_INPUT_CHANNEL: u32 = 2;
const SET_INPUT_CHANNEL_FRAME: u32 = 3;
/// `IAppHostAgent.killProcess(int pid)`, `mountStorage(int userId, int
/// daemonUid)`.
const KILL_PROCESS: u32 = 4;
const MOUNT_STORAGE: u32 = 5;
/// `IAppHostAgent.playSoundEffect(String path, float gain)`.
const PLAY_SOUND_EFFECT: u32 = 6;
/// `IAppHostAgent.injectMotion(int action, long downTimeNanos, long
/// eventTimeNanos, float x, float y, float pressure)`: a one-finger
/// touchscreen motion in the display's pixels.
const INJECT_MOTION: u32 = 7;
/// `IAppHostAgent.readSmaps(int pid)`: the process's `/proc/<pid>/smaps`,
/// null when it cannot be opened (the process is gone).
const READ_SMAPS: u32 = 8;
/// `IAppHostAgent.readProcStatus(int tid)`: the first 4095 bytes of
/// `/proc/<tid>/status`, read once as `android_os_Process_readProcLines`
/// reads it; null when it cannot be opened.
const READ_PROC_STATUS: u32 = 9;
/// `InputTransport.cpp`'s `SOCKET_BUFFER_SIZE`.
const SOCKET_BUFFER_SIZE: libc::c_int = 32 * 1024;

fn main() {
    daemon_log::init("aim-app-host-agent");
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(storage::SERVE) {
        return storage::serve_main(args.get(2).map_or("/data/media", String::as_str));
    }
    let fd: RawFd = match std::env::var(SOCKET).ok().and_then(|v| v.parse().ok()) {
        Some(fd) => fd,
        None => {
            log::error!("{SOCKET} is not set: not started by init");
            std::process::exit(1);
        }
    };
    // init binds the socket and leaves listening to the service, as
    // `android_get_control_socket`'s users do.
    // SAFETY: a socket descriptor init passed, owned by nothing else in
    // this process.
    if unsafe { libc::listen(fd, 4) } != 0 {
        log::error!("listen: {}", std::io::Error::last_os_error());
        std::process::exit(1);
    }
    let listener = unsafe { UnixListener::from_raw_fd(fd) };
    binder::ProcessState::start_thread_pool();
    std::thread::spawn(attach);
    for host in listener.incoming() {
        match host {
            Ok(host) => {
                std::thread::spawn(move || relay(host));
            }
            // init starts it again.
            Err(error) => {
                log::error!("accept: {error}");
                std::process::exit(1);
            }
        }
    }
}

/// Relays the commands of one host connection until it closes.
fn relay(host: UnixStream) {
    let mut reply_to = match host.try_clone() {
        Ok(stream) => stream,
        Err(error) => return log::error!("host connection: {error}"),
    };
    let mut commands = BufReader::new(host);
    let mut zygote: Option<UnixStream> = None;
    let mut child: Option<String> = None;
    if commands
        .fill_buf()
        .map(|b| b.first() == Some(&b'@'))
        .unwrap_or(false)
    {
        let mut line = String::new();
        if let Err(error) = commands.read_line(&mut line) {
            return log::error!("host target: {error}");
        }
        child = Some(line[1..].trim_end().to_string());
    }
    loop {
        let command = match read_command(&mut commands) {
            Ok(Some(command)) => command,
            Ok(None) => return,
            Err(error) => return log::error!("host command: {error}"),
        };
        if zygote.is_none() {
            match connect_zygote(child.as_deref()) {
                Ok(stream) => zygote = Some(stream),
                Err(error) => {
                    return log::error!("{}: {error}", child.as_deref().unwrap_or(ZYGOTE));
                }
            }
        }
        let session = zygote.as_mut().unwrap();
        // `preloadApp` reads an `int`; a start, the pid and a `boolean`.
        let preload = command.split(|&b| b == b'\n').nth(1) == Some(b"--preload-app");
        let mut reply = [0u8; 5];
        let reply = if preload {
            &mut reply[..4]
        } else {
            &mut reply[..]
        };
        if let Err(error) = session
            .write_all(&command)
            .and_then(|()| session.read_exact(reply))
        {
            // The host sees its connection close, as ZygoteProcess sees
            // the zygote's.
            return log::error!("zygote session: {error}");
        }
        if let Err(error) = reply_to.write_all(reply) {
            return log::error!("host reply: {error}");
        }
    }
}

/// A session with zygote, or the child zygote on abstract socket `child`,
/// waiting for it to listen as `ZygoteProcess.waitForConnectionToZygote`
/// does.
fn connect_zygote(child: Option<&str>) -> std::io::Result<UnixStream> {
    use std::os::android::net::SocketAddrExt;
    let deadline = std::time::Instant::now() + ZYGOTE_CONNECT_TIMEOUT;
    loop {
        let connected = match child {
            Some(name) => std::os::unix::net::SocketAddr::from_abstract_name(name)
                .and_then(|a| UnixStream::connect_addr(&a)),
            None => UnixStream::connect(ZYGOTE),
        };
        match connected {
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                ) && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(ZYGOTE_CONNECT_RETRY_DELAY)
            }
            result => return result,
        }
    }
}

/// One command, as framed: `None` when the host closed the connection
/// before one began.
fn read_command(from: &mut impl BufRead) -> std::io::Result<Option<Vec<u8>>> {
    let mut line = String::new();
    if from.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let count: usize = line
        .trim_end()
        .parse()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "argument count"))?;
    let mut command = line.into_bytes();
    for _ in 0..count {
        let mut argument = Vec::new();
        if from.read_until(b'\n', &mut argument)? == 0 || argument.last() != Some(&b'\n') {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        command.extend_from_slice(&argument);
    }
    Ok(Some(command))
}

/// The app host's control service, only called.
pub trait IAppHost: Interface {}

declare_binder_interface! {
    IAppHost["dev.aim.IAppHost"] {
        native: BnAppHost(no_calls),
        proxy: BpAppHost,
    }
}

impl IAppHost for BpAppHost {}
impl IAppHost for binder::binder_impl::Binder<BnAppHost> {}

fn no_calls(
    _: &dyn IAppHost,
    _: TransactionCode,
    _: &BorrowedParcel<'_>,
    _: &mut BorrowedParcel<'_>,
) -> Result<(), StatusCode> {
    Err(StatusCode::UNKNOWN_TRANSACTION)
}

pub trait IAppHostAgent: Interface {
    fn open_input_channel(
        &self,
        name: &str,
        display: (i32, i32),
        region: input::Region,
    ) -> std::io::Result<(i32, OwnedFd)>;
    fn focus_input_channel(&self, index: i32) -> bool;
    fn set_input_channel_frame(&self, index: i32, region: input::Region) -> bool;
    fn play_sound_effect(&self, path: &str, gain: f32) -> Result<(), String>;
    fn inject_motion(&self, motion: &aim_input_transport::Motion);
}

declare_binder_interface! {
    IAppHostAgent["dev.aim.IAppHostAgent"] {
        native: BnAppHostAgent(on_transact),
        proxy: BpAppHostAgent,
    }
}

impl IAppHostAgent for BpAppHostAgent {
    fn open_input_channel(
        &self,
        _: &str,
        _: (i32, i32),
        _: input::Region,
    ) -> std::io::Result<(i32, OwnedFd)> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
    fn focus_input_channel(&self, _: i32) -> bool {
        false
    }
    fn set_input_channel_frame(&self, _: i32, _: input::Region) -> bool {
        false
    }
    fn play_sound_effect(&self, _: &str, _: f32) -> Result<(), String> {
        Err("not the agent".into())
    }
    fn inject_motion(&self, _: &aim_input_transport::Motion) {}
}

impl IAppHostAgent for binder::binder_impl::Binder<BnAppHostAgent> {
    fn open_input_channel(
        &self,
        name: &str,
        display: (i32, i32),
        region: input::Region,
    ) -> std::io::Result<(i32, OwnedFd)> {
        self.0.open_input_channel(name, display, region)
    }
    fn focus_input_channel(&self, index: i32) -> bool {
        self.0.focus_input_channel(index)
    }
    fn set_input_channel_frame(&self, index: i32, region: input::Region) -> bool {
        self.0.set_input_channel_frame(index, region)
    }
    fn play_sound_effect(&self, path: &str, gain: f32) -> Result<(), String> {
        self.0.play_sound_effect(path, gain)
    }
    fn inject_motion(&self, motion: &aim_input_transport::Motion) {
        self.0.inject_motion(motion)
    }
}

fn on_transact(
    agent: &dyn IAppHostAgent,
    code: TransactionCode,
    data: &BorrowedParcel<'_>,
    reply: &mut BorrowedParcel<'_>,
) -> Result<(), StatusCode> {
    match code {
        OPEN_INPUT_CHANNEL => {
            let name: String = data.read()?;
            let display: (i32, i32) = (data.read()?, data.read()?);
            let region = read_region(data)?;
            match agent.open_input_channel(&name, display, region) {
                Ok((index, client)) => {
                    reply.write(&Status::ok())?;
                    reply.write(&index)?;
                    reply.write(&ParcelFileDescriptor::new(client))?;
                }
                Err(error) => {
                    log::error!("input channel {name}: {error}");
                    reply.write(&Status::new_exception_str(
                        binder::ExceptionCode::ILLEGAL_STATE,
                        Some(error.to_string()),
                    ))?;
                }
            }
            Ok(())
        }
        FOCUS_INPUT_CHANNEL | SET_INPUT_CHANNEL_FRAME => {
            let index: i32 = data.read()?;
            let known = if code == FOCUS_INPUT_CHANNEL {
                agent.focus_input_channel(index)
            } else {
                agent.set_input_channel_frame(index, read_region(data)?)
            };
            reply.write(&if known {
                Status::ok()
            } else {
                Status::new_exception_str(
                    binder::ExceptionCode::ILLEGAL_ARGUMENT,
                    Some(format!("no input channel {index}")),
                )
            })?;
            Ok(())
        }
        MOUNT_STORAGE => {
            let (user, daemon_uid): (i32, i32) = (data.read()?, data.read()?);
            reply.write(&match storage::mount(user, daemon_uid as u32) {
                Ok(()) => Status::ok(),
                Err(error) => {
                    log::error!("{error}");
                    Status::new_exception_str(binder::ExceptionCode::ILLEGAL_STATE, Some(error))
                }
            })?;
            Ok(())
        }
        PLAY_SOUND_EFFECT => {
            let (path, gain): (String, f32) = (data.read()?, data.read()?);
            reply.write(&match agent.play_sound_effect(&path, gain) {
                Ok(()) => Status::ok(),
                Err(error) => {
                    log::error!("{error}");
                    Status::new_exception_str(binder::ExceptionCode::ILLEGAL_STATE, Some(error))
                }
            })?;
            Ok(())
        }
        INJECT_MOTION => {
            let action: i32 = data.read()?;
            let (down_time, time): (i64, i64) = (data.read()?, data.read()?);
            let (x, y, pressure): (f32, f32, f32) = (data.read()?, data.read()?, data.read()?);
            reply.write(&match aim_input_transport::Motion::injected(
                action, down_time, time, x, y, pressure,
            ) {
                Some(motion) => {
                    agent.inject_motion(&motion);
                    Status::ok()
                }
                None => Status::new_exception_str(
                    binder::ExceptionCode::ILLEGAL_ARGUMENT,
                    Some(format!("injected action {action}")),
                ),
            })?;
            Ok(())
        }
        READ_PROC_STATUS => {
            let tid: i32 = data.read()?;
            reply.write(&Status::ok())?;
            reply.write(&proc_status(tid))?;
            Ok(())
        }
        KILL_PROCESS => {
            let pid: i32 = data.read()?;
            // SAFETY: a signal to a process id.
            let status = if pid > 0 && unsafe { libc::kill(pid, libc::SIGKILL) } == 0 {
                Status::ok()
            } else {
                let error = std::io::Error::last_os_error();
                Status::new_exception_str(
                    binder::ExceptionCode::ILLEGAL_STATE,
                    Some(format!("kill {pid}: {error}")),
                )
            };
            reply.write(&status)?;
            Ok(())
        }
        // Read as system_server reads it for `Debug.getMemoryInfo`, with
        // CAP_SYS_PTRACE.
        READ_SMAPS => {
            let pid: i32 = data.read()?;
            let smaps = std::fs::read_to_string(format!("/proc/{pid}/smaps")).ok();
            reply.write(&Status::ok())?;
            reply.write(&smaps)?;
            Ok(())
        }
        _ => Err(StatusCode::UNKNOWN_TRANSACTION),
    }
}

/// `/proc/<tid>/status` as one `read` of up to 4095 bytes gets it: none
/// when it cannot be opened, nothing when the read fails.
fn proc_status(tid: i32) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(format!("/proc/{tid}/status")).ok()?;
    let mut buffer = vec![0; 4095];
    let len = file.read(&mut buffer).unwrap_or(0);
    buffer.truncate(len);
    Some(buffer)
}

/// A rectangle's left, top, right and bottom.
fn read_rect(data: &BorrowedParcel<'_>) -> Result<[i32; 4], StatusCode> {
    Ok([data.read()?, data.read()?, data.read()?, data.read()?])
}

/// A window's frame, the region it takes touches in, whether it watches
/// touches outside it, and its owner's uid.
fn read_region(data: &BorrowedParcel<'_>) -> Result<input::Region, StatusCode> {
    Ok(input::Region {
        frame: read_rect(data)?,
        touchable: read_rect(data)?,
        watch_outside: data.read()?,
        owner: data.read()?,
    })
}

/// The agent: its windows' input.
struct Agent {
    input: Arc<input::Dispatcher>,
    channels: std::sync::atomic::AtomicI32,
    effects: Arc<effects::Effects>,
}

impl Interface for Agent {}

impl IAppHostAgent for Agent {
    /// `InputChannel::openInputChannelPair`: a sequenced-packet socket
    /// pair with 32 KiB buffers each way; the window, the latest, comes to
    /// the front and takes the focus.
    fn open_input_channel(
        &self,
        _name: &str,
        display: (i32, i32),
        region: input::Region,
    ) -> std::io::Result<(i32, OwnedFd)> {
        let mut pair = [0; 2];
        // SAFETY: a socket pair into a local array; both ends are owned below.
        if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET, 0, pair.as_mut_ptr()) }
            != 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let (server, client) =
            unsafe { (OwnedFd::from_raw_fd(pair[0]), OwnedFd::from_raw_fd(pair[1])) };
        for fd in [&server, &client] {
            for option in [libc::SO_SNDBUF, libc::SO_RCVBUF] {
                // SAFETY: an int option of a socket this function owns.
                unsafe {
                    libc::setsockopt(
                        std::os::fd::AsRawFd::as_raw_fd(fd),
                        libc::SOL_SOCKET,
                        option,
                        (&SOCKET_BUFFER_SIZE as *const libc::c_int).cast(),
                        std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                    )
                };
            }
        }
        let index = self
            .channels
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.input.add_window(index, server, display, region);
        Ok((index, client))
    }

    fn focus_input_channel(&self, index: i32) -> bool {
        self.input.focus_window(index)
    }

    fn set_input_channel_frame(&self, index: i32, region: input::Region) -> bool {
        self.input.set_region(index, region)
    }

    fn play_sound_effect(&self, path: &str, gain: f32) -> Result<(), String> {
        self.effects.play(path, gain)
    }
    fn inject_motion(&self, motion: &aim_input_transport::Motion) {
        self.input.inject(motion)
    }
}

/// Attaches the agent to the app host, which waits for it.
fn attach() {
    let agent = BnAppHostAgent::new_binder(
        Agent {
            input: input::Dispatcher::new(),
            channels: Default::default(),
            effects: Default::default(),
        },
        BinderFeatures::default(),
    );
    let Some(mut host) = binder::wait_for_service(APP_HOST) else {
        return log::error!("{APP_HOST}: not found");
    };
    if !host.associate_class(BnAppHost::get_class()) {
        return log::error!("{APP_HOST} is not a dev.aim.IAppHost");
    }
    let result = (|| -> Result<(), Status> {
        let mut data = host.prepare_transact()?;
        data.write(&agent.as_binder())?;
        let reply = host.submit_transact(ATTACH_AGENT, data, 0)?;
        let status: Status = reply.read()?;
        if status.is_ok() { Ok(()) } else { Err(status) }
    })();
    match result {
        Ok(()) => log::info!("attached to {APP_HOST}"),
        Err(error) => log::error!("attach to {APP_HOST}: {error}"),
    }
    // The agent lives as long as the process: the host holds it.
    std::mem::forget(agent);
}
