//! Membership fragments. Births take shared policy/state locks and change
//! only their process's fragment. An exclusive transaction folds all fragments
//! into the base file while holding its lock, including across rollback.
use super::*;
use std::fs::{File as HostFile, OpenOptions};
use std::io::{Seek as _, Write as _};

pub(super) fn lock_file(path: &Path, operation: i32) -> Result<HostFile, Errno> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(io_error)?;
    crate::errno::retry_host(|| unsafe { libc::flock(file.as_raw_fd(), operation) })?;
    Ok(file)
}

pub(super) fn policy_lock(root: &Path, operation: i32) -> Result<HostFile, Errno> {
    let lock_root = vfs::cgroup_v1_hierarchies()
        .first()
        .map(|(_, p)| p.clone())
        .unwrap_or_else(|| root.to_path_buf());
    lock_file(&lock_root.with_extension("policy-lock"), operation)
}

fn directory(root: &Path) -> PathBuf {
    let mut path = cgroup_v1::state_path(root).into_os_string();
    path.push(".d");
    path.into()
}

fn text(file: &mut HostFile) -> Result<String, Errno> {
    let mut out = String::new();
    file.read_to_string(&mut out).map_err(io_error)?;
    Ok(out)
}

fn replace(file: &mut HostFile, bytes: &[u8]) -> Result<(), Errno> {
    crate::errno::retry_io(|| file.rewind()).map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    crate::errno::retry_io(|| file.set_len(bytes.len() as u64)).map_err(io_error)
}

fn merge(m: &mut Membership, fragment: &Membership) {
    for entry in &fragment.threads {
        m.threads.retain(|(tid, ..)| *tid != entry.0);
        m.threads.push(entry.clone());
    }
}

/// The caller holds the base file lock, excluding consolidation. Fragment
/// locks exclude partial reads from births in that process.
pub(super) fn fragments(root: &Path, m: &mut Membership) -> Result<Vec<(PathBuf, String)>, Errno> {
    let entries = match std::fs::read_dir(directory(root)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_error(e)),
    };
    let mut saved = Vec::new();
    for entry in entries {
        let path = entry.map_err(io_error)?.path();
        let mut file = lock_file(&path, libc::LOCK_SH)?;
        let contents = text(&mut file)?;
        let fragment = Membership::parse(&contents);
        let pid = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.parse().ok());
        if let Some(pid) = pid
            && fragment
                .threads
                .first()
                .is_some_and(|(_, start, _)| live(pid, *start))
        {
            merge(m, &fragment);
        }
        saved.push((path, contents));
    }
    Ok(saved)
}

pub(super) fn read(root: &Path) -> Result<Membership, Errno> {
    let mut file = match HostFile::open(cgroup_v1::state_path(root)) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Membership::default()),
        Err(e) => return Err(io_error(e)),
    };
    crate::errno::retry_host(|| unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) })?;
    let mut m = Membership::parse(&text(&mut file)?);
    fragments(root, &mut m)?;
    Ok(m)
}

pub(super) fn commit(
    file: &mut HostFile,
    before: &str,
    after: &str,
    fragments: &[(PathBuf, String)],
) -> Result<(), Errno> {
    let result = (|| {
        replace(file, after.as_bytes())?;
        for (path, _) in fragments {
            std::fs::remove_file(path).map_err(io_error)?;
        }
        Ok(())
    })();
    if result.is_err() {
        replace(file, before.as_bytes())?;
        for (path, contents) in fragments {
            std::fs::write(path, contents).map_err(io_error)?;
        }
    }
    result
}

/// No pruning, guest-wide enumeration or host policy update: only this new
/// task's membership changes. Policy is derived at thread start under the
/// same shared policy lock; a migration must wait until inheritance is saved.
pub(super) fn inherit(root: &Path, creator: i32, tid: i32, start: Start) -> Result<(), Errno> {
    let _policy = policy_lock(root, libc::LOCK_SH)?;
    let mut base = lock_file(&cgroup_v1::state_path(root), libc::LOCK_SH)?;
    let mut m = Membership::parse(&text(&mut base)?);
    let path = directory(root).join(me().to_string());
    match std::fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if !m
                .threads
                .iter()
                .any(|(t, saved, _)| *t == creator && *saved == start)
            {
                return Ok(());
            }
            std::fs::create_dir_all(directory(root)).map_err(io_error)?;
        }
        Err(e) => return Err(io_error(e)),
        Ok(_) => {}
    }
    let mut file = lock_file(&path, libc::LOCK_EX)?;
    let before = text(&mut file)?;
    let mut fragment = Membership::parse(&before);
    fragment.threads.retain(|(_, saved, _)| *saved == start);
    merge(&mut m, &fragment);
    // An implicit process membership needs no per-thread record. Explicit
    // membership must be copied, even '/' (a thread moved out of its process).
    if let Some((.., group)) = m
        .threads
        .iter()
        .find(|(t, saved, _)| *t == creator && *saved == start)
    {
        fragment.threads.retain(|(t, ..)| *t != tid);
        fragment.threads.push((tid, start, group.clone()));
        if let Err(error) = replace(&mut file, fragment.text().as_bytes()) {
            replace(&mut file, before.as_bytes())?;
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Root(PathBuf);
    impl Root {
        fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("aim-2389-{name}-{}", me()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p.join("cpu"))
        }
        fn base(&self) -> HostFile {
            lock_file(&cgroup_v1::state_path(&self.0), libc::LOCK_EX).unwrap()
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            std::fs::remove_dir_all(self.0.parent().unwrap()).unwrap();
        }
    }
    fn entry(tid: i32, group: &str) -> Membership {
        Membership {
            processes: Vec::new(),
            threads: vec![(tid, own_start().unwrap(), group.into())],
        }
    }
    fn group(m: &Membership, tid: i32) -> &str {
        &m.threads.iter().find(|(t, ..)| *t == tid).unwrap().2
    }
    #[test]
    fn inherited_membership_is_independent_and_consolidates_on_migration() {
        let r = Root::new("inherit");
        replace(&mut r.base(), entry(10, "/a").text().as_bytes()).unwrap();
        inherit(&r.0, 10, 20, own_start().unwrap()).unwrap();
        inherit(&r.0, 20, 30, own_start().unwrap()).unwrap();
        // A move sees every birth. Moving only the creator leaves its children.
        let mut base = r.base();
        let before = text(&mut base).unwrap();
        let mut m = Membership::parse(&before);
        let saved = fragments(&r.0, &mut m).unwrap();
        m.threads.iter_mut().find(|(t, ..)| *t == 10).unwrap().2 = "/b".into();
        commit(&mut base, &before, &m.text(), &saved).unwrap();
        drop(base);
        inherit(&r.0, 10, 40, own_start().unwrap()).unwrap();
        let m = read(&r.0).unwrap();
        assert_eq!(group(&m, 20), "/a");
        assert_eq!(group(&m, 30), "/a");
        assert_eq!(group(&m, 40), "/b");
        // An explicit root override must survive, too.
        replace(&mut r.base(), entry(10, "/").text().as_bytes()).unwrap();
        inherit(&r.0, 10, 50, own_start().unwrap()).unwrap();
        assert_eq!(group(&read(&r.0).unwrap(), 50), "/");
    }
    #[test]
    fn concurrent_births_and_reads_do_not_lose_records() {
        let r = Root::new("parallel");
        replace(&mut r.base(), entry(10, "/a").text().as_bytes()).unwrap();
        std::thread::scope(|s| {
            for worker in 0..8 {
                let r = &r;
                s.spawn(move || {
                    for i in 0..32 {
                        let tid = 100 + worker * 32 + i;
                        inherit(&r.0, 10, tid, own_start().unwrap()).unwrap();
                        assert_eq!(group(&read(&r.0).unwrap(), tid), "/a");
                    }
                });
            }
        });
        assert_eq!(read(&r.0).unwrap().threads.len(), 257);
    }
    #[test]
    fn births_racing_migration_remain_independently_attached() {
        let r = Root::new("migrate");
        replace(&mut r.base(), entry(10, "/a").text().as_bytes()).unwrap();
        std::thread::scope(|s| {
            s.spawn(|| {
                for i in 0..64 {
                    let _policy = policy_lock(&r.0, libc::LOCK_EX).unwrap();
                    let mut base = r.base();
                    let before = text(&mut base).unwrap();
                    let mut m = Membership::parse(&before);
                    let saved = fragments(&r.0, &mut m).unwrap();
                    m.threads.iter_mut().find(|(t, ..)| *t == 10).unwrap().2 =
                        if i % 2 == 0 { "/b" } else { "/a" }.into();
                    commit(&mut base, &before, &m.text(), &saved).unwrap();
                }
            });
            for worker in 0..8 {
                let r = &r;
                s.spawn(move || {
                    for i in 0..16 {
                        let tid = 100 + worker * 16 + i;
                        inherit(&r.0, 10, tid, own_start().unwrap()).unwrap();
                        let inherited = group(&read(&r.0).unwrap(), tid).to_string();
                        assert!(inherited == "/a" || inherited == "/b");
                        std::thread::yield_now();
                        assert_eq!(group(&read(&r.0).unwrap(), tid), inherited);
                    }
                });
            }
        });
        assert_eq!(read(&r.0).unwrap().threads.len(), 129);
    }

    #[test]
    fn fragments_from_a_previous_process_incarnation_are_not_inherited() {
        let r = Root::new("reuse");
        replace(&mut r.base(), entry(10, "/a").text().as_bytes()).unwrap();
        std::fs::create_dir_all(directory(&r.0)).unwrap();
        let path = directory(&r.0).join(me().to_string());
        let old = Membership {
            processes: Vec::new(),
            threads: vec![(20, (1, 1), "/old".into())],
        };
        std::fs::write(&path, old.text()).unwrap();
        assert_eq!(read(&r.0).unwrap().threads.len(), 1);
        inherit(&r.0, 10, 20, own_start().unwrap()).unwrap();
        assert_eq!(group(&read(&r.0).unwrap(), 20), "/a");
    }

    #[test]
    fn implicit_process_membership_needs_no_fragment() {
        let r = Root::new("implicit");
        let mut m = Membership::default();
        m.processes.push((me(), own_start().unwrap(), "/a".into()));
        replace(&mut r.base(), m.text().as_bytes()).unwrap();
        inherit(&r.0, me(), 20, own_start().unwrap()).unwrap();
        assert!(!directory(&r.0).exists());
        assert_eq!(read(&r.0).unwrap(), m);
    }

    #[test]
    fn failed_commit_restores_base_and_fragments() {
        let r = Root::new("rollback");
        let mut base = r.base();
        let before = entry(10, "/a").text();
        replace(&mut base, before.as_bytes()).unwrap();
        std::fs::create_dir_all(directory(&r.0)).unwrap();
        let path = directory(&r.0).join("first");
        let fragment = entry(20, "/a").text();
        std::fs::write(&path, &fragment).unwrap();
        // Force removal failure after the first fragment was removed.
        let bad = directory(&r.0).join("directory");
        std::fs::create_dir(&bad).unwrap();
        let result = commit(
            &mut base,
            &before,
            &entry(10, "/b").text(),
            &[(path.clone(), fragment.clone()), (bad, String::new())],
        );
        assert!(result.is_err());
        base.rewind().unwrap();
        assert_eq!(text(&mut base).unwrap(), before);
        assert_eq!(std::fs::read_to_string(path).unwrap(), fragment);
    }
}
