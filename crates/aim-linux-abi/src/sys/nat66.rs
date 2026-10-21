//! The guest's global IPv6 addresses on `eth0` stand for the Mac's
//! (docs/network.md, #267). `eth0`'s prefix is the Mac's (`addrconf`'s
//! router advertises it), but SLAAC and temporary addresses give the
//! guest addresses the Mac does not have, and guest sockets are host
//! sockets. As a NAT66 router translating between a network and an
//! outside prefix does (RFC 6296), the socket layer gives the host the
//! Mac's address where the guest names one of its own (bind, connect,
//! sendto), and gives the guest its own where the host reports the Mac's
//! (getsockname, getpeername, recvfrom, accept): a temporary address
//! stands for the Mac's temporary one, a stable one for the Mac's stable
//! one. Back, the Mac's temporary address is the newest temporary one,
//! its stable address the stable one; with no temporary address on the
//! Mac, every guest address stands for its one address, which comes back
//! as the stable one.
//!
//! The table is built from the shared device state and kept until the
//! state file changes, so a socket call costs one stat at most.

use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, Mutex};

use super::addrconf::{IFA_F_TEMPORARY, prefix_of};
use super::netif::{self, Devices};

/// Pairs of a guest address and the Mac's it stands for, each way.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Table {
    to_host: Vec<(Ipv6Addr, Ipv6Addr)>,
    to_guest: Vec<(Ipv6Addr, Ipv6Addr)>,
}

impl Table {
    /// The pairs for `eth0`'s addresses in the prefix its router last
    /// advertised (`Devices::ra_seen`).
    pub fn of(d: &Devices) -> Table {
        let mut t = Table::default();
        let (Some(adv), Some(eth0)) = (&d.ra_seen, d.links.iter().find(|l| l.index == netif::ETH0))
        else {
            return t;
        };
        let up = &adv.up;
        let prefix = prefix_of(up.addr, up.prefix);
        let mut public = Vec::new();
        let mut temps = Vec::new();
        for a in &eth0.addrs {
            let IpAddr::V6(ip) = a.ip else { continue };
            if a.scope() != 0 || prefix_of(ip, up.prefix) != prefix {
                continue;
            }
            if a.flags & IFA_F_TEMPORARY != 0 {
                temps.push(*a);
            } else {
                public.push(ip);
            }
        }
        // The newest temporary address first, a preferred one before a
        // deprecated one: the one source selection picks.
        temps.sort_by_key(|a| (a.life.deprecated, std::cmp::Reverse(a.life.created)));
        let temps: Vec<Ipv6Addr> = temps
            .iter()
            .filter_map(|a| match a.ip {
                IpAddr::V6(ip) => Some(ip),
                IpAddr::V4(_) => None,
            })
            .collect();
        let mac_temp = up.temporary.unwrap_or(up.addr);
        for &g in &public {
            t.to_host.push((g, up.addr));
        }
        for &g in &temps {
            t.to_host.push((g, mac_temp));
        }
        if let (Some(m), Some(&g)) = (up.temporary, temps.first().or(public.first())) {
            t.to_guest.push((m, g));
        }
        if let Some(&g) = public.first().or(temps.first()) {
            t.to_guest.push((up.addr, g));
        }
        t
    }

    pub fn to_host(&self, a: Ipv6Addr) -> Ipv6Addr {
        self.to_host
            .iter()
            .find(|(g, _)| *g == a)
            .map_or(a, |(_, m)| *m)
    }

    pub fn to_guest(&self, a: Ipv6Addr) -> Ipv6Addr {
        self.to_guest
            .iter()
            .find(|(m, _)| *m == a)
            .map_or(a, |(_, g)| *g)
    }
}

/// Whether `a` could be one of the pairs: a global unicast address.
fn global(a: &Ipv6Addr) -> bool {
    !(a.is_unspecified()
        || a.is_loopback()
        || a.is_multicast()
        || a.segments()[0] & 0xffc0 == 0xfe80
        || a.to_ipv4_mapped().is_some())
}

type Stamp = (i64, i64, u64);

static CACHE: Mutex<Option<(Stamp, Arc<Table>)>> = Mutex::new(None);

/// The table for the device state as it is now.
fn table() -> Arc<Table> {
    let Some(stamp) = netif::state_stamp() else {
        return Arc::new(Table::default());
    };
    let mut c = CACHE.lock().unwrap();
    if let Some((s, t)) = &*c
        && *s == stamp
    {
        return t.clone();
    }
    let t = Arc::new(netif::peek().map(|d| Table::of(&d)).unwrap_or_default());
    *c = Some((stamp, t.clone()));
    t
}

/// The host's address for a guest's `a`.
pub fn to_host(a: Ipv6Addr) -> Ipv6Addr {
    if global(&a) { table().to_host(a) } else { a }
}

/// The guest's address for a host's `a`.
pub fn to_guest(a: Ipv6Addr) -> Ipv6Addr {
    if global(&a) { table().to_guest(a) } else { a }
}

/// A `sockaddr_in6`'s address (bytes 8 to 24, the same in Linux's and
/// Darwin's layouts) translated by `f`.
pub fn translate(sa: &mut [u8], f: fn(Ipv6Addr) -> Ipv6Addr) {
    if let Some(b) = sa.get_mut(8..24) {
        let a: [u8; 16] = (&*b).try_into().unwrap();
        b.copy_from_slice(&f(Ipv6Addr::from(a)).octets());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::addrconf::IFA_F_TEMPORARY;
    use crate::sys::netif::{Addr, Life};
    use crate::sys::uplink::{Advert, Uplink6};

    fn addr(ip: &str, flags: u32, created: u64) -> Addr {
        let mut a = Addr::new(ip.parse().unwrap(), 64);
        a.life = Life {
            created,
            ..Life::new(86400, 14400).unwrap()
        };
        a.flags = flags;
        a
    }

    fn devices(temporary: Option<&str>) -> Devices {
        let mut d = netif::boot_devices();
        d.links[1].addrs = vec![
            addr("fe80::61:69ff:fe6d:2", 0, 1),
            addr("2001:db8:1:2:61:69ff:fe6d:2", 0, 2),
            addr("2001:db8:1:2::aaaa", IFA_F_TEMPORARY, 3),
            addr("2001:db8:1:2::bbbb", IFA_F_TEMPORARY, 4),
            addr("2001:db8:7:7::1", 0, 5),
        ];
        d.ra_seen = Some(Advert {
            up: Uplink6 {
                index: 7,
                addr: "2001:db8:1:2::5".parse().unwrap(),
                temporary: temporary.map(|t| t.parse().unwrap()),
                prefix: 64,
                gateway: "fe80::1".parse().unwrap(),
                mtu: 1500,
            },
            dns: Vec::new(),
        });
        d
    }

    fn ip(s: &str) -> Ipv6Addr {
        s.parse().unwrap()
    }

    /// The stable address stands for the Mac's stable one, temporary ones
    /// for its temporary one; back, the newest temporary address.
    #[test]
    fn guest_addresses_stand_for_the_macs() {
        let t = Table::of(&devices(Some("2001:db8:1:2::77")));
        assert_eq!(
            t.to_host(ip("2001:db8:1:2:61:69ff:fe6d:2")),
            ip("2001:db8:1:2::5")
        );
        assert_eq!(t.to_host(ip("2001:db8:1:2::aaaa")), ip("2001:db8:1:2::77"));
        assert_eq!(t.to_host(ip("2001:db8:1:2::bbbb")), ip("2001:db8:1:2::77"));
        assert_eq!(t.to_guest(ip("2001:db8:1:2::77")), ip("2001:db8:1:2::bbbb"));
        assert_eq!(
            t.to_guest(ip("2001:db8:1:2::5")),
            ip("2001:db8:1:2:61:69ff:fe6d:2")
        );
        // Outside the prefix, and other addresses, stay as they are.
        assert_eq!(t.to_host(ip("2001:db8:7:7::1")), ip("2001:db8:7:7::1"));
        assert_eq!(t.to_guest(ip("2606:4700::1111")), ip("2606:4700::1111"));
        assert_eq!(
            t.to_host(ip("fe80::61:69ff:fe6d:2")),
            ip("fe80::61:69ff:fe6d:2")
        );
    }

    /// A Mac without a temporary address: both stand for its one address,
    /// which comes back as the stable guest address.
    #[test]
    fn a_mac_without_temporary_addresses() {
        let t = Table::of(&devices(None));
        assert_eq!(t.to_host(ip("2001:db8:1:2::aaaa")), ip("2001:db8:1:2::5"));
        assert_eq!(
            t.to_guest(ip("2001:db8:1:2::5")),
            ip("2001:db8:1:2:61:69ff:fe6d:2")
        );
        let mut d = devices(None);
        d.ra_seen = None;
        assert_eq!(Table::of(&d), Table::default());
    }

    #[test]
    fn sockaddrs_translate_in_place() {
        let mut sa = vec![0u8; 28];
        sa[8..24].copy_from_slice(&ip("2001:db8::1").octets());
        translate(&mut sa, |_| ip("2001:db8::2"));
        assert_eq!(&sa[8..24], &ip("2001:db8::2").octets());
        assert!(!global(&ip("::ffff:1.2.3.4")) && global(&ip("2001:db8::1")));
    }
}
