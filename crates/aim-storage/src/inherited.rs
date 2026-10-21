//! Descriptors a host helper process must not keep from its parent.
//!
//! A guest process's descriptors are host descriptors with the guest's
//! close-on-exec flags, so a host helper it spawns (the FUSE broker, the
//! pty holder) inherits every guest descriptor opened without O_CLOEXEC.
//! Held by a helper for its whole life, such a descriptor never sees its
//! last close: a pipe's reader never reads EOF, a FUSE description is
//! never released. Helpers take what they need by name and path, so they
//! keep only stdio.

const EMPTY: libc::proc_fdinfo = libc::proc_fdinfo {
    proc_fd: -1,
    proc_fdtype: 0,
};

/// Closes every descriptor above stdio: a helper's first step. It does
/// not allocate.
pub fn close_above_stdio() {
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
    for f in &buffer[..count] {
        if f.proc_fd > 2 {
            // SAFETY: closing a descriptor of this process nothing uses yet.
            unsafe { libc::close(f.proc_fd) };
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
}
