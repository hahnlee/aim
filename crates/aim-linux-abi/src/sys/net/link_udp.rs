//! UDP endpoints on the virtual link. Linux udp_lib_get_port conflicts
//! wildcard/exact local addresses unless both sockets allow reuse;
//! __udp4_lib_mcast_deliver fans broadcasts out, while unicast selects one.
//!
//! Each endpoint uses its original host socket, bound to a private loopback
//! port, as its receive queue. The namespace registry names actual socket
//! cookies, so dup/fork share a queue and the last close releases the bind.
//! No host DHCP port or background forwarding thread is needed.

use super::*;
use crate::errno::{EIO, Errno};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd};
use std::os::unix::fs::OpenOptionsExt;

#[derive(Clone, Debug)]
struct Binding {
    cookie: u64,
    port: u16,
    addr: [u8; 4],
    peer: Option<([u8; 4], u16)>,
    reuseport: bool,
    uid: u32,
}

impl Binding {
    fn conflicts(&self, other: &Self, options: &BTreeMap<u64, (u32, u16)>) -> bool {
        let reuseaddr = |cookie| {
            options
                .get(&cookie)
                .is_some_and(|o| o.0 & libc::SO_REUSEADDR as u32 != 0)
        };
        (self.addr == [0; 4] || other.addr == [0; 4] || self.addr == other.addr)
            && !(reuseaddr(self.cookie) && reuseaddr(other.cookie))
            && !(self.reuseport && other.reuseport && self.uid == other.uid)
    }

    fn accepts(&self, src: ([u8; 4], u16), dst: [u8; 4]) -> bool {
        (self.addr == [0; 4] || self.addr == dst) && self.peer.is_none_or(|peer| peer == src)
    }
}

struct Hidden<T: AsRawFd>(Option<T>);
impl<T: AsRawFd + IntoRawFd + FromRawFd> Hidden<T> {
    fn new(value: T) -> Self {
        // SAFETY: ownership moves to the relocated private descriptor.
        Self(Some(unsafe {
            T::from_raw_fd(fdtab::hide(value.into_raw_fd()))
        }))
    }
}
impl<T: AsRawFd> std::ops::Deref for Hidden<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0.as_ref().unwrap()
    }
}
impl<T: AsRawFd> std::ops::DerefMut for Hidden<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.0.as_mut().unwrap()
    }
}
impl<T: AsRawFd> Drop for Hidden<T> {
    fn drop(&mut self) {
        let value = self.0.take().unwrap();
        let fd = value.as_raw_fd();
        drop(value);
        fdtab::unhide(fd);
    }
}
struct Registry {
    file: Hidden<File>,
    entries: Vec<Binding>,
    options: BTreeMap<u64, (u32, u16)>,
}
fn io_error(e: std::io::Error) -> Errno {
    e.raw_os_error().map(errno::from_darwin).unwrap_or(EIO)
}
impl Registry {
    fn open() -> Result<Self, Errno> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .mode(0o600)
            .open(super::super::netif::kernel_dir("net").join("udp-bindings"))
            .map_err(io_error)?;
        let file = Hidden::new(file);
        let mut r = Self {
            file,
            entries: Vec::new(),
            options: BTreeMap::new(),
        };
        loop {
            if unsafe { libc::flock(r.file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                break;
            }
            let error = errno::last();
            if error != errno::EINTR {
                return Err(error);
            }
        }
        let mut bytes = Vec::new();
        r.file.read_to_end(&mut bytes).map_err(io_error)?;
        if bytes.len() % 32 != 0 {
            return Err(EIO);
        }
        r.options = proc_table::live_socket_options()?;
        for b in bytes.chunks_exact(32) {
            let cookie = u64::from_le_bytes(b[..8].try_into().unwrap());
            let port = u16::from_be_bytes(b[8..10].try_into().unwrap());
            // A recycled host socket handle is not the old bound endpoint.
            if !r.options.get(&cookie).is_some_and(|o| o.1 == port) {
                continue;
            }
            r.entries.push(Binding {
                cookie,
                port,
                addr: b[10..14].try_into().unwrap(),
                peer: (b[20] != 0).then(|| {
                    (
                        b[14..18].try_into().unwrap(),
                        u16::from_be_bytes(b[18..20].try_into().unwrap()),
                    )
                }),
                reuseport: b[21] != 0,
                uid: u32::from_le_bytes(b[22..26].try_into().unwrap()),
            });
        }
        Ok(r)
    }
    fn save(&mut self) -> Result<(), Errno> {
        let mut bytes = Vec::new();
        for e in &self.entries {
            let mut b = [0u8; 32];
            b[..8].copy_from_slice(&e.cookie.to_le_bytes());
            b[8..10].copy_from_slice(&e.port.to_be_bytes());
            b[10..14].copy_from_slice(&e.addr);
            if let Some((addr, port)) = e.peer {
                b[14..18].copy_from_slice(&addr);
                b[18..20].copy_from_slice(&port.to_be_bytes());
                b[20] = 1;
            }
            b[21] = u8::from(e.reuseport);
            b[22..26].copy_from_slice(&e.uid.to_le_bytes());
            bytes.extend_from_slice(&b);
        }
        self.file.rewind().map_err(io_error)?;
        self.file.write_all(&bytes).map_err(io_error)?;
        self.file.set_len(bytes.len() as u64).map_err(io_error)
    }
}

fn loopback(port: u16) -> libc::sockaddr_in {
    libc::sockaddr_in {
        sin_len: 16,
        sin_family: libc::AF_INET as u8,
        sin_port: port.to_be(),
        sin_addr: libc::in_addr {
            s_addr: u32::from_ne_bytes([127, 0, 0, 1]),
        },
        sin_zero: [0; 8],
    }
}

pub(super) fn bind(fd: i32, addr: [u8; 4], reuseport: bool) -> Result<(), Errno> {
    let _own = super::super::fork::spawn::own_fds();
    let owner = proc_table::created(fd)?;
    let mut registry = Registry::open()?;
    let mut entry = Binding {
        cookie: owner.cookie,
        port: 0,
        addr,
        peer: None,
        reuseport,
        uid: owner.uid,
    };
    if registry
        .entries
        .iter()
        .any(|e| e.conflicts(&entry, &registry.options))
    {
        return Err(98);
    }
    // The transport is local; guest SO_BINDTODEVICE continues to name eth0.
    bind_host_interface(fd, 0)?;
    let sa = loopback(0);
    if unsafe { libc::bind(fd, (&raw const sa).cast(), 16) } < 0 {
        return Err(errno::last());
    }
    entry.port = host_inet_name(fd, false).ok_or(EIO)?.0;
    registry.entries.push(entry);
    registry.save()
}

pub(super) fn update(
    fd: i32,
    peer: Option<Option<([u8; 4], u16)>>,
    reuseport: Option<bool>,
) -> Result<(), Errno> {
    let _own = super::super::fork::spawn::own_fds();
    let cookie = proc_table::created(fd)?.cookie;
    let mut registry = Registry::open()?;
    let entry = registry
        .entries
        .iter_mut()
        .find(|e| e.cookie == cookie)
        .ok_or(EIO)?;
    if let Some(peer) = peer {
        entry.peer = peer;
    }
    if let Some(reuseport) = reuseport {
        entry.reuseport = reuseport;
    }
    registry.save()
}

/// Deliver an incoming IPv4 UDP frame to the namespace's matching endpoints.
/// The router constructs these frames, including their lengths/checksums.
pub(crate) fn deliver(frame: &[u8]) -> Result<(), Errno> {
    if frame.len() < 42 || frame[12..14] != [8, 0] || frame[23] != 17 {
        return Ok(());
    }
    let ip = &frame[14..];
    let ihl = (ip[0] as usize & 15) * 4;
    if ihl < 20 || ip.len() < ihl + 8 {
        return Err(EIO);
    }
    let udp = &ip[ihl..];
    let size = u16::from_be_bytes([udp[4], udp[5]]) as usize;
    if size < 8 || size > udp.len() {
        return Err(EIO);
    }
    if u16::from_be_bytes([udp[2], udp[3]]) != super::super::dhcp::CLIENT_PORT {
        return Ok(());
    }
    let src = (
        ip[12..16].try_into().unwrap(),
        u16::from_be_bytes([udp[0], udp[1]]),
    );
    let dst: [u8; 4] = ip[16..20].try_into().unwrap();
    if dst != [255; 4]
        && !super::super::netif::by_index(super::super::netif::ETH0).is_some_and(|link| {
            link.addrs
                .iter()
                .any(|a| a.ip == std::net::IpAddr::V4(dst.into()))
        })
    {
        // ip_rcv routes only local destinations into UDP input; an OFFER
        // unicasted to an address not configured yet reaches AF_PACKET only.
        return Ok(());
    }
    let _own = super::super::fork::spawn::own_fds();
    let registry = Registry::open()?;
    let mut targets: Vec<_> = registry
        .entries
        .iter()
        .filter(|e| e.accepts(src, dst))
        .collect();
    if dst != [255; 4] {
        // __udp4_lib_lookup: connected and exact-address sockets score higher;
        // an equal-score unicast goes to one socket, not every reuse binding.
        targets.sort_by_key(|e| (e.peer.is_some(), e.addr != [0; 4]));
        targets = targets.last().copied().into_iter().collect();
    }
    if targets.is_empty() {
        return Ok(());
    }
    let sender =
        Hidden::new(std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).map_err(io_error)?);
    sender.set_nonblocking(true).map_err(io_error)?;
    let mut data = vec![0; 16];
    data[..2].copy_from_slice(&L_AF_INET.to_le_bytes());
    data[2..4].copy_from_slice(&src.1.to_be_bytes());
    data[4..8].copy_from_slice(&src.0);
    data.extend_from_slice(&udp[8..size]);
    for target in targets {
        if let Err(error) = sender.send_to(&data, (Ipv4Addr::LOCALHOST, target.port)) {
            let error = io_error(error);
            // Linux udp_queue_rcv_skb drops a datagram when the receive queue
            // is full; a socket closing during delivery also drops it.
            if !matches!(error, errno::EAGAIN | 105 | 111) {
                return Err(error);
            }
        }
    }
    Ok(())
}

pub(super) fn recv(
    fd: i32,
    iov: &[libc::iovec],
    ctrl_cap: usize,
    flags: u64,
) -> Result<Received, i64> {
    let mut data = vec![0u8; 65535];
    let mut vector = [libc::iovec {
        iov_base: data.as_mut_ptr().cast(),
        iov_len: data.len(),
    }];
    let mut ctrl = IncomingControl::buffer(if ctrl_cap > 0 { ctrl_cap + 64 } else { 0 });
    let (n, _, cl, _) = recvmsg_host(fd, &mut vector, &mut ctrl, false, host_recv_flags(flags))?;
    if n < 16 {
        return Err(-(EIO as i64));
    }
    ctrl.truncate(cl);
    let msg = &data[16..n];
    let mut at = 0;
    for v in iov {
        let k = v.iov_len.min(msg.len() - at);
        unsafe { std::ptr::copy_nonoverlapping(msg[at..].as_ptr(), v.iov_base.cast(), k) };
        at += k;
    }
    Ok(Received {
        n: if flags & L_MSG_TRUNC != 0 {
            msg.len()
        } else {
            at
        } as i64,
        flags: if at < msg.len() { libc::MSG_TRUNC } else { 0 },
        ctrl,
        cred: None,
        name: Some(data[..16].to_vec()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::OwnedFd;

    fn socket() -> OwnedFd {
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
        assert!(fd >= 0);
        unsafe { OwnedFd::from_raw_fd(fd) }
    }
    fn endpoint(cookie: u64, addr: [u8; 4]) -> Binding {
        Binding {
            cookie,
            port: 0,
            addr,
            peer: None,
            reuseport: false,
            uid: 1000,
        }
    }

    #[test]
    fn wildcard_conflicts_and_both_sockets_must_allow_reuse() {
        let any = endpoint(1, [0; 4]);
        let exact = endpoint(2, [192, 0, 2, 1]);
        for (a, b, conflicts) in [
            (0, 0, true),
            (libc::SO_REUSEADDR, 0, true),
            (0, libc::SO_REUSEADDR, true),
            (libc::SO_REUSEADDR, libc::SO_REUSEADDR, false),
        ] {
            let options = BTreeMap::from([(1, (a as u32, 0)), (2, (b as u32, 0))]);
            assert_eq!(any.conflicts(&exact, &options), conflicts);
            assert_eq!(exact.conflicts(&any, &options), conflicts);
        }
        assert!(!exact.conflicts(&endpoint(3, [192, 0, 2, 2]), &BTreeMap::new()));
        let mut first = any.clone();
        let mut next = exact.clone();
        first.reuseport = true;
        next.reuseport = true;
        assert!(!first.conflicts(&next, &BTreeMap::new()));
        next.uid += 1;
        assert!(first.conflicts(&next, &BTreeMap::new()));
    }

    #[test]
    fn input_matches_local_address_and_connected_peer() {
        let src = ([192, 0, 2, 254], 67);
        let mut e = endpoint(1, [192, 0, 2, 1]);
        assert!(e.accepts(src, e.addr));
        assert!(!e.accepts(src, [255; 4]));
        e.addr = [0; 4];
        assert!(e.accepts(src, [255; 4]));
        e.peer = Some(src);
        assert!(e.accepts(src, [255; 4]));
        assert!(!e.accepts((src.0, 66), [255; 4]));
        assert!(!e.accepts(([192, 0, 2, 253], 67), [255; 4]));
    }

    #[test]
    fn binding_lives_until_last_host_alias_closes() {
        if fdtab::isolated_kernel_test(
            "sys::net::link_udp::tests::binding_lives_until_last_host_alias_closes",
        ) {
            return;
        }
        let (_view, _dir) = crate::vfs::test_view();
        let a = socket();
        let b = socket();
        bind(a.as_raw_fd(), [0; 4], false).unwrap();
        assert_eq!(bind(b.as_raw_fd(), [0; 4], false), Err(98));
        let alias = a.try_clone().unwrap();
        drop(a);
        assert_eq!(bind(b.as_raw_fd(), [0; 4], false), Err(98));
        drop(alias);
        bind(b.as_raw_fd(), [0; 4], false).unwrap();
    }

    #[test]
    fn broadcasts_queue_payloads_with_peek_truncation_and_source() {
        if fdtab::isolated_kernel_test(
            "sys::net::link_udp::tests::broadcasts_queue_payloads_with_peek_truncation_and_source",
        ) {
            return;
        }
        let (_view, _dir) = crate::vfs::test_view();
        let a = socket();
        let b = socket();
        for fd in [a.as_raw_fd(), b.as_raw_fd()] {
            set_int(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, 1);
            bind(fd, [0; 4], false).unwrap();
        }
        let mut frame = vec![0; 42];
        frame[12..14].copy_from_slice(&[8, 0]);
        frame[14] = 0x45;
        frame[23] = 17;
        frame[26..30].copy_from_slice(&[192, 0, 2, 254]);
        frame[30..34].fill(255);
        frame[34..40].copy_from_slice(&[0, 67, 0, 68, 0, 13]);
        frame.extend_from_slice(b"hello");
        deliver(&frame).unwrap();
        for fd in [a.as_raw_fd(), b.as_raw_fd()] {
            let mut data = [0; 2];
            let v = [libc::iovec {
                iov_base: data.as_mut_ptr().cast(),
                iov_len: 2,
            }];
            let r = recv(fd, &v, 0, L_MSG_PEEK | L_MSG_DONTWAIT | L_MSG_TRUNC).unwrap();
            assert_eq!((r.n, r.flags), (5, libc::MSG_TRUNC));
            assert_eq!(&data, b"he");
            assert_eq!(&r.name.unwrap()[..8], &[2, 0, 0, 67, 192, 0, 2, 254]);
            assert_eq!(recv(fd, &v, 0, L_MSG_DONTWAIT).unwrap().n, 2);
            assert_eq!(
                recv(fd, &v, 0, L_MSG_DONTWAIT).err(),
                Some(-(errno::EAGAIN as i64))
            );
        }
    }
}
