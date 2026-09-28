//! The far end of `eth0`'s virtual link (docs/network.md): a router that
//! answers DHCP and ARP for the guest, as a virtual machine's NAT network
//! does. Its lease is the Mac's own network (`uplink`): the Mac's address,
//! prefix, gateway, MTU and DNS servers, because guest sockets are host
//! sockets and use exactly those. The lease time is finite
//! (`uplink::LEASE_SECS`), so DhcpClient renews and picks up what changed;
//! its renewals, UDP datagrams to the server's port, come here too
//! ([`client_frame`]) rather than to the real network.
//!
//! Only the guest's DHCP client and ARP requests for the gateway get an
//! answer; every other frame the guest transmits goes nowhere, since no
//! guest traffic crosses this link.

use std::net::Ipv4Addr;

use super::uplink::{Lease, Uplink};

/// The virtual router's hardware address: locally administered, "aim".
pub const ROUTER_MAC: [u8; 6] = [0x02, 0x61, 0x69, 0x6d, 0x00, 0x01];

const ETH_P_IP: u16 = 0x0800;
const ETH_P_ARP: u16 = 0x0806;
const MAGIC: [u8; 4] = [99, 130, 83, 99];

const DISCOVER: u8 = 1;
const OFFER: u8 = 2;
const REQUEST: u8 = 3;
const ACK: u8 = 5;
const NAK: u8 = 6;

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn ip4(b: &[u8], at: usize) -> Option<Ipv4Addr> {
    let v: [u8; 4] = b.get(at..at + 4)?.try_into().ok()?;
    Some(Ipv4Addr::from(v))
}

/// The DHCP server's port.
pub const SERVER_PORT: u16 = 67;
const CLIENT_PORT: u16 = 68;

/// The frame the router sends back for `frame`, if any.
pub fn answer(frame: &[u8], lease: &Lease) -> Option<Vec<u8>> {
    match be16(frame, 12)? {
        ETH_P_ARP => arp(frame, &lease.up),
        ETH_P_IP => dhcp(frame, lease),
        _ => None,
    }
}

/// The frame `eth0` would carry for a DHCP message a UDP socket sent from
/// `src` to the server at `dst` (a renewal to the lease's server, or a
/// rebinding broadcast).
pub fn client_frame(src: Ipv4Addr, dst: Ipv4Addr, msg: &[u8]) -> Vec<u8> {
    let dst_mac = if dst == Ipv4Addr::BROADCAST {
        [0xff; 6]
    } else {
        ROUTER_MAC
    };
    frame(
        dst_mac,
        super::netif::ETH0_MAC,
        (src, CLIENT_PORT),
        (dst, SERVER_PORT),
        msg,
    )
}

/// An ARP request for the gateway gets the router's address.
fn arp(f: &[u8], up: &Uplink) -> Option<Vec<u8>> {
    let a = f.get(14..42)?;
    // Ethernet/IPv4 request.
    if a[..8] != [0, 1, 8, 0, 6, 4, 0, 1] || ip4(a, 24)? != up.gateway {
        return None;
    }
    let mut r = Vec::with_capacity(42);
    r.extend_from_slice(&a[8..14]);
    r.extend_from_slice(&ROUTER_MAC);
    r.extend_from_slice(&ETH_P_ARP.to_be_bytes());
    r.extend_from_slice(&[0, 1, 8, 0, 6, 4, 0, 2]);
    r.extend_from_slice(&ROUTER_MAC);
    r.extend_from_slice(&up.gateway.octets());
    r.extend_from_slice(&a[8..18]);
    Some(r)
}

/// A DHCP client message: DISCOVER gets an OFFER, REQUEST an ACK for the
/// Mac's address or a NAK; the rest is ignored.
fn dhcp(f: &[u8], lease: &Lease) -> Option<Vec<u8>> {
    let up = &lease.up;
    let ihl = ((*f.get(14)? & 0xf) as usize) * 4;
    if f[14] >> 4 != 4 || *f.get(23)? != 17 || be16(f, 14 + ihl + 2)? != SERVER_PORT {
        return None;
    }
    let b = f.get(14 + ihl + 8..)?;
    if b.len() < 240 || b[0] != 1 || b[1] != 1 || b[2] != 6 || b[236..240] != MAGIC {
        return None;
    }
    let (mut kind, mut requested, mut server) = (0, None, None);
    let mut at = 240;
    while at < b.len() && b[at] != 255 {
        if b[at] == 0 {
            at += 1;
            continue;
        }
        let len = *b.get(at + 1)? as usize;
        let v = b.get(at + 2..at + 2 + len)?;
        match (b[at], len) {
            (53, 1) => kind = v[0],
            (50, 4) => requested = ip4(v, 0),
            (54, 4) => server = ip4(v, 0),
            _ => {}
        }
        at += 2 + len;
    }
    let ciaddr = ip4(b, 12)?;
    let reply = match kind {
        DISCOVER => OFFER,
        // Another server's offer was chosen.
        REQUEST if server.is_some_and(|s| s != up.gateway) => return None,
        REQUEST => {
            let want = requested.unwrap_or(ciaddr);
            if want == up.addr { ACK } else { NAK }
        }
        _ => return None,
    };
    Some(reply_frame(b, reply, lease))
}

fn checksum(data: &[u8], mut sum: u32) -> u16 {
    for c in data.chunks(2) {
        sum += u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]) as u32;
    }
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

fn reply_frame(req: &[u8], kind: u8, lease: &Lease) -> Vec<u8> {
    let (up, dns) = (&lease.up, &lease.dns);
    let nak = kind == NAK;
    let broadcast = nak || u16::from_be_bytes([req[10], req[11]]) & 0x8000 != 0;
    let chaddr: [u8; 6] = req[28..34].try_into().unwrap();
    // BOOTP: op, htype, hlen, hops, xid, secs, flags, ciaddr, yiaddr,
    // siaddr, giaddr, chaddr, sname, file.
    let mut b = vec![2u8, 1, 6, 0];
    b.extend_from_slice(&req[4..8]);
    b.extend_from_slice(&[0, 0]);
    b.extend_from_slice(&req[10..12]);
    b.extend_from_slice(if nak { &[0; 4] } else { &req[12..16] });
    b.extend_from_slice(&(if nak { Ipv4Addr::UNSPECIFIED } else { up.addr }).octets());
    b.extend_from_slice(&[0; 8]);
    b.extend_from_slice(&req[28..44]);
    b.resize(236, 0);
    b.extend_from_slice(&MAGIC);
    let mut opt = |code: u8, v: &[u8]| {
        b.push(code);
        b.push(v.len() as u8);
        b.extend_from_slice(v);
    };
    opt(53, &[kind]);
    opt(54, &up.gateway.octets());
    if !nak {
        let mask = u32::MAX.checked_shl(32 - up.prefix as u32).unwrap_or(0);
        opt(51, &lease.secs.to_be_bytes());
        opt(1, &mask.to_be_bytes());
        opt(3, &up.gateway.octets());
        if !dns.is_empty() {
            let v: Vec<u8> = dns.iter().take(63).flat_map(|d| d.octets()).collect();
            opt(6, &v);
        }
        opt(26, &(up.mtu.min(u16::MAX as u32) as u16).to_be_bytes());
        opt(28, &(u32::from(up.addr) | !mask).to_be_bytes());
    }
    b.push(255);
    b.resize(b.len().max(300), 0);

    let dst = if broadcast {
        Ipv4Addr::BROADCAST
    } else {
        up.addr
    };
    let dst_mac = if broadcast { [0xff; 6] } else { chaddr };
    frame(
        dst_mac,
        ROUTER_MAC,
        (up.gateway, SERVER_PORT),
        (dst, CLIENT_PORT),
        &b,
    )
}

/// An Ethernet frame of an IPv4 UDP datagram carrying `payload`.
fn frame(
    dst_mac: [u8; 6],
    src_mac: [u8; 6],
    src: (Ipv4Addr, u16),
    dst: (Ipv4Addr, u16),
    payload: &[u8],
) -> Vec<u8> {
    let udp_len = (8 + payload.len()) as u16;
    let mut udp = Vec::with_capacity(udp_len as usize);
    udp.extend_from_slice(&src.1.to_be_bytes());
    udp.extend_from_slice(&dst.1.to_be_bytes());
    udp.extend_from_slice(&udp_len.to_be_bytes());
    udp.extend_from_slice(&[0, 0]);
    udp.extend_from_slice(payload);
    let mut pseudo = Vec::with_capacity(12);
    pseudo.extend_from_slice(&src.0.octets());
    pseudo.extend_from_slice(&dst.0.octets());
    pseudo.extend_from_slice(&[0, 17]);
    pseudo.extend_from_slice(&udp_len.to_be_bytes());
    let pre = checksum(&pseudo, 0);
    let c = match checksum(&udp, (!pre) as u32) {
        0 => 0xffff,
        c => c,
    };
    udp[6..8].copy_from_slice(&c.to_be_bytes());

    let mut ip = vec![0x45u8, 0];
    ip.extend_from_slice(&(20 + udp_len).to_be_bytes());
    ip.extend_from_slice(&[0, 0, 0, 0, 64, 17, 0, 0]);
    ip.extend_from_slice(&src.0.octets());
    ip.extend_from_slice(&dst.0.octets());
    let c = checksum(&ip, 0);
    ip[10..12].copy_from_slice(&c.to_be_bytes());

    let mut f = Vec::with_capacity(14 + ip.len() + udp.len());
    f.extend_from_slice(&dst_mac);
    f.extend_from_slice(&src_mac);
    f.extend_from_slice(&ETH_P_IP.to_be_bytes());
    f.extend_from_slice(&ip);
    f.extend_from_slice(&udp);
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up() -> Uplink {
        Uplink {
            index: 4,
            addr: Ipv4Addr::new(192, 168, 1, 20),
            prefix: 24,
            gateway: Ipv4Addr::new(192, 168, 1, 1),
            mtu: 1500,
        }
    }

    fn lease(dns: &[Ipv4Addr]) -> Lease {
        Lease {
            up: up(),
            dns: dns.to_vec(),
            secs: 3600,
        }
    }

    const MAC: [u8; 6] = [2, 0x61, 0x69, 0x6d, 0, 2];

    /// A client message as DhcpClient sends it: broadcast from 0.0.0.0.
    fn client(kind: u8, requested: Option<Ipv4Addr>) -> Vec<u8> {
        let mut b = vec![1u8, 1, 6, 0, 0xde, 0xad, 0xbe, 0xef, 0, 0, 0, 0];
        b.resize(28, 0);
        b.extend_from_slice(&MAC);
        b.resize(236, 0);
        b.extend_from_slice(&MAGIC);
        b.extend_from_slice(&[53, 1, kind]);
        if let Some(r) = requested {
            b.extend_from_slice(&[50, 4]);
            b.extend_from_slice(&r.octets());
        }
        b.push(255);
        let mut f = vec![0xff; 6];
        f.extend_from_slice(&MAC);
        f.extend_from_slice(&[8, 0, 0x45, 0]);
        f.extend_from_slice(&((28 + b.len()) as u16).to_be_bytes());
        f.extend_from_slice(&[0, 0, 0, 0, 64, 17, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255]);
        f.extend_from_slice(&[0, 68, 0, 67]);
        f.extend_from_slice(&((8 + b.len()) as u16).to_be_bytes());
        f.extend_from_slice(&[0, 0]);
        f.extend_from_slice(&b);
        f
    }

    fn option(f: &[u8], code: u8) -> Option<Vec<u8>> {
        let b = &f[14 + 20 + 8..];
        let mut at = 240;
        while at < b.len() && b[at] != 255 {
            let len = b[at + 1] as usize;
            if b[at] == code {
                return Some(b[at + 2..at + 2 + len].to_vec());
            }
            at += 2 + len;
        }
        None
    }

    #[test]
    fn discover_gets_an_offer_of_the_macs_network() {
        let dns = [Ipv4Addr::new(9, 9, 9, 9)];
        let r = answer(&client(DISCOVER, None), &lease(&dns)).unwrap();
        assert_eq!(&r[..6], &MAC);
        assert_eq!(option(&r, 53).unwrap(), [OFFER]);
        assert_eq!(&r[14 + 20 + 8 + 16..14 + 20 + 8 + 20], &[192, 168, 1, 20]);
        assert_eq!(option(&r, 1).unwrap(), [255, 255, 255, 0]);
        assert_eq!(option(&r, 3).unwrap(), [192, 168, 1, 1]);
        assert_eq!(option(&r, 6).unwrap(), [9, 9, 9, 9]);
        assert_eq!(option(&r, 51).unwrap(), 3600u32.to_be_bytes());
        // Valid IPv4 and UDP checksums.
        assert_eq!(checksum(&r[14..34], 0), 0);
        let mut pseudo = r[26..34].to_vec();
        pseudo.extend_from_slice(&[0, 17]);
        pseudo.extend_from_slice(&r[38..40]);
        assert_eq!(checksum(&r[34..], (!checksum(&pseudo, 0)) as u32), 0);
    }

    #[test]
    fn request_is_acked_or_naked() {
        let ok = answer(&client(REQUEST, Some(up().addr)), &lease(&[])).unwrap();
        assert_eq!(option(&ok, 53).unwrap(), [ACK]);
        let old = Some(Ipv4Addr::new(10, 0, 0, 7));
        let nak = answer(&client(REQUEST, old), &lease(&[])).unwrap();
        assert_eq!(option(&nak, 53).unwrap(), [NAK]);
        assert_eq!(&nak[..6], &[0xff; 6]);
    }

    #[test]
    fn a_renewal_over_udp_is_answered() {
        // RENEWING: ciaddr is the leased address, no server identifier or
        // requested address, unicast to the server.
        let mut b = client(REQUEST, None)[14 + 20 + 8..].to_vec();
        b[12..16].copy_from_slice(&up().addr.octets());
        let f = client_frame(up().addr, up().gateway, &b);
        assert_eq!(&f[..6], &ROUTER_MAC);
        assert_eq!(checksum(&f[14..34], 0), 0);
        let ack = answer(&f, &lease(&[])).unwrap();
        assert_eq!(option(&ack, 53).unwrap(), [ACK]);
        assert_eq!(&ack[..6], &MAC);
        // The Mac moved to another network: the old address is NAKed.
        let mut moved = lease(&[]);
        moved.up.addr = Ipv4Addr::new(10, 0, 0, 7);
        let nak = answer(&f, &moved).unwrap();
        assert_eq!(option(&nak, 53).unwrap(), [NAK]);
    }

    #[test]
    fn arp_for_the_gateway_is_answered() {
        let mut f = vec![0xff; 6];
        f.extend_from_slice(&MAC);
        f.extend_from_slice(&[8, 6, 0, 1, 8, 0, 6, 4, 0, 1]);
        f.extend_from_slice(&MAC);
        f.extend_from_slice(&[192, 168, 1, 20, 0, 0, 0, 0, 0, 0, 192, 168, 1, 1]);
        let r = answer(&f, &lease(&[])).unwrap();
        assert_eq!(&r[6..12], &ROUTER_MAC);
        assert_eq!(&r[20..22], &[0, 2]);
        f[41] = 9;
        assert!(answer(&f, &lease(&[])).is_none());
    }
}
