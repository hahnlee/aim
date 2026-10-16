//! The native ActivityManager's zygote client (docs/app-host.md). Zygote
//! takes commands only from the system uid (`ZygoteConnection`), as
//! system_server's `ZygoteProcess` sends them; the native ActivityManager
//! runs on the host, so this system daemon sends them for it.
//!
//! init passes it the listening socket `aim_zygote_relay`. Each command
//! the host writes there, in zygote's own framing (the argument count,
//! then each argument, one per line: `ZygoteProcess
//! .attemptZygoteSendArgsAndGetResult`), goes to zygote over one kept
//! session, and zygote's reply (the pid, a big-endian `int`, and whether
//! a wrapper started it, a `boolean`) goes back unchanged.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{FromRawFd, RawFd};
use std::os::unix::net::{UnixListener, UnixStream};

/// init's socket for this service (`socket aim_zygote_relay` in its rc).
const SOCKET: &str = "ANDROID_SOCKET_aim_zygote_relay";
/// The primary zygote's command socket.
const ZYGOTE: &str = "/dev/socket/zygote";

fn main() {
    daemon_log::init("aim-zygote-relay");
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
