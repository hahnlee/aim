//! SELinux as the device runs it (ADR 0012: permissive), as far as
//! libselinux's `selinux_check_access` needs it:
//!
//! - selinuxfs at `/sys/fs/selinux` (libselinux finds it by its statfs
//!   magic) with `status` (the page `selinux_status_open` maps), `enforce`
//!   = 0 and `deny_unknown` = 0;
//! - `policyvers`: the newest policy format Linux loads, which libvintf
//!   checks against the framework matrix's `kernel-sepolicy-version`
//!   (without it libselinux answers 15, and ActivityManager warns of an
//!   inconsistent build);
//! - `class/<name>/index` and `class/<name>/perms/<perm>` for the classes
//!   of the image's policy (`plat_sepolicy.cil`, numbered as the kernel
//!   numbers the loaded policy), so every class is known;
//! - `access`, the transaction that computes a decision: every permission
//!   is allowed;
//! - `context`, which `security_check_context` writes: every context is
//!   valid;
//! - `/proc/<self>/attr/current`: the process's context (the identity's
//!   seclabel), which a write changes (`setcon`, zygote's
//!   `selinux_android_setcontext`); the other attributes (`exec`,
//!   `fscreate`, `keycreate`, `sockcreate`) keep what was written.

use std::sync::{Mutex, OnceLock};

use super::dir::{self, Entry};
use super::procfs::Node;

pub const SELINUX_MAGIC: u64 = 0xf97c_ff8c;
const MOUNT: &str = "/sys/fs/selinux";
const PAGE: usize = 16384;
/// Where the classes and permissions come from.
const POLICY: &str = "/system/etc/selinux/plat_sepolicy.cil";
/// `POLICYDB_VERSION_MAX` of Linux 6.12 (`POLICYDB_VERSION_COMP_FTRANS`).
const POLICY_VERSION: &[u8] = b"33";
/// `access`'s reply: allowed, decided, auditallow, auditdeny, seqno, flags.
const ALLOW_ALL: &[u8] = b"ffffffff ffffffff 0 ffffffff 0 0";

/// The statfs type of `guest`, if it is on selinuxfs.
pub fn statfs_magic(guest: &str) -> Option<u64> {
    (guest == MOUNT || guest.strip_prefix(MOUNT)?.starts_with('/')).then_some(SELINUX_MAGIC)
}

const ATTRS: [&str; 5] = ["current", "exec", "fscreate", "keycreate", "sockcreate"];

/// Written attributes other than `current`.
static WRITTEN: Mutex<Vec<(&'static str, Vec<u8>)>> = Mutex::new(Vec::new());

/// The attribute `guest` names, if it is this process's (or thread's).
fn own_attr(guest: &str) -> Option<&'static str> {
    let rest = guest.strip_prefix("/proc/")?;
    let (who, name) = rest.split_once("/attr/")?;
    let name = ATTRS.into_iter().find(|a| *a == name)?;
    let pid = super::process::getpid().to_string();
    let who = match who.split_once("/task/") {
        Some((p, t)) if t.bytes().all(|b| b.is_ascii_digit()) => p,
        Some(_) => return None,
        None => who,
    };
    (who == "self" || who == "thread-self" || who == pid).then_some(name)
}

fn attr_value(name: &str) -> Vec<u8> {
    if name == "current" {
        let mut bytes = super::cred::seclabel().into_bytes();
        bytes.push(0);
        return bytes;
    }
    let written = WRITTEN.lock().unwrap();
    written
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

/// A write to an attribute file: a whole context, NUL-terminated or not.
fn write_attr(name: &'static str, bytes: &[u8]) {
    let text = bytes
        .split(|&b| b == 0 || b == b'\n')
        .next()
        .unwrap_or_default();
    if name == "current" {
        if let Ok(label) = std::str::from_utf8(text)
            && !label.is_empty()
        {
            super::cred::set_seclabel(label);
        }
    } else {
        let mut written = WRITTEN.lock().unwrap();
        written.retain(|(n, _)| *n != name);
        written.push((name, text.to_vec()));
    }
}

const O_CLOEXEC: u64 = 0o2000000;
const O_ACCMODE: u64 = 3;

/// `openat` of a file served here rather than as a `/sys` node; None for
/// other paths.
pub fn open(guest: &str, flags: u64) -> Option<i64> {
    let cloexec = flags & O_CLOEXEC != 0;
    if let Some(name) = own_attr(guest) {
        let value = attr_value(name);
        return Some(if flags & O_ACCMODE != 0 {
            super::knob::open(&value, cloexec, move |req| {
                write_attr(name, req);
                Ok(None)
            })
        } else {
            super::procfs::content_fd(&value, cloexec)
        });
    }
    match guest.strip_prefix(MOUNT)? {
        "/status" => {
            // struct selinux_kernel_status: version 1, sequence 0 (stable),
            // enforcing 0, policyload 0, deny_unknown 0.
            let mut page = vec![0u8; PAGE];
            page[0..4].copy_from_slice(&1u32.to_le_bytes());
            Some(super::procfs::content_fd(&page, cloexec))
        }
        "/access" => Some(super::knob::open(b"", cloexec, |_| {
            Ok(Some(ALLOW_ALL.to_vec()))
        })),
        // With no policy loaded every context is valid.
        "/context" => Some(super::knob::open(b"", cloexec, |_| Ok(None))),
        _ => None,
    }
}

/// A class: its name and permissions, in value order (value = index + 1).
struct Class {
    name: String,
    perms: Vec<String>,
}

/// The classes in `classorder` order (index = position + 1), from the
/// policy's `common`, `class`, `classcommon` and `classorder` statements.
fn parse(cil: &str) -> Vec<Class> {
    use std::collections::HashMap;
    let mut commons: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut own: HashMap<&str, Vec<&str>> = HashMap::new();
    let mut inherits: HashMap<&str, &str> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for line in cil.lines() {
        let words = |s: &'static str| {
            let rest = line.strip_prefix(s)?;
            Some(
                rest.split(|c: char| c.is_whitespace() || c == '(' || c == ')')
                    .filter(|w| !w.is_empty())
                    .collect::<Vec<&str>>(),
            )
        };
        if let Some(w) = words("(common ") {
            if let Some((n, p)) = w.split_first() {
                commons.insert(n, p.to_vec());
            }
        } else if let Some(w) = words("(classcommon ") {
            if let [c, common] = w[..] {
                inherits.insert(c, common);
            }
        } else if let Some(w) = words("(classorder ") {
            order.extend(w.into_iter().filter(|&c| c != "unordered"));
        } else if let Some(w) = words("(class ")
            && let Some((n, p)) = w.split_first()
        {
            own.insert(n, p.to_vec());
        }
    }
    order
        .into_iter()
        .map(|name| {
            let common = inherits.get(name).and_then(|c| commons.get(c));
            let perms = common
                .into_iter()
                .flatten()
                .chain(own.get(name).into_iter().flatten())
                .map(|p| p.to_string())
                .collect();
            Class {
                name: name.to_string(),
                perms,
            }
        })
        .collect()
}

fn classes() -> &'static [Class] {
    static CLASSES: OnceLock<Vec<Class>> = OnceLock::new();
    CLASSES.get_or_init(|| {
        let (host, _) = crate::vfs::lookup(POLICY);
        std::fs::read_to_string(host).map_or(Vec::new(), |s| parse(&s))
    })
}

fn names<'a>(list: impl Iterator<Item = &'a str>, ty: u8) -> Vec<Entry> {
    list.map(|n| Entry::new(1, ty, n)).collect()
}

/// The node at `rest`, the path below `/sys/fs/selinux` ("" for the mount).
pub fn node(rest: &str) -> Option<Node> {
    let rest = rest.trim_start_matches('/');
    Some(match rest {
        "" => Node::Dir(
            [
                ("access", dir::DT_REG),
                ("class", dir::DT_DIR),
                ("deny_unknown", dir::DT_REG),
                ("enforce", dir::DT_REG),
                ("policyvers", dir::DT_REG),
                ("status", dir::DT_REG),
            ]
            .iter()
            .map(|(n, t)| Entry::new(1, *t, *n))
            .collect(),
        ),
        "enforce" | "deny_unknown" => Node::File(b"0".to_vec()),
        "policyvers" => Node::File(POLICY_VERSION.to_vec()),
        "status" | "access" => Node::File(Vec::new()),
        "class" => Node::Dir(names(
            classes().iter().map(|c| c.name.as_str()),
            dir::DT_DIR,
        )),
        _ => {
            let c = rest.strip_prefix("class/")?;
            let (name, tail) = c.split_once('/').unwrap_or((c, ""));
            let (i, c) = classes().iter().enumerate().find(|(_, c)| c.name == name)?;
            match tail {
                "" => Node::Dir(vec![
                    Entry::new(1, dir::DT_REG, "index"),
                    Entry::new(1, dir::DT_DIR, "perms"),
                ]),
                "index" => Node::File((i + 1).to_string().into_bytes()),
                "perms" => Node::Dir(names(c.perms.iter().map(|p| p.as_str()), dir::DT_REG)),
                _ => {
                    let p = tail.strip_prefix("perms/")?;
                    let v = c.perms.iter().position(|x| x == p)?;
                    Node::File((v + 1).to_string().into_bytes())
                }
            }
        }
    })
}

/// Fork: the written attributes (a task's `exec`, `fscreate`, ...
/// contexts are inherited).
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    let v = WRITTEN.lock().unwrap();
    w.seq(v.iter(), |w, (name, value)| {
        w.str(name);
        w.bytes(value);
    });
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    let v = r.seq(|r| (r.str(), r.bytes()));
    let mut written = WRITTEN.lock().unwrap();
    for (name, value) in v {
        if let Some(n) = ATTRS.into_iter().find(|a| *a == name) {
            written.push((n, value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// libvintf's runtime check needs `kernel-sepolicy-version` 30 or
    /// newer from the Android 16 framework matrix.
    #[test]
    fn policyvers_is_a_current_kernels() {
        let Some(Node::File(v)) = node("policyvers") else {
            panic!("no policyvers");
        };
        let v: u32 = std::str::from_utf8(&v).unwrap().parse().unwrap();
        assert!(v >= 30, "{v}");
        let Some(Node::Dir(root)) = node("") else {
            panic!("no root");
        };
        assert!(root.iter().any(|e| e.name == b"policyvers"));
    }

    #[test]
    fn classes_are_numbered_as_the_kernel_loads_them() {
        let cil = "(common file (ioctl read write ))\n\
            (class file (execute_no_trans entrypoint ))\n\
            (class service_manager (add find list ))\n\
            (classcommon file file)\n\
            (classorder (security file service_manager ))\n\
            (class security (compute_av ))\n";
        let c = parse(cil);
        let names: Vec<&str> = c.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["security", "file", "service_manager"]);
        assert_eq!(
            c[1].perms,
            ["ioctl", "read", "write", "execute_no_trans", "entrypoint"]
        );
        assert_eq!(c[2].perms, ["add", "find", "list"]);
    }
}
