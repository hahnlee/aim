//! The guest's pid namespace: the processes of its process table, the
//! `by-pid` directory of `cred` (`docs/guest-init-contract.md` section 4).
//!
//! Every call that names another process resolves the pid here, as in a
//! Linux pid namespace (pid_namespaces(7)): a Mac process outside it does
//! not exist for the guest (ESRCH), `kill(-1)` and process groups reach
//! members only, and a session or group led from outside reads as 0. A
//! process started without a table has no namespace of its own and sees
//! every host process.
//!
//! An entry names a member only if its process started before the entry
//! was written: a pid the Mac reused after a member died without removing
//! its entry names a stranger. The caller's children (all forked by the
//! guest) are members, zombies included, as they are until reaped.

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
/// else 0 (pid_vnr of a pid from the parent namespace).
pub fn id_in_ns(id: i32) -> i32 {
    let inside = contains(id)
        || (bsd_info(id).is_none()
            && members_info().is_none_or(|m| m.iter().any(|(_, info)| info.pbi_pgid as i32 == id)));
    if inside { id } else { 0 }
}
