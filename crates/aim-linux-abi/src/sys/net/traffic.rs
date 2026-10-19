//! netd's traffic accounting: what its `cgroupskb/ingress/stats` and
//! `cgroupskb/egress/stats` programs (`bpf_traffic_account`, bpf/progs/
//! netd.c in Connectivity at android-16.0.0_r1) do for each packet of a
//! socket, applied where the kernel would run them. Guest sockets are host
//! sockets, so the layer sees their sends and receives, not their packets:
//! one call that moves `n` bytes counts as one packet of `n` bytes behind
//! its IP and transport headers, which the programs' GSO correction splits
//! into MTU-sized packets as it splits a large skb. TCP's acknowledgements
//! and other control packets are not counted.
//!
//! The effect applies while NetBpfLoad's netd maps are pinned (ADR 0012
//! appendix, "Kernel features to emulate"). netd's firewall
//! (`bpf_owner_match`) and its local network protection do not run, so no
//! packet is dropped uncounted; the interface counters that netd's xt_bpf
//! programs keep (`iface_stats_map`) are #1720.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::super::bpf::Kernel;
use super::super::netif;
use super::{Family, Sock, SockType};

const COOKIE_TAG: &[u8] = b"/sys/fs/bpf/netd_shared/map_netd_cookie_tag_map";
const UID_COUNTERSET: &[u8] = b"/sys/fs/bpf/netd_shared/map_netd_uid_counterset_map";
const APP_UID_STATS: &[u8] = b"/sys/fs/bpf/netd_shared/map_netd_app_uid_stats_map";
const STATS_A: &[u8] = b"/sys/fs/bpf/netd_shared/map_netd_stats_map_A";
const STATS_B: &[u8] = b"/sys/fs/bpf/netd_shared/map_netd_stats_map_B";
const CONFIGURATION: &[u8] = b"/sys/fs/bpf/netd_shared/map_netd_configuration_map";

/// bpf_shared.h: `CURRENT_STATS_MAP_CONFIGURATION_KEY`, `SELECT_MAP_A`.
const CURRENT_STATS_MAP_CONFIGURATION_KEY: u32 = 1;
const SELECT_MAP_A: u32 = 0;
const AID_DNS: u32 = 1051;
const AID_CLAT: u32 = 1029;
/// The kernel's overflow uid, which the programs count as root's.
const OVERFLOW_UID: u32 = 65534;
const TAG_SYSTEM_DNS: u32 = 0xFFFF_FF82;
const MTU: u64 = 1500;
/// How long a process that found no maps waits before it looks again.
const RETRY: Duration = Duration::from_secs(1);

/// netd's maps, as its programs reach them.
#[derive(Clone, Copy)]
struct Maps {
    cookie_tag: Kernel,
    uid_counterset: Kernel,
    app_uid_stats: Kernel,
    stats_a: Kernel,
    stats_b: Kernel,
    configuration: Kernel,
}

impl Maps {
    fn pinned() -> Option<Maps> {
        Some(Maps {
            cookie_tag: Kernel::pinned(COOKIE_TAG)?,
            uid_counterset: Kernel::pinned(UID_COUNTERSET)?,
            app_uid_stats: Kernel::pinned(APP_UID_STATS)?,
            stats_a: Kernel::pinned(STATS_A)?,
            stats_b: Kernel::pinned(STATS_B)?,
            configuration: Kernel::pinned(CONFIGURATION)?,
        })
    }
}

/// This process's view of the maps: found, or when it last looked.
static MAPS: Mutex<(Option<Maps>, Option<Instant>)> = Mutex::new((None, None));

fn maps() -> Option<Maps> {
    let mut g = MAPS.lock().unwrap();
    if g.0.is_none() && g.1.is_none_or(|t| t.elapsed() >= RETRY) {
        g.0 = Maps::pinned();
        g.1 = Some(Instant::now());
    }
    g.0
}

/// One packet as the programs see it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Packet {
    sock_uid: u32,
    cookie: u64,
    ifindex: u32,
    ipv6: bool,
    /// The transport header's bytes.
    transport: u64,
    egress: bool,
    /// `skb->len` less the headers.
    payload: u64,
}

impl Packet {
    /// `DEFINE_UPDATE_STATS`'s packets and bytes for this skb.
    fn counts(&self) -> (u64, u64) {
        let ip = if self.ipv6 { 40 } else { 20 };
        let len = ip + self.transport + self.payload;
        if len <= MTU {
            return (1, len);
        }
        let overhead = ip + 20 + 12;
        let mss = MTU - overhead;
        let payload = len - overhead;
        let packets = payload.div_ceil(mss);
        (packets, overhead * packets + payload)
    }
}

/// `StatsValue`: rxPackets, rxBytes, txPackets, txBytes.
fn add(value: &mut [u8], egress: bool, (packets, bytes): (u64, u64)) {
    let at = if egress { 16 } else { 0 };
    for (i, n) in [(at, packets), (at + 8, bytes)] {
        let v = u64::from_ne_bytes(value[i..i + 8].try_into().unwrap());
        value[i..i + 8].copy_from_slice(&v.wrapping_add(n).to_ne_bytes());
    }
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// `bpf_traffic_account` for one packet.
fn account_packet(maps: &Maps, p: &Packet) {
    let sock_uid = if p.sock_uid == OVERFLOW_UID {
        0
    } else {
        p.sock_uid
    };
    // UidTagValue: uid, tag.
    let (mut uid, tag) = match maps.cookie_tag.lookup(&p.cookie.to_ne_bytes()) {
        Some(v) => (
            u32_at(&v, 0).unwrap_or(sock_uid),
            u32_at(&v, 4).unwrap_or(0),
        ),
        None => (sock_uid, 0),
    };
    if p.egress && uid == AID_CLAT {
        return;
    }
    if tag == TAG_SYSTEM_DNS && uid == AID_DNS {
        uid = sock_uid;
    }
    let counter_set = maps
        .uid_counterset
        .lookup(&uid.to_ne_bytes())
        .map_or(0, |v| v[0] as u32);
    let Some(selected) = maps
        .configuration
        .lookup(&CURRENT_STATS_MAP_CONFIGURATION_KEY.to_ne_bytes())
        .and_then(|v| u32_at(&v, 0))
    else {
        return;
    };
    // StatsKey: uid, tag, counterSet, ifaceIndex.
    let mut key = [0u8; 16];
    for (i, v) in [uid, tag, counter_set, p.ifindex].into_iter().enumerate() {
        key[i * 4..i * 4 + 4].copy_from_slice(&v.to_ne_bytes());
    }
    let stats = if selected == SELECT_MAP_A {
        &maps.stats_a
    } else {
        &maps.stats_b
    };
    let counts = p.counts();
    stats.update(&key, |v| add(v, p.egress, counts));
    maps.app_uid_stats
        .update(&uid.to_ne_bytes(), |v| add(v, p.egress, counts));
}

/// The interface and IP version of a packet to or from `addr` (a Linux
/// sockaddr), or, without one, of the socket's peer or local name: the
/// loopback for loopback addresses, `eth0` for the rest.
fn route(fd: i32, addr: Option<&[u8]>) -> Option<(u32, bool)> {
    let classify = |v6: bool, sa: &[u8]| -> Option<(u32, bool)> {
        let (ip, v6) = if v6 {
            let ip = sa.get(8..24)?;
            if ip[..10].iter().all(|&b| b == 0) && ip[10..12] == [0xff, 0xff] {
                (&ip[12..16], false)
            } else {
                (ip, true)
            }
        } else {
            (sa.get(4..8)?, false)
        };
        let lo = if v6 {
            ip[..15].iter().all(|&b| b == 0) && ip[15] == 1
        } else {
            ip[0] == 127
        };
        Some((if lo { netif::LO } else { netif::ETH0 }, v6))
    };
    if let Some(sa) = addr {
        return match u16::from_ne_bytes(sa.get(..2)?.try_into().ok()?) {
            2 => classify(false, sa),
            10 => classify(true, sa),
            _ => None,
        };
    }
    // SAFETY: an all-zero sockaddr_storage is valid.
    let mut ss: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let name = |peer: bool, ss: &mut libc::sockaddr_storage| {
        let mut len = std::mem::size_of::<libc::sockaddr_storage>() as u32;
        let p = (ss as *mut libc::sockaddr_storage).cast();
        // SAFETY: the name into local storage of its size.
        (unsafe {
            if peer {
                libc::getpeername(fd, p, &mut len)
            } else {
                libc::getsockname(fd, p, &mut len)
            }
        }) == 0
    };
    if !name(true, &mut ss) && !name(false, &mut ss) {
        return None;
    }
    // SAFETY: the host sockaddr's bytes.
    let sa = unsafe {
        std::slice::from_raw_parts((&raw const ss).cast::<u8>(), std::mem::size_of_val(&ss))
    };
    match ss.ss_family as i32 {
        libc::AF_INET => classify(false, sa),
        libc::AF_INET6 => classify(true, sa),
        _ => None,
    }
}

/// `n` bytes sent (`egress`) or received on AF_INET/AF_INET6 socket `fd`,
/// to or from `addr` when the call names it.
pub(super) fn account(s: &Sock, fd: i32, egress: bool, n: i64, addr: Option<&[u8]>) {
    let Family::Inet(o) = &s.family else {
        return;
    };
    if n <= 0 {
        return;
    }
    let Some(owner) = *s.inet_owner.lock().unwrap() else {
        return;
    };
    let Some(maps) = maps() else {
        return;
    };
    let Some((ifindex, ipv6)) = route(fd, addr) else {
        return;
    };
    let transport = match s.ty {
        // With the timestamp option, as the programs assume.
        SockType::Stream => 32,
        SockType::Dgram if !o.icmp4.load(std::sync::atomic::Ordering::Relaxed) => 8,
        _ => 0,
    };
    account_packet(
        &maps,
        &Packet {
            sock_uid: owner.uid,
            cookie: owner.cookie,
            ifindex,
            ipv6,
            transport,
            egress,
            payload: n as u64,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(ty: u32, key: u32, value: u32, entries: u32) -> Kernel {
        let mut attr = [ty, key, value, entries, 0, 0, 0, 0, 0, 0, 0, 0];
        let fd = super::super::super::bpf::bpf([0, attr.as_mut_ptr() as u64, 48, 0, 0, 0]);
        assert!(fd >= 0, "{fd}");
        Kernel::of(fd as i32).unwrap()
    }

    fn maps() -> Maps {
        Maps {
            cookie_tag: map(1, 8, 8, 16),
            uid_counterset: map(1, 4, 1, 16),
            app_uid_stats: map(1, 4, 32, 16),
            stats_a: map(1, 16, 32, 16),
            stats_b: map(1, 16, 32, 16),
            configuration: map(2, 4, 4, 2),
        }
    }

    fn stats(v: Option<Vec<u8>>) -> [u64; 4] {
        let v = v.unwrap();
        std::array::from_fn(|i| u64::from_ne_bytes(v[i * 8..i * 8 + 8].try_into().unwrap()))
    }

    fn key(uid: u32, tag: u32, set: u32, ifindex: u32) -> Vec<u8> {
        [uid, tag, set, ifindex]
            .iter()
            .flat_map(|v| v.to_ne_bytes())
            .collect()
    }

    const TCP: Packet = Packet {
        sock_uid: 10100,
        cookie: 7,
        ifindex: 2,
        ipv6: false,
        transport: 32,
        egress: true,
        payload: 100,
    };

    #[test]
    fn packets_count_their_headers_and_split_as_gso() {
        assert_eq!(TCP.counts(), (1, 152));
        assert_eq!(Packet { ipv6: true, ..TCP }.counts(), (1, 172));
        // 1448 bytes of payload per 1500-byte segment.
        assert_eq!(
            Packet {
                payload: 1448,
                ..TCP
            }
            .counts(),
            (1, 1500)
        );
        assert_eq!(
            Packet {
                payload: 1449,
                ..TCP
            }
            .counts(),
            (2, 1449 + 2 * 52)
        );
        assert_eq!(
            Packet {
                payload: 1_000_000,
                ..TCP
            }
            .counts(),
            (691, 1_000_000 + 691 * 52)
        );
        assert_eq!(
            Packet {
                transport: 8,
                payload: 1472,
                ..TCP
            }
            .counts(),
            (1, 1500)
        );
    }

    #[test]
    fn traffic_counts_for_the_uid_in_the_selected_map() {
        let m = maps();
        account_packet(&m, &TCP);
        account_packet(
            &m,
            &Packet {
                egress: false,
                payload: 1000,
                ..TCP
            },
        );
        assert_eq!(
            stats(m.app_uid_stats.lookup(&10100u32.to_ne_bytes())),
            [1, 1052, 1, 152]
        );
        assert_eq!(
            stats(m.stats_a.lookup(&key(10100, 0, 0, 2))),
            [1, 1052, 1, 152]
        );
        assert_eq!(m.stats_b.lookup(&key(10100, 0, 0, 2)), None);
        // The overflow uid counts as root's.
        account_packet(
            &m,
            &Packet {
                sock_uid: OVERFLOW_UID,
                ..TCP
            },
        );
        assert_eq!(
            stats(m.app_uid_stats.lookup(&0u32.to_ne_bytes())),
            [0, 0, 1, 152]
        );
    }

    #[test]
    fn a_tag_counter_set_and_map_selection_steer_the_counters() {
        let m = maps();
        assert!(
            m.configuration
                .update(&CURRENT_STATS_MAP_CONFIGURATION_KEY.to_ne_bytes(), |v| v
                    .copy_from_slice(&1u32.to_ne_bytes()))
        );
        // Tagged for another uid, which is in the foreground set.
        let tag = [10200u32.to_ne_bytes(), 42u32.to_ne_bytes()].concat();
        assert!(
            m.cookie_tag
                .update(&7u64.to_ne_bytes(), |v| v.copy_from_slice(&tag))
        );
        assert!(
            m.uid_counterset
                .update(&10200u32.to_ne_bytes(), |v| v[0] = 1)
        );
        account_packet(&m, &TCP);
        assert_eq!(
            stats(m.stats_b.lookup(&key(10200, 42, 1, 2))),
            [0, 0, 1, 152]
        );
        assert_eq!(
            stats(m.app_uid_stats.lookup(&10200u32.to_ne_bytes())),
            [0, 0, 1, 152]
        );
        assert_eq!(m.app_uid_stats.lookup(&10100u32.to_ne_bytes()), None);
        // DNS on an app's behalf counts for the socket's uid; clat's own
        // sends not at all.
        let dns = [AID_DNS.to_ne_bytes(), TAG_SYSTEM_DNS.to_ne_bytes()].concat();
        assert!(
            m.cookie_tag
                .update(&8u64.to_ne_bytes(), |v| v.copy_from_slice(&dns))
        );
        account_packet(&m, &Packet { cookie: 8, ..TCP });
        assert_eq!(
            stats(m.app_uid_stats.lookup(&10100u32.to_ne_bytes())),
            [0, 0, 1, 152]
        );
        let clat = [AID_CLAT.to_ne_bytes(), 0u32.to_ne_bytes()].concat();
        assert!(
            m.cookie_tag
                .update(&9u64.to_ne_bytes(), |v| v.copy_from_slice(&clat))
        );
        account_packet(&m, &Packet { cookie: 9, ..TCP });
        assert_eq!(m.app_uid_stats.lookup(&AID_CLAT.to_ne_bytes()), None);
    }

    #[test]
    fn loopback_addresses_route_to_lo() {
        let v4 = |a: [u8; 4]| [&2u16.to_ne_bytes()[..], &[0, 80], &a].concat();
        let v6 = |a: [u8; 16]| [&10u16.to_ne_bytes()[..], &[0; 6], &a].concat();
        assert_eq!(
            route(-1, Some(&v4([127, 0, 0, 1]))),
            Some((netif::LO, false))
        );
        assert_eq!(
            route(-1, Some(&v4([93, 184, 216, 34]))),
            Some((netif::ETH0, false))
        );
        let mut lo6 = [0u8; 16];
        lo6[15] = 1;
        assert_eq!(route(-1, Some(&v6(lo6))), Some((netif::LO, true)));
        let mut mapped = [0u8; 16];
        mapped[10..].copy_from_slice(&[0xff, 0xff, 127, 0, 0, 1]);
        assert_eq!(route(-1, Some(&v6(mapped))), Some((netif::LO, false)));
        mapped[12] = 10;
        assert_eq!(route(-1, Some(&v6(mapped))), Some((netif::ETH0, false)));
        assert_eq!(
            route(
                -1,
                Some(&v6([
                    0x20, 1, 0xd, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1
                ]))
            ),
            Some((netif::ETH0, true))
        );
    }
}
