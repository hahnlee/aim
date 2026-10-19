//! Own the pinned CTS classic launcher's JVM and foreground adb (#1323).
use aim_storage::process_namespace::ProcessIdentity;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufRead, BufReader, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    path::PathBuf,
    process::{Child, Command},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn stop(_: i32) {
    STOP.store(true, Ordering::Relaxed);
}
fn invalid() -> io::Error {
    io::Error::from_raw_os_error(libc::EPROTO)
}
fn identity(child: &Child) -> io::Result<ProcessIdentity> {
    ProcessIdentity::running_strict(i32::try_from(child.id()).map_err(|_| invalid())?)
}
fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c < ' ' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
fn live_result(value: io::Result<ProcessIdentity>, expected: ProcessIdentity) -> io::Result<bool> {
    match value {
        Ok(actual) => Ok(actual == expected),
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(false),
        Err(error) => Err(error),
    }
}
fn live(process: ProcessIdentity) -> io::Result<bool> {
    live_result(ProcessIdentity::running_strict(process.host_pid), process)
}
fn children(parent: ProcessIdentity, deadline: Instant) -> io::Result<Vec<ProcessIdentity>> {
    if !live(parent)? {
        return Ok(Vec::new());
    }
    let mut pids = vec![0i32; 4096];
    unsafe {
        *libc::__error() = 0;
    }
    let count = unsafe {
        libc::proc_listchildpids(
            parent.host_pid,
            pids.as_mut_ptr().cast(),
            (pids.len() * 4) as i32,
        )
    };
    let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
    if count < 0 || (count == 0 && errno != 0) {
        if errno == libc::ESRCH && !live(parent)? {
            return Ok(Vec::new());
        }
        return Err(io::Error::from_raw_os_error(if errno != 0 {
            errno
        } else {
            libc::EIO
        }));
    }
    if count as usize >= pids.len() {
        return Err(io::Error::from_raw_os_error(libc::E2BIG));
    }
    if !live(parent)? {
        return Ok(Vec::new());
    }
    let mut rows = Vec::new();
    for pid in pids.into_iter().take(count as usize).filter(|p| *p > 0) {
        if Instant::now() >= deadline {
            return Err(io::Error::from_raw_os_error(libc::ETIMEDOUT));
        }
        let info = match ProcessIdentity::metadata_strict(pid) {
            Ok(info) => info,
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => continue,
            Err(error) => return Err(error),
        };
        let child = ProcessIdentity {
            host_pid: pid,
            start_seconds: info.pbi_start_tvsec,
            start_microseconds: info.pbi_start_tvusec,
        };
        if !live(parent)? {
            return Ok(rows);
        }
        if !live(child)? {
            continue;
        }
        if info.pbi_ppid as i32 != parent.host_pid {
            return Err(invalid());
        }
        rows.push(child);
    }
    Ok(rows)
}
fn discover(owned: &mut BTreeMap<i32, ProcessIdentity>, deadline: Instant) -> io::Result<()> {
    for _ in 0..8 {
        let before = owned.len();
        for parent in owned.values().copied().collect::<Vec<_>>() {
            if Instant::now() >= deadline {
                return Err(io::Error::from_raw_os_error(libc::ETIMEDOUT));
            }
            for child in children(parent, deadline)? {
                if let Some(previous) = owned.get(&child.host_pid) {
                    if *previous != child {
                        return Err(invalid());
                    }
                }
                if owned.len() >= 4096 && !owned.contains_key(&child.host_pid) {
                    return Err(io::Error::from_raw_os_error(libc::E2BIG));
                }
                owned.insert(child.host_pid, child);
            }
        }
        if owned.len() == before {
            return Ok(());
        }
    }
    Err(io::Error::from_raw_os_error(libc::E2BIG))
}
fn signal(process: ProcessIdentity, value: i32) -> io::Result<()> {
    if !live(process)? {
        return Ok(());
    }
    if process.host_pid <= 0 {
        return Err(invalid());
    }
    if unsafe { libc::kill(process.host_pid, value) } < 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error);
        }
    }
    Ok(())
}
fn cleanup(owned: &mut BTreeMap<i32, ProcessIdentity>, children: &mut [Child]) -> io::Result<()> {
    let started = Instant::now();
    let term_until = started + Duration::from_secs(5);
    let deadline = started + Duration::from_secs(7);
    let mut errors = BTreeSet::new();
    if let Err(error) = discover(owned, started + Duration::from_millis(100)) {
        errors.insert(format!("initial census: {error}"));
    }
    for process in owned.values().rev().copied() {
        if Instant::now() >= term_until {
            errors.insert("TERM snapshot deadline".into());
            break;
        }
        if let Err(error) = signal(process, libc::SIGTERM) {
            errors.insert(format!("TERM {}: {error}", process.host_pid));
        }
    }
    loop {
        let until = if Instant::now() < term_until {
            term_until
        } else {
            deadline
        };
        for child in children.iter_mut() {
            if let Err(error) = child.try_wait() {
                errors.insert(format!("reap {}: {error}", child.id()));
            }
        }
        let snapshot_deadline = until.min(Instant::now() + Duration::from_millis(100));
        if let Err(error) = discover(owned, snapshot_deadline) {
            errors.insert(format!("cleanup census: {error}"));
        }
        let mut active = false;
        for process in owned.values().copied() {
            if Instant::now() >= until {
                active = true;
                errors.insert("metadata snapshot deadline".into());
                break;
            }
            match live(process) {
                Ok(running) => active |= running,
                Err(error) => {
                    active = true;
                    errors.insert(format!("metadata {}: {error}", process.host_pid));
                }
            }
        }
        if !active {
            break;
        }
        if Instant::now() >= deadline {
            errors.insert("cleanup deadline reached with live or unknown process".into());
            break;
        }
        if Instant::now() >= term_until {
            for process in owned.values().rev().copied() {
                if Instant::now() >= deadline {
                    errors.insert("KILL snapshot deadline".into());
                    break;
                }
                if let Err(error) = signal(process, libc::SIGKILL) {
                    errors.insert(format!("KILL {}: {error}", process.host_pid));
                }
            }
        }
        thread::sleep(Duration::from_millis(10));
    }
    // Each snapshot/actor count/depth and the whole cleanup have fixed bounds.
    if errors.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(
            errors.into_iter().collect::<Vec<_>>().join("; "),
        ))
    }
}
fn admit(fd: i32) -> io::Result<()> {
    if fd != 198 {
        return Err(invalid());
    }
    let process = ProcessIdentity::running_strict(unsafe { libc::getppid() })?;
    let mut stream = unsafe { UnixStream::from_raw_fd(fd) };
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    writeln!(
        stream,
        "{} {} {}",
        process.host_pid, process.start_seconds, process.start_microseconds
    )?;
    let mut response = String::new();
    BufReader::new(&mut stream).read_line(&mut response)?;
    if response != "ADMITTED\n" || !live(process)? {
        return Err(invalid());
    }
    Ok(())
}
fn executable(process: ProcessIdentity) -> io::Result<PathBuf> {
    if !live(process)? {
        return Err(io::Error::from_raw_os_error(libc::ESRCH));
    }
    let mut path = vec![0u8; 4096];
    unsafe {
        *libc::__error() = 0;
    }
    let count = unsafe {
        libc::proc_pidpath(
            process.host_pid,
            path.as_mut_ptr().cast(),
            path.len() as u32,
        )
    };
    let errno = io::Error::last_os_error().raw_os_error().unwrap_or(0);
    if count <= 0 {
        return Err(io::Error::from_raw_os_error(if errno != 0 {
            errno
        } else {
            libc::EIO
        }));
    }
    if !live(process)? {
        return Err(io::Error::from_raw_os_error(libc::ESRCH));
    }
    path.truncate(count as usize);
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(std::ffi::OsString::from_vec(path)).canonicalize()?)
}
fn read_owner_text(path: &std::path::Path) -> io::Result<String> {
    use std::{io::Read, os::unix::fs::OpenOptionsExt};
    let mut file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid());
    }
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0 {
            break;
        }
        let error = io::Error::last_os_error();
        if !matches!(
            error.raw_os_error(),
            Some(libc::EWOULDBLOCK) | Some(libc::EINTR)
        ) {
            return Err(error);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::from_raw_os_error(libc::ETIMEDOUT));
        }
        thread::sleep(Duration::from_millis(5));
    }
    let mut text = String::new();
    Read::by_ref(&mut file)
        .take(16385)
        .read_to_string(&mut text)?;
    if text.len() > 16384 {
        return Err(invalid());
    }
    Ok(text)
}
fn authenticated_init_pid(table: &std::path::Path) -> io::Result<i32> {
    use aim_storage::process_namespace::InitRegistration;
    let table = table.canonicalize()?;
    let registration = read_owner_text(&table.join("namespace-init"))?;
    let fields = registration
        .trim_end_matches('\n')
        .split('\t')
        .collect::<Vec<_>>();
    if fields.len() != 5 || fields[0] != "AIMNS1" {
        return Err(invalid());
    }
    let process = ProcessIdentity {
        host_pid: fields[1].parse().map_err(|_| invalid())?,
        start_seconds: fields[2].parse().map_err(|_| invalid())?,
        start_microseconds: fields[3].parse().map_err(|_| invalid())?,
    };
    if process.host_pid <= 0 || process.start_seconds == 0 || process.start_microseconds >= 1000000
    {
        return Err(invalid());
    }
    let init = InitRegistration {
        process,
        mount_namespace: fields[4].into(),
    };
    if init.mount_namespace
        != format!(
            "init-{}-{}-{}",
            process.host_pid, process.start_seconds, process.start_microseconds
        )
    {
        return Err(invalid());
    }
    if ProcessIdentity::running_strict(process.host_pid)? != process {
        return Err(io::Error::from_raw_os_error(libc::EPERM));
    }
    let expected = table
        .parent()
        .ok_or_else(invalid)?
        .join("init.identity")
        .canonicalize()?;
    if table.join(process.host_pid.to_string()).canonicalize()? != expected {
        return Err(io::Error::from_raw_os_error(libc::EPERM));
    }
    let identity = read_owner_text(&expected)?;
    let lines = identity.lines().collect::<Vec<_>>();
    if lines.first() != Some(&"# aim-guest-init identity v1") {
        return Err(invalid());
    }
    for (key, value) in [("service", "init"), ("uid", "0"), ("gid", "0")] {
        let values = lines
            .iter()
            .filter_map(|line| line.strip_prefix(&format!("{key}\t")))
            .collect::<Vec<_>>();
        if values != [value] {
            return Err(io::Error::from_raw_os_error(libc::EPERM));
        }
    }
    if read_owner_text(&table.join("namespace-init"))? != registration
        || ProcessIdentity::running_strict(process.host_pid)? != process
    {
        return Err(io::Error::from_raw_os_error(libc::EPERM));
    }
    Ok(process.host_pid)
}

fn run(args: Vec<String>) -> io::Result<i32> {
    if args.len() == 2 && args[0] == "--init-pid" {
        println!(
            "{}",
            authenticated_init_pid(std::path::Path::new(&args[1]))?
        );
        return Ok(0);
    }
    if args.len() == 2 && args[0] == "--admit" {
        admit(args[1].parse().map_err(|_| invalid())?)?;
        return Ok(0);
    }
    if args.len() < 10
        || args[0] != "--receipt"
        || args[2] != "--adb"
        || args[4] != "--server-port"
        || args[6] != "--java"
        || args[8] != "--"
    {
        return Err(invalid());
    }
    let java = PathBuf::from(&args[7]).canonicalize()?;
    let receipt = PathBuf::from(&args[1]);
    if receipt.exists() {
        return Err(io::Error::from_raw_os_error(libc::EEXIST));
    }
    let port = args[5].parse::<u16>().map_err(|_| invalid())?;
    use std::os::unix::fs::OpenOptionsExt;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(std::env::temp_dir().join(format!("aim-cts-owner-{port}.lock")))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // Bind failure preserves any foreign server and fails before spawning.
    let port_guard = std::net::TcpListener::bind(("127.0.0.1", port))?;
    let own_exe = std::env::current_exe()?.canonicalize()?;
    let helper_sha = format!("{:x}", Sha256::digest(fs::read(&own_exe)?));
    let self_pid = unsafe { libc::getpid() };
    unsafe {
        libc::signal(libc::SIGTERM, stop as *const () as usize);
        libc::signal(libc::SIGINT, stop as *const () as usize);
    }
    drop(port_guard);
    let adb = Command::new(&args[3])
        .args(["-L", &format!("tcp:localhost:{port}"), "server", "nodaemon"])
        .spawn()?;
    let mut spawned = vec![adb];
    let mut owned = BTreeMap::new();
    let mut jvm_identity = None;
    let result = (|| -> io::Result<i32> {
        let adb_identity = identity(&spawned[0])?;
        owned.insert(adb_identity.host_pid, adb_identity);
        let (mut controller, admission) = UnixStream::pair()?;
        controller.set_read_timeout(Some(Duration::from_secs(10)))?;
        let fd = admission.as_raw_fd();
        let mut wrapper = Command::new(&args[9]);
        wrapper
            .args([
                "--owner-admission-fd",
                "198",
                "--cts-owner",
                own_exe.to_str().ok_or_else(invalid)?,
            ])
            .args(&args[10..]);
        unsafe {
            wrapper.pre_exec(move || {
                if libc::dup2(fd, 198) < 0 || libc::fcntl(198, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        spawned.push(wrapper.spawn()?);
        drop(admission);
        let wrapper_identity = identity(&spawned[1])?;
        owned.insert(wrapper_identity.host_pid, wrapper_identity);
        let mut request = String::new();
        BufReader::new(&mut controller).read_line(&mut request)?;
        if request
            != format!(
                "{} {} {}\n",
                wrapper_identity.host_pid,
                wrapper_identity.start_seconds,
                wrapper_identity.start_microseconds
            )
            || !live(wrapper_identity)?
        {
            return Err(invalid());
        }
        writeln!(controller, "ADMITTED")?;
        drop(controller);
        loop {
            discover(&mut owned, Instant::now() + Duration::from_millis(100))?;
            match executable(wrapper_identity) {
                Ok(path) if path == java => jvm_identity = Some(wrapper_identity),
                Ok(_) => {}
                Err(error) if error.raw_os_error() == Some(libc::ESRCH) => {}
                Err(error) => return Err(error),
            }
            if STOP.load(Ordering::Relaxed) {
                return Ok(143);
            }
            if let Some(status) = spawned[1].try_wait()? {
                if jvm_identity.is_none() {
                    return Err(invalid());
                }
                return Ok(status.code().unwrap_or(1));
            }
            if spawned[0].try_wait()?.is_some() {
                return Err(io::Error::from_raw_os_error(libc::EPIPE));
            }
            thread::sleep(Duration::from_millis(10));
        }
    })();
    let cleaned = cleanup(&mut owned, &mut spawned);
    let mut uncertain = result
        .as_ref()
        .err()
        .map(ToString::to_string)
        .into_iter()
        .chain(cleaned.as_ref().err().map(ToString::to_string))
        .collect::<Vec<_>>();
    let jvm = jvm_identity
        .map(|p| {
            format!(
                "{{\"pid\":{},\"start_seconds\":{},\"start_microseconds\":{}}}",
                p.host_pid, p.start_seconds, p.start_microseconds
            )
        })
        .unwrap_or_else(|| "null".into());
    let mut records = Vec::new();
    let mut remaining = Vec::new();
    let final_deadline = Instant::now() + Duration::from_millis(100);
    for p in owned.values() {
        let state = if Instant::now() >= final_deadline {
            Err(io::Error::from_raw_os_error(libc::ETIMEDOUT))
        } else {
            live(*p)
        };
        let terminal = match state {
            Ok(false) => "true",
            Ok(true) => {
                remaining.push(p.host_pid.to_string());
                "false"
            }
            Err(error) => {
                uncertain.push(format!("final metadata {}: {error}", p.host_pid));
                remaining.push(p.host_pid.to_string());
                "null"
            }
        };
        records.push(format!(
            "{{\"pid\":{},\"start_seconds\":{},\"start_microseconds\":{},\"terminal\":{terminal}}}",
            p.host_pid, p.start_seconds, p.start_microseconds
        ));
    }
    let records = records.join(",");
    let remaining = remaining.join(",");
    let argv = args[9..]
        .iter()
        .map(|s| quote(s))
        .collect::<Vec<_>>()
        .join(",");
    let text = format!(
        "{{\"schema\":\"AIM_CTS_OWNER1\",\"supervisor_pid\":{self_pid},\"helper_sha256\":{},\"wrapper_argv\":[{argv}],\"jvm_identity\":{jvm},\"cleanup\":{},\"processes\":[{records}],\"remaining_owned\":[{remaining}],\"uncertainties\":[{}]}}\n",
        quote(&helper_sha),
        quote(
            if cleaned.is_ok() && uncertain.is_empty() && remaining.is_empty() {
                "complete"
            } else {
                "incomplete"
            }
        ),
        uncertain
            .iter()
            .map(|s| quote(s))
            .collect::<Vec<_>>()
            .join(",")
    );
    use std::fs::OpenOptions;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(receipt)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    cleaned?;
    if !uncertain.is_empty() || !remaining.is_empty() {
        return Err(invalid());
    }
    result
}
fn main() {
    match run(std::env::args().skip(1).collect()) {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("CTS owned processes: {error}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(Child);
    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Ok(process) = identity(&self.0) {
                let _ = signal(process, libc::SIGKILL);
            }
            let _ = self.0.wait();
        }
    }
    fn sleeper() -> Fixture {
        Fixture(Command::new("/bin/sleep").arg("30").spawn().unwrap())
    }
    #[test]
    fn cleanup_reaps_actual_owned_child_and_preserves_foreign_child() {
        let mut owned = sleeper();
        let foreign = sleeper();
        let ours = identity(&owned.0).unwrap();
        let theirs = identity(&foreign.0).unwrap();
        let mut registry = BTreeMap::from([(ours.host_pid, ours)]);
        cleanup(&mut registry, std::slice::from_mut(&mut owned.0)).unwrap();
        assert!(!live(ours).unwrap());
        assert!(live(theirs).unwrap());
        assert!(owned.0.try_wait().unwrap().is_some());
    }
    #[test]
    fn stale_birth_never_signals_live_same_pid() {
        let child = sleeper();
        let actual = identity(&child.0).unwrap();
        let stale = ProcessIdentity {
            start_seconds: actual.start_seconds + 1,
            ..actual
        };
        signal(stale, libc::SIGKILL).unwrap();
        assert!(live(actual).unwrap());
    }
    #[test]
    fn kernel_children_are_counted_and_cleaned_by_positive_pid() {
        let mut parent = Command::new("/bin/sh")
            .args(["-c", "/bin/sleep 30 & wait"])
            .spawn()
            .unwrap();
        let parent_birth = identity(&parent).unwrap();
        let mut registry = BTreeMap::from([(parent_birth.host_pid, parent_birth)]);
        let deadline = Instant::now() + Duration::from_secs(2);
        while registry.len() == 1 && Instant::now() < deadline {
            discover(&mut registry, Instant::now() + Duration::from_millis(100)).unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            registry.len() > 1,
            "real child must be admitted by PPID and birth"
        );
        let all = registry.values().copied().collect::<Vec<_>>();
        cleanup(&mut registry, std::slice::from_mut(&mut parent)).unwrap();
        assert!(all.iter().all(|p| !live(*p).unwrap()));
        assert!(all.iter().all(|p| p.host_pid > 0));
    }
    #[test]
    fn unknown_metadata_is_not_process_exit() {
        let child = sleeper();
        let actual = identity(&child.0).unwrap();
        for errno in [libc::EPERM, libc::EIO, libc::EACCES] {
            assert_eq!(
                live_result(Err(io::Error::from_raw_os_error(errno)), actual)
                    .unwrap_err()
                    .raw_os_error(),
                Some(errno)
            );
        }
        assert!(!live_result(Err(io::Error::from_raw_os_error(libc::ESRCH)), actual).unwrap());
        assert!(live(actual).unwrap());
    }
    #[test]
    fn expired_snapshot_is_bounded_and_preserves_live_owned_process() {
        let child = sleeper();
        let actual = identity(&child.0).unwrap();
        let mut registry = BTreeMap::from([(actual.host_pid, actual)]);
        assert_eq!(
            discover(&mut registry, Instant::now())
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ETIMEDOUT)
        );
        assert!(live(actual).unwrap());
        assert_eq!(registry.len(), 1);
    }
    #[test]
    fn init_selection_requires_registered_birth_and_init_kind() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!(
            "aim-cts-init-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let table = root.join("by-pid");
        fs::create_dir_all(&table).unwrap();
        let child = sleeper();
        let process = identity(&child.0).unwrap();
        let record = format!(
            "AIMNS1\t{}\t{}\t{}\tinit-{}-{}-{}\n",
            process.host_pid,
            process.start_seconds,
            process.start_microseconds,
            process.host_pid,
            process.start_seconds,
            process.start_microseconds
        );
        fs::write(table.join("namespace-init"), &record).unwrap();
        let identity = root.join("init.identity");
        fs::write(
            &identity,
            "# aim-guest-init identity v1\nservice\tinit\nuid\t0\ngid\t0\n",
        )
        .unwrap();
        symlink(&identity, table.join(process.host_pid.to_string())).unwrap();
        assert_eq!(authenticated_init_pid(&table).unwrap(), process.host_pid);
        fs::write(
            &identity,
            "# aim-guest-init identity v1\nservice\tcontroller\nuid\t0\ngid\t0\n",
        )
        .unwrap();
        assert!(authenticated_init_pid(&table).is_err());
        fs::write(
            table.join("namespace-init"),
            record.replace(
                &format!("\t{}\t", process.start_seconds),
                &format!("\t{}\t", process.start_seconds + 1),
            ),
        )
        .unwrap();
        assert!(authenticated_init_pid(&table).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn invalid_admission_descriptor_fails_before_touching_foreign_fd() {
        let (mut own, mut peer) = UnixStream::pair().unwrap();
        assert!(admit(-1).is_err());
        own.write_all(b"still open").unwrap();
        let mut line = String::new();
        use std::io::Read;
        peer.take(10).read_to_string(&mut line).unwrap();
        assert_eq!(line, "still open");
    }
}
