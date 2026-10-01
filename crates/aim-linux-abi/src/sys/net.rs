//! Sockets: AF_UNIX with Linux semantics on Darwin, AF_INET/AF_INET6
//! passed through with address and option translation, AF_NETLINK
//! (`netlink`) and AF_PACKET (`packet`) on the kernel's devices (`netif`).
//!
//! - **SEQPACKET** (Darwin has none for AF_UNIX) is a stream socket carrying
//!   frames: a 16-byte header (length, sender pid, uid, gid) and the
//!   payload. A receive returns one whole message (MSG_TRUNC when the buffer
//!   is short); EOF and POLLHUP come from the stream.
//! - **Datagram** sockets prefix each datagram with the same credentials
//!   header (magic instead of length), stripped on receive, since Darwin
//!   cannot attach SCM_CREDENTIALS. A datagram without it (from a host
//!   program) is delivered unchanged.
//! - **Credentials:** SCM_CREDENTIALS (with SO_PASSCRED) and SO_PEERCRED
//!   report guest identities: the sender's own for framed messages, the
//!   peer pid's `<runtime>/identity/by-pid/<pid>` for stream peers
//!   (`docs/guest-init-contract.md` section 4).
//! - **Names:** filesystem paths go through the path map. Host paths can
//!   exceed Darwin's 104-byte `sun_path`, so bind/connect use the short name
//!   inside the directory, made the thread's working directory with
//!   `pthread_fchdir_np` for the call. Abstract names (leading NUL) are files
//!   in a private directory. getsockname/getpeername return guest names.
//! - **Recognition across processes:** SEQPACKET and datagram sockets carry
//!   a marker (the SO_LINGER time, unused while lingering is off); sockets
//!   guest-init created are listed in `<runtime>/sockets`.
//! - **Options Darwin has no place for:** an AF_INET/AF_INET6 socket given
//!   SO_MARK, SO_BINDTODEVICE or TCP_USER_TIMEOUT gets an entry holding
//!   them (`Family::Inet`); SO_BINDTODEVICE also binds the host socket to
//!   the host interface behind the device (`IP_BOUND_IF`), and
//!   TCP_USER_TIMEOUT sets Darwin's `TCP_RXT_CONNDROPTIME` in seconds. A datagram socket
//!   connected to port 0 is connected to the discard port on the host.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::fdtab::{self, Kind};
use crate::errno::{self, EAGAIN, EBADF, EINVAL};
use crate::vfs;

const L_AF_UNIX: u16 = 1;
const L_AF_INET: u16 = 2;
const L_AF_INET6: u16 = 10;
const L_AF_KEY: u16 = 15;
const PF_KEY_V2: u64 = 2;

const L_SOCK_STREAM: u64 = 1;
const L_SOCK_DGRAM: u64 = 2;
const L_SOCK_RAW: u64 = 3;
const L_SOCK_SEQPACKET: u64 = 5;
const L_SOCK_NONBLOCK: u64 = 0o4000;
const L_SOCK_CLOEXEC: u64 = 0o2000000;

const EAFNOSUPPORT: i64 = 97;
pub(super) const ENOTSOCK: i64 = 88;
const EPROTONOSUPPORT: i64 = 93;
const ENOPROTOOPT: i64 = 92;
const EOPNOTSUPP: i64 = 95;

// Linux MSG_* flags.
const L_MSG_OOB: u64 = 1;
const L_MSG_PEEK: u64 = 2;
const L_MSG_DONTROUTE: u64 = 4;
const L_MSG_CTRUNC: i32 = 8;
const L_MSG_TRUNC: u64 = 0x20;
const L_MSG_DONTWAIT: u64 = 0x40;
const L_MSG_EOR: u64 = 0x80;
const L_MSG_WAITALL: u64 = 0x100;
const L_MSG_NOSIGNAL: u64 = 0x4000;
const L_MSG_CMSG_CLOEXEC: u64 = 0x4000_0000;

const L_SOL_SOCKET: i32 = 1;
const L_SCM_RIGHTS: i32 = 1;
const L_SCM_CREDENTIALS: i32 = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SockType {
    Stream,
    Dgram,
    SeqPacket,
}

/// What kind of socket a [`Sock`] is.
pub enum Family {
    Unix,
    Inet(InetOpts),
    Netlink(super::netlink::Socket),
    Packet(Arc<super::packet::Socket>),
}

/// SO_MARK, SO_BINDTODEVICE and IP_MULTICAST_ALL of an AF_INET/AF_INET6
/// socket, whether it is connected to port 0 (`datagram_to_port_zero`) or
/// to the virtual DHCP server (`connect_to_router`), and whether it is an
/// IPv4 ping socket (`recv_ping`).
#[derive(Default)]
pub struct InetOpts {
    mark: std::sync::atomic::AtomicU32,
    device: Mutex<String>,
    /// IP_MULTICAST_ALL cleared (Linux sets it on a new socket).
    multicast_own: AtomicBool,
    /// TCP_USER_TIMEOUT in milliseconds, as set.
    user_timeout: std::sync::atomic::AtomicU32,
    port_zero: AtomicBool,
    router: AtomicBool,
    icmp4: AtomicBool,
}

/// A socket's Linux state: an AF_UNIX socket's, or another family's.
pub struct Sock {
    pub ty: SockType,
    pub family: Family,
    /// SO_PASSCRED: attach SCM_CREDENTIALS to received messages.
    passcred: AtomicBool,
    /// Serializes framed receives, so one message is read whole.
    recv: Mutex<()>,
    /// Linux sockaddr of the local and peer names, when known.
    local: Mutex<Option<Vec<u8>>>,
    peer: Mutex<Option<Vec<u8>>>,
}

impl Sock {
    fn new(ty: SockType) -> Arc<Sock> {
        Sock::of(ty, Family::Unix)
    }

    fn of(ty: SockType, family: Family) -> Arc<Sock> {
        Arc::new(Sock {
            ty,
            family,
            passcred: AtomicBool::new(false),
            recv: Mutex::new(()),
            local: Mutex::new(None),
            peer: Mutex::new(None),
        })
    }
}

/// Fork: a socket's Linux state.
pub(super) fn save_sock(s: &Sock, w: &mut super::fork_state::Writer) {
    w.u32(match s.ty {
        SockType::Stream => 0,
        SockType::Dgram => 1,
        SockType::SeqPacket => 2,
    });
    w.bool(s.passcred.load(std::sync::atomic::Ordering::Relaxed));
    w.opt(s.local.lock().unwrap().as_deref(), |w, a| w.bytes(a));
    w.opt(s.peer.lock().unwrap().as_deref(), |w, a| w.bytes(a));
    match &s.family {
        Family::Unix => w.u32(0),
        Family::Inet(o) => {
            w.u32(1);
            w.u32(o.mark.load(Ordering::Relaxed));
            w.bytes(o.device.lock().unwrap().as_bytes());
            w.bool(o.port_zero.load(Ordering::Relaxed));
            w.bool(o.router.load(Ordering::Relaxed));
            w.bool(o.icmp4.load(Ordering::Relaxed));
            w.bool(o.multicast_own.load(Ordering::Relaxed));
            w.u32(o.user_timeout.load(Ordering::Relaxed));
        }
        Family::Netlink(n) => {
            w.u32(2);
            n.save(w);
        }
        Family::Packet(p) => {
            w.u32(3);
            p.save(w);
        }
    }
}

pub(super) fn load_sock(r: &mut super::fork_state::Reader) -> Arc<Sock> {
    let ty = match r.u32() {
        0 => SockType::Stream,
        1 => SockType::Dgram,
        _ => SockType::SeqPacket,
    };
    let passcred = r.bool();
    let local = r.opt(|r| r.bytes());
    let peer = r.opt(|r| r.bytes());
    let family = match r.u32() {
        1 => {
            let o = InetOpts::default();
            o.mark.store(r.u32(), Ordering::Relaxed);
            *o.device.lock().unwrap() = String::from_utf8_lossy(&r.bytes()).into_owned();
            o.port_zero.store(r.bool(), Ordering::Relaxed);
            o.router.store(r.bool(), Ordering::Relaxed);
            o.icmp4.store(r.bool(), Ordering::Relaxed);
            o.multicast_own.store(r.bool(), Ordering::Relaxed);
            o.user_timeout.store(r.u32(), Ordering::Relaxed);
            Family::Inet(o)
        }
        2 => Family::Netlink(super::netlink::Socket::load(r)),
        3 => Family::Packet(super::packet::Socket::load(r)),
        _ => Family::Unix,
    };
    let s = Sock::of(ty, family);
    s.passcred.store(passcred, Ordering::Relaxed);
    *s.local.lock().unwrap() = local;
    *s.peer.lock().unwrap() = peer;
    s
}

/// The socket's Linux state, of any family.
fn any_sock(fd: i32) -> Option<Arc<Sock>> {
    match fdtab::get(fd) {
        Some(Kind::Sock(s)) => Some(s),
        _ => None,
    }
}

/// An AF_UNIX socket's Linux state.
fn sock(fd: i32) -> Option<Arc<Sock>> {
    match fdtab::get(fd) {
        Some(Kind::Sock(s)) if matches!(s.family, Family::Unix) => Some(s),
        _ => None,
    }
}

// ---- recognition --------------------------------------------------------

const MARK_SEQPACKET: i32 = 0x5351;
const MARK_DGRAM: i32 = 0x4447;

fn set_int(fd: i32, level: i32, opt: i32, v: i32) {
    // SAFETY: setting an int option on a socket.
    unsafe { libc::setsockopt(fd, level, opt, (&v as *const i32).cast(), 4) };
}

fn get_int(fd: i32, level: i32, opt: i32) -> Option<i32> {
    let mut v = 0i32;
    let mut len = 4u32;
    // SAFETY: reading an int option.
    (unsafe { libc::getsockopt(fd, level, opt, (&mut v as *mut i32).cast(), &mut len) } == 0)
        .then_some(v)
}

fn mark(fd: i32, ty: SockType) {
    let l = libc::linger {
        l_onoff: 0,
        l_linger: match ty {
            SockType::SeqPacket => MARK_SEQPACKET,
            SockType::Dgram => MARK_DGRAM,
            SockType::Stream => return,
        },
    };
    // SAFETY: setting SO_LINGER (off) on our socket.
    unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_LINGER,
            (&l as *const libc::linger).cast(),
            std::mem::size_of::<libc::linger>() as u32,
        )
    };
}

/// The `<runtime>/sockets` line (last one wins) for a socket name: its
/// guest path, guest type and passcred. guest-init binds by the short name
/// inside the directory, so a bare name matches a path's last component.
fn sockets_table(name: &str) -> Option<(String, SockType, bool)> {
    let text = std::fs::read_to_string(vfs::runtime_dir()?.join("sockets")).ok()?;
    let mut found = None;
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 5 {
            continue;
        }
        let short = !name.contains('/') && f[1].rsplit('/').next() == Some(name);
        if f[1] != name && !short {
            continue;
        }
        let ty = match f[2] {
            "seqpacket" => SockType::SeqPacket,
            "dgram" => SockType::Dgram,
            _ => SockType::Stream,
        };
        found = Some((f[1].to_string(), ty, f[4] == "passcred"));
    }
    found
}

/// A Linux AF_UNIX sockaddr for a guest path.
fn unix_addr(guest: &str) -> Vec<u8> {
    let mut v = L_AF_UNIX.to_le_bytes().to_vec();
    v.extend_from_slice(guest.as_bytes());
    v.push(0);
    v
}

/// Recognize an AF_UNIX socket that arrived from elsewhere (exec,
/// SCM_RIGHTS, binder) and give it its Linux state.
pub fn adopt(fd: i32) {
    let Some(host_ty) = get_int(fd, libc::SOL_SOCKET, libc::SO_TYPE) else {
        return;
    };
    let mut sa: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::sockaddr_storage>() as u32;
    // SAFETY: getsockname into local storage.
    if unsafe {
        libc::getsockname(
            fd,
            (&mut sa as *mut libc::sockaddr_storage).cast(),
            &mut len,
        )
    } < 0
        || sa.ss_family as i32 != libc::AF_UNIX
    {
        return;
    }
    if let Some(n) = netlink_of_host(&sa) {
        fdtab::insert(
            fd,
            Kind::Sock(Sock::of(SockType::Dgram, Family::Netlink(n))),
        );
        return;
    }
    let mut l = libc::linger {
        l_onoff: 0,
        l_linger: 0,
    };
    let mut ll = std::mem::size_of::<libc::linger>() as u32;
    // SAFETY: reading SO_LINGER into a local.
    unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_LINGER,
            (&mut l as *mut libc::linger).cast(),
            &mut ll,
        )
    };
    let mut local = linux_addr_of_host(&sa, len);
    let listed = local
        .as_deref()
        .and_then(unix_path_of)
        .and_then(|n| sockets_table(&n));
    if let Some((guest, ..)) = &listed {
        local = Some(unix_addr(guest));
    }
    if l.l_linger == aim_sync_file::MARK {
        super::sync_file::adopt(fd);
        return;
    }
    if l.l_linger == super::evdev::MARK {
        super::evdev::adopt(fd);
        return;
    }
    let passcred = listed.as_ref().is_some_and(|l| l.2);
    let ty = match (l.l_linger, &listed) {
        (MARK_SEQPACKET, _) => SockType::SeqPacket,
        (MARK_DGRAM, _) => SockType::Dgram,
        (_, Some((_, t, _))) => *t,
        _ if host_ty == libc::SOCK_DGRAM => SockType::Dgram,
        _ => SockType::Stream,
    };
    let s = Sock::new(ty);
    s.passcred.store(passcred, Ordering::Relaxed);
    *s.local.lock().unwrap() = local;
    fdtab::insert(fd, Kind::Sock(s));
}

/// A netlink socket's state from its host name: a name in the netlink
/// directory (or its last component, when it was bound relative to it).
fn netlink_of_host(sa: &libc::sockaddr_storage) -> Option<super::netlink::Socket> {
    // SAFETY: an AF_UNIX address.
    let sun = unsafe { &*(sa as *const libc::sockaddr_storage as *const libc::sockaddr_un) };
    let path: Vec<u8> = sun
        .sun_path
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    let path = Path::new(std::ffi::OsStr::from_bytes(&path));
    super::netlink::adopt(path.file_name()?)
}

// ---- credentials ---------------------------------------------------------

/// Linux `struct ucred`.
#[derive(Clone, Copy, Default)]
struct Cred {
    pid: i32,
    uid: u32,
    gid: u32,
}

/// The guest identity of host process `pid` (`sys::cred`, from the
/// process table; this process's own for itself).
fn cred_of(pid: i32) -> Cred {
    let c = super::cred::peer(pid);
    Cred {
        pid: c.pid,
        uid: c.uid,
        gid: c.gid,
    }
}

fn own_cred() -> Cred {
    cred_of(super::process::getpid() as i32)
}

fn peer_cred(fd: i32) -> Option<Cred> {
    Some(cred_of(get_int(fd, 0, libc::LOCAL_PEERPID)?))
}

const FRAME: usize = 16;
const DGRAM_MAGIC: u32 = 0x5243_584c; // "LXCR"

fn header(first: u32, c: Cred) -> [u8; FRAME] {
    let mut h = [0u8; FRAME];
    h[0..4].copy_from_slice(&first.to_le_bytes());
    h[4..8].copy_from_slice(&c.pid.to_le_bytes());
    h[8..12].copy_from_slice(&c.uid.to_le_bytes());
    h[12..16].copy_from_slice(&c.gid.to_le_bytes());
    h
}

fn parse_header(h: &[u8; FRAME]) -> (u32, Cred) {
    let w = |i: usize| u32::from_le_bytes(h[i..i + 4].try_into().unwrap());
    (
        w(0),
        Cred {
            pid: w(4) as i32,
            uid: w(8),
            gid: w(12),
        },
    )
}

// ---- addresses ------------------------------------------------------------

unsafe extern "C" {
    /// Sets the calling thread's working directory; -1 returns it to the
    /// process's.
    fn pthread_fchdir_np(fd: libc::c_int) -> libc::c_int;
}

/// Where an address points on the host.
pub(super) enum Target {
    /// A ready Darwin sockaddr.
    Host(Vec<u8>),
    /// An AF_UNIX name relative to a directory.
    UnixAt { dir: PathBuf, name: Vec<u8> },
    /// An AF_UNIX address with no name (Linux autobind).
    Unnamed,
}

fn sun(name: &[u8]) -> Vec<u8> {
    let mut v = vec![0u8; std::mem::size_of::<libc::sockaddr_un>()];
    v[0] = v.len() as u8;
    v[1] = libc::AF_UNIX as u8;
    v[2..2 + name.len()].copy_from_slice(name);
    v
}

/// Run `f` with the host sockaddr of `t`.
pub(super) fn with_target(t: &Target, f: impl FnOnce(*const libc::sockaddr, u32) -> i64) -> i64 {
    match t {
        Target::Host(sa) => f(sa.as_ptr().cast(), sa.len() as u32),
        Target::Unnamed => -(EINVAL as i64),
        Target::UnixAt { dir, name } => {
            let Ok(c) = CString::new(dir.as_os_str().as_bytes()) else {
                return -(EINVAL as i64);
            };
            // SAFETY: a directory fd made this thread's cwd for one call,
            // then restored.
            unsafe {
                let dfd = libc::open(
                    c.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
                );
                if dfd < 0 {
                    return -(errno::last() as i64);
                }
                if pthread_fchdir_np(dfd) != 0 {
                    let e = errno::last();
                    libc::close(dfd);
                    return -(e as i64);
                }
                let sa = sun(name);
                let r = f(sa.as_ptr().cast(), sa.len() as u32);
                let e = errno::last();
                pthread_fchdir_np(-1);
                libc::close(dfd);
                if r < 0 { -(e as i64) } else { r }
            }
        }
    }
}

/// Directory holding abstract-namespace sockets: one per guest instance.
fn abstract_dir() -> PathBuf {
    let d = match vfs::runtime_dir() {
        Some(r) => r.join("abstract"),
        None => {
            use sha2::Digest;
            let h = sha2::Sha256::digest(vfs::root().as_os_str().as_bytes());
            let tag: String = h[..6].iter().map(|b| format!("{b:02x}")).collect();
            // SAFETY: trivial.
            std::env::temp_dir().join(format!("linux-abi-abstract-{}-{tag}", unsafe {
                libc::getuid()
            }))
        }
    };
    let _ = std::fs::create_dir_all(&d);
    d
}

fn escape(name: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for &b in name {
        if b.is_ascii_alphanumeric() || b"._-".contains(&b) {
            out.push(b);
        } else {
            out.extend_from_slice(format!("%{b:02x}").as_bytes());
        }
    }
    if out.len() > 100 {
        use sha2::Digest;
        let h = sha2::Sha256::digest(name);
        out = h
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
            .into_bytes();
    }
    out
}

fn unescape(name: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < name.len() {
        if name[i] == b'%'
            && i + 2 < name.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&name[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(name[i]);
            i += 1;
        }
    }
    out
}

pub(super) fn host_target_of_path(host: &Path) -> Target {
    let bytes = host.as_os_str().as_bytes();
    if bytes.len() < 104 {
        return Target::Host(sun(bytes));
    }
    Target::UnixAt {
        dir: host.parent().map(Path::to_path_buf).unwrap_or_default(),
        name: host
            .file_name()
            .map(|n| n.as_bytes().to_vec())
            .unwrap_or_default(),
    }
}

/// Whether `addr` is an AF_INET6 address for an AF_INET socket, which
/// Linux refuses with EAFNOSUPPORT (Darwin: EINVAL). libcore relies on it:
/// it tries a v4-mapped IPv6 address first and falls back to AF_INET on
/// that errno.
fn family_mismatch(fd: i32, addr: &[u8]) -> bool {
    if addr.len() < 2 || u16::from_le_bytes([addr[0], addr[1]]) != L_AF_INET6 {
        return false;
    }
    let mut sa: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::sockaddr_storage>() as u32;
    // SAFETY: getsockname into local storage.
    unsafe {
        libc::getsockname(
            fd,
            (&mut sa as *mut libc::sockaddr_storage).cast(),
            &mut len,
        )
    };
    sa.ss_family as i32 == libc::AF_INET
}

/// Translate a guest sockaddr. `bind`: the last path component is not
/// followed. Returns the target and the guest name to remember.
fn target_of(ptr: u64, len: u32, bind: bool) -> Result<(Target, Vec<u8>), i64> {
    if ptr == 0 || !(2..=128).contains(&len) {
        return Err(-(EINVAL as i64));
    }
    // SAFETY: guest sockaddr of `len` bytes.
    let g = unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize) }.to_vec();
    let fam = u16::from_le_bytes([g[0], g[1]]);
    match fam {
        L_AF_UNIX => {
            let path = &g[2..];
            if path.is_empty() {
                return Ok((Target::Unnamed, g));
            }
            if path[0] == 0 {
                let name = escape(&path[1..]);
                return Ok((
                    Target::UnixAt {
                        dir: abstract_dir(),
                        name,
                    },
                    g,
                ));
            }
            let end = path.iter().position(|&c| c == 0).unwrap_or(path.len());
            let r =
                vfs::resolve(vfs::LINUX_AT_FDCWD, &path[..end], !bind).map_err(|e| -(e as i64))?;
            if bind && r.read_only() {
                return Err(-(libc::EROFS as i64));
            }
            let host = PathBuf::from(std::ffi::OsStr::from_bytes(r.host.as_bytes()));
            let mut name = (L_AF_UNIX).to_le_bytes().to_vec();
            name.extend_from_slice(r.guest.as_bytes());
            name.push(0);
            Ok((host_target_of_path(&host), name))
        }
        L_AF_INET if len >= 16 => {
            let mut h = g[..16].to_vec();
            h[0] = 16;
            h[1] = libc::AF_INET as u8;
            Ok((Target::Host(h), g))
        }
        L_AF_INET6 if len >= 24 => {
            let mut h = vec![0u8; 28];
            h[..len.min(28) as usize].copy_from_slice(&g[..len.min(28) as usize]);
            h[0] = 28;
            h[1] = libc::AF_INET6 as u8;
            Ok((Target::Host(h), g))
        }
        L_AF_INET | L_AF_INET6 => Err(-(EINVAL as i64)),
        _ => Err(-EAFNOSUPPORT),
    }
}

/// Whether a Linux sockaddr is an AF_UNIX abstract name.
fn is_abstract(addr: &[u8]) -> bool {
    addr.len() > 2 && addr[..2] == L_AF_UNIX.to_le_bytes() && addr[2] == 0
}

/// Guest path of a Linux AF_UNIX sockaddr with a filesystem name.
fn unix_path_of(addr: &[u8]) -> Option<String> {
    let p = addr.get(2..)?;
    if p.first().is_none_or(|&c| c == 0) {
        return None;
    }
    let end = p.iter().position(|&c| c == 0).unwrap_or(p.len());
    Some(String::from_utf8_lossy(&p[..end]).into_owned())
}

/// A Linux sockaddr for a host one.
fn linux_addr_of_host(sa: &libc::sockaddr_storage, len: u32) -> Option<Vec<u8>> {
    // SAFETY: viewing the storage the kernel filled.
    let raw = unsafe {
        std::slice::from_raw_parts(
            (sa as *const libc::sockaddr_storage).cast::<u8>(),
            (len as usize).min(std::mem::size_of::<libc::sockaddr_storage>()),
        )
    };
    if raw.len() < 2 {
        return Some(L_AF_UNIX.to_le_bytes().to_vec());
    }
    let (fam, rest) = (raw[1] as i32, &raw[2..]);
    let lfam = match fam {
        libc::AF_UNIX => L_AF_UNIX,
        libc::AF_INET => L_AF_INET,
        libc::AF_INET6 => L_AF_INET6,
        _ => return None,
    };
    let mut out = lfam.to_le_bytes().to_vec();
    if fam != libc::AF_UNIX {
        out.extend_from_slice(rest);
        return Some(out);
    }
    let end = rest.iter().position(|&c| c == 0).unwrap_or(rest.len());
    let path = std::ffi::OsStr::from_bytes(&rest[..end]);
    if path.is_empty() {
        return Some(out);
    }
    let host = Path::new(path);
    let adir = abstract_dir();
    if let Ok(name) = host.strip_prefix(&adir) {
        out.push(0);
        out.extend_from_slice(&unescape(name.as_os_str().as_bytes()));
        return Some(out);
    }
    match vfs::guest_path_of_host(host) {
        Some(g) if host.is_absolute() => out.extend_from_slice(g.as_bytes()),
        _ => out.extend_from_slice(path.as_bytes()),
    }
    out.push(0);
    Some(out)
}

/// Store a Linux sockaddr into guest (addr, *addrlen), Linux-style: the
/// length written back is the full length even when truncated.
fn put_addr(addr: &[u8], out: u64, outlen: u64) {
    if out == 0 || outlen == 0 {
        return;
    }
    // SAFETY: guest socklen_t and buffer of that many bytes.
    unsafe {
        let cap = (outlen as *const u32).read_unaligned() as usize;
        std::ptr::copy_nonoverlapping(addr.as_ptr(), out as *mut u8, cap.min(addr.len()));
        (outlen as *mut u32).write_unaligned(addr.len() as u32);
    }
}

// ---- socket lifecycle -----------------------------------------------------

fn new_host_socket(domain: i32, ty: i32) -> Result<i32, i64> {
    // SAFETY: plain socket.
    let fd = unsafe { libc::socket(domain, ty, 0) };
    if fd < 0 {
        return Err(-(errno::last() as i64));
    }
    set_int(fd, libc::SOL_SOCKET, libc::SO_NOSIGPIPE, 1);
    Ok(fd)
}

/// Linux's default buffers (net.core.[rw]mem_default); Darwin's AF_UNIX
/// defaults (2 KiB datagrams, 8 KiB streams) are too small for logd.
const BUFFER: i32 = 212_992;

/// Give a host datagram socket Linux's default buffers.
pub(super) fn size_buffers(fd: i32) {
    set_int(fd, libc::SOL_SOCKET, libc::SO_SNDBUF, BUFFER);
    set_int(fd, libc::SOL_SOCKET, libc::SO_RCVBUF, BUFFER);
}

fn setup_unix(fd: i32, ty: SockType) {
    if ty != SockType::Stream {
        size_buffers(fd);
        mark(fd, ty);
    }
}

fn unix_type(t: u64) -> Result<(SockType, i32), i64> {
    Ok(match t {
        L_SOCK_STREAM => (SockType::Stream, libc::SOCK_STREAM),
        L_SOCK_DGRAM => (SockType::Dgram, libc::SOCK_DGRAM),
        L_SOCK_SEQPACKET => (SockType::SeqPacket, libc::SOCK_STREAM),
        _ => return Err(-EPROTONOSUPPORT),
    })
}

pub fn socket(a: [u64; 6]) -> i64 {
    let (domain, ty, proto) = (a[0] as u16, a[1], a[2]);
    let (nonblock, cloexec) = (ty & L_SOCK_NONBLOCK != 0, ty & L_SOCK_CLOEXEC != 0);
    let base = ty & 0xf;
    let fd = match domain {
        L_AF_UNIX => {
            if proto != 0 {
                return -EPROTONOSUPPORT;
            }
            let (st, host_ty) = match unix_type(base) {
                Ok(t) => t,
                Err(e) => return e,
            };
            let fd = match new_host_socket(libc::AF_UNIX, host_ty) {
                Ok(fd) => fd,
                Err(e) => return e,
            };
            setup_unix(fd, st);
            fdtab::insert(fd, Kind::Sock(Sock::new(st)));
            fd
        }
        L_AF_INET | L_AF_INET6 => {
            let host_ty = match base {
                L_SOCK_STREAM => libc::SOCK_STREAM,
                L_SOCK_DGRAM => libc::SOCK_DGRAM,
                L_SOCK_RAW => libc::SOCK_RAW,
                _ => return -EPROTONOSUPPORT,
            };
            let d = if domain == L_AF_INET {
                libc::AF_INET
            } else {
                libc::AF_INET6
            };
            // SAFETY: plain socket with the guest's protocol number (IPPROTO
            // values agree).
            let fd = unsafe { libc::socket(d, host_ty, proto as i32) };
            if fd < 0 {
                return -(errno::last() as i64);
            }
            set_int(fd, libc::SOL_SOCKET, libc::SO_NOSIGPIPE, 1);
            if d == libc::AF_INET && base == L_SOCK_DGRAM && proto == IPPROTO_ICMP {
                let o = InetOpts::default();
                o.icmp4.store(true, Ordering::Relaxed);
                fdtab::insert(fd, Kind::Sock(Sock::of(SockType::Dgram, Family::Inet(o))));
            }
            fd
        }
        // PF_KEY: libbpf_android's synchronizeKernelRCU opens and closes
        // one for the synchronize_rcu() of its release, before it reads a
        // bpf map it swapped out. Map updates here are visible at once, so
        // the socket only has to exist; it is an unconnected datagram
        // socket, and no key management is offered through it.
        L_AF_KEY if base == L_SOCK_RAW && proto == PF_KEY_V2 => {
            let fd = match new_host_socket(libc::AF_UNIX, libc::SOCK_DGRAM) {
                Ok(fd) => fd,
                Err(e) => return e,
            };
            fdtab::insert(fd, Kind::Sock(Sock::new(SockType::Dgram)));
            fd
        }
        super::netlink::L_AF_NETLINK => {
            if base != L_SOCK_RAW && base != L_SOCK_DGRAM {
                return -94; // ESOCKTNOSUPPORT
            }
            let (fd, n) = match super::netlink::socket(base, proto) {
                Ok(s) => s,
                Err(e) => return e,
            };
            fdtab::insert(
                fd,
                Kind::Sock(Sock::of(SockType::Dgram, Family::Netlink(n))),
            );
            fd
        }
        super::packet::L_AF_PACKET => {
            let (fd, p) = match super::packet::socket(base, proto) {
                Ok(s) => s,
                Err(e) => return e,
            };
            fdtab::insert(fd, Kind::Sock(Sock::of(SockType::Dgram, Family::Packet(p))));
            fd
        }
        _ => return -EAFNOSUPPORT,
    };
    fdtab::set_flags(fd, nonblock, cloexec);
    fd as i64
}

pub fn socketpair(a: [u64; 6]) -> i64 {
    let (domain, ty, sv) = (a[0] as u16, a[1], a[3]);
    if domain != L_AF_UNIX {
        return -EAFNOSUPPORT;
    }
    let (st, host_ty) = match unix_type(ty & 0xf) {
        Ok(t) => t,
        Err(e) => return e,
    };
    let mut fds = [0i32; 2];
    // SAFETY: socketpair into a local array.
    if unsafe { libc::socketpair(libc::AF_UNIX, host_ty, 0, fds.as_mut_ptr()) } < 0 {
        return -(errno::last() as i64);
    }
    for fd in fds {
        set_int(fd, libc::SOL_SOCKET, libc::SO_NOSIGPIPE, 1);
        setup_unix(fd, st);
        fdtab::set_flags(fd, ty & L_SOCK_NONBLOCK != 0, ty & L_SOCK_CLOEXEC != 0);
        fdtab::insert(fd, Kind::Sock(Sock::new(st)));
    }
    // SAFETY: guest int[2].
    unsafe { (sv as *mut [i32; 2]).write_unaligned(fds) };
    0
}

/// Whether `fd` is a file the layer keeps as a host socket that is no
/// socket to the guest (eventfd, timerfd, evdev, sync_file, binder):
/// socket calls on it fail with ENOTSOCK, as on Linux.
pub fn hidden_socket(fd: i32) -> bool {
    matches!(
        fdtab::get(fd),
        Some(Kind::Event(_) | Kind::Timer(_) | Kind::Evdev(_) | Kind::SyncFile | Kind::Binder(_))
    )
}

fn is_socket(fd: i32) -> Result<(), i64> {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat into a local buffer.
    if unsafe { libc::fstat(fd, &mut st) } < 0 {
        return Err(-(EBADF as i64));
    }
    if st.st_mode & libc::S_IFMT != libc::S_IFSOCK {
        return Err(-ENOTSOCK);
    }
    Ok(())
}

pub fn bind(a: [u64; 6]) -> i64 {
    let fd = a[0] as i32;
    if let Err(e) = is_socket(fd) {
        return e;
    }
    if let Some(s) = any_sock(fd) {
        match &s.family {
            Family::Netlink(n) => {
                return match super::netlink::parse_sockaddr(a[1], a[2] as u32) {
                    Ok((portid, groups)) => super::netlink::bind(n, portid, groups),
                    Err(e) => e,
                };
            }
            Family::Packet(p) => {
                return match super::packet::parse_sockaddr(a[1], a[2] as u32) {
                    Ok((proto, ifindex)) => super::packet::bind(p, proto, ifindex),
                    Err(e) => e,
                };
            }
            Family::Unix | Family::Inet(_) => {}
        }
    }
    let (t, name) = match target_of(a[1], a[2] as u32, true) {
        Ok(t) => t,
        Err(e) => return e,
    };
    if family_mismatch(fd, &name) {
        return -EAFNOSUPPORT;
    }
    if matches!(t, Target::Unnamed) {
        // Linux autobind: nothing to do for a Darwin AF_UNIX socket.
        return 0;
    }
    if let Some(r) = bind_on_link(fd, &name) {
        return r;
    }
    let bind_once = || {
        with_target(&t, |sa, len| {
            errno::check(unsafe { libc::bind(fd, sa, len) } as i64)
        })
    };
    let mut r = bind_once();
    if r == -98
        && is_abstract(&name)
        && let Target::UnixAt { dir, name } = &t
    {
        // A stale file of an abstract socket that was never unbound. A
        // datagram probe tells without queueing a connection: a live stream
        // socket answers EPROTOTYPE, a live datagram socket accepts, and a
        // file with no socket refuses.
        let probe = new_host_socket(libc::AF_UNIX, libc::SOCK_DGRAM).unwrap_or(-1);
        let c = with_target(&t, |sa, len| {
            errno::check(unsafe { libc::connect(probe, sa, len) } as i64)
        });
        // SAFETY: our probe socket.
        unsafe { libc::close(probe) };
        if c == -111 {
            let _ = std::fs::remove_file(dir.join(std::ffi::OsStr::from_bytes(name)));
            r = bind_once();
        }
    }
    if r == 0
        && let Some(s) = sock(fd)
    {
        *s.local.lock().unwrap() = Some(name);
    }
    r
}

pub fn connect(a: [u64; 6]) -> i64 {
    let fd = a[0] as i32;
    if let Err(e) = is_socket(fd) {
        return e;
    }
    if let Some(s) = any_sock(fd) {
        match &s.family {
            Family::Netlink(_) => {
                return match super::netlink::parse_sockaddr(a[1], a[2] as u32) {
                    Ok((portid, _)) => super::netlink::connect(portid),
                    Err(e) => e,
                };
            }
            Family::Packet(_) => return -EOPNOTSUPP,
            Family::Unix | Family::Inet(_) => {}
        }
    }
    let (mut t, name) = match target_of(a[1], a[2] as u32, false) {
        Ok(t) => t,
        Err(e) => return e,
    };
    if family_mismatch(fd, &name) {
        return -EAFNOSUPPORT;
    }
    if let Some(r) = connect_to_router(fd, &name) {
        return r;
    }
    let zero_port = datagram_to_port_zero(fd, &mut t);
    let mut r = with_target(&t, |sa, len| {
        errno::check(unsafe { libc::connect(fd, sa, len) } as i64)
    });
    if r == -(libc::ENOENT as i64) && is_abstract(&name) {
        // An abstract name nobody bound: refused, not missing.
        r = -111;
    }
    if (r == 0 || r == -115)
        && let Some(s) = sock(fd)
    {
        *s.peer.lock().unwrap() = Some(name);
    }
    if r == 0 {
        match (any_sock(fd), zero_port) {
            (Some(s), _) => {
                if let Family::Inet(o) = &s.family {
                    o.port_zero.store(zero_port, Ordering::Relaxed);
                }
            }
            (None, true) => {
                let o = InetOpts::default();
                o.port_zero.store(true, Ordering::Relaxed);
                fdtab::insert(fd, Kind::Sock(Sock::of(SockType::Dgram, Family::Inet(o))));
            }
            (None, false) => {}
        }
    }
    r
}

/// A datagram socket bound to a device other than the loopback that
/// connects to the DHCP server port (DhcpClient, once bound to its lease)
/// is connected to `eth0`'s virtual router, where its datagrams go
/// (`dhcp_to_router`), not to the Mac's network. The host socket stays
/// unconnected: the Mac's own DHCP client holds that address pair, so a
/// host connect fails with EADDRINUSE. None: an ordinary connect.
fn connect_to_router(fd: i32, name: &[u8]) -> Option<i64> {
    let s = any_sock(fd)?;
    let Family::Inet(o) = &s.family else {
        return None;
    };
    let server = name.len() >= 8
        && u16::from_le_bytes([name[0], name[1]]) == L_AF_INET
        && u16::from_be_bytes([name[2], name[3]]) == super::dhcp::SERVER_PORT;
    let routed = server && on_link(fd, o);
    o.router.store(routed, Ordering::Relaxed);
    if !routed {
        return None;
    }
    *s.peer.lock().unwrap() = Some(name.to_vec());
    Some(0)
}

/// A datagram socket bound to a device other than the loopback that binds
/// the DHCP client port (DhcpClient) is bound on the device's virtual link
/// only, as the local name it reports: its DHCP messages go to the virtual
/// router (`dhcp_to_router`), which answers on the packet sockets, and
/// nothing from the Mac's network reaches it. A host bind would take the
/// Mac's UDP port 68 from the Mac and from other guests. None: an ordinary
/// bind.
fn bind_on_link(fd: i32, name: &[u8]) -> Option<i64> {
    let s = any_sock(fd)?;
    let Family::Inet(o) = &s.family else {
        return None;
    };
    let client = name.len() >= 8
        && u16::from_le_bytes([name[0], name[1]]) == L_AF_INET
        && u16::from_be_bytes([name[2], name[3]]) == super::dhcp::CLIENT_PORT;
    if !client || !on_link(fd, o) {
        return None;
    }
    let mut local = s.local.lock().unwrap();
    if local.is_some() || host_inet_name(fd, false).is_some_and(|(port, _)| port != 0) {
        return Some(-(EINVAL as i64));
    }
    *local = Some(name[..name.len().min(16)].to_vec());
    Some(0)
}

/// Whether `fd` is a datagram socket bound to a device other than the
/// loopback, whose DHCP messages go to the virtual router.
fn on_link(fd: i32, o: &InetOpts) -> bool {
    let device = o.device.lock().unwrap().clone();
    !device.is_empty()
        && !o.icmp4.load(Ordering::Relaxed)
        && get_int(fd, libc::SOL_SOCKET, libc::SO_TYPE) == Some(libc::SOCK_DGRAM)
        && super::netif::by_name(&device).is_some_and(|l| !l.loopback())
}

/// The discard port, which stands in for port 0 on the host.
const DISCARD: u16 = 9;

/// Linux connects a datagram socket to port 0 (bionic's and
/// DnsResolver's "have IPv4/IPv6" probes do, then read the source address
/// the route chose); Darwin refuses it. The host socket is connected to
/// the discard port instead, and getpeername reports port 0. Returns
/// whether `t` was changed.
fn datagram_to_port_zero(fd: i32, t: &mut Target) -> bool {
    let Target::Host(sa) = t else {
        return false;
    };
    let inet = sa[1] as i32 == libc::AF_INET || sa[1] as i32 == libc::AF_INET6;
    if !inet
        || sa[2..4] != [0, 0]
        || get_int(fd, libc::SOL_SOCKET, libc::SO_TYPE) != Some(libc::SOCK_DGRAM)
    {
        return false;
    }
    sa[2..4].copy_from_slice(&DISCARD.to_be_bytes());
    true
}

pub fn listen(a: [u64; 6]) -> i64 {
    // SAFETY: plain listen.
    errno::check(unsafe { libc::listen(a[0] as i32, a[1] as i32) } as i64)
}

pub fn accept4(a: [u64; 6]) -> i64 {
    let (fd, addr, addrlen, flags) = (a[0] as i32, a[1], a[2], a[3]);
    if flags & !(L_SOCK_NONBLOCK | L_SOCK_CLOEXEC) != 0 {
        return -(EINVAL as i64);
    }
    let mut sa: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::sockaddr_storage>() as u32;
    // SAFETY: accept into local storage.
    let nfd = unsafe {
        libc::accept(
            fd,
            (&mut sa as *mut libc::sockaddr_storage).cast(),
            &mut len,
        )
    };
    if nfd < 0 {
        return -(errno::last() as i64);
    }
    // Darwin accepted sockets inherit O_NONBLOCK; Linux's do not.
    // SAFETY: plain fcntl on the new fd.
    unsafe {
        let fl = libc::fcntl(nfd, libc::F_GETFL);
        libc::fcntl(nfd, libc::F_SETFL, fl & !libc::O_NONBLOCK);
    }
    fdtab::set_flags(
        nfd,
        flags & L_SOCK_NONBLOCK != 0,
        flags & L_SOCK_CLOEXEC != 0,
    );
    set_int(nfd, libc::SOL_SOCKET, libc::SO_NOSIGPIPE, 1);
    let peer = linux_addr_of_host(&sa, len);
    if let Some(l) = sock(fd) {
        let s = Sock::new(l.ty);
        setup_unix(nfd, l.ty);
        *s.local.lock().unwrap() = l.local.lock().unwrap().clone();
        s.passcred
            .store(l.passcred.load(Ordering::Relaxed), Ordering::Relaxed);
        fdtab::insert(nfd, Kind::Sock(s));
    }
    if let Some(p) = peer {
        put_addr(&p, addr, addrlen);
    }
    nfd as i64
}

fn name_of(fd: i32, peer: bool, out: u64, outlen: u64) -> i64 {
    if let Some(s) = any_sock(fd) {
        let name = match &s.family {
            Family::Netlink(_) if peer => Some(super::netlink::sockaddr_nl(0, 0)),
            Family::Netlink(n) => Some(n.local_name()),
            Family::Packet(_) if peer => return -107, // ENOTCONN
            Family::Packet(p) => Some(p.local_name()),
            Family::Inet(o) if peer && o.router.load(Ordering::Relaxed) => {
                s.peer.lock().unwrap().clone()
            }
            // Bound on a virtual link (`bind_on_link`).
            Family::Inet(_) if !peer && s.local.lock().unwrap().is_some() => {
                s.local.lock().unwrap().clone()
            }
            Family::Inet(o) if peer && o.port_zero.load(Ordering::Relaxed) => {
                let mut sa: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
                let mut len = std::mem::size_of::<libc::sockaddr_storage>() as u32;
                // SAFETY: getpeername into local storage.
                let r = unsafe {
                    libc::getpeername(
                        fd,
                        (&mut sa as *mut libc::sockaddr_storage).cast(),
                        &mut len,
                    )
                };
                if r < 0 {
                    return -(errno::last() as i64);
                }
                linux_addr_of_host(&sa, len).map(|mut a| {
                    a[2..4].fill(0);
                    a
                })
            }
            Family::Unix | Family::Inet(_) => None,
        };
        if let Some(n) = name {
            put_addr(&n, out, outlen);
            return 0;
        }
    }
    if let Some(s) = sock(fd) {
        let known = if peer { &s.peer } else { &s.local };
        if let Some(n) = known.lock().unwrap().clone() {
            // A peer name is only valid while connected.
            if !peer || get_int(fd, 0, libc::LOCAL_PEERPID).is_some() {
                put_addr(&n, out, outlen);
                return 0;
            }
        }
    }
    let mut sa: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::sockaddr_storage>() as u32;
    let p = (&mut sa as *mut libc::sockaddr_storage).cast();
    // SAFETY: into local storage.
    let r = unsafe {
        if peer {
            libc::getpeername(fd, p, &mut len)
        } else {
            libc::getsockname(fd, p, &mut len)
        }
    };
    if r < 0 {
        return -(errno::last() as i64);
    }
    match linux_addr_of_host(&sa, len) {
        Some(a) => {
            put_addr(&a, out, outlen);
            0
        }
        None => -(EOPNOTSUPP),
    }
}

pub fn getsockname(a: [u64; 6]) -> i64 {
    name_of(a[0] as i32, false, a[1], a[2])
}

pub fn getpeername(a: [u64; 6]) -> i64 {
    name_of(a[0] as i32, true, a[1], a[2])
}

pub fn shutdown(a: [u64; 6]) -> i64 {
    if a[1] > 2 {
        return -(EINVAL as i64);
    }
    // SAFETY: SHUT_RD/WR/RDWR agree.
    errno::check(unsafe { libc::shutdown(a[0] as i32, a[1] as i32) } as i64)
}

// ---- control messages -------------------------------------------------------

/// Linux cmsgs -> Darwin cmsgs. SCM_CREDENTIALS is dropped: receivers get
/// the sender's identity from the frame header or the peer.
fn control_to_host(ptr: u64, len: usize) -> Result<Vec<u8>, i64> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 16 <= len {
        // SAFETY: inside the guest control buffer.
        let (clen, level, ty) = unsafe {
            let p = (ptr as *const u8).add(off);
            (
                (p as *const u64).read_unaligned() as usize,
                (p.add(8) as *const i32).read_unaligned(),
                (p.add(12) as *const i32).read_unaligned(),
            )
        };
        if clen < 16 || off + clen > len {
            return Err(-(EINVAL as i64));
        }
        // An SCM_RIGHTS without fds passes nothing on Linux (libbase's
        // SendFileDescriptors sends one for an empty list).
        if level == L_SOL_SOCKET && ty == L_SCM_RIGHTS && clen >= 20 {
            let n = clen - 16;
            let hlen = 12 + n;
            out.extend_from_slice(&(hlen as u32).to_le_bytes());
            out.extend_from_slice(&libc::SOL_SOCKET.to_le_bytes());
            out.extend_from_slice(&libc::SCM_RIGHTS.to_le_bytes());
            // SAFETY: the fds follow the Linux header.
            out.extend_from_slice(unsafe {
                std::slice::from_raw_parts((ptr as *const u8).add(off + 16), n)
            });
            out.resize((out.len() + 3) & !3, 0);
        } else if !(level == L_SOL_SOCKET && matches!(ty, L_SCM_CREDENTIALS | L_SCM_RIGHTS)) {
            return Err(-(EINVAL as i64));
        }
        off += (clen + 7) & !7;
    }
    Ok(out)
}

/// Darwin cmsgs (plus credentials) -> Linux cmsgs in the guest buffer.
/// Returns (bytes written, MSG_CTRUNC if something did not fit).
fn control_to_guest(
    host: &[u8],
    cred: Option<Cred>,
    cloexec: bool,
    out: u64,
    cap: usize,
) -> (usize, i32) {
    let mut msgs: Vec<(i32, Vec<u8>)> = Vec::new();
    let mut off = 0usize;
    while off + 12 <= host.len() {
        let w = |i: usize| u32::from_le_bytes(host[off + i..off + i + 4].try_into().unwrap());
        let (clen, level, ty) = (w(0) as usize, w(4) as i32, w(8) as i32);
        if clen < 12 || off + clen > host.len() {
            break;
        }
        if level == libc::SOL_SOCKET && ty == libc::SCM_RIGHTS {
            let data = host[off + 12..off + clen].to_vec();
            for fd in data
                .chunks_exact(4)
                .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
            {
                fdtab::set_flags(fd, false, cloexec);
                fdtab::adopt(fd);
            }
            // Darwin can hand back a bare SCM_RIGHTS header (a sender's
            // empty one); Linux never delivers one, and libbase aborts on it.
            if !data.is_empty() {
                msgs.push((L_SCM_RIGHTS, data));
            }
        }
        off += (clen + 3) & !3;
    }
    if let Some(c) = cred {
        let mut d = c.pid.to_le_bytes().to_vec();
        d.extend_from_slice(&c.uid.to_le_bytes());
        d.extend_from_slice(&c.gid.to_le_bytes());
        msgs.push((L_SCM_CREDENTIALS, d));
    }
    let (mut w, mut flags) = (0usize, 0);
    for (ty, data) in msgs {
        let clen = 16 + data.len();
        let space = (clen + 7) & !7;
        if out == 0 || w + clen > cap {
            flags |= L_MSG_CTRUNC;
            if ty == L_SCM_RIGHTS {
                for fd in data
                    .chunks_exact(4)
                    .map(|c| i32::from_le_bytes(c.try_into().unwrap()))
                {
                    fdtab::on_close(fd);
                    // SAFETY: an fd we received and cannot deliver.
                    unsafe { libc::close(fd) };
                }
            }
            continue;
        }
        let mut rec = vec![0u8; space.min(cap - w)];
        rec[0..8].copy_from_slice(&(clen as u64).to_le_bytes());
        rec[8..12].copy_from_slice(&L_SOL_SOCKET.to_le_bytes());
        rec[12..16].copy_from_slice(&ty.to_le_bytes());
        rec[16..clen].copy_from_slice(&data);
        // SAFETY: inside the guest control buffer.
        unsafe { std::ptr::copy_nonoverlapping(rec.as_ptr(), (out as *mut u8).add(w), rec.len()) };
        w += rec.len();
    }
    (w, flags)
}

// ---- send / receive --------------------------------------------------------

fn host_send_flags(f: u64) -> i32 {
    let mut h = 0;
    for (l, d) in [
        (L_MSG_OOB, libc::MSG_OOB),
        (L_MSG_DONTROUTE, libc::MSG_DONTROUTE),
        (L_MSG_DONTWAIT, libc::MSG_DONTWAIT),
        (L_MSG_EOR, libc::MSG_EOR),
    ] {
        if f & l != 0 {
            h |= d;
        }
    }
    h
}

fn host_recv_flags(f: u64) -> i32 {
    let mut h = 0;
    for (l, d) in [
        (L_MSG_OOB, libc::MSG_OOB),
        (L_MSG_PEEK, libc::MSG_PEEK),
        (L_MSG_DONTWAIT, libc::MSG_DONTWAIT),
        (L_MSG_WAITALL, libc::MSG_WAITALL),
    ] {
        if f & l != 0 {
            h |= d;
        }
    }
    h
}

fn linux_msg_flags(h: i32) -> i32 {
    let mut f = 0;
    if h & libc::MSG_TRUNC != 0 {
        f |= L_MSG_TRUNC as i32;
    }
    if h & libc::MSG_CTRUNC != 0 {
        f |= L_MSG_CTRUNC;
    }
    if h & libc::MSG_EOR != 0 {
        f |= L_MSG_EOR as i32;
    }
    if h & libc::MSG_OOB != 0 {
        f |= L_MSG_OOB as i32;
    }
    f
}

/// A send failed with EPIPE: Linux raises SIGPIPE unless MSG_NOSIGNAL.
fn epipe(r: i64, flags: u64) -> i64 {
    if r == -32 && flags & L_MSG_NOSIGNAL == 0 {
        // SAFETY: directing SIGPIPE at the calling thread, as Linux does.
        unsafe { libc::pthread_kill(libc::pthread_self(), libc::SIGPIPE) };
    }
    r
}

fn sendmsg_host(
    fd: i32,
    iov: &mut [libc::iovec],
    name: Option<(*const libc::sockaddr, u32)>,
    ctrl: &mut [u8],
    flags: i32,
) -> i64 {
    let mut m: libc::msghdr = unsafe { std::mem::zeroed() };
    if let Some((sa, len)) = name {
        m.msg_name = sa as *mut _;
        m.msg_namelen = len;
    }
    m.msg_iov = iov.as_mut_ptr();
    m.msg_iovlen = iov.len() as i32;
    if !ctrl.is_empty() {
        m.msg_control = ctrl.as_mut_ptr().cast();
        m.msg_controllen = ctrl.len() as u32;
    }
    // SAFETY: a msghdr over live buffers.
    errno::check(unsafe { libc::sendmsg(fd, &m, flags) } as i64)
}

/// Guest iovecs as host iovecs (same layout: base, len).
fn guest_iov(ptr: u64, n: u64) -> Result<Vec<libc::iovec>, i64> {
    if n > 1024 {
        return Err(-(EINVAL as i64));
    }
    if n == 0 {
        return Ok(Vec::new());
    }
    // SAFETY: guest iovec array of n entries.
    Ok(unsafe { std::slice::from_raw_parts(ptr as *const libc::iovec, n as usize) }.to_vec())
}

fn iov_len(iov: &[libc::iovec]) -> usize {
    iov.iter().map(|v| v.iov_len).sum()
}

/// Drop the first `n` bytes of an iovec list.
fn advance(iov: &mut Vec<libc::iovec>, mut n: usize) {
    while n > 0 && !iov.is_empty() {
        if iov[0].iov_len <= n {
            n -= iov[0].iov_len;
            iov.remove(0);
        } else {
            // SAFETY: moving inside the same guest buffer.
            iov[0].iov_base = unsafe { (iov[0].iov_base as *mut u8).add(n) }.cast();
            iov[0].iov_len -= n;
            n = 0;
        }
    }
}

/// The part of an iovec list that holds its first `n` bytes.
fn prefix(iov: &[libc::iovec], mut n: usize) -> Vec<libc::iovec> {
    let mut out = Vec::new();
    for v in iov {
        if n == 0 {
            break;
        }
        let take = v.iov_len.min(n);
        out.push(libc::iovec {
            iov_base: v.iov_base,
            iov_len: take,
        });
        n -= take;
    }
    out
}

/// One Linux sendmsg on `fd`.
fn send(
    fd: i32,
    iov: &[libc::iovec],
    name: Option<(u64, u32)>,
    ctrl: Option<(u64, usize)>,
    flags: u64,
) -> i64 {
    if let Some(r) = send_other(fd, iov, name) {
        return r;
    }
    let s = sock(fd);
    let (target, abstract_target) = match name {
        Some((p, l)) if p != 0 => match target_of(p, l, false) {
            Ok((_, n)) if family_mismatch(fd, &n) => return -EAFNOSUPPORT,
            Ok((t, n)) => (Some(t), is_abstract(&n)),
            Err(e) => return e,
        },
        _ => (None, false),
    };
    let mut hctrl = match ctrl {
        Some((p, l)) if p != 0 && l > 0 => match control_to_host(p, l) {
            Ok(c) => c,
            Err(e) => return e,
        },
        _ => Vec::new(),
    };
    let hflags = host_send_flags(flags);
    let total = iov_len(iov);
    let ty = s.as_ref().map(|s| s.ty);
    let mut full: Vec<libc::iovec> = Vec::with_capacity(iov.len() + 1);
    let mut hdr = [0u8; FRAME];
    match ty {
        Some(SockType::SeqPacket) => hdr = header(total as u32, own_cred()),
        Some(SockType::Dgram) => hdr = header(DGRAM_MAGIC, own_cred()),
        _ => {}
    }
    if matches!(ty, Some(SockType::SeqPacket | SockType::Dgram)) {
        full.push(libc::iovec {
            iov_base: hdr.as_mut_ptr().cast(),
            iov_len: FRAME,
        });
    }
    full.extend_from_slice(iov);
    let r = match &target {
        Some(t) => with_target(t, |sa, len| {
            sendmsg_host(fd, &mut full, Some((sa, len)), &mut hctrl, hflags)
        }),
        None => sendmsg_host(fd, &mut full, None, &mut hctrl, hflags),
    };
    if r == -(libc::ENOENT as i64) && abstract_target {
        return -111;
    }
    if r < 0 {
        return epipe(r, flags);
    }
    match ty {
        Some(SockType::SeqPacket) => {
            // A frame must go out whole: finish a partial one, waiting.
            let want = FRAME + total;
            let mut sent = r as usize;
            advance(&mut full, sent);
            while sent < want {
                let n = sendmsg_host(fd, &mut full, None, &mut [], 0);
                if n == -(EAGAIN as i64) {
                    let w = fdtab::wait_for(fd, libc::POLLOUT);
                    if w < 0 && w != -4 {
                        return w;
                    }
                    continue;
                }
                if n < 0 {
                    return epipe(n, flags);
                }
                advance(&mut full, n as usize);
                sent += n as usize;
            }
            total as i64
        }
        Some(SockType::Dgram) => (r as usize).saturating_sub(FRAME) as i64,
        _ => r,
    }
}

/// A send on a netlink or packet socket, which the layer handles. None:
/// another family.
fn send_other(fd: i32, iov: &[libc::iovec], name: Option<(u64, u32)>) -> Option<i64> {
    let s = any_sock(fd)?;
    let name = name.filter(|n| n.0 != 0);
    let gather = || -> Vec<u8> {
        iov.iter()
            .flat_map(|v| {
                // SAFETY: the guest's iovec buffers.
                unsafe { std::slice::from_raw_parts(v.iov_base as *const u8, v.iov_len) }
            })
            .copied()
            .collect()
    };
    Some(match &s.family {
        Family::Netlink(n) => {
            let dst = match name.map(|(p, l)| super::netlink::parse_sockaddr(p, l)) {
                Some(Ok(d)) => Some(d),
                Some(Err(e)) => return Some(e),
                None => None,
            };
            super::netlink::send(n, &gather(), dst)
        }
        Family::Packet(p) => {
            let ifindex = match name.map(|(a, l)| super::packet::parse_sockaddr(a, l)) {
                Some(Ok((_, i))) => i,
                Some(Err(e)) => return Some(e),
                None => 0,
            };
            super::packet::send(p, &gather(), ifindex)
        }
        Family::Inet(_) => return dhcp_to_router(fd, &s, iov, name),
        Family::Unix => return None,
    })
}

/// A DHCP client's datagram to the server port from a socket bound to a
/// device other than the loopback: DhcpClient's renewals (to the lease's
/// server) and rebinding broadcasts. They go to `eth0`'s virtual router
/// (`packet::udp_to_router`), as they would on a real link, instead of to
/// the Mac's network. None: another datagram, sent as usual.
fn dhcp_to_router(fd: i32, s: &Sock, iov: &[libc::iovec], name: Option<(u64, u32)>) -> Option<i64> {
    let Family::Inet(o) = &s.family else {
        return None;
    };
    if o.device.lock().unwrap().is_empty() {
        return None;
    }
    let inet = |b: &[u8]| {
        (b.len() >= 8 && u16::from_le_bytes([b[0], b[1]]) == L_AF_INET)
            .then(|| (u16::from_be_bytes([b[2], b[3]]), [b[4], b[5], b[6], b[7]]))
    };
    let dst = match name {
        // SAFETY: a guest sockaddr of `l` bytes.
        Some((p, l)) => inet(unsafe { std::slice::from_raw_parts(p as *const u8, l as usize) }),
        None if o.router.load(Ordering::Relaxed) => inet(s.peer.lock().unwrap().as_deref()?),
        None => host_inet_name(fd, true),
    };
    let (port, dst) = dst?;
    if port != super::dhcp::SERVER_PORT || !on_link(fd, o) {
        return None;
    }
    let src = match s.local.lock().unwrap().as_deref() {
        Some(local) => [local[4], local[5], local[6], local[7]],
        None => host_inet_name(fd, false).map_or([0; 4], |(_, a)| a),
    };
    let msg: Vec<u8> = iov
        .iter()
        .flat_map(|v| {
            // SAFETY: the guest's iovec buffers.
            unsafe { std::slice::from_raw_parts(v.iov_base as *const u8, v.iov_len) }
        })
        .copied()
        .collect();
    Some(super::packet::udp_to_router(src.into(), dst.into(), &msg))
}

/// The port and address of a host AF_INET socket's peer or local name.
fn host_inet_name(fd: i32, peer: bool) -> Option<(u16, [u8; 4])> {
    // SAFETY: an all-zero sockaddr_in is valid.
    let mut sa: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::sockaddr_in>() as u32;
    let p = (&mut sa as *mut libc::sockaddr_in).cast();
    // SAFETY: the name into local storage of its size.
    let r = unsafe {
        if peer {
            libc::getpeername(fd, p, &mut len)
        } else {
            libc::getsockname(fd, p, &mut len)
        }
    };
    (r == 0 && sa.sin_family as i32 == libc::AF_INET)
        .then(|| (u16::from_be(sa.sin_port), sa.sin_addr.s_addr.to_ne_bytes()))
}

/// A receive on a netlink or packet socket. None: another family.
fn recv_other(fd: i32, iov: &[libc::iovec], flags: u64) -> Option<Result<Received, i64>> {
    let s = any_sock(fd)?;
    let hflags = host_recv_flags(flags);
    Some(match &s.family {
        Family::Netlink(_) => {
            let mut meta = [0u8; super::netlink::META];
            let mut v = vec![libc::iovec {
                iov_base: meta.as_mut_ptr().cast(),
                iov_len: meta.len(),
            }];
            v.extend_from_slice(iov);
            recvmsg_host(fd, &mut v, &mut [], false, hflags).map(|(n, f, ..)| {
                let (name, [pid, uid, gid]) = super::netlink::parse_meta(&meta);
                Received {
                    n: n.saturating_sub(meta.len()) as i64,
                    flags: f,
                    ctrl: Vec::new(),
                    cred: s.passcred.load(Ordering::Relaxed).then_some(Cred {
                        pid: pid as i32,
                        uid,
                        gid,
                    }),
                    name: Some(name),
                }
            })
        }
        Family::Packet(p) => {
            let mut buf = vec![0u8; iov_len(iov)];
            super::packet::recv(p, fd, &mut buf, hflags, flags & L_MSG_TRUNC != 0).map(
                |(n, truncated, name)| {
                    let filled = n.min(buf.len());
                    let mut at = 0;
                    for v in iov {
                        let k = v.iov_len.min(filled - at);
                        // SAFETY: the guest's iovec buffers.
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                buf[at..].as_ptr(),
                                v.iov_base as *mut u8,
                                k,
                            )
                        };
                        at += k;
                    }
                    Received {
                        n: n as i64,
                        flags: if truncated { libc::MSG_TRUNC } else { 0 },
                        ctrl: Vec::new(),
                        cred: None,
                        name: Some(name),
                    }
                },
            )
        }
        Family::Inet(o) if o.icmp4.load(Ordering::Relaxed) => recv_ping(fd, iov, hflags, flags),
        Family::Unix | Family::Inet(_) => return None,
    })
}

/// A receive on an IPv4 ping socket (SOCK_DGRAM, IPPROTO_ICMP): Darwin
/// delivers the IP header too, Linux only the ICMP message.
fn recv_ping(fd: i32, iov: &[libc::iovec], hflags: i32, flags: u64) -> Result<Received, i64> {
    let mut pkt = vec![0u8; 65536];
    let mut v = [libc::iovec {
        iov_base: pkt.as_mut_ptr().cast(),
        iov_len: pkt.len(),
    }];
    let (n, _, _, name) = recvmsg_host(fd, &mut v, &mut [], true, hflags)?;
    let ihl = pkt.first().map_or(0, |b| ((b & 0xf) as usize) * 4).min(n);
    let msg = &pkt[ihl..n];
    let mut at = 0;
    for v in iov {
        let k = v.iov_len.min(msg.len() - at);
        // SAFETY: the guest's iovec buffers.
        unsafe { std::ptr::copy_nonoverlapping(msg[at..].as_ptr(), v.iov_base as *mut u8, k) };
        at += k;
    }
    Ok(Received {
        n: if flags & L_MSG_TRUNC != 0 {
            msg.len()
        } else {
            at
        } as i64,
        flags: if at < msg.len() { libc::MSG_TRUNC } else { 0 },
        ctrl: Vec::new(),
        cred: None,
        name,
    })
}

/// What one host recvmsg returned: bytes, msg_flags, control length and
/// the sender's Linux address.
type HostRecv = (usize, i32, usize, Option<Vec<u8>>);

struct Received {
    /// Bytes delivered to the guest buffers (or the message length with
    /// MSG_TRUNC requested).
    n: i64,
    flags: i32,
    ctrl: Vec<u8>,
    cred: Option<Cred>,
    name: Option<Vec<u8>>,
}

fn recvmsg_host(
    fd: i32,
    iov: &mut [libc::iovec],
    ctrl: &mut [u8],
    name: bool,
    flags: i32,
) -> Result<HostRecv, i64> {
    let mut sa: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut m: libc::msghdr = unsafe { std::mem::zeroed() };
    if name {
        m.msg_name = (&mut sa as *mut libc::sockaddr_storage).cast();
        m.msg_namelen = std::mem::size_of::<libc::sockaddr_storage>() as u32;
    }
    m.msg_iov = iov.as_mut_ptr();
    m.msg_iovlen = iov.len() as i32;
    if !ctrl.is_empty() {
        m.msg_control = ctrl.as_mut_ptr().cast();
        m.msg_controllen = ctrl.len() as u32;
    }
    // SAFETY: a msghdr over live buffers.
    let r = unsafe { libc::recvmsg(fd, &mut m, flags) };
    if r < 0 {
        return Err(-(errno::last() as i64));
    }
    let addr = (name && m.msg_namelen > 0)
        .then(|| linux_addr_of_host(&sa, m.msg_namelen))
        .flatten();
    Ok((r as usize, m.msg_flags, m.msg_controllen as usize, addr))
}

/// Receive exactly the bytes of `iov`, waiting as needed (the rest of a
/// frame whose start already arrived). Returns what arrived before EOF.
fn recv_exact(fd: i32, mut iov: Vec<libc::iovec>, flags: i32) -> Result<usize, i64> {
    let mut got = 0;
    iov.retain(|v| v.iov_len > 0);
    while !iov.is_empty() {
        match recvmsg_host(fd, &mut iov, &mut [], false, flags | libc::MSG_WAITALL) {
            Ok((0, ..)) => break,
            Ok((n, ..)) => {
                got += n;
                advance(&mut iov, n);
            }
            Err(e) if e == -(EAGAIN as i64) || e == -4 => {
                let w = fdtab::wait_for(fd, libc::POLLIN);
                if w < 0 && w != -4 {
                    return Err(w);
                }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(got)
}

fn recv(
    fd: i32,
    iov: &[libc::iovec],
    want_name: bool,
    ctrl_cap: usize,
    flags: u64,
) -> Result<Received, i64> {
    if let Some(r) = recv_other(fd, iov, flags) {
        return r;
    }
    let s = sock(fd);
    let mut hctrl = vec![0u8; if ctrl_cap > 0 { ctrl_cap + 64 } else { 0 }];
    let hflags = host_recv_flags(flags);
    let cap = iov_len(iov);
    let Some(s) = s else {
        let mut v = iov.to_vec();
        let (n, f, cl, name) = recvmsg_host(fd, &mut v, &mut hctrl, want_name, hflags)?;
        hctrl.truncate(cl);
        return Ok(Received {
            n: n as i64,
            flags: f,
            ctrl: hctrl,
            cred: None,
            name,
        });
    };
    let passcred = s.passcred.load(Ordering::Relaxed);
    match s.ty {
        SockType::Stream => {
            let mut v = iov.to_vec();
            let (n, f, cl, name) = recvmsg_host(fd, &mut v, &mut hctrl, want_name, hflags)?;
            hctrl.truncate(cl);
            Ok(Received {
                n: n as i64,
                flags: f,
                ctrl: hctrl,
                cred: if passcred && n > 0 {
                    peer_cred(fd)
                } else {
                    None
                },
                name,
            })
        }
        SockType::Dgram => {
            let mut hdr = [0u8; FRAME];
            let mut v = vec![libc::iovec {
                iov_base: hdr.as_mut_ptr().cast(),
                iov_len: FRAME,
            }];
            v.extend_from_slice(iov);
            let (n, f, cl, name) = recvmsg_host(fd, &mut v, &mut hctrl, want_name, hflags)?;
            hctrl.truncate(cl);
            let (magic, c) = parse_header(&hdr);
            if n >= FRAME && magic == DGRAM_MAGIC {
                return Ok(Received {
                    n: (n - FRAME) as i64,
                    flags: f,
                    ctrl: hctrl,
                    cred: passcred.then_some(c),
                    name,
                });
            }
            // Not framed (a host sender): move the data into place.
            let mut data = hdr[..n.min(FRAME)].to_vec();
            let rest = n.saturating_sub(FRAME);
            for p in prefix(iov, rest) {
                // SAFETY: bytes the kernel just wrote to the guest buffer.
                data.extend_from_slice(unsafe {
                    std::slice::from_raw_parts(p.iov_base as *const u8, p.iov_len)
                });
            }
            let mut w = 0;
            for p in prefix(iov, data.len().min(cap)) {
                // SAFETY: guest buffer.
                unsafe { std::ptr::copy(data[w..].as_ptr(), p.iov_base as *mut u8, p.iov_len) };
                w += p.iov_len;
            }
            Ok(Received {
                n: w as i64,
                flags: f | if data.len() > cap { libc::MSG_TRUNC } else { 0 },
                ctrl: hctrl,
                cred: if passcred { peer_cred(fd) } else { None },
                name,
            })
        }
        SockType::SeqPacket => {
            let _g = s.recv.lock().unwrap();
            let peek = flags & L_MSG_PEEK != 0;
            let dontwait = flags & L_MSG_DONTWAIT != 0;
            let mut hdr = [0u8; FRAME];
            let first = if dontwait { libc::MSG_DONTWAIT } else { 0 }
                | if peek { libc::MSG_PEEK } else { 0 };
            let mut v = [libc::iovec {
                iov_base: hdr.as_mut_ptr().cast(),
                iov_len: FRAME,
            }];
            // A blocking receive here honors O_NONBLOCK, SO_RCVTIMEO and
            // signals as Linux's does; the rest of the frame follows.
            let (n, _, cl, _) = recvmsg_host(fd, &mut v, &mut hctrl, false, first)?;
            hctrl.truncate(cl);
            let mut got = n;
            if got == 0 {
                return Ok(Received {
                    n: 0,
                    flags: 0,
                    ctrl: hctrl,
                    cred: None,
                    name: None,
                });
            }
            let peekf = if peek { libc::MSG_PEEK } else { 0 };
            if got < FRAME {
                if peek {
                    got = recv_exact(fd, vec![v[0]], libc::MSG_PEEK)?;
                } else {
                    let rest = libc::iovec {
                        iov_base: hdr[got..].as_mut_ptr().cast(),
                        iov_len: FRAME - got,
                    };
                    got += recv_exact(fd, vec![rest], 0)?;
                }
                if got < FRAME {
                    return Ok(Received {
                        n: 0,
                        flags: 0,
                        ctrl: hctrl,
                        cred: None,
                        name: None,
                    });
                }
            }
            let (len, c) = parse_header(&hdr);
            let len = len as usize;
            let copy = len.min(cap);
            if peek {
                let mut buf = vec![0u8; FRAME + copy];
                let n = recv_exact(
                    fd,
                    vec![libc::iovec {
                        iov_base: buf.as_mut_ptr().cast(),
                        iov_len: buf.len(),
                    }],
                    peekf,
                )?;
                let mut w = 0;
                for p in prefix(iov, n.saturating_sub(FRAME)) {
                    // SAFETY: guest buffer.
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            buf[FRAME + w..].as_ptr(),
                            p.iov_base as *mut u8,
                            p.iov_len,
                        )
                    };
                    w += p.iov_len;
                }
            } else {
                recv_exact(fd, prefix(iov, copy), 0)?;
                let mut left = len - copy;
                let mut scratch = [0u8; 4096];
                while left > 0 {
                    let k = left.min(scratch.len());
                    let n = recv_exact(
                        fd,
                        vec![libc::iovec {
                            iov_base: scratch.as_mut_ptr().cast(),
                            iov_len: k,
                        }],
                        0,
                    )?;
                    if n == 0 {
                        break;
                    }
                    left -= n;
                }
            }
            Ok(Received {
                n: if flags & L_MSG_TRUNC != 0 {
                    len as i64
                } else {
                    copy as i64
                },
                flags: if len > cap { libc::MSG_TRUNC } else { 0 },
                ctrl: hctrl,
                cred: passcred.then_some(c),
                name: s.peer.lock().unwrap().clone(),
            })
        }
    }
}

// ---- syscalls ---------------------------------------------------------------

pub fn sendto(a: [u64; 6]) -> i64 {
    let iov = [libc::iovec {
        iov_base: a[1] as *mut _,
        iov_len: a[2] as usize,
    }];
    send(a[0] as i32, &iov, Some((a[4], a[5] as u32)), None, a[3])
}

pub fn recvfrom(a: [u64; 6]) -> i64 {
    let (fd, buf, len, flags, addr, addrlen) = (a[0] as i32, a[1], a[2] as usize, a[3], a[4], a[5]);
    let iov = [libc::iovec {
        iov_base: buf as *mut _,
        iov_len: len,
    }];
    match recv(fd, &iov, addr != 0, 0, flags) {
        Ok(r) => {
            if let Some(n) = &r.name {
                put_addr(n, addr, addrlen);
            } else if addr != 0 && addrlen != 0 {
                // SAFETY: guest socklen_t.
                unsafe { (addrlen as *mut u32).write_unaligned(0) };
            }
            r.n
        }
        Err(e) => e,
    }
}

/// Linux arm64 `struct msghdr`.
#[repr(C)]
#[derive(Clone, Copy)]
struct LinuxMsghdr {
    name: u64,
    namelen: u32,
    _pad: u32,
    iov: u64,
    iovlen: u64,
    control: u64,
    controllen: u64,
    flags: i32,
    _pad2: u32,
}
const _: () = assert!(std::mem::size_of::<LinuxMsghdr>() == 56);

fn sendmsg_one(fd: i32, m: &LinuxMsghdr, flags: u64) -> i64 {
    let iov = match guest_iov(m.iov, m.iovlen) {
        Ok(v) => v,
        Err(e) => return e,
    };
    send(
        fd,
        &iov,
        (m.name != 0).then_some((m.name, m.namelen)),
        Some((m.control, m.controllen as usize)),
        flags,
    )
}

fn recvmsg_one(fd: i32, mp: u64, flags: u64) -> i64 {
    // SAFETY: guest struct msghdr.
    let mut m = unsafe { (mp as *const LinuxMsghdr).read_unaligned() };
    let iov = match guest_iov(m.iov, m.iovlen) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let r = match recv(fd, &iov, m.name != 0, m.controllen as usize, flags) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let (cl, ctrunc) = control_to_guest(
        &r.ctrl,
        r.cred,
        flags & L_MSG_CMSG_CLOEXEC != 0,
        m.control,
        m.controllen as usize,
    );
    m.controllen = cl as u64;
    m.flags = linux_msg_flags(r.flags) | ctrunc;
    if m.name != 0 {
        match &r.name {
            Some(n) => {
                let k = n.len().min(m.namelen as usize);
                // SAFETY: guest name buffer of namelen bytes.
                unsafe { std::ptr::copy_nonoverlapping(n.as_ptr(), m.name as *mut u8, k) };
                m.namelen = n.len() as u32;
            }
            None => m.namelen = 0,
        }
    }
    // SAFETY: guest struct msghdr.
    unsafe { (mp as *mut LinuxMsghdr).write_unaligned(m) };
    r.n
}

pub fn sendmsg(a: [u64; 6]) -> i64 {
    // SAFETY: guest struct msghdr.
    let m = unsafe { (a[1] as *const LinuxMsghdr).read_unaligned() };
    sendmsg_one(a[0] as i32, &m, a[2])
}

pub fn recvmsg(a: [u64; 6]) -> i64 {
    recvmsg_one(a[0] as i32, a[1], a[2])
}

/// Linux `struct mmsghdr`: a msghdr and the byte count, 64 bytes.
const MMSG: u64 = 64;

pub fn sendmmsg(a: [u64; 6]) -> i64 {
    let (fd, vec, n, flags) = (a[0] as i32, a[1], a[2].min(1024), a[3]);
    for i in 0..n {
        let p = vec + i * MMSG;
        // SAFETY: guest mmsghdr array.
        let m = unsafe { (p as *const LinuxMsghdr).read_unaligned() };
        let r = sendmsg_one(fd, &m, flags);
        if r < 0 {
            return if i > 0 { i as i64 } else { r };
        }
        // SAFETY: msg_len follows the msghdr.
        unsafe { ((p + 56) as *mut u32).write_unaligned(r as u32) };
    }
    n as i64
}

pub fn recvmmsg(a: [u64; 6]) -> i64 {
    const L_MSG_WAITFORONE: u64 = 0x10000;
    let (fd, vec, n, mut flags) = (a[0] as i32, a[1], a[2].min(1024), a[3]);
    for i in 0..n {
        let p = vec + i * MMSG;
        let r = recvmsg_one(fd, p, flags & !L_MSG_WAITFORONE);
        if r < 0 {
            return if i > 0 { i as i64 } else { r };
        }
        // SAFETY: msg_len follows the msghdr.
        unsafe { ((p + 56) as *mut u32).write_unaligned(r as u32) };
        if flags & L_MSG_WAITFORONE != 0 {
            flags |= L_MSG_DONTWAIT;
        }
    }
    n as i64
}

/// read/readv on a socket with Linux state (a plain host read for
/// AF_INET options).
pub fn read(fd: i32, iov: &[libc::iovec]) -> Option<i64> {
    if let Family::Inet(o) = &any_sock(fd)?.family
        && !o.icmp4.load(Ordering::Relaxed)
    {
        return None;
    }
    Some(match recv(fd, iov, false, 0, 0) {
        Ok(r) => r.n,
        Err(e) => e,
    })
}

/// write/writev on a socket with Linux state (a plain host write for
/// AF_INET options, but for DHCP to the virtual router).
pub fn write(fd: i32, iov: &[libc::iovec]) -> Option<i64> {
    let s = any_sock(fd)?;
    if let Family::Inet(_) = &s.family {
        return dhcp_to_router(fd, &s, iov, None);
    }
    Some(send(fd, iov, None, None, 0))
}

// ---- options -----------------------------------------------------------------

const L_SO_DEBUG: u64 = 1;
const L_SO_REUSEADDR: u64 = 2;
const L_SO_TYPE: u64 = 3;
const L_SO_ERROR: u64 = 4;
const L_SO_DONTROUTE: u64 = 5;
const L_SO_BROADCAST: u64 = 6;
const L_SO_SNDBUF: u64 = 7;
const L_SO_RCVBUF: u64 = 8;
const L_SO_KEEPALIVE: u64 = 9;
const L_SO_OOBINLINE: u64 = 10;
const L_SO_PRIORITY: u64 = 12;
const L_SO_BINDTODEVICE: u64 = 25;
const L_SO_ATTACH_FILTER: u64 = 26;
const L_SO_DETACH_FILTER: u64 = 27;
const L_SO_MARK: u64 = 36;
const L_SO_LINGER: u64 = 13;
const L_SO_REUSEPORT: u64 = 15;
const L_SO_PASSCRED: u64 = 16;
const L_SO_PEERCRED: u64 = 17;
const L_SO_RCVLOWAT: u64 = 18;
const L_SO_SNDLOWAT: u64 = 19;
const L_SO_RCVTIMEO: u64 = 20;
const L_SO_SNDTIMEO: u64 = 21;
const L_SO_ACCEPTCONN: u64 = 30;
const L_SO_SNDBUFFORCE: u64 = 32;
const L_SO_RCVBUFFORCE: u64 = 33;
const L_SO_PROTOCOL: u64 = 38;
const L_SO_DOMAIN: u64 = 39;
const L_SO_RCVTIMEO_NEW: u64 = 66;
const L_SO_SNDTIMEO_NEW: u64 = 67;

const IPPROTO_IP: u64 = 0;
const L_IP_MULTICAST_ALL: u64 = 49;
const IPPROTO_ICMP: u64 = 1;
const IPPROTO_TCP: u64 = 6;
const L_TCP_USER_TIMEOUT: u64 = 18;
/// Darwin's TCP_RXT_CONNDROPTIME: seconds of unacknowledged retransmission
/// before the connection drops.
const TCP_RXT_CONNDROPTIME: i32 = 0x80;
const IPPROTO_IPV6: u64 = 41;

/// Darwin (level, option) of a plain int option.
fn int_option(level: u64, opt: u64) -> Option<(i32, i32)> {
    Some(match (level, opt) {
        (1, L_SO_DEBUG) => (libc::SOL_SOCKET, libc::SO_DEBUG),
        (1, L_SO_REUSEADDR) => (libc::SOL_SOCKET, libc::SO_REUSEADDR),
        (1, L_SO_DONTROUTE) => (libc::SOL_SOCKET, libc::SO_DONTROUTE),
        (1, L_SO_BROADCAST) => (libc::SOL_SOCKET, libc::SO_BROADCAST),
        (1, L_SO_SNDBUF | L_SO_SNDBUFFORCE) => (libc::SOL_SOCKET, libc::SO_SNDBUF),
        (1, L_SO_RCVBUF | L_SO_RCVBUFFORCE) => (libc::SOL_SOCKET, libc::SO_RCVBUF),
        (1, L_SO_KEEPALIVE) => (libc::SOL_SOCKET, libc::SO_KEEPALIVE),
        (1, L_SO_OOBINLINE) => (libc::SOL_SOCKET, libc::SO_OOBINLINE),
        (1, L_SO_REUSEPORT) => (libc::SOL_SOCKET, libc::SO_REUSEPORT),
        (1, L_SO_RCVLOWAT) => (libc::SOL_SOCKET, libc::SO_RCVLOWAT),
        (1, L_SO_SNDLOWAT) => (libc::SOL_SOCKET, libc::SO_SNDLOWAT),
        (1, L_SO_ACCEPTCONN) => (libc::SOL_SOCKET, libc::SO_ACCEPTCONN),
        (IPPROTO_TCP, 1) => (libc::IPPROTO_TCP, libc::TCP_NODELAY),
        (IPPROTO_TCP, 2) => (libc::IPPROTO_TCP, libc::TCP_MAXSEG),
        (IPPROTO_TCP, 4) => (libc::IPPROTO_TCP, libc::TCP_KEEPALIVE),
        (IPPROTO_TCP, 5) => (libc::IPPROTO_TCP, libc::TCP_KEEPINTVL),
        (IPPROTO_TCP, 6) => (libc::IPPROTO_TCP, libc::TCP_KEEPCNT),
        (IPPROTO_IP, 1) => (libc::IPPROTO_IP, libc::IP_TOS),
        (IPPROTO_IP, 2) => (libc::IPPROTO_IP, libc::IP_TTL),
        (IPPROTO_IPV6, 16) => (libc::IPPROTO_IPV6, libc::IPV6_UNICAST_HOPS),
        (IPPROTO_IPV6, 26) => (libc::IPPROTO_IPV6, libc::IPV6_V6ONLY),
        _ => return None,
    })
}

fn put_opt(v: &[u8], out: u64, outlen: u64) -> i64 {
    // SAFETY: guest option buffer and socklen_t.
    unsafe {
        let cap = (outlen as *const u32).read_unaligned() as usize;
        let n = cap.min(v.len());
        std::ptr::copy_nonoverlapping(v.as_ptr(), out as *mut u8, n);
        (outlen as *mut u32).write_unaligned(n as u32);
    }
    0
}

pub fn setsockopt(a: [u64; 6]) -> i64 {
    let (fd, level, opt, val, len) = (a[0] as i32, a[1], a[2], a[3], a[4] as u32);
    let int = || {
        if len < 4 {
            None
        } else {
            // SAFETY: guest int option value.
            Some(unsafe { (val as *const i32).read_unaligned() })
        }
    };
    if let Some((l, o)) = int_option(level, opt) {
        let Some(mut v) = int() else {
            return -(EINVAL as i64);
        };
        if l == libc::SOL_SOCKET && (o == libc::SO_RCVBUF || o == libc::SO_SNDBUF) {
            // Linux raises a small (or zero) size to its minimum (2304
            // bytes, SOCK_MIN_RCVBUF); Darwin refuses zero.
            v = v.max(2304);
        }
        if l == libc::SOL_SOCKET
            && o == libc::SO_REUSEADDR
            && get_int(fd, libc::SOL_SOCKET, libc::SO_TYPE) == Some(libc::SOCK_DGRAM)
        {
            // Datagram sockets that all set SO_REUSEADDR share a port on
            // Linux (socket(7)); on Darwin that takes SO_REUSEPORT too.
            // SAFETY: an int option.
            let r = unsafe {
                libc::setsockopt(fd, l, libc::SO_REUSEPORT, (&v as *const i32).cast(), 4)
            };
            if r < 0 {
                return -(errno::last() as i64);
            }
        }
        // SAFETY: an int option.
        return errno::check(
            unsafe { libc::setsockopt(fd, l, o, (&v as *const i32).cast(), 4) } as i64,
        );
    }
    if let Some(r) = setsockopt_other(fd, level, opt, val, len) {
        return r;
    }
    match (level, opt) {
        (1, L_SO_PASSCRED) => {
            let Some(v) = int() else {
                return -(EINVAL as i64);
            };
            match any_sock(fd) {
                Some(s) => s.passcred.store(v != 0, Ordering::Relaxed),
                None => {
                    if let Err(e) = is_socket(fd) {
                        return e;
                    }
                }
            }
            0
        }
        (1, L_SO_RCVTIMEO | L_SO_SNDTIMEO | L_SO_RCVTIMEO_NEW | L_SO_SNDTIMEO_NEW) => {
            if len < 16 {
                return -(EINVAL as i64);
            }
            // SAFETY: guest struct timeval { i64, i64 }.
            let t = unsafe { (val as *const [i64; 2]).read_unaligned() };
            let tv = libc::timeval {
                tv_sec: t[0],
                tv_usec: t[1] as i32,
            };
            let o = if matches!(opt, L_SO_RCVTIMEO | L_SO_RCVTIMEO_NEW) {
                libc::SO_RCVTIMEO
            } else {
                libc::SO_SNDTIMEO
            };
            // SAFETY: a timeval option.
            errno::check(unsafe {
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    o,
                    (&tv as *const libc::timeval).cast(),
                    std::mem::size_of::<libc::timeval>() as u32,
                )
            } as i64)
        }
        (1, L_SO_LINGER) => {
            if len < 8 {
                return -(EINVAL as i64);
            }
            if sock(fd).is_some_and(|s| s.ty != SockType::Stream) {
                // The linger time is the socket's marker; AF_UNIX sockets
                // have nothing to linger for.
                return 0;
            }
            // SAFETY: struct linger is { int, int } on both.
            errno::check(unsafe {
                libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_LINGER, val as *const _, 8)
            } as i64)
        }
        // Queueing priority only orders the device queue on Linux.
        (1, L_SO_PRIORITY) => 0,
        _ => -ENOPROTOOPT,
    }
}

/// The AF_INET options, netlink options and packet filters. None: not
/// one of them.
fn setsockopt_other(fd: i32, level: u64, opt: u64, val: u64, len: u32) -> Option<i64> {
    let s = any_sock(fd);
    let int = || {
        // SAFETY: a guest int option value.
        (len >= 4).then(|| unsafe { (val as *const u32).read_unaligned() })
    };
    Some(match (level, opt, s.as_deref().map(|s| &s.family)) {
        (super::netlink::SOL_NETLINK, _, Some(Family::Netlink(n))) => match int() {
            Some(v) => super::netlink::setsockopt(n, opt, v),
            None => -(EINVAL as i64),
        },
        (1, L_SO_ATTACH_FILTER, Some(Family::Packet(p))) => {
            super::packet::attach_filter(p, val, len)
        }
        (1, L_SO_DETACH_FILTER, Some(Family::Packet(p))) => super::packet::detach_filter(p),
        (1, L_SO_MARK | L_SO_BINDTODEVICE, None | Some(Family::Inet(_)))
        | (IPPROTO_IP, L_IP_MULTICAST_ALL, None | Some(Family::Inet(_)))
        | (IPPROTO_TCP, L_TCP_USER_TIMEOUT, None | Some(Family::Inet(_))) => {
            if let Err(e) = is_socket(fd) {
                return Some(e);
            }
            let s = match s {
                Some(s) => s,
                None => {
                    let s = Sock::of(SockType::Stream, Family::Inet(InetOpts::default()));
                    fdtab::insert(fd, Kind::Sock(s.clone()));
                    s
                }
            };
            let Family::Inet(o) = &s.family else {
                unreachable!()
            };
            inet_option(fd, o, opt, val, len)
        }
        _ => return None,
    })
}

/// SO_MARK (CAP_NET_ADMIN), SO_BINDTODEVICE, IP_MULTICAST_ALL and
/// TCP_USER_TIMEOUT on an AF_INET/AF_INET6 socket.
///
/// IP_MULTICAST_ALL off limits a socket's multicast to the groups it
/// joined itself (ip(7)). Darwin delivers multicast that way whatever the
/// flag (`udp_input` skips sockets without their own memberships), so the
/// flag is recorded for getsockopt; a socket that keeps it on does not get
/// the groups other sockets joined (#524).
fn inet_option(fd: i32, o: &InetOpts, opt: u64, val: u64, len: u32) -> i64 {
    if opt == L_TCP_USER_TIMEOUT {
        // tcp(7): the milliseconds transmitted data may stay unacknowledged
        // before the connection closes, 0 the system default. Darwin counts
        // whole seconds, so the host gets the timeout rounded up.
        if len < 4 {
            return -(EINVAL as i64);
        }
        // SAFETY: a guest int option value.
        let ms = unsafe { (val as *const i32).read_unaligned() };
        if ms < 0 {
            return -(EINVAL as i64);
        }
        let secs = (ms as u32).div_ceil(1000) as i32;
        // SAFETY: an int option on the host socket.
        let r = unsafe {
            libc::setsockopt(
                fd,
                libc::IPPROTO_TCP,
                TCP_RXT_CONNDROPTIME,
                (&secs as *const i32).cast(),
                4,
            )
        };
        if r < 0 {
            return -(errno::last() as i64);
        }
        o.user_timeout.store(ms as u32, Ordering::Relaxed);
        return 0;
    }
    if opt == L_IP_MULTICAST_ALL {
        // ip_setsockopt reads an int, or one byte from a shorter value.
        // SAFETY: the guest's option value of `len` bytes.
        let v = unsafe {
            match len {
                0 => return -(EINVAL as i64),
                1..4 => (val as *const u8).read() as i32,
                _ => (val as *const i32).read_unaligned(),
            }
        };
        if v != 0 && v != 1 {
            return -(EINVAL as i64);
        }
        o.multicast_own.store(v == 0, Ordering::Relaxed);
        return 0;
    }
    if opt == L_SO_MARK {
        if len < 4 {
            return -(EINVAL as i64);
        }
        if !super::cred::capable(12) {
            return -1; // EPERM
        }
        // SAFETY: a guest int option value.
        o.mark.store(
            unsafe { (val as *const u32).read_unaligned() },
            Ordering::Relaxed,
        );
        return 0;
    }
    // SAFETY: the guest's device name of `len` bytes.
    let name = unsafe { std::slice::from_raw_parts(val as *const u8, (len as usize).min(16)) };
    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
    let name = String::from_utf8_lossy(&name[..end]).into_owned();
    let index = match super::netif::host_index_of(&name) {
        Ok(i) => i,
        Err(e) => return -(e as i64),
    };
    // IP_BOUND_IF or IPV6_BOUND_IF, by the socket's family.
    let mut sa: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut sl = std::mem::size_of::<libc::sockaddr_storage>() as u32;
    // SAFETY: getsockname into local storage.
    unsafe { libc::getsockname(fd, (&mut sa as *mut libc::sockaddr_storage).cast(), &mut sl) };
    let v6 = sa.ss_family as i32 == libc::AF_INET6;
    let (l, o2) = if v6 {
        (libc::IPPROTO_IPV6, libc::IPV6_BOUND_IF)
    } else {
        (libc::IPPROTO_IP, libc::IP_BOUND_IF)
    };
    let i = index as i32;
    // SAFETY: an int option on the host socket.
    let r = unsafe { libc::setsockopt(fd, l, o2, (&i as *const i32).cast(), 4) };
    if r < 0 {
        return -(errno::last() as i64);
    }
    *o.device.lock().unwrap() = name;
    0
}

/// The protocol of a host socket (`soi_protocol` of its
/// PROC_PIDFDSOCKETINFO): IPPROTO_UDP for a UDP socket, 0 for AF_UNIX.
/// libcore exempts UDP connects from StrictMode's network check by it.
fn host_protocol(fd: i32) -> i32 {
    const PROC_PIDFDSOCKETINFO: i32 = 3;
    /// sizeof(struct socket_fdinfo) and offsetof(.psi.soi_protocol).
    const SIZE: usize = 792;
    const PROTOCOL: usize = 180;
    let mut info = [0u8; SIZE];
    // SAFETY: proc_pidfdinfo into a buffer of the struct's size.
    let n = unsafe {
        libc::proc_pidfdinfo(
            libc::getpid(),
            fd,
            PROC_PIDFDSOCKETINFO,
            info.as_mut_ptr().cast(),
            SIZE as i32,
        )
    };
    if n as usize != SIZE {
        return 0;
    }
    i32::from_ne_bytes(info[PROTOCOL..PROTOCOL + 4].try_into().unwrap())
}

pub fn getsockopt(a: [u64; 6]) -> i64 {
    let (fd, level, opt, val, len) = (a[0] as i32, a[1], a[2], a[3], a[4]);
    if let Err(e) = is_socket(fd) {
        return e;
    }
    if let Some(s) = any_sock(fd) {
        let int = |v: i32| put_opt(&v.to_le_bytes(), val, len);
        match (level, opt, &s.family) {
            (1, L_SO_MARK, Family::Inet(o)) => return int(o.mark.load(Ordering::Relaxed) as i32),
            (IPPROTO_IP, L_IP_MULTICAST_ALL, Family::Inet(o)) => {
                return int(!o.multicast_own.load(Ordering::Relaxed) as i32);
            }
            (IPPROTO_TCP, L_TCP_USER_TIMEOUT, Family::Inet(o)) => {
                return int(o.user_timeout.load(Ordering::Relaxed) as i32);
            }
            (1, L_SO_BINDTODEVICE, Family::Inet(o)) => {
                let mut n = o.device.lock().unwrap().as_bytes().to_vec();
                n.push(0);
                return put_opt(&n, val, len);
            }
            (1, L_SO_DOMAIN, Family::Netlink(_)) => {
                return int(super::netlink::L_AF_NETLINK as i32);
            }
            (1, L_SO_DOMAIN, Family::Packet(_)) => return int(super::packet::L_AF_PACKET as i32),
            (1, L_SO_TYPE, Family::Netlink(n)) => return int(n.ty as i32),
            (1, L_SO_TYPE, Family::Packet(_)) => return int(L_SOCK_RAW as i32),
            (1, L_SO_PROTOCOL, Family::Netlink(n)) => return int(n.proto as i32),
            (1, L_SO_PROTOCOL, Family::Packet(p)) => return int(p.protocol() as i32),
            _ => {}
        }
    }
    if level == 1 && opt == L_SO_MARK {
        return put_opt(&0i32.to_le_bytes(), val, len);
    }
    if level == IPPROTO_IP && opt == L_IP_MULTICAST_ALL {
        return put_opt(&1i32.to_le_bytes(), val, len);
    }
    if level == IPPROTO_TCP && opt == L_TCP_USER_TIMEOUT {
        // Never set: the host's (a non-TCP socket fails as on Linux).
        let Some(v) = get_int(fd, libc::IPPROTO_TCP, TCP_RXT_CONNDROPTIME) else {
            return -(errno::last() as i64);
        };
        return put_opt(&(v * 1000).to_le_bytes(), val, len);
    }
    if let Some((l, o)) = int_option(level, opt) {
        let Some(v) = get_int(fd, l, o) else {
            return -(errno::last() as i64);
        };
        return put_opt(&v.to_le_bytes(), val, len);
    }
    let s = sock(fd);
    match (level, opt) {
        (1, L_SO_TYPE) => {
            let t = match s.map(|s| s.ty) {
                Some(SockType::SeqPacket) => L_SOCK_SEQPACKET,
                Some(SockType::Dgram) => L_SOCK_DGRAM,
                Some(SockType::Stream) => L_SOCK_STREAM,
                None => match get_int(fd, libc::SOL_SOCKET, libc::SO_TYPE) {
                    Some(libc::SOCK_DGRAM) => L_SOCK_DGRAM,
                    Some(libc::SOCK_RAW) => L_SOCK_RAW,
                    _ => L_SOCK_STREAM,
                },
            };
            put_opt(&(t as i32).to_le_bytes(), val, len)
        }
        (1, L_SO_ERROR) => {
            let e = get_int(fd, libc::SOL_SOCKET, libc::SO_ERROR).unwrap_or(0);
            let l = if e == 0 { 0 } else { errno::from_darwin(e) };
            put_opt(&l.to_le_bytes(), val, len)
        }
        (1, L_SO_DOMAIN) => {
            let mut sa: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut sl = std::mem::size_of::<libc::sockaddr_storage>() as u32;
            // SAFETY: getsockname into local storage.
            unsafe {
                libc::getsockname(fd, (&mut sa as *mut libc::sockaddr_storage).cast(), &mut sl)
            };
            let d = match sa.ss_family as i32 {
                libc::AF_INET => L_AF_INET,
                libc::AF_INET6 => L_AF_INET6,
                _ => L_AF_UNIX,
            };
            put_opt(&(d as i32).to_le_bytes(), val, len)
        }
        (1, L_SO_PROTOCOL) => put_opt(&host_protocol(fd).to_le_bytes(), val, len),
        (1, L_SO_PASSCRED) => {
            let v = any_sock(fd).is_some_and(|s| s.passcred.load(Ordering::Relaxed)) as i32;
            put_opt(&v.to_le_bytes(), val, len)
        }
        (1, L_SO_PEERCRED) => {
            let c = peer_cred(fd).unwrap_or(Cred {
                pid: 0,
                uid: u32::MAX,
                gid: u32::MAX,
            });
            let mut b = c.pid.to_le_bytes().to_vec();
            b.extend_from_slice(&c.uid.to_le_bytes());
            b.extend_from_slice(&c.gid.to_le_bytes());
            put_opt(&b, val, len)
        }
        (1, L_SO_RCVTIMEO | L_SO_SNDTIMEO | L_SO_RCVTIMEO_NEW | L_SO_SNDTIMEO_NEW) => {
            let mut tv = libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            };
            let mut tl = std::mem::size_of::<libc::timeval>() as u32;
            let o = if matches!(opt, L_SO_RCVTIMEO | L_SO_RCVTIMEO_NEW) {
                libc::SO_RCVTIMEO
            } else {
                libc::SO_SNDTIMEO
            };
            // SAFETY: a timeval option into a local.
            unsafe {
                libc::getsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    o,
                    (&mut tv as *mut libc::timeval).cast(),
                    &mut tl,
                )
            };
            let mut b = tv.tv_sec.to_le_bytes().to_vec();
            b.extend_from_slice(&(tv.tv_usec as i64).to_le_bytes());
            put_opt(&b, val, len)
        }
        (1, L_SO_LINGER) => {
            let mut l = [0i32; 2];
            if s.is_none_or(|s| s.ty == SockType::Stream) {
                let mut ll = 8u32;
                // SAFETY: struct linger into a local.
                unsafe {
                    libc::getsockopt(
                        fd,
                        libc::SOL_SOCKET,
                        libc::SO_LINGER,
                        l.as_mut_ptr().cast(),
                        &mut ll,
                    )
                };
            }
            let mut b = l[0].to_le_bytes().to_vec();
            b.extend_from_slice(&l[1].to_le_bytes());
            put_opt(&b, val, len)
        }
        (1, L_SO_PRIORITY) => put_opt(&0i32.to_le_bytes(), val, len),
        _ => -ENOPROTOOPT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abstract_names_round_trip() {
        for n in [&b"logd"[..], b"jdwp-control", b"a/b c\0d"] {
            assert_eq!(unescape(&escape(n)), n);
        }
    }

    #[test]
    fn multicast_all_is_recorded_per_socket() {
        // SAFETY: a host socket this test owns.
        let fd = unsafe { libc::socket(libc::AF_INET6, libc::SOCK_DGRAM, 0) };
        assert!(fd >= 0);
        let get = || {
            let (mut v, mut l) = (-1i32, 4u32);
            let r = getsockopt([
                fd as u64,
                IPPROTO_IP,
                L_IP_MULTICAST_ALL,
                &mut v as *mut i32 as u64,
                &mut l as *mut u32 as u64,
                0,
            ]);
            (r, v)
        };
        let set = |v: i32, len: u64| {
            setsockopt([
                fd as u64,
                IPPROTO_IP,
                L_IP_MULTICAST_ALL,
                &v as *const i32 as u64,
                len,
                0,
            ])
        };
        assert_eq!(get(), (0, 1));
        assert_eq!(set(0, 4), 0);
        assert_eq!(get(), (0, 0));
        assert_eq!(set(2, 4), -(EINVAL as i64));
        assert_eq!(set(1, 1), 0);
        assert_eq!(get(), (0, 1));
        assert_eq!(set(0, 0), -(EINVAL as i64));
        fdtab::on_close(fd);
        // SAFETY: our socket.
        unsafe { libc::close(fd) };
    }

    #[test]
    fn tcp_user_timeout_round_trips() {
        // SAFETY: a host socket this test owns.
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        assert!(fd >= 0);
        let get = || {
            let (mut v, mut l) = (-1i32, 4u32);
            let r = getsockopt([
                fd as u64,
                IPPROTO_TCP,
                L_TCP_USER_TIMEOUT,
                &mut v as *mut i32 as u64,
                &mut l as *mut u32 as u64,
                0,
            ]);
            (r, v)
        };
        let set = |v: i32| {
            setsockopt([
                fd as u64,
                IPPROTO_TCP,
                L_TCP_USER_TIMEOUT,
                &v as *const i32 as u64,
                4,
                0,
            ])
        };
        assert_eq!(get(), (0, 0));
        assert_eq!(set(1500), 0);
        assert_eq!(get(), (0, 1500));
        assert_eq!(
            get_int(fd, libc::IPPROTO_TCP, TCP_RXT_CONNDROPTIME),
            Some(2)
        );
        assert_eq!(set(-1), -(EINVAL as i64));
        fdtab::on_close(fd);
        // SAFETY: our socket.
        unsafe { libc::close(fd) };
    }

    #[test]
    fn frame_header_round_trips() {
        let c = Cred {
            pid: 42,
            uid: 1036,
            gid: 1037,
        };
        let (len, d) = parse_header(&header(99, c));
        assert_eq!((len, d.pid, d.uid, d.gid), (99, 42, 1036, 1037));
    }
}
