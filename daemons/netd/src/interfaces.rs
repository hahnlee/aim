//! Interfaces as the kernel has them, set and read the way the original
//! netd's InterfaceController does (libnetutils' ifc_utils): addresses,
//! flags and MTU through the socket ioctls, other addresses through
//! rtnetlink. The kernel announces each change to its netlink groups, which
//! is how EthernetTracker and IpClient learn of them.

use std::ffi::CStr;
use std::net::{IpAddr, Ipv4Addr};

use android_net_netd::aidl::android::net::{
    INetd, InterfaceConfigurationParcel::InterfaceConfigurationParcel,
};

const SIOCGIFFLAGS: libc::c_int = 0x8913;
const SIOCSIFFLAGS: libc::c_int = 0x8914;
const SIOCGIFADDR: libc::c_int = 0x8915;
const SIOCSIFADDR: libc::c_int = 0x8916;
const SIOCGIFNETMASK: libc::c_int = 0x891b;
const SIOCSIFNETMASK: libc::c_int = 0x891c;
const SIOCSIFMTU: libc::c_int = 0x8922;
const SIOCGIFHWADDR: libc::c_int = 0x8927;

/// A failure as the errno netd reports in a ServiceSpecificException.
pub type Errno = i32;

fn errno() -> Errno {
    std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EIO)
}

/// `struct ifreq`: the name, then a 24-byte union.
#[repr(C)]
struct IfReq {
    name: [u8; 16],
    data: [u8; 24],
}

impl IfReq {
    fn new(ifname: &str) -> Result<IfReq, Errno> {
        if ifname.is_empty() || ifname.len() >= 16 {
            return Err(libc::ENODEV);
        }
        let mut r = IfReq {
            name: [0; 16],
            data: [0; 24],
        };
        r.name[..ifname.len()].copy_from_slice(ifname.as_bytes());
        Ok(r)
    }

    fn ipv4(&self) -> Ipv4Addr {
        Ipv4Addr::new(self.data[4], self.data[5], self.data[6], self.data[7])
    }

    fn set_ipv4(&mut self, a: Ipv4Addr) {
        self.data = [0; 24];
        self.data[..2].copy_from_slice(&(libc::AF_INET as u16).to_ne_bytes());
        self.data[4..8].copy_from_slice(&a.octets());
    }

    fn short(&self) -> u16 {
        u16::from_ne_bytes([self.data[0], self.data[1]])
    }
}

/// Run socket ioctl `req` on `r` with a datagram socket, as ifc_utils does.
fn ioctl(req: libc::c_int, r: &mut IfReq) -> Result<(), Errno> {
    // SAFETY: a socket of our own for the call, closed below.
    let s = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM | libc::SOCK_CLOEXEC, 0) };
    if s < 0 {
        return Err(errno());
    }
    // SAFETY: an ifreq-sized buffer for an interface ioctl.
    let rc = unsafe { libc::ioctl(s, req as _, r as *mut IfReq) };
    let e = errno();
    // SAFETY: closing our socket.
    unsafe { libc::close(s) };
    if rc < 0 { Err(e) } else { Ok(()) }
}

/// The kernel's interface names.
pub fn list() -> Vec<String> {
    let mut out = Vec::new();
    // SAFETY: if_nameindex's array, freed below.
    unsafe {
        let all = libc::if_nameindex();
        if all.is_null() {
            return out;
        }
        let mut p = all;
        while (*p).if_index != 0 {
            out.push(CStr::from_ptr((*p).if_name).to_string_lossy().into_owned());
            p = p.add(1);
        }
        libc::if_freenameindex(all);
    }
    out
}

fn prefix_of(mask: Ipv4Addr) -> i32 {
    u32::from(mask).leading_ones() as i32
}

/// InterfaceController::getCfg.
pub fn get_cfg(ifname: &str) -> Result<InterfaceConfigurationParcel, Errno> {
    let mut r = IfReq::new(ifname)?;
    ioctl(SIOCGIFHWADDR, &mut r)?;
    let hw = r.data[2..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":");
    let (mut addr, mut prefix) = (Ipv4Addr::UNSPECIFIED, 0);
    if ioctl(SIOCGIFADDR, &mut r).is_ok() {
        addr = r.ipv4();
        if ioctl(SIOCGIFNETMASK, &mut r).is_ok() {
            prefix = prefix_of(r.ipv4());
        }
    }
    ioctl(SIOCGIFFLAGS, &mut r)?;
    let f = r.short() as i32;
    let mut flags = vec![if f & libc::IFF_UP != 0 {
        INetd::IF_STATE_UP
    } else {
        INetd::IF_STATE_DOWN
    }];
    for (bit, name) in [
        (libc::IFF_BROADCAST, INetd::IF_FLAG_BROADCAST),
        (libc::IFF_LOOPBACK, INetd::IF_FLAG_LOOPBACK),
        (libc::IFF_POINTOPOINT, INetd::IF_FLAG_POINTOPOINT),
        (libc::IFF_RUNNING, INetd::IF_FLAG_RUNNING),
        (libc::IFF_MULTICAST, INetd::IF_FLAG_MULTICAST),
    ] {
        if f & bit != 0 {
            flags.push(name);
        }
    }
    Ok(InterfaceConfigurationParcel {
        ifName: ifname.into(),
        hwAddr: hw,
        ipv4Addr: addr.to_string(),
        prefixLength: prefix,
        flags: flags.into_iter().map(String::from).collect(),
    })
}

fn set_up(ifname: &str, up: bool) -> Result<(), Errno> {
    let mut r = IfReq::new(ifname)?;
    ioctl(SIOCGIFFLAGS, &mut r)?;
    let mut f = r.short();
    if up {
        f |= libc::IFF_UP as u16;
    } else {
        f &= !(libc::IFF_UP as u16);
    }
    r.data[..2].copy_from_slice(&f.to_ne_bytes());
    ioctl(SIOCSIFFLAGS, &mut r)
}

/// InterfaceController::setCfg: the IPv4 address (0.0.0.0 removes it), its
/// prefix, then "up" or "down".
pub fn set_cfg(cfg: &InterfaceConfigurationParcel) -> Result<(), Errno> {
    let addr: Ipv4Addr = cfg.ipv4Addr.parse().map_err(|_| libc::EINVAL)?;
    let mut r = IfReq::new(&cfg.ifName)?;
    r.set_ipv4(addr);
    ioctl(SIOCSIFADDR, &mut r)?;
    if !addr.is_unspecified() {
        if !(0..=32).contains(&cfg.prefixLength) {
            return Err(libc::EINVAL);
        }
        let mask = u32::MAX
            .checked_shl(32 - cfg.prefixLength as u32)
            .unwrap_or(0);
        r.set_ipv4(Ipv4Addr::from(mask));
        ioctl(SIOCSIFNETMASK, &mut r)?;
    }
    for flag in &cfg.flags {
        match flag.as_str() {
            INetd::IF_STATE_UP => set_up(&cfg.ifName, true)?,
            INetd::IF_STATE_DOWN => set_up(&cfg.ifName, false)?,
            _ => {}
        }
    }
    Ok(())
}

pub fn set_mtu(ifname: &str, mtu: i32) -> Result<(), Errno> {
    let mut r = IfReq::new(ifname)?;
    r.data[..4].copy_from_slice(&mtu.to_ne_bytes());
    ioctl(SIOCSIFMTU, &mut r)
}

fn index_of(ifname: &str) -> Result<u32, Errno> {
    let c = std::ffi::CString::new(ifname).map_err(|_| libc::ENODEV)?;
    // SAFETY: a NUL-terminated name.
    match unsafe { libc::if_nametoindex(c.as_ptr()) } {
        0 => Err(libc::ENODEV),
        i => Ok(i),
    }
}

// ---- rtnetlink ----------------------------------------------------------------

const RTM_NEWADDR: u16 = 20;
const RTM_DELADDR: u16 = 21;
const RTM_GETADDR: u16 = 22;
const NLMSG_ERROR: u16 = 2;
const NLMSG_DONE: u16 = 3;
const NLM_F_REQUEST: u16 = 0x1;
const NLM_F_ACK: u16 = 0x4;
const NLM_F_REPLACE: u16 = 0x100;
const NLM_F_DUMP: u16 = 0x300;
const NLM_F_CREATE: u16 = 0x400;
const IFA_ADDRESS: u16 = 1;
const IFA_LOCAL: u16 = 2;

struct Rtnl(i32);

impl Drop for Rtnl {
    fn drop(&mut self) {
        // SAFETY: closing our socket.
        unsafe { libc::close(self.0) };
    }
}

impl Rtnl {
    fn open() -> Result<Rtnl, Errno> {
        // SAFETY: a netlink socket of our own.
        let s = unsafe {
            libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_DGRAM | libc::SOCK_CLOEXEC,
                libc::NETLINK_ROUTE,
            )
        };
        if s < 0 { Err(errno()) } else { Ok(Rtnl(s)) }
    }

    /// Send one request and call `f` on each reply until the ack (or the
    /// end of a dump).
    fn transact(&self, msg: &[u8], mut f: impl FnMut(u16, &[u8])) -> Result<(), Errno> {
        // SAFETY: sending our buffer.
        if unsafe { libc::send(self.0, msg.as_ptr().cast(), msg.len(), 0) } < 0 {
            return Err(errno());
        }
        let mut buf = vec![0u8; 16384];
        loop {
            // SAFETY: receiving into our buffer.
            let n = unsafe { libc::recv(self.0, buf.as_mut_ptr().cast(), buf.len(), 0) };
            if n < 0 {
                return Err(errno());
            }
            let mut at = 0;
            while at + 16 <= n as usize {
                let len = u32::from_ne_bytes(buf[at..at + 4].try_into().unwrap()) as usize;
                if len < 16 || at + len > n as usize {
                    break;
                }
                let ty = u16::from_ne_bytes([buf[at + 4], buf[at + 5]]);
                let body = &buf[at + 16..at + len];
                match ty {
                    NLMSG_DONE => return Ok(()),
                    NLMSG_ERROR => {
                        let e = i32::from_ne_bytes(body[..4].try_into().unwrap());
                        return if e == 0 { Ok(()) } else { Err(-e) };
                    }
                    _ => f(ty, body),
                }
                at += (len + 3) & !3;
            }
        }
    }
}

fn message(ty: u16, flags: u16, body: &[u8]) -> Vec<u8> {
    let mut m = ((16 + body.len()) as u32).to_ne_bytes().to_vec();
    m.extend_from_slice(&ty.to_ne_bytes());
    m.extend_from_slice(&(flags | NLM_F_REQUEST).to_ne_bytes());
    m.extend_from_slice(&[0; 8]);
    m.extend_from_slice(body);
    m
}

fn attr(out: &mut Vec<u8>, ty: u16, data: &[u8]) {
    out.extend_from_slice(&((4 + data.len()) as u16).to_ne_bytes());
    out.extend_from_slice(&ty.to_ne_bytes());
    out.extend_from_slice(data);
    out.resize((out.len() + 3) & !3, 0);
}

/// struct ifaddrmsg and the address attributes of `ip/prefix` on `index`.
fn ifaddr(index: u32, ip: IpAddr, prefix: u8) -> Vec<u8> {
    let (family, bytes) = match ip {
        IpAddr::V4(a) => (libc::AF_INET as u8, a.octets().to_vec()),
        IpAddr::V6(a) => (libc::AF_INET6 as u8, a.octets().to_vec()),
    };
    let mut b = vec![family, prefix, 0, 0];
    b.extend_from_slice(&index.to_ne_bytes());
    attr(&mut b, IFA_LOCAL, &bytes);
    attr(&mut b, IFA_ADDRESS, &bytes);
    b
}

/// ifc_act_on_address: add (or replace) or remove an address.
pub fn address(ifname: &str, addr: &str, prefix: i32, add: bool) -> Result<(), Errno> {
    let ip: IpAddr = addr.parse().map_err(|_| libc::EINVAL)?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    if !(0..=max).contains(&prefix) {
        return Err(libc::EINVAL);
    }
    let body = ifaddr(index_of(ifname)?, ip, prefix as u8);
    let msg = if add {
        message(RTM_NEWADDR, NLM_F_ACK | NLM_F_CREATE | NLM_F_REPLACE, &body)
    } else {
        message(RTM_DELADDR, NLM_F_ACK, &body)
    };
    Rtnl::open()?.transact(&msg, |_, _| {})
}

/// ifc_clear_addresses: remove every address of the interface.
pub fn clear(ifname: &str) -> Result<(), Errno> {
    let index = index_of(ifname)?;
    let nl = Rtnl::open()?;
    let mut found = Vec::new();
    let dump = message(RTM_GETADDR, NLM_F_DUMP, &[0; 8]);
    nl.transact(&dump, |ty, body| {
        if ty != RTM_NEWADDR || body.len() < 8 {
            return;
        }
        if u32::from_ne_bytes(body[4..8].try_into().unwrap()) == index {
            found.push(body.to_vec());
        }
    })?;
    for b in found {
        nl.transact(&message(RTM_DELADDR, NLM_F_ACK, &b), |_, _| {})?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_request_layout() {
        let b = ifaddr(2, "10.0.2.15".parse().unwrap(), 24);
        assert_eq!(&b[..8], &[2, 24, 0, 0, 2, 0, 0, 0]);
        assert_eq!(&b[8..16], &[8, 0, 2, 0, 10, 0, 2, 15]);
        let m = message(RTM_NEWADDR, NLM_F_ACK, &b);
        assert_eq!(
            u32::from_ne_bytes(m[..4].try_into().unwrap()) as usize,
            m.len()
        );
        assert_eq!(u16::from_ne_bytes([m[6], m[7]]), NLM_F_ACK | NLM_F_REQUEST);
    }

    #[test]
    fn prefixes() {
        assert_eq!(prefix_of(Ipv4Addr::new(255, 255, 252, 0)), 22);
    }
}
