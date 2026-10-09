//! Actual anonymous pipe identities from the Darwin kernel.
use std::{io, mem, os::fd::AsRawFd};
pub type Key = (u64, u64);
#[repr(C)]
struct FileInfo {
    flags: u32,
    status: u32,
    offset: i64,
    kind: i32,
    guard: u32,
}
#[repr(C)]
struct PipeInfo {
    stat: libc::vinfo_stat,
    handle: u64,
    peer: u64,
    status: i32,
    reserved: i32,
}
#[repr(C)]
struct PipeFd {
    file: FileInfo,
    pipe: PipeInfo,
}
fn failure(code: i32) -> io::Error {
    io::Error::from_raw_os_error(code)
}
pub fn info(fd: i32) -> io::Result<Key> {
    let mut value: PipeFd = unsafe { mem::zeroed() };
    let size = mem::size_of_val(&value) as i32;
    let count = unsafe {
        libc::proc_pidfdinfo(
            libc::getpid(),
            fd,
            6,
            (&mut value as *mut PipeFd).cast(),
            size,
        )
    };
    if count != size {
        let error = io::Error::last_os_error();
        return Err(if error.raw_os_error() == Some(0) {
            failure(libc::EIO)
        } else {
            error
        });
    }
    Ok((value.pipe.handle, value.pipe.peer))
}
/// Only anonymous, write-end pipe descriptors can be registered capabilities.
pub fn key(fd: i32) -> io::Result<Option<Key>> {
    let mut stat: libc::stat = unsafe { mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFIFO {
        return Ok(None);
    }
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if flags & libc::O_ACCMODE != libc::O_WRONLY {
        return Ok(None);
    }
    match info(fd) {
        Ok(key) => Ok(Some(key)),
        Err(error) if error.raw_os_error() == Some(libc::EBADF) => {
            // PROC_PIDFDPIPEINFO uses EBADF for a live vnode FIFO, too. Preserve a
            // genuinely closed descriptor error instead of misclassifying that FIFO.
            if unsafe { libc::fstat(fd, &mut stat) } < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(None)
        }
        Err(error) => Err(error),
    }
}
