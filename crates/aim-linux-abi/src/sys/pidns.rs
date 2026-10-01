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

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::errno::ESRCH;

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
/// as its init. Tables whose init no longer runs are removed.
pub fn new_table() -> Result<PathBuf, String> {
    let tmp = std::env::temp_dir();
    for e in std::fs::read_dir(&tmp).into_iter().flatten().flatten() {
        let name = e.file_name();
        let init = name
            .to_str()
            .and_then(|n| n.strip_prefix(PRIVATE)?.parse().ok());
        if init.is_some_and(|p: i32| p == me() || bsd_info(p).is_none()) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    let dir = tmp.join(format!("{PRIVATE}{}", me()));
    std::fs::create_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(dir)
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
    // The table goes once no member can add to it: a member killed as it
    // starts may still be writing its files, and one may be forking.
    loop {
        let alive: Vec<i32> = members()
            .unwrap_or_default()
            .into_iter()
            .filter(|&p| p != me() && running(p))
            .collect();
        if alive.is_empty() {
            break;
        }
        for &p in &alive {
            // SAFETY: a process of this namespace.
            unsafe { libc::kill(p, libc::SIGKILL) };
        }
        if !wait_exits(&alive) {
            break;
        }
    }
    let _ = std::fs::remove_dir_all(dir);
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

/// The members in process group `pgrp`. None without a table.
pub fn group(pgrp: i32) -> Option<Vec<i32>> {
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
    if inside { id } else { 0 }
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
