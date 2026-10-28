//! Explicit path-only descriptor transport. Classification uses registered
//! kernel socket identities, never a vnode inode or Darwin O_EVTONLY alone.
use crate::proxy_file::{receive_flags, send, socket_identity};
use std::{
    collections::HashMap,
    io,
    os::{
        fd::{AsRawFd, BorrowedFd, OwnedFd, RawFd},
        unix::net::UnixDatagram,
    },
    sync::{Arc, LazyLock, Mutex, Weak},
    thread,
};
pub const CLASS: u32 = 2;
pub const O_PATH: u32 = 0x200000;
const FLAGS: u32 = O_PATH | 0x4000 | 0x8000;
const MAGIC: u32 = 0x50415448;
static REGISTRY: LazyLock<Mutex<HashMap<(u64, u64), Weak<Owner>>>> =
    LazyLock::new(Default::default);
pub struct Owner {
    backing: OwnedFd,
    reference: Option<CharacterReference>,
    flags: u32,
    server: UnixDatagram,
}
pub struct Capability {
    pub carrier: OwnedFd,
    pub owner: Arc<Owner>,
}
pub struct Adopted {
    pub backing: OwnedFd,
    pub reference: Option<CharacterReference>,
    pub flags: u32,
    pub proof: Arc<OwnedFd>,
}
/// A devfs character inode reached relative to a directory, never by opening
/// the device. Generation and birth time distinguish reused PTY allocations.
#[derive(Clone)]
pub struct CharacterReference {
    name: std::ffi::CString,
    pub guest: String,
    identity: aim_storage::inode_lease::Identity,
}
impl CharacterReference {
    pub fn capture(directory: RawFd, name: &std::ffi::CStr, guest: String) -> io::Result<Self> {
        if name.to_bytes().is_empty() || name.to_bytes().contains(&b'/') {
            return Err(error(libc::EINVAL));
        }
        let stat = Self::lookup(directory, name)?;
        Ok(Self {
            name: name.into(),
            guest,
            identity: Self::identity(&stat),
        })
    }
    fn identity(stat: &libc::stat) -> aim_storage::inode_lease::Identity {
        aim_storage::inode_lease::Identity {
            dev: stat.st_dev as u32 as u64,
            ino: stat.st_ino,
            generation: stat.st_gen,
            birth_seconds: stat.st_birthtime,
            birth_nanoseconds: stat.st_birthtime_nsec,
        }
    }
    fn lookup(directory: RawFd, name: &std::ffi::CStr) -> io::Result<libc::stat> {
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::fstatat(
                directory,
                name.as_ptr(),
                &mut stat,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFCHR {
            return Err(error(libc::ESTALE));
        }
        Ok(stat)
    }
    pub fn stat(&self, directory: RawFd) -> io::Result<libc::stat> {
        let stat = Self::lookup(directory, &self.name)?;
        if Self::identity(&stat) != self.identity {
            return Err(error(libc::ESTALE));
        }
        Ok(stat)
    }
    pub fn host(&self, directory: RawFd) -> io::Result<std::ffi::CString> {
        self.stat(directory)?;
        let mut path = vec![0u8; libc::PATH_MAX as usize];
        if unsafe { libc::fcntl(directory, libc::F_GETPATH, path.as_mut_ptr()) } < 0 {
            return Err(io::Error::last_os_error());
        }
        path.truncate(
            path.iter()
                .position(|&byte| byte == 0)
                .ok_or(error(libc::EINVAL))?,
        );
        path.push(b'/');
        path.extend_from_slice(self.name.to_bytes());
        std::ffi::CString::new(path).map_err(|_| error(libc::EINVAL))
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut writer = crate::wire::Writer::default();
        writer.bytes(self.name.to_bytes());
        writer.bytes(self.guest.as_bytes());
        writer.bytes(&self.identity.to_bytes());
        writer.0
    }
    pub fn decode(bytes: &[u8]) -> io::Result<Self> {
        let mut reader = crate::wire::Reader::new(bytes);
        let mut decode = || {
            let name = reader.bytes()?;
            if name.is_empty() || name.contains(&b'/') || name.contains(&0) || name.len() > 255 {
                return Err(crate::wire::EPROTO);
            }
            let name = std::ffi::CString::new(name).map_err(|_| crate::wire::EPROTO)?;
            let guest = std::str::from_utf8(reader.bytes()?)
                .map_err(|_| crate::wire::EPROTO)?
                .to_owned();
            if !guest.starts_with('/')
                || guest.len() >= libc::PATH_MAX as usize
                || guest.contains('\0')
            {
                return Err(crate::wire::EPROTO);
            }
            let identity = aim_storage::inode_lease::Identity::from_bytes(reader.bytes()?)
                .map_err(|_| crate::wire::EPROTO)?;
            if reader.remaining() != 0 {
                return Err(crate::wire::EPROTO);
            }
            Ok(Self {
                name,
                guest,
                identity,
            })
        };
        decode().map_err(|_| error(libc::EPROTO))
    }
}
fn error(value: i32) -> io::Error {
    io::Error::from_raw_os_error(value)
}
fn valid_flags(flags: u32) -> bool {
    flags & O_PATH != 0 && flags & !FLAGS == 0
}
fn actual_path(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if flags & libc::O_EVTONLY == 0 {
        return Err(error(libc::EINVAL));
    }
    Ok(())
}
pub fn create(backing: OwnedFd, flags: u32) -> io::Result<Capability> {
    create_reference(backing, flags, None)
}
pub fn create_reference(
    backing: OwnedFd,
    flags: u32,
    reference: Option<CharacterReference>,
) -> io::Result<Capability> {
    if !valid_flags(flags) || (reference.is_some() && flags & 0x4000 != 0) {
        return Err(error(libc::EINVAL));
    }
    actual_path(backing.as_raw_fd())?;
    if let Some(reference) = &reference {
        reference.stat(backing.as_raw_fd())?;
    }
    let (client, server) = UnixDatagram::pair()?;
    let identity = socket_identity(client.as_raw_fd())?;
    let peer = socket_identity(server.as_raw_fd())?;
    if identity.0 == 0 || identity.1 != peer.0 || peer.1 != identity.0 {
        return Err(error(libc::EPROTO));
    }
    let owner = Arc::new(Owner {
        backing,
        reference,
        flags,
        server,
    });
    {
        let mut registry = REGISTRY.lock().unwrap();
        registry.retain(|_, value| value.strong_count() != 0);
        registry.insert(identity, Arc::downgrade(&owner));
    }
    let live = owner.clone();
    if let Err(failure) = thread::Builder::new()
        .name("path-capability".into())
        .spawn(move || {
            loop {
                if socket_identity(live.server.as_raw_fd()).is_ok_and(|peer| peer.1 != identity.0) {
                    break;
                }
                let mut poll = libc::pollfd {
                    fd: live.server.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ready = unsafe { libc::poll(&mut poll, 1, 100) };
                if ready < 0 {
                    if io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                        continue;
                    }
                    break;
                }
                if ready == 0 {
                    continue;
                }
                let mut bytes = [0u8; 8];
                let Ok((n, Some(reply))) =
                    receive_flags(live.server.as_raw_fd(), &mut bytes, libc::MSG_DONTWAIT)
                else {
                    continue;
                };
                let valid = n == 8
                    && u32::from_le_bytes(bytes[..4].try_into().unwrap()) == MAGIC
                    && bytes[4..] == [0; 4];
                let mut response = MAGIC.to_le_bytes().to_vec();
                response.extend_from_slice(&(if valid { 0 } else { libc::EINVAL }).to_le_bytes());
                response.extend_from_slice(&live.flags.to_le_bytes());
                if let Some(reference) = &live.reference {
                    response.extend_from_slice(&reference.encode());
                }
                let result = if valid {
                    send(reply.as_raw_fd(), &response, live.backing.as_raw_fd())
                } else {
                    let sent = unsafe {
                        libc::send(
                            reply.as_raw_fd(),
                            response.as_ptr().cast(),
                            response.len(),
                            0,
                        )
                    };
                    if sent == response.len() as isize {
                        Ok(())
                    } else {
                        Err(io::Error::last_os_error())
                    }
                };
                if let Err(failure) = result {
                    eprintln!("path capability reply: {failure}");
                }
            }
            REGISTRY.lock().unwrap().remove(&identity);
        })
    {
        REGISTRY.lock().unwrap().remove(&identity);
        return Err(failure);
    }
    Ok(Capability {
        carrier: client.into(),
        owner,
    })
}
pub fn registered_class_result(fd: RawFd) -> io::Result<u32> {
    let identity = match socket_identity(fd) {
        Ok(value) => value,
        Err(failure) => {
            let mut ty = 0i32;
            let mut length = 4;
            if unsafe {
                libc::getsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_TYPE,
                    (&mut ty as *mut i32).cast(),
                    &mut length,
                )
            } < 0
                && io::Error::last_os_error().raw_os_error() == Some(libc::ENOTSOCK)
            {
                return Ok(0);
            }
            return Err(failure);
        }
    };
    let mut registry = REGISTRY.lock().unwrap();
    registry.retain(|_, value| value.strong_count() != 0);
    Ok(
        if registry
            .get(&identity)
            .and_then(Weak::upgrade)
            .is_some_and(|owner| {
                socket_identity(owner.server.as_raw_fd())
                    .is_ok_and(|peer| peer == (identity.1, identity.0))
            })
        {
            CLASS
        } else {
            0
        },
    )
}
/// The caller first verifies CLASS using the native owner's registry. The proof
/// remains hidden in the receiving fd owner while backing replaces the carrier.
/// Errors are Darwin io::Error; the syscall owner translates them to Linux errno.
pub fn unwrap(carrier: BorrowedFd<'_>) -> io::Result<Adopted> {
    let proof = Arc::new(carrier.try_clone_to_owned()?);
    let (receiver, reply) = UnixDatagram::pair()?;
    let mut request = MAGIC.to_le_bytes().to_vec();
    request.extend_from_slice(&0u32.to_le_bytes());
    send(carrier.as_raw_fd(), &request, reply.as_raw_fd())?;
    let mut response = vec![0u8; 2 * libc::PATH_MAX as usize + 128];
    let (n, backing) = receive_flags(receiver.as_raw_fd(), &mut response, 0)?;
    if n < 12 || u32::from_le_bytes(response[..4].try_into().unwrap()) != MAGIC {
        return Err(error(libc::EPROTO));
    }
    let status = i32::from_le_bytes(response[4..8].try_into().unwrap());
    if status != 0 {
        return Err(error(status));
    }
    let flags = u32::from_le_bytes(response[8..12].try_into().unwrap());
    if !valid_flags(flags) {
        return Err(error(libc::EPROTO));
    }
    let backing = backing.ok_or_else(|| error(libc::EPROTO))?;
    actual_path(backing.as_raw_fd())?;
    let reference = (n > 12)
        .then(|| CharacterReference::decode(&response[12..n]))
        .transpose()?;
    if let Some(reference) = &reference {
        reference.stat(backing.as_raw_fd())?;
    }
    Ok(Adopted {
        backing,
        reference,
        flags,
        proof,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::CString,
        os::fd::{AsFd, FromRawFd},
    };
    fn directory() -> (std::path::PathBuf, OwnedFd) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "aim-path-cap-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        let name = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        let fd = unsafe { libc::open(name.as_ptr(), libc::O_EVTONLY | libc::O_CLOEXEC) };
        assert!(fd >= 0);
        (path, unsafe { OwnedFd::from_raw_fd(fd) })
    }
    #[test]
    fn character_reference_transfer_carries_only_a_directory_and_checks_allocation_identity() {
        let master = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC) };
        assert!(master >= 0);
        let master = unsafe { OwnedFd::from_raw_fd(master) };
        assert_eq!(unsafe { libc::grantpt(master.as_raw_fd()) }, 0);
        let mut name = [0u8; 128];
        assert_eq!(
            unsafe { libc::ioctl(master.as_raw_fd(), 0x4080_7453u64, name.as_mut_ptr()) },
            0
        );
        let name = std::ffi::CStr::from_bytes_until_nul(&name).unwrap();
        let name = std::ffi::CString::new(name.to_bytes().strip_prefix(b"/dev/").unwrap()).unwrap();
        let directory = unsafe {
            libc::open(
                c"/dev".as_ptr(),
                libc::O_EVTONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        assert!(directory >= 0);
        let directory = unsafe { OwnedFd::from_raw_fd(directory) };
        let reference =
            CharacterReference::capture(directory.as_raw_fd(), &name, "/dev/pts/0".into()).unwrap();
        let original = reference.stat(directory.as_raw_fd()).unwrap();
        let capability = create_reference(directory, O_PATH, Some(reference)).unwrap();
        let adopted = unwrap(capability.carrier.as_fd()).unwrap();
        let mut backing: libc::stat = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::fstat(adopted.backing.as_raw_fd(), &mut backing) },
            0
        );
        assert_eq!(backing.st_mode & libc::S_IFMT, libc::S_IFDIR);
        let mut reference = adopted.reference.unwrap();
        let stat = reference.stat(adopted.backing.as_raw_fd()).unwrap();
        assert_eq!(
            (stat.st_dev, stat.st_ino),
            (original.st_dev, original.st_ino)
        );
        assert_eq!(reference.guest, "/dev/pts/0");
        reference.identity.generation ^= 1;
        assert_eq!(
            reference
                .stat(adopted.backing.as_raw_fd())
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ESTALE)
        );
        let bytes = reference.encode();
        assert!(CharacterReference::decode(&bytes[..bytes.len() - 1]).is_err());
    }
    #[test]
    fn carrier_preserves_real_path_and_arm64_flags_without_classifying_native_watchers() {
        let (path, backing) = directory();
        assert_eq!(registered_class_result(backing.as_raw_fd()).unwrap(), 0);
        let ordinary = std::fs::File::open(&path).unwrap();
        assert_eq!(registered_class_result(ordinary.as_raw_fd()).unwrap(), 0);
        let mut original = std::mem::MaybeUninit::<libc::stat>::uninit();
        assert_eq!(
            unsafe { libc::fstat(backing.as_raw_fd(), original.as_mut_ptr()) },
            0
        );
        let original = unsafe { original.assume_init() };
        let flags = O_PATH | 0x4000 | 0x8000;
        let cap = create(backing, flags).unwrap();
        assert_eq!(
            registered_class_result(cap.carrier.as_raw_fd()).unwrap(),
            CLASS
        );
        let imported = unwrap(cap.carrier.as_fd()).unwrap();
        assert_eq!(imported.flags, flags);
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        assert_eq!(
            unsafe { libc::fstat(imported.backing.as_raw_fd(), stat.as_mut_ptr()) },
            0
        );
        let stat = unsafe { stat.assume_init() };
        assert_eq!(
            (stat.st_dev, stat.st_ino, stat.st_mode),
            (original.st_dev, original.st_ino, original.st_mode)
        );
        let mut byte = 0u8;
        assert_eq!(
            unsafe {
                libc::read(
                    imported.backing.as_raw_fd(),
                    (&mut byte as *mut u8).cast(),
                    1,
                )
            },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::EISDIR)
        ); // Native backing is a real Darwin directory; Linux O_PATH IO denial belongs to fdtab.
        drop(cap);
        assert_eq!(
            registered_class_result(imported.proof.as_raw_fd()).unwrap(),
            CLASS,
            "receiver proof keeps the native path owner alive"
        );
        let again = unwrap(imported.proof.as_ref().as_fd()).unwrap();
        assert_eq!(again.flags, flags);
        let forwarded = crate::server::file_from_fd(imported.proof.as_ref().as_fd()).unwrap();
        assert_eq!(crate::server::file_class(&forwarded), Some(CLASS));
        drop(again);
        drop(imported);
        drop(forwarded);
        std::fs::remove_dir(path).unwrap();
    }
    #[test]
    fn plain_stream_scm_rights_transfers_single_typed_carrier_without_payload_changes() {
        use std::os::fd::AsFd;
        let (path, backing) = directory();
        let cap = create(backing, O_PATH | 0x4000).unwrap();
        let (send_to, receive_at) = std::os::unix::net::UnixStream::pair().unwrap();
        send(send_to.as_raw_fd(), b"guest", cap.carrier.as_raw_fd()).unwrap();
        drop(cap);
        let mut bytes = [0u8; 5];
        let (n, carrier) = receive_flags(receive_at.as_raw_fd(), &mut bytes, 0).unwrap();
        assert_eq!(n, 5);
        assert_eq!(&bytes, b"guest");
        let carrier = carrier.unwrap();
        assert_eq!(registered_class_result(carrier.as_raw_fd()).unwrap(), CLASS);
        let imported = unwrap(carrier.as_fd()).unwrap();
        assert_eq!(imported.flags, O_PATH | 0x4000);
        drop(carrier);
        assert!(actual_path(imported.backing.as_raw_fd()).is_ok());
        std::fs::remove_dir(path).unwrap();
    }
    #[test]
    fn rejects_readable_backing_and_non_arm64_flag_claims() {
        use std::os::fd::AsFd;
        let (path, backing) = directory();
        assert!(create(backing, O_PATH | 0x10000).is_err());
        let readable = std::fs::File::open(&path).unwrap();
        assert!(create(readable.as_fd().try_clone_to_owned().unwrap(), O_PATH).is_err());
        std::fs::remove_dir(path).unwrap();
    }
}
