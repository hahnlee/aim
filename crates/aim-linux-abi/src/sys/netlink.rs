//! AF_NETLINK, NETLINK_ROUTE: the rtnetlink subset Android's network stack
//! uses on the kernel's devices (`netif`): link and address dumps
//! (bionic's getifaddrs), address and link changes (netd, NetworkStack),
//! and the link and address groups (EthernetTracker's and IpClient's
//! NetlinkMonitors). Other netlink families are EPROTONOSUPPORT.
//!
//! A netlink socket is a host AF_UNIX datagram socket bound to
//! `<runtime>/netlink/<portid>-<groups>`, so poll, epoll and plain reads
//! work on it. A request is handled in the calling process, and the
//! replies and every change's announcement are datagrams sent to the
//! listening sockets' names, one netlink message each (a dump is several
//! reads, as a large dump is on Linux). A name whose socket is gone is
//! removed when a send finds it refused.
//!
//! - Dumps: RTM_GETLINK, RTM_GETADDR; RTM_GETROUTE, RTM_GETNEIGH and
//!   RTM_GETRULE dump nothing (no routing table or neighbours: the host
//!   routes every socket).
//! - Changes: RTM_NEWLINK/RTM_SETLINK (flags, MTU), RTM_NEWADDR,
//!   RTM_DELADDR; with CAP_NET_ADMIN.
//! - Groups: RTNLGRP_LINK, RTNLGRP_IPV4_IFADDR, RTNLGRP_IPV6_IFADDR.

use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use super::netif::{self, Addr, Event, Link};
use crate::errno::{self, EINVAL, ENODEV, EPERM, Errno};

pub const NETLINK_ROUTE: u64 = 0;
pub const L_AF_NETLINK: u16 = 16;
pub const SOL_NETLINK: u64 = 270;

const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const NLM_F_REQUEST: u16 = 0x1;
const NLM_F_MULTI: u16 = 0x2;
const NLM_F_ACK: u16 = 0x4;
const NLM_F_DUMP: u16 = 0x300;
const NLM_F_REPLACE: u16 = 0x100;
const NLM_F_EXCL: u16 = 0x200;

const RTM_NEWLINK: u16 = 16;
const RTM_DELLINK: u16 = 17;
const RTM_GETLINK: u16 = 18;
const RTM_SETLINK: u16 = 19;
const RTM_NEWADDR: u16 = 20;
const RTM_DELADDR: u16 = 21;
const RTM_GETADDR: u16 = 22;
const RTM_GETROUTE: u16 = 26;
const RTM_GETNEIGH: u16 = 30;
const RTM_GETRULE: u16 = 34;

const IFLA_ADDRESS: u16 = 1;
const IFLA_BROADCAST: u16 = 2;
const IFLA_IFNAME: u16 = 3;
const IFLA_MTU: u16 = 4;
const IFLA_TXQLEN: u16 = 13;
const IFLA_OPERSTATE: u16 = 16;

const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;
const IFA_LABEL: u16 = 3;
const IFA_BROADCAST: u16 = 4;
const IFA_CACHEINFO: u16 = 6;
const IFA_FLAGS: u16 = 8;
const IFA_F_PERMANENT: u32 = 0x80;

/// RTNLGRP_LINK, RTNLGRP_IPV4_IFADDR, RTNLGRP_IPV6_IFADDR as bind masks.
const GRP_LINK: u32 = 1 << 0;
const GRP_IPV4_IFADDR: u32 = 1 << 4;
const GRP_IPV6_IFADDR: u32 = 1 << 8;

const NETLINK_ADD_MEMBERSHIP: u64 = 1;
const NETLINK_DROP_MEMBERSHIP: u64 = 2;
const NETLINK_NO_ENOBUFS: u64 = 5;
const NETLINK_CAP_ACK: u64 = 10;
const NETLINK_EXT_ACK: u64 = 11;
const NETLINK_GET_STRICT_CHK: u64 = 12;

const EOPNOTSUPP: Errno = 95;
const ECONNREFUSED: Errno = 111;
const EADDRINUSE: Errno = 98;
const CAP_NET_ADMIN: u32 = 12;

/// A netlink socket's state.
pub struct Socket {
    /// SOCK_RAW or SOCK_DGRAM (the same for netlink).
    pub ty: u64,
    portid: AtomicU32,
    groups: AtomicU32,
    cap_ack: AtomicU32,
    /// The host name the socket is bound to.
    path: Mutex<PathBuf>,
    /// The process that bound it, which removes the name when it closes
    /// the socket (a fork child that closes its copy does not).
    owner: i64,
}

impl Drop for Socket {
    fn drop(&mut self) {
        if self.owner == super::process::getpid() {
            let _ = std::fs::remove_file(&*self.path.lock().unwrap());
        }
    }
}

fn dir() -> PathBuf {
    netif::kernel_dir("netlink")
}

fn name_of(portid: u32, groups: u32) -> String {
    format!("{}-{groups:x}", portid as i32)
}

static NEXT: AtomicU32 = AtomicU32::new(0);

/// Bind host socket `fd` to the name of `portid` (and `groups`), replacing
/// a stale name of the same port.
fn bind_name(fd: i32, portid: u32, groups: u32) -> Result<PathBuf, Errno> {
    let p = dir().join(name_of(portid, groups));
    let t = super::net::host_target_of_path(&p);
    for _ in 0..2 {
        let r = super::net::with_target(&t, |sa, len| {
            // SAFETY: binding our socket to a name in the netlink directory.
            errno::check(unsafe { libc::bind(fd, sa, len) } as i64)
        });
        match r {
            0 => return Ok(p),
            r if r == -(EADDRINUSE as i64) && !alive(&p) => {
                let _ = std::fs::remove_file(&p);
            }
            r => return Err(-r as Errno),
        }
    }
    Err(EADDRINUSE)
}

/// Whether a socket is bound at `p`: connecting a datagram probe to a
/// name with no socket is refused.
fn alive(p: &std::path::Path) -> bool {
    // SAFETY: a probe socket of our own, closed below.
    let probe = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_DGRAM, 0) };
    let r = super::net::with_target(&super::net::host_target_of_path(p), |sa, len| {
        // SAFETY: connecting our probe.
        errno::check(unsafe { libc::connect(probe, sa, len) } as i64)
    });
    // SAFETY: our probe.
    unsafe { libc::close(probe) };
    r != -(ECONNREFUSED as i64) && r != -2
}

/// A new port id: the process id for its first socket, then negative
/// numbers unique to the process, as Linux autobinds.
fn new_portid() -> u32 {
    let pid = super::process::getpid() as u32;
    match NEXT.fetch_add(1, Ordering::Relaxed) {
        0 => pid,
        n => (-((pid as i32 & 0xf_ffff) << 11 | (n as i32 & 0x7ff)) - 4096) as u32,
    }
}

/// `socket(AF_NETLINK, ty, proto)`: a bound host datagram socket.
pub fn socket(ty: u64, proto: u64) -> Result<(i32, Socket), i64> {
    if proto != NETLINK_ROUTE {
        return Err(-93); // EPROTONOSUPPORT
    }
    // SAFETY: a plain datagram socket.
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_DGRAM, 0) };
    if fd < 0 {
        return Err(-(errno::last() as i64));
    }
    super::net::size_buffers(fd);
    let portid = new_portid();
    match bind_name(fd, portid, 0) {
        Ok(path) => Ok((
            fd,
            Socket {
                ty,
                portid: AtomicU32::new(portid),
                groups: AtomicU32::new(0),
                cap_ack: AtomicU32::new(0),
                path: Mutex::new(path),
                owner: super::process::getpid(),
            },
        )),
        Err(e) => {
            // SAFETY: closing the socket we made.
            unsafe { libc::close(fd) };
            Err(-(e as i64))
        }
    }
}

impl Socket {
    /// Give the socket a new port id and group set, renaming its name.
    fn rename(&self, portid: u32, groups: u32) -> Result<(), Errno> {
        let mut path = self.path.lock().unwrap();
        let new = dir().join(name_of(portid, groups));
        if new != *path {
            if portid != self.portid.load(Ordering::Relaxed) && alive(&new) {
                return Err(EADDRINUSE);
            }
            std::fs::rename(&*path, &new).map_err(|e| e.raw_os_error().unwrap_or(EINVAL))?;
            *path = new;
        }
        self.portid.store(portid, Ordering::Relaxed);
        self.groups.store(groups, Ordering::Relaxed);
        listening(groups);
        Ok(())
    }

    /// Linux `struct sockaddr_nl` of this socket.
    pub fn local_name(&self) -> Vec<u8> {
        sockaddr_nl(
            self.portid.load(Ordering::Relaxed),
            self.groups.load(Ordering::Relaxed),
        )
    }

    /// Fork: the socket's state.
    pub fn save(&self, w: &mut super::fork_state::Writer) {
        w.u64(self.ty);
        w.u32(self.portid.load(Ordering::Relaxed));
        w.u32(self.groups.load(Ordering::Relaxed));
        w.u32(self.cap_ack.load(Ordering::Relaxed));
        w.bytes(self.path.lock().unwrap().as_os_str().as_encoded_bytes());
    }

    pub fn load(r: &mut super::fork_state::Reader) -> Socket {
        let ty = r.u64();
        let (portid, groups, cap_ack) = (r.u32(), r.u32(), r.u32());
        let path = PathBuf::from(String::from_utf8_lossy(&r.bytes()).into_owned());
        Socket {
            ty,
            portid: AtomicU32::new(portid),
            groups: AtomicU32::new(groups),
            cap_ack: AtomicU32::new(cap_ack),
            path: Mutex::new(path),
            owner: 0,
        }
    }
}

/// A process listening to link changes watches the Mac's network, whose
/// changes are link changes.
fn listening(groups: u32) {
    if groups & GRP_LINK != 0 {
        super::uplink::watch(netif::refresh);
    }
}

/// The state of the socket bound to `name` in the netlink directory (a
/// netlink socket that arrived by exec or `SCM_RIGHTS`).
pub fn adopt(name: &std::ffi::OsStr) -> Option<Socket> {
    let (p, g) = name.to_str()?.rsplit_once('-')?;
    let (portid, groups) = (
        p.parse::<i32>().ok()? as u32,
        u32::from_str_radix(g, 16).ok()?,
    );
    let path = dir().join(name);
    std::fs::symlink_metadata(&path).ok()?;
    listening(groups);
    Some(Socket {
        ty: 3,
        portid: AtomicU32::new(portid),
        groups: AtomicU32::new(groups),
        cap_ack: AtomicU32::new(0),
        path: Mutex::new(path),
        owner: super::process::getpid(),
    })
}

pub fn sockaddr_nl(portid: u32, groups: u32) -> Vec<u8> {
    let mut v = L_AF_NETLINK.to_le_bytes().to_vec();
    v.extend_from_slice(&[0, 0]);
    v.extend_from_slice(&portid.to_le_bytes());
    v.extend_from_slice(&groups.to_le_bytes());
    v
}

/// Parse a guest `sockaddr_nl`: (port id, groups).
pub fn parse_sockaddr(ptr: u64, len: u32) -> Result<(u32, u32), i64> {
    if ptr == 0 || len < 12 {
        return Err(-(EINVAL as i64));
    }
    // SAFETY: a guest sockaddr of at least 12 bytes.
    let b = unsafe { std::slice::from_raw_parts(ptr as *const u8, 12) };
    if u16::from_le_bytes([b[0], b[1]]) != L_AF_NETLINK {
        return Err(-(EINVAL as i64));
    }
    Ok((
        u32::from_le_bytes(b[4..8].try_into().unwrap()),
        u32::from_le_bytes(b[8..12].try_into().unwrap()),
    ))
}

/// `bind`: a port id of 0 keeps the socket's; groups replace its groups.
pub fn bind(s: &Socket, portid: u32, groups: u32) -> i64 {
    let portid = if portid == 0 {
        s.portid.load(Ordering::Relaxed)
    } else {
        portid
    };
    match s.rename(portid, groups) {
        Ok(()) => 0,
        Err(e) => -(e as i64),
    }
}

/// `connect`: only the kernel (port 0) is reachable.
pub fn connect(portid: u32) -> i64 {
    if portid == 0 {
        0
    } else {
        -(ECONNREFUSED as i64)
    }
}

pub fn setsockopt(s: &Socket, opt: u64, v: u32) -> i64 {
    let bit = |g: u32| {
        if (1..=32).contains(&g) {
            Ok(1u32 << (g - 1))
        } else {
            Err(-(EINVAL as i64))
        }
    };
    let groups = s.groups.load(Ordering::Relaxed);
    let r = match opt {
        NETLINK_ADD_MEMBERSHIP => bit(v).map(|b| groups | b),
        NETLINK_DROP_MEMBERSHIP => bit(v).map(|b| groups & !b),
        NETLINK_CAP_ACK => {
            s.cap_ack.store(v, Ordering::Relaxed);
            return 0;
        }
        // Extended acks are optional, the kernel checks strictly anyway,
        // and a full receive queue drops announcements without ENOBUFS.
        NETLINK_EXT_ACK | NETLINK_GET_STRICT_CHK | NETLINK_NO_ENOBUFS => return 0,
        _ => return -92, // ENOPROTOOPT
    };
    match r {
        Ok(g) => bind(s, 0, g),
        Err(e) => e,
    }
}

// ---- delivery ---------------------------------------------------------------

/// The process's unbound sending socket.
fn sender() -> i32 {
    static FD: OnceLock<i32> = OnceLock::new();
    *FD.get_or_init(|| {
        // SAFETY: a plain datagram socket, kept for the process.
        let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_DGRAM, 0) };
        if fd < 0 {
            return -1;
        }
        // SAFETY: plain fcntl on our fd.
        unsafe {
            libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK);
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        super::fdtab::hide(fd)
    })
}

/// Send one datagram to the socket named `p`. Returns the Linux errno of a
/// failure. A full queue drops the message, as Linux drops a broadcast to
/// a full receiver.
fn deliver(p: &std::path::Path, msg: &[u8]) -> Option<Errno> {
    let fd = sender();
    let r = super::net::with_target(&super::net::host_target_of_path(p), |sa, len| {
        // SAFETY: sending our buffer to a named socket.
        errno::check(unsafe { libc::sendto(fd, msg.as_ptr().cast(), msg.len(), 0, sa, len) } as i64)
    });
    (r < 0).then_some(-r as Errno)
}

/// Announce device changes to the sockets listening to their groups.
pub fn announce(events: &[Event]) {
    if events.is_empty() {
        return;
    }
    let msgs: Vec<(u32, Vec<u8>)> = events
        .iter()
        .map(|e| match e {
            Event::Link(l) => (GRP_LINK, link_msg(RTM_NEWLINK, 0, 0, 0, l)),
            Event::NewAddr(l, a) | Event::DelAddr(l, a) => {
                let ty = if matches!(e, Event::NewAddr(..)) {
                    RTM_NEWADDR
                } else {
                    RTM_DELADDR
                };
                let g = if a.ip.is_ipv4() {
                    GRP_IPV4_IFADDR
                } else {
                    GRP_IPV6_IFADDR
                };
                (g, addr_msg(ty, 0, 0, 0, l, a))
            }
        })
        .collect();
    let Ok(entries) = std::fs::read_dir(dir()) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(groups) = name
            .to_str()
            .and_then(|n| n.rsplit_once('-'))
            .and_then(|(_, g)| u32::from_str_radix(g, 16).ok())
        else {
            continue;
        };
        for (g, m) in &msgs {
            if groups & g == 0 {
                continue;
            }
            if matches!(deliver(&e.path(), m), Some(ECONNREFUSED | 2)) {
                let _ = std::fs::remove_file(e.path());
                break;
            }
        }
    }
}

// ---- messages ---------------------------------------------------------------

fn align(n: usize) -> usize {
    (n + 3) & !3
}

fn attr(out: &mut Vec<u8>, ty: u16, data: &[u8]) {
    out.extend_from_slice(&((4 + data.len()) as u16).to_le_bytes());
    out.extend_from_slice(&ty.to_le_bytes());
    out.extend_from_slice(data);
    out.resize(align(out.len()), 0);
}

fn nlmsg(ty: u16, flags: u16, seq: u32, portid: u32, body: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(16 + body.len());
    m.extend_from_slice(&((16 + body.len()) as u32).to_le_bytes());
    m.extend_from_slice(&ty.to_le_bytes());
    m.extend_from_slice(&flags.to_le_bytes());
    m.extend_from_slice(&seq.to_le_bytes());
    m.extend_from_slice(&portid.to_le_bytes());
    m.extend_from_slice(body);
    m
}

/// RTM_NEWLINK for `l`.
fn link_msg(ty: u16, flags: u16, seq: u32, portid: u32, l: &Link) -> Vec<u8> {
    let live = l.live_flags();
    let mut b = vec![0u8, 0];
    b.extend_from_slice(&l.arphrd().to_le_bytes());
    b.extend_from_slice(&(l.index as i32).to_le_bytes());
    b.extend_from_slice(&live.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    let mut name = l.name.as_bytes().to_vec();
    name.push(0);
    attr(&mut b, IFLA_IFNAME, &name);
    attr(&mut b, IFLA_MTU, &l.mtu.to_le_bytes());
    attr(&mut b, IFLA_TXQLEN, &1000u32.to_le_bytes());
    // IF_OPER_UNKNOWN for the loopback, else UP or DOWN.
    let oper = if l.loopback() {
        0u8
    } else if live & netif::IFF_RUNNING != 0 {
        6
    } else {
        2
    };
    attr(&mut b, IFLA_OPERSTATE, &[oper]);
    attr(&mut b, IFLA_ADDRESS, &l.mac);
    let bcast = if l.loopback() { [0u8; 6] } else { [0xff; 6] };
    attr(&mut b, IFLA_BROADCAST, &bcast);
    nlmsg(ty, flags, seq, portid, &b)
}

fn ip_bytes(ip: &IpAddr) -> Vec<u8> {
    match ip {
        IpAddr::V4(a) => a.octets().to_vec(),
        IpAddr::V6(a) => a.octets().to_vec(),
    }
}

/// RTM_NEWADDR or RTM_DELADDR for address `a` of `l`.
fn addr_msg(ty: u16, flags: u16, seq: u32, portid: u32, l: &Link, a: &Addr) -> Vec<u8> {
    let family = if a.ip.is_ipv4() { 2u8 } else { 10 };
    let mut b = vec![family, a.prefix, IFA_F_PERMANENT as u8, a.scope()];
    b.extend_from_slice(&l.index.to_le_bytes());
    let ip = ip_bytes(&a.ip);
    attr(&mut b, IFA_ADDRESS, &ip);
    if a.ip.is_ipv4() {
        attr(&mut b, IFA_LOCAL, &ip);
        if let Some(bc) = a.broadcast().filter(|_| !l.loopback()) {
            attr(&mut b, IFA_BROADCAST, &bc.octets());
        }
        let mut label = l.name.as_bytes().to_vec();
        label.push(0);
        attr(&mut b, IFA_LABEL, &label);
    }
    // Permanent: infinite preferred and valid lifetimes.
    let mut ci = Vec::new();
    for v in [u32::MAX, u32::MAX, 0, 0] {
        ci.extend_from_slice(&v.to_le_bytes());
    }
    attr(&mut b, IFA_CACHEINFO, &ci);
    attr(&mut b, IFA_FLAGS, &IFA_F_PERMANENT.to_le_bytes());
    nlmsg(ty, flags, seq, portid, &b)
}

/// The attributes after a fixed header of `hdr` bytes in `body`.
fn attrs(body: &[u8], hdr: usize) -> Vec<(u16, &[u8])> {
    let mut out = Vec::new();
    let mut at = hdr;
    while at + 4 <= body.len() {
        let len = u16::from_le_bytes([body[at], body[at + 1]]) as usize;
        let ty = u16::from_le_bytes([body[at + 2], body[at + 3]]) & 0x3fff;
        if len < 4 || at + len > body.len() {
            break;
        }
        out.push((ty, &body[at + 4..at + len]));
        at += align(len);
    }
    out
}

fn ip_of(family: u8, b: &[u8]) -> Option<IpAddr> {
    match (family, b.len()) {
        (2, 4) => Some(IpAddr::from(<[u8; 4]>::try_from(b).ok()?)),
        (10, 16) => Some(IpAddr::from(<[u8; 16]>::try_from(b).ok()?)),
        _ => None,
    }
}

fn admin() -> Result<(), Errno> {
    if super::cred::capable(CAP_NET_ADMIN) {
        Ok(())
    } else {
        Err(EPERM)
    }
}

/// Handle one request; its replies (and, for a dump, NLMSG_DONE) go to
/// `out`. Returns the errno to report (0: success).
fn request(
    ty: u16,
    flags: u16,
    seq: u32,
    portid: u32,
    body: &[u8],
    out: &mut Vec<Vec<u8>>,
) -> Errno {
    let multi = NLM_F_MULTI;
    let dump = flags & NLM_F_DUMP == NLM_F_DUMP;
    let r: Result<(), Errno> = match ty {
        RTM_GETLINK if dump => {
            for l in netif::links() {
                out.push(link_msg(RTM_NEWLINK, multi, seq, portid, &l));
            }
            Ok(())
        }
        RTM_GETLINK => {
            let index = body
                .get(4..8)
                .map_or(0, |b| i32::from_le_bytes(b.try_into().unwrap()));
            let name = attrs(body, 16)
                .into_iter()
                .find(|(t, _)| *t == IFLA_IFNAME)
                .map(|(_, v)| {
                    String::from_utf8_lossy(v)
                        .trim_end_matches('\0')
                        .to_string()
                });
            let l = if index > 0 {
                netif::by_index(index as u32)
            } else {
                name.and_then(|n| netif::by_name(&n))
            };
            match l {
                Some(l) => {
                    out.push(link_msg(RTM_NEWLINK, 0, seq, portid, &l));
                    Ok(())
                }
                None => Err(ENODEV),
            }
        }
        RTM_GETADDR if dump => {
            let family = body.first().copied().unwrap_or(0);
            let index = body
                .get(4..8)
                .map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
            for l in netif::links() {
                if index != 0 && l.index != index {
                    continue;
                }
                for a in &l.addrs {
                    let f = if a.ip.is_ipv4() { 2 } else { 10 };
                    if family == 0 || family == f {
                        out.push(addr_msg(RTM_NEWADDR, multi, seq, portid, &l, a));
                    }
                }
            }
            Ok(())
        }
        RTM_GETROUTE | RTM_GETNEIGH | RTM_GETRULE if dump => Ok(()),
        RTM_NEWLINK | RTM_SETLINK => admin().and_then(|()| {
            if body.len() < 16 {
                return Err(EINVAL);
            }
            let index = i32::from_le_bytes(body[4..8].try_into().unwrap());
            let fl = u32::from_le_bytes(body[8..12].try_into().unwrap());
            // A zero ifi_change with flags sets them all; with none, none.
            let change = match u32::from_le_bytes(body[12..16].try_into().unwrap()) {
                0 if fl != 0 => u32::MAX,
                c => c,
            };
            let a = attrs(body, 16);
            let mtu = a
                .iter()
                .find(|(t, _)| *t == IFLA_MTU)
                .and_then(|(_, v)| Some(u32::from_le_bytes((*v).try_into().ok()?)));
            let l = if index > 0 {
                netif::by_index(index as u32)
            } else {
                a.iter()
                    .find(|(t, _)| *t == IFLA_IFNAME)
                    .and_then(|(_, v)| {
                        netif::by_name(String::from_utf8_lossy(v).trim_end_matches('\0'))
                    })
            };
            match l {
                // Creating links (a new index or kind) is not offered.
                Some(l) => netif::change_link(l.index, change, fl, mtu),
                None if ty == RTM_NEWLINK => Err(EOPNOTSUPP),
                None => Err(ENODEV),
            }
        }),
        RTM_DELLINK => admin().and(Err(EOPNOTSUPP)),
        RTM_NEWADDR | RTM_DELADDR => admin().and_then(|()| {
            if body.len() < 8 {
                return Err(EINVAL);
            }
            let (family, prefix) = (body[0], body[1]);
            let index = u32::from_le_bytes(body[4..8].try_into().unwrap());
            let a = attrs(body, 8);
            // IFA_LOCAL is the address; IFA_ADDRESS alone for IPv6.
            let ip = a
                .iter()
                .find(|(t, _)| *t == IFA_LOCAL)
                .or_else(|| a.iter().find(|(t, _)| *t == IFA_ADDRESS))
                .and_then(|(_, v)| ip_of(family, v))
                .ok_or(EINVAL)?;
            if ty == RTM_NEWADDR {
                let replace = flags & NLM_F_REPLACE != 0 && flags & NLM_F_EXCL == 0;
                netif::add_addr(index, Addr { ip, prefix }, replace)
            } else {
                netif::del_addr(index, ip, (prefix != 0).then_some(prefix))
            }
        }),
        _ => Err(EOPNOTSUPP),
    };
    match r {
        Ok(()) if dump => {
            out.push(nlmsg(NLMSG_DONE, multi, seq, portid, &0i32.to_le_bytes()));
            0
        }
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// A message the socket sent to the kernel: handle each request in it and
/// queue the replies on the socket. Returns the bytes consumed.
pub fn send(s: &Socket, data: &[u8]) -> i64 {
    let portid = s.portid.load(Ordering::Relaxed);
    let cap_ack = s.cap_ack.load(Ordering::Relaxed) != 0;
    let mut replies = Vec::new();
    let mut at = 0;
    while at + 16 <= data.len() {
        let len = u32::from_le_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        if len < 16 || at + len > data.len() {
            break;
        }
        let m = &data[at..at + len];
        let ty = u16::from_le_bytes([m[4], m[5]]);
        let flags = u16::from_le_bytes([m[6], m[7]]);
        let seq = u32::from_le_bytes(m[8..12].try_into().unwrap());
        // Control messages and non-requests are not answered.
        if flags & NLM_F_REQUEST != 0 && ty >= 16 {
            let err = request(ty, flags, seq, portid, &m[16..], &mut replies);
            if err != 0 || flags & NLM_F_ACK != 0 {
                // struct nlmsgerr: the error and the request's header,
                // with its payload for an error unless NETLINK_CAP_ACK.
                let mut b = (-err).to_le_bytes().to_vec();
                b.extend_from_slice(if err != 0 && !cap_ack { m } else { &m[..16] });
                replies.push(nlmsg(NLMSG_ERROR, 0, seq, portid, &b));
            }
        }
        at += align(len);
    }
    let path = s.path.lock().unwrap().clone();
    for r in &replies {
        deliver(&path, r);
    }
    data.len() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attributes_round_trip() {
        let mut b = vec![0u8; 8];
        attr(&mut b, IFA_ADDRESS, &[10, 0, 0, 1]);
        attr(&mut b, IFA_LABEL, b"eth0\0");
        let a = attrs(&b, 8);
        assert_eq!(
            a,
            vec![
                (IFA_ADDRESS, &[10u8, 0, 0, 1][..]),
                (IFA_LABEL, &b"eth0\0"[..])
            ]
        );
    }

    #[test]
    fn address_message_layout() {
        let l = Link {
            index: 2,
            name: "eth0".into(),
            flags: netif::IFF_UP,
            mtu: 1500,
            mac: netif::ETH0_MAC,
            addrs: Vec::new(),
            carrier: true,
        };
        let a = Addr {
            ip: "192.168.1.20".parse().unwrap(),
            prefix: 24,
        };
        let m = addr_msg(RTM_NEWADDR, 0, 7, 99, &l, &a);
        assert_eq!(
            u32::from_le_bytes(m[..4].try_into().unwrap()) as usize,
            m.len()
        );
        assert_eq!(u16::from_le_bytes([m[4], m[5]]), RTM_NEWADDR);
        assert_eq!(&m[16..20], &[2, 24, 0x80, 0]);
        assert_eq!(u32::from_le_bytes(m[20..24].try_into().unwrap()), 2);
        let at = attrs(&m[16..], 8);
        assert!(at.contains(&(IFA_LOCAL, &[192u8, 168, 1, 20][..])));
        assert!(at.contains(&(IFA_BROADCAST, &[192u8, 168, 1, 255][..])));
    }

    #[test]
    fn port_ids_are_unique_and_later_ones_negative() {
        let (a, b) = (new_portid(), new_portid());
        assert_ne!(a, b);
        assert!((b as i32) < -4096);
    }
}
