//! AF_PACKET, SOCK_RAW: whole Ethernet frames on the kernel's devices
//! (`netif`), for NetworkStack's DHCP client and packet tracker.
//!
//! No guest traffic crosses `eth0` (guest sockets are host sockets), so
//! its link carries only what packet sockets transmit and what the virtual
//! router answers (`dhcp`). A frame sent on a link is seen by this
//! process's ETH_P_ALL sockets on it (as outgoing), and the router's
//! answer by every socket bound to its protocol there. Packet sockets of
//! other processes do not see them.
//!
//! A packet socket is one end of a host datagram socketpair; frames are
//! written into the other end, which the layer keeps, so poll, epoll and
//! plain reads work. Classic BPF filters (`SO_ATTACH_FILTER`) run on each
//! frame before it is queued. SOCK_DGRAM packet sockets (without the link
//! header) are not offered.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};

use super::{dhcp, netif, uplink};
use crate::errno::{self, EINVAL, EPERM, Errno};

pub const L_AF_PACKET: u16 = 17;
const ETH_P_ALL: u16 = 0x0003;
const CAP_NET_RAW: u32 = 13;
const ENXIO: Errno = 6;
const ENETDOWN: Errno = 100;

/// PACKET_HOST, PACKET_BROADCAST, PACKET_MULTICAST, PACKET_OTHERHOST,
/// PACKET_OUTGOING.
const HOST: u8 = 0;
const BROADCAST: u8 = 1;
const MULTICAST: u8 = 2;
const OTHERHOST: u8 = 3;
const OUTGOING: u8 = 4;

/// One classic BPF instruction (`struct sock_filter`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Insn {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

pub struct Socket {
    /// The layer's end of the pair, where frames for the socket go.
    peer: i32,
    /// The protocol in host order (0: none yet, so nothing is received).
    proto: AtomicU32,
    ifindex: AtomicU32,
    filter: Mutex<Option<Vec<Insn>>>,
}

/// This process's packet sockets.
static SOCKETS: Mutex<Vec<Weak<Socket>>> = Mutex::new(Vec::new());

/// `socket(AF_PACKET, SOCK_RAW, proto)`: returns the guest's end.
pub fn socket(ty: u64, proto: u64) -> Result<(i32, Arc<Socket>), i64> {
    if ty != 3 {
        return Err(-94); // ESOCKTNOSUPPORT
    }
    if !super::cred::capable(CAP_NET_RAW) {
        return Err(-(EPERM as i64));
    }
    let mut fds = [0i32; 2];
    // SAFETY: socketpair into a local array.
    if unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_DGRAM, 0, fds.as_mut_ptr()) } < 0 {
        return Err(-(errno::last() as i64));
    }
    for fd in fds {
        super::net::size_buffers(fd);
    }
    // SAFETY: plain fcntl on our end: a full queue drops frames.
    unsafe { libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK) };
    let s = Arc::new(Socket {
        peer: super::fdtab::hide(fds[1]),
        proto: AtomicU32::new(u16::from_be(proto as u16) as u32),
        ifindex: AtomicU32::new(0),
        filter: Mutex::new(None),
    });
    register(&s);
    Ok((fds[0], s))
}

fn register(s: &Arc<Socket>) {
    let mut all = SOCKETS.lock().unwrap();
    all.retain(|w| w.strong_count() > 0);
    all.push(Arc::downgrade(s));
}

impl Socket {
    /// Fork: the socket's state (the peer end is inherited hidden).
    pub fn save(&self, w: &mut super::fork_state::Writer) {
        w.i32(self.peer);
        w.u32(self.proto.load(Ordering::Relaxed));
        w.u32(self.ifindex.load(Ordering::Relaxed));
        let f = self.filter.lock().unwrap();
        w.opt(f.as_deref(), |w, f| {
            w.seq(f.iter(), |w, i| {
                w.u32(i.code as u32 | (i.jt as u32) << 16 | (i.jf as u32) << 24);
                w.u32(i.k);
            })
        });
    }

    pub fn load(r: &mut super::fork_state::Reader) -> Arc<Socket> {
        let peer = r.i32();
        let (proto, ifindex) = (r.u32(), r.u32());
        let filter = r.opt(|r| {
            r.seq(|r| {
                let c = r.u32();
                Insn {
                    code: c as u16,
                    jt: (c >> 16) as u8,
                    jf: (c >> 24) as u8,
                    k: r.u32(),
                }
            })
        });
        let s = Arc::new(Socket {
            peer,
            proto: AtomicU32::new(proto),
            ifindex: AtomicU32::new(ifindex),
            filter: Mutex::new(filter),
        });
        register(&s);
        s
    }
}

impl Socket {
    /// SO_PROTOCOL: the protocol as `socket` or `bind` gave it (network
    /// order).
    pub fn protocol(&self) -> u16 {
        (self.proto.load(Ordering::Relaxed) as u16).to_be()
    }

    /// Linux `sockaddr_ll` of the socket's binding.
    pub fn local_name(&self) -> Vec<u8> {
        let index = self.ifindex.load(Ordering::Relaxed);
        let link = netif::by_index(index);
        let mut v = L_AF_PACKET.to_le_bytes().to_vec();
        v.extend_from_slice(&(self.proto.load(Ordering::Relaxed) as u16).to_be_bytes());
        v.extend_from_slice(&(index as i32).to_le_bytes());
        v.extend_from_slice(&link.as_ref().map_or(0, |l| l.arphrd()).to_le_bytes());
        v.push(0);
        v.push(if link.is_some() { 6 } else { 0 });
        v.extend_from_slice(&link.map_or([0; 6], |l| l.mac));
        v.extend_from_slice(&[0, 0]);
        v
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        // SAFETY: the layer's end, which only this socket uses.
        unsafe { libc::close(self.peer) };
        super::fdtab::unhide(self.peer);
    }
}

/// Parse a guest `sockaddr_ll`: (protocol in host order, ifindex).
pub fn parse_sockaddr(ptr: u64, len: u32) -> Result<(u16, u32), i64> {
    if ptr == 0 || len < 12 {
        return Err(-(EINVAL as i64));
    }
    // SAFETY: a guest sockaddr of at least 12 bytes.
    let b = unsafe { std::slice::from_raw_parts(ptr as *const u8, 12) };
    if u16::from_le_bytes([b[0], b[1]]) != L_AF_PACKET {
        return Err(-(EINVAL as i64));
    }
    Ok((
        u16::from_be_bytes([b[2], b[3]]),
        i32::from_le_bytes(b[4..8].try_into().unwrap()) as u32,
    ))
}

/// `bind`: a protocol of 0 keeps the socket's.
pub fn bind(s: &Socket, proto: u16, ifindex: u32) -> i64 {
    if ifindex != 0 && netif::by_index(ifindex).is_none() {
        return -(crate::errno::ENODEV as i64);
    }
    if proto != 0 {
        s.proto.store(proto as u32, Ordering::Relaxed);
    }
    s.ifindex.store(ifindex, Ordering::Relaxed);
    0
}

/// The Linux `sockaddr_ll` of a frame received on `ifindex`.
fn sockaddr_ll(frame: &[u8], ifindex: u32) -> Vec<u8> {
    let mut v = L_AF_PACKET.to_le_bytes().to_vec();
    v.extend_from_slice(frame.get(12..14).unwrap_or(&[0, 0]));
    v.extend_from_slice(&(ifindex as i32).to_le_bytes());
    v.extend_from_slice(&netif::ARPHRD_ETHER.to_le_bytes());
    v.push(pkttype(frame));
    v.push(6);
    v.extend_from_slice(frame.get(6..12).unwrap_or(&[0; 6]));
    v.extend_from_slice(&[0, 0]);
    v
}

fn pkttype(frame: &[u8]) -> u8 {
    let (dst, src) = (&frame[..6], &frame[6..12]);
    if src == netif::ETH0_MAC {
        OUTGOING
    } else if dst == [0xff; 6] {
        BROADCAST
    } else if dst[0] & 1 != 0 {
        MULTICAST
    } else if dst == netif::ETH0_MAC {
        HOST
    } else {
        OTHERHOST
    }
}

/// Queue `frame` on the sockets that receive it: bound to `ifindex` (or
/// to every link) and to its protocol, or ETH_P_ALL taps for an outgoing
/// frame (except its sender), each through its filter.
fn deliver(frame: &[u8], ifindex: u32, outgoing: Option<&Socket>) {
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]) as u32;
    let all: Vec<Arc<Socket>> = SOCKETS
        .lock()
        .unwrap()
        .iter()
        .filter_map(Weak::upgrade)
        .collect();
    for s in all {
        let proto = s.proto.load(Ordering::Relaxed);
        let bound = s.ifindex.load(Ordering::Relaxed);
        let wants = match outgoing {
            Some(from) => proto == ETH_P_ALL as u32 && !std::ptr::eq(from, &*s),
            None => proto == ETH_P_ALL as u32 || proto == ethertype,
        };
        if !wants || (bound != 0 && bound != ifindex) {
            continue;
        }
        let keep = match &*s.filter.lock().unwrap() {
            Some(prog) => run(prog, frame, ifindex).min(frame.len() as u32) as usize,
            None => frame.len(),
        };
        if keep > 0 {
            // SAFETY: a datagram into the socket's queue; a full one drops it.
            unsafe { libc::send(s.peer, frame.as_ptr().cast(), keep, 0) };
        }
    }
}

/// Transmit `frame` on link `ifindex` (the bound one when 0).
pub fn send(s: &Socket, frame: &[u8], ifindex: u32) -> i64 {
    let ifindex = if ifindex == 0 {
        s.ifindex.load(Ordering::Relaxed)
    } else {
        ifindex
    };
    let Some(link) = netif::by_index(ifindex) else {
        return -(ENXIO as i64);
    };
    if frame.len() < 14 {
        return -(EINVAL as i64);
    }
    if frame.len() > 14 + link.mtu as usize {
        return -90; // EMSGSIZE
    }
    if link.live_flags() & netif::IFF_RUNNING == 0 {
        return -(ENETDOWN as i64);
    }
    deliver(frame, ifindex, Some(s));
    if link.index == netif::ETH0
        && let Some(up) = uplink::uplink()
        && let Some(reply) = dhcp::answer(frame, &up, &uplink::dns_servers())
    {
        deliver(&reply, ifindex, None);
    }
    frame.len() as i64
}

/// Receive one frame into `buf`: (bytes copied or, with `trunc`, the
/// frame's length; whether it was truncated; the sender's address).
pub fn recv(
    s: &Socket,
    fd: i32,
    buf: &mut [u8],
    flags: i32,
    trunc: bool,
) -> Result<(usize, bool, Vec<u8>), i64> {
    let mut frame = vec![0u8; 65536 + 14];
    // SAFETY: receiving into our buffer.
    let n = unsafe { libc::recv(fd, frame.as_mut_ptr().cast(), frame.len(), flags) };
    if n < 0 {
        return Err(-(errno::last() as i64));
    }
    let frame = &frame[..n as usize];
    let copy = frame.len().min(buf.len());
    buf[..copy].copy_from_slice(&frame[..copy]);
    let bound = s.ifindex.load(Ordering::Relaxed);
    let ifindex = if bound == 0 { netif::ETH0 } else { bound };
    let name = if frame.len() >= 14 {
        sockaddr_ll(frame, ifindex)
    } else {
        Vec::new()
    };
    Ok((
        if trunc { frame.len() } else { copy },
        frame.len() > buf.len(),
        name,
    ))
}

// ---- classic BPF ------------------------------------------------------------

/// `SO_ATTACH_FILTER` with a guest `struct sock_fprog`.
pub fn attach_filter(s: &Socket, val: u64, len: u32) -> i64 {
    if val == 0 || len < 16 {
        return -(EINVAL as i64);
    }
    // SAFETY: the guest's struct sock_fprog { u16 len; sock_filter *filter; }.
    let (n, p) = unsafe {
        (
            (val as *const u16).read_unaligned() as usize,
            ((val + 8) as *const u64).read_unaligned(),
        )
    };
    if n == 0 || n > 4096 || p == 0 {
        return -(EINVAL as i64);
    }
    let prog: Vec<Insn> = (0..n)
        .map(|i| {
            // SAFETY: the guest's filter array of n entries.
            let b = unsafe { std::slice::from_raw_parts((p + 8 * i as u64) as *const u8, 8) };
            Insn {
                code: u16::from_le_bytes([b[0], b[1]]),
                jt: b[2],
                jf: b[3],
                k: u32::from_le_bytes(b[4..8].try_into().unwrap()),
            }
        })
        .collect();
    if !valid(&prog) {
        return -(EINVAL as i64);
    }
    *s.filter.lock().unwrap() = Some(prog);
    0
}

pub fn detach_filter(s: &Socket) -> i64 {
    match s.filter.lock().unwrap().take() {
        Some(_) => 0,
        None => -2, // ENOENT
    }
}

/// Jumps stay in the program, which ends in a return (sk_chk_filter).
fn valid(prog: &[Insn]) -> bool {
    prog.iter().enumerate().all(|(i, p)| {
        let rest = prog.len() - i - 1;
        match p.code & 7 {
            5 if p.code & 0xf0 == 0 => (p.k as usize) < rest,
            5 => (p.jt as usize) < rest && (p.jf as usize) < rest,
            _ => true,
        }
    }) && prog.last().is_some_and(|p| p.code & 7 == 6)
}

/// Linux's ancillary loads (SKF_AD_OFF + n), and loads relative to the
/// network header (SKF_NET_OFF) or the link header (SKF_LL_OFF).
const SKF_AD_OFF: u32 = 0xffff_f000;
const SKF_NET_OFF: u32 = 0xfff0_0000;
const SKF_LL_OFF: u32 = 0xffe0_0000;

/// The frame offset of load address `at`: the network header follows the
/// 14-byte Ethernet header.
fn offset(at: u32) -> Option<usize> {
    if at >= SKF_AD_OFF {
        None
    } else if at >= SKF_NET_OFF {
        Some(14 + (at - SKF_NET_OFF) as usize)
    } else if at >= SKF_LL_OFF {
        Some((at - SKF_LL_OFF) as usize)
    } else {
        Some(at as usize)
    }
}

/// Run a filter on a frame: the number of bytes to keep (0 drops it).
fn run(prog: &[Insn], pkt: &[u8], ifindex: u32) -> u32 {
    let (mut a, mut x) = (0u32, 0u32);
    let mut mem = [0u32; 16];
    let load = |at: u32, size: u32| -> Option<u32> {
        if at >= SKF_AD_OFF {
            return Some(match at - SKF_AD_OFF {
                0 => u16::from_be_bytes([pkt[12], pkt[13]]) as u32,
                4 => pkttype(pkt) as u32,
                8 => ifindex,
                _ => 0,
            });
        }
        let at = offset(at)?;
        let b = pkt.get(at..at.checked_add(size as usize)?)?;
        Some(b.iter().fold(0u32, |v, &c| v << 8 | c as u32))
    };
    let mut pc = 0;
    while let Some(i) = prog.get(pc) {
        pc += 1;
        let size = match i.code & 0x18 {
            0x00 => 4,
            0x08 => 2,
            _ => 1,
        };
        let src = if i.code & 0x08 != 0 { x } else { i.k };
        match i.code & 7 {
            // LD
            0 => {
                a = match i.code & 0xe0 {
                    0x00 => i.k,
                    0x20 => match load(i.k, size) {
                        Some(v) => v,
                        None => return 0,
                    },
                    0x40 => match load(x.wrapping_add(i.k), size) {
                        Some(v) => v,
                        None => return 0,
                    },
                    0x60 => mem[(i.k & 15) as usize],
                    0x80 => pkt.len() as u32,
                    _ => return 0,
                }
            }
            // LDX
            1 => {
                x = match i.code & 0xe0 {
                    0x00 => i.k,
                    0x60 => mem[(i.k & 15) as usize],
                    0x80 => pkt.len() as u32,
                    // MSH: 4 * (P[k] & 0xf), an IPv4 header length.
                    0xa0 => match offset(i.k).and_then(|o| pkt.get(o)) {
                        Some(b) => 4 * (b & 0xf) as u32,
                        None => return 0,
                    },
                    _ => return 0,
                }
            }
            2 => mem[(i.k & 15) as usize] = a,
            3 => mem[(i.k & 15) as usize] = x,
            // ALU
            4 => {
                a = match i.code & 0xf0 {
                    0x00 => a.wrapping_add(src),
                    0x10 => a.wrapping_sub(src),
                    0x20 => a.wrapping_mul(src),
                    0x30 if src == 0 => return 0,
                    0x30 => a / src,
                    0x40 => a | src,
                    0x50 => a & src,
                    0x60 => a.checked_shl(src).unwrap_or(0),
                    0x70 => a.checked_shr(src).unwrap_or(0),
                    0x80 => a.wrapping_neg(),
                    0x90 if src == 0 => return 0,
                    0x90 => a % src,
                    0xa0 => a ^ src,
                    _ => return 0,
                }
            }
            // JMP
            5 => {
                let taken = match i.code & 0xf0 {
                    0x00 => {
                        pc += i.k as usize;
                        continue;
                    }
                    0x10 => a == src,
                    0x20 => a > src,
                    0x30 => a >= src,
                    0x40 => a & src != 0,
                    _ => return 0,
                };
                pc += if taken { i.jt } else { i.jf } as usize;
            }
            // RET
            6 => {
                return match i.code & 0x18 {
                    0x00 => i.k,
                    0x08 => x,
                    _ => a,
                };
            }
            // MISC: TAX, TXA
            _ => {
                if i.code & 0xf8 == 0 {
                    x = a
                } else {
                    a = x
                }
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stmt(code: u16, k: u32) -> Insn {
        Insn {
            code,
            jt: 0,
            jf: 0,
            k,
        }
    }
    fn jump(code: u16, k: u32, jt: u8, jf: u8) -> Insn {
        Insn { code, jt, jf, k }
    }

    /// NetworkStack's DHCP filter (attachDhcpFilter): UDP, not a
    /// fragment, to port 68.
    fn dhcp_filter() -> Vec<Insn> {
        vec![
            stmt(0x30, 23),
            jump(0x15, 17, 0, 6),
            stmt(0x28, 20),
            jump(0x45, 0x1fff, 4, 0),
            stmt(0xb1, 14),
            stmt(0x48, 16),
            jump(0x15, 68, 0, 1),
            stmt(0x06, 0xffff),
            stmt(0x06, 0),
        ]
    }

    #[test]
    fn dhcp_filter_accepts_replies_only() {
        let prog = dhcp_filter();
        assert!(valid(&prog));
        let mut f = vec![0u8; 14 + 20 + 8];
        f[12..14].copy_from_slice(&[8, 0]);
        f[14] = 0x45;
        f[23] = 17;
        f[34..38].copy_from_slice(&[0, 67, 0, 68]);
        assert_eq!(run(&prog, &f, 2), 0xffff);
        f[37] = 67;
        assert_eq!(run(&prog, &f, 2), 0);
        f[37] = 68;
        f[21] = 1; // a fragment at offset 8
        assert_eq!(run(&prog, &f, 2), 0);
        // A short frame fails the load and is dropped.
        assert_eq!(run(&prog, &f[..20], 2), 0);
    }

    /// NetworkStack's Android 16 DHCP filter: loads relative to the
    /// network header (SKF_NET_OFF).
    #[test]
    fn network_header_loads() {
        let net = SKF_NET_OFF;
        let prog = vec![
            stmt(0x30, net + 9),
            jump(0x15, 17, 1, 0),
            stmt(0x06, 0),
            stmt(0x28, net + 6),
            jump(0x45, 0x3fff, 0, 1),
            stmt(0x06, 0),
            stmt(0xb1, net),
            stmt(0x48, net + 2),
            jump(0x15, 68, 1, 0),
            stmt(0x06, 0),
            stmt(0x06, u32::MAX),
        ];
        assert!(valid(&prog));
        let mut f = vec![0u8; 14 + 20 + 8];
        f[12..14].copy_from_slice(&[8, 0]);
        f[14] = 0x45;
        f[23] = 17;
        f[34..38].copy_from_slice(&[0, 67, 0, 68]);
        assert_eq!(run(&prog, &f, 2), u32::MAX);
        f[37] = 67;
        assert_eq!(run(&prog, &f, 2), 0);
        // SKF_AD_PROTOCOL is the ethertype.
        assert_eq!(run(&[stmt(0x28, SKF_AD_OFF), stmt(0x16, 0)], &f, 2), 0x800);
    }

    #[test]
    fn filters_must_end_in_a_return_and_jump_inside() {
        assert!(!valid(&[stmt(0x30, 0)]));
        assert!(!valid(&[jump(0x15, 0, 3, 0), stmt(0x06, 0)]));
        assert!(valid(&[stmt(0x06, 1)]));
    }
}
