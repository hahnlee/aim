//! NETLINK_AUDIT (kernel/audit.c, Linux v6.12): the audit daemon's
//! registration and status, and userspace's audit records.
//!
//! The state the kernel keeps (`audit_enabled`, the registered auditd and
//! its port, the limits, the record serial) is shared by every guest
//! process in `<runtime>/netlink/audit`, changed under its flock. The
//! layer generates no kernel records of its own (no syscall auditing or
//! LSM denials), so the records are userspace's AUDIT_USER messages: sent
//! to a registered auditd's socket and to the AUDIT_NLGRP_READLOG group,
//! else printed to the kernel log as `kauditd_printk_skb` prints them.
//! Rule changes, TTY auditing and feature changes are not offered (#1952).

use std::os::fd::AsRawFd;
use std::path::PathBuf;

use super::netlink::{self, NLM_F_MULTI, NLMSG_DONE, nlmsg};
use crate::errno::{EACCES, EEXIST, EINVAL, EPERM, Errno};

pub const NETLINK_AUDIT: u64 = 9;
/// AUDIT_NLGRP_READLOG as a bind mask.
pub const GRP_READLOG: u32 = 1;

const AUDIT_GET: u16 = 1000;
const AUDIT_SET: u16 = 1001;
const AUDIT_LIST: u16 = 1002;
const AUDIT_ADD: u16 = 1003;
const AUDIT_DEL: u16 = 1004;
const AUDIT_USER: u16 = 1005;
const AUDIT_SIGNAL_INFO: u16 = 1010;
const AUDIT_LIST_RULES: u16 = 1013;
const AUDIT_GET_FEATURE: u16 = 1019;
const AUDIT_USER_AVC: u16 = 1107;
const AUDIT_USER_TTY: u16 = 1124;

const STATUS_ENABLED: u32 = 0x1;
const STATUS_FAILURE: u32 = 0x2;
const STATUS_PID: u32 = 0x4;
const STATUS_RATE_LIMIT: u32 = 0x8;
const STATUS_BACKLOG_LIMIT: u32 = 0x10;
const STATUS_BACKLOG_WAIT_TIME: u32 = 0x20;
const STATUS_LOST: u32 = 0x40;
const STATUS_BACKLOG_WAIT_TIME_ACTUAL: u32 = 0x80;
/// AUDIT_FEATURE_BITMAP_ALL.
const FEATURE_BITMAP_ALL: u32 = 0x7f;
/// AUDIT_BACKLOG_WAIT_TIME: 60 * HZ (CONFIG_HZ=250).
const BACKLOG_WAIT_TIME: u32 = 60 * 250;
/// AUDIT_MESSAGE_TEXT_MAX.
const MESSAGE_TEXT_MAX: usize = 8560;
/// AUDIT_LOCKED.
const LOCKED: u32 = 2;

const CAP_AUDIT_WRITE: u32 = 29;
const CAP_AUDIT_CONTROL: u32 = 30;
const CAP_AUDIT_READ: u32 = 37;
const EOPNOTSUPP: Errno = 95;

/// What the kernel keeps of audit.
#[derive(Clone, Copy, Debug, PartialEq)]
struct State {
    enabled: u32,
    failure: u32,
    /// The registered auditd's pid and port id (0: none).
    pid: u32,
    portid: u32,
    rate_limit: u32,
    backlog_limit: u32,
    lost: u32,
    backlog_wait_time: u32,
    serial: u32,
}

impl Default for State {
    /// The boot defaults: audit off (no `audit=1`), failures printed.
    fn default() -> State {
        State {
            enabled: 0,
            failure: 1,
            pid: 0,
            portid: 0,
            rate_limit: 0,
            backlog_limit: 64,
            lost: 0,
            backlog_wait_time: BACKLOG_WAIT_TIME,
            serial: 0,
        }
    }
}

impl State {
    fn fields(&mut self) -> [&mut u32; 9] {
        [
            &mut self.enabled,
            &mut self.failure,
            &mut self.pid,
            &mut self.portid,
            &mut self.rate_limit,
            &mut self.backlog_limit,
            &mut self.lost,
            &mut self.backlog_wait_time,
            &mut self.serial,
        ]
    }

    fn encode(mut self) -> String {
        self.fields().map(|v| v.to_string()).join(" ")
    }

    fn decode(text: &str) -> Option<State> {
        let mut s = State::default();
        let mut values = text.split_whitespace().map(str::parse::<u32>);
        for v in s.fields() {
            *v = values.next()?.ok()?;
        }
        Some(s)
    }

    /// `struct audit_status`.
    fn status(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(44);
        for v in [
            0,
            self.enabled,
            self.failure,
            self.pid,
            self.rate_limit,
            self.backlog_limit,
            self.lost,
            0,
            FEATURE_BITMAP_ALL,
            self.backlog_wait_time,
            0,
        ] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }
}

fn path() -> PathBuf {
    super::netif::kernel_dir("netlink").join("audit")
}

/// Runs `f` on the shared state under its lock, storing what it leaves.
fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> Result<T, Errno> {
    let p = path();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&p)
        .map_err(|e| crate::errno::from_darwin(e.raw_os_error().unwrap_or(libc::EIO)))?;
    // SAFETY: flock of the file we opened; released when it closes.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    let text = std::fs::read_to_string(&p).unwrap_or_default();
    let mut state = State::decode(&text).unwrap_or_default();
    // An auditd whose socket is gone is unregistered (`auditd_reset`).
    if state.portid != 0 && !netlink::taken(NETLINK_AUDIT, state.portid) {
        (state.pid, state.portid) = (0, 0);
    }
    let before = state;
    let r = f(&mut state);
    if state != before {
        std::fs::write(&p, state.encode())
            .map_err(|e| crate::errno::from_darwin(e.raw_os_error().unwrap_or(libc::EIO)))?;
    }
    Ok(r)
}

/// `audit_netlink_ok`: the capability a message type needs.
fn permitted(ty: u16) -> Result<(), Errno> {
    let cap = match ty {
        AUDIT_LIST | AUDIT_ADD | AUDIT_DEL => return Err(EOPNOTSUPP),
        1000..=1004 | 1010..=1019 => CAP_AUDIT_CONTROL,
        AUDIT_USER | 1100..=1199 | 2100..=2999 => CAP_AUDIT_WRITE,
        _ => return Err(EINVAL),
    };
    if super::cred::capable(cap) {
        Ok(())
    } else {
        Err(EPERM)
    }
}

/// `audit_multicast_bind`: joining AUDIT_NLGRP_READLOG needs CAP_AUDIT_READ.
pub fn bind_groups(groups: u32) -> Result<u32, Errno> {
    if groups & GRP_READLOG != 0 && !super::cred::capable(CAP_AUDIT_READ) {
        return Err(EPERM);
    }
    // The family has one group.
    Ok(groups & GRP_READLOG)
}

/// Handle one audit request (`audit_receive_msg`); its replies go to
/// `out`. The errno to report (0: success).
pub fn request(ty: u16, seq: u32, portid: u32, body: &[u8], out: &mut Vec<Vec<u8>>) -> Errno {
    if let Err(e) = permitted(ty) {
        return e;
    }
    let pid = super::process::getpid() as u32;
    let r = match ty {
        AUDIT_GET => with_state(|s| {
            out.push(nlmsg(AUDIT_GET, 0, seq, portid, &s.status()));
            Ok(())
        }),
        AUDIT_SET => with_state(|s| set(s, body, pid, portid)),
        AUDIT_LIST_RULES => {
            // No rules.
            out.push(nlmsg(
                NLMSG_DONE,
                NLM_F_MULTI,
                seq,
                portid,
                &0i32.to_le_bytes(),
            ));
            Ok(Ok(()))
        }
        AUDIT_SIGNAL_INFO => {
            // struct audit_sig_info: no signal to auditd recorded.
            let mut b = u32::MAX.to_le_bytes().to_vec();
            b.extend_from_slice(&(-1i32).to_le_bytes());
            out.push(nlmsg(AUDIT_SIGNAL_INFO, 0, seq, portid, &b));
            Ok(Ok(()))
        }
        AUDIT_GET_FEATURE => {
            // struct audit_features: version 1, no feature set or locked.
            let mut b = 1u32.to_le_bytes().to_vec();
            b.extend_from_slice(&[0; 12]);
            out.push(nlmsg(AUDIT_GET_FEATURE, 0, seq, portid, &b));
            Ok(Ok(()))
        }
        AUDIT_USER | 1100..=1199 | 2100..=2999 => with_state(|s| user_record(s, ty, body, pid)),
        _ => Ok(Err(EOPNOTSUPP)),
    };
    match r {
        Ok(Ok(())) => 0,
        Ok(Err(e)) | Err(e) => e,
    }
}

/// AUDIT_SET's changes, in the kernel's order.
fn set(s: &mut State, body: &[u8], pid: u32, portid: u32) -> Result<(), Errno> {
    let mut v = [0u32; 11];
    for (i, w) in body.chunks_exact(4).take(11).enumerate() {
        v[i] = u32::from_le_bytes(w.try_into().unwrap());
    }
    let [
        mask,
        enabled,
        failure,
        new_pid,
        rate_limit,
        backlog_limit,
        _,
        _,
        _,
        wait_time,
        _,
    ] = v;
    let locked = s.enabled == LOCKED;
    if mask & STATUS_ENABLED != 0 {
        if enabled > LOCKED {
            return Err(EINVAL);
        }
        if locked {
            return Err(EPERM);
        }
        s.enabled = enabled;
    }
    if mask & STATUS_FAILURE != 0 {
        if failure > 2 {
            return Err(EINVAL);
        }
        if locked {
            return Err(EPERM);
        }
        s.failure = failure;
    }
    if mask & STATUS_PID != 0 {
        if s.pid != 0 {
            // Replacing a healthy auditd is refused; only it unregisters.
            if new_pid != 0 {
                return Err(EEXIST);
            }
            if s.pid != pid {
                return Err(EACCES);
            }
        }
        (s.pid, s.portid) = if new_pid != 0 { (pid, portid) } else { (0, 0) };
    }
    if mask & STATUS_RATE_LIMIT != 0 {
        if locked {
            return Err(EPERM);
        }
        s.rate_limit = rate_limit;
    }
    if mask & STATUS_BACKLOG_LIMIT != 0 {
        if locked {
            return Err(EPERM);
        }
        s.backlog_limit = backlog_limit;
    }
    if mask & STATUS_BACKLOG_WAIT_TIME != 0 {
        if body.len() < 44 || wait_time > 10 * BACKLOG_WAIT_TIME {
            return Err(EINVAL);
        }
        if locked {
            return Err(EPERM);
        }
        s.backlog_wait_time = wait_time;
    }
    if mask == STATUS_LOST || mask == STATUS_BACKLOG_WAIT_TIME_ACTUAL {
        // Reading and resetting a counter that never moves here.
        s.lost = 0;
    }
    Ok(())
}

/// A userspace record (`audit_log_common_recv_msg`): dropped while audit
/// is off, except AUDIT_USER_AVC.
fn user_record(s: &mut State, ty: u16, body: &[u8], pid: u32) -> Result<(), Errno> {
    if s.enabled == 0 && ty != AUDIT_USER_AVC {
        return Ok(());
    }
    if ty == AUDIT_USER_TTY {
        return Err(EOPNOTSUPP);
    }
    s.serial = s.serial.wrapping_add(1);
    let me = super::cred::peer(pid as i32);
    // The text is NUL-terminated at its last byte, as the kernel ends it.
    let text = body
        .split_last()
        .map_or(&b""[..], |(_, t)| t)
        .split(|&c| c == 0)
        .next()
        .unwrap_or_default();
    let text = &text[..text.len().min(MESSAGE_TEXT_MAX)];
    let now = super::clock::realtime_ns() as u64;
    let record = format!(
        "audit({}.{:03}:{}): pid={pid} uid={} auid=4294967295 ses=4294967295 subj={} msg='{}'",
        now / 1_000_000_000,
        now / 1_000_000 % 1000,
        s.serial,
        me.uid,
        super::cred::seclabel(),
        String::from_utf8_lossy(text)
    );
    let mut msg = record.clone().into_bytes();
    msg.push(0);
    let delivered =
        s.portid != 0 && netlink::unicast(NETLINK_AUDIT, s.portid, &nlmsg(ty, 0, 0, 0, &msg));
    netlink::multicast(NETLINK_AUDIT, GRP_READLOG, &nlmsg(ty, 0, 0, 0, &msg));
    if !delivered {
        // `kauditd_printk_skb`: KERN_NOTICE, "audit: " (pr_fmt).
        super::kmsg::printk(0, 5, format!("audit: type={ty} {record}").as_bytes());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_round_trips() {
        let mut s = State::default();
        s.pid = 42;
        s.serial = 7;
        assert_eq!(State::decode(&s.encode()), Some(s));
        assert_eq!(State::decode("1 2"), None);
        assert_eq!(s.status().len(), 44);
    }

    fn status(mask: u32, values: &[(usize, u32)]) -> Vec<u8> {
        let mut v = [0u32; 11];
        v[0] = mask;
        for &(i, x) in values {
            v[i] = x;
        }
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    #[test]
    fn auditd_registration() {
        let mut s = State::default();
        assert_eq!(set(&mut s, &status(STATUS_PID, &[(3, 10)]), 10, 99), Ok(()));
        assert_eq!((s.pid, s.portid), (10, 99));
        // Another process may not replace or unregister it.
        assert_eq!(
            set(&mut s, &status(STATUS_PID, &[(3, 11)]), 11, 98),
            Err(EEXIST)
        );
        assert_eq!(set(&mut s, &status(STATUS_PID, &[]), 11, 98), Err(EACCES));
        assert_eq!(set(&mut s, &status(STATUS_PID, &[]), 10, 99), Ok(()));
        assert_eq!((s.pid, s.portid), (0, 0));
    }

    #[test]
    fn limits_and_the_lock() {
        let mut s = State::default();
        assert_eq!(
            set(&mut s, &status(STATUS_RATE_LIMIT, &[(4, 5)]), 1, 1),
            Ok(())
        );
        assert_eq!(s.rate_limit, 5);
        assert_eq!(
            set(&mut s, &status(STATUS_ENABLED, &[(1, 3)]), 1, 1),
            Err(EINVAL)
        );
        assert_eq!(
            set(&mut s, &status(STATUS_FAILURE, &[(2, 3)]), 1, 1),
            Err(EINVAL)
        );
        let long = BACKLOG_WAIT_TIME * 11;
        assert_eq!(
            set(
                &mut s,
                &status(STATUS_BACKLOG_WAIT_TIME, &[(9, long)]),
                1,
                1
            ),
            Err(EINVAL)
        );
        assert_eq!(
            set(&mut s, &status(STATUS_ENABLED, &[(1, 2)]), 1, 1),
            Ok(())
        );
        assert_eq!(
            set(&mut s, &status(STATUS_RATE_LIMIT, &[(4, 1)]), 1, 1),
            Err(EPERM)
        );
        assert_eq!(
            set(&mut s, &status(STATUS_ENABLED, &[(1, 0)]), 1, 1),
            Err(EPERM)
        );
    }

    #[test]
    fn obsolete_and_unknown_types() {
        assert_eq!(permitted(AUDIT_LIST), Err(EOPNOTSUPP));
        assert_eq!(permitted(999), Err(EINVAL));
        assert_eq!(permitted(1300), Err(EINVAL));
    }
}
