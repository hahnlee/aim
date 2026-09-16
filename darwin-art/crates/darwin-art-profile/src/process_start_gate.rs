//! One-shot Darwin launch ordering. No Android credential or service policy.
//! Child runtime initialization starts only after its parent registers the PID.
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};

const VARIABLE: &str = "DARWIN_ART_PROCESS_START_FD";
const READY: u8 = 0x71;

pub(crate) struct StartGate {
    parent: UnixStream,
    child: UnixStream,
}

impl StartGate {
    pub(crate) fn prepare(command: &mut Command) -> io::Result<Self> {
        let (parent, child) = UnixStream::pair()?;
        parent.set_write_timeout(Some(Duration::from_secs(30)))?;
        let fd = child.as_raw_fd();
        command.env(VARIABLE, fd.to_string());
        // SAFETY: the post-fork hook performs only async-signal-safe fcntl.
        // The parent keeps the descriptor CLOEXEC; only this child inherits it.
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(Self { parent, child })
    }

    pub(crate) fn release(mut self) -> io::Result<()> {
        // The exec child has its own inherited reference. This local reference
        // must not disguise an early child exit from the parent writer.
        drop(self.child);
        self.parent.write_all(&[READY])
    }
}

/// Consume the profile supervisor's inherited startup channel, if present.
/// Call before starting runtime threads or querying the registered identity.
/// Absence is valid for direct/test processes without a supervisor.
///
/// # Safety
/// Call during single-threaded process startup, before any other environment
/// readers/writers. The consumed descriptor variable is removed before exec
/// children can inherit a stale descriptor number.
pub unsafe fn wait_for_process_registration() -> io::Result<()> {
    let Some(value) = std::env::var_os(VARIABLE) else {
        return Ok(());
    };
    // SAFETY: caller guarantees single-threaded environment access.
    unsafe {
        std::env::remove_var(VARIABLE);
    }
    let fd: i32 = value
        .to_str()
        .and_then(|v| v.parse().ok())
        .filter(|fd| *fd >= 3)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid process startup descriptor",
            )
        })?;
    // Verify that this inherited capability is a socket before taking ownership.
    let mut kind = 0_i32;
    let mut size = std::mem::size_of_val(&kind) as libc::socklen_t;
    // SAFETY: pointers cover the output type and live length for getsockopt.
    if unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&mut kind as *mut i32).cast(),
            &mut size,
        )
    } != 0
    {
        return Err(io::Error::other(format!(
            "startup getsockopt fd={fd}: {}",
            io::Error::last_os_error()
        )));
    }
    if kind != libc::SOCK_STREAM {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "startup channel is not a stream socket",
        ));
    }
    // SAFETY: the launcher transfers this dedicated descriptor through exec;
    // it has not been wrapped in another owner in this process.
    let mut stream = unsafe { UnixStream::from_raw_fd(fd) };
    // Restore CLOEXEC before any later process launch, including on read error.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::other(format!(
            "startup cloexec fd={fd}: {}",
            io::Error::last_os_error()
        )));
    }
    // Darwin rejects SO_RCVTIMEO on an already-disconnected local socket, even
    // when the parent's completion byte is queued. Poll handles both completion
    // orders and EOF without configuring an already-closed peer connection.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "process registration timed out",
            ));
        }
        let mut ready = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let milliseconds = remaining.as_millis().clamp(1, 30_000) as i32;
        // SAFETY: one live descriptor and one writable poll record.
        let status = unsafe { libc::poll(&mut ready, 1, milliseconds) };
        if status > 0 {
            break;
        }
        if status < 0 {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
    let mut marker = [0];
    stream
        .read_exact(&mut marker)
        .map_err(|e| io::Error::other(format!("startup read fd={fd}: {e}")))?;
    if marker != [READY] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "process registration rejected",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Stdio};

    struct Reap(Child);
    impl Drop for Reap {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(Some(_))) {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }

    #[test]
    #[ignore = "isolated process fixture"]
    fn child_fixture() {
        println!("BEFORE");
        io::stdout().flush().unwrap();
        // SAFETY: this isolated subprocess runs only this fixture; no other
        // test accesses the environment and no application threads are started.
        let result = unsafe { wait_for_process_registration() };
        assert!(std::env::var_os(VARIABLE).is_none());
        println!("AFTER {}", result.is_ok());
    }

    fn line(reader: &mut BufReader<std::process::ChildStdout>) -> String {
        let mut poll = libc::pollfd {
            fd: reader.get_ref().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if reader.buffer().is_empty() {
            assert!(
                unsafe { libc::poll(&mut poll, 1, 5000) } > 0,
                "child output timed out"
            );
        }
        let mut line = String::new();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        line
    }

    #[test]
    fn inherited_gate_blocks_until_release_and_parent_loss_fails() {
        for (release, early) in [(true, false), (false, false), (true, true), (false, true)] {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "process_start_gate::tests::child_fixture",
                    "--ignored",
                    "--nocapture",
                ])
                .stdout(Stdio::piped());
            let mut gate = Some(StartGate::prepare(&mut command).unwrap());
            let mut child = Reap(command.spawn().unwrap());
            if early {
                let gate = gate.take().unwrap();
                if release {
                    gate.release().unwrap();
                } else {
                    drop(gate);
                }
            }
            let mut reader = BufReader::new(child.0.stdout.take().unwrap());
            while line(&mut reader).trim() != "BEFORE" {}
            if !early {
                assert!(reader.buffer().is_empty());
                let mut poll = libc::pollfd {
                    fd: reader.get_ref().as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                assert_eq!(
                    unsafe { libc::poll(&mut poll, 1, 100) },
                    0,
                    "child initialized before registration"
                );
                if release {
                    gate.take().unwrap().release().unwrap();
                } else {
                    drop(gate.take());
                }
            }
            let expected = format!("AFTER {release}");
            assert_eq!(line(&mut reader).trim(), expected);
            assert!(child.0.wait().unwrap().success());
        }
    }
}
