//! Ashmem's Darwin backing-file representation, shared by native producers
//! and the Linux syscall owner. The guest still uses original libcutils/Parcel.
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd};

/// Both flags distinguish an ashmem region from ordinary guest files.
pub const MARK: u32 = libc::UF_NODUMP | libc::UF_OPAQUE;
pub const XATTR: &core::ffi::CStr = c"dev.aim.ashmem";

/// A region's state besides its size.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub prot: u32,
    pub mapped: bool,
    /// Unpinned page ranges, inclusive, sorted and disjoint.
    pub unpinned: Vec<(u64, u64)>,
    pub name: Vec<u8>,
}

impl State {
    /// "prot mapped [start-end ...]\nname".
    pub fn encode(&self) -> Vec<u8> {
        let ranges: Vec<String> = self
            .unpinned
            .iter()
            .map(|(a, b)| format!(" {a}-{b}"))
            .collect();
        let mut v =
            format!("{} {}{}\n", self.prot, self.mapped as u8, ranges.concat()).into_bytes();
        v.extend_from_slice(&self.name);
        v
    }

    pub fn decode(b: &[u8]) -> Option<State> {
        let nl = b.iter().position(|&c| c == b'\n')?;
        let head = std::str::from_utf8(&b[..nl]).ok()?;
        let mut w = head.split(' ');
        let prot = w.next()?.parse().ok()?;
        let mapped = w.next()? == "1";
        let unpinned = w
            .map(|r| {
                let (a, b) = r.split_once('-')?;
                Some((a.parse().ok()?, b.parse().ok()?))
            })
            .collect::<Option<_>>()?;
        Some(State {
            prot,
            mapped,
            unpinned,
            name: b[nl + 1..].to_vec(),
        })
    }
}

impl Default for State {
    fn default() -> State {
        State {
            prot: 7,
            mapped: false,
            unpinned: Vec::new(),
            name: Vec::new(),
        }
    }
}

/// A complete immutable region for a native Parcel blob. The syscall owner
/// enforces this protection mask when the file reaches a guest.
pub fn immutable_blob(bytes: &[u8]) -> io::Result<std::fs::File> {
    let mut name = std::env::temp_dir()
        .join("aim-ashmem.XXXXXX")
        .into_os_string()
        .into_encoded_bytes();
    name.push(0);
    // SAFETY: mkstemp initializes the mutable NUL-terminated template.
    let fd = unsafe { libc::mkstemp(name.as_mut_ptr().cast()) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: ownership of the new fd is transferred exactly once.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    // SAFETY: the generated name is NUL-terminated and this fd is owned.
    if unsafe { libc::unlink(name.as_ptr().cast()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    file.write_all(bytes)?;
    let state = State {
        prot: 1,
        mapped: true,
        unpinned: vec![],
        name: b"Parcel Blob".to_vec(),
    }
    .encode();
    // SAFETY: writes the shared metadata from its live encoded buffer.
    if unsafe { libc::fsetxattr(fd, XATTR.as_ptr(), state.as_ptr().cast(), state.len(), 0, 0) } < 0
        || unsafe { libc::fchflags(file.as_raw_fd(), MARK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}
