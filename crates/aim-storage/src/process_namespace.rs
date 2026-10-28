//! Authenticated guest process identities shared by native init and syscall owners (#1158).
//! A recorded host process is valid only for its exact kernel start time.
use std::{
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub host_pid: i32,
    pub start_seconds: u64,
    pub start_microseconds: u64,
}
impl ProcessIdentity {
    pub fn running(pid: i32) -> io::Result<Self> {
        if pid <= 0 {
            return Err(io::Error::from_raw_os_error(libc::ESRCH));
        }
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of_val(&info) as i32;
        if unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                1,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        } != size
        {
            return Err(io::Error::from_raw_os_error(libc::ESRCH));
        }
        Ok(Self {
            host_pid: pid,
            start_seconds: info.pbi_start_tvsec,
            start_microseconds: info.pbi_start_tvusec,
        })
    }
    /// Preserve metadata errors for owners that must prove process termination.
    pub fn running_strict(pid: i32) -> io::Result<Self> {
        Self::metadata_strict(pid).map(|info| Self {
            host_pid: pid,
            start_seconds: info.pbi_start_tvsec,
            start_microseconds: info.pbi_start_tvusec,
        })
    }
    pub fn metadata_strict(pid: i32) -> io::Result<libc::proc_bsdinfo> {
        if pid <= 0 {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of_val(&info) as i32;
        unsafe {
            *libc::__error() = 0;
        }
        let returned = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                1,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
        if returned != size {
            return Err(io::Error::from_raw_os_error(if errno != 0 {
                errno
            } else {
                libc::EIO
            }));
        }
        Ok(info)
    }
    /// A zombie has exited: PROC_PIDTBSDINFO with arg 1 still reports it until it is reaped.
    pub fn is_live(self) -> bool {
        Self::metadata_strict(self.host_pid).is_ok_and(|info| {
            info.pbi_status != libc::SZOMB
                && Self {
                    host_pid: self.host_pid,
                    start_seconds: info.pbi_start_tvsec,
                    start_microseconds: info.pbi_start_tvusec,
                } == self
        })
    }
}
#[derive(Clone, Debug)]
pub struct InitRegistration {
    pub process: ProcessIdentity,
    pub mount_namespace: String,
}
impl InitRegistration {
    fn text(&self) -> String {
        format!(
            "AIMNS1\t{}\t{}\t{}\t{}\n",
            self.process.host_pid,
            self.process.start_seconds,
            self.process.start_microseconds,
            self.mount_namespace
        )
    }
    pub fn register(
        table: &Path,
        process: ProcessIdentity,
        mount_namespace: &str,
    ) -> io::Result<Self> {
        if !process.is_live()
            || mount_namespace.is_empty()
            || mount_namespace.contains(['\t', '\n', '/', '\0'])
        {
            return Err(io::Error::from_raw_os_error(libc::EINVAL));
        }
        fs::create_dir_all(table)?;
        let _lock = entry_lock(table)?;
        let value = Self {
            process,
            mount_namespace: mount_namespace.into(),
        };
        atomic_write(&table.join("namespace-init"), value.text().as_bytes())?;
        Ok(value)
    }
    pub fn read(table: &Path) -> io::Result<Self> {
        let value = Self::read_record(table)?;
        if ProcessIdentity::running_strict(value.process.host_pid)? != value.process {
            return Err(io::Error::from_raw_os_error(libc::ESRCH));
        }
        Ok(value)
    }
    fn read_record(table: &Path) -> io::Result<Self> {
        let text = fs::read_to_string(table.join("namespace-init"))?;
        let fields: Vec<_> = text.trim_end_matches('\n').split('\t').collect();
        if fields.len() != 5 || fields[0] != "AIMNS1" {
            return Err(io::Error::from_raw_os_error(libc::EPROTO));
        }
        let number = |at: usize| {
            fields[at]
                .parse::<u64>()
                .map_err(|_| io::Error::from_raw_os_error(libc::EPROTO))
        };
        let host_pid =
            i32::try_from(number(1)?).map_err(|_| io::Error::from_raw_os_error(libc::EPROTO))?;
        let value = Self {
            process: ProcessIdentity {
                host_pid,
                start_seconds: number(2)?,
                start_microseconds: number(3)?,
            },
            mount_namespace: fields[4].into(),
        };
        Ok(value)
    }
    pub fn activate_pid_mapping(&self, table: &Path) -> io::Result<()> {
        let _lock = entry_lock(table)?;
        let current = Self::read(table)?;
        if current.process != self.process || current.mount_namespace != self.mount_namespace {
            return Err(io::Error::from_raw_os_error(libc::ESRCH));
        }
        init_caught_signals(table, self.process)?;
        atomic_write(&table.join("namespace-pid-ready"), b"AIMNS-PID-READY1\n")
    }
    pub fn remove(&self, table: &Path) -> io::Result<()> {
        let _lock = entry_lock(table)?;
        let current = match Self::read_record(table) {
            Ok(current) => current,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        if current.process != self.process || current.mount_namespace != self.mount_namespace {
            return Err(io::Error::from_raw_os_error(libc::ESTALE));
        }
        match fs::remove_file(table.join("namespace-pid-ready")) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::remove_file(table.join("namespace-init"))
    }
}
#[derive(Clone, Debug)]
pub struct InitNamespaceEntry {
    pub actor: ProcessIdentity,
    pub source: ProcessIdentity,
    pub namespace: String,
}
impl InitNamespaceEntry {
    /// Capture the real target namespace before it can switch; callers never
    /// supply a namespace name. The resulting entry is bound to both births.
    pub fn capture(
        table: &Path,
        actor: ProcessIdentity,
    ) -> io::Result<crate::mount_namespace::Namespace> {
        let _lock = entry_lock(table)?;
        let init = InitRegistration::read(table)?;
        let namespace = mount_namespace_of(table, init.process)?;
        let runtime = table
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EPROTO))?;
        let owner = crate::mount_namespace::Namespace::open(runtime, &namespace)?;
        owner.read()?;
        let current = InitRegistration::read(table)?;
        if !actor.is_live()
            || current.process != init.process
            || current.mount_namespace != init.mount_namespace
        {
            return Err(io::Error::from_raw_os_error(libc::ESRCH));
        }
        register_mount_namespace(table, actor, owner.id())?;
        atomic_write(
            &table.join(format!("{}.init-namespace-entry", actor.host_pid)),
            format!(
                "AIMINITENTRY1\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                actor.host_pid,
                actor.start_seconds,
                actor.start_microseconds,
                init.process.host_pid,
                init.process.start_seconds,
                init.process.start_microseconds,
                owner.id()
            )
            .as_bytes(),
        )?;
        Ok(owner)
    }
    pub fn read(table: &Path, actor: ProcessIdentity, source: ProcessIdentity) -> io::Result<Self> {
        let entry = Self::read_record(table, actor.host_pid)?;
        if entry.actor != actor || entry.source != source {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
        if !actor.is_live() || !source.is_live() {
            return Err(io::Error::from_raw_os_error(libc::ESRCH));
        }
        Ok(entry)
    }
    pub fn retire_dead(table: &Path, source: ProcessIdentity) -> io::Result<()> {
        for file in fs::read_dir(table)? {
            let name = file?.file_name();
            let Some(pid) = name
                .to_str()
                .and_then(|name| name.strip_suffix(".init-namespace-entry"))
                .and_then(|pid| pid.parse::<i32>().ok())
            else {
                continue;
            };
            if pid <= 0 {
                continue;
            }
            let _lock = entry_lock(table)?;
            let entry = match Self::read_record(table, pid) {
                Ok(entry) => entry,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            if entry.actor.host_pid != pid
                || entry.source != source
                || ProcessIdentity::running(pid).is_ok()
            {
                continue;
            }
            // A failed identity query alone does not establish process exit.
            if unsafe { libc::kill(pid, 0) } == 0 {
                continue;
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
            fs::remove_file(table.join(format!("{pid}.init-namespace-entry")))?;
        }
        Ok(())
    }
    fn read_record(table: &Path, pid: i32) -> io::Result<Self> {
        use std::{io::Read, os::unix::fs::OpenOptionsExt};
        let mut file = crate::private_fd::PrivateFile::allocate(|| {
            fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(table.join(format!("{pid}.init-namespace-entry")))
        })?;
        use std::os::fd::AsRawFd;
        let mut stat: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(file.as_raw_fd(), &mut stat) } < 0 {
            return Err(io::Error::last_os_error());
        }
        if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err(io::Error::from_raw_os_error(libc::EPROTO));
        }
        let mut text = String::new();
        file.by_ref().take(4097).read_to_string(&mut text)?;
        if text.len() > 4096 {
            return Err(io::Error::from_raw_os_error(libc::EPROTO));
        }
        let fields = text.trim_end_matches('\n').split('\t').collect::<Vec<_>>();
        if fields.len() != 8
            || fields[0] != "AIMINITENTRY1"
            || fields[7].is_empty()
            || matches!(fields[7], "." | "..")
            || fields[7].contains(['/', '\0', '\n', '\t'])
        {
            return Err(io::Error::from_raw_os_error(libc::EPROTO));
        }
        let identity = |at: usize| -> io::Result<ProcessIdentity> {
            Ok(ProcessIdentity {
                host_pid: fields[at]
                    .parse()
                    .map_err(|_| io::Error::from_raw_os_error(libc::EPROTO))?,
                start_seconds: fields[at + 1]
                    .parse()
                    .map_err(|_| io::Error::from_raw_os_error(libc::EPROTO))?,
                start_microseconds: fields[at + 2]
                    .parse()
                    .map_err(|_| io::Error::from_raw_os_error(libc::EPROTO))?,
            })
        };
        let entry = Self {
            actor: identity(1)?,
            source: identity(4)?,
            namespace: fields[7].into(),
        };
        Ok(entry)
    }
}
fn entry_lock(table: &Path) -> io::Result<crate::private_fd::PrivateFile> {
    record_lock(&table.join("namespace-entry-lock"))
}
fn record_lock(path: &Path) -> io::Result<crate::private_fd::PrivateFile> {
    use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
    let file = crate::private_fd::PrivateFile::allocate(|| {
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
    })?;
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(file);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}
pub fn register_mount_namespace(
    table: &Path,
    process: ProcessIdentity,
    namespace: &str,
) -> io::Result<()> {
    let _lock = record_lock(&table.join("mount-namespace-lock"))?;
    if !process.is_live() || namespace.is_empty() || namespace.contains(['/', '\t', '\n', '\0']) {
        return Err(io::Error::from_raw_os_error(libc::EINVAL));
    }
    atomic_write(
        &table.join(format!("{}.mount-namespace", process.host_pid)),
        format!(
            "AIMPROCNS1\t{}\t{}\t{}\t{}\n",
            process.host_pid, process.start_seconds, process.start_microseconds, namespace
        )
        .as_bytes(),
    )
}
/// Retire only this incarnation's receipt, even after waitpid has reaped it.
pub fn retire_mount_namespace(table: &Path, process: ProcessIdentity) -> io::Result<()> {
    use std::{io::Read, os::unix::fs::OpenOptionsExt};
    let _lock = record_lock(&table.join("mount-namespace-lock"))?;
    let path = table.join(format!("{}.mount-namespace", process.host_pid));
    let file = match fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut text = String::new();
    file.take(4097).read_to_string(&mut text)?;
    let fields: Vec<_> = text.trim_end_matches('\n').split('\t').collect();
    if text.len() > 4096
        || fields.len() != 5
        || fields[0] != "AIMPROCNS1"
        || fields[4].is_empty()
        || fields[4].contains(['/', '\0', '\n'])
    {
        return Err(io::Error::from_raw_os_error(libc::EPROTO));
    }
    if fields[1].parse() != Ok(process.host_pid)
        || fields[2].parse() != Ok(process.start_seconds)
        || fields[3].parse() != Ok(process.start_microseconds)
    {
        return Err(io::Error::from_raw_os_error(libc::ESTALE));
    }
    fs::remove_file(path)
}
pub fn mount_namespace_of(table: &Path, process: ProcessIdentity) -> io::Result<String> {
    let text = fs::read_to_string(table.join(format!("{}.mount-namespace", process.host_pid)))?;
    let fields: Vec<_> = text.trim_end_matches('\n').split('\t').collect();
    if fields.len() != 5
        || fields[0] != "AIMPROCNS1"
        || fields[1].parse() != Ok(process.host_pid)
        || fields[2].parse() != Ok(process.start_seconds)
        || fields[3].parse() != Ok(process.start_microseconds)
        || !process.is_live()
    {
        return Err(io::Error::from_raw_os_error(libc::ESRCH));
    }
    Ok(fields[4].into())
}

pub fn register_init_signals(
    table: &Path,
    process: ProcessIdentity,
    caught: u64,
) -> io::Result<()> {
    if !process.is_live() {
        return Err(io::Error::from_raw_os_error(libc::ESRCH));
    }
    atomic_write(
        &table.join("namespace-init-signals"),
        format!(
            "AIMINIT-SIGNALS1\t{}\t{}\t{}\t{caught:x}\n",
            process.host_pid, process.start_seconds, process.start_microseconds
        )
        .as_bytes(),
    )
}
pub fn init_caught_signals(table: &Path, process: ProcessIdentity) -> io::Result<u64> {
    let text = fs::read_to_string(table.join("namespace-init-signals"))?;
    let fields = text.trim_end_matches('\n').split('\t').collect::<Vec<_>>();
    if fields.len() != 5
        || fields[0] != "AIMINIT-SIGNALS1"
        || fields[1].parse() != Ok(process.host_pid)
        || fields[2].parse() != Ok(process.start_seconds)
        || fields[3].parse() != Ok(process.start_microseconds)
        || !process.is_live()
    {
        return Err(io::Error::from_raw_os_error(libc::ESRCH));
    }
    u64::from_str_radix(fields[4], 16).map_err(|_| io::Error::from_raw_os_error(libc::EPROTO))
}

/// Readers see the old record or the new one (the rename). The table lives
/// in the boot's runtime directory, which no boot outlives, so nothing is
/// flushed to the disk.
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::io::Write;
    // The time, and a count of this process's own: two writers of one
    // process within the clock's resolution (a microsecond) get two
    // names (#2117).
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let count = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temporary: PathBuf =
        path.with_extension(format!("{}-{nonce}-{count}.tmp", std::process::id()));
    // A name that exists is another writer's, not this one's to remove.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            if self.0.try_wait().ok().flatten().is_none() {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }
    #[test]
    fn mount_retirement_preserves_foreign_symlinks_and_is_idempotent() {
        let table = crate::scratch_dir("aim-mount-retire");
        let process = ProcessIdentity::running_strict(std::process::id() as i32).unwrap();
        let receipt = table.join(format!("{}.mount-namespace", process.host_pid));
        register_mount_namespace(&table, process, "owned").unwrap();
        let target = table.join("foreign");
        fs::rename(&receipt, &target).unwrap();
        std::os::unix::fs::symlink(&target, &receipt).unwrap();
        assert_eq!(
            retire_mount_namespace(&table, process)
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ELOOP)
        );
        assert!(
            fs::symlink_metadata(&receipt)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_file(&receipt).unwrap();
        fs::rename(&target, &receipt).unwrap();
        retire_mount_namespace(&table, process).unwrap();
        retire_mount_namespace(&table, process).unwrap();
        assert!(!receipt.exists());
        fs::remove_dir_all(table).unwrap();
    }
    #[test]
    fn actual_process_start_time_rejects_reused_or_dead_init_identity() {
        let root = crate::scratch_dir("aim-pid-owner");
        let mut child = Child(
            Command::new("sleep")
                .arg("30")
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let process = ProcessIdentity::running(child.0.id() as i32).unwrap();
        let registration =
            InitRegistration::register(&root, process, "namespace-original").unwrap();
        assert_eq!(InitRegistration::read(&root).unwrap().process, process);
        let stale = ProcessIdentity {
            start_microseconds: process.start_microseconds.wrapping_add(1),
            ..process
        };
        assert!(!stale.is_live());
        assert!(InitRegistration::register(&root, stale, "namespace-original").is_err());
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        assert!(InitRegistration::read(&root).is_err());
        registration.remove(&root).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn exited_unreaped_process_is_not_live() {
        let mut child = Child(Command::new("true").spawn().unwrap());
        let process = ProcessIdentity::running(child.0.id() as i32).unwrap();
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // WNOWAIT leaves the exited child a zombie.
        assert_eq!(
            unsafe {
                libc::waitid(
                    libc::P_PID,
                    process.host_pid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOWAIT,
                )
            },
            0
        );
        assert!(ProcessIdentity::running(process.host_pid).is_ok());
        assert!(!process.is_live());
        child.0.wait().unwrap();
    }
    #[test]
    fn retirement_closes_namespace_admission_while_owner_process_is_live() {
        let root = crate::scratch_dir("aim-retire-admission");
        let table = root.join("identity/by-pid");
        let image = root.join("image");
        fs::create_dir_all(&image).unwrap();
        let process = ProcessIdentity::running_strict(std::process::id() as i32).unwrap();
        let namespace = crate::mount_namespace::Namespace::open(&root, "owned").unwrap();
        namespace
            .initialize(&format!("root\t/\t{}\n", image.display()))
            .unwrap();
        let registration = InitRegistration::register(&table, process, namespace.id()).unwrap();
        register_mount_namespace(&table, process, namespace.id()).unwrap();
        register_init_signals(&table, process, 0).unwrap();
        registration.activate_pid_mapping(&table).unwrap();
        assert_eq!(
            InitNamespaceEntry::capture(&table, process).unwrap().id(),
            namespace.id()
        );
        registration.remove(&table).unwrap();
        assert!(process.is_live());
        assert!(namespace.read().is_ok());
        assert!(!table.join("namespace-init").exists());
        assert!(!table.join("namespace-pid-ready").exists());
        assert_eq!(
            InitNamespaceEntry::capture(&table, process)
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::NotFound
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn retirement_preserves_new_namespace_and_foreign_process_records() {
        let root = crate::scratch_dir("aim-retire-cas");
        let process = ProcessIdentity::running_strict(std::process::id() as i32).unwrap();
        let old = InitRegistration::register(&root, process, "old").unwrap();
        let new = InitRegistration::register(&root, process, "new").unwrap();
        register_init_signals(&root, process, 0).unwrap();
        new.activate_pid_mapping(&root).unwrap();
        let before = fs::read(root.join("namespace-init")).unwrap();
        let ready = fs::read(root.join("namespace-pid-ready")).unwrap();
        assert_eq!(
            old.remove(&root).unwrap_err().raw_os_error(),
            Some(libc::ESTALE)
        );
        assert_eq!(fs::read(root.join("namespace-init")).unwrap(), before);
        assert_eq!(fs::read(root.join("namespace-pid-ready")).unwrap(), ready);
        let child = Child(
            Command::new("sleep")
                .arg("30")
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let foreign = ProcessIdentity::running_strict(child.0.id() as i32).unwrap();
        InitRegistration::register(&root, foreign, "foreign").unwrap();
        let before = fs::read(root.join("namespace-init")).unwrap();
        assert_eq!(
            new.remove(&root).unwrap_err().raw_os_error(),
            Some(libc::ESTALE)
        );
        assert_eq!(fs::read(root.join("namespace-init")).unwrap(), before);
        assert_eq!(fs::read(root.join("namespace-pid-ready")).unwrap(), ready);
        drop(child);
        fs::remove_dir_all(root).unwrap();
    }
}

pub fn signal_to_host(sig: i32) -> i32 {
    match sig {
        1 => libc::SIGHUP,
        2 => libc::SIGINT,
        3 => libc::SIGQUIT,
        4 => libc::SIGILL,
        5 => libc::SIGTRAP,
        6 => libc::SIGABRT,
        7 => libc::SIGBUS,
        8 => libc::SIGFPE,
        9 => libc::SIGKILL,
        10 => libc::SIGUSR1,
        11 => libc::SIGSEGV,
        12 => libc::SIGUSR2,
        13 => libc::SIGPIPE,
        14 => libc::SIGALRM,
        15 => libc::SIGTERM,
        17 => libc::SIGCHLD,
        18 => libc::SIGCONT,
        19 => libc::SIGSTOP,
        20 => libc::SIGTSTP,
        21 => libc::SIGTTIN,
        22 => libc::SIGTTOU,
        23 => libc::SIGURG,
        24 => libc::SIGXCPU,
        25 => libc::SIGXFSZ,
        26 => libc::SIGVTALRM,
        27 => libc::SIGPROF,
        28 => libc::SIGWINCH,
        29 => libc::SIGIO,
        31 => libc::SIGSYS,
        _ => 0,
    }
}

pub fn signal_from_host(host: i32) -> i32 {
    (1..32)
        .find(|&signal| signal_to_host(signal) == host)
        .unwrap_or(0)
}
