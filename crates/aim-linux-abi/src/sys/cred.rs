//! Process identity: user and group ids, supplementary groups,
//! capabilities, securebits and the attributes init sets before `execv`
//! (seclabel, priority, oom_score_adj, rlimits), with Linux semantics.
//!
//! Darwin runs every guest process as the host user, so the guest's
//! credentials are a model of their own. A process starts from the
//! identity file aim-guest-init wrote for it (`linux-run --identity`,
//! `docs/guest-init-contract.md` section 4), or as root with every
//! capability when started without one. The set*id calls, `setgroups`,
//! `capset` and the capability prctls change it under the kernel's rules
//! (`kernel/sys.c`, `security/commoncap.c`); the file itself never changes.
//!
//! - fork: the child inherits the state (memory is copied) and writes it to
//!   `by-pid/<pid>` next to the identity file, the process table peers read
//!   for `SO_PEERCRED` ([`peer`]). The process keeps its entry open and
//!   every later change rewrites it in place under an exclusive `flock`,
//!   which readers take shared ([`read_entry`]): a change costs a few
//!   microseconds, where replacing the file costs a create and a rename.
//!   The entry is removed when the process exits or is reaped.
//! - exec: the state after the exec capability transform travels on
//!   `linux-run`'s command line (`--identity-text`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::errno::{EACCES, EFAULT, EINVAL, EPERM, ESRCH};

/// Highest capability of the kernel we present (CAP_CHECKPOINT_RESTORE).
pub const CAP_LAST_CAP: u32 = 40;
const CAP_FULL: u64 = (1 << (CAP_LAST_CAP + 1)) - 1;
const CAP_KILL: u32 = 5;
const CAP_SETGID: u32 = 6;
const CAP_SETUID: u32 = 7;
const CAP_SETPCAP: u32 = 8;
const CAP_SYS_PTRACE: u32 = 19;
pub(super) const CAP_SYS_ADMIN: u32 = 21;
const CAP_SYS_NICE: u32 = 23;
const RLIMIT_NICE: usize = 13;
pub(super) const CAP_DAC_OVERRIDE: u32 = 1;
const CAP_SYS_RESOURCE: u32 = 24;
/// Capabilities that follow the fsuid (CAP_FS_MASK).
const CAP_FS_MASK: u64 =
    (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3) | (1 << 4) | (1 << 9) | (1 << 27) | (1 << 32);

const SECBIT_NOROOT: u32 = 1 << 0;
const SECBIT_NO_SETUID_FIXUP: u32 = 1 << 2;
const SECBIT_KEEP_CAPS: u32 = 1 << 4;
/// The three flags and their `*_LOCKED` companions.
const SECBITS_MASK: u32 = 0x3f;

const NGROUPS_MAX: usize = 65536;
/// Linux RLIM_NLIMITS.
const RLIM_NLIMITS: usize = 16;
/// `RLIMIT_DATA`, `RLIMIT_AS`.
const RLIMIT_DATA: usize = 2;
const RLIMIT_AS: usize = 9;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// The service (or `exec` name) it was started for.
    pub service: String,
    /// Real, effective, saved and filesystem uid.
    pub uid: [u32; 4],
    /// Real, effective, saved and filesystem gid.
    pub gid: [u32; 4],
    /// Supplementary groups in `setgroups` order.
    pub groups: Vec<u32>,
    pub cap_eff: u64,
    pub cap_perm: u64,
    pub cap_inh: u64,
    pub cap_bset: u64,
    pub cap_amb: u64,
    pub securebits: u32,
    /// SELinux label; empty when init would take it from the file context.
    pub seclabel: String,
    /// Nice value.
    pub priority: i32,
    pub oom_score_adj: i32,
    /// Limits set by init or the guest, per Linux resource (soft, hard);
    /// None: the host's.
    pub rlimits: [Option<(u64, u64)>; RLIM_NLIMITS],
}

impl Default for Identity {
    /// A process started as root: every capability.
    fn default() -> Identity {
        Identity {
            service: String::new(),
            uid: [0; 4],
            gid: [0; 4],
            groups: Vec::new(),
            cap_eff: CAP_FULL,
            cap_perm: CAP_FULL,
            cap_inh: 0,
            cap_bset: CAP_FULL,
            cap_amb: 0,
            securebits: 0,
            seclabel: String::new(),
            priority: 0,
            oom_score_adj: 0,
            rlimits: [None; RLIM_NLIMITS],
        }
    }
}

fn num(v: &str) -> Result<u64, String> {
    match v.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => v.parse(),
    }
    .map_err(|e| format!("'{v}': {e}"))
}

fn limit(v: &str) -> Result<u64, String> {
    if v == "unlimited" {
        Ok(u64::MAX)
    } else {
        num(v)
    }
}

fn limit_text(v: u64) -> String {
    if v == u64::MAX {
        "unlimited".into()
    } else {
        v.to_string()
    }
}

impl Identity {
    /// Parse the identity file format (`# aim-guest-init identity v1`).
    /// Besides the contract's keys, `euid`, `egid`, `suid`, `sgid` and
    /// `securebits` carry state a process can reach on its own; unknown
    /// keys are ignored.
    pub fn parse(text: &str) -> Result<Identity, String> {
        let mut id = Identity::default();
        let (mut euid, mut egid, mut suid, mut sgid) = (None, None, None, None);
        let mut caps_given = false;
        for line in text.lines() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line.split_once('\t').ok_or(format!("bad line '{line}'"))?;
            match key {
                "service" => id.service = value.into(),
                "uid" => id.uid = [num(value)? as u32; 4],
                "gid" => id.gid = [num(value)? as u32; 4],
                "euid" => euid = Some(num(value)? as u32),
                "egid" => egid = Some(num(value)? as u32),
                "suid" => suid = Some(num(value)? as u32),
                "sgid" => sgid = Some(num(value)? as u32),
                "groups" => {
                    id.groups = value
                        .split(' ')
                        .filter(|s| !s.is_empty())
                        .map(|g| num(g).map(|g| g as u32))
                        .collect::<Result<_, _>>()?
                }
                "cap_effective" | "cap_permitted" | "cap_inheritable" | "cap_ambient"
                | "cap_bounding" => {
                    if !caps_given {
                        (id.cap_eff, id.cap_perm, id.cap_inh, id.cap_amb) = (0, 0, 0, 0);
                        caps_given = true;
                    }
                    let v = num(value)? & CAP_FULL;
                    match key {
                        "cap_effective" => id.cap_eff = v,
                        "cap_permitted" => id.cap_perm = v,
                        "cap_inheritable" => id.cap_inh = v,
                        "cap_ambient" => id.cap_amb = v,
                        _ => id.cap_bset = v,
                    }
                }
                "securebits" => id.securebits = num(value)? as u32 & SECBITS_MASK,
                "seclabel" => id.seclabel = value.into(),
                "priority" => id.priority = value.parse().map_err(|e| format!("{e}"))?,
                "oom_score_adj" => id.oom_score_adj = value.parse().map_err(|e| format!("{e}"))?,
                "rlimit" => {
                    let f: Vec<&str> = value.split('\t').collect();
                    let [res, soft, hard] = f[..] else {
                        return Err(format!("bad rlimit '{value}'"));
                    };
                    let res = num(res)? as usize;
                    if res < RLIM_NLIMITS {
                        id.rlimits[res] = Some((limit(soft)?, limit(hard)?));
                    }
                }
                _ => {}
            }
        }
        if let Some(e) = euid {
            id.uid[1..].fill(e);
        }
        if let Some(e) = egid {
            id.gid[1..].fill(e);
        }
        if let Some(s) = suid {
            id.uid[2] = s;
        }
        if let Some(s) = sgid {
            id.gid[2] = s;
        }
        if !caps_given && id.uid[1] != 0 {
            // init leaves a non-root service without a `capabilities` line
            // with no capability.
            (id.cap_eff, id.cap_perm) = (0, 0);
        }
        Ok(id)
    }

    /// The identity file text. Filesystem ids are not kept: they follow
    /// the effective ones, as saved ids do after exec.
    pub fn to_text(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::from("# aim-guest-init identity v1\n");
        let _ = writeln!(out, "service\t{}", self.service);
        let _ = writeln!(out, "uid\t{}", self.uid[0]);
        let _ = writeln!(out, "gid\t{}", self.gid[0]);
        if self.uid[1] != self.uid[0] {
            let _ = writeln!(out, "euid\t{}", self.uid[1]);
        }
        if self.gid[1] != self.gid[0] {
            let _ = writeln!(out, "egid\t{}", self.gid[1]);
        }
        if self.uid[2] != self.uid[1] {
            let _ = writeln!(out, "suid\t{}", self.uid[2]);
        }
        if self.gid[2] != self.gid[1] {
            let _ = writeln!(out, "sgid\t{}", self.gid[2]);
        }
        let groups: Vec<String> = self.groups.iter().map(u32::to_string).collect();
        let _ = writeln!(out, "groups\t{}", groups.join(" "));
        let _ = writeln!(out, "cap_effective\t{:#x}", self.cap_eff);
        let _ = writeln!(out, "cap_permitted\t{:#x}", self.cap_perm);
        let _ = writeln!(out, "cap_inheritable\t{:#x}", self.cap_inh);
        let _ = writeln!(out, "cap_ambient\t{:#x}", self.cap_amb);
        let _ = writeln!(out, "cap_bounding\t{:#x}", self.cap_bset);
        if self.securebits != 0 {
            let _ = writeln!(out, "securebits\t{:#x}", self.securebits);
        }
        let _ = writeln!(out, "seclabel\t{}", self.seclabel);
        let _ = writeln!(out, "priority\t{}", self.priority);
        let _ = writeln!(out, "oom_score_adj\t{}", self.oom_score_adj);
        for (res, l) in self.rlimits.iter().enumerate() {
            if let Some((soft, hard)) = l {
                let _ = writeln!(
                    out,
                    "rlimit\t{res}\t{}\t{}",
                    limit_text(*soft),
                    limit_text(*hard)
                );
            }
        }
        out
    }

    fn capable(&self, cap: u32) -> bool {
        self.cap_eff & (1 << cap) != 0
    }

    /// Credentials after execve of a file with no capabilities and no
    /// set-id bits (`cap_bprm_creds_from_file`).
    pub fn exec_transform(&mut self) {
        self.uid[2] = self.uid[1];
        self.uid[3] = self.uid[1];
        self.gid[2] = self.gid[1];
        self.gid[3] = self.gid[1];
        let root = self.securebits & SECBIT_NOROOT == 0 && (self.uid[0] == 0 || self.uid[1] == 0);
        self.cap_amb &= self.cap_perm & self.cap_inh;
        if root {
            self.cap_perm = (self.cap_inh | self.cap_bset | self.cap_amb) & CAP_FULL;
            self.cap_eff = if self.uid[1] == 0 {
                self.cap_perm
            } else {
                self.cap_amb
            };
        } else {
            self.cap_perm = self.cap_amb;
            self.cap_eff = self.cap_amb;
        }
        self.securebits &= !SECBIT_KEEP_CAPS;
    }

    /// `cap_emulate_setxuid`: capability fixups after the uids changed.
    fn fix_setuid(&mut self, old: [u32; 4]) {
        if self.securebits & SECBIT_NO_SETUID_FIXUP != 0 {
            return;
        }
        if (old[0] == 0 || old[1] == 0 || old[2] == 0)
            && self.uid[0] != 0
            && self.uid[1] != 0
            && self.uid[2] != 0
        {
            if self.securebits & SECBIT_KEEP_CAPS == 0 {
                self.cap_perm = 0;
                self.cap_eff = 0;
            }
            self.cap_amb = 0;
        }
        if old[1] == 0 && self.uid[1] != 0 {
            self.cap_eff = 0;
        }
        if old[1] != 0 && self.uid[1] == 0 {
            self.cap_eff = self.cap_perm;
        }
    }

    fn fix_setfsuid(&mut self, old_fs: u32) {
        if self.securebits & SECBIT_NO_SETUID_FIXUP != 0 {
            return;
        }
        if old_fs == 0 && self.uid[3] != 0 {
            self.cap_eff &= !CAP_FS_MASK;
        }
        if old_fs != 0 && self.uid[3] == 0 {
            self.cap_eff |= self.cap_perm & CAP_FS_MASK;
        }
    }
}

static STATE: Mutex<Option<Identity>> = Mutex::new(None);
/// The `by-pid` directory: the identity of each process, by host pid.
static BY_PID: OnceLock<PathBuf> = OnceLock::new();

/// uid, euid, gid and egid, read by the lean syscall path in `trampoline.S`.
pub static IDS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn publish(id: &Identity) {
    for (slot, v) in IDS.iter().zip([id.uid[0], id.uid[1], id.gid[0], id.gid[1]]) {
        slot.store(v as u64, Ordering::Relaxed);
    }
}

/// This process's `by-pid` entry, open for writing in place, and the
/// length written so far (a shorter text is padded to it with empty
/// lines, which readers skip).
static ENTRY: Mutex<Option<(aim_storage::private_fd::PrivateFile, usize)>> = Mutex::new(None);

/// Write `id` as the `by-pid` entry of `pid`, replacing (never modifying)
/// what is there. Returns the new file, open for writing.
fn write_entry(
    dir: &std::path::Path,
    pid: i32,
    id: &Identity,
) -> Option<aim_storage::private_fd::PrivateFile> {
    write_entry_checked(dir, pid, id).ok()
}
fn write_entry_checked(
    dir: &std::path::Path,
    pid: i32,
    id: &Identity,
) -> std::io::Result<aim_storage::private_fd::PrivateFile> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let tmp = dir.join(format!(".{pid}.tmp"));
    let mut f = aim_storage::private_fd::PrivateFile::allocate(|| {
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&tmp)
    })?;
    let result = (|| {
        f.write_all(id.to_text().as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, dir.join(pid.to_string()))
    })();
    if let Err(error) = result {
        if let Err(cleanup) = std::fs::remove_file(&tmp) {
            eprintln!("identity publication cleanup: {cleanup}");
        }
        return Err(error);
    }
    Ok(f)
}

/// An exclusive or shared `flock` on `f` for the life of the guard.
struct Flock<'a>(&'a std::fs::File);

impl<'a> Flock<'a> {
    fn checked(f: &'a std::fs::File, op: i32) -> std::io::Result<Self> {
        use std::os::fd::AsRawFd;
        loop {
            if unsafe { libc::flock(f.as_raw_fd(), op) } == 0 {
                return Ok(Self(f));
            }
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
    fn new(f: &'a std::fs::File, op: i32) -> Flock<'a> {
        use std::os::fd::AsRawFd as _;
        // SAFETY: plain flock on an open file.
        while unsafe { libc::flock(f.as_raw_fd(), op) } < 0 {
            if crate::errno::last() != crate::errno::EINTR {
                break;
            }
        }
        Flock(f)
    }
}

impl Drop for Flock<'_> {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd as _;
        // SAFETY: as in `new`.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

/// This process's entry, opened for writing in place: the one its parent
/// wrote before the fork handed it its state, or one it had before an
/// exec; a new one when there is none, or only guest-init's link to the
/// identity file (which never changes).
fn open_entry(
    dir: &std::path::Path,
    pid: i32,
    id: &Identity,
) -> Option<(aim_storage::private_fd::PrivateFile, usize)> {
    open_entry_checked(dir, pid, id).ok()
}
fn open_entry_checked(
    dir: &std::path::Path,
    pid: i32,
    id: &Identity,
) -> std::io::Result<(aim_storage::private_fd::PrivateFile, usize)> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt as _;
    let f = match aim_storage::private_fd::PrivateFile::allocate(|| {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(dir.join(pid.to_string()))
    }) {
        Ok(file) => file,
        Err(error) if matches!(error.raw_os_error(), Some(libc::ENOENT) | Some(libc::ELOOP)) => {
            write_entry_checked(dir, pid, id)?
        }
        Err(error) => return Err(error),
    };
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(f.as_raw_fd(), &mut stat) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG || stat.st_size < 0 {
        return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
    }
    Ok((f, stat.st_size as usize))
}

/// Publish `id` as this process's `by-pid` entry, in place.
fn write_own_entry(id: &Identity) {
    let Some(dir) = BY_PID.get() else { return };
    // SAFETY: trivial.
    let pid = unsafe { libc::getpid() };
    let mut g = ENTRY.lock().unwrap();
    if g.is_none() {
        *g = open_entry(dir, pid, id);
    }
    if let Some((f, len)) = g.as_mut() {
        rewrite(f, len, id.to_text());
    }
}

/// Rewrite the entry `f`, `len` bytes long, with `text` in place.
fn rewrite(f: &aim_storage::private_fd::PrivateFile, len: &mut usize, text: String) {
    use std::os::fd::{AsRawFd, FromRawFd};
    let borrowed =
        std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(f.as_raw_fd()) });
    let f = &*borrowed;
    use std::os::unix::fs::FileExt as _;
    let mut text = text.into_bytes();
    if text.len() < *len {
        text.resize(*len, b'\n');
    }
    let _lock = Flock::new(f, libc::LOCK_EX);
    if f.write_all_at(&text, 0).is_ok() {
        *len = text.len();
    }
}

/// Process `pid`'s `by-pid` entry, read under a shared `flock` so that no
/// rewrite in place is seen half done.
fn read_entry(pid: i32) -> Option<String> {
    read_entry_at(&BY_PID.get()?.join(pid.to_string()))
}

fn read_entry_at(path: &std::path::Path) -> Option<String> {
    use std::io::Read as _;
    // The locked file must not reach a fork child (`spawn::own_fds`).
    let _own = super::fork::spawn::own_fds();
    let f = std::fs::File::open(path).ok()?;
    let _lock = Flock::new(&f, libc::LOCK_SH);
    let mut text = String::new();
    (&f).read_to_string(&mut text).ok()?;
    Some(text)
}

/// execve in place: the new program's credentials
/// ([`Identity::exec_transform`]).
pub fn exec() {
    let mut g = STATE.lock().unwrap();
    let id = g.get_or_insert_with(Identity::default);
    id.exec_transform();
    publish(id);
    write_own_entry(id);
}

/// Set this process's identity at start-up. `by_pid` is the process table
/// directory, when the process belongs to one; its entry is written there.
pub fn init(id: Identity, by_pid: Option<PathBuf>) {
    publish(&id);
    // init sets a service's value as root, with CAP_SYS_RESOURCE, which
    // makes it the floor too (`__set_oom_adj`).
    super::procrec::set_oom([id.oom_score_adj, id.oom_score_adj]);
    if let Some(d) = by_pid {
        let _ = BY_PID.set(d);
        write_own_entry(&id);
    }
    *STATE.lock().unwrap() = Some(id);
}

pub fn by_pid_dir() -> Option<&'static PathBuf> {
    BY_PID.get()
}
/// Establish an actual kernel roster for standalone native owners, without
/// declaring namespace init or changing this process's credentials.
pub(crate) fn publish_current_for_owner(
    directory: &std::path::Path,
) -> Result<PathBuf, crate::errno::Errno> {
    use std::{
        io,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::{DirBuilderExt, MetadataExt},
        },
    };
    let io =
        |error: io::Error| crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO));
    if BY_PID.get().is_none() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(io)?;
        let metadata = std::fs::symlink_metadata(directory).map_err(io)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(crate::errno::EPERM);
        }
        let directory = std::fs::canonicalize(directory).map_err(io)?;
        if crate::vfs::guest_path_of_host(&directory).is_some() {
            return Err(crate::errno::EPERM);
        }
        let _ = BY_PID.set(directory);
    }
    let table = BY_PID.get().ok_or(crate::errno::EIO)?;
    let namespace = crate::vfs::mount_namespace_id().ok_or(crate::errno::ENODEV)?;
    let process =
        aim_storage::process_namespace::ProcessIdentity::running(unsafe { libc::getpid() })
            .map_err(io)?;
    aim_storage::process_namespace::register_mount_namespace(table, process, &namespace)
        .map_err(io)?;
    let identity = current();
    let text = identity.to_text();
    let (mut file, mut length) =
        open_entry_checked(table, process.host_pid, &identity).map_err(io)?;
    let borrowed =
        std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(file.as_raw_fd()) });
    let lock = Flock::checked(&borrowed, libc::LOCK_EX).map_err(io)?;
    use std::os::unix::fs::FileExt;
    let mut bytes = text.into_bytes();
    if bytes.len() < length {
        bytes.resize(length, b'\n');
    }
    borrowed.write_all_at(&bytes, 0).map_err(io)?;
    file.sync_all().map_err(io)?;
    length = bytes.len();
    drop(lock);
    let previous = ENTRY.lock().unwrap().replace((file, length));
    drop(previous);
    let actual = aim_storage::posix_broker::read_credentials(table, process).map_err(io)?;
    if actual.effective_uid != identity.uid[1] || actual.effective_capabilities != identity.cap_eff
    {
        return Err(crate::errno::EIO);
    }
    Ok(table.clone())
}

/// The soft RLIMIT_AS and the RLIMIT_DATA pair the layer enforces
/// (`vm_account`); an unrecorded limit is unlimited, as getrlimit
/// reports it.
pub(super) fn memory_limits() -> super::vm_account::Limits {
    read(|id| super::vm_account::Limits {
        address_space: id.rlimits[RLIMIT_AS].map_or(u64::MAX, |l| l.0),
        data: id.rlimits[RLIMIT_DATA].unwrap_or((u64::MAX, u64::MAX)),
    })
}

fn read<R>(f: impl FnOnce(&Identity) -> R) -> R {
    f(STATE.lock().unwrap().get_or_insert_with(Identity::default))
}

/// Run a change; a changed identity is republished.
fn with<R>(f: impl FnOnce(&mut Identity) -> R) -> R {
    let mut g = STATE.lock().unwrap();
    let id = g.get_or_insert_with(Identity::default);
    let before = id.clone();
    let r = f(id);
    if *id != before {
        publish(id);
        write_own_entry(id);
    }
    r
}

/// A copy of this process's identity.
pub fn current() -> Identity {
    read(|id| id.clone())
}

/// A new SELinux context for the process (a write to `attr/current`).
pub fn set_seclabel(label: &str) {
    with(|id| id.seclabel = label.to_string());
}

/// Whether the process has capability `cap` in its effective set.
pub fn capable(cap: u32) -> bool {
    read(|id| id.capable(cap))
}

/// Fork: the identity, the process table, this process's open entry (the
/// child closes the copy it inherits) and the forking thread's nice value
/// (the child's main thread has it).
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    use std::os::fd::AsRawFd as _;
    w.str(&current().to_text());
    w.opt(BY_PID.get(), |w, d| w.path(d));
    let entry = ENTRY.lock().unwrap().as_ref().map(|e| e.0.as_raw_fd());
    w.opt(entry, |w, fd| w.i32(fd));
    let tid = super::thread::gettid() as i32;
    w.opt(Some(thread_nice(tid)), |w, n| w.i32(n));
    // The child has the parent's signal struct.
    let [adj, min] = super::procrec::oom();
    w.i32(adj);
    w.i32(min);
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    let mut id = Identity::parse(&r.str()).unwrap_or_default();
    let by_pid = r.opt(|r| r.path());
    if let Some(fd) = r.opt(|r| r.i32()) {
        // SAFETY: the parent's entry fd, inherited and unused here.
        unsafe { libc::close(fd) };
    }
    if let Some(nice) = r.opt(|r| r.i32()) {
        id.priority = nice;
    }
    let oom = [r.i32(), r.i32()];
    init(id, by_pid);
    super::procrec::set_oom(oom);
}

/// In the parent of a fork: the child's entry, before the child takes its
/// state, so the pid is in the process table as soon as anyone can know
/// it. The child keeps it and writes it again when it starts.
pub(super) fn note_child(pid: i32) {
    if let Some(dir) = BY_PID.get() {
        write_entry(dir, pid, &current());
    }
}

/// The process `pid` is gone (exited or reaped): drop its entry and
/// record.
pub(super) fn forget(pid: i32) {
    if let Some(dir) = BY_PID.get() {
        let _ = std::fs::remove_file(dir.join(pid.to_string()));
    }
    super::procrec::forget(pid);
}

/// Credentials of a peer process, as `SO_PEERCRED` and `SCM_CREDENTIALS`
/// report them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerCred {
    pub pid: i32,
    pub uid: u32,
    pub gid: u32,
}

/// The identity of host process `pid`: this process's, or its entry in the
/// process table.
pub(super) fn identity_of(pid: i32) -> Option<Identity> {
    // SAFETY: trivial.
    if pid == unsafe { libc::getpid() } {
        return Some(current());
    }
    read_entry(pid).and_then(|t| Identity::parse(&t).ok())
}

/// The effective ids of host process `pid` from the process table. A pid
/// with no entry is reported as root.
pub fn peer(pid: i32) -> PeerCred {
    let id = identity_of(pid);
    let (uid, gid) = id.map_or((0, 0), |id| (id.uid[1], id.gid[1]));
    PeerCred { pid, uid, gid }
}

/// The real uid of host process `pid` from the process table, as a signal
/// it sends carries it (`si_uid`).
pub fn real_uid_of(pid: i32) -> Option<u32> {
    identity_of(pid).map(|id| id.uid[0])
}

/// The credentials of process `pid` of the namespace, for a permission
/// check: a pid with no entry is root.
fn target(pid: i32) -> Identity {
    identity_of(pid).unwrap_or_default()
}

/// `kill_ok_by_cred` (kernel/signal.c): the caller's real or effective uid
/// is the target's real or saved uid, or it has CAP_KILL.
pub fn may_signal(pid: i32) -> bool {
    let t = target(pid);
    read(|id| {
        id.capable(CAP_KILL)
            || [id.uid[0], id.uid[1]]
                .iter()
                .any(|u| *u == t.uid[0] || *u == t.uid[2])
    })
}

/// `__ptrace_may_access` (kernel/ptrace.c) but for the dumpable rule: the
/// caller's real ids (with `fs`, its filesystem ids) are each of the
/// target's real, effective and saved ids, or it has CAP_SYS_PTRACE.
pub fn may_ptrace(pid: i32, fs: bool) -> bool {
    let t = target(pid);
    read(|id| {
        let (u, g) = if fs {
            (id.uid[3], id.gid[3])
        } else {
            (id.uid[0], id.gid[0])
        };
        id.capable(CAP_SYS_PTRACE)
            || (t.uid[..3].iter().all(|&x| x == u) && t.gid[..3].iter().all(|&x| x == g))
    })
}

/// `__cgroup1_procs_write`: an opener with effective uid `euid` (root, or
/// the target's real or saved uid) may move process `pid`; CAP_SYS_NICE is
/// the caller's to check.
pub fn may_attach_task(pid: i32, euid: u32) -> bool {
    let t = target(pid);
    euid == 0 || euid == t.uid[0] || euid == t.uid[2]
}

/// `set_one_prio_perm` and the scheduler's `check_same_owner`
/// (kernel/sys.c, kernel/sched/syscalls.c): the caller's effective uid is
/// the target's real or effective uid, or it has CAP_SYS_NICE.
pub fn may_renice(pid: i32) -> bool {
    let t = target(pid);
    read(|id| id.capable(CAP_SYS_NICE) || id.uid[1] == t.uid[0] || id.uid[1] == t.uid[1])
}

/// `check_prlimit_permission` (kernel/sys.c): the caller's real uid and
/// gid are each of the target's real, effective and saved ids, or it has
/// CAP_SYS_RESOURCE.
fn may_prlimit(t: &Identity) -> bool {
    read(|id| {
        id.capable(CAP_SYS_RESOURCE)
            || (t.uid[..3].iter().all(|&u| u == id.uid[0])
                && t.gid[..3].iter().all(|&g| g == id.gid[0]))
    })
}

/// The credential lines of `/proc/<pid>/status`: this process's
/// identity, another's from the process table (root without an entry).
pub fn proc_status(pid: i32) -> String {
    let id = identity_of(pid).unwrap_or_default();
    let ids = |v: [u32; 4]| format!("{}\t{}\t{}\t{}", v[0], v[1], v[2], v[3]);
    let groups: String = id.groups.iter().map(|g| format!("{g} ")).collect();
    format!(
        "Uid:\t{}\nGid:\t{}\nGroups:\t{groups}\nCapInh:\t{:016x}\nCapPrm:\t{:016x}\nCapEff:\t{:016x}\nCapBnd:\t{:016x}\nCapAmb:\t{:016x}\n",
        ids(id.uid),
        ids(id.gid),
        id.cap_inh,
        id.cap_perm,
        id.cap_eff,
        id.cap_bset,
        id.cap_amb
    )
}

/// `/proc/self/attr/current`.
pub fn seclabel() -> String {
    // SAFETY: trivial.
    seclabel_of(unsafe { libc::getpid() })
}

/// `/proc/<pid>/attr/current`: another process's from the process table.
pub fn seclabel_of(pid: i32) -> String {
    let l = identity_of(pid).unwrap_or_default().seclabel;
    if l.is_empty() {
        "u:r:init:s0".into()
    } else {
        l
    }
}

/// `generic_permission` for writing a file of process `pid`'s /proc
/// directory (mode 0644, owned by its effective ids): the owner, or
/// CAP_DAC_OVERRIDE.
pub fn may_write_owned_by(pid: i32) -> bool {
    let owner = peer(pid).uid;
    read(|id| id.uid[3] == owner || id.capable(CAP_DAC_OVERRIDE))
}

/// `/proc/<pid>/oom_score_adj`: this process's, another's from its record
/// (none without one).
pub fn oom_score_adj_of(pid: i32) -> Option<i32> {
    // SAFETY: trivial.
    if pid == unsafe { libc::getpid() } {
        return Some(super::procrec::oom()[0]);
    }
    super::procrec::oom_of(pid).map(|v| v[0])
}

/// A write of `text` to `/proc/<pid>/oom_score_adj` (`proc_oom_score_adj_write`
/// and `__set_oom_adj`): a number in [-1000, 1000] (`kstrtoint` of the
/// stripped text, any base); lowering it below the process's floor needs
/// CAP_SYS_RESOURCE, and a writer with CAP_SYS_RESOURCE moves the floor to
/// it.
pub fn write_oom_score_adj(pid: i32, text: &[u8]) -> Result<(), crate::errno::Errno> {
    const OOM_SCORE_ADJ_MAX: i32 = 1000;
    // PROC_NUMBUF: the first 12 bytes count.
    let text = &text[..text.len().min(12)];
    let value = std::str::from_utf8(text)
        .ok()
        .and_then(|t| kstrtoint(t.trim()))
        .filter(|v| (-OOM_SCORE_ADJ_MAX..=OOM_SCORE_ADJ_MAX).contains(v))
        .ok_or(crate::errno::EINVAL)?;
    // SAFETY: trivial.
    let me = pid == unsafe { libc::getpid() };
    let [_, min] = if me {
        super::procrec::oom()
    } else {
        super::procrec::oom_of(pid).ok_or(crate::errno::ESRCH)?
    };
    let resource = capable(CAP_SYS_RESOURCE);
    if value < min && !resource {
        return Err(crate::errno::EACCES);
    }
    let values = [value, if resource { value } else { min }];
    if me {
        super::procrec::set_oom(values);
    } else if !super::procrec::set_oom_of(pid, values) {
        return Err(crate::errno::ESRCH);
    }
    Ok(())
}

/// `kstrtoint(s, 0)`: an optional sign, then decimal, `0x` hex or `0`
/// octal, within i32.
fn kstrtoint(s: &str) -> Option<i32> {
    let (negative, digits) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (radix, digits) = if let Some(hex) = digits
        .strip_prefix("0x")
        .or_else(|| digits.strip_prefix("0X"))
    {
        (16, hex)
    } else if digits.len() > 1 && digits.starts_with('0') {
        (8, &digits[1..])
    } else {
        (10, digits)
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    let magnitude = i64::from_str_radix(digits, radix).ok()?;
    i32::try_from(if negative { -magnitude } else { magnitude }).ok()
}

pub fn getuid(nr: u64) -> i64 {
    read(|id| match nr {
        174 => id.uid[0],
        175 => id.uid[1],
        176 => id.gid[0],
        _ => id.gid[1],
    }) as i64
}

const KEEP: u32 = u32::MAX;

/// setuid (146) / setgid (144).
pub fn setid(nr: u64, a: [u64; 6]) -> i64 {
    let v = a[0] as u32;
    if v == KEEP {
        return -(EINVAL as i64);
    }
    let is_uid = nr == 146;
    with(|id| {
        let privileged = id.capable(if is_uid { CAP_SETUID } else { CAP_SETGID });
        let ids = if is_uid { &mut id.uid } else { &mut id.gid };
        let old = *ids;
        if privileged {
            *ids = [v; 4];
        } else if v == old[0] || v == old[2] {
            ids[1] = v;
            ids[3] = v;
        } else {
            return -(EPERM as i64);
        }
        if is_uid {
            id.fix_setuid(old);
        }
        0
    })
}

/// setreuid (145) / setregid (143).
pub fn setreid(nr: u64, a: [u64; 6]) -> i64 {
    let (r, e) = (a[0] as u32, a[1] as u32);
    let is_uid = nr == 145;
    with(|id| {
        let privileged = id.capable(if is_uid { CAP_SETUID } else { CAP_SETGID });
        let ids = if is_uid { &mut id.uid } else { &mut id.gid };
        let old = *ids;
        if !privileged
            && ((r != KEEP && r != old[0] && r != old[1])
                || (e != KEEP && e != old[0] && e != old[1] && e != old[2]))
        {
            return -(EPERM as i64);
        }
        if r != KEEP {
            ids[0] = r;
        }
        if e != KEEP {
            ids[1] = e;
        }
        if r != KEEP || (e != KEEP && e != old[0]) {
            ids[2] = ids[1];
        }
        ids[3] = ids[1];
        if is_uid {
            id.fix_setuid(old);
        }
        0
    })
}

/// setresuid (147) / setresgid (149).
pub fn setresid(nr: u64, a: [u64; 6]) -> i64 {
    let new = [a[0] as u32, a[1] as u32, a[2] as u32];
    let is_uid = nr == 147;
    with(|id| {
        let privileged = id.capable(if is_uid { CAP_SETUID } else { CAP_SETGID });
        let ids = if is_uid { &mut id.uid } else { &mut id.gid };
        let old = *ids;
        if !privileged && !new.iter().all(|&v| v == KEEP || old[..3].contains(&v)) {
            return -(EPERM as i64);
        }
        for (i, v) in new.into_iter().enumerate() {
            if v != KEEP {
                ids[i] = v;
            }
        }
        ids[3] = ids[1];
        if is_uid {
            id.fix_setuid(old);
        }
        0
    })
}

/// getresuid (148) / getresgid (150).
pub fn getresid(nr: u64, a: [u64; 6]) -> i64 {
    let ids = read(|id| if nr == 148 { id.uid } else { id.gid });
    if a[..3].contains(&0) {
        return -(EFAULT as i64);
    }
    for (i, p) in a[..3].iter().enumerate() {
        // SAFETY: guest uid_t/gid_t pointers.
        user_write!(*p, u32, ids[i]);
    }
    0
}

/// setfsuid (151) / setfsgid (152): returns the previous value, and
/// changes it only when allowed.
pub fn setfsid(nr: u64, a: [u64; 6]) -> i64 {
    let v = a[0] as u32;
    let is_uid = nr == 151;
    with(|id| {
        let privileged = id.capable(if is_uid { CAP_SETUID } else { CAP_SETGID });
        let ids = if is_uid { &mut id.uid } else { &mut id.gid };
        let old = ids[3];
        if v != KEEP && (privileged || ids.contains(&v)) {
            ids[3] = v;
            if is_uid {
                id.fix_setfsuid(old);
            }
        }
        old as i64
    })
}

pub fn getgroups(a: [u64; 6]) -> i64 {
    let (size, list) = (a[0] as i32, a[1]);
    if size < 0 {
        return -(EINVAL as i64);
    }
    read(|id| {
        let n = id.groups.len();
        if size == 0 {
            return n as i64;
        }
        if (size as usize) < n {
            return -(EINVAL as i64);
        }
        // SAFETY: guest gid_t array of `size` entries.
        user_try!(unsafe {
            super::user_memory::copy_bytes(id.groups.as_ptr().cast(), list as *mut u8, n * 4)
        });
        n as i64
    })
}

pub fn setgroups(a: [u64; 6]) -> i64 {
    let (size, list) = (a[0] as i32, a[1]);
    if size < 0 || size as usize > NGROUPS_MAX {
        return -(EINVAL as i64);
    }
    let mut groups = vec![0u32; size as usize];
    if size > 0 {
        // SAFETY: guest gid_t array of `size` entries.
        unsafe {
            user_try!(super::user_memory::copy_bytes(
                list as *const u8,
                groups.as_mut_ptr().cast(),
                groups.len() * 4
            ))
        };
    }
    with(|id| {
        if !id.capable(CAP_SETGID) {
            return -(EPERM as i64);
        }
        id.groups = groups;
        0
    })
}

const CAP_V1: u32 = 0x1998_0330;
const CAP_V2: u32 = 0x2007_1026;
const CAP_V3: u32 = 0x2008_0522;

/// Validate a `cap_user_header_t`; returns the number of 32-bit data sets
/// and the pid.
fn cap_header(hdr: u64) -> Result<(usize, i32), i64> {
    if hdr == 0 {
        return Err(-(EFAULT as i64));
    }
    // SAFETY: guest header { u32 version; i32 pid; }.
    let (version, pid) = { (user_read!(hdr, u32), user_read!(hdr + 4, i32)) };
    let n = match version {
        CAP_V1 => 1,
        CAP_V2 | CAP_V3 => 2,
        _ => {
            // SAFETY: as above.
            user_write!(hdr, u32, CAP_V3);
            return Err(-(EINVAL as i64));
        }
    };
    Ok((n, pid))
}

fn is_self(pid: i32) -> bool {
    pid == 0 || pid as i64 == super::process::getpid()
}

pub fn capget(a: [u64; 6]) -> i64 {
    let (n, pid) = match cap_header(a[0]) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if a[1] == 0 {
        return 0;
    }
    if pid < 0 {
        return -(EINVAL as i64);
    }
    let (eff, perm, inh) = if is_self(pid) {
        read(|id| (id.cap_eff, id.cap_perm, id.cap_inh))
    } else {
        let host = match super::pidns::syscall_pid(pid) {
            Ok(host) => host,
            Err(error) => return error,
        };
        if let Err(e) = super::pidns::check(host) {
            return e;
        }
        let entry = identity_of(host).unwrap_or_default();
        (entry.cap_eff, entry.cap_perm, entry.cap_inh)
    };
    for i in 0..n {
        let set = [
            (eff >> (32 * i)) as u32,
            (perm >> (32 * i)) as u32,
            (inh >> (32 * i)) as u32,
        ];
        // SAFETY: guest array of `n` cap_user_data_t.
        user_write!(a[1] + 12 * i as u64, [u32; 3], set);
    }
    0
}

pub fn capset(a: [u64; 6]) -> i64 {
    let (n, pid) = match cap_header(a[0]) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if !is_self(pid) {
        return -(EPERM as i64);
    }
    if a[1] == 0 {
        return -(EFAULT as i64);
    }
    let (mut eff, mut perm, mut inh) = (0u64, 0u64, 0u64);
    for i in 0..n {
        // SAFETY: guest array of `n` cap_user_data_t.
        let [e, p, h] = user_read!(a[1] + 12 * i as u64, [u32; 3]);
        eff |= (e as u64) << (32 * i);
        perm |= (p as u64) << (32 * i);
        inh |= (h as u64) << (32 * i);
    }
    let (eff, perm, inh) = (eff & CAP_FULL, perm & CAP_FULL, inh & CAP_FULL);
    with(|id| {
        if !id.capable(CAP_SETPCAP) && inh & !(id.cap_inh | id.cap_perm) != 0 {
            return -(EPERM as i64);
        }
        if inh & !(id.cap_inh | id.cap_bset) != 0 || perm & !id.cap_perm != 0 || eff & !perm != 0 {
            return -(EPERM as i64);
        }
        id.cap_eff = eff;
        id.cap_perm = perm;
        id.cap_inh = inh;
        id.cap_amb &= perm & inh;
        0
    })
}

const PR_GET_KEEPCAPS: u64 = 7;
const PR_SET_KEEPCAPS: u64 = 8;
const PR_CAPBSET_READ: u64 = 23;
const PR_CAPBSET_DROP: u64 = 24;
const PR_GET_SECUREBITS: u64 = 27;
const PR_SET_SECUREBITS: u64 = 28;
const PR_CAP_AMBIENT: u64 = 47;
const PR_CAP_AMBIENT_IS_SET: u64 = 1;
const PR_CAP_AMBIENT_RAISE: u64 = 2;
const PR_CAP_AMBIENT_LOWER: u64 = 3;
const PR_CAP_AMBIENT_CLEAR_ALL: u64 = 4;

/// The capability prctl options, or None for any other option.
pub fn prctl(a: [u64; 6]) -> Option<i64> {
    let cap_bit = |c: u64| (c <= CAP_LAST_CAP as u64).then(|| 1u64 << c);
    let r = match a[0] {
        PR_GET_KEEPCAPS => read(|id| (id.securebits & SECBIT_KEEP_CAPS != 0) as i64),
        PR_SET_KEEPCAPS => match a[1] {
            0 | 1 => with(|id| {
                id.securebits &= !SECBIT_KEEP_CAPS;
                id.securebits |= a[1] as u32 * SECBIT_KEEP_CAPS;
                0
            }),
            _ => -(EINVAL as i64),
        },
        PR_CAPBSET_READ => match cap_bit(a[1]) {
            Some(b) => read(|id| (id.cap_bset & b != 0) as i64),
            None => -(EINVAL as i64),
        },
        PR_CAPBSET_DROP => match cap_bit(a[1]) {
            Some(b) => with(|id| {
                if !id.capable(CAP_SETPCAP) {
                    return -(EPERM as i64);
                }
                id.cap_bset &= !b;
                0
            }),
            None => -(EINVAL as i64),
        },
        PR_GET_SECUREBITS => read(|id| id.securebits as i64),
        PR_SET_SECUREBITS => with(|id| {
            let bits = a[1] as u32;
            if bits & !SECBITS_MASK != 0 {
                return -(EINVAL as i64);
            }
            // A locked bit (the one above each flag) freezes its flag.
            let locked = (id.securebits >> 1) & 0x15;
            if (bits ^ id.securebits) & (locked | locked << 1) != 0 || !id.capable(CAP_SETPCAP) {
                return -(EPERM as i64);
            }
            id.securebits = bits;
            0
        }),
        PR_CAP_AMBIENT => match (a[1], cap_bit(a[2])) {
            (PR_CAP_AMBIENT_CLEAR_ALL, _) if a[2] == 0 => with(|id| {
                id.cap_amb = 0;
                0
            }),
            (PR_CAP_AMBIENT_IS_SET, Some(b)) => read(|id| (id.cap_amb & b != 0) as i64),
            (PR_CAP_AMBIENT_LOWER, Some(b)) => with(|id| {
                id.cap_amb &= !b;
                0
            }),
            (PR_CAP_AMBIENT_RAISE, Some(b)) => with(|id| {
                if id.cap_perm & id.cap_inh & b == 0 {
                    return -(EPERM as i64);
                }
                id.cap_amb |= b;
                0
            }),
            _ => -(EINVAL as i64),
        },
        _ => return None,
    };
    Some(r)
}

/// getrlimit (163), setrlimit (164) and prlimit64 (261): limits set by
/// init or the guest are reported as set; the others are the host's
/// (`process::prlimit`), which every guest process inherits.
pub fn prlimit(nr: u64, a: [u64; 6]) -> i64 {
    let (pid, res, new, old) = match nr {
        163 => (0, a[0], 0, a[1]),
        164 => (0, a[0], a[1], 0),
        _ => (a[0] as i32, a[1], a[2], a[3]),
    };
    if !is_self(pid) {
        return prlimit_other(pid, res, new, old);
    }
    if res as usize >= RLIM_NLIMITS {
        return super::process::prlimit(261, [0, res, new, old, 0, 0]);
    }
    // SAFETY: guest struct rlimit64.
    let new = if new != 0 {
        Some(user_read!(new, [u64; 2]))
    } else {
        None
    };
    let may_raise = read(|id| id.capable(CAP_SYS_RESOURCE));
    match own_rlimit(res as u32, new, may_raise) {
        Ok(current) => {
            if old != 0 {
                // SAFETY: guest struct rlimit64.
                user_write!(old, [u64; 2], current);
            }
            0
        }
        Err(e) => e,
    }
}

/// `do_prlimit` on this process, for itself or for another that may
/// (`ptrace`'s agent): limit `res` before `new` is set. Raising the hard
/// limit takes the caller's CAP_SYS_RESOURCE, `may_raise`.
pub fn own_rlimit(res: u32, new: Option<[u64; 2]>, may_raise: bool) -> Result<[u64; 2], i64> {
    let res = res as usize;
    if res >= RLIM_NLIMITS {
        return Err(-(EINVAL as i64));
    }
    let current = match read(|id| id.rlimits[res]) {
        Some(l) => [l.0, l.1],
        None => {
            let mut l = [0u64; 2];
            let r = super::process::prlimit(261, [0, res as u64, 0, l.as_mut_ptr() as u64, 0, 0]);
            if r < 0 {
                return Err(r);
            }
            l
        }
    };
    if let Some([soft, hard]) = new {
        if soft > hard {
            return Err(-(EINVAL as i64));
        }
        if hard > current[1] && !may_raise {
            return Err(-(EPERM as i64));
        }
        with(|id| id.rlimits[res] = Some((soft, hard)));
        // Enforce what the host allows; the guest sees what it set. The
        // layer enforces RLIMIT_AS and RLIMIT_DATA itself (`vm_account`):
        // the host's limits would count the layer's memory too.
        if !matches!(res, RLIMIT_DATA | RLIMIT_AS) {
            let l = [soft, hard];
            super::process::prlimit(261, [0, res as u64, l.as_ptr() as u64, 0, 0, 0]);
        }
    }
    Ok(current)
}

/// prlimit64 on another process of the namespace, with
/// `check_prlimit_permission`: its limits as its table entry records them.
/// Darwin changes a process's limits only from inside it, so a new limit
/// is set by the process itself, through its agent ([`own_rlimit`]).
fn prlimit_other(pid: i32, res: u64, new: u64, old: u64) -> i64 {
    let pid = match super::pidns::syscall_pid(pid) {
        Ok(pid) => pid,
        Err(error) => return error,
    };
    if let Err(e) = super::pidns::check(pid) {
        return e;
    }
    let t = target(pid);
    if !may_prlimit(&t) {
        return -(EPERM as i64);
    }
    if res as usize >= RLIM_NLIMITS {
        return -(EINVAL as i64);
    }
    if new != 0 {
        // SAFETY: guest struct rlimit64.
        let new = user_read!(new, [u64; 2]);
        let may_raise = read(|id| id.capable(CAP_SYS_RESOURCE));
        return match super::ptrace::remote_rlimit(pid, res as u32, new, may_raise) {
            Ok(limit) => {
                if old != 0 {
                    // SAFETY: guest struct rlimit64.
                    user_write!(old, [u64; 2], limit);
                }
                0
            }
            Err(e) => e,
        };
    }
    if old != 0 {
        let limit = match t.rlimits[res as usize] {
            Some(l) => [l.0, l.1],
            None => {
                let mut l = [0u64; 2];
                let r = super::process::prlimit(261, [0, res, 0, l.as_mut_ptr() as u64, 0, 0]);
                if r < 0 {
                    return r;
                }
                l
            }
        };
        // SAFETY: guest struct rlimit64.
        user_write!(old, [u64; 2], limit);
    }
    0
}

const PRIO_PROCESS: u64 = 0;
const PRIO_PGRP: u64 = 1;
const PRIO_USER: u64 = 2;

/// Nice values set on this process's other threads. Linux keeps one per
/// thread (PRIO_PROCESS with a tid); Darwin has no per-thread nice value,
/// so the guest sees what it set.
static THREAD_NICE: Mutex<Vec<(i32, i32)>> = Mutex::new(Vec::new());

/// PRIO_PROCESS names a thread; zero names the calling thread.
fn prio_who(who: u64) -> i32 {
    if who == 0 {
        super::thread::gettid() as i32
    } else {
        who as i32
    }
}

/// The processes a PRIO_PGRP or PRIO_USER call names, in the guest's pid
/// namespace (`pidns`); None leaves a process group to the host, when
/// there is no namespace. Guest uids are known only for this process and
/// the process table.
fn prio_targets(which: u64, who: u64) -> Option<Vec<i32>> {
    if which == PRIO_PGRP {
        // SAFETY: trivial.
        let pgrp = if who == 0 {
            unsafe { libc::getpgrp() }
        } else {
            who as i32
        };
        return super::pidns::group(pgrp).map(priority_threads);
    }
    // SAFETY: trivial.
    let me = unsafe { libc::getpid() };
    let uid = if who == 0 {
        read(|id| id.uid[0])
    } else {
        who as u32
    };
    let members = super::pidns::members().unwrap_or_else(|| vec![me]);
    Some(priority_threads(
        members
            .into_iter()
            .filter(|&p| identity_of(p).is_some_and(|id| id.uid[0] == uid))
            .collect(),
    ))
}

/// Linux's group/user priority calls visit every task, including workers.
fn priority_threads(processes: Vec<i32>) -> Vec<i32> {
    processes
        .into_iter()
        .flat_map(|p| {
            if is_self(p) {
                super::thread::with_table(|t| {
                    if t.is_empty() {
                        vec![p]
                    } else {
                        t.keys().copied().collect()
                    }
                })
            } else {
                super::procrec::threads(p)
                    .map_or_else(|| vec![p], |t| t.into_iter().map(|t| t.tid).collect())
            }
        })
        .collect()
}

/// The nice value of thread `p`, read at its owner.
fn process_nice(p: i32) -> Result<i32, i64> {
    let r = getpriority([PRIO_PROCESS, p as u64, 0, 0, 0, 0]);
    if r < 0 { Err(r) } else { Ok(20 - r as i32) }
}

/// getpriority (141): the kernel's `20 - nice` for this process's (or one
/// of its threads') priority, and the highest of a group's or user's
/// processes. Another process's agent reads its own per-thread state.
pub fn getpriority(a: [u64; 6]) -> i64 {
    let mut a = a;
    if a[0] == PRIO_PROCESS {
        a[1] = match super::pidns::syscall_pid(a[1] as i32) {
            Ok(pid) => pid as u64,
            Err(error) => return error,
        };
    }
    if a[0] > PRIO_USER {
        return -(EINVAL as i64);
    }
    if a[0] != PRIO_PROCESS
        && let Some(targets) = prio_targets(a[0], a[1])
    {
        return targets
            .into_iter()
            .filter_map(|p| process_nice(p).ok())
            .min()
            .map_or(-(ESRCH as i64), |nice| 20 - nice as i64);
    }
    if a[0] == PRIO_PROCESS {
        let tid = prio_who(a[1]);
        let owner = super::thread::owner(tid);
        if is_self(owner) {
            return own_priority(tid, None, &current());
        }
        if let Err(e) = super::pidns::check(owner) {
            return e;
        }
        if tid != owner {
            return super::ptrace::remote_priority(owner, tid, None);
        }
        if let Some(id) = identity_of(owner) {
            return 20 - id.priority as i64;
        }
        // A native namespace owner has no guest nice state or agent;
        // Darwin owns its scheduling, so read that task's actual value.
        a[1] = tid as u32 as u64;
    }
    // SAFETY: errno is cleared first because -1 is a valid priority.
    unsafe {
        *libc::__error() = 0;
        let p = libc::getpriority(a[0] as i32, a[1] as u32);
        if p == -1 && *libc::__error() != 0 {
            return -(crate::errno::last() as i64);
        }
        20 - p as i64
    }
}

/// The nice value of `tid`, a thread of this process.
pub fn thread_nice(tid: i32) -> i32 {
    if is_self(tid) {
        return read(|id| id.priority);
    }
    let nice = THREAD_NICE
        .lock()
        .unwrap()
        .iter()
        .find(|t| t.0 == tid)
        .map(|t| t.1);
    nice.unwrap_or_else(|| read(|id| id.priority))
}

/// The host scheduling of thread `tid` of this process after its nice value
/// changed to `nice`: set now for the calling thread, by `tid` itself at its
/// next syscall otherwise.
fn nice_changed(tid: i32, nice: i32) {
    if let Some(th) = super::thread::current()
        && th.tid == tid
    {
        let policy = th.sched.policy.load(Ordering::SeqCst);
        super::process::apply_host_sched(nice, policy);
    } else if let Some(th) = super::thread::find(tid) {
        th.sched.host_stale.store(true, Ordering::SeqCst);
    }
}

/// Linux `set_one_prio_perm` followed by `can_nice`: ownership is
/// checked even for an unchanged value; RLIMIT_NICE belongs to the target.
fn renice_permission(caller: &Identity, target: &Identity, old: i32, nice: i32) -> Result<(), i64> {
    if !caller.capable(CAP_SYS_NICE)
        && caller.uid[1] != target.uid[0]
        && caller.uid[1] != target.uid[1]
    {
        return Err(-(EPERM as i64));
    }
    let limit = target.rlimits[RLIMIT_NICE].map_or(u64::MAX, |l| l.0);
    if nice < old && !caller.capable(CAP_SYS_NICE) && (20 - nice) as u64 > limit {
        return Err(-(EACCES as i64));
    }
    Ok(())
}

/// Read or change a thread of this process, including calls from its
/// agent. Success stores the value here and schedules the host update on
/// the target thread; Darwin refuses remote raises of task importance.
pub(super) fn own_priority(tid: i32, new: Option<i32>, caller: &Identity) -> i64 {
    let th = super::thread::find(tid);
    if !is_self(tid) && th.is_none() {
        return -(ESRCH as i64);
    }
    let result = with(|id| {
        let mut table = THREAD_NICE.lock().unwrap();
        let old = if is_self(tid) {
            id.priority
        } else {
            table
                .iter()
                .find(|t| t.0 == tid)
                .map_or(id.priority, |t| t.1)
        };
        let Some(nice) = new else {
            return 20 - old as i64;
        };
        if let Err(e) = renice_permission(caller, id, old, nice) {
            return e;
        }
        if is_self(tid) {
            id.priority = nice;
        } else {
            table.retain(|t| t.0 != tid);
            table.push((tid, nice));
        }
        0
    });
    if result == 0
        && let Some(nice) = new
    {
        nice_changed(tid, nice);
    }
    result
}

/// A clone inherits the creating thread's nice value, independently of
/// later changes to the process leader.
pub(super) fn inherit_nice(tid: i32, nice: i32) {
    let mut table = THREAD_NICE.lock().unwrap();
    table.retain(|t| t.0 != tid);
    table.push((tid, nice));
}

/// A thread's nice state ends with its task, before its tid is reused.
pub(super) fn forget_nice(tid: i32) {
    THREAD_NICE.lock().unwrap().retain(|t| t.0 != tid);
}

/// setpriority (140). A thread's nice value also sets its host scheduling
/// (`process::apply_host_sched`).
pub fn setpriority(a: [u64; 6]) -> i64 {
    let mut a = a;
    if a[0] == PRIO_PROCESS {
        a[1] = match super::pidns::syscall_pid(a[1] as i32) {
            Ok(pid) => pid as u64,
            Err(error) => return error,
        };
    }
    let nice = (a[2] as i32).clamp(-20, 19);
    if a[0] > PRIO_USER {
        return -(EINVAL as i64);
    }
    if a[0] != PRIO_PROCESS
        && let Some(targets) = prio_targets(a[0], a[1])
    {
        // Each process in turn; the first success clears ESRCH and any
        // failure sticks, as in the kernel's `set_one_prio`.
        let mut err = -(ESRCH as i64);
        for p in targets {
            match setpriority([PRIO_PROCESS, p as u64, nice as u64, 0, 0, 0]) {
                0 if err == -(ESRCH as i64) => err = 0,
                0 => {}
                e => err = e,
            }
        }
        return err;
    }
    if a[0] == PRIO_PROCESS {
        let tid = prio_who(a[1]);
        let owner = super::thread::owner(tid);
        if is_self(owner) {
            return own_priority(tid, Some(nice), &current());
        }
        if let Err(e) = super::pidns::check(owner) {
            return e;
        }
        return super::ptrace::remote_priority(owner, tid, Some(nice));
    }
    // SAFETY: a process group outside a guest namespace.
    crate::errno::check(unsafe { libc::setpriority(a[0] as i32, a[1] as u32, nice) } as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renicing_checks_owner_then_target_limit() {
        let caller = Identity {
            uid: [10050; 4],
            cap_eff: 0,
            ..Identity::default()
        };
        let mut target = caller.clone();
        target.rlimits[RLIMIT_NICE] = Some((20, 40));
        assert_eq!(renice_permission(&caller, &target, 10, 0), Ok(()));
        assert_eq!(
            renice_permission(&caller, &target, 10, -1),
            Err(-(EACCES as i64))
        );
        assert_eq!(renice_permission(&caller, &target, -4, -4), Ok(()));
        assert_eq!(renice_permission(&caller, &target, -4, 19), Ok(()));
        target.uid = [10051; 4];
        assert_eq!(
            renice_permission(&caller, &target, 10, 10),
            Err(-(EPERM as i64))
        );
        target.uid[1] = caller.uid[1];
        assert_eq!(renice_permission(&caller, &target, 10, 0), Ok(()));
        target.uid[1] = 10051;
        target.uid[2] = caller.uid[1];
        assert_eq!(
            renice_permission(&caller, &target, 10, 19),
            Err(-(EPERM as i64))
        );
        let privileged = Identity {
            cap_eff: 1 << CAP_SYS_NICE,
            ..caller
        };
        assert_eq!(renice_permission(&privileged, &target, 19, -20), Ok(()));
        target.uid = privileged.uid;
        target.rlimits[RLIMIT_NICE] = Some((40, 40));
        let unprivileged = Identity {
            cap_eff: 0,
            ..privileged
        };
        assert_eq!(renice_permission(&unprivileged, &target, 19, -20), Ok(()));
    }

    const LOGD: &str = "# aim-guest-init identity v1\nservice\tlogd\nuid\t1036\ngid\t1036\n\
        groups\t1000 1032 3009\ncap_effective\t0x440000000\ncap_permitted\t0x440000000\n\
        cap_inheritable\t0x440000000\ncap_ambient\t0x440000000\ncap_bounding\t0x440000000\n\
        seclabel\t\npriority\t10\noom_score_adj\t-1000\nrlimit\t7\t32768\t32768\n\
        rlimit\t13\t40\t40\nrlimit\t7\t1024\tunlimited\nfuture\tkey\n";

    #[test]
    fn identity_files_parse_and_round_trip() {
        let id = Identity::parse(LOGD).unwrap();
        assert_eq!((id.uid, id.gid), ([1036; 4], [1036; 4]));
        assert_eq!(id.groups, [1000, 1032, 3009]);
        assert_eq!((id.cap_eff, id.cap_bset), (0x4_4000_0000, 0x4_4000_0000));
        assert_eq!((id.priority, id.oom_score_adj), (10, -1000));
        // Later lines for a resource win.
        assert_eq!(id.rlimits[7], Some((1024, u64::MAX)));
        assert_eq!(id.rlimits[13], Some((40, 40)));
        assert_eq!(Identity::parse(&id.to_text()).unwrap(), id);

        let mut split = id.clone();
        split.uid = [1000, 0, 0, 0];
        split.securebits = SECBIT_KEEP_CAPS;
        assert_eq!(Identity::parse(&split.to_text()).unwrap(), split);
        // A saved id other than the effective one, as kill(2) checks it.
        split.uid = [10060, 10060, 10050, 10060];
        split.gid = [10060, 0, 10050, 0];
        assert_eq!(Identity::parse(&split.to_text()).unwrap(), split);
    }

    #[test]
    fn capabilities_default_as_init_leaves_them() {
        let root = Identity::parse("uid\t0\ngid\t0\n").unwrap();
        assert_eq!((root.cap_eff, root.cap_perm), (CAP_FULL, CAP_FULL));
        let shell = Identity::parse("uid\t2000\ngid\t2000\n").unwrap();
        assert_eq!(
            (shell.cap_eff, shell.cap_perm, shell.cap_bset),
            (0, 0, CAP_FULL)
        );
    }

    #[test]
    fn dropping_root_clears_capabilities_unless_kept() {
        let mut id = Identity::default();
        let old = id.uid;
        id.uid = [1000; 4];
        id.fix_setuid(old);
        assert_eq!((id.cap_eff, id.cap_perm), (0, 0));

        let mut id = Identity {
            securebits: SECBIT_KEEP_CAPS,
            ..Identity::default()
        };
        let old = id.uid;
        id.uid = [1000; 4];
        id.fix_setuid(old);
        assert_eq!((id.cap_eff, id.cap_perm), (0, CAP_FULL));
    }

    #[test]
    fn exec_keeps_ambient_capabilities_for_non_root() {
        let mut id = Identity::parse(LOGD).unwrap();
        id.exec_transform();
        assert_eq!((id.cap_eff, id.cap_perm), (0x4_4000_0000, 0x4_4000_0000));
        let mut root = Identity {
            cap_bset: 0xff,
            ..Identity::default()
        };
        root.exec_transform();
        assert_eq!((root.cap_eff, root.cap_perm), (0xff, 0xff));
    }

    #[test]
    fn entries_are_rewritten_in_place_never_through_init_links() {
        let dir = std::env::temp_dir().join(format!("aim-cred-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir(&dir).unwrap();
        // guest-init links a service's pid to its identity file, which
        // must never change.
        let file = dir.join("logd.identity");
        std::fs::write(&file, LOGD).unwrap();
        std::os::unix::fs::symlink(&file, dir.join("7")).unwrap();
        let mut id = Identity::parse(LOGD).unwrap();
        let (f, mut len) = open_entry(&dir, 7, &id).unwrap();
        assert!(std::fs::symlink_metadata(dir.join("7")).unwrap().is_file());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), LOGD);

        // A shorter text leaves padding the parser skips, on the same file.
        id.groups = (0..500).collect();
        rewrite(&f, &mut len, id.to_text());
        let long = len;
        id.groups.clear();
        id.uid = [10060; 4];
        rewrite(&f, &mut len, id.to_text());
        assert_eq!(len, long);
        let text = read_entry_at(&dir.join("7")).unwrap();
        assert_eq!(Identity::parse(&text).unwrap(), id);

        // A process that has an entry of its own opens it again.
        let (_, again) = open_entry(&dir, 7, &Identity::default()).unwrap();
        assert_eq!(again, long);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prlimit_answers_efault_for_memory_it_cannot_copy() {
        if super::super::fdtab::isolated_kernel_test(
            "sys::cred::tests::prlimit_answers_efault_for_memory_it_cannot_copy",
        ) {
            return;
        }
        crate::diag::install_signal_handlers();
        const RLIMIT_NPROC: u64 = 6;
        let page = 16384;
        // SAFETY: two fresh anonymous pages, unmapped below.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                2 * page,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        assert_ne!(p, libc::MAP_FAILED);
        let at = p as u64;
        assert_eq!(prlimit(163, [RLIMIT_NPROC, at, 0, 0, 0, 0]), 0);
        // SAFETY: our page.
        let [soft, hard] = unsafe { (p as *const [u64; 2]).read() };
        assert!(soft <= hard);
        // A write that would straddle into an inaccessible page, and a
        // read-only page (Chromium's AssertMemoryIsReadOnly).
        // SAFETY: our second page.
        assert_eq!(
            unsafe { libc::mprotect(p.byte_add(page), page, libc::PROT_NONE) },
            0
        );
        let end = at + page as u64 - 8;
        assert_eq!(
            prlimit(163, [RLIMIT_NPROC, end, 0, 0, 0, 0]),
            -(EFAULT as i64)
        );
        // SAFETY: our page.
        assert_eq!(unsafe { libc::mprotect(p, page, libc::PROT_READ) }, 0);
        assert_eq!(
            prlimit(163, [RLIMIT_NPROC, at, 0, 0, 0, 0]),
            -(EFAULT as i64)
        );
        assert_eq!(
            prlimit(261, [0, RLIMIT_NPROC, 0, at, 0, 0]),
            -(EFAULT as i64)
        );
        // Reading the new limit (the current one) from it is fine; an
        // inaccessible one is not. The pages stay mapped until the end, so
        // no other thread can map memory there meanwhile and have the
        // limit read from it.
        assert_eq!(prlimit(164, [RLIMIT_NPROC, at, 0, 0, 0, 0]), 0);
        // SAFETY: our pages.
        assert_eq!(unsafe { libc::mprotect(p, 2 * page, libc::PROT_NONE) }, 0);
        assert_eq!(
            prlimit(164, [RLIMIT_NPROC, at, 0, 0, 0, 0]),
            -(EFAULT as i64)
        );
        // SAFETY: our pages.
        assert_eq!(unsafe { libc::munmap(p, 2 * page) }, 0);
    }
}
