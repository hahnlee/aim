//! Descriptors a host helper process must not keep from its parent.
//!
//! A guest process's descriptors are host descriptors with the guest's
//! close-on-exec flags, so a host helper it spawns (the FUSE broker, the
//! pty holder) inherits every guest descriptor opened without O_CLOEXEC.
//! Held by a helper for its whole life, such a descriptor never sees its
//! last close: a pipe's reader never reads EOF, a FUSE description is
//! never released. Helpers take what they need by name and path, so they
//! keep only stdio.
//!
//! A host process spawning with `std::process::Command` while another of
//! its threads makes a socket or pipe leaks it the same way: Darwin has no
//! `SOCK_CLOEXEC` or `pipe2`, so std sets close-on-exec only after the
//! descriptor exists, and a child forked in between keeps it past exec
//! (#1617). [`stdio_only`] closes, in the child, what is not close-on-exec.

const EMPTY: libc::proc_fdinfo = libc::proc_fdinfo {
    proc_fd: -1,
    proc_fdtype: 0,
};

/// Closes every descriptor above stdio: a helper's first step. It does
/// not allocate.
pub fn close_above_stdio() {
    above_stdio(|fd| {
        // SAFETY: closing a descriptor of this process nothing uses yet.
        unsafe { libc::close(fd) };
    });
}

/// `command`'s child keeps only its stdio and the descriptors std itself
/// passes: before exec it closes every descriptor above stdio that is
/// not close-on-exec, which only a descriptor caught between its
/// creation and its close-on-exec is. The closes run in the forked
/// child and are async-signal-safe: they neither allocate nor lock.
pub fn stdio_only(command: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::unix::process::CommandExt;
    // SAFETY: the hook only calls proc_pidinfo, fcntl and close.
    unsafe {
        command.pre_exec(|| {
            above_stdio(|fd| {
                if libc::fcntl(fd, libc::F_GETFD) & libc::FD_CLOEXEC == 0 {
                    libc::close(fd);
                }
            });
            Ok(())
        })
    }
}

/// Calls `f` with each of this process's descriptors above stdio, from a
/// stack buffer.
fn above_stdio(mut f: impl FnMut(i32)) {
    let mut buffer = [EMPTY; 4096];
    let size = std::mem::size_of_val(&buffer) as i32;
    // SAFETY: proc_pidinfo writes at most `size` bytes into `buffer`.
    let n = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDLISTFDS,
            0,
            buffer.as_mut_ptr().cast(),
            size,
        )
    };
    let count = (n.max(0) as usize / std::mem::size_of::<libc::proc_fdinfo>()).min(buffer.len());
    for info in &buffer[..count] {
        if info.proc_fd > 2 {
            f(info.proc_fd);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    /// Whether a child of `command` has `fd` open.
    fn child_has(mut command: Command, fd: i32) -> bool {
        command
            .arg("-c")
            .arg(format!("test -e /dev/fd/{fd}"))
            .status()
            .unwrap()
            .success()
    }

    #[test]
    fn a_helper_keeps_only_its_stdio() {
        let mut ends = [-1; 2];
        assert_eq!(unsafe { libc::pipe(ends.as_mut_ptr()) }, 0);
        // A guest pipe opened without O_CLOEXEC reaches a child.
        assert!(child_has(Command::new("/bin/sh"), ends[1]));
        let mut helper = Command::new("/bin/sh");
        // SAFETY: proc_pidinfo into a stack buffer and close.
        unsafe {
            helper.pre_exec(|| {
                super::close_above_stdio();
                Ok(())
            })
        };
        assert!(!child_has(helper, ends[1]));
        unsafe {
            libc::close(ends[0]);
            libc::close(ends[1]);
        }
    }

    /// A descriptor not close-on-exec when the child forks, as one another
    /// thread is still making, does not reach it; its stdio and std's own
    /// exec error report work as before.
    #[test]
    fn a_spawned_child_keeps_only_its_stdio() {
        let mut ends = [-1; 2];
        assert_eq!(unsafe { libc::pipe(ends.as_mut_ptr()) }, 0);
        let mut child = Command::new("/bin/sh");
        super::stdio_only(&mut child);
        assert!(!child_has(child, ends[1]));
        let mut echo = Command::new("/bin/echo");
        super::stdio_only(&mut echo).arg("stdio");
        assert_eq!(echo.output().unwrap().stdout, b"stdio\n");
        let mut missing = Command::new("/nonexistent/program");
        super::stdio_only(&mut missing);
        assert_eq!(
            missing.status().unwrap_err().kind(),
            std::io::ErrorKind::NotFound
        );
        unsafe {
            libc::close(ends[0]);
            libc::close(ends[1]);
        }
    }
}
