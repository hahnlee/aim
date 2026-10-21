//! `/proc/sys/net` values, written and read the way the original netd's
//! InterfaceController does (netd `server/InterfaceController.cpp` at
//! android-16.0.0_r1): the kernel's IPv6 configuration takes effect there
//! (`disable_ipv6`, `addr_gen_mode`, `stable_secret`), and the other values
//! are kept by the kernel side as written.

use std::ffi::{CStr, CString};

use crate::interfaces::Errno;

const IPV6_CONF: &str = "/proc/sys/net/ipv6/conf";
const IPV4_CONF: &str = "/proc/sys/net/ipv4/conf";
const IPV4_NEIGH: &str = "/proc/sys/net/ipv4/neigh";
const IPV6_NEIGH: &str = "/proc/sys/net/ipv6/neigh";
/// The RFC 7217 secret netd keeps across boots.
const STABLE_SECRET_PROPERTY: &CStr = c"persist.netd.stable_secret";

/// INetd's IPV6_ADDR_GEN_MODE_*.
const ADDR_GEN_EUI64: i32 = 0;
const ADDR_GEN_STABLE_PRIVACY: i32 = 2;

unsafe extern "C" {
    fn __system_property_get(name: *const libc::c_char, value: *mut libc::c_char) -> libc::c_int;
    fn __system_property_set(name: *const libc::c_char, value: *const libc::c_char) -> libc::c_int;
}

fn last_errno() -> Errno {
    std::io::Error::last_os_error()
        .raw_os_error()
        .unwrap_or(libc::EIO)
}

/// `isNormalPathComponent`.
fn normal(c: &str) -> bool {
    c != "." && c != ".." && !c.contains('/')
}

/// `isInterfaceName` (of a path component).
fn interface_name(c: &str) -> bool {
    normal(c) && c != "default" && c != "all"
}

/// NetdConstants' `isIfaceName`: alphanumerics and `_-:.`, shorter than
/// IFNAMSIZ.
pub fn is_iface_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() < 16
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-:.".contains(&b))
}

/// `WriteStringToFile`: the value written, or the errno.
fn write_file(path: &str, value: &str) -> Result<(), Errno> {
    std::fs::write(path, value).map_err(|e| e.raw_os_error().unwrap_or(libc::EIO))
}

/// `writeValueToPath`: any failure is EREMOTEIO.
fn write_value(dir: &str, sub: &str, name: &str, value: &str) -> Result<(), Errno> {
    write_file(&format!("{dir}/{sub}/{name}"), value).map_err(|_| libc::EREMOTEIO)
}

/// `forEachInterface`: `default`, then each interface directory.
fn on_all_interfaces(dir: &str, name: &str, value: &str) {
    let _ = write_value(dir, "default", name, value);
    let Ok(entries) = std::fs::read_dir(dir) else {
        log::error!("Can't list {dir}");
        return;
    };
    for e in entries.flatten() {
        let iface = e.file_name().to_string_lossy().into_owned();
        if e.file_type().is_ok_and(|t| t.is_dir()) && interface_name(&iface) {
            let _ = write_value(dir, &iface, name, value);
        }
    }
}

/// `InterfaceController::initializeAll`: the IPv6 and neighbour settings
/// netd makes at start.
pub fn initialize_all() {
    // Accept RAs even with forwarding on; RIOs of /48 to /64; RA routes
    // in per-interface tables (ROUTE_TABLE_OFFSET_FROM_INDEX); optimistic
    // DAD; a 15 s base reachable time; source addresses of the outgoing
    // interface only; no ICMP redirects.
    on_all_interfaces(IPV6_CONF, "accept_ra", "2");
    on_all_interfaces(IPV6_CONF, "accept_ra_rt_info_min_plen", "48");
    on_all_interfaces(IPV6_CONF, "accept_ra_rt_info_max_plen", "64");
    on_all_interfaces(IPV6_CONF, "accept_ra_rt_table", "-1000");
    on_all_interfaces(IPV6_CONF, "optimistic_dad", "1");
    on_all_interfaces(IPV6_CONF, "use_optimistic", "1");
    on_all_interfaces(IPV4_NEIGH, "base_reachable_time_ms", "15000");
    on_all_interfaces(IPV6_NEIGH, "base_reachable_time_ms", "15000");
    on_all_interfaces(IPV6_CONF, "use_oif_addrs_only", "1");
    let _ = write_value(IPV4_CONF, "all", "accept_redirects", "0");
    let _ = write_value(IPV6_CONF, "all", "accept_redirects", "0");
    on_all_interfaces(IPV4_CONF, "accept_redirects", "0");
    on_all_interfaces(IPV6_CONF, "accept_redirects", "0");
}

/// `getPathComponents` and `getParameterPathname`.
fn parameter_path(
    ipversion: i32,
    which: i32,
    ifname: &str,
    parameter: &str,
) -> Result<String, Errno> {
    let family = match ipversion {
        4 => "ipv4",
        6 => "ipv6",
        _ => return Err(libc::EAFNOSUPPORT),
    };
    let kind = match which {
        1 => "conf",
        2 => "neigh",
        _ => return Err(libc::EINVAL),
    };
    if !interface_name(ifname) || !normal(parameter) {
        return Err(libc::EINVAL);
    }
    Ok(format!(
        "/proc/sys/net/{family}/{kind}/{ifname}/{parameter}"
    ))
}

/// `getProcSysNet`: the value, trimmed.
pub fn get(ipversion: i32, which: i32, ifname: &str, parameter: &str) -> Result<String, Errno> {
    let path = parameter_path(ipversion, which, ifname, parameter)?;
    std::fs::read_to_string(path)
        .map(|v| v.trim().to_string())
        .map_err(|e| e.raw_os_error().unwrap_or(libc::EIO))
}

/// `setProcSysNet`.
pub fn set(
    ipversion: i32,
    which: i32,
    ifname: &str,
    parameter: &str,
    value: &str,
) -> Result<(), Errno> {
    write_file(&parameter_path(ipversion, which, ifname, parameter)?, value)
}

/// `setEnableIPv6`: `disable_ipv6`, whose change starts or stops
/// autoconfiguration.
pub fn set_enable_ipv6(ifname: &str, on: bool) -> Result<(), Errno> {
    if !is_iface_name(ifname) {
        return Err(libc::ENOENT);
    }
    write_value(
        IPV6_CONF,
        ifname,
        "disable_ipv6",
        if on { "0" } else { "1" },
    )
}

/// `setIPv6PrivacyExtensions`: `use_tempaddr` 2 (temporary addresses,
/// preferred) or 0.
pub fn set_privacy_extensions(ifname: &str, on: bool) -> Result<(), Errno> {
    if !is_iface_name(ifname) {
        return Err(libc::ENOENT);
    }
    write_value(
        IPV6_CONF,
        ifname,
        "use_tempaddr",
        if on { "2" } else { "0" },
    )
}

/// `setIPv6AddrGenMode`: EUI-64, or RFC 7217 stable privacy addresses
/// with the persisted secret; other modes are EOPNOTSUPP.
pub fn set_addr_gen_mode(ifname: &str, mode: i32) -> Result<(), Errno> {
    if !is_iface_name(ifname) {
        return Err(libc::ENOENT);
    }
    match mode {
        ADDR_GEN_EUI64 => {
            // Without addr_gen_mode the device is in EUI-64 mode already.
            let _ = write_value(IPV6_CONF, ifname, "addr_gen_mode", "0");
            Ok(())
        }
        ADDR_GEN_STABLE_PRIVACY => enable_stable_privacy(ifname),
        _ => Err(libc::EOPNOTSUPP),
    }
}

fn property(name: &CStr) -> Option<String> {
    let mut value = [0 as libc::c_char; 92];
    // SAFETY: a property name and a PROP_VALUE_MAX buffer.
    let n = unsafe { __system_property_get(name.as_ptr(), value.as_mut_ptr()) };
    // SAFETY: NUL-terminated by the call.
    (n > 0).then(|| {
        unsafe { CStr::from_ptr(value.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    })
}

/// `randomIPv6Address`: 128 bits from /dev/random, as an address.
fn random_secret() -> Result<String, Errno> {
    use std::io::Read;
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/random")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(|e| e.raw_os_error().unwrap_or(libc::EIO))?;
    Ok(std::net::Ipv6Addr::from(b).to_string())
}

/// `enableStablePrivacyAddresses`: the persisted secret (a new one the
/// first time, then persisted) written to the device's `stable_secret`.
fn enable_stable_privacy(ifname: &str) -> Result<(), Errno> {
    use std::io::Write;
    let path = format!("{IPV6_CONF}/{ifname}/stable_secret");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .map_err(|e| match e.raw_os_error() {
            // Kernels before 4.4 have no stable privacy addresses.
            Some(libc::ENOENT) => libc::EOPNOTSUPP,
            Some(e) => e,
            None => libc::EIO,
        })?;
    let old = property(STABLE_SECRET_PROPERTY);
    let secret = match &old {
        Some(s) => s.clone(),
        None => random_secret()?,
    };
    f.write_all(secret.as_bytes())
        .map_err(|e| e.raw_os_error().unwrap_or(libc::EIO))?;
    if old.is_some() {
        return Ok(());
    }
    let value = CString::new(secret).map_err(|_| libc::EINVAL)?;
    // SAFETY: a property name and value.
    if unsafe { __system_property_set(STABLE_SECRET_PROPERTY.as_ptr(), value.as_ptr()) } != 0 {
        return Err(last_errno());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_paths_follow_interface_controller() {
        assert_eq!(
            parameter_path(6, 1, "eth0", "accept_ra"),
            Ok("/proc/sys/net/ipv6/conf/eth0/accept_ra".into())
        );
        assert_eq!(
            parameter_path(4, 2, "lo", "ucast_solicit"),
            Ok("/proc/sys/net/ipv4/neigh/lo/ucast_solicit".into())
        );
        assert_eq!(parameter_path(5, 1, "eth0", "x"), Err(libc::EAFNOSUPPORT));
        assert_eq!(parameter_path(6, 3, "eth0", "x"), Err(libc::EINVAL));
        assert_eq!(parameter_path(6, 1, "all", "x"), Err(libc::EINVAL));
        assert_eq!(parameter_path(6, 1, "eth0", "../x"), Err(libc::EINVAL));
    }

    #[test]
    fn interface_names() {
        assert!(is_iface_name("eth0"));
        assert!(is_iface_name("v4-wlan0"));
        assert!(!is_iface_name(""));
        assert!(!is_iface_name("a/b"));
        assert!(!is_iface_name("0123456789abcdef"));
    }
}
