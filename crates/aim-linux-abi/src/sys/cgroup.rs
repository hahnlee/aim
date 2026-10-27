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
//! Membership has no effect on the host yet (#297): no Darwin policy
//! follows a group's controller values.

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
    controller: Controller,
    root: PathBuf,
    dir: PathBuf,
    file: File,
    name: String,
    euid: u32,
    pos: Mutex<u64>,
}

impl Open {
    fn new(controller: Controller, root: PathBuf, host: &Path, euid: u32) -> Option<Self> {
        let dir = host.parent()?.to_path_buf();
        let name = host.file_name()?.to_string_lossy().into_owned();
        let file = cgroup_v1::file(controller, dir == root, &name)?;
        Some(Open {
            controller,
            root,
            dir,
            file,
            name,
            euid,
            pos: Mutex::new(0),
        })
    }
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
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) };
    let mut text = String::new();
    let _ = file.read_to_string(&mut text);
    Membership::parse(&text)
}

/// Runs `f` on the membership of the hierarchy at `root` under its lock,
/// with exited tasks dropped, and writes back what changed.
fn transact<R>(
    root: &Path,
    f: impl FnOnce(&mut Membership) -> Result<R, Errno>,
) -> Result<R, Errno> {
    let _own = super::fork::spawn::own_fds();
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(cgroup_v1::state_path(root))
        .map_err(|e| crate::errno::from_darwin(e.raw_os_error().unwrap_or(libc::EIO)))?;
    // SAFETY: locking our open file; released when it closes.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    let mut text = String::new();
    let _ = file.read_to_string(&mut text);
    let mut m = Membership::parse(&text);
    prune(&mut m);
    let r = f(&mut m)?;
    let new = m.text();
    if new != text {
        let _ = file.set_len(0);
        let _ = file.rewind();
        let _ = file.write_all(new.as_bytes());
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
        _ => std::fs::read(open.dir.join(&open.name))
            .map_err(|e| crate::errno::from_darwin(e.raw_os_error().unwrap_or(libc::EIO))),
    }
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
    if !super::cred::may_attach_task(owner, open.euid) && !super::cred::capable(CAP_SYS_NICE) {
        return Err(EACCES);
    }
    // cpuset_can_attach_check: a cpuset with no cpus or mems takes no task.
    if open.controller == Controller::Cpuset
        && ["cpus", "mems"]
            .iter()
            .any(|f| std::fs::read_to_string(open.dir.join(f)).is_ok_and(|v| v.trim().is_empty()))
    {
        return Err(ENOSPC);
    }
    let group = cgroup_v1::group_path(&open.dir, &open.root);
    transact(&open.root, |m| {
        if procs {
            m.threads
                .retain(|(t, ..)| super::thread::owner(*t) != owner);
            m.processes.retain(|(p, ..)| *p != owner);
            if group != "/" {
                m.processes.push((owner, start, group));
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
        // cgroup_release_agent_write: the agent runs with every capability.
        File::ReleaseAgent if !super::cred::capable(CAP_SYS_ADMIN) => Err(EPERM),
        file => cgroup_v1::write(
            file,
            &open.dir,
            &open.root,
            input,
            offset,
            matches!(
                file,
                File::Cpus | File::Mems | File::CpuExclusive | File::MemExclusive
            ) && populated(&open.root, &open.dir),
        ),
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
pub(super) fn opened(fd: i32, host: &Path, controller: Controller, root: PathBuf) {
    let euid = super::cred::current().uid[1];
    if let Some(open) = Open::new(controller, root, host, euid) {
        fdtab::insert(fd, Kind::Cgroup(Arc::new(open)));
    }
}

/// `fd`, an inherited or received descriptor, is a control file if its
/// host file is one; the caller's credentials stand for the opener's.
pub(super) fn adopt(fd: i32) -> bool {
    let hierarchies = vfs::cgroup_v1_hierarchies();
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
    let Some((controller, root)) = vfs::cgroup_v1(guest) else {
        return Ok(());
    };
    let parent = host.parent().filter(|_| host != root);
    let made = cgroup_v1::populate(controller, host, parent)
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
    let (_, root) = vfs::cgroup_v1(guest)?;
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
    let from = vfs::cgroup_v1(old_guest);
    let to = vfs::cgroup_v1(new_guest);
    let (_, root) = match (from, to) {
        (None, None) => return None,
        (Some(f), Some(t)) if f == t => f,
        _ => return Some(-(libc::EXDEV as i64)),
    };
    if !old.is_dir() {
        return Some(-(ENOTDIR as i64));
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
pub fn proc_cgroup(pid: i32, tid: i32) -> String {
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
        .and_then(|root| {
            let pid = super::pidns::guest_pid(pid).unwrap_or(pid).to_string();
            v2_group(&root, &root, &pid)
        })
        .unwrap_or_else(|| "/".into());
    out.push_str(&format!("0::{v2}\n"));
    out
}

/// The deepest group of the cgroup v2 tree at `dir` whose `cgroup.procs`
/// lists `pid`: the tree is a plain directory zygote writes them into.
fn v2_group(root: &Path, dir: &Path, pid: &str) -> Option<String> {
    let deeper = cgroup_v1::children(dir)
        .iter()
        .find_map(|child| v2_group(root, child, pid));
    deeper.or_else(|| {
        std::fs::read_to_string(dir.join("cgroup.procs"))
            .ok()
            .filter(|procs| procs.lines().any(|l| l.trim() == pid))
            .map(|_| cgroup_v1::group_path(dir, root))
    })
}

/// A new thread `tid` is in its creator's group (`cgroup_post_fork`).
pub fn thread_created(creator: i32, tid: i32) {
    for (_, root) in vfs::cgroup_v1_hierarchies() {
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
    for (_, root) in vfs::cgroup_v1_hierarchies() {
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
    for (_, root) in vfs::cgroup_v1_hierarchies() {
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
    let groups: Vec<(Controller, String)> = vfs::cgroup_v1_hierarchies()
        .into_iter()
        .map(|(c, root)| (c, group_of(&read_state(&root), tid)))
        .filter(|(_, g)| g != "/")
        .collect();
    w.seq(groups.into_iter(), |w, (c, g)| {
        w.str(c.name());
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
    for (c, root) in vfs::cgroup_v1_hierarchies() {
        let Some((_, group)) = groups.iter().find(|(name, _)| name == c.name()) else {
            continue;
        };
        let _ = transact(&root, |m| {
            m.processes.retain(|(p, ..)| *p != pid);
            m.threads.retain(|(t, ..)| super::thread::owner(*t) != pid);
            m.processes.push((pid, start, group.clone()));
            Ok(())
        });
    }
}

/// Fork of an open control file: what it is, its opener's uid and
/// position.
pub(super) fn save_open(w: &mut super::fork_state::Writer, open: &Open) {
    w.str(open.controller.name());
    w.path(&open.root);
    w.path(&open.dir.join(&open.name));
    w.u32(open.euid);
    w.u64(*open.pos.lock().unwrap());
}

pub(super) fn restore_open(r: &mut super::fork_state::Reader) -> Option<Arc<Open>> {
    let controller = Controller::from_name(&r.str());
    let root = r.path();
    let host = r.path();
    let euid = r.u32();
    let pos = r.u64();
    let open = Open::new(controller?, root, &host, euid)?;
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
