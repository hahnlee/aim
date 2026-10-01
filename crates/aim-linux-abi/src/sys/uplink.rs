//! The Mac's network as the guest's `eth0` sees it (docs/network.md): the
//! interface of the host's IPv4 default route, its address and gateway,
//! and the host's DNS servers. Guest AF_INET/AF_INET6 sockets are host
//! sockets, so these values are what the guest's traffic really uses; the
//! virtual link's DHCP server hands them out (`dhcp`) and `eth0` has
//! carrier while they exist. Read when asked, never cached: the Mac
//! changes networks, and [`watch`] makes sure the guest hears of it.
//!
//! A boot's test hook, `<runtime>/net/simulate`, changes what the guest
//! sees without touching the Mac's network: a line `down` takes the
//! network away; `addr A/P`, `gateway G`, `dns S...` and `lease SECONDS`
//! replace those values of a present one.

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::errno;

/// The host's primary IPv4 network.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uplink {
    /// Host interface index (for `IP_BOUND_IF`).
    pub index: u32,
    pub addr: Ipv4Addr,
    pub prefix: u8,
    pub gateway: Ipv4Addr,
    pub mtu: u32,
}

/// Round a routing-socket address length up to Darwin's alignment.
fn roundup(len: usize) -> usize {
    if len == 0 { 4 } else { (len + 3) & !3 }
}

/// The interface index and gateway of the IPv4 default route: the
/// unscoped one of the kernel's routing table, read with `sysctl`
/// (NET_RT_FLAGS), as netstat(1) reads it. A routing socket's RTM_GET
/// reply can be lost among other routing messages, and reading it blocks.
fn default_route() -> Option<(u32, Ipv4Addr)> {
    let mut mib = [
        libc::CTL_NET,
        libc::PF_ROUTE,
        0,
        libc::AF_INET,
        libc::NET_RT_FLAGS,
        libc::RTF_GATEWAY,
    ];
    let mut table = Vec::new();
    loop {
        let mut len = 0usize;
        // SAFETY: sizing call.
        let r = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                6,
                std::ptr::null_mut(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if r < 0 {
            return None;
        }
        // Room for routes added since.
        len += len / 2 + 1024;
        table.resize(len, 0u8);
        // SAFETY: sysctl into our buffer of `len` bytes.
        let r = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                6,
                table.as_mut_ptr().cast(),
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if r == 0 {
            table.truncate(len);
            break;
        }
        if errno::last() != errno::ENOMEM {
            return None;
        }
    }
    let hdr = std::mem::size_of::<libc::rt_msghdr>();
    let mut at = 0;
    while at + hdr <= table.len() {
        // SAFETY: a whole header inside the table.
        let h = unsafe { (table.as_ptr().add(at) as *const libc::rt_msghdr).read_unaligned() };
        let end = (at + h.rtm_msglen as usize).min(table.len());
        if h.rtm_msglen == 0 {
            break;
        }
        if h.rtm_flags & (libc::RTF_IFSCOPE | libc::RTF_HOST) == 0
            && let Some(gateway) = default_gateway(h.rtm_addrs, &table[at + hdr..end])
        {
            return Some((h.rtm_index as u32, gateway));
        }
        at = end;
    }
    None
}

/// The IPv4 gateway of a route whose addresses (`addrs`, an RTA_ mask)
/// are `sa`, if it is a default route: destination 0.0.0.0, mask 0.
fn default_gateway(addrs: i32, sa: &[u8]) -> Option<Ipv4Addr> {
    let (mut at, mut gateway, mut default) = (0, None, false);
    for bit in 0..libc::RTAX_MAX {
        if addrs & (1 << bit) == 0 {
            continue;
        }
        let len = *sa.get(at)? as usize;
        let addr = sa.get(at..at + len)?;
        // A mask is stored without its trailing zero bytes.
        let zero = addr.get(4..).is_none_or(|a| a.iter().all(|&b| b == 0));
        match 1 << bit {
            libc::RTA_DST => default = addr.get(1) == Some(&(libc::AF_INET as u8)) && zero,
            libc::RTA_NETMASK => default &= zero,
            libc::RTA_GATEWAY if addr.get(1) == Some(&(libc::AF_INET as u8)) => {
                gateway = addr
                    .get(4..8)
                    .map(|a| Ipv4Addr::new(a[0], a[1], a[2], a[3]));
            }
            _ => {}
        }
        at += roundup(len);
    }
    gateway.filter(|_| default)
}

fn prefix_of(mask: u32) -> u8 {
    mask.leading_ones() as u8
}

/// The host's primary IPv4 network, or None when the Mac has none.
pub fn uplink() -> Option<Uplink> {
    let (index, gateway) = default_route()?;
    let mut ifa: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs into a list we free below.
    if unsafe { libc::getifaddrs(&mut ifa) } != 0 {
        return None;
    }
    let mut name = [0 as libc::c_char; libc::IF_NAMESIZE];
    // SAFETY: if_indextoname into a buffer of IF_NAMESIZE.
    if unsafe { libc::if_indextoname(index, name.as_mut_ptr()) }.is_null() {
        // SAFETY: freeing the list we got.
        unsafe { libc::freeifaddrs(ifa) };
        return None;
    }
    let (mut addr, mut prefix, mut mtu) = (None, 0, 1500);
    let mut p = ifa;
    // SAFETY: walking the list getifaddrs returned.
    unsafe {
        while !p.is_null() {
            let e = &*p;
            p = e.ifa_next;
            if e.ifa_addr.is_null() || libc::strcmp(e.ifa_name, name.as_ptr()) != 0 {
                continue;
            }
            match (*e.ifa_addr).sa_family as i32 {
                libc::AF_INET if addr.is_none() => {
                    let a = &*(e.ifa_addr as *const libc::sockaddr_in);
                    addr = Some(Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr)));
                    if !e.ifa_netmask.is_null() {
                        let m = &*(e.ifa_netmask as *const libc::sockaddr_in);
                        prefix = prefix_of(u32::from_be(m.sin_addr.s_addr));
                    }
                }
                libc::AF_LINK if !e.ifa_data.is_null() => {
                    mtu = (*(e.ifa_data as *const libc::if_data)).ifi_mtu;
                }
                _ => {}
            }
        }
        libc::freeifaddrs(ifa);
    }
    Some(Uplink {
        index,
        addr: addr?,
        prefix,
        gateway,
        mtu,
    })
}

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

/// How long the watcher waits for a routing message before it looks
/// anyway: DNS servers and the test hook change without one.
const WATCH_PERIOD_MS: u64 = 2000;

/// Watch the Mac's network from a thread of this process, calling
/// `changed` on each routing change and every [`WATCH_PERIOD_MS`]. Every
/// process listening to link changes runs one; the shared device state
/// makes sure each change is announced once (`netif`).
pub fn watch(changed: fn()) {
    static STARTED: AtomicBool = AtomicBool::new(false);
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("linux-abi-uplink".into())
        .spawn(move || watch_loop(changed));
    if spawned.is_err() {
        STARTED.store(false, Ordering::Relaxed);
    }
}

/// Whether a routing message may change the default route or the
/// addresses: route changes other than cloned, host and link-layer
/// entries, address and interface changes.
fn relevant(msg: &[u8]) -> bool {
    if msg.len() < std::mem::size_of::<libc::rt_msghdr>() {
        return false;
    }
    // SAFETY: a whole header, checked above.
    let h = unsafe { (msg.as_ptr() as *const libc::rt_msghdr).read_unaligned() };
    let skip = libc::RTF_WASCLONED | libc::RTF_HOST | libc::RTF_LLINFO;
    match h.rtm_type as i32 {
        libc::RTM_ADD | libc::RTM_DELETE | libc::RTM_CHANGE => h.rtm_flags & skip == 0,
        libc::RTM_NEWADDR | libc::RTM_DELADDR | libc::RTM_IFINFO => true,
        _ => false,
    }
}

fn watch_loop(changed: fn()) {
    // SAFETY: this host thread takes no signals, so the kernel never picks
    // it for a process-directed one.
    unsafe {
        let mut all: libc::sigset_t = 0;
        libc::sigfillset(&mut all);
        libc::pthread_sigmask(libc::SIG_BLOCK, &all, std::ptr::null_mut());
    }
    // SAFETY: a routing socket of our own, kept for the process.
    let s = unsafe { libc::socket(libc::PF_ROUTE, libc::SOCK_RAW, 0) };
    let s = if s >= 0 { super::fdtab::hide(s) } else { s };
    let mut buf = vec![0u8; 2048];
    let period = std::time::Duration::from_millis(WATCH_PERIOD_MS);
    let mut due = std::time::Instant::now();
    loop {
        let now = std::time::Instant::now();
        if now >= due {
            changed();
            due = now + period;
        }
        let mut p = libc::pollfd {
            fd: s,
            events: libc::POLLIN,
            revents: 0,
        };
        let wait = due.saturating_duration_since(now).as_millis() as i32;
        // SAFETY: one pollfd on our stack (none: a plain sleep).
        if unsafe { libc::poll(&mut p, (s >= 0) as u32, wait) } <= 0 {
            continue;
        }
        // A change comes as a burst of messages: let it finish, then
        // look once.
        std::thread::sleep(std::time::Duration::from_millis(200));
        loop {
            // SAFETY: reading into our buffer without blocking.
            let n =
                unsafe { libc::recv(s, buf.as_mut_ptr().cast(), buf.len(), libc::MSG_DONTWAIT) };
            if n <= 0 {
                break;
            }
            if relevant(&buf[..n as usize]) {
                due = std::time::Instant::now();
            }
        }
    }
}

/// The host's IPv4 DNS servers (`/etc/resolv.conf`, which macOS writes
/// from its primary resolver configuration).
pub fn dns_servers() -> Vec<Ipv4Addr> {
    std::fs::read_to_string("/etc/resolv.conf")
        .map(|t| parse_nameservers(&t))
        .unwrap_or_default()
}

fn parse_nameservers(text: &str) -> Vec<Ipv4Addr> {
    text.lines()
        .filter_map(|l| {
            let mut w = l.split_whitespace();
            (w.next()? == "nameserver").then(|| w.next()?.parse().ok())?
        })
        .collect()
}

/// A host interface's index by name (0 when there is none).
pub fn host_index(name: &str) -> u32 {
    let Ok(c) = std::ffi::CString::new(name) else {
        return 0;
    };
    // SAFETY: a NUL-terminated name.
    unsafe { libc::if_nametoindex(c.as_ptr()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sin(a: [u8; 4]) -> Vec<u8> {
        let mut v = vec![16, libc::AF_INET as u8, 0, 0];
        v.extend(a);
        v.extend([0; 8]);
        v
    }

    #[test]
    fn a_default_route_has_destination_and_mask_0() {
        let addrs = libc::RTA_DST | libc::RTA_GATEWAY | libc::RTA_NETMASK;
        let gw = sin([172, 30, 1, 254]);
        // The kernel stores a 0 mask as an empty address.
        let default = [sin([0; 4]), gw.clone(), vec![0, 0, 0, 0]].concat();
        assert_eq!(
            default_gateway(addrs, &default),
            Some(Ipv4Addr::new(172, 30, 1, 254))
        );
        let net = [
            sin([10, 0, 0, 0]),
            gw.clone(),
            vec![5, 0, 0, 0, 255, 0, 0, 0],
        ]
        .concat();
        assert_eq!(default_gateway(addrs, &net), None);
        let narrow = [sin([0; 4]), gw, vec![5, 0, 0, 0, 128, 0, 0, 0]].concat();
        assert_eq!(default_gateway(addrs, &narrow), None);
    }

    #[test]
    fn the_default_route_is_the_one_route_8_gets() {
        let out = std::process::Command::new("/sbin/route")
            .args(["-n", "get", "default"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let field = |k: &str| {
            text.lines()
                .find_map(|l| l.trim().strip_prefix(k)?.strip_prefix(": "))
                .map(str::to_string)
        };
        let want = field("gateway").and_then(|g| {
            let index = std::ffi::CString::new(field("interface")?).ok()?;
            // SAFETY: a NUL-terminated interface name.
            Some((
                unsafe { libc::if_nametoindex(index.as_ptr()) },
                g.parse().ok()?,
            ))
        });
        assert_eq!(default_route(), want, "{text}");
    }

    #[test]
    fn nameservers_skip_comments_and_ipv6() {
        let t = "# macOS\nnameserver 192.168.0.1\nnameserver fe80::1%en0\nsearch lan\nnameserver 8.8.8.8\n";
        assert_eq!(
            parse_nameservers(t),
            [Ipv4Addr::new(192, 168, 0, 1), Ipv4Addr::new(8, 8, 8, 8)]
        );
    }

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

    #[test]
    fn prefix_from_mask() {
        assert_eq!(prefix_of(0xffff_ff00), 24);
        assert_eq!(prefix_of(0), 0);
    }

    #[test]
    fn uplink_matches_the_default_route() {
        // A Mac without a network has none; one with a network has an
        // address inside its gateway's subnet (or a point-to-point one).
        if let Some(u) = uplink() {
            assert!(u.index > 0 && u.prefix <= 32 && u.mtu >= 576, "{u:?}");
        }
    }
}
