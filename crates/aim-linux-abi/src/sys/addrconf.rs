//! IPv6 address autoconfiguration on the devices (Linux 6.6
//! `net/ipv6/addrconf.c`, docs/network.md): a device whose link becomes
//! ready with IPv6 enabled gets a link-local address by its
//! `addr_gen_mode` (EUI-64 from its hardware address, or RFC 7217 stable
//! privacy from `stable_secret`) and the fe80::/64 route; addresses run
//! duplicate address detection; a device taken down or with IPv6 disabled
//! loses its IPv6 addresses and routes. The routes are the kernel's IPv6
//! table as `RTM_GETROUTE` dumps it, in the tables Android's
//! `accept_ra_rt_table` puts them; nothing routes by them (the host routes
//! every socket).

use std::net::{IpAddr, Ipv6Addr};

use super::ipv6conf::{self, Scope};
use super::netif::{Addr, Devices, Event, IFF_UP, INFINITY, Life, Link};

/// `IFA_F_*`.
pub const IFA_F_TEMPORARY: u32 = 0x01;
pub const IFA_F_NODAD: u32 = 0x02;
pub const IFA_F_OPTIMISTIC: u32 = 0x04;
pub const IFA_F_TENTATIVE: u32 = 0x40;
pub const IFA_F_MANAGETEMPADDR: u32 = 0x100;
pub const IFA_F_NOPREFIXROUTE: u32 = 0x200;
pub const IFA_F_STABLE_PRIVACY: u32 = 0x800;
/// The flags userspace may give an IPv6 address (`inet6_rtm_newaddr`).
pub const IFA_F_USER: u32 =
    IFA_F_NODAD | IFA_F_OPTIMISTIC | IFA_F_NOPREFIXROUTE | IFA_F_MANAGETEMPADDR | 0x10 | 0x400;

/// `RTPROT_KERNEL`, `RTPROT_RA`.
pub const RTPROT_KERNEL: u8 = 2;
pub const RTPROT_RA: u8 = 9;
/// `RTN_UNICAST`, `RTN_LOCAL`, `RTN_MULTICAST`.
pub const RTN_UNICAST: u8 = 1;
pub const RTN_LOCAL: u8 = 2;
pub const RTN_MULTICAST: u8 = 5;
/// `RT_TABLE_MAIN`, `RT_TABLE_LOCAL`.
pub const RT_TABLE_MAIN: u32 = 254;
pub const RT_TABLE_LOCAL: u32 = 255;
/// `IP6_RT_PRIO_ADDRCONF`.
const PRIO_ADDRCONF: u32 = 256;
/// `ndisc`'s RETRANS_TIMER: one DAD probe a second.
const RETRANS_MS: u64 = 1000;
/// `idgen_retries`.
const IDGEN_RETRIES: u8 = 3;

/// A route of the kernel's IPv6 table (`fib6_info`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route6 {
    pub index: u32,
    pub dst: Ipv6Addr,
    pub plen: u8,
    pub gateway: Option<Ipv6Addr>,
    pub table: u32,
    pub proto: u8,
    pub kind: u8,
    pub metric: u32,
    /// When it expires (CLOCK_MONOTONIC ms), 0 for never.
    pub expires: u64,
    /// RTAX_HOPLIMIT, 0 for none.
    pub hoplimit: u8,
}

impl Route6 {
    /// The same route (`fib6_info` match): its device, prefix, gateway,
    /// table, type and metric.
    pub fn same(&self, o: &Route6) -> bool {
        (
            self.index,
            self.dst,
            self.plen,
            self.gateway,
            self.table,
            self.kind,
            self.metric,
        ) == (o.index, o.dst, o.plen, o.gateway, o.table, o.kind, o.metric)
    }

    pub fn line(&self) -> String {
        format!(
            "route6 {} {}/{} {} {} {} {} {} {} {}\n",
            self.index,
            self.dst,
            self.plen,
            self.gateway.map_or("-".into(), |g| g.to_string()),
            self.table,
            self.proto,
            self.kind,
            self.metric,
            self.expires,
            self.hoplimit
        )
    }

    pub fn parse(w: &[&str]) -> Option<Route6> {
        let [
            index,
            dst,
            gateway,
            table,
            proto,
            kind,
            metric,
            expires,
            hoplimit,
        ] = w
        else {
            return None;
        };
        let (dst, plen) = dst.split_once('/')?;
        Some(Route6 {
            index: index.parse().ok()?,
            dst: dst.parse().ok()?,
            plen: plen.parse().ok()?,
            gateway: match *gateway {
                "-" => None,
                g => Some(g.parse().ok()?),
            },
            table: table.parse().ok()?,
            proto: proto.parse().ok()?,
            kind: kind.parse().ok()?,
            metric: metric.parse().ok()?,
            expires: expires.parse().ok()?,
            hoplimit: hoplimit.parse().ok()?,
        })
    }
}

/// `addrconf_rt_table` (Android): a device's `accept_ra_rt_table` puts
/// its autoconfigured routes in its own table (negative: an offset from
/// its index), else `default`.
pub fn rt_table(l: &Link, default: u32) -> u32 {
    match l
        .conf6
        .get("accept_ra_rt_table", Scope::Dev(l.index), l.mtu)
    {
        0 => default,
        t if t > 0 => t as u32,
        t => l.index + t.unsigned_abs(),
    }
}

/// `masked`: `a` with the bits after `plen` cleared.
pub fn prefix_of(a: Ipv6Addr, plen: u8) -> Ipv6Addr {
    let bits = u128::from(a);
    let mask = u128::MAX.checked_shl(128 - plen as u32).unwrap_or(0);
    Ipv6Addr::from(bits & mask)
}

/// `addrconf_ifid_eui48`: the interface identifier of an Ethernet address.
pub fn eui64(mac: &[u8; 6]) -> [u8; 8] {
    [
        mac[0] ^ 2,
        mac[1],
        mac[2],
        0xff,
        0xfe,
        mac[3],
        mac[4],
        mac[5],
    ]
}

/// `lib/crypto/sha1.c`'s `sha1_transform`: SHA-1's compression of one
/// 64-byte block from the initial state, without padding.
fn sha1_transform(block: &[u8; 64]) -> [u32; 5] {
    let mut w = [0u32; 80];
    for (i, c) in block.chunks(4).enumerate() {
        w[i] = u32::from_be_bytes(c.try_into().unwrap());
    }
    for i in 16..80 {
        w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
    }
    let h = [
        0x67452301u32,
        0xefcdab89,
        0x98badcfe,
        0x10325476,
        0xc3d2e1f0,
    ];
    let [mut a, mut b, mut c, mut d, mut e] = h;
    for (i, wi) in w.iter().enumerate() {
        let (f, k) = match i {
            0..=19 => ((b & c) | (!b & d), 0x5a827999),
            20..=39 => (b ^ c ^ d, 0x6ed9eba1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
            _ => (b ^ c ^ d, 0xca62c1d6),
        };
        let t = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(*wi);
        (e, d, c, b, a) = (d, c, b.rotate_left(30), a, t);
    }
    [
        h[0].wrapping_add(a),
        h[1].wrapping_add(b),
        h[2].wrapping_add(c),
        h[3].wrapping_add(d),
        h[4].wrapping_add(e),
    ]
}

/// `ipv6_reserved_interfaceid`: the subnet-router anycast identifier,
/// IANA's reserved ones and the reserved anycast ones (RFC 5453).
fn reserved_iid(iid: [u8; 8]) -> bool {
    let hi = u32::from_be_bytes(iid[..4].try_into().unwrap());
    let lo = u32::from_be_bytes(iid[4..].try_into().unwrap());
    (hi | lo) == 0
        || (hi == 0x0200_5eff && lo & 0xfe00_0000 == 0xfe00_0000)
        || (hi == 0xfdff_ffff && lo & 0xffff_ff80 == 0xffff_ff80)
}

/// `ipv6_generate_stable_address`: the RFC 7217 address in `prefix` (its
/// first 64 bits) for `mac` and `secret`, or None after `idgen_retries`
/// reserved identifiers.
pub fn stable_address(
    prefix: Ipv6Addr,
    mac: &[u8; 6],
    secret: Ipv6Addr,
    dad_count: u8,
) -> Option<Ipv6Addr> {
    let mut count = dad_count;
    loop {
        // secret, prefix[2], hwaddr[MAX_ADDR_LEN], dad_count, packed.
        let mut block = [0u8; 64];
        block[..16].copy_from_slice(&secret.octets());
        block[16..24].copy_from_slice(&prefix.octets()[..8]);
        block[24..30].copy_from_slice(mac);
        block[56] = count;
        let digest = sha1_transform(&block);
        // s6_addr32[2] and [3] take digest[0] and [1] as they are in
        // memory (little-endian on arm64).
        let mut iid = [0u8; 8];
        iid[..4].copy_from_slice(&digest[0].to_le_bytes());
        iid[4..].copy_from_slice(&digest[1].to_le_bytes());
        if !reserved_iid(iid) {
            let mut a = prefix.octets();
            a[8..].copy_from_slice(&iid);
            return Some(Ipv6Addr::from(a));
        }
        count += 1;
        if count > dad_count + IDGEN_RETRIES {
            return None;
        }
    }
}

/// The interface identifier `addr_gen_mode` gives `l` in `prefix`:
/// None for IN6_ADDR_GEN_MODE_NONE, or stable privacy without a secret.
/// RANDOM mode makes the device's secret first, as
/// `ipv6_gen_mode_random_init` does.
pub fn address_in(d: &mut Devices, i: usize, prefix: Ipv6Addr) -> Option<(Ipv6Addr, u32)> {
    let l = &d.links[i];
    let mode = l.conf6.get("addr_gen_mode", Scope::Dev(l.index), l.mtu);
    let mut a = prefix.octets();
    match mode {
        ipv6conf::ADDR_GEN_EUI64 => {
            a[8..].copy_from_slice(&eui64(&l.mac));
            Some((Ipv6Addr::from(a), 0))
        }
        ipv6conf::ADDR_GEN_STABLE_PRIVACY | ipv6conf::ADDR_GEN_RANDOM => {
            if mode == ipv6conf::ADDR_GEN_RANDOM && d.links[i].conf6.secret.is_none() {
                d.links[i].conf6.secret = Some(random_secret());
            }
            let l = &d.links[i];
            let secret = l.conf6.secret.or(d.dflt.secret)?;
            stable_address(prefix, &l.mac, secret, 0).map(|a| (a, IFA_F_STABLE_PRIVACY))
        }
        _ => None,
    }
}

pub(super) fn random_secret() -> Ipv6Addr {
    let mut b = [0u8; 16];
    // SAFETY: filling our buffer.
    unsafe { libc::arc4random_buf(b.as_mut_ptr().cast(), b.len()) };
    Ipv6Addr::from(b)
}

fn conf(l: &Link, name: &str) -> i32 {
    l.conf6.get(name, Scope::Dev(l.index), l.mtu)
}

/// Whether IPv6 runs on the link: up, with carrier, not disabled
/// (`addrconf_link_ready`).
fn link_ready(l: &Link) -> bool {
    l.flags & IFF_UP != 0 && l.carrier && conf(l, "disable_ipv6") == 0
}

/// The time duplicate address detection of a new address on `l` takes:
/// none without `accept_dad` or `dad_transmits`, on the loopback, or for
/// IFA_F_NODAD.
fn dad_ms(d: &Devices, l: &Link, flags: u32) -> u64 {
    let accept = conf(l, "accept_dad").max(d.all.get("accept_dad", Scope::All, 0));
    let transmits = conf(l, "dad_transmits");
    if l.loopback() || flags & IFA_F_NODAD != 0 || accept < 1 || transmits <= 0 {
        0
    } else {
        transmits as u64 * RETRANS_MS
    }
}

/// `ipv6_add_addr` and `addrconf_dad_start`: the address, tentative
/// while DAD runs (optimistic when asked and `optimistic_dad` allows),
/// with its prefix route unless IFA_F_NOPREFIXROUTE, and its local route
/// once usable.
pub fn add(d: &mut Devices, i: usize, mut a: Addr, now: u64, ev: &mut Vec<Event>) {
    let IpAddr::V6(ip) = a.ip else { return };
    let dad = dad_ms(d, &d.links[i], a.flags);
    if dad > 0 {
        a.flags |= IFA_F_TENTATIVE;
        a.dad_end = now + dad;
        let l = &d.links[i];
        let optimistic =
            conf(l, "optimistic_dad") != 0 && d.all.get("forwarding", Scope::All, 0) == 0;
        if !optimistic {
            a.flags &= !IFA_F_OPTIMISTIC;
        }
    } else {
        a.flags &= !(IFA_F_TENTATIVE | IFA_F_OPTIMISTIC);
    }
    let l = &mut d.links[i];
    match l.addrs.iter_mut().find(|x| x.ip == a.ip) {
        Some(x) => *x = a,
        None => l.addrs.push(a),
    }
    let l = d.links[i].clone();
    ev.push(Event::NewAddr(l.clone(), a));
    if a.flags & IFA_F_NOPREFIXROUTE == 0 && a.prefix < 128 {
        let table = rt_table(&l, RT_TABLE_MAIN);
        add_route(
            d,
            unicast(&l, prefix_of(ip, a.prefix), a.prefix, table, 0),
            ev,
        );
    }
    if a.flags & IFA_F_TENTATIVE == 0 || a.flags & IFA_F_OPTIMISTIC != 0 {
        add_route(d, local(&l, ip), ev);
    }
}

/// A prefix route of `l` (proto kernel, `IP6_RT_PRIO_ADDRCONF`).
pub fn unicast(l: &Link, dst: Ipv6Addr, plen: u8, table: u32, expires: u64) -> Route6 {
    Route6 {
        index: l.index,
        dst,
        plen,
        gateway: None,
        table,
        proto: RTPROT_KERNEL,
        kind: RTN_UNICAST,
        metric: PRIO_ADDRCONF,
        expires,
        hoplimit: 0,
    }
}

/// The local table's route of an address of `l`.
fn local(l: &Link, ip: Ipv6Addr) -> Route6 {
    Route6 {
        index: l.index,
        dst: ip,
        plen: 128,
        gateway: None,
        table: RT_TABLE_LOCAL,
        proto: RTPROT_KERNEL,
        kind: RTN_LOCAL,
        metric: 0,
        expires: 0,
        hoplimit: 0,
    }
}

/// Add `r`, or refresh the same route's expiry and hop limit; announced.
pub fn add_route(d: &mut Devices, r: Route6, ev: &mut Vec<Event>) {
    match d.routes6.iter_mut().find(|x| x.same(&r)) {
        Some(x) if *x == r => {}
        Some(x) => {
            *x = r;
            ev.push(Event::NewRoute(r));
        }
        None => {
            d.routes6.push(r);
            ev.push(Event::NewRoute(r));
        }
    }
}

pub fn del_route(d: &mut Devices, r: &Route6, ev: &mut Vec<Event>) {
    if let Some(i) = d.routes6.iter().position(|x| x.same(r)) {
        ev.push(Event::DelRoute(d.routes6.remove(i)));
    }
}

/// An address removed (`ipv6_del_addr`): its local route, and its prefix
/// route when no other address of the device has the prefix.
pub fn removed(d: &mut Devices, l: &Link, a: &Addr, ev: &mut Vec<Event>) {
    let IpAddr::V6(ip) = a.ip else { return };
    del_route(d, &local(l, ip), ev);
    let p = prefix_of(ip, a.prefix);
    let shared = l.addrs.iter().any(|x| {
        x.ip != a.ip
            && matches!(x.ip, IpAddr::V6(o) if prefix_of(o, a.prefix) == p)
            && x.prefix == a.prefix
    });
    if !shared && a.prefix < 128 && a.flags & IFA_F_NOPREFIXROUTE == 0 {
        let routes: Vec<Route6> = d
            .routes6
            .iter()
            .filter(|r| {
                r.index == l.index
                    && r.kind == RTN_UNICAST
                    && r.gateway.is_none()
                    && r.dst == p
                    && r.plen == a.prefix
            })
            .copied()
            .collect();
        for r in routes {
            del_route(d, &r, ev);
        }
    }
}

/// `addrconf_ifdown`: the device's IPv6 addresses and routes go.
fn ifdown(d: &mut Devices, i: usize, ev: &mut Vec<Event>) {
    let index = d.links[i].index;
    let gone: Vec<Addr> = d.links[i]
        .addrs
        .iter()
        .filter(|a| a.ip.is_ipv6())
        .copied()
        .collect();
    d.links[i].addrs.retain(|a| a.ip.is_ipv4());
    let l = d.links[i].clone();
    for a in gone {
        ev.push(Event::DelAddr(l.clone(), a));
    }
    let routes: Vec<Route6> = d
        .routes6
        .iter()
        .filter(|r| r.index == index)
        .copied()
        .collect();
    for r in routes {
        del_route(d, &r, ev);
    }
    d.links[i].ready6 = false;
}

/// `addrconf_dev_config` and `addrconf_addr_gen`: the link-local address
/// by `addr_gen_mode` (with the fe80::/64 route), and the multicast
/// route.
pub fn dev_config(d: &mut Devices, i: usize, now: u64, ev: &mut Vec<Event>) {
    let fe80 = Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0);
    let l = d.links[i].clone();
    match address_in(d, i, fe80) {
        Some((ip, flags)) if !d.links[i].addrs.iter().any(|a| a.ip == IpAddr::V6(ip)) => {
            let life = Life::new(INFINITY, INFINITY).unwrap();
            add(
                d,
                i,
                Addr {
                    ip: ip.into(),
                    prefix: 64,
                    life,
                    flags: flags | IFA_F_OPTIMISTIC,
                    dad_end: 0,
                },
                now,
                ev,
            );
        }
        Some(_) => {}
        // No identifier: the prefix route alone, for stable privacy
        // without a secret.
        None if conf(&l, "addr_gen_mode") != ipv6conf::ADDR_GEN_NONE => {
            add_route(d, unicast(&l, fe80, 64, rt_table(&l, RT_TABLE_MAIN), 0), ev);
        }
        None => {}
    }
    // `addrconf_add_mroute`: ff00::/8 in the local table.
    let mut m = unicast(
        &l,
        Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0),
        8,
        RT_TABLE_LOCAL,
        0,
    );
    m.kind = RTN_MULTICAST;
    add_route(d, m, ev);
}

/// Bring IPv6 on the devices up to date at `now`: a link that became
/// ready is configured, one taken down or disabled is cleared, DAD that
/// ran its time completes, expired routes go. Returns the devices whose
/// link-local address just completed DAD (they solicit routers).
pub fn reconcile(d: &mut Devices, now: u64, ev: &mut Vec<Event>) -> Vec<usize> {
    let mut solicit = Vec::new();
    for i in 0..d.links.len() {
        let l = &d.links[i];
        if l.loopback() {
            continue;
        }
        let up = l.flags & IFF_UP != 0 && conf(l, "disable_ipv6") == 0;
        if !up && (l.ready6 || l.addrs.iter().any(|a| a.ip.is_ipv6())) {
            ifdown(d, i, ev);
        } else if link_ready(l) && !l.ready6 {
            d.links[i].ready6 = true;
            dev_config(d, i, now, ev);
        }
    }
    for i in 0..d.links.len() {
        let mut done = Vec::new();
        for a in &mut d.links[i].addrs {
            if a.flags & IFA_F_TENTATIVE != 0 && a.dad_end != 0 && now >= a.dad_end {
                a.flags &= !(IFA_F_TENTATIVE | IFA_F_OPTIMISTIC);
                a.dad_end = 0;
                done.push(*a);
            }
        }
        let l = d.links[i].clone();
        for a in done {
            ev.push(Event::NewAddr(l.clone(), a));
            if let IpAddr::V6(ip) = a.ip {
                add_route(d, local(&l, ip), ev);
                if a.scope() == 253 {
                    solicit.push(i);
                }
            }
        }
    }
    let expired: Vec<Route6> = d
        .routes6
        .iter()
        .filter(|r| r.expires != 0 && now >= r.expires)
        .copied()
        .collect();
    for r in expired {
        del_route(d, &r, ev);
    }
    solicit
}

/// The routes of the loopback at boot: `::1` in the main and local tables.
pub fn boot_routes(lo: &Link) -> Vec<Route6> {
    vec![
        unicast(lo, Ipv6Addr::LOCALHOST, 128, RT_TABLE_MAIN, 0),
        local(lo, Ipv6Addr::LOCALHOST),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha1_transform_is_one_compression() {
        // SHA-1 of "abc" is one compression of its padded block.
        let mut block = [0u8; 64];
        block[..3].copy_from_slice(b"abc");
        block[3] = 0x80;
        block[63] = 24;
        assert_eq!(
            sha1_transform(&block),
            [0xa9993e36, 0x4706816a, 0xba3e2571, 0x7850c26c, 0x9cd0d89d]
        );
    }

    #[test]
    fn link_local_identifiers() {
        let mac = [0x02, 0x61, 0x69, 0x6d, 0x00, 0x02];
        assert_eq!(
            eui64(&mac),
            [0x00, 0x61, 0x69, 0xff, 0xfe, 0x6d, 0x00, 0x02]
        );
        let fe80: Ipv6Addr = "fe80::".parse().unwrap();
        let secret: Ipv6Addr = "2001:db8::1".parse().unwrap();
        let a = stable_address(fe80, &mac, secret, 0).unwrap();
        // Stable: the same inputs give the same address; another secret,
        // prefix or DAD count another.
        assert_eq!(stable_address(fe80, &mac, secret, 0), Some(a));
        assert_eq!(prefix_of(a, 64), fe80);
        assert_ne!(
            stable_address(fe80, &mac, "::2".parse().unwrap(), 0),
            Some(a)
        );
        assert_ne!(stable_address(fe80, &mac, secret, 1), Some(a));
        let p: Ipv6Addr = "2001:db8:1:2::".parse().unwrap();
        assert_eq!(
            prefix_of(stable_address(p, &mac, secret, 0).unwrap(), 64),
            p
        );
        assert!(reserved_iid([0; 8]));
        assert!(reserved_iid([
            0xfd, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x80
        ]));
        assert!(!reserved_iid(eui64(&mac)));
    }

    #[test]
    fn prefixes() {
        let a: Ipv6Addr = "2001:db8:1:2:3:4:5:6".parse().unwrap();
        assert_eq!(
            prefix_of(a, 64),
            "2001:db8:1:2::".parse::<Ipv6Addr>().unwrap()
        );
        assert_eq!(prefix_of(a, 0), Ipv6Addr::UNSPECIFIED);
        assert_eq!(prefix_of(a, 128), a);
    }

    #[test]
    fn routes_round_trip() {
        let r = Route6 {
            index: 2,
            dst: "::".parse().unwrap(),
            plen: 0,
            gateway: Some("fe80::61:69ff:fe6d:1".parse().unwrap()),
            table: 1002,
            proto: RTPROT_RA,
            kind: RTN_UNICAST,
            metric: 1024,
            expires: 12345,
            hoplimit: 64,
        };
        let line = r.line();
        let w: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(w[0], "route6");
        assert_eq!(Route6::parse(&w[1..]), Some(r));
    }
}
