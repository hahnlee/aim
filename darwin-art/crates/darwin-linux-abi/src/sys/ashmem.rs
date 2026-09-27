//! `/dev/ashmem`, Android's anonymous shared memory device (#195), over an
//! unlinked host file like a memfd: read, fstat, `MAP_SHARED` mappings,
//! fork, dup and descriptor passing work natively and across processes.
//!
//! Every process holding the fd (by fork, binder or `SCM_RIGHTS`) sees the
//! region's state, because it lives with the file:
//! - the file carries marker flags ([`MARK`]) that fstat reports at no
//!   cost, so an ashmem fd is recognized wherever it arrives, and fstat
//!   shows the character device libcutils' `ashmem_valid` expects;
//! - the size is the file's size;
//! - the name, protection mask, whether it was mapped and the unpinned
//!   ranges are in an extended attribute.
//!
//! Unpinned pages are never purged: PIN reports ASHMEM_NOT_PURGED.

use std::sync::Mutex;

use crate::errno::{self, EINVAL, EPERM};

/// `st_flags` of an ashmem file (both set; guest files never have them).
const MARK: u32 = libc::UF_NODUMP | libc::UF_OPAQUE;
/// The device number fstat reports (misc major 10).
const RDEV: i32 = (10 << 24) | 58;
const XATTR: &core::ffi::CStr = c"dev.darwinart.ashmem";
const PAGE: u64 = 16384;
const NAME_LEN: usize = 256;
const PROT_MASK: u32 = 7; // PROT_READ | PROT_WRITE | PROT_EXEC

const SET_NAME: u64 = 0x4100_7701;
const GET_NAME: u64 = 0x8100_7702;
const SET_SIZE: u64 = 0x4008_7703;
const GET_SIZE: u64 = 0x7704;
const SET_PROT_MASK: u64 = 0x4008_7705;
const GET_PROT_MASK: u64 = 0x7706;
const PIN: u64 = 0x4008_7707;
const UNPIN: u64 = 0x4008_7708;
const GET_PIN_STATUS: u64 = 0x7709;
const PURGE_ALL_CACHES: u64 = 0x770a;
const GET_FILE_ID: u64 = 0x8008_770b;

const NOT_PURGED: i64 = 0;
const IS_UNPINNED: i64 = 0;
const IS_PINNED: i64 = 1;

/// `/dev/ashmem`, or libcutils' `/dev/ashmem<boot_id>`.
fn is_device(guest: &str) -> bool {
    guest
        .strip_prefix("/dev/ashmem")
        .is_some_and(|id| id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'))
}

const O_CLOEXEC: u64 = 0o2000000;

/// `openat` of the device: a new region. None for other paths.
pub fn open(guest: &str, flags: u64) -> Option<i64> {
    if !is_device(guest) {
        return None;
    }
    let mut tmpl = std::env::temp_dir()
        .join("linux-abi-ashmem.XXXXXX")
        .into_os_string()
        .into_encoded_bytes();
    tmpl.push(0);
    // SAFETY: mkstemp fills in the template; the name is removed at once.
    Some(unsafe {
        let fd = libc::mkstemp(tmpl.as_mut_ptr().cast());
        if fd < 0 {
            return Some(-(errno::last() as i64));
        }
        libc::unlink(tmpl.as_ptr().cast());
        if libc::fchflags(fd, MARK) < 0 {
            let e = errno::last();
            libc::close(fd);
            return Some(-(e as i64));
        }
        super::fdtab::set_flags(fd, false, flags & O_CLOEXEC != 0);
        fd as i64
    })
}

/// Whether a host stat describes an ashmem file.
fn marked(st: &libc::stat) -> bool {
    st.st_mode & libc::S_IFMT == libc::S_IFREG && st.st_nlink == 0 && st.st_flags & MARK == MARK
}

fn stat_of(fd: i32) -> Option<libc::stat> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat into a local buffer.
    (unsafe { libc::fstat(fd, &mut st) } == 0 && marked(&st)).then_some(st)
}

/// fstat of an ashmem fd shows the device, as on Linux. False: not one.
pub fn as_device(st: &mut libc::stat) -> bool {
    if !marked(st) {
        return false;
    }
    st.st_mode = libc::S_IFCHR | 0o666;
    st.st_rdev = RDEV;
    st.st_nlink = 1;
    st.st_size = 0;
    st.st_blocks = 0;
    st.st_uid = 0;
    st.st_gid = 0;
    true
}

/// Link text for `/proc/self/fd/N`.
pub fn link_name(st: &libc::stat) -> Option<String> {
    marked(st).then(|| "/dev/ashmem".to_string())
}

/// A region's state besides its size.
#[derive(Debug, PartialEq)]
struct State {
    prot: u32,
    mapped: bool,
    /// Unpinned page ranges, inclusive, sorted and disjoint.
    unpinned: Vec<(u64, u64)>,
    name: Vec<u8>,
}

impl State {
    /// "prot mapped [start-end ...]\nname".
    fn encode(&self) -> Vec<u8> {
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

    fn decode(b: &[u8]) -> Option<State> {
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
            prot: PROT_MASK,
            mapped: false,
            unpinned: Vec::new(),
            name: Vec::new(),
        }
    }
}

/// Serializes this process's read-modify-write of the attribute.
static LOCK: Mutex<()> = Mutex::new(());

fn load(fd: i32) -> State {
    let mut buf = vec![0u8; 4096];
    // SAFETY: reading our attribute into a local buffer.
    let n =
        unsafe { libc::fgetxattr(fd, XATTR.as_ptr(), buf.as_mut_ptr().cast(), buf.len(), 0, 0) };
    if n <= 0 {
        return State::default();
    }
    State::decode(&buf[..n as usize]).unwrap_or_default()
}

fn store(fd: i32, s: &State) -> i64 {
    let v = s.encode();
    // SAFETY: writing our attribute from a local buffer.
    let r = unsafe { libc::fsetxattr(fd, XATTR.as_ptr(), v.as_ptr().cast(), v.len(), 0, 0) };
    if r < 0 { -(errno::last() as i64) } else { 0 }
}

fn update(fd: i32, f: impl FnOnce(&mut State) -> i64) -> i64 {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = load(fd);
    let r = f(&mut s);
    if r < 0 {
        return r;
    }
    match store(fd, &s) {
        0 => r,
        e => e,
    }
}

fn page_up(n: u64) -> u64 {
    n.div_ceil(PAGE) * PAGE
}

/// mmap of an ashmem fd: Linux's checks, and the region counts as mapped
/// (its name and size are fixed from then on). None: not ashmem or fine.
pub fn before_mmap(fd: i32, len: u64, prot: u64) -> Option<i64> {
    let st = stat_of(fd)?;
    let r = update(fd, |s| {
        if st.st_size == 0 {
            return -(EINVAL as i64);
        }
        if prot as u32 & PROT_MASK & !s.prot != 0 {
            return -(EPERM as i64);
        }
        if page_up(len) > page_up(st.st_size as u64) {
            return -(EINVAL as i64);
        }
        s.mapped = true;
        0
    });
    (r < 0).then_some(r)
}

/// Add `[a, b]` to disjoint sorted ranges, or remove it.
fn change_ranges(v: &mut Vec<(u64, u64)>, a: u64, b: u64, add: bool) {
    let mut out = Vec::with_capacity(v.len() + 1);
    let (mut lo, mut hi) = (a, b);
    for &(x, y) in v.iter() {
        if y + 1 < a || x > b + 1 {
            out.push((x, y));
        } else if add {
            lo = lo.min(x);
            hi = hi.max(y);
        } else {
            if x < a {
                out.push((x, a - 1));
            }
            if y > b {
                out.push((b + 1, y));
            }
        }
    }
    if add {
        out.push((lo, hi));
    }
    out.sort_unstable();
    *v = out;
}

fn pin(fd: i32, req: u64, arg: u64, size: u64) -> i64 {
    // SAFETY: guest struct ashmem_pin { u32 offset, len; }.
    let [off, len] = unsafe { (arg as *const [u32; 2]).read_unaligned() }.map(|v| v as u64);
    update(fd, |s| {
        if !s.mapped {
            return -(EINVAL as i64);
        }
        let len = if len == 0 {
            page_up(size).saturating_sub(off)
        } else {
            len
        };
        if (off | len) & (PAGE - 1) != 0 || len == 0 || off + len > page_up(size) {
            return -(EINVAL as i64);
        }
        let (a, b) = (off / PAGE, (off + len) / PAGE - 1);
        match req {
            PIN => {
                change_ranges(&mut s.unpinned, a, b, false);
                NOT_PURGED
            }
            UNPIN => {
                change_ranges(&mut s.unpinned, a, b, true);
                0
            }
            _ if s.unpinned.iter().any(|&(x, y)| x <= b && a <= y) => IS_UNPINNED,
            _ => IS_PINNED,
        }
    })
}

/// ioctl on an ashmem fd; None when `fd` is not one or `req` is not an
/// ashmem request.
pub fn ioctl(fd: i32, req: u64, arg: u64) -> Option<i64> {
    if (req >> 8) & 0xff != 0x77 {
        return None;
    }
    let st = stat_of(fd)?;
    Some(match req {
        SET_NAME => {
            // SAFETY: guest buffer of up to ASHMEM_NAME_LEN bytes.
            let name = unsafe { std::slice::from_raw_parts(arg as *const u8, NAME_LEN) };
            let len = name.iter().position(|&c| c == 0).unwrap_or(NAME_LEN - 1);
            update(fd, |s| {
                if s.mapped {
                    return -(EINVAL as i64);
                }
                s.name = name[..len].to_vec();
                0
            })
        }
        GET_NAME => {
            let s = load(fd);
            let name: &[u8] = if s.name.is_empty() {
                b"dev/ashmem"
            } else {
                &s.name
            };
            // SAFETY: guest buffer of ASHMEM_NAME_LEN bytes.
            unsafe {
                std::ptr::copy_nonoverlapping(name.as_ptr(), arg as *mut u8, name.len());
                (arg as *mut u8).add(name.len()).write(0);
            }
            0
        }
        SET_SIZE => update(fd, |s| {
            if s.mapped {
                return -(EINVAL as i64);
            }
            // SAFETY: resizing our own file.
            errno::check(unsafe { libc::ftruncate(fd, arg as i64) } as i64)
        }),
        GET_SIZE => st.st_size,
        SET_PROT_MASK => update(fd, |s| {
            // Bits can only be removed.
            if arg as u32 & !s.prot != 0 || arg > PROT_MASK as u64 {
                return -(EINVAL as i64);
            }
            s.prot = arg as u32;
            0
        }),
        GET_PROT_MASK => load(fd).prot as i64,
        PIN | UNPIN | GET_PIN_STATUS => pin(fd, req, arg, st.st_size as u64),
        // Nothing is ever purged, so there is nothing to purge.
        PURGE_ALL_CACHES => 0,
        GET_FILE_ID => {
            // SAFETY: guest unsigned long.
            unsafe { (arg as *mut u64).write_unaligned(st.st_ino) };
            0
        }
        _ => -(libc::ENOTTY as i64),
    })
}

/// This module's locks for a fork (`sys::forklock`).
pub(crate) fn fork_try(held: &mut Vec<super::forklock::Guard>) -> bool {
    super::forklock::mutex(&LOCK, held)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region() -> i32 {
        open("/dev/ashmem", O_CLOEXEC).unwrap() as i32
    }

    #[test]
    fn state_round_trips() {
        let s = State {
            prot: 5,
            mapped: true,
            unpinned: vec![(0, 1), (4, 4)],
            name: b"with\nnewline".to_vec(),
        };
        assert_eq!(State::decode(&s.encode()), Some(s));
    }

    #[test]
    fn ranges_merge_and_split() {
        let mut v = Vec::new();
        change_ranges(&mut v, 2, 3, true);
        change_ranges(&mut v, 5, 6, true);
        change_ranges(&mut v, 4, 4, true);
        assert_eq!(v, [(2, 6)]);
        change_ranges(&mut v, 4, 4, false);
        assert_eq!(v, [(2, 3), (5, 6)]);
    }

    #[test]
    fn region_ioctls() {
        assert!(open("/dev/ashmem0123-abcd", 0).is_some());
        assert!(open("/dev/ashmemx", 0).is_none());
        let fd = region();
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: fstat into a local buffer.
        assert_eq!(unsafe { libc::fstat(fd, &mut st) }, 0);
        assert!(as_device(&mut st));
        assert_eq!(st.st_mode, libc::S_IFCHR | 0o666);
        let name = *b"sensor queue\0";
        let mut buf = [0u8; NAME_LEN];
        assert_eq!(ioctl(fd, GET_NAME, buf.as_mut_ptr() as u64), Some(0));
        assert_eq!(&buf[..11], b"dev/ashmem\0");
        assert_eq!(ioctl(fd, SET_NAME, name.as_ptr() as u64), Some(0));
        assert_eq!(ioctl(fd, SET_SIZE, 3 * PAGE), Some(0));
        assert_eq!(ioctl(fd, GET_SIZE, 0), Some(3 * PAGE as i64));
        assert_eq!(ioctl(fd, SET_PROT_MASK, 3), Some(0));
        assert_eq!(ioctl(fd, SET_PROT_MASK, 7), Some(-(EINVAL as i64)));
        assert_eq!(ioctl(fd, GET_PROT_MASK, 0), Some(3));
        // Pinning needs a mapping; a mapping must fit and respect the mask.
        let pin_arg = [PAGE as u32, PAGE as u32];
        assert_eq!(
            ioctl(fd, UNPIN, pin_arg.as_ptr() as u64),
            Some(-(EINVAL as i64))
        );
        assert_eq!(before_mmap(fd, 4 * PAGE, 3), Some(-(EINVAL as i64)));
        assert_eq!(before_mmap(fd, PAGE, 7), Some(-(EPERM as i64)));
        assert_eq!(before_mmap(fd, 3 * PAGE, 3), None);
        assert_eq!(ioctl(fd, SET_SIZE, PAGE), Some(-(EINVAL as i64)));
        assert_eq!(ioctl(fd, UNPIN, pin_arg.as_ptr() as u64), Some(0));
        assert_eq!(
            ioctl(fd, GET_PIN_STATUS, pin_arg.as_ptr() as u64),
            Some(IS_UNPINNED)
        );
        assert_eq!(ioctl(fd, PIN, pin_arg.as_ptr() as u64), Some(NOT_PURGED));
        assert_eq!(
            ioctl(fd, GET_PIN_STATUS, [0u32, 0].as_ptr() as u64),
            Some(IS_PINNED)
        );
        assert_eq!(ioctl(fd, GET_NAME, buf.as_mut_ptr() as u64), Some(0));
        assert_eq!(&buf[..13], &name);
        // SAFETY: our fds.
        unsafe { libc::close(fd) };
    }
}
