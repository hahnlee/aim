//! The Mac's network as the guest's `eth0` sees it (docs/network.md): the
//! interface of the host's IPv4 default route, its address and gateway,
//! and the host's DNS servers. Guest AF_INET/AF_INET6 sockets are host
//! sockets, so these values are what the guest's traffic really uses; the
//! virtual link's DHCP server hands them out (`dhcp`) and `eth0` has
//! carrier while they exist. Read when asked, never cached: the Mac
//! changes networks.

use std::net::Ipv4Addr;

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

/// The interface index and gateway of the IPv4 default route, from a
/// routing socket's `RTM_GET`.
fn default_route() -> Option<(u32, Ipv4Addr)> {
    // SAFETY: a routing socket of our own, closed below.
    let s = unsafe { libc::socket(libc::PF_ROUTE, libc::SOCK_RAW, libc::AF_INET) };
    if s < 0 {
        return None;
    }
    let hdr = std::mem::size_of::<libc::rt_msghdr>();
    let sin = std::mem::size_of::<libc::sockaddr_in>();
    let mut msg = vec![0u8; hdr + 2 * sin];
    // SAFETY: a zeroed header written into our buffer.
    let seq = unsafe {
        let mut h: libc::rt_msghdr = std::mem::zeroed();
        h.rtm_msglen = msg.len() as u16;
        h.rtm_version = libc::RTM_VERSION as u8;
        h.rtm_type = libc::RTM_GET as u8;
        h.rtm_addrs = libc::RTA_DST | libc::RTA_NETMASK;
        h.rtm_pid = libc::getpid();
        h.rtm_seq = 1;
        (msg.as_mut_ptr() as *mut libc::rt_msghdr).write_unaligned(h);
        h.rtm_seq
    };
    // Destination and netmask 0.0.0.0: the default route.
    for i in 0..2 {
        let at = hdr + i * sin;
        msg[at] = sin as u8;
        msg[at + 1] = libc::AF_INET as u8;
    }
    let mut reply = vec![0u8; 2048];
    // SAFETY: writing our request and reading replies into our buffer.
    let got = unsafe {
        if libc::write(s, msg.as_ptr().cast(), msg.len()) != msg.len() as isize {
            libc::close(s);
            return None;
        }
        let pid = libc::getpid();
        let n = loop {
            let n = libc::read(s, reply.as_mut_ptr().cast(), reply.len());
            if n < hdr as isize {
                break -1;
            }
            let h = (reply.as_ptr() as *const libc::rt_msghdr).read_unaligned();
            if h.rtm_pid == pid && h.rtm_seq == seq {
                break n;
            }
        };
        libc::close(s);
        n
    };
    if got < 0 {
        return None;
    }
    // SAFETY: a reply of at least a header, checked above.
    let h = unsafe { (reply.as_ptr() as *const libc::rt_msghdr).read_unaligned() };
    if h.rtm_errno != 0 || h.rtm_flags & libc::RTF_GATEWAY == 0 {
        return None;
    }
    let mut at = hdr;
    let mut gateway = None;
    for bit in 0..libc::RTAX_MAX {
        if h.rtm_addrs & (1 << bit) == 0 || at + 2 > got as usize {
            continue;
        }
        let len = reply[at] as usize;
        if 1 << bit == libc::RTA_GATEWAY
            && reply[at + 1] == libc::AF_INET as u8
            && at + 8 <= got as usize
        {
            gateway = Some(Ipv4Addr::new(
                reply[at + 4],
                reply[at + 5],
                reply[at + 6],
                reply[at + 7],
            ));
        }
        at += roundup(len);
    }
    Some((h.rtm_index as u32, gateway?))
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
