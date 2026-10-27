//! Namespace-wide cyclic task IDs, as in Linux's alloc_pid (pid_namespaces(7)).
//! Darwin assigns process IDs below 100000, including future children, so that
//! range is reserved. Pthread IDs use the rest of Linux's 64-bit PID_MAX_LIMIT;
//! both kinds are strictly below the same /proc/sys/kernel/pid_max.
//! The sparse by-pid table retains each TID's owning process incarnation. A
//! killed process's reservations can be reclaimed without trusting a reused PID.

use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{
    OnceLock,
    atomic::{AtomicU64, Ordering::SeqCst},
};

use aim_storage::{private_fd::PrivateFile, process_namespace::ProcessIdentity};

pub(in crate::sys) const PID_MAX: i32 = 1 << 22;
const FIRST: i32 = 100000;
const RECORD: u64 = 24;
const HEADER: u64 = 8;

#[repr(C)]
struct Record {
    pid: AtomicU64,
    seconds: AtomicU64,
    microseconds: AtomicU64,
}
impl Record {
    fn get(&self) -> ProcessIdentity {
        let pid = self.pid.load(SeqCst);
        let seconds = self.seconds.load(SeqCst);
        let microseconds = self.microseconds.load(SeqCst);
        ProcessIdentity {
            host_pid: if self.pid.load(SeqCst) == pid {
                pid as i32
            } else {
                0
            },
            start_seconds: seconds,
            start_microseconds: microseconds,
        }
    }
    fn set(&self, owner: ProcessIdentity) {
        // Publish the owner last. A reader racing release or allocation may
        // observe no owner; it must never pair a new PID with old start times.
        self.pid.store(0, SeqCst);
        self.seconds.store(owner.start_seconds, SeqCst);
        self.microseconds.store(owner.start_microseconds, SeqCst);
        self.pid.store(owner.host_pid as u64, SeqCst);
    }
}
const _: () = assert!(std::mem::size_of::<Record>() as u64 == RECORD);
const SIZE: usize = (HEADER + (PID_MAX - FIRST) as u64 * RECORD) as usize;

struct Mapping(usize);
// SAFETY: shared records contain only atomics; the mapping lives with its owner.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}
impl Mapping {
    fn record(&self, tid: i32) -> &Record {
        // SAFETY: all callers check FIRST <= tid < PID_MAX; the file has SIZE
        // bytes and the record is naturally aligned in its shared mapping.
        unsafe { &*((self.0 + offset(tid) as usize) as *const Record) }
    }
}
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe { libc::munmap(self.0 as *mut _, SIZE) };
    }
}

// Attach before guest execution, including in a spawned fork child. Lookups
// then need no file I/O that could turn a namespace storage error into ESRCH.
static OWN: OnceLock<Mapping> = OnceLock::new();

struct Table(PrivateFile, Mapping);
impl Table {
    fn open(dir: &Path, write: bool) -> io::Result<Self> {
        let file = PrivateFile::allocate(|| {
            OpenOptions::new()
                .read(true)
                .write(write)
                .create(write)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(dir.join("task-ids"))
        })?;
        let op = if write { libc::LOCK_EX } else { libc::LOCK_SH };
        // Each operation opens its own description: flock also serializes
        // different pthreads, and closes release it even after process death.
        while unsafe { libc::flock(file.as_raw_fd(), op) } != 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EINTR) {
                return Err(error);
            }
        }
        let mut file = file;
        let size = SIZE as u64;
        let len = file.seek(SeekFrom::End(0))?;
        if len == 0 && write {
            if unsafe { libc::ftruncate(file.as_raw_fd(), size as i64) } != 0 {
                return Err(io::Error::last_os_error());
            }
        } else if len != size {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        let protection = libc::PROT_READ | if write { libc::PROT_WRITE } else { 0 };
        let mapped = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                SIZE,
                protection,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if mapped == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(file, Mapping(mapped as usize)))
    }

    fn read(&self, tid: i32) -> ProcessIdentity {
        self.1.record(tid).get()
    }

    fn write(&self, tid: i32, owner: ProcessIdentity) {
        self.1.record(tid).set(owner);
    }

    fn allocate(&mut self, owner: ProcessIdentity, end: i32) -> io::Result<i32> {
        self.0.seek(SeekFrom::Start(0))?;
        let mut cursor = [0; 8];
        self.0.read_exact(&mut cursor)?;
        let mut tid = i64::from_le_bytes(cursor).clamp(FIRST as i64, (end - 1) as i64) as i32;
        for _ in FIRST..end {
            let previous = self.read(tid);
            if !alive(previous)? {
                self.0.seek(SeekFrom::Start(0))?;
                let next = if tid + 1 == end { FIRST } else { tid + 1 };
                self.0.write_all(&(next as i64).to_le_bytes())?;
                self.write(tid, owner);
                return Ok(tid);
            }
            tid = if tid + 1 == end { FIRST } else { tid + 1 };
        }
        // clone(2): the namespace's available task IDs have been exhausted.
        Err(io::Error::from_raw_os_error(libc::EAGAIN))
    }
}

fn offset(tid: i32) -> u64 {
    HEADER + (tid - FIRST) as u64 * RECORD
}

fn alive(owner: ProcessIdentity) -> io::Result<bool> {
    if owner.host_pid == 0 {
        return Ok(false);
    }
    match ProcessIdentity::metadata_strict(owner.host_pid) {
        Ok(info) => Ok(info.pbi_status != libc::SZOMB
            && info.pbi_start_tvsec == owner.start_seconds
            && info.pbi_start_tvusec == owner.start_microseconds),
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => Ok(false),
        Err(e) => Err(e),
    }
}

pub(in crate::sys) struct Reservation {
    pub tid: i32,
    dir: PathBuf,
    owner: ProcessIdentity,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        let release = || -> io::Result<()> {
            let table = Table::open(&self.dir, true)?;
            if table.read(self.tid) == self.owner {
                table.write(
                    self.tid,
                    ProcessIdentity {
                        host_pid: 0,
                        ..self.owner
                    },
                );
            }
            Ok(())
        };
        if let Err(error) = release() {
            crate::diag!("release task id {}: {error}", self.tid);
        }
    }
}

pub(in crate::sys) fn allocate() -> Result<Reservation, crate::errno::Errno> {
    let allocate = || -> io::Result<Reservation> {
        initialize()?;
        let dir = super::super::cred::by_pid_dir().unwrap();
        let owner = ProcessIdentity::running_strict(unsafe { libc::getpid() })?;
        let tid = Table::open(dir, true)?.allocate(owner, PID_MAX)?;
        Ok(Reservation {
            tid,
            dir: dir.clone(),
            owner,
        })
    };
    allocate().map_err(|error| crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))
}

pub(crate) fn initialize() -> io::Result<()> {
    if OWN.get().is_none() {
        let dir = super::super::cred::by_pid_dir()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::ESRCH))?;
        let Table(file, mapping) = Table::open(dir, true)?;
        // Concurrent initializers attach the same namespace, fixed for life.
        let _ = OWN.set(mapping);
        drop(file);
    }
    Ok(())
}

pub(in crate::sys) fn owner(tid: i32) -> Option<i32> {
    if tid < FIRST {
        return Some(tid);
    }
    if tid >= PID_MAX {
        return None;
    }
    let owner = OWN.get()?.record(tid).get();
    // As for pidns::contains, an exited or reused process no longer names a
    // task of this namespace. No filesystem operation is needed for lookup.
    owner.is_live().then_some(owner.host_pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

    struct Dir(PathBuf);
    impl Dir {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "aim-task-ids-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Relaxed)
            ));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn me() -> ProcessIdentity {
        ProcessIdentity::running_strict(std::process::id() as i32).unwrap()
    }
    fn reserve(dir: &Path, end: i32) -> Reservation {
        let owner = me();
        let tid = Table::open(dir, true)
            .unwrap()
            .allocate(owner, end)
            .unwrap();
        Reservation {
            tid,
            dir: dir.to_owned(),
            owner,
        }
    }

    #[test]
    fn wraps_skips_live_ids_exhausts_and_releases() {
        let dir = Dir::new();
        let first = reserve(&dir.0, FIRST + 2);
        let second = reserve(&dir.0, FIRST + 2);
        assert_eq!((first.tid, second.tid), (FIRST, FIRST + 1));
        assert_eq!(
            Table::open(&dir.0, true)
                .unwrap()
                .allocate(me(), FIRST + 2)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EAGAIN)
        );
        drop(first);
        let reused = reserve(&dir.0, FIRST + 2);
        assert_eq!(reused.tid, FIRST);
        assert_eq!(Table::open(&dir.0, false).unwrap().read(reused.tid), me());
        drop((second, reused));
    }

    #[test]
    fn stale_incarnation_is_reclaimed_and_bound_fits_cpu_clocks() {
        let dir = Dir::new();
        let mut previous = me();
        previous.start_seconds -= 1;
        {
            let table = Table::open(&dir.0, true).unwrap();
            table.write(FIRST, previous);
        }
        let current = reserve(&dir.0, FIRST + 1);
        assert_eq!(current.tid, FIRST);
        for tid in [FIRST, PID_MAX - 1] {
            for bits in 0..=6 {
                let clock = (!(tid as u32) << 3 | bits) as i32;
                assert!(clock < 0);
                assert_eq!(!(clock >> 3), tid);
            }
        }
        drop(current);
    }

    #[test]
    fn last_id_wraps_and_existing_mapping_observes_publication_and_release() {
        let dir = Dir::new();
        let Table(file, mapping) = Table::open(&dir.0, true).unwrap();
        drop(file);
        let tid = {
            let mut table = Table::open(&dir.0, true).unwrap();
            table.0.seek(SeekFrom::Start(0)).unwrap();
            table
                .0
                .write_all(&((PID_MAX - 1) as i64).to_le_bytes())
                .unwrap();
            table.allocate(me(), PID_MAX).unwrap()
        };
        let last = Reservation {
            tid,
            dir: dir.0.clone(),
            owner: me(),
        };
        assert_eq!(last.tid, PID_MAX - 1);
        assert_eq!(mapping.record(last.tid).get(), me());
        let next = reserve(&dir.0, PID_MAX);
        assert_eq!(next.tid, FIRST);
        drop(last);
        assert_eq!(mapping.record(PID_MAX - 1).get().host_pid, 0);
        drop(next);
    }

    #[test]
    fn storage_failures_are_returned() {
        let dir = Dir::new();
        assert_eq!(
            Table::open(&dir.0, false).err().unwrap().kind(),
            io::ErrorKind::NotFound
        );
        std::fs::write(dir.0.join("task-ids"), b"bad").unwrap();
        assert_eq!(
            Table::open(&dir.0, true).err().unwrap().raw_os_error(),
            Some(libc::EIO)
        );
    }

    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            if self.0.try_wait().unwrap().is_none() {
                self.0.kill().unwrap();
            }
            self.0.wait().unwrap();
        }
    }

    #[test]
    #[ignore = "subprocess helper for task_ids_are_shared_across_processes_and_reclaim_dead_owners"]
    fn allocator_process() {
        let mut path = String::new();
        std::io::stdin().read_line(&mut path).unwrap();
        let dir = Path::new(path.trim());
        let reservations: Vec<_> = (0..8).map(|_| reserve(dir, FIRST + 32)).collect();
        for r in &reservations {
            println!("{}", r.tid);
        }
        std::io::stdout().flush().unwrap();
        let mut done = String::new();
        std::io::stdin().read_line(&mut done).unwrap();
    }

    #[test]
    fn task_ids_are_shared_across_processes_and_reclaim_dead_owners() {
        let dir = Dir::new();
        let mut children = Vec::new();
        // Start all allocators before reading: their independent file
        // descriptions must serialize allocation of the same namespace.
        for _ in 0..4 {
            let mut child = Child(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--ignored",
                        "--exact",
                        "sys::pidns::tids::tests::allocator_process",
                        "--nocapture",
                    ])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .spawn()
                    .unwrap(),
            );
            writeln!(child.0.stdin.as_mut().unwrap(), "{}", dir.0.display()).unwrap();
            children.push(child);
        }
        let mut ids = HashSet::new();
        for child in &mut children {
            let mut reader = BufReader::new(child.0.stdout.take().unwrap());
            let mut count = 0;
            while count < 8 {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if let Ok(tid) = line.trim().parse::<i32>() {
                    assert!(ids.insert(tid));
                    let owner = Table::open(&dir.0, false).unwrap().read(tid);
                    assert_eq!(owner.host_pid, child.0.id() as i32);
                    count += 1;
                }
            }
        }
        assert_eq!(ids.len(), 32);
        assert_eq!(
            Table::open(&dir.0, true)
                .unwrap()
                .allocate(me(), FIRST + 32)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::EAGAIN)
        );
        for child in &mut children {
            child.0.kill().unwrap();
            child.0.wait().unwrap();
        }
        let reclaimed: Vec<_> = (0..32).map(|_| reserve(&dir.0, FIRST + 32)).collect();
        assert_eq!(reclaimed.iter().map(|r| r.tid).collect::<HashSet<_>>(), ids);
        drop(reclaimed);
    }
}
