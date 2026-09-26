//! Host commands for the running system server, the role adb's shell plays
//! for `cmd package install` and friends (ADR 0009). The system server keeps
//! a few listening connections to the daemon, each served by its own thread;
//! a local client's command is relayed over an idle one and the system
//! server's reply returned, so a long command (an install) does not hold up
//! the others, as each adb shell command is its own process. Commands run in
//! the system server with AOSP's own shell-command implementations.

use crate::{ProfileError, protocol};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::{Condvar, Mutex};
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
    listeners: Mutex<Listeners>,
    idle: Condvar,
}

#[derive(Default)]
struct Listeners {
    /// The system server incarnation the connections belong to.
    owner: Option<u32>,
    /// Bumped when a new incarnation replaces the connections, so a
    /// connection taken from the old one is not returned to the pool.
    generation: u64,
    idle: Vec<UnixStream>,
    busy: usize,
}

impl HostCommands {
    /// One of the system server's connections. A new system server
    /// incarnation's first connection replaces a previous incarnation's.
    pub(crate) fn listen(&self, owner: u32, stream: UnixStream) {
        let mut listeners = self.listeners.lock().unwrap();
        if listeners.owner != Some(owner) {
            listeners.owner = Some(owner);
            listeners.generation += 1;
            listeners.idle.clear();
            listeners.busy = 0;
        }
        listeners.idle.push(stream);
        drop(listeners);
        self.idle.notify_one();
    }

    /// Relays `arguments` over an idle connection, waiting for one while all
    /// are running commands, and returns the system server's (status, output).
    pub(crate) fn run(&self, arguments: &[u8]) -> Result<(u32, Vec<u8>), ProfileError> {
        let mut listeners = self.listeners.lock().unwrap();
        let (mut stream, generation) = loop {
            if listeners.idle.is_empty() && listeners.busy == 0 {
                return Ok((
                    STATUS_UNAVAILABLE,
                    b"the system server is not running".to_vec(),
                ));
            }
            if let Some(stream) = listeners.idle.pop() {
                listeners.busy += 1;
                break (stream, listeners.generation);
            }
            let (next, timeout) = self
                .idle
                .wait_timeout(listeners, COMMAND_TIMEOUT)
                .unwrap();
            listeners = next;
            if timeout.timed_out() && listeners.idle.is_empty() {
                return Ok((
                    STATUS_UNAVAILABLE,
                    b"every host command connection stayed busy".to_vec(),
                ));
            }
        };
        drop(listeners);
        let reply = (|| {
            stream.set_read_timeout(Some(COMMAND_TIMEOUT))?;
            protocol::write_request(&mut stream, protocol::OP_HOST_COMMAND, arguments)?;
            read_reply(&mut stream)
        })();
        let mut listeners = self.listeners.lock().unwrap();
        if listeners.generation == generation {
            listeners.busy -= 1;
            // A lost or confused system server takes that connection with it.
            if reply.is_ok() {
                listeners.idle.push(stream);
            }
        }
        drop(listeners);
        self.idle.notify_one();
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
        commands.listen(1, daemon);
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
        commands.listen(1, daemon);
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
    fn a_long_command_does_not_block_another_connection() {
        let commands = std::sync::Arc::new(HostCommands::default());
        let (release, released) = std::sync::mpsc::channel::<()>();
        let released = std::sync::Arc::new(Mutex::new(released));
        let mut servers = Vec::new();
        for _ in 0..2 {
            let (daemon, mut system) = UnixStream::pair().unwrap();
            commands.listen(1, daemon);
            let released = released.clone();
            // Each connection's thread: an install waits to be released.
            servers.push(std::thread::spawn(move || {
                if let Ok(command) = next_command(&mut system) {
                    if command == b"install" {
                        released.lock().unwrap().recv().unwrap();
                    }
                    write_reply(&mut system, 0, &command).unwrap();
                }
                system
            }));
        }
        let long = {
            let commands = commands.clone();
            std::thread::spawn(move || commands.run(b"install").unwrap())
        };
        std::thread::sleep(Duration::from_millis(100));
        // The install holds one connection; the other answers meanwhile.
        assert_eq!(commands.run(b"list").unwrap().1, b"list");
        release.send(()).unwrap();
        assert_eq!(long.join().unwrap().1, b"install");
        for server in servers {
            drop(server.join().unwrap());
        }
    }

    #[test]
    fn a_new_incarnation_replaces_the_connections() {
        let commands = HostCommands::default();
        let (old, _old_server) = UnixStream::pair().unwrap();
        commands.listen(1, old);
        let (daemon, mut system) = UnixStream::pair().unwrap();
        commands.listen(2, daemon);
        let server = std::thread::spawn(move || {
            next_command(&mut system).unwrap();
            write_reply(&mut system, 0, b"new").unwrap();
            system
        });
        assert_eq!(commands.run(b"x").unwrap().1, b"new");
        drop(server.join().unwrap());
    }

    #[test]
    fn relays_a_command_and_its_reply() {
        let commands = HostCommands::default();
        assert_eq!(commands.run(b"x").unwrap().0, STATUS_UNAVAILABLE);
        let (daemon, mut system) = UnixStream::pair().unwrap();
        commands.listen(1, daemon);
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
