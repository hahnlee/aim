//! Bounded command capture, including inherited output writers (#1268).

use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

pub fn run(command: &mut Command, timeout: Duration) -> Result<(ExitStatus, String), String> {
    use std::os::fd::AsRawFd;
    let context = format!("{command:?}");
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("{context}: spawn: {error}"))?;
    let pid = child.id();
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut failure = None;
    for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            failure = Some(format!("pipe configuration: {}", std::io::Error::last_os_error()));
            break;
        }
    }
    let pipes_ready = failure.is_none();
    let drain = |pipe: &mut dyn Read, bytes: &mut Vec<u8>| -> std::io::Result<bool> {
        let mut buffer = [0; 8192];
        // Bound each pass so a continuously writing child cannot starve the deadline.
        for _ in 0..16 {
            match pipe.read(&mut buffer) {
                Ok(0) => return Ok(true),
                Ok(n) => bytes.extend_from_slice(&buffer[..n]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(false)
    };
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut out_eof = false;
    let mut err_eof = false;
    let mut status = None;
    loop {
        if failure.is_none() {
            for (pipe, bytes, eof) in [
                (&mut stdout as &mut dyn Read, &mut out, &mut out_eof),
                (&mut stderr as &mut dyn Read, &mut err, &mut err_eof),
            ] {
                if !*eof {
                    match drain(pipe, bytes) {
                        Ok(value) => *eof = value,
                        Err(error) => failure = Some(format!("output read: {error}")),
                    }
                }
            }
        }
        if status.is_none() {
            match child.try_wait() {
                Ok(value) => status = value,
                Err(error) => failure = Some(format!("wait: {error}")),
            }
        }
        if status.is_some() && out_eof && err_eof { break; }
        if failure.is_some() || Instant::now() >= deadline {
            if failure.is_none() {
                failure = Some(format!("timed out after {timeout:?}; output pipes eof={out_eof}/{err_eof}"));
            }
            if status.is_none() {
                // Signal only our still-owned child PID, never its process group.
                unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                let grace = Instant::now() + Duration::from_secs(1);
                while Instant::now() < grace {
                    if let Ok(Some(value)) = child.try_wait() { status = Some(value); break; }
                    std::thread::sleep(Duration::from_millis(10));
                }
                if status.is_none() {
                    let kill_error = child.kill().err();
                    status = Some(child.wait().map_err(|error| {
                        format!("{context}: pid {pid}: reap: {error}; kill: {kill_error:?}")
                    })?);
                }
            }
            // Read only currently available bytes. Dropping our pipe ends below
            // does not wait for an unrelated inherited writer to exit.
            if pipes_ready {
                for (pipe, bytes) in [(&mut stdout as &mut dyn Read, &mut out),
                    (&mut stderr as &mut dyn Read, &mut err)] {
                    if let Err(error) = drain(pipe, bytes) {
                        let prior = failure.take().unwrap_or_default();
                        failure = Some(format!("{prior}; final output read: {error}"));
                    }
                }
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let status = status.expect("child reaped");
    let out = String::from_utf8_lossy(&out).into_owned();
    let err = String::from_utf8_lossy(&err).into_owned();
    if failure.is_some() || !status.success() {
        let reason = failure.unwrap_or_else(|| {
            if status.signal() == Some(libc::SIGKILL) {
                "linux-run got SIGKILL (#232; sender unknown)".into()
            } else {
                "unsuccessful exit".into()
            }
        });
        return Err(format!("{context}: pid {pid}: {reason}; status {status}; stdout: {out}; stderr: {err}"));
    }
    Ok((status, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn actual_child_status_and_stderr_are_preserved() {
        assert_eq!(run(&mut sh("printf '1\\n'; printf 'note' >&2"),
            Duration::from_secs(2)).unwrap().1, "1\n");
        let error = run(&mut sh("printf 'partial'; printf 'query failed' >&2; exit 1"),
            Duration::from_secs(2)).unwrap_err();
        assert!(error.contains("exit status: 1"), "{error}");
        assert!(error.contains("stdout: partial; stderr: query failed"), "{error}");
        assert!(error.contains("/bin/sh") && error.contains("pid "), "{error}");
        let error = run(&mut sh("printf 'before signal' >&2; kill -TERM $$"),
            Duration::from_secs(2)).unwrap_err();
        assert!(error.contains("signal: 15") && error.contains("before signal"), "{error}");
        let error = run(&mut sh("printf 'before kill' >&2; kill -KILL $$"),
            Duration::from_secs(2)).unwrap_err();
        assert!(error.contains("SIGKILL") && error.contains("signal: 9")
            && error.contains("before kill"), "{error}");
        let error = run(&mut sh("printf '%s' $$; exec sleep 10"),
            Duration::from_millis(50)).unwrap_err();
        assert!(error.contains("timed out after 50ms") && error.contains("signal: 15"), "{error}");
        let pid: i32 = error.split("stdout: ").nth(1).unwrap()
            .split(';').next().unwrap().parse().unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "child {pid} must be reaped");
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }
    #[test]
    fn inherited_writer_does_not_extend_capture_deadline() {
        let start = Instant::now();
        let error = run(&mut sh(
            "sleep 10 & child=$!; printf 'birth='; ps -p $child -o lstart=; printf 'child=%s\\n' $child; exit 0"),
            Duration::from_millis(150)).unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(2), "{error}");
        assert!(error.contains("status exit status: 0") && error.contains("output pipes eof=false/false"), "{error}");
        let pid: i32 = error.split("child=").last().unwrap().split_whitespace()
            .next().unwrap().parse().unwrap();
        let birth = error.split("stdout: birth=").nth(1).unwrap()
            .lines().next().unwrap().trim();
        // The fixture started this descendant. Check the retained birth evidence
        // before terminating this individual PID; never signal a process group.
        let actual = Command::new("/bin/ps").args(["-p", &pid.to_string(), "-o", "lstart="])
            .output().unwrap();
        assert_eq!(String::from_utf8_lossy(&actual.stdout).trim(), birth);
        assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
        let deadline = Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

}
