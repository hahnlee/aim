//! The Mac's network as the guest's `eth0` sees it (docs/network.md):
//! `aim_uplink`'s default route, address, gateway and DNS servers, which
//! the virtual link's DHCP server hands out (`dhcp`); `eth0` has carrier
//! while they exist, and [`watch`] makes sure the guest hears of a change.
//!
//! A boot's test hook, `<runtime>/net/simulate`, changes what the guest
//! sees without touching the Mac's network: a line `down` takes the
//! network away; `addr A/P`, `gateway G`, `dns S...` and `lease SECONDS`
//! replace those values of a present one.

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};

pub use aim_uplink::{Uplink, host_index};
use aim_uplink::{dns_servers, uplink};

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
    }
}
