//! Retain the inner shell status before aimctl maps signals to exit codes (#1311).
use aim_storage::process_namespace::ProcessIdentity;
use std::{
    io::Write,
    os::unix::process::ExitStatusExt,
    process::{Command, ExitStatus},
};

pub fn run(command: &mut Command, diagnostic: &mut dyn Write) -> Result<ExitStatus, String> {
    let mut child = command
        .spawn()
        .map_err(|e| format!("linux-run: spawn: {e}"))?;
    let pid = child.id();
    // Retain the birth before wait reaps the child. Even an exited, unreaped
    // child still has kernel metadata; no later PID lookup can replace it.
    let birth = ProcessIdentity::running_strict(pid as i32);
    let status = child
        .wait()
        .map_err(|e| format!("linux-run: pid={pid} birth={birth:?}: wait: {e}"))?;
    if !status.success() || birth.is_err() {
        writeln!(
            diagnostic,
            "aimctl: shell-child pid={pid} birth={birth:?} raw={} code={:?} signal={:?}",
            status.into_raw(),
            status.code(),
            status.signal()
        )
        .map_err(|e| format!("linux-run: pid={pid} status={status}: diagnostic: {e}"))?;
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    fn invoke(script: &str) -> (ExitStatus, String) {
        let mut diagnostic = Vec::new();
        let status = run(
            Command::new("/bin/sh").args(["-c", script]),
            &mut diagnostic,
        )
        .unwrap();
        (status, String::from_utf8(diagnostic).unwrap())
    }

    #[test]
    fn explicit_137_and_sigkill_retain_distinct_inner_statuses() {
        for (script, raw, code, signal) in [
            ("exit 137", 137 << 8, Some(137), None),
            ("kill -KILL $$", libc::SIGKILL, None, Some(libc::SIGKILL)),
        ] {
            let (status, receipt) = invoke(script);
            assert_eq!(status.into_raw(), raw);
            assert_eq!(status.code(), code);
            assert_eq!(status.signal(), signal);
            assert!(receipt.contains("birth=Ok(ProcessIdentity {"), "{receipt}");
            assert!(receipt.contains("start_seconds:"), "{receipt}");
            assert!(receipt.contains("start_microseconds:"), "{receipt}");
            assert!(receipt.contains(&format!("raw={raw} code={code:?} signal={signal:?}")));
        }
        let (status, receipt) = invoke("exit 0");
        assert!(status.success());
        assert!(receipt.is_empty(), "{receipt}");
    }

    #[test]
    fn wait_receipt_does_not_depend_on_output_eof() {
        use std::io::Read;
        use std::os::unix::net::UnixStream;
        let (mut reader, writer) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let child_writer = writer.try_clone().unwrap();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "printf partial; exit 137"]);
        command.stdout(Stdio::from(std::os::fd::OwnedFd::from(child_writer)));
        let mut receipt = Vec::new();
        let status = run(&mut command, &mut receipt).unwrap();
        assert_eq!(status.code(), Some(137));
        assert!(!receipt.is_empty());
        let mut bytes = [0; 7];
        reader.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"partial");
        assert_eq!(
            reader.read(&mut bytes).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        writer.shutdown(std::net::Shutdown::Write).unwrap();
        assert_eq!(reader.read(&mut bytes).unwrap(), 0);
    }
}
