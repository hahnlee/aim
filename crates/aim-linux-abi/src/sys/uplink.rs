//! The Mac's network as the guest's `eth0` sees it (docs/network.md):
//! `aim_uplink`'s default route, address, gateway and DNS servers, which
//! the virtual link's DHCP server hands out (`dhcp`); `eth0` has carrier
//! while they exist, and [`watch`] makes sure the guest hears of a change.
//!
//! The Mac's IPv6 network is what the virtual router advertises
//! (`addrconf`): its prefix, its IPv6 DNS servers, and the Mac's
//! addresses a guest address stands for (`net`).
//!
//! A boot's test hook, `<runtime>/net/simulate`, changes what the guest
//! sees without touching the Mac's network: a line `down` takes the
//! network away; `addr A/P`, `gateway G`, `dns S...` and `lease SECONDS`
//! replace those values of a present one, and `interface NAME` puts the
//! Mac's host interface NAME behind it (the Mac moving from Wi-Fi to
//! cable). For IPv6, `down6` takes the Mac's IPv6 away; `addr6 A/P` gives
//! it the global address A in prefix P (an IPv6 network on the IPv4 one's
//! interface, router fe80::1, when the Mac has none), `temp6 A` its
//! temporary address, and `gateway6 G` and `dns6 S...` its router and DNS
//! servers.

use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicBool, Ordering};

pub use aim_uplink::{Uplink, Uplink6, host_index};
use aim_uplink::{dns_servers, dns6_servers, uplink, uplink6};

/// The lease time the virtual router gives: DhcpClient renews after half
/// of it, which is how DNS changes reach the guest.
pub const LEASE_SECS: u32 = 3600;
/// What `eth0` stands for: the Mac's network, its DNS servers and the
/// lease time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    pub up: Uplink,
    pub dns: Vec<Ipv4Addr>,
    pub secs: u32,
}

fn hook() -> String {
    std::fs::read_to_string(super::netif::kernel_dir("net").join("simulate")).unwrap_or_default()
}

/// The Mac's network as the guest sees it (the test hook applied), or
/// None when there is none.
pub fn current() -> Option<Uplink> {
    simulate(&hook(), uplink()?, Vec::new()).map(|l| l.up)
}

/// What the virtual router leases now, or None without a network.
pub fn lease() -> Option<Lease> {
    simulate(&hook(), uplink()?, dns_servers())
}

/// What the virtual router advertises: the Mac's IPv6 network and its
/// IPv6 DNS servers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advert {
    pub up: Uplink6,
    pub dns: Vec<Ipv6Addr>,
}

impl Advert {
    /// The state file's line (`netif`).
    pub fn line(&self) -> String {
        let u = &self.up;
        let mut s = format!(
            "uplink6 {} {}/{} {} {} {}",
            u.index,
            u.addr,
            u.prefix,
            u.temporary.map_or("-".into(), |t| t.to_string()),
            u.gateway,
            u.mtu
        );
        for d in &self.dns {
            s += &format!(" {d}");
        }
        s + "\n"
    }

    pub fn parse(w: &[&str]) -> Option<Advert> {
        let [index, addr, temporary, gateway, mtu, dns @ ..] = w else {
            return None;
        };
        let (a, p) = addr.split_once('/')?;
        Some(Advert {
            up: Uplink6 {
                index: index.parse().ok()?,
                addr: a.parse().ok()?,
                prefix: p.parse().ok()?,
                temporary: match *temporary {
                    "-" => None,
                    t => Some(t.parse().ok()?),
                },
                gateway: gateway.parse().ok()?,
                mtu: mtu.parse().ok()?,
            },
            dns: dns.iter().map(|d| d.parse().ok()).collect::<Option<_>>()?,
        })
    }
}

/// What the virtual router advertises now, or None when the Mac has no
/// IPv6 network (or no network at all).
pub fn advert() -> Option<Advert> {
    let v4 = uplink()?;
    simulate6(&hook(), uplink6(), dns6_servers(), &v4)
}

/// The Mac's IPv6 network `up` and `dns` with the test hook's `text`
/// applied; `v4` is its IPv4 network.
fn simulate6(text: &str, up: Option<Uplink6>, dns: Vec<Ipv6Addr>, v4: &Uplink) -> Option<Advert> {
    let mut a = up.map(|up| Advert { up, dns });
    for line in text.lines() {
        let mut w = line.split_whitespace();
        match (w.next(), w.next()) {
            (Some("down" | "down6"), _) => return None,
            (Some("addr6"), Some(s)) => {
                let (ip, p) = s.split_once('/').unwrap_or((s, "64"));
                if let (Ok(ip), Ok(p @ 0..=128)) = (ip.parse(), p.parse()) {
                    let x = a.get_or_insert_with(|| Advert {
                        up: Uplink6 {
                            index: v4.index,
                            addr: ip,
                            temporary: None,
                            prefix: p,
                            gateway: Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1),
                            mtu: v4.mtu,
                        },
                        dns: Vec::new(),
                    });
                    (x.up.addr, x.up.prefix) = (ip, p);
                }
            }
            (Some("temp6"), Some(t)) => {
                if let (Some(x), Ok(t)) = (a.as_mut(), t.parse()) {
                    x.up.temporary = Some(t);
                }
            }
            (Some("gateway6"), Some(g)) => {
                if let (Some(x), Ok(g)) = (a.as_mut(), g.parse()) {
                    x.up.gateway = g;
                }
            }
            (Some("dns6"), Some(d)) => {
                if let Some(x) = a.as_mut() {
                    x.dns = std::iter::once(d)
                        .chain(w)
                        .filter_map(|d| d.parse().ok())
                        .collect();
                }
            }
            (Some("interface"), Some(n)) => {
                if let (Some(x), i @ 1..) = (a.as_mut(), host_index(n)) {
                    x.up.index = i;
                }
            }
            _ => {}
        }
    }
    a
}

/// `up` and `dns` with the test hook's `text` applied.
fn simulate(text: &str, up: Uplink, dns: Vec<Ipv4Addr>) -> Option<Lease> {
    let mut l = Lease {
        up,
        dns,
        secs: LEASE_SECS,
    };
    for line in text.lines() {
        let mut w = line.split_whitespace();
        match (w.next(), w.next()) {
            (Some("down"), _) => return None,
            (Some("addr"), Some(a)) => {
                let (ip, p) = a.split_once('/').unwrap_or((a, "24"));
                if let (Ok(ip), Ok(p @ 0..=32)) = (ip.parse(), p.parse()) {
                    (l.up.addr, l.up.prefix) = (ip, p);
                }
            }
            (Some("gateway"), Some(g)) => l.up.gateway = g.parse().unwrap_or(l.up.gateway),
            (Some("dns"), Some(d)) => {
                l.dns = std::iter::once(d)
                    .chain(w)
                    .filter_map(|d| d.parse().ok())
                    .collect()
            }
            (Some("lease"), Some(s)) => l.secs = s.parse().unwrap_or(l.secs),
            (Some("interface"), Some(n)) => match host_index(n) {
                0 => {}
                i => l.up.index = i,
            },
            _ => {}
        }
    }
    Some(l)
}

/// Watch the Mac's network from a thread of this process, calling
/// `changed` on each routing change and every `aim_uplink::WATCH_PERIOD`
/// (the test hook changes without a routing message). Every process
/// listening to link changes runs one; the shared device state makes sure
/// each change is announced once (`netif`).
pub fn watch(changed: fn()) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("linux-abi-uplink".into())
        .spawn(move || {
            // SAFETY: this host thread takes no signals, so the kernel
            // never picks it for a process-directed one.
            unsafe {
                let mut all: libc::sigset_t = 0;
                libc::sigfillset(&mut all);
                libc::pthread_sigmask(libc::SIG_BLOCK, &all, std::ptr::null_mut());
            }
            // A routing socket of our own, kept for the process.
            let s = aim_uplink::routing_socket();
            let s = if s >= 0 { super::fdtab::hide(s) } else { s };
            aim_uplink::watch(s, changed)
        });
    if spawned.is_err() {
        STARTED.store(false, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_hook_changes_the_lease() {
        let up = Uplink {
            index: 4,
            addr: Ipv4Addr::new(192, 168, 1, 20),
            prefix: 24,
            gateway: Ipv4Addr::new(192, 168, 1, 1),
            mtu: 1500,
        };
        let dns = vec![Ipv4Addr::new(192, 168, 1, 1)];
        let plain = simulate("", up.clone(), dns.clone()).unwrap();
        assert_eq!((&plain.up, &plain.dns, plain.secs), (&up, &dns, LEASE_SECS));
        assert!(simulate("lease 60\ndown\n", up.clone(), dns.clone()).is_none());
        let l = simulate(
            "addr 10.1.2.3/16\ngateway 10.1.0.1\ndns 9.9.9.9 1.1.1.1\nlease 120\nbogus\n",
            up.clone(),
            dns,
        )
        .unwrap();
        assert_eq!((l.up.addr, l.up.prefix), (Ipv4Addr::new(10, 1, 2, 3), 16));
        assert_eq!(l.up.gateway, Ipv4Addr::new(10, 1, 0, 1));
        assert_eq!(
            l.dns,
            [Ipv4Addr::new(9, 9, 9, 9), Ipv4Addr::new(1, 1, 1, 1)]
        );
        assert_eq!((l.secs, l.up.index), (120, 4));
        let moved = simulate("interface lo0\n", up.clone(), Vec::new()).unwrap();
        assert_eq!(moved.up.index, host_index("lo0"));
        let unknown = simulate("interface no-such-if\n", up, Vec::new()).unwrap();
        assert_eq!(unknown.up.index, 4);
    }

    #[test]
    fn the_test_hook_gives_the_mac_ipv6() {
        let v4 = Uplink {
            index: 4,
            addr: Ipv4Addr::new(192, 168, 1, 20),
            prefix: 24,
            gateway: Ipv4Addr::new(192, 168, 1, 1),
            mtu: 1500,
        };
        assert_eq!(simulate6("", None, Vec::new(), &v4), None);
        let a = simulate6(
            "addr6 2001:db8:1:2::5/64\ntemp6 2001:db8:1:2::77\ndns6 2001:db8::53\n",
            None,
            Vec::new(),
            &v4,
        )
        .unwrap();
        assert_eq!(a.up.index, 4);
        assert_eq!(a.up.addr, "2001:db8:1:2::5".parse::<Ipv6Addr>().unwrap());
        assert_eq!(a.up.prefix, 64);
        assert_eq!(a.up.temporary, Some("2001:db8:1:2::77".parse().unwrap()));
        assert_eq!(a.up.gateway, "fe80::1".parse::<Ipv6Addr>().unwrap());
        assert_eq!(a.dns, ["2001:db8::53".parse::<Ipv6Addr>().unwrap()]);
        assert_eq!(
            simulate6("down6\n", Some(a.up.clone()), Vec::new(), &v4),
            None
        );
        let line = a.line();
        let w: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(Advert::parse(&w[1..]), Some(a));
    }
}
