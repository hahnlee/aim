//! The kernel's IPv6 configuration of each device (`ipv6_devconf`,
//! docs/network.md): `/proc/sys/net/ipv6/conf/{all,default,lo,eth0}/*`,
//! kept with the devices (`netif`) so that every process of a boot sees
//! one value and a write takes effect as Linux's `addrconf_sysctl_*`
//! handlers make it (`disable_ipv6`, `addr_gen_mode`, `stable_secret`
//! act; the rest are read where the kernel reads them).
//!
//! The IPv4 and neighbour directories (`ipv4/conf/<dev>`,
//! `ipv{4,6}/neigh/<dev>`) list the devices; their values are not
//! emulated and are kept as init's writes are (`procfs`).

use std::collections::BTreeMap;
use std::net::Ipv6Addr;

use super::dir::{self, Entry};
use super::netif;
use super::procfs::Node;
use crate::errno::{EINVAL, Errno};

const EIO: Errno = 5;
const EACCES: Errno = 13;
const CAP_NET_ADMIN: u32 = 12;

/// `IN6_ADDR_GEN_MODE_*`.
pub const ADDR_GEN_EUI64: i32 = 0;
pub const ADDR_GEN_NONE: i32 = 1;
pub const ADDR_GEN_STABLE_PRIVACY: i32 = 2;
pub const ADDR_GEN_RANDOM: i32 = 3;

/// The sysctls of `addrconf_sysctl` at Linux 6.6 (and Android's
/// `accept_ra_rt_table`), with `ipv6_devconf_dflt`'s values; `mtu` is a
/// device's own MTU, and `all` differs only in `accept_dad`.
const KEYS: &[(&str, i32)] = &[
    ("accept_dad", 1),
    ("accept_ra", 1),
    ("accept_ra_defrtr", 1),
    ("accept_ra_from_local", 0),
    ("accept_ra_min_hop_limit", 1),
    ("accept_ra_min_lft", 0),
    ("accept_ra_mtu", 1),
    ("accept_ra_pinfo", 1),
    ("accept_ra_rt_info_max_plen", 0),
    ("accept_ra_rt_info_min_plen", 0),
    ("accept_ra_rt_table", 0),
    ("accept_ra_rtr_pref", 1),
    ("accept_redirects", 1),
    ("accept_source_route", 0),
    ("accept_untracked_na", 0),
    ("addr_gen_mode", ADDR_GEN_EUI64),
    ("autoconf", 1),
    ("dad_transmits", 1),
    ("disable_ipv6", 0),
    ("disable_policy", 0),
    ("drop_unicast_in_l2_multicast", 0),
    ("drop_unsolicited_na", 0),
    ("enhanced_dad", 1),
    ("force_mld_version", 0),
    ("force_tllao", 0),
    ("forwarding", 0),
    ("hop_limit", 64),
    ("ignore_routes_with_linkdown", 0),
    ("ioam6_enabled", 0),
    ("ioam6_id", 65535),
    ("ioam6_id_wide", -1),
    ("keep_addr_on_down", 0),
    ("max_addresses", 16),
    ("max_desync_factor", 600),
    ("mc_forwarding", 0),
    ("mldv1_unsolicited_report_interval", 10000),
    ("mldv2_unsolicited_report_interval", 1000),
    ("mtu", 1280),
    ("ndisc_evict_nocarrier", 1),
    ("ndisc_notify", 0),
    ("ndisc_tclass", 0),
    ("optimistic_dad", 0),
    ("proxy_ndp", 0),
    ("ra_defrtr_metric", 1024),
    ("regen_max_retry", 3),
    ("router_probe_interval", 60),
    ("router_solicitation_delay", 1),
    ("router_solicitation_interval", 4),
    ("router_solicitation_max_interval", 3600),
    ("router_solicitations", -1),
    ("rpl_seg_enabled", 0),
    ("seg6_enabled", 0),
    ("seg6_require_hmac", 0),
    ("stable_secret", 0),
    ("suppress_frag_ndisc", 1),
    ("temp_prefered_lft", 86400),
    ("temp_valid_lft", 604800),
    ("use_oif_addrs_only", 0),
    ("use_optimistic", 0),
    ("use_tempaddr", 0),
];

/// Which configuration a directory holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    Default,
    Dev(u32),
}

/// One device's (or `all`'s or `default`'s) configuration: the values set
/// away from the defaults, and the RFC 7217 secret once one is written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Conf6 {
    set: BTreeMap<&'static str, i32>,
    pub secret: Option<Ipv6Addr>,
}

fn key(name: &str) -> Option<&'static str> {
    KEYS.iter().map(|(k, _)| *k).find(|k| *k == name)
}

impl Conf6 {
    /// `key`'s value in a configuration of `scope`; `mtu` is a device's MTU.
    pub fn get(&self, name: &str, scope: Scope, mtu: u32) -> i32 {
        if let Some(v) = self.set.get(name) {
            return *v;
        }
        match (name, scope) {
            ("accept_dad", Scope::All) => 0,
            ("mtu", Scope::Dev(_)) => mtu as i32,
            _ => KEYS.iter().find(|(k, _)| *k == name).map_or(0, |(_, v)| *v),
        }
    }

    pub fn put(&mut self, name: &str, value: i32) {
        if let Some(k) = key(name) {
            self.set.insert(k, value);
        }
    }

    /// The state file's lines of the values set (`netif`).
    pub fn lines(&self, scope: &str) -> String {
        let mut s = String::new();
        for (k, v) in &self.set {
            s += &format!("conf6 {scope} {k} {v}\n");
        }
        if let Some(secret) = self.secret {
            s += &format!("secret6 {scope} {secret}\n");
        }
        s
    }
}

/// `ipv6_accept_ra`: whether the device takes router advertisements.
pub fn accept_ra(c: &Conf6, scope: Scope) -> bool {
    let ra = c.get("accept_ra", scope, 0);
    if c.get("forwarding", scope, 0) != 0 {
        ra == 2
    } else {
        ra != 0
    }
}

/// `%pI6`: eight groups of four hexadecimal digits.
fn full_text(a: Ipv6Addr) -> String {
    a.segments()
        .iter()
        .map(|s| format!("{s:04x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// `proc_dointvec`'s parse of one integer: blanks, an optional sign,
/// decimal digits, blanks.
fn parse_int(b: &[u8]) -> Result<i32, Errno> {
    let s = std::str::from_utf8(b).map_err(|_| EINVAL)?;
    let s = s.trim_matches(|c: char| c.is_ascii_whitespace() || c == '\0');
    let digits = s.strip_prefix('+').unwrap_or(s);
    if digits.is_empty() || digits.starts_with('+') {
        return Err(EINVAL);
    }
    digits
        .parse::<i64>()
        .ok()
        .and_then(|v| i32::try_from(v).ok())
        .ok_or(EINVAL)
}

/// The value a write sets, with the ranges the handlers check.
fn checked(name: &str, value: i32, mtu: u32) -> Result<i32, Errno> {
    let ok = match name {
        "hop_limit" => (1..=255).contains(&value),
        "mtu" => value >= 1280 && (mtu == 0 || value as u32 <= mtu),
        "addr_gen_mode" => (ADDR_GEN_EUI64..=ADDR_GEN_RANDOM).contains(&value),
        "ndisc_tclass" => (0..=255).contains(&value),
        "ioam6_id" => (0..=65535).contains(&value),
        _ => true,
    };
    if ok { Ok(value) } else { Err(EINVAL) }
}

/// `net_ctl_permissions`: root or CAP_NET_ADMIN has the owner's bits.
fn privileged() -> bool {
    super::attrs::ids(super::attrs::EFFECTIVE).0 == 0 || super::cred::capable(CAP_NET_ADMIN)
}

/// A configuration file's mode: `stable_secret` is 0600, `mc_forwarding`
/// 0444, the rest 0644.
fn mode(name: &str) -> u32 {
    match name {
        "stable_secret" => 0o600,
        "mc_forwarding" => 0o444,
        _ => 0o644,
    }
}

/// A conf file named by its path below `/proc/sys/net/ipv6/conf`.
fn file(rest: &str) -> Option<(Scope, &'static str)> {
    let (dir, name) = rest.split_once('/')?;
    Some((scope_of(dir)?, key(name)?))
}

fn scope_of(dir: &str) -> Option<Scope> {
    match dir {
        "all" => Some(Scope::All),
        "default" => Some(Scope::Default),
        name => netif::by_name(name).map(|l| Scope::Dev(l.index)),
    }
}

fn device_dirs(with_all: bool) -> Vec<Entry> {
    let mut names: Vec<String> = Vec::new();
    if with_all {
        names.push("all".into());
    }
    names.push("default".into());
    names.extend(netif::links().into_iter().map(|l| l.name));
    names
        .into_iter()
        .map(|n| Entry::new(1, dir::DT_DIR, n))
        .collect()
}

/// The directories of `/proc/sys/net`, and the IPv6 configuration's
/// files. `tail` is the path below `/proc/sys/net`.
pub fn node(tail: &str) -> Option<Node> {
    let dirs = |names: &[&str]| {
        Node::Dir(
            names
                .iter()
                .map(|n| Entry::new(1, dir::DT_DIR, *n))
                .collect(),
        )
    };
    Some(match tail {
        "" => dirs(&["ipv4", "ipv6"]),
        "ipv4" | "ipv6" => dirs(&["conf", "neigh"]),
        "ipv4/conf" | "ipv6/conf" => Node::Dir(device_dirs(true)),
        "ipv4/neigh" | "ipv6/neigh" => Node::Dir(device_dirs(false)),
        _ => {
            if let Some(rest) = tail.strip_prefix("ipv6/conf/") {
                if !rest.contains('/') {
                    scope_of(rest)?;
                    return Some(Node::Dir(
                        KEYS.iter()
                            .map(|(k, _)| Entry::new(1, dir::DT_REG, *k))
                            .collect(),
                    ));
                }
                let (scope, name) = file(rest)?;
                return Some(Node::File(read(scope, name).unwrap_or_default()));
            }
            // A device's IPv4 and neighbour directories: their values are
            // init's writes.
            let (_, dev) = ["ipv4/conf/", "ipv4/neigh/", "ipv6/neigh/"]
                .iter()
                .find_map(|p| Some(((), tail.strip_prefix(p)?)))?;
            if dev.contains('/')
                || (dev != "all" && dev != "default" && netif::by_name(dev).is_none())
            {
                return None;
            }
            Node::Dir(Vec::new())
        }
    })
}

/// A configuration file's contents: `%d\n`, the secret as `%pI6\n`;
/// reading a secret never written is EIO.
fn read(scope: Scope, name: &str) -> Result<Vec<u8>, Errno> {
    let (c, mtu) = netif::conf6(scope)?;
    if name == "stable_secret" {
        return c
            .secret
            .map(|s| format!("{}\n", full_text(s)).into_bytes())
            .ok_or(EIO);
    }
    Ok(format!("{}\n", c.get(name, scope, mtu)).into_bytes())
}

/// open of a configuration file under `/proc/sys/net/ipv6/conf` (`rest`
/// is the path below it). None: not one.
pub fn open(rest: &str, read_access: bool, write: bool, cloexec: bool) -> Option<i64> {
    let (scope, name) = file(rest)?;
    let m = if privileged() {
        mode(name) >> 6
    } else {
        mode(name)
    };
    if (read_access && m & 0o4 == 0) || (write && m & 0o2 == 0) {
        return Some(-(EACCES as i64));
    }
    // A secret never written reads as EIO: refused here, as the read
    // would be.
    let contents = match read(scope, name) {
        Ok(c) => c,
        Err(e) if read_access => return Some(-(e as i64)),
        Err(_) => Vec::new(),
    };
    Some(super::knob::open(&contents, cloexec, move |b| {
        let (_, mtu) = netif::conf6(scope)?;
        if name == "stable_secret" {
            return set_secret(scope, b).map(|()| None);
        }
        let value = checked(
            name,
            parse_int(b)?,
            if matches!(scope, Scope::Dev(_)) {
                mtu
            } else {
                0
            },
        )?;
        netif::set_conf6(scope, name, value)?;
        Ok(Some(format!("{value}\n").into_bytes()))
    }))
}

/// `addrconf_sysctl_stable_secret`'s write: an IPv6 address; `all` has
/// none (EIO).
fn set_secret(scope: Scope, b: &[u8]) -> Result<(), Errno> {
    if scope == Scope::All {
        return Err(EIO);
    }
    let s = std::str::from_utf8(b).map_err(|_| EIO)?;
    let secret: Ipv6Addr = s.trim().parse().map_err(|_| EIO)?;
    netif::set_secret6(scope, secret)
}

/// The mode of a configuration file, for stat.
pub fn file_mode(rest: &str) -> Option<u32> {
    let (_, name) = file(rest)?;
    Some(mode(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_ipv6_devconf() {
        let c = Conf6::default();
        assert_eq!(c.get("accept_ra", Scope::Dev(2), 1500), 1);
        assert_eq!(c.get("accept_dad", Scope::All, 0), 0);
        assert_eq!(c.get("accept_dad", Scope::Default, 0), 1);
        assert_eq!(c.get("mtu", Scope::Dev(2), 1500), 1500);
        assert_eq!(c.get("mtu", Scope::Default, 0), 1280);
        assert_eq!(c.get("router_solicitations", Scope::Dev(2), 0), -1);
    }

    #[test]
    fn writes_parse_as_proc_dointvec() {
        assert_eq!(parse_int(b"2\n"), Ok(2));
        assert_eq!(parse_int(b"  -1000 "), Ok(-1000));
        assert_eq!(parse_int(b"+7"), Ok(7));
        assert_eq!(parse_int(b"two"), Err(EINVAL));
        assert_eq!(parse_int(b""), Err(EINVAL));
        assert_eq!(parse_int(b"99999999999"), Err(EINVAL));
        assert_eq!(checked("hop_limit", 0, 0), Err(EINVAL));
        assert_eq!(checked("addr_gen_mode", 4, 0), Err(EINVAL));
        assert_eq!(checked("mtu", 1600, 1500), Err(EINVAL));
        assert_eq!(checked("mtu", 1400, 1500), Ok(1400));
    }

    #[test]
    fn accept_ra_with_forwarding_needs_2() {
        let mut c = Conf6::default();
        assert!(accept_ra(&c, Scope::Dev(2)));
        c.put("forwarding", 1);
        assert!(!accept_ra(&c, Scope::Dev(2)));
        c.put("accept_ra", 2);
        assert!(accept_ra(&c, Scope::Dev(2)));
        c.put("accept_ra", 0);
        c.put("forwarding", 0);
        assert!(!accept_ra(&c, Scope::Dev(2)));
    }

    #[test]
    fn secrets_read_in_full() {
        assert_eq!(
            full_text("2001:db8::1".parse().unwrap()),
            "2001:0db8:0000:0000:0000:0000:0000:0001"
        );
        let mut c = Conf6::default();
        c.put("use_tempaddr", 2);
        c.secret = Some("::1".parse().unwrap());
        assert_eq!(c.lines("2"), "conf6 2 use_tempaddr 2\nsecret6 2 ::1\n");
    }
}
