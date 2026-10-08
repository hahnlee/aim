//! Regular open-description transport. The backing file and existing writer
//! lock travel as independent fileports owned by the same Binder File Arc.
use std::{io, os::fd::{AsRawFd, BorrowedFd}};
use crate::wire::RegularMetadata;
pub const CLASS: u32 = 3;

/// The ABI owns these descriptors until the ioctl has captured their fileports.
pub struct Export {
    pub metadata: RegularMetadata,
    pub writer_fd: Option<i32>,
}

pub fn identity(fd: BorrowedFd<'_>) -> io::Result<[u8; 36]> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } < 0 { return Err(io::Error::last_os_error()); }
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG { return Err(io::Error::from_raw_os_error(libc::EINVAL)); }
    let mut bytes = [0; 36];
    bytes[..8].copy_from_slice(&(stat.st_dev as u32 as u64).to_le_bytes());
    bytes[8..16].copy_from_slice(&stat.st_ino.to_le_bytes());
    bytes[16..20].copy_from_slice(&stat.st_gen.to_le_bytes());
    bytes[20..28].copy_from_slice(&stat.st_birthtime.to_le_bytes());
    bytes[28..36].copy_from_slice(&stat.st_birthtime_nsec.to_le_bytes());
    Ok(bytes)
}

/// Authenticate the backing incarnation and access mode at the transport
/// boundary. The receiving inode owner additionally admits the writer lock
/// against its configured lock directory; it never acquires a replacement lock.
pub fn validate(backing: BorrowedFd<'_>, writer: Option<BorrowedFd<'_>>, metadata: &RegularMetadata) -> io::Result<()> {
    if identity(backing)? != metadata.identity || writer.is_some() != metadata.writer {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    let flags = unsafe { libc::fcntl(backing.as_raw_fd(), libc::F_GETFL) };
    if flags < 0 { return Err(io::Error::last_os_error()); }
    if flags & libc::O_EVTONLY != 0 || metadata.flags & crate::path_file::O_PATH as u64 != 0 || flags & libc::O_ACCMODE != (metadata.flags & 3) as i32 || (flags & libc::O_ACCMODE != libc::O_RDONLY) != metadata.writer {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    if let Some(writer) = writer { identity(writer)?; }
    Ok(())
}

/// A hidden incoming writer capability. Converting it to an fd belongs inside
/// the ABI's private-descriptor allocation guard; dropping always releases it.
pub struct WriterPort(crate::mach::Port);
impl WriterPort {
    pub(crate) fn new(port: crate::mach::Port) -> Self { Self(port) }
    pub fn as_port(&self) -> crate::mach::Port { self.0 }
}
impl Drop for WriterPort { fn drop(&mut self) { crate::mach::release_send(self.0); } }
