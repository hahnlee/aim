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
use super::uplink::Advert;

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
/// None for IN6_ADDR_GEN_MODE_NONE's link-local address, or stable
/// privacy without a secret. RANDOM mode makes the device's secret
/// first, as `ipv6_gen_mode_random_init` does; NONE gives a prefix's
/// address by EUI-64 (`addrconf_prefix_rcv`).
pub fn address_in(
    d: &mut Devices,
    i: usize,
    prefix: Ipv6Addr,
    link_local: bool,
) -> Option<(Ipv6Addr, u32)> {
    let l = &d.links[i];
    let mode = match l.conf6.get("addr_gen_mode", Scope::Dev(l.index), l.mtu) {
        ipv6conf::ADDR_GEN_NONE if !link_local => ipv6conf::ADDR_GEN_EUI64,
        m => m,
    };
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
pub fn add(d: &mut Devices, i: usize, a: Addr, now: u64, ev: &mut Vec<Event>) {
    add_with(d, i, a, now, ev, true);
}

fn add_with(
    d: &mut Devices,
    i: usize,
    mut a: Addr,
    now: u64,
    ev: &mut Vec<Event>,
    prefix_route: bool,
) {
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
    if prefix_route && a.flags & IFA_F_NOPREFIXROUTE == 0 && a.prefix < 128 {
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
    match address_in(d, i, fe80, true) {
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
            // A link-local address without DAD completes at once
            // (`addrconf_dad_completed`).
            if d.links[i]
                .addrs
                .iter()
                .any(|a| a.scope() == 253 && a.flags & IFA_F_TENTATIVE == 0)
            {
                solicit.push(i);
            }
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
    regenerate_temps(d, now, ev);
    solicit
}

/// The routes of the loopback at boot: `::1` in the main and local tables.
pub fn boot_routes(lo: &Link) -> Vec<Route6> {
    vec![
        unicast(lo, Ipv6Addr::LOCALHOST, 128, RT_TABLE_MAIN, 0),
        local(lo, Ipv6Addr::LOCALHOST),
    ]
}

// ---- router advertisements --------------------------------------------------

/// The virtual router's link-local address: EUI-64 of its hardware
/// address (`dhcp::ROUTER_MAC`).
pub fn router_ll() -> Ipv6Addr {
    let mut a = [0u8; 16];
    a[..2].copy_from_slice(&[0xfe, 0x80]);
    a[8..].copy_from_slice(&eui64(&super::dhcp::ROUTER_MAC));
    Ipv6Addr::from(a)
}

/// The router's AdvDefaultLifetime (3 x MaxRtrAdvInterval) and
/// MaxRtrAdvInterval; the prefix's AdvValidLifetime and
/// AdvPreferredLifetime (RFC 4861's defaults); the RDNSS lifetime.
const ROUTER_LIFETIME: u16 = 1800;
const RA_INTERVAL_MS: u64 = 600_000;
const PREFIX_VALID: u32 = 2_592_000;
const PREFIX_PREFERRED: u32 = 604_800;
const RDNSS_LIFETIME: u32 = 1800;
/// `MIN_VALID_LIFETIME`: the two hours RFC 4862 5.5.3 e) keeps.
const MIN_VALID_LIFETIME: u32 = 7200;
/// `ND_OPT_RDNSS`.
const ND_OPT_RDNSS: u8 = 25;

/// A prefix information option.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pio {
    pub prefix: Ipv6Addr,
    pub plen: u8,
    pub onlink: bool,
    pub autonomous: bool,
    pub valid: u32,
    pub preferred: u32,
}

/// A router advertisement, as `ndisc_router_discovery` reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ra {
    pub hop_limit: u8,
    pub lifetime: u16,
    pub mtu: Option<u32>,
    pub prefixes: Vec<Pio>,
    /// RDNSS options: their lifetime and servers.
    pub rdnss: Vec<(u32, Vec<Ipv6Addr>)>,
}

/// The advertisement the router sends for the Mac's IPv6 network `adv`
/// after it advertised `prev`: the Mac's prefix and DNS servers; a prefix
/// or servers it no longer has with zero lifetimes; and when the Mac has
/// no IPv6 any more, a router lifetime of zero (RFC 4861 6.2.5). None
/// when there was and is nothing to advertise.
pub fn ra_for(adv: Option<&Advert>, prev: Option<&Advert>) -> Option<Ra> {
    if adv.is_none() && prev.is_none() {
        return None;
    }
    let mut ra = Ra {
        hop_limit: 64,
        lifetime: 0,
        mtu: None,
        prefixes: Vec::new(),
        rdnss: Vec::new(),
    };
    let pio = |a: &Advert, valid, preferred| Pio {
        prefix: prefix_of(a.up.addr, a.up.prefix),
        plen: a.up.prefix,
        onlink: true,
        autonomous: true,
        valid,
        preferred,
    };
    if let Some(a) = adv {
        ra.lifetime = ROUTER_LIFETIME;
        ra.mtu = Some(a.up.mtu);
        ra.prefixes.push(pio(a, PREFIX_VALID, PREFIX_PREFERRED));
        if !a.dns.is_empty() {
            ra.rdnss.push((RDNSS_LIFETIME, a.dns.clone()));
        }
    }
    if let Some(p) = prev {
        let old = pio(p, 0, 0);
        if !ra
            .prefixes
            .iter()
            .any(|x| (x.prefix, x.plen) == (old.prefix, old.plen))
        {
            ra.prefixes.push(old);
        }
        let gone: Vec<Ipv6Addr> = p
            .dns
            .iter()
            .filter(|d| adv.is_none_or(|a| !a.dns.contains(d)))
            .copied()
            .collect();
        if !gone.is_empty() {
            ra.rdnss.push((0, gone));
        }
    }
    Some(ra)
}

/// An RDNSS option (RFC 8106): type, length in 8-octet units, reserved,
/// lifetime, servers.
fn rdnss_option(lifetime: u32, servers: &[Ipv6Addr]) -> Vec<u8> {
    let mut o = vec![ND_OPT_RDNSS, (1 + 2 * servers.len()) as u8, 0, 0];
    o.extend_from_slice(&lifetime.to_be_bytes());
    for s in servers {
        o.extend_from_slice(&s.octets());
    }
    o
}

/// Lifetimes set at `now`, an address keeping when it was added.
fn relife(old: &Life, valid: u32, preferred: u32, now: u64) -> Life {
    Life {
        valid,
        preferred,
        created: old.created,
        stamp: now,
        deprecated: preferred == 0,
    }
}

/// `ndisc_router_discovery` on device `i` for advertisement `ra` from the
/// virtual router: the default route, the hop limit, the prefixes, the
/// MTU and the user options, each as the device's configuration allows.
pub fn receive_ra(d: &mut Devices, i: usize, ra: &Ra, now: u64, ev: &mut Vec<Event>) {
    let l = d.links[i].clone();
    if !ipv6conf::accept_ra(&l.conf6, Scope::Dev(l.index)) {
        return;
    }
    let router = router_ll();
    if conf(&l, "accept_ra_defrtr") != 0
        && !(ra.lifetime != 0 && (ra.lifetime as i32) < conf(&l, "accept_ra_min_lft"))
    {
        // `rt6_add_dflt_router`.
        let mut r = Route6 {
            index: l.index,
            dst: Ipv6Addr::UNSPECIFIED,
            plen: 0,
            gateway: Some(router),
            table: rt_table(&l, RT_TABLE_MAIN),
            proto: RTPROT_RA,
            kind: RTN_UNICAST,
            metric: conf(&l, "ra_defrtr_metric") as u32,
            expires: 0,
            hoplimit: ra.hop_limit,
        };
        if ra.lifetime == 0 {
            del_route(d, &r, ev);
        } else {
            r.expires = now + ra.lifetime as u64 * 1000;
            add_route(d, r, ev);
        }
    }
    if ra.hop_limit != 0 && conf(&l, "accept_ra_min_hop_limit") <= ra.hop_limit as i32 {
        d.links[i].conf6.put("hop_limit", ra.hop_limit as i32);
    }
    if conf(&l, "accept_ra_pinfo") != 0 {
        for p in &ra.prefixes {
            prefix_rcv(d, i, p, now, ev);
        }
    }
    if let Some(mtu) = ra.mtu
        && conf(&l, "accept_ra_mtu") != 0
        && (1280..=l.mtu).contains(&mtu)
    {
        d.links[i].conf6.put("mtu", mtu as i32);
    }
    for (lifetime, servers) in &ra.rdnss {
        ev.push(Event::UserOpt {
            index: l.index,
            opt: rdnss_option(*lifetime, servers),
            router,
        });
    }
}

/// `addrconf_prefix_rcv`: the on-link prefix's route, and with
/// autoconfiguration the device's address in it (and its temporary
/// addresses).
fn prefix_rcv(d: &mut Devices, i: usize, p: &Pio, now: u64, ev: &mut Vec<Event>) {
    let link_local = p.prefix.segments()[0] & 0xffc0 == 0xfe80;
    if p.plen > 128 || link_local || p.prefix.is_multicast() || p.preferred > p.valid {
        return;
    }
    let prefix = prefix_of(p.prefix, p.plen);
    let l = d.links[i].clone();
    if p.onlink {
        let expires = if p.valid == INFINITY {
            0
        } else {
            now + p.valid as u64 * 1000
        };
        let r = unicast(&l, prefix, p.plen, rt_table(&l, RT_TABLE_MAIN), expires);
        if p.valid == 0 {
            del_route(d, &r, ev);
        } else {
            add_route(d, r, ev);
        }
    }
    if p.autonomous && conf(&l, "autoconf") != 0 && p.plen == 64 {
        if let Some((ip, flags)) = address_in(d, i, prefix, false) {
            prefix_rcv_add_addr(d, i, ip, flags, p, now, ev);
        }
    }
    ev.push(Event::Prefix {
        index: l.index,
        prefix,
        plen: p.plen,
        flags: p.onlink as u8 | (p.autonomous as u8) << 1,
        valid: p.valid,
        preferred: p.preferred,
    });
}

/// `addrconf_prefix_rcv_add_addr`: a new address in the prefix, or the
/// existing one's lifetimes by RFC 4862's two-hour rule; then
/// `manage_tempaddrs`.
fn prefix_rcv_add_addr(
    d: &mut Devices,
    i: usize,
    ip: Ipv6Addr,
    flags: u32,
    p: &Pio,
    now: u64,
    ev: &mut Vec<Event>,
) {
    let (mut valid, mut preferred) = (p.valid, p.preferred);
    let existing = d.links[i].addrs.iter().position(|a| a.ip == IpAddr::V6(ip));
    let public = match existing {
        None if valid == 0 => return,
        None => {
            let a = Addr {
                ip: ip.into(),
                prefix: 64,
                life: Life::new(valid, preferred).unwrap(),
                flags: flags | IFA_F_OPTIMISTIC,
                dad_end: 0,
            };
            add_kernel(d, i, a, now, ev);
            let a = *d.links[i].addrs.iter().find(|x| x.ip == a.ip).unwrap();
            if conf(&d.links[i], "use_tempaddr") > 0 {
                create_temp(d, i, &a, now, ev);
            }
            return;
        }
        Some(k) => d.links[i].addrs[k],
    };
    let (stored, _) = public.life.left(now);
    let update = if valid > MIN_VALID_LIFETIME || valid > stored {
        true
    } else if stored <= MIN_VALID_LIFETIME {
        false
    } else {
        valid = MIN_VALID_LIFETIME;
        preferred = preferred.min(valid);
        true
    };
    if update {
        let k = existing.unwrap();
        d.links[i].addrs[k].life = relife(&public.life, valid, preferred, now);
        ev.push(Event::NewAddr(d.links[i].clone(), d.links[i].addrs[k]));
    } else {
        (valid, preferred) = public.life.left(now);
    }
    manage_temps(d, i, &public, valid, preferred, now, ev);
}

/// `regen_advance`: how long before a temporary address's preferred
/// lifetime ends a new one is made.
fn regen_advance(l: &Link) -> u32 {
    (conf(l, "regen_max_retry") * conf(l, "dad_transmits")).max(0) as u32
}

/// The device's `desync_factor`, made once below `max_desync_factor`.
fn desync(d: &mut Devices, i: usize) -> u32 {
    let l = &d.links[i];
    let max =
        conf(l, "max_desync_factor").min(conf(l, "temp_prefered_lft") - regen_advance(l) as i32);
    if d.links[i].desync6 == 0 && max > 0 {
        d.links[i].desync6 = 1 + rand_u32() % max as u32;
    }
    d.links[i].desync6.saturating_sub(1)
}

fn rand_u32() -> u32 {
    // SAFETY: trivial.
    unsafe { libc::arc4random() }
}

fn same64(a: &IpAddr, b: &IpAddr) -> bool {
    matches!((a, b), (IpAddr::V6(x), IpAddr::V6(y)) if prefix_of(*x, 64) == prefix_of(*y, 64))
}

/// `manage_tempaddrs`: the public address's new lifetimes extend its
/// temporary addresses' within TEMP_VALID_LIFETIME and
/// TEMP_PREFERRED_LIFETIME - DESYNC_FACTOR of their creation; one is made
/// when there is none.
fn manage_temps(
    d: &mut Devices,
    i: usize,
    public: &Addr,
    valid: u32,
    preferred: u32,
    now: u64,
    ev: &mut Vec<Event>,
) {
    let desync = desync(d, i);
    let l = d.links[i].clone();
    let (temp_valid, temp_pref) = (conf(&l, "temp_valid_lft"), conf(&l, "temp_prefered_lft"));
    let mut any = false;
    let mut gone = Vec::new();
    for t in d.links[i]
        .addrs
        .iter_mut()
        .filter(|t| t.flags & IFA_F_TEMPORARY != 0 && same64(&t.ip, &public.ip))
    {
        any = true;
        let age = (now.saturating_sub(t.life.created) / 1000) as i64;
        let max_valid = (temp_valid as i64 - age).max(0) as u32;
        let max_pref = (temp_pref as i64 - desync as i64 - age).max(0) as u32;
        let v = valid.min(max_valid);
        let pr = preferred.min(max_pref).min(v);
        if v == 0 {
            gone.push(*t);
            continue;
        }
        t.life = relife(&t.life, v, pr, now);
        ev.push(Event::NewAddr(l.clone(), *t));
    }
    for t in gone {
        d.links[i].addrs.retain(|x| x.ip != t.ip);
        ev.push(Event::DelAddr(l.clone(), t));
        removed(d, &l, &t, ev);
    }
    if !any && conf(&l, "use_tempaddr") > 0 {
        create_temp(d, i, public, now, ev);
    }
}

/// `ipv6_create_tempaddr`: an address with a random interface identifier
/// (RFC 8981) in the public address's prefix, its lifetimes bounded by
/// the public address's and the temporary limits; none when its preferred
/// lifetime would not outlast `regen_advance`.
fn create_temp(d: &mut Devices, i: usize, public: &Addr, now: u64, ev: &mut Vec<Event>) {
    let IpAddr::V6(p) = public.ip else { return };
    let desync = desync(d, i);
    let l = &d.links[i];
    let (pub_valid, pub_pref) = public.life.left(now);
    let valid = pub_valid.min(conf(l, "temp_valid_lft").max(0) as u32);
    let preferred = pub_pref
        .min((conf(l, "temp_prefered_lft") - desync as i32).max(0) as u32)
        .min(valid);
    if preferred <= regen_advance(l) {
        return;
    }
    let ip = loop {
        let mut iid = [0u8; 8];
        // SAFETY: filling our buffer.
        unsafe { libc::arc4random_buf(iid.as_mut_ptr().cast(), iid.len()) };
        let mut a = p.octets();
        a[8..].copy_from_slice(&iid);
        let ip = Ipv6Addr::from(a);
        if !reserved_iid(iid) && !l.addrs.iter().any(|x| x.ip == IpAddr::V6(ip)) {
            break ip;
        }
    };
    let a = Addr {
        ip: ip.into(),
        prefix: public.prefix,
        life: Life {
            valid,
            preferred,
            created: now,
            stamp: now,
            deprecated: false,
        },
        flags: IFA_F_TEMPORARY | (public.flags & IFA_F_OPTIMISTIC),
        dad_end: 0,
    };
    add_kernel(d, i, a, now, ev);
}

/// An address the kernel makes in a prefix (SLAAC, temporary): no prefix
/// route of its own, the PIO's route stands for the prefix.
fn add_kernel(d: &mut Devices, i: usize, a: Addr, now: u64, ev: &mut Vec<Event>) {
    add_with(d, i, a, now, ev, false);
}

/// `addrconf_verify`'s regeneration: a temporary address within
/// `regen_advance` of the end of its preferred lifetime gets a successor
/// while the public address of its prefix is still preferred.
pub fn regenerate_temps(d: &mut Devices, now: u64, ev: &mut Vec<Event>) {
    for i in 0..d.links.len() {
        let l = d.links[i].clone();
        if conf(&l, "use_tempaddr") <= 0 {
            continue;
        }
        let advance = regen_advance(&l);
        let publics: Vec<Addr> = l
            .addrs
            .iter()
            .filter(|a| {
                a.ip.is_ipv6()
                    && a.flags & IFA_F_TEMPORARY == 0
                    && a.scope() == 0
                    && a.life.left(now).1 > 0
            })
            .copied()
            .collect();
        for public in publics {
            let temps: Vec<&Addr> = l
                .addrs
                .iter()
                .filter(|t| t.flags & IFA_F_TEMPORARY != 0 && same64(&t.ip, &public.ip))
                .collect();
            if !temps.is_empty() && temps.iter().all(|t| t.life.left(now).1 <= advance) {
                create_temp(d, i, &public, now, ev);
            }
        }
    }
}

/// The virtual router on `eth0`: it answers the solicitation a device
/// sends when its link-local address completes DAD, advertises again
/// every MaxRtrAdvInterval and at once when the Mac's IPv6 network
/// changes, and its advertisements are received as `receive_ra` takes
/// them. `advert` is the Mac's IPv6 network, read only when an
/// advertisement could be received.
pub fn advertise(
    d: &mut Devices,
    advert: impl FnOnce() -> Option<Advert>,
    solicited: &[usize],
    now: u64,
    ev: &mut Vec<Event>,
) {
    let Some(i) = d.links.iter().position(|l| l.index == super::netif::ETH0) else {
        return;
    };
    let l = &d.links[i];
    let listening = l.ready6
        && l.carrier
        && l.addrs.iter().any(|a| {
            a.scope() == 253 && (a.flags & IFA_F_TENTATIVE == 0 || a.flags & IFA_F_OPTIMISTIC != 0)
        });
    if !listening {
        return;
    }
    let adv = advert();
    let solicit = solicited.contains(&i) && conf(l, "router_solicitations") != 0;
    let changed = adv.as_ref() != d.ra_seen.as_ref();
    let due = d.ra_next != 0 && now >= d.ra_next;
    if !(solicit || changed || due) {
        if d.ra_next == 0 {
            d.ra_next = now + RA_INTERVAL_MS;
        }
        return;
    }
    if let Some(ra) = ra_for(adv.as_ref(), d.ra_seen.as_ref()) {
        receive_ra(d, i, &ra, now, ev);
    }
    d.ra_seen = adv;
    d.ra_next = now + RA_INTERVAL_MS;
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

    fn mac_network(addr: &str) -> Advert {
        Advert {
            up: super::super::uplink::Uplink6 {
                index: 7,
                addr: addr.parse().unwrap(),
                temporary: None,
                prefix: 64,
                gateway: "fe80::1".parse().unwrap(),
                mtu: 1500,
            },
            dns: vec!["2001:db8::53".parse().unwrap()],
        }
    }

    /// eth0 up, with carrier and no DAD, configured at `t`.
    fn ready_eth0(t: u64) -> (Devices, Vec<usize>) {
        let mut d = super::super::netif::boot_devices();
        let e = &mut d.links[1];
        e.flags |= IFF_UP;
        e.carrier = true;
        e.conf6.put("dad_transmits", 0);
        e.conf6.put("use_tempaddr", 2);
        let mut ev = Vec::new();
        let solicited = reconcile(&mut d, t, &mut ev);
        (d, solicited)
    }

    fn v6(l: &Link) -> Vec<Addr> {
        l.addrs.iter().filter(|a| a.ip.is_ipv6()).copied().collect()
    }

    /// The virtual router answers eth0's solicitation: a default route via
    /// its link-local address, the prefix's route, a SLAAC and a
    /// temporary address, and the RDNSS option for userspace.
    #[test]
    fn a_solicited_advertisement_provisions_eth0() {
        let t = 1_000_000;
        let (mut d, solicited) = ready_eth0(t);
        assert_eq!(solicited, [1]);
        assert_eq!(v6(&d.links[1]).len(), 1);
        let mut ev = Vec::new();
        let adv = mac_network("2001:db8:1:2::5");
        advertise(&mut d, || Some(adv.clone()), &solicited, t, &mut ev);
        let default = d
            .routes6
            .iter()
            .find(|r| r.plen == 0)
            .expect("a default route");
        assert_eq!(default.gateway, Some(router_ll()));
        assert_eq!(
            (default.proto, default.metric, default.hoplimit),
            (RTPROT_RA, 1024, 64)
        );
        assert_eq!(default.expires, t + 1_800_000);
        assert!(d.routes6.iter().any(|r| r.plen == 64
            && r.dst == "2001:db8:1:2::".parse::<Ipv6Addr>().unwrap()
            && r.expires != 0));
        let addrs = v6(&d.links[1]);
        let slaac = addrs
            .iter()
            .find(|a| a.ip == "2001:db8:1:2:61:69ff:fe6d:2".parse::<IpAddr>().unwrap())
            .expect("the EUI-64 SLAAC address");
        assert!(!slaac.life.permanent() && slaac.flags & IFA_F_TEMPORARY == 0);
        let temp = addrs
            .iter()
            .find(|a| a.flags & IFA_F_TEMPORARY != 0)
            .expect("a temporary address");
        assert!(same64(&temp.ip, &slaac.ip));
        assert!(temp.life.left(t).1 <= 86400 && temp.life.left(t).0 <= 604800);
        assert!(
            ev.iter()
                .any(|e| matches!(e, Event::UserOpt { opt, .. } if opt[0] == ND_OPT_RDNSS))
        );
        assert!(ev.iter().any(|e| matches!(
            e,
            Event::Prefix {
                plen: 64,
                flags: 3,
                ..
            }
        )));
        // Nothing new: no advertisement until MaxRtrAdvInterval.
        let mut ev = Vec::new();
        advertise(&mut d, || Some(adv.clone()), &[], t + 1000, &mut ev);
        assert!(ev.is_empty());
    }

    /// The Mac moves to another IPv6 network: the old prefix is
    /// deprecated with two hours left (RFC 4862 5.5.3 e)); the Mac losing
    /// IPv6 takes the default route away.
    #[test]
    fn the_router_follows_the_macs_ipv6() {
        let t = 1_000_000;
        let (mut d, solicited) = ready_eth0(t);
        let mut ev = Vec::new();
        advertise(
            &mut d,
            || Some(mac_network("2001:db8:1:2::5")),
            &solicited,
            t,
            &mut ev,
        );
        advertise(
            &mut d,
            || Some(mac_network("2001:db8:9:9::5")),
            &[],
            t + 5000,
            &mut ev,
        );
        let old: IpAddr = "2001:db8:1:2:61:69ff:fe6d:2".parse().unwrap();
        let a = d.links[1].addrs.iter().find(|a| a.ip == old).unwrap();
        assert_eq!(a.life.left(t + 5000), (MIN_VALID_LIFETIME, 0));
        assert!(a.life.deprecated);
        assert!(
            d.links[1]
                .addrs
                .iter()
                .any(|a| a.ip == "2001:db8:9:9:61:69ff:fe6d:2".parse::<IpAddr>().unwrap())
        );
        assert!(
            !d.routes6
                .iter()
                .any(|r| r.dst == "2001:db8:1:2::".parse::<Ipv6Addr>().unwrap())
        );
        advertise(&mut d, || None, &[], t + 9000, &mut ev);
        assert!(!d.routes6.iter().any(|r| r.plen == 0));
        assert!(d.ra_seen.is_none());
    }

    /// A router lifetime below `accept_ra_min_lft` adds no default route;
    /// without `accept_ra` nothing is taken.
    #[test]
    fn devices_choose_what_they_take() {
        let t = 1_000_000;
        let (mut d, solicited) = ready_eth0(t);
        d.links[1].conf6.put("accept_ra_min_lft", 3600);
        let mut ev = Vec::new();
        advertise(
            &mut d,
            || Some(mac_network("2001:db8:1:2::5")),
            &solicited,
            t,
            &mut ev,
        );
        assert!(!d.routes6.iter().any(|r| r.plen == 0));
        assert!(d.routes6.iter().any(|r| r.plen == 64 && r.expires != 0));
        let (mut d, solicited) = ready_eth0(t);
        d.links[1].conf6.put("accept_ra", 0);
        advertise(
            &mut d,
            || Some(mac_network("2001:db8:1:2::5")),
            &solicited,
            t,
            &mut ev,
        );
        assert_eq!(v6(&d.links[1]).len(), 1);
    }

    #[test]
    fn rdnss_options() {
        let o = rdnss_option(1800, &["2001:db8::53".parse().unwrap()]);
        assert_eq!(o.len(), 24);
        assert_eq!(&o[..8], &[25, 3, 0, 0, 0, 0, 0x07, 0x08]);
        assert_eq!(
            router_ll(),
            "fe80::61:69ff:fe6d:1".parse::<Ipv6Addr>().unwrap()
        );
    }
}
