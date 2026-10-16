//! The app host's agent (docs/app-host.md): what the native services on
//! the host need done by a guest process of the system uid.
//!
//! - Zygote takes commands only from the system uid (`ZygoteConnection`),
//!   as system_server's `ZygoteProcess` sends them. init passes the agent
//!   the listening socket `aim_zygote_relay`; each command the host writes
//!   there, in zygote's own framing (the argument count, then each
//!   argument, one per line: `attemptZygoteSendArgsAndGetResult`), goes to
//!   zygote over one kept session, and zygote's reply (the pid, a
//!   big-endian `int`, and whether a wrapper started it, a `boolean`) goes
//!   back unchanged.
//! - A window's input channel is a guest socket pair, as
//!   `InputChannel::openInputChannelPair` makes it: the agent makes it,
//!   hands the client's end to the host (`dev.aim.IAppHostAgent`, which it
//!   attaches to `aim.app_host`) and keeps the server's.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Mutex;

use binder::binder_impl::{AssociateClass, BorrowedParcel, IBinderInternal, Remotable, TransactionCode};
use binder::{BinderFeatures, Interface, ParcelFileDescriptor, Status, StatusCode, declare_binder_interface};

/// init's socket for this service (`socket aim_zygote_relay` in its rc).
const SOCKET: &str = "ANDROID_SOCKET_aim_zygote_relay";
/// The primary zygote's command socket.
const ZYGOTE: &str = "/dev/socket/zygote";
/// The app host's control service, and its `attachAgent(IBinder)`.
const APP_HOST: &str = "aim.app_host";
const ATTACH_AGENT: u32 = 2;
/// `IAppHostAgent.openInputChannel(String name)`: the channel's index and
/// the client's end.
const OPEN_INPUT_CHANNEL: u32 = 1;
/// `InputTransport.cpp`'s `SOCKET_BUFFER_SIZE`.
const SOCKET_BUFFER_SIZE: libc::c_int = 32 * 1024;

fn main() {
    daemon_log::init("aim-app-host-agent");
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
    loop {
        let command = match read_command(&mut commands) {
            Ok(Some(command)) => command,
            Ok(None) => return,
            Err(error) => return log::error!("host command: {error}"),
        };
        if zygote.is_none() {
            match UnixStream::connect(ZYGOTE) {
                Ok(stream) => zygote = Some(stream),
                Err(error) => return log::error!("{ZYGOTE}: {error}"),
            }
        }
        let session = zygote.as_mut().unwrap();
        let mut reply = [0u8; 5];
        if let Err(error) = session
            .write_all(&command)
            .and_then(|()| session.read_exact(&mut reply))
        {
            // The host sees its connection close, as ZygoteProcess sees
            // the zygote's.
            return log::error!("zygote session: {error}");
        }
        if let Err(error) = reply_to.write_all(&reply) {
            return log::error!("host reply: {error}");
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
    let count: usize = line.trim_end().parse().map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "argument count")
    })?;
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

fn no_calls(_: &dyn IAppHost, _: TransactionCode, _: &BorrowedParcel<'_>, _: &mut BorrowedParcel<'_>) -> Result<(), StatusCode> {
    Err(StatusCode::UNKNOWN_TRANSACTION)
}

pub trait IAppHostAgent: Interface {
    fn open_input_channel(&self, name: &str) -> std::io::Result<(i32, OwnedFd)>;
}

declare_binder_interface! {
    IAppHostAgent["dev.aim.IAppHostAgent"] {
        native: BnAppHostAgent(on_transact),
        proxy: BpAppHostAgent,
    }
}

impl IAppHostAgent for BpAppHostAgent {
    fn open_input_channel(&self, _: &str) -> std::io::Result<(i32, OwnedFd)> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

impl IAppHostAgent for binder::binder_impl::Binder<BnAppHostAgent> {
    fn open_input_channel(&self, name: &str) -> std::io::Result<(i32, OwnedFd)> {
        self.0.open_input_channel(name)
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
            match agent.open_input_channel(&name) {
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
        _ => Err(StatusCode::UNKNOWN_TRANSACTION),
    }
}

/// The input channels' server ends, by index.
struct Agent {
    channels: Mutex<Vec<OwnedFd>>,
}

impl Interface for Agent {}

impl IAppHostAgent for Agent {
    /// `InputChannel::openInputChannelPair`: a sequenced-packet socket
    /// pair with 32 KiB buffers each way.
    fn open_input_channel(&self, _name: &str) -> std::io::Result<(i32, OwnedFd)> {
        let mut pair = [0; 2];
        // SAFETY: a socket pair into a local array; both ends are owned below.
        if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET, 0, pair.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let (server, client) = unsafe { (OwnedFd::from_raw_fd(pair[0]), OwnedFd::from_raw_fd(pair[1])) };
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
        let mut channels = self.channels.lock().unwrap();
        channels.push(server);
        Ok(((channels.len() - 1) as i32, client))
    }
}

/// Attaches the agent to the app host, which waits for it.
fn attach() {
    let agent = BnAppHostAgent::new_binder(Agent { channels: Mutex::new(Vec::new()) }, BinderFeatures::default());
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
