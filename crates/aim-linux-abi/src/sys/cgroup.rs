//! The cgroup v1 hierarchies (`aim_storage::cgroup_v1`) for the guest, as
//! `kernel/cgroup/cgroup-v1.c` serves them: a hierarchy is a host directory
//! the path map mounts (`cgroup:<controller>`), whose control files are read
//! and written as the kernel's (`Kind::Cgroup`); a new group gets its files
//! and only an empty one can be removed.
//!
//! Which group each task is in is kernel state shared by the processes of a
//! boot: a text file next to the hierarchy ([`cgroup_v1::state_path`]),
//! read and changed under `flock`, naming the groups of processes that left
//! the root and of threads moved alone. A thread is in its process's group
//! unless it was moved alone; a new thread is in its creator's, a fork
//! child in its forking thread's. Entries name the process by its start
//! time too, so that a reused pid is not taken for one that exited; exited
//! tasks leave when the state is next changed.
//!
//! Controller values constrain Darwin scheduling through `host`; see
//! docs/cgroups.md for the vendor mapping and host limitations.

mod host;
pub(super) use host::qos as clamp_qos;
pub(crate) use host::{Policy, apply as apply_policy};

use std::io::{Read as _, Seek as _, Write as _};
use std::os::fd::AsRawFd as _;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aim_storage::cgroup_v1::{self, Controller, File, Membership, Start};
use aim_storage::process_namespace::ProcessIdentity;

use super::fdtab::{self, Kind};
use crate::errno::{EACCES, EBUSY, EINVAL, ENOSPC, ENOTDIR, EPERM, ESRCH, Errno};
use crate::vfs;

const CAP_SYS_NICE: u32 = 23;
const CAP_SYS_ADMIN: u32 = 21;
/// `cgroup1_rename`: a group moves only within its parent.
const EIO: Errno = 5;

/// An open control file: what it is, and the opener's effective uid, which
/// a task move checks (`of->file->f_cred`).
pub struct Open {
    controller: Option<Controller>,
    root: PathBuf,
    dir: PathBuf,
    file: File,
    name: String,
    opener: super::cred::Identity,
    pos: Mutex<u64>,
}

impl Open {
    fn new(
        controller: Option<Controller>,
        root: PathBuf,
        host: &Path,
        opener: super::cred::Identity,
    ) -> Option<Self> {
        let dir = host.parent()?.to_path_buf();
        let name = host.file_name()?.to_string_lossy().into_owned();
        let file = match controller {
            Some(c) => cgroup_v1::file(c, dir == root, &name),
            None => aim_storage::cgroup_v2::file(dir == root, &name),
        }?;
        Some(Open {
            controller,
            root,
            dir,
            file,
            name,
            opener,
            pos: Mutex::new(0),
        })
    }
}

fn io_error(e: std::io::Error) -> Errno {
    crate::errno::from_darwin(e.raw_os_error().unwrap_or(libc::EIO))
}

fn hierarchies() -> Vec<(Option<Controller>, PathBuf)> {
    let mut all: Vec<_> = vfs::cgroup_v1_hierarchies()
        .into_iter()
        .map(|(c, root)| (Some(c), root))
        .collect();
    if let Some(root) = vfs::kernel_fs_root("cgroup2") {
        all.push((None, root));
    }
    all
}

fn controller_name(c: Option<Controller>) -> &'static str {
    c.map_or("cgroup2", Controller::name)
}

fn me() -> i32 {
    // SAFETY: trivial.
    unsafe { libc::getpid() }
}

/// The calling process's identity in membership entries.
fn own_start() -> Option<Start> {
    ProcessIdentity::running(me())
        .ok()
        .map(|p| (p.start_seconds, p.start_microseconds))
}

fn live(pid: i32, start: Start) -> bool {
    ProcessIdentity {
        host_pid: pid,
        start_seconds: start.0,
        start_microseconds: start.1,
    }
    .is_live()
}

/// The tids of process `pid` (host).
fn tids(pid: i32) -> Vec<i32> {
    super::procfs::tids(pid)
}

/// Whether thread `tid` of process `owner` (started at `start`) exists.
fn thread_live(owner: i32, start: Start, tid: i32) -> bool {
    live(owner, start) && tids(owner).contains(&tid)
}

/// A process's start time, if it is in the guest's pid namespace.
fn start_of(pid: i32) -> Option<Start> {
    if !super::pidns::contains(pid) || !super::pidns::running(pid) {
        return None;
    }
    ProcessIdentity::running(pid)
        .ok()
        .map(|p| (p.start_seconds, p.start_microseconds))
}

/// Drops entries of tasks that exited.
fn prune(m: &mut Membership) {
    m.processes.retain(|(pid, start, _)| live(*pid, *start));
    m.threads
        .retain(|(tid, start, _)| thread_live(super::thread::owner(*tid), *start, *tid));
}

/// The group of thread `tid`: its own, else its process's, else the root.
fn group_of(m: &Membership, tid: i32) -> String {
    let owner = super::thread::owner(tid);
    m.threads
        .iter()
        .find(|(t, ..)| *t == tid)
        .or_else(|| m.processes.iter().find(|(p, ..)| *p == owner))
        .map_or_else(|| "/".to_string(), |(.., g)| g.clone())
}

/// The membership of the hierarchy at `root`, read under a shared lock.
fn snapshot(root: &Path) -> Membership {
    let _own = super::fork::spawn::own_fds();
    read_state(root)
}

/// [`snapshot`] by a caller that keeps forks out already.
fn read_state(root: &Path) -> Membership {
    let Ok(mut file) = std::fs::File::open(cgroup_v1::state_path(root)) else {
        return Membership::default();
    };
    // SAFETY: locking our open file; released when it closes.
    if crate::errno::retry_host(|| unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) }).is_err()
    {
        return Membership::default();
    }
    let mut text = String::new();
    let _ = file.read_to_string(&mut text);
    Membership::parse(&text)
}

fn checked_state(root: &Path) -> Result<Membership, Errno> {
    let mut file = match std::fs::File::open(cgroup_v1::state_path(root)) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Membership::default()),
        Err(e) => return Err(io_error(e)),
    };
    crate::errno::retry_host(|| unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) })?;
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(io_error)?;
    Ok(Membership::parse(&text))
}

/// Runs `f` on the membership of the hierarchy at `root` under its lock,
/// with exited tasks dropped, and writes back what changed.
fn transact<R>(
    root: &Path,
    f: impl FnOnce(&mut Membership) -> Result<R, Errno>,
) -> Result<R, Errno> {
    let _own = super::fork::spawn::own_fds();
    let lock_root = vfs::cgroup_v1_hierarchies()
        .first()
        .map(|(_, p)| p.clone())
        .unwrap_or_else(|| root.to_path_buf());
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_root.with_extension("policy-lock"))
        .map_err(io_error)?;
    crate::errno::retry_host(|| unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) })?;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(cgroup_v1::state_path(root))
        .map_err(|e| crate::errno::from_darwin(e.raw_os_error().unwrap_or(libc::EIO)))?;
    // SAFETY: locking our open file; released when it closes.
    crate::errno::retry_host(|| unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) })?;
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(io_error)?;
    let mut m = Membership::parse(&text);
    prune(&mut m);
    let old = m.clone();
    let r = f(&mut m)?;
    let new = m.text();
    if new != text {
        host::update(root, &old, &m, None)?;
        if let Err(error) = (|| {
            crate::errno::retry_io(|| file.set_len(0))?;
            crate::errno::retry_io(|| file.rewind())?;
            file.write_all(new.as_bytes())
        })() {
            crate::errno::retry_io(|| file.rewind()).map_err(io_error)?;
            file.write_all(text.as_bytes()).map_err(io_error)?;
            crate::errno::retry_io(|| file.set_len(text.len() as u64)).map_err(io_error)?;
            host::update(root, &m, &old, None)?;
            return Err(io_error(error));
        }
    }
    Ok(r)
}

/// The live tasks of the guest, with their process: `(pid, tid)`.
fn all_tasks() -> Vec<(i32, i32)> {
    super::procfs::pids()
        .into_iter()
        .filter_map(|guest| super::pidns::host_pid(guest).ok())
        .flat_map(|pid| tids(pid).into_iter().map(move |tid| (pid, tid)))
        .collect()
}

/// A tid as the guest numbers it (the main thread's is its pid's).
fn guest_tid(pid: i32, tid: i32) -> i32 {
    if tid == pid {
        super::pidns::guest_pid(pid).unwrap_or(tid)
    } else {
        tid
    }
}

/// The tasks in group `group`, as `(guest pid, guest tid)`.
fn members(m: &Membership, group: &str) -> Vec<(i32, i32)> {
    let mut v: Vec<(i32, i32)> = all_tasks()
        .into_iter()
        .filter(|&(_, tid)| group_of(m, tid) == group)
        .map(|(pid, tid)| (guest_tid(pid, pid), guest_tid(pid, tid)))
        .collect();
    v.sort_unstable();
    v
}

/// Whether the group at `dir` has tasks (`cgroup_is_populated`, here
/// counting the group itself).
fn populated(root: &Path, dir: &Path) -> bool {
    !members(&snapshot(root), &cgroup_v1::group_path(dir, root)).is_empty()
}

/// What reading the file shows now.
fn content(open: &Open) -> Result<Vec<u8>, Errno> {
    let group = cgroup_v1::group_path(&open.dir, &open.root);
    match open.file {
        File::Tasks | File::Procs => {
            let tasks = members(&snapshot(&open.root), &group);
            let mut ids: Vec<i32> = tasks
                .into_iter()
                .map(|(pid, tid)| if open.file == File::Procs { pid } else { tid })
                .collect();
            ids.dedup();
            Ok(ids
                .iter()
                .map(|id| format!("{id}\n"))
                .collect::<String>()
                .into_bytes())
        }
        File::ResetStats => Err(EINVAL),
        File::Events => {
            let state = snapshot(&open.root);
            let populated = all_tasks().iter().any(|(_, tid)| {
                let g = group_of(&state, *tid);
                g == group || group == "/" || g.starts_with(&format!("{group}/"))
            });
            Ok(format!("populated {}\n", u8::from(populated)).into_bytes())
        }
        File::Controllers | File::SubtreeControl => Ok(Vec::new()),
        _ => std::fs::read(open.dir.join(&open.name))
            .map_err(|e| crate::errno::from_darwin(e.raw_os_error().unwrap_or(libc::EIO))),
    }
}

fn may_write(id: &super::cred::Identity, stat: &libc::stat) -> bool {
    let shift = if id.uid[3] == stat.st_uid {
        6
    } else if id.gid[3] == stat.st_gid || id.groups.contains(&stat.st_gid) {
        3
    } else {
        0
    };
    (stat.st_mode as u32 >> shift) & 2 != 0 || id.cap_eff & (1 << 1) != 0
}

/// `cgroup_procs_write_start` and `__cgroup1_procs_write`: the task `text`
/// names (0 for the caller) moves to the group, alone or with its thread
/// group.
fn move_task(open: &Open, text: &str) -> Result<(), Errno> {
    let id: i32 = text.trim().parse().map_err(|_| EINVAL)?;
    let procs = open.file == File::Procs;
    let tid = match id {
        0 if procs => me(),
        0 => super::thread::gettid() as i32,
        1 => super::pidns::host_pid(1).map_err(|e| -e as Errno)?,
        id if id < 0 => return Err(ESRCH),
        id => id,
    };
    let owner = super::thread::owner(tid);
    let start = start_of(owner).ok_or(ESRCH)?;
    if !tids(owner).contains(&tid) {
        return Err(ESRCH);
    }
    if open.controller.is_some()
        && !super::cred::may_attach_task(owner, open.opener.uid[1])
        && !super::cred::capable(CAP_SYS_NICE)
    {
        return Err(EACCES);
    }
    // cpuset_can_attach_check: a cpuset with no cpus or mems takes no task.
    if open.controller == Some(Controller::Cpuset)
        && ["cpus", "mems"]
            .iter()
            .any(|f| std::fs::read_to_string(open.dir.join(f)).is_ok_and(|v| v.trim().is_empty()))
    {
        return Err(ENOSPC);
    }
    let group = cgroup_v1::group_path(&open.dir, &open.root);
    // A Linux thread group can span multiple Darwin processes (a clone
    // with its own file table). cgroup.procs migrates every such task.
    let process = if procs {
        super::procrec::tgid(owner)
    } else {
        owner
    };
    let hosts: std::collections::BTreeSet<_> = if procs {
        tids(process)
            .into_iter()
            .map(super::thread::owner)
            .collect()
    } else {
        [owner].into_iter().collect()
    };
    let starts = hosts
        .iter()
        .map(|pid| Ok((*pid, start_of(*pid).ok_or(ESRCH)?)))
        .collect::<Result<Vec<_>, Errno>>()?;
    transact(&open.root, |m| {
        if open.controller.is_none() {
            // cgroup_procs_write_permission uses the opener's credentials
            // at the common ancestor, not the migrating process's uid.
            let mut ancestor = open.root.join(group_of(m, owner).trim_start_matches('/'));
            while !open.dir.starts_with(&ancestor) {
                if !ancestor.pop() {
                    return Err(EINVAL);
                }
            }
            let path = ancestor.join("cgroup.procs");
            let guest = vfs::guest_path_of_host(&path).ok_or(EACCES)?;
            let stat = super::attrs::path_stat(&guest)?;
            if !may_write(&open.opener, &stat) {
                return Err(EACCES);
            }
        }
        if procs {
            m.threads
                .retain(|(t, ..)| !hosts.contains(&super::thread::owner(*t)));
            m.processes.retain(|(p, ..)| !hosts.contains(p));
            if group != "/" {
                for (pid, start) in &starts {
                    m.processes.push((*pid, *start, group.clone()));
                }
            }
        } else {
            m.threads.retain(|(t, ..)| *t != tid);
            let process = m
                .processes
                .iter()
                .find(|(p, ..)| *p == owner)
                .map_or("/", |(.., g)| g.as_str());
            if process != group {
                m.threads.push((tid, start, group));
            }
        }
        Ok(())
    })
}

fn write_at(open: &Open, input: &[u8], offset: u64) -> i64 {
    let result = match open.file {
        File::Tasks | File::Procs => std::str::from_utf8(input)
            .map_err(|_| EINVAL)
            .and_then(|t| move_task(open, t)),
        File::SubtreeControl => {
            let text = std::str::from_utf8(input).map_err(|_| EINVAL);
            text.and_then(|t| {
                for word in t.split_whitespace() {
                    if !word.starts_with(['+', '-']) || word.len() == 1 {
                        return Err(EINVAL);
                    }
                    // cgroup_subtree_control_write: no v2 controller is
                    // available (the resource controllers are bound to v1).
                    return Err(crate::errno::ENOENT);
                }
                Ok(())
            })
        }
        File::Events | File::Controllers => Err(EACCES),
        // cgroup_release_agent_write: the agent runs with every capability.
        File::ReleaseAgent if !super::cred::capable(CAP_SYS_ADMIN) => Err(EPERM),
        file => transact(&open.root, |m| {
            let path = open.dir.join(&open.name);
            let before = std::fs::read(&path).map_err(io_error)?;
            let effective = match file {
                File::Cpus => Some(open.dir.join("effective_cpus")),
                File::Mems => Some(open.dir.join("effective_mems")),
                _ => None,
            };
            let effective_before = effective
                .as_ref()
                .map(std::fs::read)
                .transpose()
                .map_err(io_error)?;
            cgroup_v1::write(
                file,
                &open.dir,
                &open.root,
                input,
                offset,
                matches!(
                    file,
                    File::Cpus | File::Mems | File::CpuExclusive | File::MemExclusive
                ) && !members(m, &cgroup_v1::group_path(&open.dir, &open.root)).is_empty(),
            )?;
            let affects_policy = matches!(
                file,
                File::Cpus
                    | File::Shares
                    | File::Idle
                    | File::UclampMin
                    | File::UclampMax
                    | File::UclampLatencySensitive
                    | File::PrioClass
            );
            if affects_policy && let Err(error) = host::update(&open.root, m, m, Some(&open.dir)) {
                std::fs::write(path, before).map_err(io_error)?;
                if let Some((path, before)) = effective.zip(effective_before) {
                    std::fs::write(path, before).map_err(io_error)?;
                }
                host::update(&open.root, m, m, Some(&open.dir))?;
                return Err(error);
            }
            Ok(())
        }),
    };
    match result {
        Ok(()) => input.len() as i64,
        Err(e) => -(e as i64),
    }
}

fn gather(iov: &[libc::iovec]) -> Result<Vec<u8>, i32> {
    super::user_memory::gather(iov)
}

fn scatter(iov: &[libc::iovec], bytes: &[u8]) -> Result<usize, i32> {
    super::user_memory::scatter(iov, bytes)
}

/// A read at `pos` (the file's position when None).
pub fn read(open: &Open, iov: &[libc::iovec], pos: Option<u64>) -> i64 {
    let bytes = match content(open) {
        Ok(b) => b,
        Err(e) => return -(e as i64),
    };
    let mut cur = open.pos.lock().unwrap();
    let at = pos.unwrap_or(*cur).min(bytes.len() as u64) as usize;
    let n = user_try!(scatter(iov, &bytes[at..]));
    if pos.is_none() {
        *cur = (at + n) as u64;
    }
    n as i64
}

/// A write at `pos` (the file's position when None): each one is a value
/// of its own (`cgroup_file_write`).
pub fn write(open: &Open, iov: &[libc::iovec], pos: Option<u64>) -> i64 {
    let input = user_try!(gather(iov));
    let mut cur = open.pos.lock().unwrap();
    let offset = pos.unwrap_or(*cur);
    let r = write_at(open, &input, offset);
    if r > 0 && pos.is_none() {
        *cur += r as u64;
    }
    r
}

/// `seq_lseek`: SEEK_SET and SEEK_CUR.
pub fn lseek(open: &Open, offset: i64, whence: i32) -> i64 {
    let mut cur = open.pos.lock().unwrap();
    let new = match whence {
        libc::SEEK_SET => offset,
        libc::SEEK_CUR => *cur as i64 + offset,
        _ => return -(EINVAL as i64),
    };
    if new < 0 {
        return -(EINVAL as i64);
    }
    *cur = new as u64;
    new
}

/// Makes `fd`, a file `open` just opened at `host` in the hierarchy of
/// `controller` rooted at `root`, a control file.
pub(super) fn opened(fd: i32, host: &Path, controller: Option<Controller>, root: PathBuf) {
    let opener = super::cred::current();
    if let Some(open) = Open::new(controller, root, host, opener) {
        fdtab::insert(fd, Kind::Cgroup(Arc::new(open)));
    }
}

/// `fd`, an inherited or received descriptor, is a control file if its
/// host file is one; the caller's credentials stand for the opener's.
pub(super) fn adopt(fd: i32) -> bool {
    let hierarchies = hierarchies();
    if hierarchies.is_empty() {
        return false;
    }
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH into a PATH_MAX buffer.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } < 0 {
        return false;
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let host = PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len]));
    let Some((controller, root)) = hierarchies.into_iter().find(|(_, root)| {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        host.starts_with(&root)
    }) else {
        return false;
    };
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    if !host.is_file() {
        return false;
    }
    opened(fd, &host, controller, root);
    matches!(fdtab::get(fd), Some(Kind::Cgroup(_)))
}

/// After a mkdir at `host` (guest `guest`) in the hierarchy: the new
/// group's control files, owned by the creator unless it is root
/// (`cgroup_kn_set_ugid`).
pub(super) fn made_group(host: &Path, guest: &str) -> Result<(), Errno> {
    let Some((controller, root)) = vfs::cgroup(guest) else {
        return Ok(());
    };
    let parent = host.parent().filter(|_| host != root);
    let made = match controller {
        Some(c) => cgroup_v1::populate(c, host, parent),
        None => aim_storage::cgroup_v2::populate(host, host == root),
    }
    .map_err(|e| crate::errno::from_darwin(e.raw_os_error().unwrap_or(libc::EIO)))?;
    let id = super::cred::current();
    let (uid, gid) = (id.uid[3], id.gid[3]);
    for (file, mode) in made {
        let name = file.file_name().unwrap_or_default().to_string_lossy();
        let path = std::ffi::CString::new(file.as_os_str().as_bytes()).map_err(|_| EINVAL)?;
        super::attrs::record(
            super::attrs::Host::Path(&path),
            || format!("{guest}/{name}"),
            super::attrs::Attr {
                uid: Some(uid),
                gid: Some(gid),
                mode: Some(mode),
            },
        );
    }
    Ok(())
}

/// An unlink or rmdir in a hierarchy (`cgroup_rmdir`): a control file
/// cannot go; a group goes with its files unless it is the root or has
/// children or tasks. None outside a hierarchy.
pub(super) fn remove(guest: &str, host: &Path, directory: bool) -> Option<i64> {
    let (_, root) = vfs::cgroup(guest)?;
    if !host.is_dir() {
        return Some(-(if directory { ENOTDIR } else { EPERM } as i64));
    }
    if !directory {
        return Some(-(libc::EISDIR as i64));
    }
    if host == root || !cgroup_v1::children(host).is_empty() || populated(&root, host) {
        return Some(-(EBUSY as i64));
    }
    Some(match std::fs::remove_dir_all(host) {
        Ok(()) => 0,
        Err(e) => -(crate::errno::from_darwin(e.raw_os_error().unwrap_or(libc::EIO)) as i64),
    })
}

/// A rename in a hierarchy (`cgroup1_rename`): a group to a new name in
/// its parent; the tasks in it and below it stay with it. None outside a
/// hierarchy.
pub(super) fn rename(old_guest: &str, old: &Path, new_guest: &str, new: &Path) -> Option<i64> {
    let from = vfs::cgroup(old_guest);
    let to = vfs::cgroup(new_guest);
    let (controller, root) = match (from, to) {
        (None, None) => return None,
        (Some(f), Some(t)) if f == t => f,
        _ => return Some(-(libc::EXDEV as i64)),
    };
    if !old.is_dir() {
        return Some(-(ENOTDIR as i64));
    }
    // cgroup_rename: unified hierarchies cannot be renamed.
    if controller.is_none() {
        return Some(-(EIO as i64));
    }
    if old.parent() != new.parent() {
        return Some(-(EIO as i64));
    }
    if new.exists() {
        return Some(-(libc::EEXIST as i64));
    }
    let (was, now) = (
        cgroup_v1::group_path(old, &root),
        cgroup_v1::group_path(new, &root),
    );
    let result = transact(&root, |m| {
        std::fs::rename(old, new)
            .map_err(|e| crate::errno::from_darwin(e.raw_os_error().unwrap_or(libc::EIO)))?;
        for (.., g) in m.processes.iter_mut().chain(m.threads.iter_mut()) {
            if *g == was {
                *g = now.clone();
            } else if let Some(rest) = g.strip_prefix(&format!("{was}/")) {
                *g = format!("{now}/{rest}");
            }
        }
        Ok(())
    });
    Some(result.map_or_else(|e| -(e as i64), |()| 0))
}

/// `/proc/<pid>/cgroup` of thread `tid` of process `pid` (host): each v1
/// hierarchy, newest first, then the v2 one (`proc_cgroup_show`).
pub fn proc_cgroup(_pid: i32, tid: i32) -> String {
    let mut out = String::new();
    for (controller, root) in vfs::cgroup_v1_hierarchies().into_iter().rev() {
        out.push_str(&format!(
            "{}:{}:{}\n",
            controller.hierarchy_id(),
            controller.name(),
            group_of(&snapshot(&root), tid)
        ));
    }
    let v2 = vfs::kernel_fs_root("cgroup2")
        .map(|root| group_of(&snapshot(&root), tid))
        .unwrap_or_else(|| "/".into());
    out.push_str(&format!("0::{v2}\n"));
    out
}

/// A new thread `tid` is in its creator's group (`cgroup_post_fork`).
pub fn thread_created(creator: i32, tid: i32) {
    for (_, root) in hierarchies() {
        let m = snapshot(&root);
        let Some((.., group)) = m.threads.iter().find(|(t, ..)| *t == creator).cloned() else {
            continue;
        };
        let Some(start) = own_start() else {
            return;
        };
        let _ = transact(&root, |m| {
            m.threads.retain(|(t, ..)| *t != tid);
            m.threads.push((tid, start, group));
            Ok(())
        });
    }
}

/// Thread `tid` exited: its tid may be given to a new thread of this
/// process, which must not inherit its group.
pub fn thread_gone(tid: i32) {
    for (_, root) in hierarchies() {
        if snapshot(&root).threads.iter().any(|(t, ..)| *t == tid) {
            let _ = transact(&root, |m| {
                m.threads.retain(|(t, ..)| *t != tid);
                Ok(())
            });
        }
    }
}

/// execve: the program's other threads are gone, and with them their own
/// groups; the main thread's (its tid is the pid) stays.
pub fn exec_reset() {
    let pid = me();
    let other = |t: i32| t != pid && super::thread::owner(t) == pid;
    for (_, root) in hierarchies() {
        if snapshot(&root).threads.iter().any(|(t, ..)| other(*t)) {
            let _ = transact(&root, |m| {
                m.threads.retain(|(t, ..)| !other(*t));
                Ok(())
            });
        }
    }
}

/// Fork: the child is in the forking thread's groups. The fork keeps
/// other forks out.
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    let tid = super::thread::gettid() as i32;
    let groups: Vec<(Option<Controller>, String)> = hierarchies()
        .into_iter()
        .map(|(c, root)| (c, group_of(&read_state(&root), tid)))
        .filter(|(_, g)| g != "/")
        .collect();
    w.seq(groups.into_iter(), |w, (c, g)| {
        w.str(controller_name(c));
        w.str(&g);
    });
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    let groups = r.seq(|r| (r.str(), r.str()));
    if groups.is_empty() {
        return;
    }
    let Some(start) = own_start() else {
        return;
    };
    let pid = me();
    for (c, root) in hierarchies() {
        let Some((_, group)) = groups.iter().find(|(name, _)| name == controller_name(c)) else {
            continue;
        };
        if let Err(error) = transact(&root, |m| {
            m.processes.retain(|(p, ..)| *p != pid);
            m.threads.retain(|(t, ..)| super::thread::owner(*t) != pid);
            m.processes.push((pid, start, group.clone()));
            Ok(())
        }) {
            r.invalidate();
            eprintln!("cgroup fork restore: errno {error}");
        }
    }
}

/// Rebuild derived host policy after a new host thread/process starts.
/// The membership remains the owner; these cache fields are not fork state.
pub fn refresh_own() -> Result<(), Errno> {
    let Some((_, root)) = vfs::cgroup_v1_hierarchies().into_iter().next() else {
        return Ok(());
    };
    let _own = super::fork::spawn::own_fds();
    let state = checked_state(&root)?;
    host::apply(&host::policies(&root, &state, me())?).map(|_| ())
}

/// Fork of an open control file: what it is, its opener's uid and
/// position.
pub(super) fn save_open(w: &mut super::fork_state::Writer, open: &Open) {
    w.str(controller_name(open.controller));
    w.path(&open.root);
    w.path(&open.dir.join(&open.name));
    w.str(&open.opener.to_text());
    w.u64(*open.pos.lock().unwrap());
}

pub(super) fn restore_open(r: &mut super::fork_state::Reader) -> Option<Arc<Open>> {
    let name = r.str();
    let controller = if name == "cgroup2" {
        None
    } else {
        Some(Controller::from_name(&name)?)
    };
    let root = r.path();
    let host = r.path();
    let opener = super::cred::Identity::parse(&r.str()).ok()?;
    let pos = r.u64();
    let open = Open::new(controller, root, &host, opener)?;
    *open.pos.lock().unwrap() = pos;
    Some(Arc::new(open))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_is_in_its_own_group_else_its_processes() {
        let owner = me();
        let m = Membership {
            processes: vec![(owner, (0, 0), "/background".into())],
            threads: vec![(owner, (0, 0), "/top-app".into())],
        };
        assert_eq!(group_of(&m, owner), "/top-app");
        assert_eq!(group_of(&Membership::default(), owner), "/");
        let only_process = Membership {
            processes: m.processes.clone(),
            threads: Vec::new(),
        };
        assert_eq!(group_of(&only_process, owner), "/background");
    }

    #[test]
    fn exited_tasks_leave_the_membership() {
        let start = own_start().unwrap();
        let mut m = Membership {
            processes: vec![(me(), start, "/a".into()), (me(), (1, 1), "/b".into())],
            threads: vec![(super::super::thread::gettid() as i32, (1, 1), "/c".into())],
        };
        prune(&mut m);
        assert_eq!(m.processes, vec![(me(), start, "/a".to_string())]);
        assert!(m.threads.is_empty());
    }
}
