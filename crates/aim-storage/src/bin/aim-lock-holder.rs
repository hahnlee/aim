//! Internal native kernel worker; its parent is the controller, never a guest.
use aim_storage::{posix_control, process_namespace};
fn install_null_stdin() -> std::io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
    let raw = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let null = unsafe { OwnedFd::from_raw_fd(raw) };
    if null.as_raw_fd() == libc::STDIN_FILENO {
        let _ = null.into_raw_fd();
    } else if unsafe { libc::dup2(null.as_raw_fd(), libc::STDIN_FILENO) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    #[test]
    fn installs_null_stdin_after_exec_even_when_stdin_was_closed() {
        let saved = unsafe { libc::fcntl(libc::STDIN_FILENO, libc::F_DUPFD_CLOEXEC, 3) };
        assert!(saved >= 0);
        let saved = unsafe { OwnedFd::from_raw_fd(saved) };
        assert_eq!(unsafe { libc::close(libc::STDIN_FILENO) }, 0);
        install_null_stdin().unwrap();
        let mut stdin_stat: libc::stat = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::fstat(libc::STDIN_FILENO, &mut stdin_stat) },
            0
        );
        let null = std::fs::File::open("/dev/null").unwrap();
        let mut null_stat: libc::stat = unsafe { std::mem::zeroed() };
        assert_eq!(unsafe { libc::fstat(null.as_raw_fd(), &mut null_stat) }, 0);
        assert_eq!(
            (stdin_stat.st_dev, stdin_stat.st_ino),
            (null_stat.st_dev, null_stat.st_ino)
        );
        assert_eq!(
            unsafe { libc::dup2(saved.as_raw_fd(), libc::STDIN_FILENO) },
            libc::STDIN_FILENO
        );
    }
}
fn main() {
    aim_storage::inherited::close_above_stdio();
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let result = (|| {
        install_null_stdin()?;
        if args.len() != 8
            || args[0] != "--service-name"
            || args[2] != "--controller-pid"
            || args[4] != "--controller-seconds"
            || args[6] != "--controller-microseconds"
        {
            return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
        }
        let number = |value: &str| {
            value
                .parse::<u64>()
                .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))
        };
        let pid = number(&args[3])?;
        let controller = process_namespace::ProcessIdentity {
            host_pid: i32::try_from(pid)
                .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?,
            start_seconds: number(&args[5])?,
            start_microseconds: number(&args[7])?,
        };
        posix_control::serve(&args[1], controller)
    })();
    if let Err(error) = result {
        eprintln!("native POSIX lock holder: {error}");
        std::process::exit(1);
    }
}
