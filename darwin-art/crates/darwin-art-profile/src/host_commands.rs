//! Host commands for the running system server, the role adb's shell plays
//! for `cmd package install` and friends (ADR 0009). The system server keeps
//! one listening connection to the daemon; a local client's command is
//! relayed over it and the system server's reply returned. Commands run in
//! the system server with AOSP's own shell-command implementations.

use crate::{ProfileError, protocol};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::time::Duration;

/// An install may copy and extract large APKs.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(600);

/// System server not listening.
pub(crate) const STATUS_UNAVAILABLE: u32 = 3;
/// Output bytes per OP_HOST_COMMAND_OUTPUT frame, within the frame limit.
const OUTPUT_CHUNK: usize = 60 * 1024;

/// A command's output as chunk frames, then its status response.
pub(crate) fn write_reply(
    stream: &mut impl Write,
    status: u32,
    output: &[u8],
) -> std::io::Result<()> {
    for chunk in output.chunks(OUTPUT_CHUNK) {
        protocol::write_request(stream, protocol::OP_HOST_COMMAND_OUTPUT, chunk)?;
    }
    protocol::write_response(stream, protocol::OP_HOST_COMMAND, status, b"")
}

/// The chunk frames and status response `write_reply` sends.
pub(crate) fn read_reply(stream: &mut impl Read) -> Result<(u32, Vec<u8>), ProfileError> {
    let mut output = Vec::new();
    loop {
        let message = protocol::read_message(stream)?;
        if message.operation == protocol::OP_HOST_COMMAND_OUTPUT {
            output.extend_from_slice(&message.payload);
        } else if message.operation == protocol::OP_HOST_COMMAND | protocol::RESPONSE_BIT
            && message.payload.len() >= 4
        {
            output.extend_from_slice(&message.payload[4..]);
            return Ok((
                u32::from_le_bytes(message.payload[..4].try_into().unwrap()),
                output,
            ));
        } else {
            return Err(ProfileError::InvalidResponse(
                "bad host command reply".into(),
            ));
        }
    }
}

#[derive(Default)]
pub(crate) struct HostCommands {
    listener: Mutex<Option<UnixStream>>,
}

impl HostCommands {
    /// The system server's connection; it replaces a previous incarnation's.
    pub(crate) fn listen(&self, stream: UnixStream) {
        *self.listener.lock().unwrap() = Some(stream);
    }

    /// Relays `arguments` and returns the system server's (status, output).
    pub(crate) fn run(&self, arguments: &[u8]) -> Result<(u32, Vec<u8>), ProfileError> {
        let mut listener = self.listener.lock().unwrap();
        let Some(stream) = listener.as_mut() else {
            return Ok((
                STATUS_UNAVAILABLE,
                b"the system server is not running".to_vec(),
            ));
        };
        let reply = (|| {
            stream.set_read_timeout(Some(COMMAND_TIMEOUT))?;
            protocol::write_request(stream, protocol::OP_HOST_COMMAND, arguments)?;
            read_reply(stream)
        })();
        if reply.is_err() {
            // A lost or confused system server takes its listening connection
            // with it.
            *listener = None;
        }
        reply
    }
}

/// Client side: runs `arguments` (NUL-separated) in the system server.
fn run_at(socket: &std::path::Path, arguments: &[&str]) -> Result<(u32, Vec<u8>), ProfileError> {
    let mut stream = UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(COMMAND_TIMEOUT + Duration::from_secs(5)))?;
    protocol::write_request(&mut stream, protocol::OP_HOST_COMMAND, &encode(arguments))?;
    read_reply(&mut stream)
}

pub(crate) fn encode(arguments: &[&str]) -> Vec<u8> {
    arguments.join("\0").into_bytes()
}

/// System-server side: waits for the next relayed command.
pub(crate) fn next_command(stream: &mut (impl Read + Write)) -> Result<Vec<u8>, ProfileError> {
    let message = protocol::read_message(stream)?;
    if message.operation != protocol::OP_HOST_COMMAND {
        return Err(ProfileError::InvalidResponse(
            "unexpected host command frame".into(),
        ));
    }
    Ok(message.payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relays_consecutive_commands() {
        let commands = HostCommands::default();
        let (daemon, mut system) = UnixStream::pair().unwrap();
        commands.listen(daemon);
        let server = std::thread::spawn(move || {
            for reply in [b"one".as_slice(), b"two".as_slice()] {
                next_command(&mut system).unwrap();
                write_reply(&mut system, 0, reply).unwrap();
            }
        });
        assert_eq!(commands.run(b"a").unwrap().1, b"one");
        assert_eq!(commands.run(b"b").unwrap().1, b"two");
        server.join().unwrap();
    }

    #[test]
    fn relays_output_larger_than_one_frame() {
        let commands = HostCommands::default();
        let (daemon, mut system) = UnixStream::pair().unwrap();
        commands.listen(daemon);
        let dump = vec![b'x'; 3 * OUTPUT_CHUNK + 17];
        let expected = dump.clone();
        let server = std::thread::spawn(move || {
            next_command(&mut system).unwrap();
            write_reply(&mut system, 0, &dump).unwrap();
            system
        });
        assert_eq!(commands.run(b"dumpsys").unwrap(), (0, expected));
        drop(server.join().unwrap());
    }

    #[test]
    fn relays_a_command_and_its_reply() {
        let commands = HostCommands::default();
        assert_eq!(commands.run(b"x").unwrap().0, STATUS_UNAVAILABLE);
        let (daemon, mut system) = UnixStream::pair().unwrap();
        commands.listen(daemon);
        let server = std::thread::spawn(move || {
            let request = next_command(&mut system).unwrap();
            assert_eq!(request, encode(&["install", "/data/local/tmp/a.apk"]));
            write_reply(&mut system, 0, b"Success\n").unwrap();
            system
        });
        let (status, output) = commands
            .run(&encode(&["install", "/data/local/tmp/a.apk"]))
            .unwrap();
        assert_eq!((status, output.as_slice()), (0, b"Success\n".as_slice()));
        drop(server.join().unwrap());
        // The system server went away: the next command reports it.
        assert!(commands.run(b"x").is_err());
        assert_eq!(commands.run(b"x").unwrap().0, STATUS_UNAVAILABLE);
    }
}

/// The system server's listening connection for relayed host commands.
pub struct HostCommandListener {
    stream: UnixStream,
}

impl HostCommandListener {
    /// Registers with the profile daemon at `socket`; only the profile's
    /// system server child is accepted.
    pub fn connect(socket: &std::path::Path) -> Result<Self, ProfileError> {
        let mut stream = UnixStream::connect(socket)?;
        protocol::write_request(&mut stream, protocol::OP_HOST_COMMAND_LISTEN, b"")?;
        let message = protocol::read_message(&mut stream)?;
        if message.operation != protocol::OP_HOST_COMMAND_LISTEN | protocol::RESPONSE_BIT
            || message.payload.len() < 4
            || message.payload[..4] != [0; 4]
        {
            return Err(ProfileError::Daemon(
                "host command listener was refused".into(),
            ));
        }
        Ok(Self { stream })
    }

    /// The next command's arguments; blocks until one arrives.
    pub fn next(&mut self) -> Result<Vec<String>, ProfileError> {
        let payload = next_command(&mut self.stream)?;
        let text = String::from_utf8(payload)
            .map_err(|_| ProfileError::Daemon("host command is not UTF-8".into()))?;
        Ok(text.split('\0').map(str::to_owned).collect())
    }

    pub fn reply(&mut self, status: u32, output: &[u8]) -> Result<(), ProfileError> {
        write_reply(&mut self.stream, status, output)?;
        Ok(())
    }
}

/// Runs a command in the profile's system server: (exit status, output).
pub fn run_host_command_at(
    socket: &std::path::Path,
    arguments: &[&str],
) -> Result<(u32, Vec<u8>), ProfileError> {
    run_at(socket, arguments)
}
