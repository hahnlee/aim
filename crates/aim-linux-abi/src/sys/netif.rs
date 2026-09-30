//! The kernel's network devices (docs/network.md): `lo` and `eth0`, their
//! flags, MTU, hardware address and IP addresses, shared by every process
//! of a boot, and the socket ioctls that read and set them.
//!
//! - Guest sockets are host sockets, so no packet goes through these
//!   devices; they are the configuration Android's network stack reads and
//!   writes (netd sets addresses with ioctls and netlink, IpClient and
//!   EthernetTracker learn of them through netlink, `netlink`).
//! - `eth0` stands for the Mac's network (`uplink`): it has carrier
//!   (IFF_RUNNING, IFF_LOWER_UP) while it is up and the Mac has an IPv4
//!   default route, and its virtual link's DHCP server leases the Mac's
//!   address (`dhcp`). Its hardware address is a fixed locally
//!   administered one.
//! - The Mac's network is part of the state: each process that reads or
//!   changes the devices compares it with the Mac's current one first, and
//!   the one that finds a change announces it (carrier lost or back, or
//!   lost and back for a different network, so that DHCP runs again).
//!   Processes listening to link changes look every few seconds and on
//!   each routing change (`uplink::watch`).
//! - There is no routing table: routes live in netd's per-network
//!   bookkeeping, and the host routes every socket.
//! - The state is a text file in the runtime directory
//!   (`<runtime>/net/links`), read and written under `flock`; each change
//!   is announced to the netlink groups it belongs to.

use std::io::{Read, Seek, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use super::netlink;
use super::uplink::{self, Uplink};
use crate::errno::{EFAULT, EINVAL, ENODEV, EPERM, Errno};
use crate::vfs;

pub const IFF_UP: u32 = 0x1;
pub const IFF_BROADCAST: u32 = 0x2;
pub const IFF_LOOPBACK: u32 = 0x8;
pub const IFF_RUNNING: u32 = 0x40;
pub const IFF_MULTICAST: u32 = 0x1000;
pub const IFF_LOWER_UP: u32 = 0x10000;
/// Flags userspace sets: UP, NOARP, PROMISC, ALLMULTI, MULTICAST.
const SETTABLE: u32 = IFF_UP | 0x80 | 0x100 | 0x200 | IFF_MULTICAST;

pub const ARPHRD_ETHER: u16 = 1;
pub const ARPHRD_LOOPBACK: u16 = 772;

const EADDRNOTAVAIL: Errno = 99;
const EEXIST: Errno = 17;
const CAP_NET_ADMIN: u32 = 12;

pub const LO: u32 = 1;
pub const ETH0: u32 = 2;
/// `eth0`'s hardware address: locally administered, "aim".
pub const ETH0_MAC: [u8; 6] = [0x02, 0x61, 0x69, 0x6d, 0x00, 0x02];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addr {
    pub ip: IpAddr,
    pub prefix: u8,
}

impl Addr {
    /// RT_SCOPE_HOST for loopback addresses, RT_SCOPE_LINK for IPv6
    /// link-local ones, RT_SCOPE_UNIVERSE for the rest.
    pub fn scope(&self) -> u8 {
        match self.ip {
            IpAddr::V4(a) if a.is_loopback() => 254,
            IpAddr::V6(a) if a.is_loopback() => 254,
            IpAddr::V6(a) if a.segments()[0] & 0xffc0 == 0xfe80 => 253,
            _ => 0,
        }
    }

    /// The IPv4 broadcast address of the subnet (None for /31 and /32).
    pub fn broadcast(&self) -> Option<Ipv4Addr> {
        match self.ip {
            IpAddr::V4(a) if self.prefix < 31 => {
                Some(Ipv4Addr::from(u32::from(a) | (u32::MAX >> self.prefix)))
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    pub index: u32,
    pub name: String,
    /// The flags userspace set, without the operational ones.
    pub flags: u32,
    pub mtu: u32,
    pub mac: [u8; 6],
    pub addrs: Vec<Addr>,
    /// Whether the link has carrier: always for the loopback, while the
    /// Mac has a network for `eth0`.
    pub carrier: bool,
}

impl Link {
    pub fn loopback(&self) -> bool {
        self.flags & IFF_LOOPBACK != 0
    }

    pub fn arphrd(&self) -> u16 {
        if self.loopback() {
            ARPHRD_LOOPBACK
        } else {
            ARPHRD_ETHER
        }
    }

    /// The flags as the kernel reports them, with IFF_RUNNING and
    /// IFF_LOWER_UP while the link is up and has carrier.
    pub fn live_flags(&self) -> u32 {
        if self.flags & IFF_UP != 0 && self.carrier {
            self.flags | IFF_RUNNING | IFF_LOWER_UP
        } else {
            self.flags
        }
    }

    fn primary_v4(&self) -> Option<usize> {
        self.addrs.iter().position(|a| a.ip.is_ipv4())
    }
}

/// A change, as netlink announces it.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Link(Link),
    NewAddr(Link, Addr),
    DelAddr(Link, Addr),
}

/// The devices at boot: the loopback up with its addresses, `eth0` down
/// with none.
fn boot_links() -> Vec<Link> {
    vec![
        Link {
            index: LO,
            name: "lo".into(),
            flags: IFF_UP | IFF_LOOPBACK,
            mtu: 65536,
            mac: [0; 6],
            addrs: vec![
                Addr {
                    ip: Ipv4Addr::LOCALHOST.into(),
                    prefix: 8,
                },
                Addr {
                    ip: Ipv6Addr::LOCALHOST.into(),
                    prefix: 128,
                },
            ],
            carrier: true,
        },
        Link {
            index: ETH0,
            name: "eth0".into(),
            flags: IFF_BROADCAST | IFF_MULTICAST,
            mtu: 1500,
            mac: ETH0_MAC,
            addrs: Vec::new(),
            carrier: false,
        },
    ]
}

fn mac_text(m: &[u8; 6]) -> String {
    m.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn serialize(links: &[Link], up: &Option<Uplink>) -> String {
    let mut s = String::new();
    if let Some(u) = up {
        s += &format!(
            "uplink {} {}/{} {} {}\n",
            u.index, u.addr, u.prefix, u.gateway, u.mtu
        );
    }
    for l in links {
        s += &format!(
            "link {} {} {:x} {} {}\n",
            l.index,
            l.name,
            l.flags,
            l.mtu,
            mac_text(&l.mac)
        );
        for a in &l.addrs {
            s += &format!("addr {} {}/{}\n", l.index, a.ip, a.prefix);
        }
    }
    s
}

/// The devices and the Mac's network they last saw.
fn parse(text: &str) -> Option<(Vec<Link>, Option<Uplink>)> {
    let mut links: Vec<Link> = Vec::new();
    let mut up = None;
    for line in text.lines() {
        let w: Vec<&str> = line.split(' ').collect();
        match w.as_slice() {
            ["uplink", index, addr, gateway, mtu] => {
                let (a, p) = addr.split_once('/')?;
                up = Some(Uplink {
                    index: index.parse().ok()?,
                    addr: a.parse().ok()?,
                    prefix: p.parse().ok()?,
                    gateway: gateway.parse().ok()?,
                    mtu: mtu.parse().ok()?,
                });
            }
            ["link", index, name, flags, mtu, mac] => {
                let mut m = [0u8; 6];
                for (i, b) in mac.split(':').enumerate() {
                    *m.get_mut(i)? = u8::from_str_radix(b, 16).ok()?;
                }
                links.push(Link {
                    index: index.parse().ok()?,
                    name: name.to_string(),
                    flags: u32::from_str_radix(flags, 16).ok()?,
                    mtu: mtu.parse().ok()?,
                    mac: m,
                    addrs: Vec::new(),
                    carrier: false,
                });
            }
            ["addr", index, addr] => {
                let (ip, prefix) = addr.split_once('/')?;
                let index: u32 = index.parse().ok()?;
                links
                    .iter_mut()
                    .find(|l| l.index == index)?
                    .addrs
                    .push(Addr {
                        ip: ip.parse().ok()?,
                        prefix: prefix.parse().ok()?,
                    });
            }
            _ => return None,
        }
    }
    for l in &mut links {
        l.carrier = l.loopback() || up.is_some();
    }
    Some((links, up))
}

/// Bring the devices from the Mac's network `was` to `now`: carrier
/// follows it, and a different network takes the carrier away and back,
/// as moving a cable to another network would. Returns whether it
/// changed.
fn follow_uplink(
    links: &mut [Link],
    was: &Option<Uplink>,
    now: &Option<Uplink>,
    ev: &mut Vec<Event>,
) -> bool {
    if was == now {
        return false;
    }
    for l in links.iter_mut().filter(|l| !l.loopback()) {
        if l.flags & IFF_UP != 0 && was.is_some() && now.is_some() {
            l.carrier = false;
            ev.push(Event::Link(l.clone()));
        }
        l.carrier = now.is_some();
        if l.flags & IFF_UP != 0 {
            ev.push(Event::Link(l.clone()));
        }
    }
    true
}

/// A directory of kernel state shared by the processes of one boot: in the
/// runtime directory, or (with no path map) a temporary one per user and
/// guest root.
pub fn kernel_dir(name: &str) -> PathBuf {
    let d = match vfs::runtime_dir() {
        Some(r) => r.join(name),
        None => {
            use sha2::Digest;
            let h = sha2::Sha256::digest(vfs::root().as_os_str().as_bytes());
            let tag: String = h[..6].iter().map(|b| format!("{b:02x}")).collect();
            // SAFETY: trivial.
            std::env::temp_dir().join(format!("linux-abi-{name}-{}-{tag}", unsafe {
                libc::getuid()
            }))
        }
    };
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Run `f` on the device state under its lock, after following the Mac's
/// network; write back what changed and announce the events.
fn transact<R>(f: impl FnOnce(&mut Vec<Link>, &mut Vec<Event>) -> R) -> R {
    let path = kernel_dir("net").join("links");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path);
    let Ok(mut file) = file else {
        // No shared state to be had: this process's own view.
        let mut links = boot_links();
        follow_uplink(&mut links, &None, &uplink::current(), &mut Vec::new());
        return f(&mut links, &mut Vec::new());
    };
    // SAFETY: locking our open file; released when it closes.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    let mut text = String::new();
    let _ = file.read_to_string(&mut text);
    let (mut links, was) = parse(&text)
        .filter(|(l, _)| !l.is_empty())
        .unwrap_or_else(|| (boot_links(), None));
    let mut events = Vec::new();
    let now = uplink::current();
    if follow_uplink(&mut links, &was, &now, &mut events) {
        match &now {
            Some(u) => crate::diag!(
                "[linux-abi] net: the Mac's network is {}/{} via {} (host interface {})",
                u.addr,
                u.prefix,
                u.gateway,
                u.index
            ),
            None => crate::diag!("[linux-abi] net: the Mac has no network; eth0 has no carrier"),
        }
    }
    let r = f(&mut links, &mut events);
    let new = serialize(&links, &now);
    if new != text {
        let _ = file.set_len(0);
        let _ = file.rewind();
        let _ = file.write_all(new.as_bytes());
    }
    netlink::announce(&events);
    r
}

pub fn links() -> Vec<Link> {
    transact(|l, _| l.clone())
}

/// Follow the Mac's network (`uplink::watch`).
pub fn refresh() {
    transact(|_, _| ());
}

pub fn by_index(index: u32) -> Option<Link> {
    links().into_iter().find(|l| l.index == index)
}

pub fn by_name(name: &str) -> Option<Link> {
    links().into_iter().find(|l| l.name == name)
}

fn find(links: &mut [Link], index: u32) -> Result<&mut Link, Errno> {
    links.iter_mut().find(|l| l.index == index).ok_or(ENODEV)
}

/// Set the flags in `mask` to `flags`, and the MTU.
pub fn change_link(index: u32, mask: u32, flags: u32, mtu: Option<u32>) -> Result<(), Errno> {
    transact(|links, ev| {
        let l = find(links, index)?;
        let mask = mask & SETTABLE;
        let before = (l.flags, l.mtu);
        l.flags = (l.flags & !mask) | (flags & mask);
        if let Some(m) = mtu {
            if !(68..=65536).contains(&m) {
                return Err(EINVAL);
            }
            l.mtu = m;
        }
        if (l.flags, l.mtu) != before {
            ev.push(Event::Link(l.clone()));
        }
        Ok(())
    })
}

/// Add `a` to the link; an existing address is EEXIST unless `replace`.
pub fn add_addr(index: u32, a: Addr, replace: bool) -> Result<(), Errno> {
    let max = if a.ip.is_ipv4() { 32 } else { 128 };
    if a.prefix > max {
        return Err(EINVAL);
    }
    transact(|links, ev| {
        let l = find(links, index)?;
        match l.addrs.iter_mut().find(|x| x.ip == a.ip) {
            Some(_) if !replace => return Err(EEXIST),
            Some(x) => *x = a,
            None => l.addrs.push(a),
        }
        ev.push(Event::NewAddr(l.clone(), a));
        Ok(())
    })
}

/// Remove the link's address `ip` (with `prefix`, when given).
pub fn del_addr(index: u32, ip: IpAddr, prefix: Option<u8>) -> Result<(), Errno> {
    transact(|links, ev| {
        let l = find(links, index)?;
        let i = l
            .addrs
            .iter()
            .position(|x| x.ip == ip && prefix.is_none_or(|p| p == x.prefix))
            .ok_or(EADDRNOTAVAIL)?;
        let a = l.addrs.remove(i);
        ev.push(Event::DelAddr(l.clone(), a));
        Ok(())
    })
}

/// The classful prefix `SIOCSIFADDR` gives a new address.
fn classful(a: Ipv4Addr) -> u8 {
    match a.octets()[0] {
        0..=127 => 8,
        128..=191 => 16,
        _ => 24,
    }
}

/// `SIOCSIFADDR`: replace the primary IPv4 address; 0.0.0.0 removes it.
fn set_primary_v4(index: u32, ip: Ipv4Addr) -> Result<(), Errno> {
    transact(|links, ev| {
        let l = find(links, index)?;
        if let Some(i) = l.primary_v4() {
            if l.addrs[i].ip == IpAddr::V4(ip) {
                return Ok(());
            }
            let old = l.addrs.remove(i);
            ev.push(Event::DelAddr(l.clone(), old));
        }
        if !ip.is_unspecified() {
            let a = Addr {
                ip: ip.into(),
                prefix: classful(ip),
            };
            l.addrs.insert(0, a);
            ev.push(Event::NewAddr(l.clone(), a));
        }
        Ok(())
    })
}

/// `SIOCSIFNETMASK`: the primary IPv4 address's prefix.
fn set_netmask_v4(index: u32, mask: Ipv4Addr) -> Result<(), Errno> {
    let m = u32::from(mask);
    let prefix = m.leading_ones();
    if m.checked_shl(prefix).unwrap_or(0) != 0 {
        return Err(EINVAL);
    }
    transact(|links, ev| {
        let l = find(links, index)?;
        let i = l.primary_v4().ok_or(EADDRNOTAVAIL)?;
        if l.addrs[i].prefix as u32 != prefix {
            let old = l.addrs[i];
            l.addrs[i].prefix = prefix as u8;
            let new = l.addrs[i];
            ev.push(Event::DelAddr(l.clone(), old));
            ev.push(Event::NewAddr(l.clone(), new));
        }
        Ok(())
    })
}

/// The host interface a guest socket bound to device `name` uses
/// (`SO_BINDTODEVICE` as `IP_BOUND_IF`): 0 for none.
pub fn host_index_of(name: &str) -> Result<u32, Errno> {
    if name.is_empty() {
        return Ok(0);
    }
    let l = by_name(name).ok_or(ENODEV)?;
    Ok(if l.loopback() {
        uplink::host_index("lo0")
    } else {
        uplink::current().map_or(0, |u| u.index)
    })
}

// ---- ioctls -----------------------------------------------------------------

const SIOCGIFNAME: u64 = 0x8910;
const SIOCGIFCONF: u64 = 0x8912;
const SIOCGIFFLAGS: u64 = 0x8913;
const SIOCSIFFLAGS: u64 = 0x8914;
const SIOCGIFADDR: u64 = 0x8915;
const SIOCSIFADDR: u64 = 0x8916;
const SIOCGIFBRDADDR: u64 = 0x8919;
const SIOCGIFNETMASK: u64 = 0x891b;
const SIOCSIFNETMASK: u64 = 0x891c;
const SIOCGIFMTU: u64 = 0x8921;
const SIOCSIFMTU: u64 = 0x8922;
const SIOCGIFHWADDR: u64 = 0x8927;
const SIOCGIFINDEX: u64 = 0x8933;

/// Linux `struct ifreq`: a 16-byte name and a 24-byte union.
const IFREQ: usize = 40;
const IFNAMSIZ: usize = 16;

/// A Linux `sockaddr_in` of `a` (16 bytes).
fn sockaddr_in(a: Ipv4Addr) -> [u8; 16] {
    let mut s = [0u8; 16];
    s[..2].copy_from_slice(&2u16.to_le_bytes());
    s[4..8].copy_from_slice(&a.octets());
    s
}

fn prefix_mask(p: u8) -> Ipv4Addr {
    Ipv4Addr::from(u32::MAX.checked_shl(32 - p as u32).unwrap_or(0))
}

fn is_socket(fd: i32) -> bool {
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat into a local buffer.
    let host =
        unsafe { libc::fstat(fd, &mut st) == 0 && st.st_mode & libc::S_IFMT == libc::S_IFSOCK };
    host && !super::net::hidden_socket(fd)
}

/// The socket ioctls on interfaces. None: not one of them (or not a
/// socket).
pub fn ioctl(fd: i32, req: u64, arg: u64) -> Option<i64> {
    if !(SIOCGIFNAME..=SIOCGIFINDEX).contains(&req) || !is_socket(fd) {
        return None;
    }
    if arg == 0 {
        return Some(-(EFAULT as i64));
    }
    let r = match req {
        SIOCGIFCONF => ifconf(arg),
        SIOCGIFNAME => ifreq(arg, |req| {
            let index = i32::from_le_bytes(req[16..20].try_into().unwrap());
            let l = by_index(index as u32).ok_or(ENODEV)?;
            req[..IFNAMSIZ].fill(0);
            req[..l.name.len()].copy_from_slice(l.name.as_bytes());
            Ok(())
        }),
        SIOCGIFFLAGS | SIOCSIFFLAGS | SIOCGIFADDR | SIOCSIFADDR | SIOCGIFBRDADDR
        | SIOCGIFNETMASK | SIOCSIFNETMASK | SIOCGIFMTU | SIOCSIFMTU | SIOCGIFHWADDR
        | SIOCGIFINDEX => ifreq(arg, |r| named(req, r)),
        _ => return None,
    };
    Some(match r {
        Ok(()) => 0,
        Err(e) => -(e as i64),
    })
}

/// Run `f` on the guest's `struct ifreq`, copied in and back out.
fn ifreq(arg: u64, f: impl FnOnce(&mut [u8; IFREQ]) -> Result<(), Errno>) -> Result<(), Errno> {
    let mut req = [0u8; IFREQ];
    // SAFETY: the guest's struct ifreq.
    unsafe { std::ptr::copy_nonoverlapping(arg as *const u8, req.as_mut_ptr(), IFREQ) };
    f(&mut req)?;
    // SAFETY: as above.
    unsafe { std::ptr::copy_nonoverlapping(req.as_ptr(), arg as *mut u8, IFREQ) };
    Ok(())
}

fn ifreq_ipv4(req: &[u8; IFREQ]) -> Result<Ipv4Addr, Errno> {
    if u16::from_le_bytes([req[16], req[17]]) != 2 {
        return Err(EINVAL);
    }
    Ok(Ipv4Addr::new(req[20], req[21], req[22], req[23]))
}

/// The ioctls on one interface named in `ifr_name`.
fn named(cmd: u64, req: &mut [u8; IFREQ]) -> Result<(), Errno> {
    let end = req[..IFNAMSIZ]
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(IFNAMSIZ);
    let name = std::str::from_utf8(&req[..end]).map_err(|_| ENODEV)?;
    // An alias label (eth0:1) names its device.
    let dev = name.split(':').next().unwrap_or(name);
    let l = by_name(dev).ok_or(ENODEV)?;
    let admin = || {
        if super::cred::capable(CAP_NET_ADMIN) {
            Ok(())
        } else {
            Err(EPERM)
        }
    };
    let v4 = l.primary_v4().map(|i| (l.addrs[i].ip, l.addrs[i].prefix));
    let data = &mut req[16..];
    match cmd {
        // ifr_flags is a short: IFF_LOWER_UP does not fit.
        SIOCGIFFLAGS => data[..2].copy_from_slice(&(l.live_flags() as u16).to_le_bytes()),
        SIOCSIFFLAGS => {
            admin()?;
            let f = u16::from_le_bytes([data[0], data[1]]) as u32;
            change_link(l.index, SETTABLE, f, None)?;
        }
        SIOCGIFADDR | SIOCGIFBRDADDR | SIOCGIFNETMASK => {
            let Some((IpAddr::V4(a), p)) = v4 else {
                return Err(EADDRNOTAVAIL);
            };
            let v = match cmd {
                SIOCGIFADDR => a,
                SIOCGIFNETMASK => prefix_mask(p),
                _ => Addr {
                    ip: a.into(),
                    prefix: p,
                }
                .broadcast()
                .unwrap_or(Ipv4Addr::UNSPECIFIED),
            };
            data[..16].copy_from_slice(&sockaddr_in(v));
        }
        SIOCSIFADDR => {
            admin()?;
            set_primary_v4(l.index, ifreq_ipv4(req)?)?;
        }
        SIOCSIFNETMASK => {
            admin()?;
            set_netmask_v4(l.index, ifreq_ipv4(req)?)?;
        }
        SIOCGIFMTU => data[..4].copy_from_slice(&l.mtu.to_le_bytes()),
        SIOCSIFMTU => {
            admin()?;
            let m = u32::from_le_bytes(data[..4].try_into().unwrap());
            change_link(l.index, 0, 0, Some(m))?;
        }
        SIOCGIFHWADDR => {
            data[..16].fill(0);
            data[..2].copy_from_slice(&l.arphrd().to_le_bytes());
            data[2..8].copy_from_slice(&l.mac);
        }
        SIOCGIFINDEX => data[..4].copy_from_slice(&(l.index as i32).to_le_bytes()),
        _ => return Err(EINVAL),
    }
    Ok(())
}

/// `SIOCGIFCONF`: an ifreq with the address of each interface that has an
/// IPv4 address; with no buffer, the length needed.
fn ifconf(arg: u64) -> Result<(), Errno> {
    // Linux arm64 struct ifconf: int ifc_len, then the buffer pointer at 8.
    // SAFETY: the guest's struct ifconf.
    let (len, buf) = unsafe {
        (
            (arg as *const i32).read_unaligned(),
            ((arg + 8) as *const u64).read_unaligned(),
        )
    };
    let mut out = Vec::new();
    for l in links() {
        for a in &l.addrs {
            if let IpAddr::V4(v) = a.ip {
                let mut r = [0u8; IFREQ];
                r[..l.name.len()].copy_from_slice(l.name.as_bytes());
                r[16..32].copy_from_slice(&sockaddr_in(v));
                out.extend_from_slice(&r);
                break;
            }
        }
    }
    let n = if buf == 0 {
        out.len()
    } else {
        let fit = (len.max(0) as usize / IFREQ * IFREQ).min(out.len());
        // SAFETY: the guest's buffer of ifc_len bytes.
        unsafe { std::ptr::copy_nonoverlapping(out.as_ptr(), buf as *mut u8, fit) };
        fit
    };
    // SAFETY: as above.
    unsafe { (arg as *mut i32).write_unaligned(n as i32) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_round_trips() {
        let mut l = boot_links();
        l[1].flags |= IFF_UP;
        l[1].addrs.push(Addr {
            ip: "192.168.1.20".parse().unwrap(),
            prefix: 24,
        });
        assert_eq!(parse(&serialize(&l, &None)).unwrap(), (l.clone(), None));
        let up = Some(Uplink {
            index: 4,
            addr: "192.168.1.20".parse().unwrap(),
            prefix: 24,
            gateway: "192.168.1.1".parse().unwrap(),
            mtu: 1500,
        });
        l[1].carrier = true;
        assert_eq!(parse(&serialize(&l, &up)).unwrap(), (l, up));
    }

    #[test]
    fn carrier_follows_the_macs_network() {
        let up = |a: &str| {
            Some(Uplink {
                index: 4,
                addr: a.parse().unwrap(),
                prefix: 24,
                gateway: "192.168.1.1".parse().unwrap(),
                mtu: 1500,
            })
        };
        let flags = |ev: &[Event]| -> Vec<u32> {
            ev.iter()
                .map(|e| match e {
                    Event::Link(l) => l.live_flags() & (IFF_RUNNING | IFF_LOWER_UP),
                    _ => panic!("{e:?}"),
                })
                .collect()
        };
        let on = IFF_RUNNING | IFF_LOWER_UP;
        let mut l = boot_links();
        let mut ev = Vec::new();
        // Down: carrier changes silently.
        assert!(follow_uplink(&mut l, &None, &up("192.168.1.20"), &mut ev));
        assert!(ev.is_empty() && l[1].carrier && l[0].carrier);
        l[1].flags |= IFF_UP;
        // Up: lost, back, and lost and back for another network.
        let was = up("192.168.1.20");
        assert!(!follow_uplink(&mut l, &was, &was.clone(), &mut ev));
        assert!(follow_uplink(&mut l, &was, &None, &mut ev));
        assert_eq!(flags(&ev), [0]);
        ev.clear();
        assert!(follow_uplink(&mut l, &None, &was, &mut ev));
        assert_eq!(flags(&ev), [on]);
        ev.clear();
        assert!(follow_uplink(&mut l, &was, &up("10.0.0.5"), &mut ev));
        assert_eq!(flags(&ev), [0, on]);
        assert!(l[1].carrier);
    }

    #[test]
    fn scopes_and_broadcast() {
        let a = |s: &str, p| Addr {
            ip: s.parse().unwrap(),
            prefix: p,
        };
        assert_eq!(a("127.0.0.1", 8).scope(), 254);
        assert_eq!(a("fe80::1", 64).scope(), 253);
        assert_eq!(a("10.1.2.3", 8).scope(), 0);
        assert_eq!(
            a("192.168.1.20", 24).broadcast(),
            Some(Ipv4Addr::new(192, 168, 1, 255))
        );
        assert_eq!(a("10.0.0.1", 32).broadcast(), None);
    }

    #[test]
    fn masks() {
        assert_eq!(prefix_mask(24), Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(prefix_mask(0), Ipv4Addr::UNSPECIFIED);
        assert_eq!(classful(Ipv4Addr::new(172, 30, 1, 5)), 16);
    }
}
