//! Kernel uevents (lib/kobject_uevent.c): a device's announcements on
//! NETLINK_KOBJECT_UEVENT (`netlink`), and the `uevent` attribute of its
//! sysfs directory. Reading the attribute lists the device's variables;
//! writing an action to it announces the device again with that action, as
//! ueventd's coldboot asks the kernel to (`kobject_synth_uevent`).
//!
//! A message is `ACTION@DEVPATH` and then `ACTION`, `DEVPATH`, `SUBSYSTEM`,
//! the caller's variables, the device's own and `SEQNUM`, each
//! NUL-terminated. The sequence number is shared by the boot's processes.
//!
//! The devices are the ones the layer models: the evdev nodes (`evdev`).

use std::io::{Read, Seek, Write};
use std::os::fd::AsRawFd;

use crate::errno::{EINVAL, Errno};

/// A device as its uevents describe it.
pub struct Device {
    /// Its sysfs directory below `/sys`, e.g.
    /// `/devices/virtual/input/input3/event3`.
    pub devpath: String,
    pub subsystem: &'static str,
    /// What its `uevent` attribute lists (`MAJOR=13`, ...).
    pub env: Vec<String>,
}

impl Device {
    /// The `uevent` attribute's contents.
    pub fn attribute(&self) -> Vec<u8> {
        self.env
            .iter()
            .flat_map(|v| [v.as_bytes(), b"\n"])
            .flatten()
            .copied()
            .collect()
    }
}

/// `kobject_actions`.
const ACTIONS: [&str; 8] = [
    "add", "remove", "change", "move", "online", "offline", "bind", "unbind",
];

/// The device whose `uevent` attribute `guest` is, if the layer models it.
pub fn attribute(guest: &str) -> Option<Device> {
    let devpath = guest.strip_prefix("/sys")?.strip_suffix("/uevent")?;
    super::evdev::uevent_device(devpath)
}

/// Announce `action` for `dev`, with the caller's variables `extra`.
pub fn emit(action: &str, dev: &Device, extra: &[String]) {
    super::netlink::uevent(&message(action, dev, extra, next_seqnum()));
}

fn message(action: &str, dev: &Device, extra: &[String], seqnum: u64) -> Vec<u8> {
    let head = [
        format!("{action}@{}", dev.devpath),
        format!("ACTION={action}"),
        format!("DEVPATH={}", dev.devpath),
        format!("SUBSYSTEM={}", dev.subsystem),
    ];
    let tail = [format!("SEQNUM={seqnum}")];
    let mut m = Vec::new();
    for v in head.iter().chain(extra).chain(&dev.env).chain(&tail) {
        m.extend_from_slice(v.as_bytes());
        m.push(0);
    }
    m
}

/// A write to `dev`'s `uevent` attribute: `ACTION [UUID [KEY=VALUE]...]`.
pub fn synthesize(dev: &Device, req: &[u8]) -> Result<(), Errno> {
    let (action, extra) = parse_synth(req)?;
    emit(action, dev, &extra);
    Ok(())
}

/// The action and variables of a synthetic uevent request: `SYNTH_UUID`
/// (0 when none is given) and a `SYNTH_ARG_` per argument.
fn parse_synth(req: &[u8]) -> Result<(&'static str, Vec<String>), Errno> {
    let req = std::str::from_utf8(req).map_err(|_| EINVAL)?;
    let req = req.strip_suffix(['\n', '\0']).unwrap_or(req);
    let mut words = req.split(' ');
    let first = words.next().unwrap_or("");
    let action = ACTIONS.into_iter().find(|a| *a == first).ok_or(EINVAL)?;
    let Some(uuid) = words.next() else {
        return Ok((action, vec!["SYNTH_UUID=0".into()]));
    };
    let hex = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_hexdigit());
    let parts: Vec<&str> = uuid.split('-').collect();
    let lens = [8, 4, 4, 4, 12];
    if parts.len() != 5 || parts.iter().zip(lens).any(|(p, n)| !hex(p, n)) {
        return Err(EINVAL);
    }
    let mut extra = vec![format!("SYNTH_UUID={uuid}")];
    for arg in words.filter(|w| !w.is_empty()) {
        let (k, _) = arg.split_once('=').ok_or(EINVAL)?;
        if k.is_empty() || !k.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(EINVAL);
        }
        extra.push(format!("SYNTH_ARG_{arg}"));
    }
    Ok((action, extra))
}

/// The next `uevent_seqnum` of the boot.
pub(super) fn next_seqnum() -> u64 {
    // The locked file must not reach a fork child (`spawn::own_fds`).
    let _own = super::fork::spawn::own_fds();
    let path = super::netif::kernel_dir("uevent").join("seqnum");
    let Ok(mut f) = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
    else {
        return 0;
    };
    // SAFETY: locking our own file; closing it unlocks.
    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) };
    let mut s = String::new();
    let _ = f.read_to_string(&mut s);
    let n = s.trim().parse::<u64>().unwrap_or(0) + 1;
    let _ = f
        .rewind()
        .and_then(|()| f.write_all(format!("{n:020}").as_bytes()));
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev() -> Device {
        Device {
            devpath: "/devices/virtual/input/input1/event1".into(),
            subsystem: "input",
            env: vec![
                "MAJOR=13".into(),
                "MINOR=65".into(),
                "DEVNAME=input/event1".into(),
            ],
        }
    }

    #[test]
    fn message_layout() {
        let m = message("add", &dev(), &["SYNTH_UUID=0".into()], 7);
        let want = "add@/devices/virtual/input/input1/event1\0ACTION=add\0\
                    DEVPATH=/devices/virtual/input/input1/event1\0SUBSYSTEM=input\0\
                    SYNTH_UUID=0\0MAJOR=13\0MINOR=65\0DEVNAME=input/event1\0SEQNUM=7\0";
        assert_eq!(m, want.as_bytes());
        assert_eq!(
            dev().attribute(),
            b"MAJOR=13\nMINOR=65\nDEVNAME=input/event1\n"
        );
    }

    #[test]
    fn synthetic_requests() {
        assert_eq!(
            parse_synth(b"add\n").unwrap(),
            ("add", vec!["SYNTH_UUID=0".into()])
        );
        assert_eq!(
            parse_synth(b"change 12345678-9abc-def0-1234-56789abcdef0 A=1 b2=x").unwrap(),
            (
                "change",
                vec![
                    "SYNTH_UUID=12345678-9abc-def0-1234-56789abcdef0".into(),
                    "SYNTH_ARG_A=1".into(),
                    "SYNTH_ARG_b2=x".into()
                ]
            )
        );
        assert_eq!(parse_synth(b"added"), Err(EINVAL));
        assert_eq!(parse_synth(b"add nope"), Err(EINVAL));
        assert_eq!(
            parse_synth(b"add 12345678-9abc-def0-1234-56789abcdef0 -x=1"),
            Err(EINVAL)
        );
    }
}
