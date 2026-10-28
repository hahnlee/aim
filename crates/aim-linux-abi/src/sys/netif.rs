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
//!   address (`dhcp`); its virtual router advertises the Mac's IPv6
//!   network (`addrconf`). Its hardware address is a fixed locally
//!   administered one.
//! - The Mac's network is part of the state: each process that reads or
//!   changes the devices compares it with the Mac's current one first, and
//!   the one that finds a change announces it (carrier lost or back, or
//!   lost and back for a different network, so that DHCP runs again).
//!   Processes listening to link changes look every few seconds and on
//!   each routing change (`uplink::watch`).
//! - Each device has its IPv6 configuration (`ipv6conf`) and its IPv6
//!   addresses and routes are autoconfigured (`addrconf`). There is no
//!   IPv4 routing table: those routes live in netd's per-network
//!   bookkeeping, and the host routes every socket.
//! - The state is a text file in the runtime directory
//!   (`<runtime>/net/links`), read and written under `flock`; each change
//!   is announced to the netlink groups it belongs to.

use std::io::{Read, Seek, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use super::addrconf::{self, Route6};
use super::ipv6conf::{self, Conf6, Scope};
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
const ENXIO: Errno = 6;
const EEXIST: Errno = 17;
const EACCES: Errno = 13;
const CAP_NET_ADMIN: u32 = 12;

pub const LO: u32 = 1;
pub const ETH0: u32 = 2;
/// `eth0`'s hardware address: locally administered, "aim".
pub const ETH0_MAC: [u8; 6] = [0x02, 0x61, 0x69, 0x6d, 0x00, 0x02];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addr {
    pub ip: IpAddr,
    pub prefix: u8,
    pub life: Life,
    /// The IPv6 flags the kernel keeps (`addrconf::IFA_F_*`).
    pub flags: u32,
    /// When a tentative address's DAD completes (CLOCK_MONOTONIC ms).
    pub dad_end: u64,
}

/// `INFINITY_LIFE_TIME`.
pub const INFINITY: u32 = u32::MAX;

/// An address's lifetimes, as `set_ifa_lifetime` keeps them: seconds valid
/// and preferred from `stamp`, INFINITY for none; `created` is when it was
/// added and `stamp` when its lifetimes were last set (CLOCK_MONOTONIC
/// milliseconds, the kernel's `ifa_cstamp` and `ifa_tstamp`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Life {
    pub valid: u32,
    pub preferred: u32,
    pub created: u64,
    pub stamp: u64,
    /// IFA_F_DEPRECATED: set with no preferred lifetime, or since it ran out.
    pub deprecated: bool,
}

impl Life {
    /// Lifetimes set now: valid must be nonzero and not below preferred
    /// (`rtm_to_ifaddr`, `inet6_rtm_newaddr`).
    pub fn new(valid: u32, preferred: u32) -> Result<Life, Errno> {
        if valid == 0 || preferred > valid {
            return Err(EINVAL);
        }
        let now = now_ms();
        Ok(Life {
            valid,
            preferred,
            created: now,
            stamp: now,
            deprecated: preferred == 0,
        })
    }

    /// IFA_F_PERMANENT: an infinite valid lifetime.
    pub fn permanent(&self) -> bool {
        self.valid == INFINITY
    }

    /// The valid and preferred seconds left at `now`, as `inet_fill_ifaddr`
    /// reports them: both INFINITY for a permanent address.
    pub fn left(&self, now: u64) -> (u32, u32) {
        if self.permanent() {
            return (INFINITY, INFINITY);
        }
        if self.preferred == INFINITY {
            return (self.valid, INFINITY);
        }
        let age = ((now.saturating_sub(self.stamp)) / 1000).min(u32::MAX as u64) as u32;
        (
            self.valid.saturating_sub(age),
            self.preferred.saturating_sub(age),
        )
    }
}

/// CLOCK_MONOTONIC in milliseconds, which every process of the boot shares.
pub fn now_ms() -> u64 {
    super::clock::Base::Monotonic.now() / 1_000_000
}

impl Addr {
    /// A permanent address, added now.
    pub fn new(ip: IpAddr, prefix: u8) -> Addr {
        let life = Life::new(INFINITY, INFINITY).unwrap();
        Addr {
            ip,
            prefix,
            life,
            flags: 0,
            dad_end: 0,
        }
    }

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
    /// Its IPv6 configuration (`ipv6conf`).
    pub conf6: Conf6,
    /// IPv6 is configured on it (`IF_READY`): its link became ready with
    /// IPv6 enabled, and it was not taken down since (`addrconf`).
    pub ready6: bool,
    /// Its `desync_factor` for temporary addresses, plus one (0: not made
    /// yet).
    pub desync6: u32,
}

/// The devices and the IPv6 configurations of `all` and `default`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Devices {
    pub links: Vec<Link>,
    pub all: Conf6,
    pub dflt: Conf6,
    /// The kernel's IPv6 routes (`addrconf`).
    pub routes6: Vec<Route6>,
    /// What `eth0`'s virtual router last advertised, and when it
    /// advertises next (CLOCK_MONOTONIC ms, 0: not scheduled).
    pub ra_seen: Option<uplink::Advert>,
    pub ra_next: u64,
}

impl Devices {
    /// The configuration of `scope` and the MTU its `mtu` defaults to.
    fn conf6(&mut self, scope: Scope) -> Result<(&mut Conf6, u32), Errno> {
        Ok(match scope {
            Scope::All => (&mut self.all, 0),
            Scope::Default => (&mut self.dflt, 0),
            Scope::Dev(i) => {
                let l = find(&mut self.links, i)?;
                (&mut l.conf6, l.mtu)
            }
        })
    }
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
    NewRoute(Route6),
    DelRoute(Route6),
    /// A router advertisement's option for userspace (RDNSS), from
    /// `router`.
    UserOpt {
        index: u32,
        opt: Vec<u8>,
        router: Ipv6Addr,
    },
    /// A prefix information option received (`inet6_prefix_notify`).
    Prefix {
        index: u32,
        prefix: Ipv6Addr,
        plen: u8,
        flags: u8,
        valid: u32,
        preferred: u32,
    },
}

/// The devices at boot: the loopback up with its addresses, `eth0` down
/// with none.
pub(super) fn boot_devices() -> Devices {
    let links = boot_links();
    Devices {
        routes6: addrconf::boot_routes(&links[0]),
        links,
        ..Devices::default()
    }
}

fn boot_links() -> Vec<Link> {
    vec![
        Link {
            index: LO,
            name: "lo".into(),
            flags: IFF_UP | IFF_LOOPBACK,
            mtu: 65536,
            mac: [0; 6],
            addrs: vec![
                Addr::new(Ipv4Addr::LOCALHOST.into(), 8),
                Addr::new(Ipv6Addr::LOCALHOST.into(), 128),
            ],
            carrier: true,
            conf6: Conf6::default(),
            ready6: true,
            desync6: 0,
        },
        Link {
            index: ETH0,
            name: "eth0".into(),
            flags: IFF_BROADCAST | IFF_MULTICAST,
            mtu: 1500,
            mac: ETH0_MAC,
            addrs: Vec::new(),
            carrier: false,
            conf6: Conf6::default(),
            ready6: false,
            desync6: 0,
        },
    ]
}

fn mac_text(m: &[u8; 6]) -> String {
    m.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn serialize(d: &Devices, up: &Option<Uplink>) -> String {
    let mut s = String::new();
    if let Some(u) = up {
        s += &format!(
            "uplink {} {}/{} {} {}\n",
            u.index, u.addr, u.prefix, u.gateway, u.mtu
        );
    }
    s += &d.all.lines("all");
    s += &d.dflt.lines("default");
    for l in &d.links {
        s += &format!(
            "link {} {} {:x} {} {}\n",
            l.index,
            l.name,
            l.flags,
            l.mtu,
            mac_text(&l.mac)
        );
        for a in &l.addrs {
            let t = &a.life;
            s += &format!(
                "addr {} {}/{} {} {} {} {} {} {:x} {}\n",
                l.index,
                a.ip,
                a.prefix,
                t.valid,
                t.preferred,
                t.created,
                t.stamp,
                t.deprecated as u8,
                a.flags,
                a.dad_end
            );
        }
        s += &l.conf6.lines(&l.index.to_string());
        if l.ready6 {
            s += &format!("ready6 {}\n", l.index);
        }
        if l.desync6 != 0 {
            s += &format!("desync6 {} {}\n", l.index, l.desync6);
        }
    }
    for r in &d.routes6 {
        s += &r.line();
    }
    if let Some(a) = &d.ra_seen {
        s += &a.line();
    }
    if d.ra_next != 0 {
        s += &format!("ra_next {}\n", d.ra_next);
    }
    s
}

/// The devices and the Mac's network they last saw.
fn parse(text: &str) -> Option<(Devices, Option<Uplink>)> {
    let mut d = Devices::default();
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
                d.links.push(Link {
                    index: index.parse().ok()?,
                    name: name.to_string(),
                    flags: u32::from_str_radix(flags, 16).ok()?,
                    mtu: mtu.parse().ok()?,
                    mac: m,
                    addrs: Vec::new(),
                    carrier: false,
                    conf6: Conf6::default(),
                    ready6: false,
                    desync6: 0,
                });
            }
            ["desync6", index, v] => {
                let index: u32 = index.parse().ok()?;
                d.links.iter_mut().find(|l| l.index == index)?.desync6 = v.parse().ok()?;
            }
            ["uplink6", rest @ ..] => d.ra_seen = Some(uplink::Advert::parse(rest)?),
            ["ra_next", t] => d.ra_next = t.parse().ok()?,
            ["ready6", index] => {
                let index: u32 = index.parse().ok()?;
                d.links.iter_mut().find(|l| l.index == index)?.ready6 = true;
            }
            ["route6", rest @ ..] => d.routes6.push(Route6::parse(rest)?),
            ["conf6", scope, key, value] => {
                let v = value.parse().ok()?;
                conf_of(&mut d, scope)?.put(key, v);
            }
            ["secret6", scope, secret] => {
                conf_of(&mut d, scope)?.secret = Some(secret.parse().ok()?)
            }
            [
                "addr",
                index,
                addr,
                valid,
                preferred,
                created,
                stamp,
                deprecated,
                flags,
                dad_end,
            ] => {
                let (ip, prefix) = addr.split_once('/')?;
                let index: u32 = index.parse().ok()?;
                d.links
                    .iter_mut()
                    .find(|l| l.index == index)?
                    .addrs
                    .push(Addr {
                        ip: ip.parse().ok()?,
                        prefix: prefix.parse().ok()?,
                        life: Life {
                            valid: valid.parse().ok()?,
                            preferred: preferred.parse().ok()?,
                            created: created.parse().ok()?,
                            stamp: stamp.parse().ok()?,
                            deprecated: *deprecated == "1",
                        },
                        flags: u32::from_str_radix(flags, 16).ok()?,
                        dad_end: dad_end.parse().ok()?,
                    });
            }
            _ => return None,
        }
    }
    for l in &mut d.links {
        l.carrier = l.loopback() || up.is_some();
    }
    Some((d, up))
}

/// The configuration a state line's scope names.
fn conf_of<'a>(d: &'a mut Devices, scope: &str) -> Option<&'a mut Conf6> {
    match scope {
        "all" => Some(&mut d.all),
        "default" => Some(&mut d.dflt),
        i => {
            let i: u32 = i.parse().ok()?;
            d.links
                .iter_mut()
                .find(|l| l.index == i)
                .map(|l| &mut l.conf6)
        }
    }
}

/// `check_lifetime` at `now`: an address whose valid lifetime ran out
/// goes, and one whose preferred lifetime ran out is deprecated, each
/// announced. Lifetimes are looked at whenever the device state is, which
/// a process following the Mac's network does every
/// `aim_uplink::WATCH_PERIOD`.
fn expire(d: &mut Devices, now: u64, ev: &mut Vec<Event>) {
    for li in 0..d.links.len() {
        let mut i = 0;
        while i < d.links[li].addrs.len() {
            let a = d.links[li].addrs[i];
            if a.life.permanent() {
                i += 1;
                continue;
            }
            let (valid, preferred) = a.life.left(now);
            if valid == 0 {
                d.links[li].addrs.remove(i);
                let l = d.links[li].clone();
                ev.push(Event::DelAddr(l.clone(), a));
                addrconf::removed(d, &l, &a, ev);
                continue;
            }
            if preferred == 0 && !a.life.deprecated {
                d.links[li].addrs[i].life.deprecated = true;
                ev.push(Event::NewAddr(d.links[li].clone(), d.links[li].addrs[i]));
            }
            i += 1;
        }
    }
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
fn transact<R>(f: impl FnOnce(&mut Devices, &mut Vec<Event>) -> R) -> R {
    // The locked file must not reach a fork child (`spawn::own_fds`).
    let _own = super::fork::spawn::own_fds();
    let path = kernel_dir("net").join("links");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path);
    let Ok(mut file) = file else {
        // No shared state to be had: this process's own view.
        let mut d = boot_devices();
        follow_uplink(&mut d.links, &None, &uplink::current(), &mut Vec::new());
        return f(&mut d, &mut Vec::new());
    };
    // SAFETY: locking our open file; released when it closes.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    let mut text = String::new();
    let _ = file.read_to_string(&mut text);
    let (mut d, was) = parse(&text)
        .filter(|(d, _)| !d.links.is_empty())
        .unwrap_or_else(|| (boot_devices(), None));
    let mut events = Vec::new();
    let now = uplink::current();
    if follow_uplink(&mut d.links, &was, &now, &mut events) {
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
    let t = now_ms();
    expire(&mut d, t, &mut events);
    let solicited = addrconf::reconcile(&mut d, t, &mut events);
    addrconf::advertise(&mut d, uplink::advert, &solicited, t, &mut events);
    let r = f(&mut d, &mut events);
    let solicited = addrconf::reconcile(&mut d, t, &mut events);
    addrconf::advertise(&mut d, uplink::advert, &solicited, t, &mut events);
    let new = serialize(&d, &now);
    if new != text {
        let _ = file.set_len(0);
        let _ = file.rewind();
        let _ = file.write_all(new.as_bytes());
    }
    netlink::announce(&events);
    r
}

pub fn links() -> Vec<Link> {
    transact(|d, _| d.links.clone())
}

/// `/proc/net/dev`, as `dev_seq_show`/`dev_seq_printf_stats` in Linux's
/// net/core/net-procfs.c. These are the backing host devices' totals,
/// including host traffic; guest-only eBPF interface accounting is in `net::traffic`.
pub fn proc_dev() -> Result<Vec<u8>, Errno> {
    let links = links();
    let uplink = uplink::current().map_or(0, |u| u.index);
    let lo = uplink::host_index("lo0");
    let mut out = String::from(concat!(
        "Inter-|   Receive                                                |  Transmit\n",
        " face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n",
    ));
    for l in links {
        let index = if l.loopback() { lo } else { uplink };
        let counters = host_counters(index)?;
        out.push_str(&dev_line(&l.name, counters));
    }
    Ok(out.into_bytes())
}

/// NET_RT_IFLIST2 supplies Darwin's 64-bit interface counters. A device
/// without a backing interface has none; other sysctl errors reach the reader.
fn host_counters(index: u32) -> Result<[u64; 16], Errno> {
    if index == 0 {
        return Ok([0; 16]);
    }
    let mut mib = [
        libc::CTL_NET,
        libc::PF_ROUTE,
        0,
        0,
        libc::NET_RT_IFLIST2,
        index as i32,
    ];
    let mut table = Vec::new();
    loop {
        let mut len = 0;
        // SAFETY: sizing call, with no output buffer.
        if unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                6,
                std::ptr::null_mut(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        } < 0
        {
            let error = crate::errno::last();
            return if error == ENXIO || error == crate::errno::ENOENT {
                Ok([0; 16])
            } else {
                Err(error)
            };
        }
        table.resize(len, 0u8);
        // SAFETY: sysctl writes at most len bytes into our table.
        if unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                6,
                table.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        } == 0
        {
            table.truncate(len);
            return counters_from_iflist(&table, index);
        }
        let error = crate::errno::last();
        if error != crate::errno::ENOMEM {
            return if error == ENXIO || error == crate::errno::ENOENT {
                Ok([0; 16])
            } else {
                Err(error)
            };
        }
        // Addresses may have been added between sizing and reading.
    }
}

fn counters_from_iflist(mut table: &[u8], index: u32) -> Result<[u64; 16], Errno> {
    while !table.is_empty() {
        let prefix = table.get(..4).ok_or(crate::errno::EIO)?;
        let len = u16::from_ne_bytes([prefix[0], prefix[1]]) as usize;
        if len < 4 || len > table.len() {
            return Err(crate::errno::EIO);
        }
        if prefix[2] as i32 == libc::RTM_VERSION && prefix[3] as i32 == libc::RTM_IFINFO2 {
            if len < std::mem::size_of::<libc::if_msghdr2>() {
                return Err(crate::errno::EIO);
            }
            // SAFETY: the whole header is inside this routing message.
            let h = unsafe { table.as_ptr().cast::<libc::if_msghdr2>().read_unaligned() };
            if h.ifm_index as u32 == index {
                return Ok(dev_counters(&h));
            }
        }
        table = &table[len..];
    }
    Ok([0; 16])
}

fn dev_counters(data: &libc::if_msghdr2) -> [u64; 16] {
    let d = data.ifm_data;
    [
        d.ifi_ibytes,
        d.ifi_ipackets,
        d.ifi_ierrors,
        d.ifi_iqdrops,
        0,
        0,
        0,
        d.ifi_imcasts,
        d.ifi_obytes,
        d.ifi_opackets,
        d.ifi_oerrors,
        data.ifm_snd_drops as u32 as u64,
        0,
        d.ifi_collisions,
        0,
        0,
    ]
}

fn dev_line(name: &str, c: [u64; 16]) -> String {
    format!(
        "{name:>6}: {:7} {:7} {:4} {:4} {:4} {:5} {:10} {:9} {:8} {:7} {:4} {:4} {:4} {:5} {:7} {:10}\n",
        c[0],
        c[1],
        c[2],
        c[3],
        c[4],
        c[5],
        c[6],
        c[7],
        c[8],
        c[9],
        c[10],
        c[11],
        c[12],
        c[13],
        c[14],
        c[15],
    )
}

fn state_path() -> &'static std::path::Path {
    static PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    PATH.get_or_init(|| kernel_dir("net").join("links"))
}

/// The device state as it was last written, read without changing it.
pub fn peek() -> Option<Devices> {
    let _own = super::fork::spawn::own_fds();
    let mut file = std::fs::File::open(state_path()).ok()?;
    // SAFETY: a shared lock on our open file; released when it closes.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) };
    let mut text = String::new();
    file.read_to_string(&mut text).ok()?;
    parse(&text).map(|(d, _)| d)
}

/// When the device state was last written, and its size: it changed
/// when this does.
pub fn state_stamp() -> Option<(i64, i64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(state_path()).ok()?;
    Some((m.mtime(), m.mtime_nsec(), m.len()))
}

/// The kernel's IPv6 routes.
pub fn routes6() -> Vec<Route6> {
    transact(|d, _| d.routes6.clone())
}

/// The IPv6 configuration of `scope`, and the MTU a device's `mtu`
/// defaults to.
pub fn conf6(scope: Scope) -> Result<(Conf6, u32), Errno> {
    transact(|d, _| d.conf6(scope).map(|(c, mtu)| (c.clone(), mtu)))
}

/// A write of `name` in `scope`'s IPv6 configuration, as its
/// `addrconf_sysctl_*` handler makes it: `all`'s `disable_ipv6`,
/// `forwarding` and `addr_gen_mode` go to `default` and every device too.
pub fn set_conf6(scope: Scope, name: &str, value: i32) -> Result<(), Errno> {
    transact(|d, _| {
        d.conf6(scope)?.0.put(name, value);
        if scope == Scope::All && matches!(name, "disable_ipv6" | "forwarding" | "addr_gen_mode") {
            d.dflt.put(name, value);
            for l in &mut d.links {
                l.conf6.put(name, value);
            }
        }
        Ok(())
    })
}

/// `stable_secret`'s write: the secret, and the stable privacy mode for
/// the device (for `default`, for every device).
pub fn set_secret6(scope: Scope, secret: Ipv6Addr) -> Result<(), Errno> {
    transact(|d, _| {
        if scope == Scope::Default {
            d.dflt.secret = Some(secret);
            for l in &mut d.links {
                l.conf6
                    .put("addr_gen_mode", ipv6conf::ADDR_GEN_STABLE_PRIVACY);
            }
        } else {
            let c = d.conf6(scope)?.0;
            c.secret = Some(secret);
            c.put("addr_gen_mode", ipv6conf::ADDR_GEN_STABLE_PRIVACY);
        }
        Ok(())
    })
}

/// Follow the Mac's network (`uplink::watch`): the shared device state,
/// and this process's sockets bound to a device when the Mac's interface
/// behind it changed.
pub fn refresh() {
    static HOST_INDEX: AtomicU32 = AtomicU32::new(0);
    transact(|_, _| ());
    let index = uplink::current().map_or(0, |u| u.index);
    if HOST_INDEX.swap(index, Ordering::Relaxed) != index {
        super::net::rebind_devices(index);
    }
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
    transact(|d, ev| {
        let l = find(&mut d.links, index)?;
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
/// An IPv6 address takes the flags userspace may give it, runs DAD and
/// gets its prefix route (`inet6_addr_add`); IPv6 disabled on the device
/// is EACCES.
pub fn add_addr(index: u32, a: Addr, replace: bool) -> Result<(), Errno> {
    let max = if a.ip.is_ipv4() { 32 } else { 128 };
    if a.prefix > max {
        return Err(EINVAL);
    }
    transact(|d, ev| {
        let i = d
            .links
            .iter()
            .position(|l| l.index == index)
            .ok_or(ENODEV)?;
        let l = &mut d.links[i];
        if let Some(x) = l.addrs.iter_mut().find(|x| x.ip == a.ip) {
            if !replace {
                return Err(EEXIST);
            }
            // `inet6_addr_modify`: new lifetimes and flags; it keeps when
            // it was added (`ifa_cstamp`) and its DAD state.
            let created = x.life.created;
            let kept = x.flags & (addrconf::IFA_F_TENTATIVE | addrconf::IFA_F_OPTIMISTIC);
            *x = Addr {
                flags: (a.flags & addrconf::IFA_F_USER) | kept,
                dad_end: x.dad_end,
                ..a
            };
            x.life.created = created;
            let x = *x;
            ev.push(Event::NewAddr(l.clone(), x));
            return Ok(());
        }
        if a.ip.is_ipv4() {
            l.addrs.push(a);
            ev.push(Event::NewAddr(l.clone(), a));
            return Ok(());
        }
        if l.conf6.get("disable_ipv6", Scope::Dev(l.index), l.mtu) != 0 {
            return Err(EACCES);
        }
        let a = Addr {
            flags: a.flags & addrconf::IFA_F_USER,
            ..a
        };
        addrconf::add(d, i, a, now_ms(), ev);
        Ok(())
    })
}

/// Remove the link's address `ip` (with `prefix`, when given), and an
/// IPv6 address's routes.
pub fn del_addr(index: u32, ip: IpAddr, prefix: Option<u8>) -> Result<(), Errno> {
    transact(|d, ev| {
        let l = find(&mut d.links, index)?;
        let i = l
            .addrs
            .iter()
            .position(|x| x.ip == ip && prefix.is_none_or(|p| p == x.prefix))
            .ok_or(EADDRNOTAVAIL)?;
        let a = l.addrs.remove(i);
        let l = l.clone();
        ev.push(Event::DelAddr(l.clone(), a));
        addrconf::removed(d, &l, &a, ev);
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
    transact(|d, ev| {
        let l = find(&mut d.links, index)?;
        if let Some(i) = l.primary_v4() {
            if l.addrs[i].ip == IpAddr::V4(ip) {
                return Ok(());
            }
            let old = l.addrs.remove(i);
            ev.push(Event::DelAddr(l.clone(), old));
        }
        if !ip.is_unspecified() {
            let a = Addr::new(ip.into(), classful(ip));
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
    transact(|d, ev| {
        let l = find(&mut d.links, index)?;
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
    host_index_with(name, uplink::current().map_or(0, |u| u.index))
}

/// The same, with the Mac's interface `uplink` (0 for none) behind the
/// guest's network devices.
pub fn host_index_with(name: &str, uplink: u32) -> Result<u32, Errno> {
    if name.is_empty() {
        return Ok(0);
    }
    let l = by_name(name).ok_or(ENODEV)?;
    Ok(if l.loopback() {
        uplink::host_index("lo0")
    } else {
        uplink
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
    user_try!(unsafe {
        crate::sys::user_memory::copy_bytes(arg as *const u8, req.as_mut_ptr() as *mut u8, IFREQ)
    });
    f(&mut req)?;
    // SAFETY: as above.
    user_try!(unsafe {
        crate::sys::user_memory::copy_bytes(req.as_ptr() as *const u8, arg as *mut u8, IFREQ)
    });
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
                _ => Addr::new(a.into(), p)
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
    let (len, buf) = { (user_read!(arg, i32), user_read!(arg + 8, u64)) };
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
        user_try!(unsafe {
            crate::sys::user_memory::copy_bytes(out.as_ptr() as *const u8, buf as *mut u8, fit)
        });
        fit
    };
    // SAFETY: as above.
    user_write!(arg, i32, n as i32);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_dev_maps_host_counters_without_truncation() {
        let mut data: libc::if_msghdr2 = unsafe { std::mem::zeroed() };
        data.ifm_data.ifi_ibytes = 1 << 40;
        data.ifm_data.ifi_ipackets = 2;
        data.ifm_data.ifi_ierrors = 3;
        data.ifm_data.ifi_iqdrops = 4;
        data.ifm_data.ifi_imcasts = 8;
        data.ifm_data.ifi_obytes = (1 << 40) + 9;
        data.ifm_data.ifi_opackets = 10;
        data.ifm_data.ifi_oerrors = 11;
        data.ifm_snd_drops = 12;
        data.ifm_data.ifi_collisions = 14;
        data.ifm_msglen = std::mem::size_of_val(&data) as u16;
        data.ifm_version = libc::RTM_VERSION as u8;
        data.ifm_type = libc::RTM_IFINFO2 as u8;
        data.ifm_index = 1;
        // SAFETY: a byte view of our initialized routing-message header.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&data as *const libc::if_msghdr2).cast::<u8>(),
                std::mem::size_of_val(&data),
            )
        };
        assert_eq!(counters_from_iflist(bytes, 1), Ok(dev_counters(&data)));
        assert_eq!(counters_from_iflist(bytes, 2), Ok([0; 16]));
        assert_eq!(counters_from_iflist(&bytes[..3], 1), Err(crate::errno::EIO));
        assert_eq!(
            counters_from_iflist(&bytes[..bytes.len() - 1], 1),
            Err(crate::errno::EIO)
        );
        assert_eq!(counters_from_iflist(&[0; 4], 1), Err(crate::errno::EIO));
        assert_eq!(
            dev_counters(&data),
            [
                1 << 40,
                2,
                3,
                4,
                0,
                0,
                0,
                8,
                (1 << 40) + 9,
                10,
                11,
                12,
                0,
                14,
                0,
                0
            ]
        );
        assert_eq!(host_counters(0), Ok([0; 16]));
        assert_eq!(host_counters(u16::MAX as u32), Ok([0; 16]));
    }

    #[test]
    fn proc_dev_has_linux_field_widths() {
        assert_eq!(
            dev_line("lo", [0; 16]),
            "    lo:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0\n"
        );
        // Widths are minima, including for names and 64-bit totals.
        let line = dev_line("long-interface", [u64::MAX; 16]);
        assert!(line.starts_with("long-interface: "));
        assert_eq!(line.split_whitespace().count(), 17);
        assert_eq!(line.matches("18446744073709551615").count(), 16);
    }

    #[test]
    fn proc_dev_reads_live_loopback_counters() {
        let lo = uplink::host_index("lo0");
        assert_ne!(lo, 0);
        let receiver = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let sender = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let before = host_counters(lo).unwrap();
        // Darwin may round the exported byte totals to KiB.
        sender
            .send_to(&[0; 4096], receiver.local_addr().unwrap())
            .unwrap();
        assert_eq!(receiver.recv(&mut [0; 4096]).unwrap(), 4096);
        let after = host_counters(lo).unwrap();
        assert!(after[0] >= before[0] + 4096 && after[8] >= before[8] + 4096);
        assert!(after[1] > before[1] && after[9] > before[9]);
    }

    #[test]
    fn state_round_trips() {
        let mut d = boot_devices();
        d.links[1].flags |= IFF_UP;
        d.links[1]
            .addrs
            .push(Addr::new("192.168.1.20".parse().unwrap(), 24));
        d.links[1].conf6.put("use_tempaddr", 2);
        d.links[1].conf6.secret = Some("2001:db8::7".parse().unwrap());
        d.all.put("forwarding", 1);
        d.dflt.put("accept_ra", 2);
        assert_eq!(parse(&serialize(&d, &None)).unwrap(), (d.clone(), None));
        let up = Some(Uplink {
            index: 4,
            addr: "192.168.1.20".parse().unwrap(),
            prefix: 24,
            gateway: "192.168.1.1".parse().unwrap(),
            mtu: 1500,
        });
        d.links[1].carrier = true;
        assert_eq!(parse(&serialize(&d, &up)).unwrap(), (d, up));
    }

    /// `set_ifa_lifetime` and `check_lifetime`: a lease's address is
    /// deprecated when its preferred lifetime runs out and goes when its
    /// valid one does; the state keeps them.
    #[test]
    fn address_lifetimes_count_down_and_expire() {
        assert_eq!(Life::new(0, 0), Err(EINVAL));
        assert_eq!(Life::new(60, 120), Err(EINVAL));
        assert!(Life::new(INFINITY, 0).unwrap().deprecated);
        let mut a = Addr::new("192.168.1.20".parse().unwrap(), 24);
        a.life = Life::new(3600, 1800).unwrap();
        let t = a.life.stamp;
        assert!(!a.life.permanent());
        assert_eq!(a.life.left(t + 600_000), (3000, 1200));
        assert_eq!(a.life.left(t + 2_000_000), (1600, 0));
        let mut d = boot_devices();
        d.links[1].addrs.push(a);
        assert_eq!(parse(&serialize(&d, &None)).unwrap().0, d);
        let mut ev = Vec::new();
        expire(&mut d, t + 1_000_000, &mut ev);
        assert!(ev.is_empty());
        expire(&mut d, t + 1_800_000, &mut ev);
        assert!(matches!(&ev[..], [Event::NewAddr(_, x)] if x.life.deprecated));
        expire(&mut d, t + 1_900_000, &mut ev);
        assert_eq!(ev.len(), 1);
        expire(&mut d, t + 3_600_000, &mut ev);
        assert!(matches!(&ev[1..], [Event::DelAddr(_, x)] if x.ip == a.ip));
        assert!(d.links[1].addrs.is_empty());
        // The loopback's are permanent.
        assert_eq!(d.links[0].addrs.len(), 2);
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
        let a = |s: &str, p| Addr::new(s.parse().unwrap(), p);
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
