//! The guest's pid namespace: the processes of its process table, the
//! `by-pid` directory of `cred` (`docs/guest-init-contract.md` section 4).
//!
//! Every call that names another process resolves the pid here, as in a
//! Linux pid namespace (pid_namespaces(7)): a Mac process outside it does
//! not exist for the guest (ESRCH), `kill(-1)` and process groups reach
//! members only, and a session or group led from outside reads as 0.
//!
//! A `linux-run` started without a table (a test, a shell) gets a private
//! one ([`new_table`]): a new namespace of itself and its descendants, whose
//! processes die with it as with the init of a Linux pid namespace
//! ([`leave`]). A shell joins a boot's namespace with `--by-pid`.
//!
//! An entry names a member only if its process started before the entry
//! was written: a pid the Mac reused after a member died without removing
//! its entry names a stranger. The caller's children (all forked by the
//! guest) are members, zombies included, as they are until reaped.

use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use aim_storage::process_namespace::ProcessIdentity;

use crate::errno::ESRCH;

pub(super) mod tids;

fn me() -> i32 {
    // SAFETY: trivial.
    unsafe { libc::getpid() }
}

fn bsd_info(pid: i32) -> Option<libc::proc_bsdinfo> {
    // SAFETY: zeroed plain data, filled by proc_pidinfo up to its size.
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
        let n = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            // Zombies too.
            1,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        );
        (n == size).then_some(info)
    }
}

/// The private tables, `aim-pidns-<pid of its init>` in the host's
/// temporary directory.
const PRIVATE: &str = "aim-pidns-";

/// The table's record of the process groups and sessions that processes
/// brought in from outside, as whitespace-separated ids.
const OUTSIDE: &str = "outside";

/// A private table for a process started without one, with this process
/// as its init. An external watcher owns teardown even after SIGKILL;
/// sweep older tables whose init no longer runs as well.
pub fn new_table() -> Result<PathBuf, String> {
    let tmp = std::env::temp_dir();
    sweep_tables(&tmp).map_err(|e| e.to_string())?;
    let dir = tmp.join(format!("{PRIVATE}{}", me()));
    let init = ProcessIdentity::running(me()).map_err(|e| e.to_string())?;
    start_watcher(init)?;
    Ok(dir)
}

fn sweep_tables(tmp: &Path) -> io::Result<()> {
    for e in std::fs::read_dir(tmp)? {
        let e = e?;
        let name = e.file_name();
        let init = name
            .to_str()
            .and_then(|n| n.strip_prefix(PRIVATE)?.parse().ok());
        if init.is_some_and(|p: i32| p == me() || bsd_info(p).is_none()) {
            // An active watcher still needs the records to kill descendants.
            // Its lock is dropped by the kernel even if the watcher is killed.
            let _holder = match std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(e.path().join("watcher"))
            {
                Ok(file) => {
                    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
                        let error = io::Error::last_os_error();
                        if error.raw_os_error() != Some(libc::EWOULDBLOCK) {
                            return Err(error);
                        }
                        continue;
                    }
                    Some(file)
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(error),
            };
            match std::fs::remove_dir_all(e.path()) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
    }
    Ok(())
}

fn start_watcher(init: ProcessIdentity) -> Result<(), String> {
    let mut command =
        std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .arg("--pidns-watch")
        .arg(init.host_pid.to_string())
        .arg(init.start_seconds.to_string())
        .arg(init.start_microseconds.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped());
    let mut child = aim_storage::inherited::stdio_only(&mut command)
        .spawn()
        .map_err(|e| e.to_string())?;
    // Reap the intermediate helper before the guest can wait for children.
    // The watcher itself belongs to launchd, outside the guest namespace.
    let status = child.wait().map_err(|e| e.to_string())?;
    let mut ready = [0];
    child
        .stdout
        .take()
        .unwrap()
        .read_exact(&mut ready)
        .map_err(|e| e.to_string())?;
    if !status.success() || ready != [1] {
        return Err("pid namespace watcher did not start".into());
    }
    Ok(())
}

/// Internal linux-run helper. Fork only here, in the fresh, single-threaded
/// helper, never in the guest's multithreaded runtime.
pub fn watch(init: ProcessIdentity) -> io::Result<()> {
    // SAFETY: no other threads or runtime owners have been initialized.
    match unsafe { libc::fork() } {
        -1 => return Err(io::Error::last_os_error()),
        0 => {}
        // SAFETY: the intermediate process owns no state to tear down.
        _ => unsafe { libc::_exit(0) },
    }
    // SAFETY: detach from the guest's session and process group.
    if unsafe { libc::setsid() } < 0 {
        return Err(io::Error::last_os_error());
    }
    let dir = std::env::temp_dir().join(format!("{PRIVATE}{}", init.host_pid));
    watch_table(&dir, init, || {
        // Close the readiness pipe before waiting.
        let mut ready = unsafe { std::fs::File::from_raw_fd(1) };
        ready.write_all(&[1])
    })
}

fn watch_table(
    dir: &Path,
    init: ProcessIdentity,
    ready: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    // Register before creating the table: even death during startup leaves
    // either no table or a watcher. NOTE_EXIT survives exec and PID reuse.
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        return Err(io::Error::last_os_error());
    }
    let kq = unsafe { OwnedFd::from_raw_fd(kq) };
    let mut event: libc::kevent = unsafe { std::mem::zeroed() };
    event.ident = init.host_pid as usize;
    event.filter = libc::EVFILT_PROC;
    event.flags = libc::EV_ADD | libc::EV_ONESHOT;
    event.fflags = libc::NOTE_EXIT;
    if unsafe {
        libc::kevent(
            kq.as_raw_fd(),
            &event,
            1,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    if !init.is_live() {
        return Err(io::Error::from_raw_os_error(libc::ESRCH));
    }
    std::fs::create_dir(dir)?;
    // Retain the directory so its inode cannot be recycled under this watcher.
    let directory = std::fs::File::open(dir)?;
    let original = directory.metadata()?;
    let holder = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(dir.join("watcher"));
    let result = holder
        .as_ref()
        .map_err(|e| io::Error::from_raw_os_error(e.raw_os_error().unwrap_or(libc::EIO)))
        .and_then(|holder| {
            if unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
                return Err(io::Error::last_os_error());
            }
            ready()?;
            loop {
                match unsafe {
                    libc::kevent(
                        kq.as_raw_fd(),
                        std::ptr::null(),
                        0,
                        &mut event,
                        1,
                        std::ptr::null(),
                    )
                } {
                    1 => break Ok(()),
                    -1 if io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) => {
                        continue;
                    }
                    _ => break Err(io::Error::last_os_error()),
                }
            }
        });
    // Normal exit may already have removed it; a reused PID may have made
    // another table. Never remove that later namespace's directory.
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.dev() == original.dev() && m.ino() == original.ino() => {
            leave_table(dir, init.host_pid)?;
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    result
}

/// This process entered the namespace of table `dir` from outside (it was
/// not forked or exec'd by a member): record the group and session it
/// brought along, which read as 0 even once their leader is gone. One it
/// leads itself is the namespace's own.
pub fn enter(dir: &std::path::Path) {
    use std::io::Write;
    let file = dir.join(OUTSIDE);
    let known = std::fs::read_to_string(&file).unwrap_or_default();
    // SAFETY: trivial.
    let ids = unsafe { [libc::getpgid(0), libc::getsid(0)] };
    let line: String = ids
        .iter()
        .filter(|&&id| id > 0 && id != me() && !listed(&known, id))
        .map(|id| format!("{id}\n"))
        .collect();
    if !line.is_empty() {
        // One short O_APPEND write: entries of concurrent entrants do not mix.
        let _ = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&file)
            .and_then(|mut f| f.write_all(line.as_bytes()));
    }
}

/// This process is ending. The init of a private namespace takes it along:
/// the kernel kills every process of a pid namespace whose init exits
/// (pid_namespaces(7)), and its table goes.
pub fn leave() {
    let Some(dir) = super::cred::by_pid_dir()
        .filter(|d| d.file_name().and_then(|n| n.to_str()) == Some(&format!("{PRIVATE}{}", me())))
    else {
        return;
    };
    if let Err(error) = leave_table(dir, me()) {
        crate::diag!("linux-run: pid namespace teardown: {error}");
    }
}

fn leave_table(dir: &Path, init: i32) -> io::Result<()> {
    // The table goes once no member can add to it: a member killed as it
    // starts may still be writing its files, and one may be forking.
    loop {
        let entries = match table_members_info(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let alive: Vec<i32> = entries
            .into_iter()
            .filter(|(p, info)| *p != init && info.pbi_status != libc::SZOMB)
            .map(|(p, _)| p)
            .collect();
        if alive.is_empty() {
            break;
        }
        for &p in &alive {
            // SAFETY: a process of this namespace.
            if unsafe { libc::kill(p, libc::SIGKILL) } < 0 {
                let error = io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::ESRCH) {
                    return Err(error);
                }
            }
        }
        if !wait_exits(&alive) {
            return Err(io::Error::from_raw_os_error(libc::ETIMEDOUT));
        }
    }
    match std::fs::remove_dir_all(dir) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// Wait until the processes `pids` have exited; false if one has not
/// within 10 s.
fn wait_exits(pids: &[i32]) -> bool {
    // SAFETY: a kqueue of our own, closed below.
    let kq = unsafe { libc::kqueue() };
    if kq < 0 {
        return false;
    }
    let mut left = 0;
    for &p in pids {
        // SAFETY: an all-zero kevent is valid.
        let mut ev: libc::kevent = unsafe { std::mem::zeroed() };
        ev.ident = p as usize;
        ev.filter = libc::EVFILT_PROC;
        ev.flags = libc::EV_ADD | libc::EV_ONESHOT;
        ev.fflags = libc::NOTE_EXIT;
        // SAFETY: registering one event; a process already gone is ESRCH.
        if unsafe { libc::kevent(kq, &ev, 1, std::ptr::null_mut(), 0, std::ptr::null()) } == 0 {
            left += 1;
        }
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while left > 0 {
        let Some(t) = deadline.checked_duration_since(std::time::Instant::now()) else {
            break;
        };
        let ts = libc::timespec {
            tv_sec: t.as_secs() as libc::time_t,
            tv_nsec: t.subsec_nanos() as libc::c_long,
        };
        // SAFETY: an all-zero kevent is valid.
        let mut ev: libc::kevent = unsafe { std::mem::zeroed() };
        // SAFETY: waiting for one event into our buffer.
        match unsafe { libc::kevent(kq, std::ptr::null(), 0, &mut ev, 1, &ts) } {
            n if n > 0 => left -= 1,
            n if n < 0 && crate::errno::last() == libc::EINTR => {}
            _ => break,
        }
    }
    // SAFETY: our kqueue.
    unsafe { libc::close(kq) };
    left == 0
}

/// Whether the table's entry for `pid` names the running process `info`.
fn entry_names(dir: &std::path::Path, pid: i32, info: &libc::proc_bsdinfo) -> bool {
    let Ok(written) =
        std::fs::symlink_metadata(dir.join(pid.to_string())).and_then(|m| m.modified())
    else {
        return false;
    };
    let started = SystemTime::UNIX_EPOCH
        + Duration::from_secs(info.pbi_start_tvsec)
        + Duration::from_micros(info.pbi_start_tvusec);
    started <= written
}

/// The members with their host process information: this process and the
/// live processes of the table. None without a table.
fn members_info() -> Option<Vec<(i32, libc::proc_bsdinfo)>> {
    let dir = super::cred::by_pid_dir()?;
    let me = me();
    let mut v: Vec<(i32, libc::proc_bsdinfo)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .filter(|&p: &i32| p > 0 && p != me)
        .filter_map(|p| Some((p, bsd_info(p)?)))
        .filter(|(p, info)| entry_names(dir, *p, info))
        .collect();
    v.extend(bsd_info(me).map(|i| (me, i)));
    v.sort_unstable_by_key(|m| m.0);
    Some(v)
}

fn table_members_info(dir: &Path) -> io::Result<Vec<(i32, libc::proc_bsdinfo)>> {
    Ok(std::fs::read_dir(dir)?
        .collect::<io::Result<Vec<_>>>()?
        .into_iter()
        .filter_map(|e| e.file_name().to_str()?.parse().ok())
        .filter(|&p: &i32| p > 0)
        .filter_map(|p| Some((p, bsd_info(p)?)))
        .filter(|(p, info)| entry_names(dir, *p, info))
        .collect())
}

static INIT_OWNER: std::sync::OnceLock<
    Result<Option<aim_storage::process_namespace::InitRegistration>, i64>,
> = std::sync::OnceLock::new();
static PID_READY: std::sync::OnceLock<Result<bool, i64>> = std::sync::OnceLock::new();
/// A process joins one namespace for its life. Retain its authenticated init
/// record, then validate incarnation only when resolving an externally named PID.
pub fn init_registration() -> Result<Option<aim_storage::process_namespace::InitRegistration>, i64>
{
    INIT_OWNER
        .get_or_init(|| {
            let Some(table) = super::cred::by_pid_dir() else {
                return Ok(None);
            };
            match aim_storage::process_namespace::InitRegistration::read(table) {
                Ok(init) => Ok(Some(init)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(-(crate::errno::from_darwin(
                    error.raw_os_error().unwrap_or(libc::EIO),
                ) as i64)),
            }
        })
        .clone()
}
fn pid_ready() -> Result<bool, i64> {
    *PID_READY.get_or_init(|| {
        let Some(table) = super::cred::by_pid_dir() else {
            return Ok(false);
        };
        match std::fs::read(table.join("namespace-pid-ready")) {
            Ok(bytes) if bytes == b"AIMNS-PID-READY1\n" => Ok(true),
            Ok(_) => Err(-(71i64)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => {
                Err(-(crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)) as i64))
            }
        }
    })
}

/// Resolve a guest identifier without ever treating Darwin PID 1 as Android init.
pub fn host_pid(guest: i32) -> Result<i32, i64> {
    if guest == 1 {
        let Some(init) = init_registration()? else {
            return Err(-(ESRCH as i64));
        };
        if !pid_ready()? || !init.process.is_live() {
            return Err(-(ESRCH as i64));
        }
        return Ok(init.process.host_pid);
    }
    if contains(guest) {
        Ok(guest)
    } else {
        Err(-(ESRCH as i64))
    }
}
pub fn is_namespace_init(host: i32) -> Result<bool, i64> {
    Ok(is_private_init(host)
        || pid_ready()? && init_registration()?.is_some_and(|init| init.process.host_pid == host))
}

/// Whether host process `host` is the init of this process's private
/// namespace: the process the table was made for ([`new_table`]).
pub fn is_private_init(host: i32) -> bool {
    super::cred::by_pid_dir().is_some_and(|d| {
        d.file_name().and_then(|n| n.to_str()) == Some(&format!("{PRIVATE}{host}"))
    })
}

/// Native namespace init can receive only a signal whose real host handler is installed.
pub fn init_signal_allowed(host: i32, signal: i32) -> Result<bool, i64> {
    let Some(init) = init_registration()? else {
        return Ok(true);
    };
    if init.process.host_pid != host || !pid_ready()? {
        return Ok(true);
    }
    if signal == 0 {
        return Ok(true);
    }
    let table = super::cred::by_pid_dir().ok_or(-(ESRCH as i64))?;
    let caught = aim_storage::process_namespace::init_caught_signals(table, init.process).map_err(
        |error| -(crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)) as i64),
    )?;
    Ok((1..=64).contains(&signal) && caught & (1u64 << (signal - 1)) != 0)
}

pub fn syscall_pid(pid: i32) -> Result<i32, i64> {
    if pid == 1 { host_pid(pid) } else { Ok(pid) }
}

pub fn guest_pid(host: i32) -> Result<i32, i64> {
    Ok(
        if pid_ready()? && init_registration()?.is_some_and(|init| init.process.host_pid == host) {
            1
        } else {
            host
        },
    )
}

/// The pids of the members, in order. None without a table.
pub fn members() -> Option<Vec<i32>> {
    Some(members_info()?.into_iter().map(|m| m.0).collect())
}

/// Whether host process `pid` is in the namespace (without one, whether it
/// exists).
pub fn contains(pid: i32) -> bool {
    if pid == me() {
        return true;
    }
    let dir = super::cred::by_pid_dir();
    pid > 0
        && bsd_info(pid).is_some_and(|info| {
            dir.is_none_or(|d| info.pbi_ppid as i32 == me() || entry_names(d, pid, &info))
        })
}

/// Whether host process `pid` exists and has not exited.
pub fn running(pid: i32) -> bool {
    bsd_info(pid).is_some_and(|i| i.pbi_status != libc::SZOMB)
}

/// Whether host process `pid` is a child of this one (zombies too).
pub fn is_child(pid: i32) -> bool {
    bsd_info(pid).is_some_and(|i| i.pbi_ppid as i32 == me())
}

/// A host pid as the namespace numbers it: itself for a member, else 0.
pub fn vnr(pid: i32) -> i32 {
    if contains(pid) { pid } else { 0 }
}

/// `pid` if it is in the namespace, else ESRCH.
pub fn check(pid: i32) -> Result<i32, i64> {
    if contains(pid) {
        Ok(pid)
    } else {
        Err(-(ESRCH as i64))
    }
}

/// Retained incarnations of a controlling terminal's actual namespace group.
pub(super) fn tty_group_members(
    pgrp: i32,
    sid: i32,
) -> Result<Vec<aim_storage::process_namespace::ProcessIdentity>, crate::errno::Errno> {
    let table = super::cred::by_pid_dir().ok_or(crate::errno::ESRCH)?;
    let mut members = Vec::new();
    for entry in std::fs::read_dir(table)
        .map_err(|error| crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?
    {
        let entry = entry.map_err(|error| {
            crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))
        })?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<i32>().ok())
        else {
            continue;
        };
        if pid <= 1 {
            continue;
        }
        let Some(info) = bsd_info(pid) else {
            continue;
        };
        if info.pbi_pgid as i32 != pgrp || unsafe { libc::getsid(pid) } != sid {
            continue;
        }
        let process =
            aim_storage::process_namespace::ProcessIdentity::running(pid).map_err(|error| {
                crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))
            })?;
        aim_storage::posix_broker::read_credentials(table, process).map_err(|error| {
            crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))
        })?;
        if process.is_live() {
            members.push(process);
        }
    }
    if members.is_empty() {
        Err(crate::errno::ESRCH)
    } else {
        Ok(members)
    }
}
pub(super) fn tty_group_orphaned(
    members: &[aim_storage::process_namespace::ProcessIdentity],
    pgrp: i32,
    sid: i32,
) -> Result<bool, crate::errno::Errno> {
    let table = super::cred::by_pid_dir().ok_or(crate::errno::ESRCH)?;
    for member in members {
        if !member.is_live() {
            continue;
        }
        let Some(info) = bsd_info(member.host_pid) else {
            continue;
        };
        let parent = info.pbi_ppid as i32;
        if parent <= 1
            || is_namespace_init(parent).map_err(|error| (-error) as crate::errno::Errno)?
        {
            continue;
        }
        let Some(parent_info) = bsd_info(parent) else {
            continue;
        };
        if parent_info.pbi_pgid as i32 == pgrp || unsafe { libc::getsid(parent) } != sid {
            continue;
        }
        let process =
            aim_storage::process_namespace::ProcessIdentity::running(parent).map_err(|error| {
                crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))
            })?;
        aim_storage::posix_broker::read_credentials(table, process).map_err(|error| {
            crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))
        })?;
        if process.is_live() {
            return Ok(false);
        }
    }
    Ok(true)
}
/// The members in process group `pgrp`. None without a table.
pub fn group(pgrp: i32) -> Option<Vec<i32>> {
    let pgrp = syscall_pid(pgrp).ok()?;
    Some(
        members_info()?
            .into_iter()
            .filter(|m| m.1.pbi_pgid as i32 == pgrp)
            .map(|m| m.0)
            .collect(),
    )
}

/// A session or process group id as the namespace numbers it: its own id
/// if it was made inside, by a member that leads it or led it and exited,
/// else 0 (pid_vnr of a pid from the parent namespace). One whose leader
/// is gone was made inside unless a process brought it in ([`enter`]).
pub fn id_in_ns(id: i32) -> i32 {
    let inside = contains(id)
        || (bsd_info(id).is_none()
            && !brought_in(id)
            && members_info().is_none_or(|m| m.iter().any(|(_, info)| info.pbi_pgid as i32 == id)));
    if inside {
        guest_pid(id).unwrap_or(0)
    } else {
        0
    }
}

fn listed(ids: &str, id: i32) -> bool {
    ids.split_whitespace().any(|i| i.parse() == Ok(id))
}

/// Whether `id` is a group or session a process brought in from outside.
fn brought_in(id: i32) -> bool {
    super::cred::by_pid_dir()
        .and_then(|d| std::fs::read_to_string(d.join(OUTSIDE)).ok())
        .is_some_and(|ids| listed(&ids, id))
}

#[cfg(test)]
mod registration_tests {
    use super::*;
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }
    fn sleeper() -> Child {
        Child(Command::new("/bin/sleep").arg("60").spawn().unwrap())
    }
    fn test_table(init: ProcessIdentity) -> PathBuf {
        std::env::temp_dir().join(format!(
            "aim-pidns-test-{}-{}-{}",
            init.host_pid, init.start_seconds, init.start_microseconds
        ))
    }
    #[test]
    fn watcher_tears_down_only_its_namespace_after_sigkill() {
        let mut init = sleeper();
        let mut member = sleeper();
        let mut outside = sleeper();
        let identity = ProcessIdentity::running(init.0.id() as i32).unwrap();
        let table = test_table(identity);
        let watched = table.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let watcher = std::thread::spawn(move || {
            watch_table(&watched, identity, || {
                tx.send(()).unwrap();
                Ok(())
            })
        });
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        std::fs::write(table.join(member.0.id().to_string()), "member").unwrap();
        init.0.kill().unwrap();
        init.0.wait().unwrap();
        watcher.join().unwrap().unwrap();
        assert!(!table.exists());
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(member.0.wait().unwrap().signal(), Some(libc::SIGKILL));
        assert!(outside.0.try_wait().unwrap().is_none());
    }
    #[test]
    fn watcher_rejects_a_reused_pid_before_creating_a_table() {
        let init = sleeper();
        let mut identity = ProcessIdentity::running(init.0.id() as i32).unwrap();
        let table = test_table(identity);
        identity.start_seconds -= 1;
        let error = watch_table(&table, identity, || panic!("not ready")).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ESRCH));
        assert!(!table.exists());
    }
    #[test]
    fn watcher_does_not_remove_a_replacement_table() {
        let mut init = sleeper();
        let identity = ProcessIdentity::running(init.0.id() as i32).unwrap();
        let table = test_table(identity);
        let watched = table.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let watcher = std::thread::spawn(move || {
            watch_table(&watched, identity, || {
                tx.send(()).unwrap();
                Ok(())
            })
        });
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let old = table.with_extension("old");
        std::fs::rename(&table, &old).unwrap();
        std::fs::create_dir(&table).unwrap();
        std::fs::write(table.join("replacement"), "keep").unwrap();
        init.0.kill().unwrap();
        init.0.wait().unwrap();
        watcher.join().unwrap().unwrap();
        assert_eq!(std::fs::read(table.join("replacement")).unwrap(), b"keep");
        std::fs::remove_dir_all(table).unwrap();
        std::fs::remove_dir_all(old).unwrap();
    }
    #[test]
    fn watcher_removes_a_table_if_readiness_fails() {
        let init = sleeper();
        let identity = ProcessIdentity::running(init.0.id() as i32).unwrap();
        let table = test_table(identity);
        let error = watch_table(&table, identity, || {
            Err(io::Error::from_raw_os_error(libc::EPIPE))
        })
        .unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EPIPE));
        assert!(!table.exists());
    }
    #[test]
    fn sweeping_retains_a_watchers_table_until_its_lock_is_released() {
        let mut init = sleeper();
        let identity = ProcessIdentity::running(init.0.id() as i32).unwrap();
        let root = test_table(identity);
        let table = root.join(format!("{PRIVATE}{}", identity.host_pid));
        std::fs::create_dir_all(&table).unwrap();
        let holder = std::fs::File::create(table.join("watcher")).unwrap();
        assert_eq!(unsafe { libc::flock(holder.as_raw_fd(), libc::LOCK_EX) }, 0);
        init.0.kill().unwrap();
        init.0.wait().unwrap();
        sweep_tables(&root).unwrap();
        assert!(table.exists());
        drop(holder);
        // A concurrent subprocess test can briefly inherit the descriptor
        // between fork and exec, before close-on-exec releases its copy.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while table.exists() && std::time::Instant::now() < deadline {
            sweep_tables(&root).unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!table.exists());
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    #[ignore = "subprocess helper exercised by registered_init_maps_actual_process_without_touching_host_pid_one"]
    fn child_mapping_probe() {
        let mut table = String::new();
        std::io::stdin().read_line(&mut table).unwrap();
        super::super::cred::init(
            super::super::cred::Identity::default(),
            Some(std::path::PathBuf::from(table.trim())),
        );
        let table = std::path::PathBuf::from(table.trim());
        let input = std::fs::read_to_string(table.join("test-paths")).unwrap();
        let mut paths = input.lines();
        crate::vfs::init(
            std::path::Path::new(paths.next().unwrap()),
            Some(std::path::Path::new(paths.next().unwrap())),
        )
        .unwrap();
        assert_eq!(
            host_pid(1).unwrap(),
            init_registration().unwrap().unwrap().process.host_pid
        );
        assert_ne!(host_pid(1).unwrap(), 1);
        let host = host_pid(1).unwrap();
        assert!(init_signal_allowed(host, 0).unwrap());
        assert!(init_signal_allowed(host, 15).unwrap());
        assert!(!init_signal_allowed(host, 9).unwrap());
        assert!(!init_signal_allowed(host, 19).unwrap());
        // Dropped, as Linux drops a signal init does not handle: the send
        // succeeds.
        assert_eq!(super::super::signal::kill([1, 9, 0, 0, 0, 0]), 0);
        assert_eq!(super::super::signal::kill([1, 0, 0, 0, 0, 0]), 0);
        let header = [0x20080522u32, 1];
        let mut capabilities = [0u32; 6];
        assert_eq!(
            super::super::cred::capget([
                header.as_ptr() as u64,
                capabilities.as_mut_ptr() as u64,
                0,
                0,
                0,
                0
            ]),
            0
        );
        let mut queue = [0u8; 128];
        let info =
            super::super::sigframe::Siginfo::from_sender(15, -1, unsafe { libc::getpid() }, 0);
        unsafe { info.write(queue.as_mut_ptr() as u64) }.unwrap();
        assert_eq!(
            super::super::signal::rt_sigqueueinfo([1, 15, queue.as_ptr() as u64, 0, 0, 0]),
            -(crate::errno::EOPNOTSUPP as i64)
        );
        let mut status = 0i32;
        assert_eq!(
            super::super::wait::wait4([1, (&mut status as *mut i32) as u64, 1, 0, 0, 0]),
            -(libc::ECHILD as i64)
        );
        assert_eq!(
            super::super::pstate::getpgid([1, 0, 0, 0, 0, 0]),
            id_in_ns(unsafe { libc::getpgid(host) }) as i64
        );
        assert_eq!(
            super::super::pstate::getsid([1, 0, 0, 0, 0, 0]),
            id_in_ns(unsafe { libc::getsid(host) }) as i64
        );
        assert_eq!(
            super::super::cred::getpriority([0, 1, 0, 0, 0, 0]),
            20 - unsafe { libc::getpriority(libc::PRIO_PROCESS, host as u32) } as i64
        );
        assert_eq!(guest_pid(host_pid(1).unwrap()).unwrap(), 1);
        assert_eq!(syscall_pid(unsafe { libc::getpid() }).unwrap(), unsafe {
            libc::getpid()
        });
        assert!(
            !contains(1),
            "Darwin PID1 must not become a host-roster member"
        );
        let path = std::ffi::CString::new("/proc/1/mountinfo").unwrap();
        let fd = super::super::fs::openat([
            crate::vfs::LINUX_AT_FDCWD as u64,
            path.as_ptr() as u64,
            0,
            0,
            0,
            0,
        ]);
        assert!(fd >= 0, "mountinfoopen {fd}");
        let mut buffer = [0u8; 8192];
        let read = super::super::fs::read([
            fd as u64,
            buffer.as_mut_ptr() as u64,
            buffer.len() as u64,
            0,
            0,
            0,
        ]);
        assert!(read > 0);
        assert!(String::from_utf8_lossy(&buffer[..read as usize]).contains("/actual-init-mount"));
        assert_eq!(super::super::fs::close([fd as u64, 0, 0, 0, 0, 0]), 0);
        assert_eq!(super::super::process::getppid(), 1);
    }
    #[test]
    fn registered_init_maps_actual_process_without_touching_host_pid_one() {
        let root = aim_paths::scratch_dir("aim-init-map");
        let actual =
            aim_storage::process_namespace::ProcessIdentity::running(unsafe { libc::getpid() })
                .unwrap();
        aim_storage::process_namespace::InitRegistration::register(
            &root,
            actual,
            "actual-mount-owner",
        )
        .unwrap();
        aim_storage::process_namespace::register_mount_namespace(
            &root,
            actual,
            "actual-mount-owner",
        )
        .unwrap();
        let runtime = root
            .parent()
            .unwrap()
            .join(format!("runtime-init-map-{}", std::process::id()));
        std::fs::create_dir_all(&runtime).unwrap();
        let image = runtime.join("image");
        std::fs::create_dir(&image).unwrap();
        let mount = runtime.join("actual-mount");
        std::fs::create_dir(&mount).unwrap();
        let map = runtime.join("path-map");
        let text = format!(
            "root\t/\t{}\nrw\t/data\t{}\n",
            image.display(),
            runtime.display()
        );
        std::fs::write(&map, &text).unwrap();
        let owner =
            aim_storage::mount_namespace::Namespace::open(&runtime, "actual-mount-owner").unwrap();
        owner.initialize(&text).unwrap();
        owner
            .append(&format!(
                "rw\t/actual-init-mount\t{}\ttmpfs\ttmpfs\n",
                mount.display()
            ))
            .unwrap();
        std::fs::write(
            root.join(actual.host_pid.to_string()),
            b"actual-host-roster",
        )
        .unwrap();
        std::fs::write(
            root.join("test-paths"),
            format!("{}\n{}\n", image.display(), map.display()),
        )
        .unwrap();
        aim_storage::process_namespace::register_init_signals(&root, actual, 1u64 << (15 - 1))
            .unwrap();
        aim_storage::process_namespace::InitRegistration::read(&root)
            .unwrap()
            .activate_pid_mapping(&root)
            .unwrap();
        let mut child = Child(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "sys::pidns::registration_tests::child_mapping_probe",
                    "--ignored",
                    "--nocapture",
                ])
                .stdin(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let child_identity =
            aim_storage::process_namespace::ProcessIdentity::running(child.0.id() as i32).unwrap();
        aim_storage::process_namespace::register_mount_namespace(
            &root,
            child_identity,
            "actual-mount-owner",
        )
        .unwrap();
        writeln!(child.0.stdin.take().unwrap(), "{}", root.display()).unwrap();
        assert!(child.0.wait().unwrap().success());
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(runtime).unwrap();
    }
}
