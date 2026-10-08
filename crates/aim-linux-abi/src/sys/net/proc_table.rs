//! Namespace socket tables (#1144). Linux proc_net_tcp and udp_seq_show;
//! Darwin socket_fdinfo is read only for registered guest socket owners.
use super::*;
use crate::errno::{Errno, EIO, ESRCH};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub(crate) struct SocketMetadata {
    pub fd: i32,
    pub uid: u32,
    pub inode: u64,
    pub cookie: u64,
    pub local: Option<[u8; 28]>,
    pub peer: Option<[u8; 28]>,
    pub port_zero: bool,
    pub probes_known: bool,
}

// SDK sys/proc_info.h socket_fdinfo, the same layout used by host_protocol.
struct Info([u8; 792]);
impl Info {
    fn read(pid: i32, fd: i32) -> Result<Self, Errno> {
        let mut info = Self([0; 792]);
        let size = unsafe { libc::proc_pidfdinfo(pid, fd, 3, info.0.as_mut_ptr().cast(), 792) };
        if size != 792 { let error = errno::last(); return Err(if error == 0 { EIO } else { error }); }
        Ok(info)
    }
    fn word(&self, at: usize) -> u32 { u32::from_ne_bytes(self.0[at..at+4].try_into().unwrap()) }
    fn cookie(&self) -> u64 { u64::from_ne_bytes(self.0[160..168].try_into().unwrap()) }
    fn family(&self) -> i32 { self.word(184) as i32 }
    fn address(&self, local: bool) -> String {
        let at = if local { 312 } else { 296 };
        if self.family() == libc::AF_INET {
            format!("{:08X}", self.word(at + 12))
        } else {
            (0..4).map(|word| format!("{:08X}", self.word(at + word * 4))).collect()
        }
    }
    fn port(&self, local: bool) -> u16 {
        u16::from_be(self.word(if local { 268 } else { 264 }) as u16)
    }
}

pub(super) fn raw_identity(fd:i32)->Result<(u64,u64),Errno>{
    let identity=aim_storage::socket_inode::identity(fd).map_err(|error|error.raw_os_error().map(errno::from_darwin).unwrap_or(EIO))?;
    Ok((identity.inode,identity.cookie))
}

pub(super) fn created(fd: i32) -> Result<SocketMetadata, Errno> {
    let info = Info::read(unsafe { libc::getpid() }, fd)?;
    if !matches!(info.family(), libc::AF_INET | libc::AF_INET6) { return Err(97); }
    // An opaque socket handle identifies the actual shared open description.
    // Encode it bijectively rather than exposing a host kernel address.
    let cookie = info.cookie();
    if cookie == 0 { return Err(EIO); }
    let inode = cookie.wrapping_mul(0x9e3779b97f4a7c15).rotate_left(23);
    Ok(SocketMetadata { fd, uid: super::super::attrs::ids(super::super::attrs::FS).0, inode, cookie, local: None, peer: None, port_zero: false, probes_known: info.word(188) & libc::SO_KEEPALIVE as u32 == 0 })
}

pub(crate) fn socket_identity(fd: i32) -> Option<(u64, u32)> {
    let Kind::Sock(socket) = fdtab::get(fd)? else { return None; };
    let owner = (*socket.inet_owner.lock().unwrap())?;
    let info = Info::read(unsafe { libc::getpid() }, fd).ok()?;
    if info.cookie()!=owner.cookie{return None;}
    let uid=super::socket_inode::stat(fd)?.ok()?.st_uid;
    Some((owner.inode,uid))
}

pub(crate) fn metadata() -> Result<Vec<SocketMetadata>, Errno> {
    let mut owners = Vec::new();
    for fd in fdtab::open_fds() {
        if fdtab::is_hidden(fd) { continue; }
        let Some(Kind::Sock(socket)) = fdtab::get(fd) else { continue; };
        let Some(mut owner) = *socket.inet_owner.lock().unwrap() else { continue; };
        owner.fd = fd;
        if let Some(stat)=super::socket_inode::stat(fd){owner.uid=stat?.st_uid;}
        let copied = |name: Option<Vec<u8>>| name.map(|bytes| { let mut value = [0;28]; let count = bytes.len().min(28); value[..count].copy_from_slice(&bytes[..count]); value });
        owner.local = copied(socket.local.lock().unwrap().clone());
        if let Family::Inet(options) = &socket.family {
            if options.router.load(Ordering::Relaxed) { owner.peer = copied(socket.peer.lock().unwrap().clone()); }
            owner.port_zero = options.port_zero.load(Ordering::Relaxed);
        }
        let info = match Info::read(unsafe { libc::getpid() }, fd) {
            Ok(info) => info,
            Err(error) if matches!(error, crate::errno::EBADF | crate::errno::ENOENT | ESRCH) => continue,
            Err(error) => return Err(error),
        };
        if info.cookie() == owner.cookie { owners.push(owner); }
    }
    Ok(owners)
}

fn endpoint(info: &Info, local: bool, guest: Option<[u8; 28]>, port_zero: bool) -> (String, u16) {
    if let Some(guest) = guest {
        let family = u16::from_ne_bytes(guest[..2].try_into().unwrap());
        let address = if family == L_AF_INET {
            format!("{:08X}", u32::from_ne_bytes(guest[4..8].try_into().unwrap()))
        } else {
            guest[8..24].chunks_exact(4).map(|word| format!("{:08X}", u32::from_ne_bytes(word.try_into().unwrap()))).collect()
        };
        return (address, u16::from_be_bytes(guest[2..4].try_into().unwrap()));
    }
    (info.address(local), if port_zero { 0 } else { info.port(local) })
}

#[derive(Clone, Copy)]
struct TcpMetrics { retransmits: u32, probes: u32 }

fn tcp_metrics(cookies: &std::collections::BTreeSet<u64>) -> Result<BTreeMap<u64, TcpMetrics>, Errno> {
    if cookies.is_empty() { return Ok(BTreeMap::new()); }
    // XNU tcp_subr.c tcpcb_to_xtcpcb64, SDK tcp_var.h pack(4). Unlike
    // TCP_CONNECTION_INFO lifetime totals, t_rxtshift is current backoff.
    let mut length = 0;
    if unsafe { libc::sysctlbyname(c"net.inet.tcp.pcblist64".as_ptr(), std::ptr::null_mut(), &mut length, std::ptr::null_mut(), 0) } != 0 {
        return Err(errno::last());
    }
    let mut bytes = Vec::new();
    loop {
        bytes.resize(length, 0);
        let result = unsafe { libc::sysctlbyname(c"net.inet.tcp.pcblist64".as_ptr(), bytes.as_mut_ptr().cast(), &mut length, std::ptr::null_mut(), 0) };
        if result == 0 { bytes.truncate(length); break; }
        let error = errno::last();
        if error != crate::errno::ENOMEM { return Err(error); }
        length = length.max(bytes.len().saturating_mul(2));
    }
    let word = |at: usize| -> Result<u32, Errno> { Ok(u32::from_ne_bytes(bytes.get(at..at+4).ok_or(EIO)?.try_into().unwrap())) };
    if bytes.len() < 48 || word(0)? != 24 { return Err(EIO); }
    let mut at = 24;
    let mut found = BTreeMap::new();
    while at < bytes.len() {
        let size = word(at)? as usize;
        if size == 24 {
            if at + size != bytes.len() { return Err(EIO); }
            break;
        }
        if size != 472 || at + size > bytes.len() { return Err(EIO); }
        let cookie = u64::from_ne_bytes(bytes[at+152..at+160].try_into().unwrap());
        if cookies.contains(&cookie) {
            let backoff = word(at+400)?;
            if backoff > i32::MAX as u32 { return Err(EIO); }
            // XNU tcp_setpersist and TCPT_REXMT use the same backoff slot;
            // an active persistence timer distinguishes zero-window probes.
            if word(at+280)? != 0 {
                // tcp_setpersist increments/caps backoff before probes. Its
                // value cannot stand in for Linux's unanswered probe count.
                return Err(errno::from_darwin(libc::EOPNOTSUPP));
            }
            found.insert(cookie, TcpMetrics { retransmits: backoff, probes: 0 });
        }
        at += size;
    }
    Ok(found)
}

pub(crate) fn table(kind: &str) -> Result<Vec<u8>, Errno> {
    let (family, protocol) = match kind {
        "tcp" => (libc::AF_INET, libc::IPPROTO_TCP),
        "tcp6" => (libc::AF_INET6, libc::IPPROTO_TCP),
        "udp" => (libc::AF_INET, libc::IPPROTO_UDP),
        "udp6" => (libc::AF_INET6, libc::IPPROTO_UDP),
        _ => return Err(crate::errno::ENOENT),
    };
    let me = unsafe { libc::getpid() };
    // Without a configured guest namespace, only this kernel process is owned.
    let members = super::super::pidns::members().unwrap_or_else(|| vec![me]);
    let mut sockets = BTreeMap::new();
    for pid in members {
        let owners = if pid == me { metadata()? } else {
            match super::super::ptrace::remote_net_sockets(pid) {
                Ok(owners) => owners,
                Err(ESRCH) => continue,
                Err(error) => return Err(error),
            }
        };
        for owner in owners {
            let info = match Info::read(pid, owner.fd) {
                Ok(info) => info,
                // FD close and process exit are normal table traversal races.
                Err(error) if matches!(error, crate::errno::EBADF | crate::errno::ENOENT | ESRCH) => continue,
                Err(error) => return Err(error),
            };
            if info.cookie() != owner.cookie || info.family() != family || info.word(180) as i32 != protocol { continue; }
            let state = if protocol == libc::IPPROTO_TCP {
                match info.word(344) {
                    0 => continue, 1 => 0x0a, 2 => 2, 3 => 3, 4 => 1, 5 => 8,
                    6 => 4, 7 => 0x0b, 8 => 9, 9 => 5, 10 => 6, _ => return Err(EIO),
                }
            } else if info.port(false) != 0 { 1 } else { 7 };
            if protocol == libc::IPPROTO_UDP && info.port(true) == 0 { continue; }
            if protocol == libc::IPPROTO_TCP && state != 0x0a
                && (!owner.probes_known || info.word(188) & libc::SO_KEEPALIVE as u32 != 0) {
                // XNU public TCB data lacks the exact keepalive probe count.
                return Err(errno::from_darwin(libc::EOPNOTSUPP));
            }
            sockets.entry(owner.inode).or_insert((owner, info, state));
        }
    }
    let cookies = sockets.values().filter(|(_, _, state)| protocol == libc::IPPROTO_TCP && *state != 0x0a)
        .map(|(owner, _, _)| owner.cookie).collect();
    let metrics = tcp_metrics(&cookies)?;
    let mut sockets = sockets.into_values().collect::<Vec<_>>();
    sockets.sort_by_key(|(owner, _, state)| (*state != 0x0a, owner.inode));
    let mut text = String::from("  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n");
    for (slot, (owner, info, state)) in sockets.into_iter().enumerate() {
        // XNU exports t_timer offsets relative to a private timer-entry epoch,
        // not absolute uptime. Its inactive keep timer is not Linux sk_timer.
        // Until the actual epoch/probe producer exists, refuse active logical
        // TCP timers rather than publishing an invented expiry (#1144).
        if protocol == libc::IPPROTO_TCP && state != 0x0a {
            for (index, enabled) in [(0,true),(1,true),(2,info.word(188) & libc::SO_KEEPALIVE as u32 != 0),(3,true)] {
                if enabled && info.word(348 + index * 4) != 0 {
                    return Err(errno::from_darwin(libc::EOPNOTSUPP));
                }
            }
        }
        let counters = if protocol == libc::IPPROTO_TCP && state != 0x0a {
            *metrics.get(&owner.cookie).ok_or(crate::errno::EAGAIN)?
        } else { TcpMetrics { retransmits: 0, probes: 0 } };
        let local = endpoint(&info, true, owner.local, false);
        let peer = endpoint(&info, false, owner.peer, owner.port_zero);
        text.push_str(&format!("{slot:4}: {}:{:04X} {}:{:04X} {state:02X} {:08X}:{:08X} {:02X}:{:08X} {:08X} {} {} {}\n",
            local.0, local.1, peer.0, peer.1,
            if protocol == libc::IPPROTO_TCP { info.word(232) } else { info.word(240) },
            if state == 0x0a { u16::from_ne_bytes(info.0[194..196].try_into().unwrap()) as u32 }
                else if protocol == libc::IPPROTO_TCP { info.word(208) } else { info.word(216) },
            0, 0, counters.retransmits, owner.uid, counters.probes, owner.inode));
    }
    Ok(text.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proc_net_actual_socket_ipv4_ipv6_stat_read_and_namespace_filter() {
        let (_view, _) = vfs::test_view();
        let external = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let unrelated = external.local_addr().unwrap().port();
        let mut owned = Vec::new();
        for (domain, ty, kind) in [(2, 1, "tcp"), (10, 1, "tcp6"), (2, 2, "udp"), (10, 2, "udp6")] {
            let fd = socket([domain, ty, 0, 0, 0, 0]) as i32;
            assert!(fd >= 0);
            let mut address = [0u8; 28];
            address[..2].copy_from_slice(&(domain as u16).to_ne_bytes());
            let len = if domain == 2 { address[4..8].copy_from_slice(&[127,0,0,1]); 16 } else { address[23] = 1; 28 };
            assert_eq!(bind([fd as u64, address.as_ptr() as u64, len, 0, 0, 0]), 0);
            if ty == 1 { assert_eq!(listen([fd as u64, 2, 0, 0, 0, 0]), 0); }
            let owner = metadata().unwrap().into_iter().find(|owner| owner.fd == fd).unwrap();
            assert_eq!(owner.uid, super::super::super::attrs::ids(super::super::super::attrs::FS).0);
            let mut actual: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut actual_len = std::mem::size_of_val(&actual) as libc::socklen_t;
            assert_eq!(unsafe { libc::getsockname(fd, (&mut actual as *mut libc::sockaddr_storage).cast(), &mut actual_len) }, 0);
            let actual_bytes = unsafe { std::slice::from_raw_parts((&actual as *const libc::sockaddr_storage).cast::<u8>(), actual_len as usize) };
            let port = u16::from_be_bytes([actual_bytes[2],actual_bytes[3]]);
            let expected_address = if domain == 2 { "0100007F" } else { "00000000000000000000000001000000" };
            let token = format!("{expected_address}:{port:04X}");
            assert_ne!(owner.inode, 0);
            let Kind::Sock(original) = fdtab::get(fd).unwrap() else { panic!() };
            let mut writer = super::super::super::fork_state::Writer::default();
            save_sock(&original, &mut writer);
            let saved = writer.into_bytes();
            let restored = load_sock(&mut super::super::super::fork_state::Reader::new(&saved));
            let fork_owner = restored.inet_owner.lock().unwrap().unwrap();
            assert_eq!((fork_owner.uid,fork_owner.inode,fork_owner.cookie), (owner.uid,owner.inode,owner.cookie));
            let text = String::from_utf8(table(kind).unwrap()).unwrap();
            let row = text.lines().find(|line| line.contains(&token)).unwrap();
            let fields = row.split_whitespace().collect::<Vec<_>>();
            assert_eq!(fields[3], if ty == 1 { "0A" } else { "07" });
            assert_eq!(fields[7].parse::<u32>().unwrap(), owner.uid);
            assert_eq!(fields[9].parse::<u64>().unwrap(), owner.inode);
            if ty == 1 {
                let client = socket([domain, 1, 0, 0, 0, 0]) as i32;
                assert!(client >= 0);
                address[2..4].copy_from_slice(&port.to_be_bytes());
                assert_eq!(connect([client as u64, address.as_ptr() as u64, len, 0, 0, 0]), 0);
                let accepted = accept4([fd as u64, 0, 0, 0, 0, 0]) as i32;
                assert!(accepted >= 0);
                let accepted_owner = metadata().unwrap().into_iter().find(|value| value.fd == accepted).unwrap();
                assert_eq!(accepted_owner.uid, owner.uid);
                assert_ne!(accepted_owner.inode, owner.inode);
                let connected = String::from_utf8(table(kind).unwrap()).unwrap();
                assert!(connected.lines().any(|line| line.contains(&token) && line.split_whitespace().nth(3) == Some("01")));
                let enabled = 1i32;
                assert_eq!(setsockopt([accepted as u64, 1, L_SO_KEEPALIVE, (&enabled as *const i32) as u64, 4, 0]), 0);
                assert_eq!(table(kind).err(), Some(errno::from_darwin(libc::EOPNOTSUPP)));
                let disabled = 0i32;
                assert_eq!(setsockopt([accepted as u64, 1, L_SO_KEEPALIVE, (&disabled as *const i32) as u64, 4, 0]), 0);
                assert_eq!(table(kind).err(), Some(errno::from_darwin(libc::EOPNOTSUPP)));
                super::super::super::fs::close([accepted as u64,0,0,0,0,0]);
                super::super::super::fs::close([client as u64,0,0,0,0,0]);
            }

            let dup = unsafe { libc::dup(fd) }; assert!(dup >= 0);
            fdtab::on_dup(fd, dup);
            assert_eq!(String::from_utf8(table(kind).unwrap()).unwrap().lines().filter(|line| line.contains(&token)).count(), 1);
            super::super::super::fs::close([dup as u64,0,0,0,0,0]);
            let path = format!("/proc/net/{kind}");
            let stat = super::super::super::procfs::stat(&path, true).unwrap().unwrap();
            assert_eq!(stat.st_mode & libc::S_IFMT, libc::S_IFREG);
            let opened = super::super::super::procfs::open(&path, 0, libc::O_RDONLY).unwrap();
            assert!(opened >= 0);
            let mut bytes = vec![0; 8192];
            let size = unsafe { libc::read(opened as i32, bytes.as_mut_ptr().cast(), bytes.len()) };
            assert!(size > 0); bytes.truncate(size as usize);
            assert!(String::from_utf8(bytes).unwrap().contains(&token));
            super::super::super::fs::close([opened as u64,0,0,0,0,0]);
            assert_eq!(super::super::super::procfs::open(&path, 1, libc::O_WRONLY), Some(-13));
            owned.push((fd, kind, token));
        }
        assert!(!String::from_utf8(table("tcp").unwrap()).unwrap().contains(&format!("0100007F:{unrelated:04X}")));
        assert!(matches!(super::super::super::procfs::node("/proc/net"), Some(super::super::super::procfs::Node::Link(path)) if path == "self/net"));
        assert!(matches!(super::super::super::procfs::node("/proc/self/net"), Some(super::super::super::procfs::Node::Dir(_))));
        for (fd, kind, token) in owned {
            super::super::super::fs::close([fd as u64,0,0,0,0,0]);
            assert!(!String::from_utf8(table(kind).unwrap()).unwrap().contains(&token));
        }
    }
}
