//! The Mac's network as Android sees it (docs/network.md): the interface
//! of the host's IPv4 default route, its address and gateway, and the
//! host's DNS servers. Guest AF_INET/AF_INET6 sockets are host sockets, so
//! these values are what the guest's traffic really uses: the guest's
//! `eth0` stands for them (aim-linux-abi's `sys/uplink.rs`), and so does
//! the app host's network (`connectivity`). Read when asked, never
//! cached: the Mac changes networks, and [`watch`] hears of it.

use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

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
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOMEM) {
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

/// How long a watcher waits for a routing message before it looks
/// anyway: DNS servers change without one.
pub const WATCH_PERIOD: Duration = Duration::from_millis(2000);

/// A routing socket (`PF_ROUTE`) for [`watch`], or -1. A routing socket
/// needs no entitlement and no run loop, unlike SCDynamicStore or
/// `nw_path_monitor`.
pub fn routing_socket() -> i32 {
    // SAFETY: a socket of the caller's.
    unsafe { libc::socket(libc::PF_ROUTE, libc::SOCK_RAW, 0) }
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

/// Watch the Mac's network on this thread: `changed` on each routing
/// change and every [`WATCH_PERIOD`]. `socket` is a [`routing_socket`];
/// without one (-1) it only looks every period.
pub fn watch(socket: i32, mut changed: impl FnMut()) -> ! {
    let mut buf = vec![0u8; 2048];
    let mut due = Instant::now();
    loop {
        let now = Instant::now();
        if now >= due {
            changed();
            due = now + WATCH_PERIOD;
        }
        let mut p = libc::pollfd {
            fd: socket,
            events: libc::POLLIN,
            revents: 0,
        };
        let wait = due.saturating_duration_since(now).as_millis() as i32;
        // SAFETY: one pollfd on our stack (none: a plain sleep).
        if unsafe { libc::poll(&mut p, (socket >= 0) as u32, wait) } <= 0 {
            continue;
        }
        // A change comes as a burst of messages: let it finish, then
        // look once.
        std::thread::sleep(Duration::from_millis(200));
        loop {
            // SAFETY: reading into our buffer without blocking.
            let n = unsafe {
                libc::recv(
                    socket,
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    libc::MSG_DONTWAIT,
                )
            };
            if n <= 0 {
                break;
            }
            if relevant(&buf[..n as usize]) {
                due = Instant::now();
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
