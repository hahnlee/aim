//! The resident guest: `aimctl run` starts the display server, the shims'
//! keeper in window mode, and guest-init, and stops them when guest-init
//! ends or it is told to stop; `aimctl start` runs it in the background
//! and `aimctl stop` stops it.

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::process::CommandExt;
use std::os::unix::fs::MetadataExt;
use aim_storage::process_namespace::ProcessIdentity;
use std::path::Path;
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::program;
use crate::state::{self, Files, State};

/// How long the display server may take to open its socket.
const DISPLAY_PATIENCE: Duration = Duration::from_secs(10);
/// How long `start` waits for guest-init to lay out the guest (the first
/// start creates the data image).
const START_PATIENCE: Duration = Duration::from_secs(180);
/// How long guest-init may take to stop its services and detach the data
/// image before it is killed.
const STOP_PATIENCE: Duration = Duration::from_secs(180);
/// How long the display server and the shims' keeper may take to quit
/// after SIGTERM before they are killed.
const CHILD_PATIENCE: Duration = Duration::from_secs(10);
/// How long `stop` waits for `aimctl run`: guest-init's patience, the
/// children's and the detach. After it, the run's process group is killed.
const RUN_PATIENCE: Duration = Duration::from_secs(STOP_PATIENCE.as_secs() + 60);
/// An absolute bound on post-reap sweep and normal DATA detach.
const FINISH_PATIENCE: Duration = Duration::from_secs(60);
/// How long a killed process group may take to go.
const KILL_PATIENCE: Duration = Duration::from_secs(10);

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn stop_requested(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Refuses a data directory in use: by a resident guest, or by another
/// guest-init (`cargo aim boot`) holding its data image.
fn check_free(files: &Files) -> Result<(), String> {
    if let Some(s) = state::resident(files)? {
        return Err(format!(
            "{}: a guest is already running (aimctl pid {})",
            files.data.display(),
            s.pid
        ));
    }
    if state::held(&files.guest_lock()) {
        return Err(format!(
            "{}: its data image is in use by another guest-init",
            files.data.display()
        ));
    }
    Ok(())
}

fn inputs(selected: &crate::inputs::Inputs) -> Result<(), String> {
    for (path, what) in [
        (selected.image.clone(), "the selected image"),
        (aim_paths::angle(), "ANGLE"),
        (aim_paths::moltenvk(), "MoltenVK"),
    ] {
        if !path.exists() {
            return Err(format!(
                "{what} is missing ({}); run `cargo aim build`",
                path.display()
            ));
        }
    }
    Ok(())
}

/// `aimctl start`: `aimctl run` in a session of its own, its output in the
/// log, and back once guest-init has laid out the guest.
pub fn start(files: &Files, windows: bool, selected: &crate::inputs::Inputs) -> Result<ExitCode, String> {
    check_free(files)?;
    inputs(selected)?;
    fs::create_dir_all(files.dir()).map_err(|e| format!("{}: {e}", files.dir().display()))?;
    let log =
        fs::File::create(files.log()).map_err(|e| format!("{}: {e}", files.log().display()))?;
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command.arg("--data").arg(&files.data).arg("run").args(selected.arguments());
    if windows {
        command.arg("--windows");
    }
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        })
    };
    let mut child = command
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log)
        .spawn()
        .map_err(|e| format!("aimctl run: {e}"))?;
    let deadline = Instant::now() + START_PATIENCE;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!(
                "the guest stopped ({status}):\n{}",
                tail(&files.log(), 4096)
            ));
        }
        let state = State::read(&files.state()).ok().flatten();
        if state.is_some_and(|s| s.guest.is_some()) && files.path_map().exists() {
            break;
        }
        if Instant::now() > deadline {
            return Err(format!(
                "the guest did not start within {START_PATIENCE:?}; see {}",
                files.log().display()
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    println!(
        "started (pid {}), booting; `aimctl status` shows when it is ready",
        child.id()
    );
    Ok(ExitCode::SUCCESS)
}

/// The last `bytes` of the file at `path`.
fn tail(path: &Path, bytes: u64) -> String {
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map_or(0, |m| m.len());
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(bytes)));
    let mut out = Vec::new();
    let _ = file.read_to_end(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

/// aim-display's arguments: its socket and capture file in the state
/// directory, and a phone-sized device window or window mode with the
/// shims.
fn display_args(files: &Files, windows: bool) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec!["--socket".into(), files.display().into()];
    if windows {
        args.extend(["--mode", "windows"].map(OsString::from));
        // The shims show the guest's notifications.
        args.push("--apps".into());
        args.push(files.apps().into());
    } else {
        args.extend(["--size", "1080x1920"].map(OsString::from));
    }
    args.push("--capture".into());
    args.push(files.capture().into());
    args
}

/// aim-apps's arguments: a shim for each launcher app in the state
/// directory, following installs; with bundle identifiers of their own
/// unless this is the user's data directory (`own`), whose shims keep
/// theirs (and with them their notification settings).
fn shims_args(files: &Files, own: bool, selected: &crate::inputs::Inputs) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "shims".into(),
        "--image".into(),
        selected.image.clone().into(),
        "--data".into(),
        files.guest_data().into(),
        "--display".into(),
        files.display().into(),
        "--host".into(),
        selected.program("aim-display").into(),
        "--into".into(),
        files.apps().into(),
        "--watch".into(),
    ];
    if !own {
        args.push("--scoped".into());
    }
    args
}

fn guest_init_args(files: &Files, selected: &crate::inputs::Inputs) -> Vec<OsString> {
    let mut args=vec![
        "--image".into(),
        selected.image.clone().into(),
        "--data".into(),
        files.data.clone().into(),
        "--run".into(),
        "--gpu".into(),
        aim_paths::angle().into(),
        "--vulkan".into(),
        aim_paths::moltenvk().into(),
        "--display".into(),
        files.display().into(),
        "--userdata".into(),
        selected.userdata.clone().into(),
        "--linux-run".into(),
        selected.program("linux-run").into(),
        "--quiet".into(),
    ];
    if let Some(path)=&selected.binder_trace {args.extend(["--binder-trace".into(),path.clone().into()]);}
    args
}

/// Children stopped when dropped, by [`end`].
struct Children(Vec<Child>);

impl Drop for Children {
    fn drop(&mut self) {
        for pid in end(&mut self.0, CHILD_PATIENCE) {
            eprintln!("aimctl: pid {pid} did not quit within {CHILD_PATIENCE:?}; killed it");
        }
    }
}

/// Ends `children`: SIGTERM to each, SIGKILL to those still running after
/// `patience`, and every one reaped. The pids that needed SIGKILL.
fn end(children: &mut [Child], patience: Duration) -> Vec<u32> {
    for child in children.iter_mut() {
        if matches!(child.try_wait(), Ok(None)) {
            // SAFETY: a directly owned child whose PID cannot have been reused.
            unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        }
    }
    let deadline = Instant::now() + patience;
    let mut killed = Vec::new();
    for child in children.iter_mut() {
        while matches!(child.try_wait(), Ok(None)) {
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                killed.push(child.id());
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    killed
}

fn start_display(files: &Files, windows: bool, selected: &crate::inputs::Inputs) -> Result<Child, String> {
    let socket = files.display();
    let _ = fs::remove_file(&socket);
    let mut display = Command::new(selected.program("aim-display"))
        .args(display_args(files, windows))
        .spawn()
        .map_err(|e| format!("aim-display: {e}"))?;
    let deadline = Instant::now() + DISPLAY_PATIENCE;
    while !socket.exists() {
        if Instant::now() > deadline || display.try_wait().map_err(|e| e.to_string())?.is_some() {
            let _ = display.kill();
            let _ = display.wait();
            return Err("aim-display did not come up".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    Ok(display)
}

/// `aimctl run`: the guest in the foreground until guest-init ends, or
/// SIGTERM, SIGINT or SIGHUP stops it.
pub fn run(files: &Files, windows: bool, selected: crate::inputs::Inputs) -> Result<ExitCode, String> {
    fs::create_dir_all(files.dir()).map_err(|e| format!("{}: {e}", files.dir().display()))?;
    let _lock = state::lock(&files.lock())
        .map_err(|_| format!("{}: a guest is already running", files.data.display()))?;
    if state::held(&files.guest_lock()) {
        return Err(format!(
            "{}: its data image is in use by another guest-init",
            files.data.display()
        ));
    }
    inputs(&selected)?;
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: the handler only stores to an atomic.
        unsafe {
            libc::signal(
                signal,
                stop_requested as extern "C" fn(libc::c_int) as libc::sighandler_t,
            )
        };
    }
    let mut state = State {
        pid: std::process::id(),
        guest: None,
        windows,
        started: now(),
        inputs: selected,
    };
    state.write(&files.state())?;
    let owner = ProcessIdentity::running(std::process::id() as i32)
        .map_err(|error| format!("resident birth: {error}"))?;
    match fs::remove_file(files.cleanup()) {
        Ok(()) => {},
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => return Err(format!("clear cleanup receipt: {error}")),
    }
    let result = supervise(files, &mut state);
    let success = matches!(&result, Ok(code) if *code == ExitCode::SUCCESS);
    let detail = match &result {
        Ok(_) => String::new(),
        Err(error) => error.clone(),
    };
    let receipt = format!("{} {} {} {}\n{}\n{}\n", owner.host_pid,
        owner.start_seconds, owner.start_microseconds, state.started,
        if success { "ok" } else { "error" }, detail);
    let temporary = files.cleanup().with_extension("new");
    let written = fs::write(&temporary, receipt)
        .and_then(|()| fs::rename(&temporary, files.cleanup()));
    let _ = fs::remove_file(files.state());
    let _ = fs::remove_file(files.display());
    if let Err(error) = written {
        return Err(format!("cleanup receipt: {error}; {}", detail));
    }
    result
}

struct DataBinding {
    image: std::path::PathBuf,
    dev: u64,
    ino: u64,
}

impl DataBinding {
    fn capture(files: &Files) -> Result<Self, String> {
        let image = aim_storage::data::image_of(&files.data);
        let metadata = fs::symlink_metadata(&image).map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("DATA backing is not a regular file".into());
        }
        Ok(Self { image: fs::canonicalize(image).map_err(|error| error.to_string())?,
            dev: metadata.dev(), ino: metadata.ino() })
    }

    fn finish(self, files: &Files, guest: ProcessIdentity, deadline: Instant) -> Result<(), String> {
        if guest.is_live() {
            return Err("owned guest must be reaped before DATA finish".into());
        }
        let _lock = state::lock(&files.guest_lock())?;
        let current = Self::capture(files)?;
        if (current.image, current.dev, current.ino) != (self.image.clone(), self.dev, self.ino) {
            return Err("DATA backing changed during the resident run".into());
        }
        let attached = aim_storage::disk::attachments_of(&self.image)?;
        if attached.len() > 1 {
            return Err("multiple attachments of the owned DATA backing".into());
        }
        for attachment in attached {
            if attachment.shadow.is_some() || !attachment.writable
                || attachment.mounts.iter().any(|mount| mount != &files.data)
            {
                return Err("DATA attachment has a foreign mount or shadow".into());
            }
            aim_storage::disk::detach_until(&attachment.device, deadline)?;
        }
        if !aim_storage::disk::attachments_of(&self.image)?.is_empty()
            || aim_storage::disk::is_mount_point(&files.data)
        {
            return Err("owned DATA remains attached after normal detach".into());
        }
        eprintln!("aimctl: DATA finish guest {}:{}:{} backing {}:{}:{} detached",
            guest.host_pid, guest.start_seconds, guest.start_microseconds,
            self.image.display(), self.dev, self.ino);
        Ok(())
    }
}

fn sweep_memfds(inputs: &crate::inputs::Inputs, deadline: Instant) -> Result<(), String> {
    if Instant::now() >= deadline {
        return Err("memfd cleanup deadline reached".into());
    }
    let mut child = Command::new(inputs.program("linux-run"))
        .arg("--sweep-memfds").stdin(Stdio::null()).stdout(Stdio::null())
        .stderr(Stdio::inherit()).spawn().map_err(|error| format!("memfd cleanup: {error}"))?;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            return if status.success() { Ok(()) } else { Err(format!("memfd cleanup: {status}")) };
        }
        if Instant::now() >= deadline {
            child.kill().map_err(|error| error.to_string())?;
            child.wait().map_err(|error| error.to_string())?;
            return Err("memfd cleanup deadline reached".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn supervise(files: &Files, state: &mut State) -> Result<ExitCode, String> {
    let mut children = Children(vec![start_display(files, state.windows, &state.inputs)?]);
    if state.windows {
        children.0.push(
            Command::new(program("aim-apps"))
                .args(shims_args(files, files.is_default(), &state.inputs))
                .spawn()
                .map_err(|e| format!("aim-apps: {e}"))?,
        );
    }
    let mut guest = Command::new(state.inputs.program("guest-init"))
        .args(guest_init_args(files, &state.inputs))
        .spawn()
        .map_err(|e| format!("guest-init: {e}"))?;
    let guest_birth = ProcessIdentity::running(guest.id() as i32);
    let mut binding = None;
    state.guest = Some(guest.id());
    state.write(&files.state())?;
    let mut stopping: Option<Instant> = None;
    let status = loop {
        if let Some(status) = guest.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if binding.is_none() && aim_storage::data::image_of(&files.data).exists() {
            binding = Some(DataBinding::capture(files));
        }
        match stopping {
            None if STOP.load(Ordering::Relaxed) => {
                // SAFETY: signals the child we started; guest-init stops its
                // services and detaches the data image.
                unsafe { libc::kill(guest.id() as i32, libc::SIGTERM) };
                stopping = Some(Instant::now());
            }
            Some(since) if since.elapsed() > STOP_PATIENCE => {
                eprintln!("aimctl: guest-init did not stop within {STOP_PATIENCE:?}; killing it");
                let _ = guest.kill();
            }
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    drop(children);
    let deadline = stopping.map_or_else(|| Instant::now() + FINISH_PATIENCE,
        |started| (started + RUN_PATIENCE).min(Instant::now() + FINISH_PATIENCE));
    let birth = guest_birth.map_err(|error| format!("guest birth: {error}"))?;
    if birth.is_live() {
        return Err("reaped guest birth still live".into());
    }
    let sweep = sweep_memfds(&state.inputs, deadline);
    let workers = verify_registered_workers_ended(files);
    let finish = match (workers, binding) {
        (Ok(()), Some(Ok(binding))) => binding.finish(files, birth, deadline),
        (Ok(()), Some(Err(error))) | (Err(error), _) => Err(error),
        (Ok(()), None) => verify_data_detached(files),
    };
    match (sweep, finish) {
        (Err(sweep), Err(finish)) => return Err(format!("{sweep}; DATA finish: {finish}")),
        (Err(error), _) | (_, Err(error)) => return Err(error),
        (Ok(()), Ok(())) => {},
    }
    if !status.success() {
        return Err(format!("guest-init stopped with {status}"));
    }
    Ok(ExitCode::SUCCESS)
}

/// `aimctl stop`: stops the resident guest and waits until it is gone.
pub fn stop(files: &Files) -> Result<ExitCode, String> {
    let Some(state) = state::resident(files)? else {
        verify_data_detached(files)?;
        match fs::read_to_string(files.cleanup()) {
            Ok(receipt) if receipt.lines().nth(1) != Some("ok") => {
                return Err("last resident cleanup failed; receipt retained".into());
            }
            Ok(_) => {},
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(format!("resident cleanup receipt: {error}")),
        }
        println!("not running");
        return Ok(ExitCode::SUCCESS);
    };
    let owner = ProcessIdentity::running(state.pid as i32)
        .map_err(|error| format!("resident birth: {error}"))?;
    if end_run_owned(&files.lock(), owner, RUN_PATIENCE)? {
        return Err(format!("resident {} required forced process termination; cleanup unproven", state.pid));
    }
    let _lock = state::lock(&files.lock())?;
    let receipt = fs::read_to_string(files.cleanup())
        .map_err(|error| format!("resident cleanup unproven: {error}"))?;
    let mut lines = receipt.lines();
    let expected = format!("{} {} {} {}", owner.host_pid, owner.start_seconds,
        owner.start_microseconds, state.started);
    if lines.next() != Some(expected.as_str()) {
        return Err("resident cleanup receipt belongs to another run".into());
    }
    if lines.next() != Some("ok") {
        return Err(format!("resident cleanup failed: {}", lines.collect::<Vec<_>>().join("\n")));
    }
    verify_data_detached(files)?;
    verify_registered_workers_ended(files)?;
    println!("stopped");
    Ok(ExitCode::SUCCESS)
}

fn verify_data_detached(files: &Files) -> Result<(), String> {
    let image = aim_storage::data::image_of(&files.data);
    if aim_storage::disk::is_mount_point(&files.data)
        || (image.exists() && !aim_storage::disk::attachments_of(&image)?.is_empty())
    {
        return Err("DATA remains attached after resident cleanup".into());
    }
    Ok(())
}

// Namespace records are a final observation, never authority to kill a PID.
// They catch live workers that reparented before the ancestor snapshot.
fn verify_registered_workers_ended(files: &Files) -> Result<(), String> {
    let table = files.by_pid();
    for file in fs::read_dir(&table).map_err(|error| format!("worker registry: {error}"))? {
        let file = file.map_err(|error| error.to_string())?;
        let name = file.file_name();
        let Some(pid) = name.to_str().and_then(|name| name.strip_suffix(".mount-namespace"))
            .and_then(|pid| pid.parse::<i32>().ok()) else { continue; };
        if pid <= 0 || !file.file_type().map_err(|error| error.to_string())?.is_file() {
            return Err("invalid worker registry record".into());
        }
        let mut text = String::new();
        fs::File::open(file.path()).map_err(|error| error.to_string())?
            .take(4097).read_to_string(&mut text).map_err(|error| error.to_string())?;
        let fields = text.trim_end_matches('\n').split('\t').collect::<Vec<_>>();
        if text.len() > 4096 || fields.len() != 5 || fields[0] != "AIMPROCNS1"
            || fields[1].parse::<i32>() != Ok(pid)
        {
            return Err("invalid worker registry binding".into());
        }
        let recorded = ProcessIdentity { host_pid: pid,
            start_seconds: fields[2].parse().map_err(|_| "invalid worker birth".to_string())?,
            start_microseconds: fields[3].parse().map_err(|_| "invalid worker birth".to_string())? };
        if same_birth(recorded)? {
            return Err(format!("registered worker still live: {}:{}:{}; not signalled without owned ancestry",
                pid, recorded.start_seconds, recorded.start_microseconds));
        }
    }
    Ok(())
}

fn process_info(pid: i32) -> Result<Option<libc::proc_bsdinfo>, String> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&info) as i32;
    // SAFETY: kernel-owned metadata, written to a correctly sized buffer.
    let count = unsafe {
        *libc::__error() = 0;
        libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 1,
            (&mut info as *mut libc::proc_bsdinfo).cast(), size)
    };
    if count == size { return Ok(Some(info)); }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) { return Ok(None); }
    Err(format!("process metadata {pid}: {error}; returned {count}/{size}"))
}

fn info_identity(info: &libc::proc_bsdinfo) -> ProcessIdentity {
    ProcessIdentity { host_pid: info.pbi_pid as i32,
        start_seconds: info.pbi_start_tvsec, start_microseconds: info.pbi_start_tvusec }
}

fn same_birth(process: ProcessIdentity) -> Result<bool, String> {
    Ok(process_info(process.host_pid)?.is_some_and(|info| info_identity(&info) == process))
}

fn signal_owned(process: ProcessIdentity, signal: i32) -> Result<(), String> {
    if !same_birth(process)? { return Ok(()); }
    // SAFETY: a retained birth from a verified owned-parent edge; individual PID only.
    if unsafe { libc::kill(process.host_pid, signal) } == 0 { return Ok(()); }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) { return Ok(()); }
    Err(format!("owned pid {} signal {signal}: {error}", process.host_pid))
}

fn collect_descendants(members: &mut Vec<ProcessIdentity>, deadline: Instant) -> Result<(), String> {
    let mut at = 0;
    while at < members.len() {
        if Instant::now() >= deadline { return Err("owned process snapshot deadline reached".into()); }
        let parent = members[at];
        at += 1;
        if !same_birth(parent)? { continue; }
        let mut pids = vec![0i32; 128];
        let count = loop {
            // proc_listchildpids returns PID count, not byte count (Apple libproc).
            let count = unsafe {
                *libc::__error() = 0;
                libc::proc_listchildpids(parent.host_pid, pids.as_mut_ptr().cast(),
                    (pids.len() * std::mem::size_of::<i32>()) as i32)
            };
            if count < 0 || (count == 0 && std::io::Error::last_os_error().raw_os_error() != Some(0)) {
                return Err(format!("owned child snapshot {}: {}", parent.host_pid,
                    std::io::Error::last_os_error()));
            }
            if (count as usize) < pids.len() { break count as usize; }
            if pids.len() >= 16384 || Instant::now() >= deadline {
                return Err("owned child snapshot truncated".into());
            }
            pids.resize(pids.len() * 2, 0);
        };
        for pid in pids.into_iter().take(count).filter(|pid| *pid > 0) {
            let Some(info) = process_info(pid)? else { continue; };
            if info.pbi_ppid as i32 != parent.host_pid || !same_birth(parent)? { continue; }
            let child = info_identity(&info);
            if !same_birth(child)? || members.contains(&child) { continue; }
            if members.len() >= 16384 { return Err("owned descendant snapshot truncated".into()); }
            eprintln!("aimctl: owned child {}:{}:{} parent {}:{}:{}",
                child.host_pid, child.start_seconds, child.start_microseconds,
                parent.host_pid, parent.start_seconds, parent.start_microseconds);
            members.push(child);
        }
    }
    Ok(())
}

fn all_ended(members: &[ProcessIdentity]) -> Result<bool, String> {
    for member in members {
        if same_birth(*member)? { return Ok(false); }
    }
    Ok(true)
}

#[cfg(test)]
fn end_run(lock: &Path, pid: u32, patience: Duration) -> Result<bool, String> {
    let owner = ProcessIdentity::running(pid as i32).map_err(|error| error.to_string())?;
    end_run_owned(lock, owner, patience)
}

/// Capture owned-parent edges before SIGTERM. Retained births stay valid after
/// reparenting; process-group membership never establishes ownership.
fn end_run_owned(lock: &Path, owner: ProcessIdentity, patience: Duration) -> Result<bool, String> {
    let deadline = Instant::now() + patience;
    if !same_birth(owner)? { return Err("resident birth ended before stop".into()); }
    let mut members = vec![owner];
    collect_descendants(&mut members, deadline)?;
    signal_owned(owner, libc::SIGTERM)?;
    while Instant::now() < deadline {
        collect_descendants(&mut members, deadline)?;
        if !same_birth(owner)? {
            for member in members.iter().skip(1).rev() { signal_owned(*member, libc::SIGTERM)?; }
        }
        if all_ended(&members)? {
            return if state::held(lock) {
                Err("owned run ended but its lock has another holder".into())
            } else { Ok(false) };
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let killed_deadline = Instant::now() + KILL_PATIENCE;
    collect_descendants(&mut members, killed_deadline)?;
    // Children first, retaining every captured incarnation through final checks.
    for member in members.iter().rev() { signal_owned(*member, libc::SIGKILL)?; }
    while Instant::now() < killed_deadline {
        if all_ended(&members)? {
            return if state::held(lock) {
                Err("owned run ended but its lock has another holder".into())
            } else { Ok(true) };
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Err("owned run workers remain after individual-PID cleanup".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[OsString]) -> Vec<&str> {
        args.iter().map(|a| a.to_str().unwrap()).collect()
    }

    #[test]
    fn the_display_server_lives_in_the_state_directory() {
        let files = Files::of(Path::new("/d")).unwrap();
        assert_eq!(
            strings(&display_args(&files, false)),
            [
                "--socket",
                "/d.aimctl/display",
                "--size",
                "1080x1920",
                "--capture",
                "/d.aimctl/capture.bmp"
            ]
        );
        assert_eq!(
            strings(&display_args(&files, true)),
            [
                "--socket",
                "/d.aimctl/display",
                "--mode",
                "windows",
                "--apps",
                "/d.aimctl/apps",
                "--capture",
                "/d.aimctl/capture.bmp"
            ]
        );
    }

    #[test]
    fn guest_init_boots_the_data_directory() {
        let files = Files::of(Path::new("/d")).unwrap();
        let args = guest_init_args(&files, &crate::inputs::Inputs::default());
        let args = strings(&args);
        let after = |flag: &str| args[args.iter().position(|a| *a == flag).unwrap() + 1];
        assert_eq!(after("--data"), "/d");
        assert_eq!(after("--display"), "/d.aimctl/display");
        assert!(args.contains(&"--run"));
        assert!(!args.contains(&"--binder-trace"));
        let shims = shims_args(&files, false, &crate::inputs::Inputs::default());
        let shims = strings(&shims);
        assert_eq!(
            shims[shims.iter().position(|a| *a == "--data").unwrap() + 1],
            "/d/data"
        );
        assert_eq!(
            shims[shims.iter().position(|a| *a == "--into").unwrap() + 1],
            "/d.aimctl/apps"
        );
        assert!(shims.contains(&"--scoped"));
        // The user's own guest keeps the stable identifiers.
        assert!(!strings(&shims_args(&files, true, &crate::inputs::Inputs::default())).contains(&"--scoped"));
        assert!(files.is_of(Path::new("/d")) && !files.is_of(Path::new("/e")));
    }

    #[test]
    fn selected_inputs_reach_init_display_and_shims() {
        let files = Files::of(Path::new("/d")).unwrap();
        let selected = crate::inputs::Inputs { image: "/image".into(),
            host_runtime: "/runtime".into(), userdata: "/templates".into(), binder_trace: Some("/trace.tsv".into()) };
        let args = guest_init_args(&files, &selected);
        let args = strings(&args);
        let after = |flag: &str| args[args.iter().position(|arg| *arg == flag).unwrap() + 1];
        assert_eq!(after("--image"), "/image");
        assert_eq!(after("--linux-run"), "/runtime/linux-run");
        assert_eq!(after("--userdata"), "/templates");
        assert_eq!(after("--binder-trace"), "/trace.tsv");
        let shims = shims_args(&files, false, &selected);
        let shims = strings(&shims);
        assert!(shims.windows(2).any(|pair| pair == ["--image", "/image"]));
        assert!(shims.windows(2).any(|pair| pair == ["--host", "/runtime/aim-display"]));
    }

    /// `program` with `args`, in a session of its own, SIGTERM ignored
    /// when `deaf`.
    fn spawn(program: &str, args: &[&str], deaf: bool) -> Child {
        let mut command = Command::new(program);
        command.args(args);
        // SAFETY: setsid and signal are async-signal-safe.
        unsafe {
            command.pre_exec(move || {
                libc::setsid();
                if deaf {
                    libc::signal(libc::SIGTERM, libc::SIG_IGN);
                }
                Ok(())
            })
        };
        command.spawn().unwrap()
    }

    #[test]
    fn children_deaf_to_sigterm_are_killed() {
        let mut children = vec![
            spawn("/bin/sleep", &["600"], false),
            spawn("/bin/sleep", &["600"], true),
        ];
        let deaf = children[1].id();
        let start = Instant::now();
        assert_eq!(end(&mut children, Duration::from_millis(300)), [deaf]);
        assert!(start.elapsed() < Duration::from_secs(5));
        for child in &mut children {
            assert!(child.try_wait().unwrap().is_some());
        }
    }

    /// A stand-in for `aimctl run`: a session leader that holds `lock`,
    /// with a child of its own (the display server), both SIGTERM-deaf
    /// when `deaf`. It is reaped by a thread, as launchd reaps a run.
    fn fake_run(lock: &Path, deaf: bool) -> u32 {
        let script = "fork or exec '/bin/sleep', '600'; \
                      use Fcntl ':flock'; open my $f, '>', $ARGV[0] or die; \
                      flock $f, LOCK_EX or die; sleep 600";
        let mut run = spawn(
            "/usr/bin/perl",
            &["-e", script, lock.to_str().unwrap()],
            deaf,
        );
        let pid = run.id();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !state::held(lock) {
            assert!(
                Instant::now() < deadline,
                "the fake run never took its lock"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::spawn(move || run.wait());
        pid
    }

    #[test]
    fn stop_kills_a_run_deaf_to_sigterm_with_its_children() {
        let lock = std::env::temp_dir().join(format!("aimctl-stop-{}", std::process::id()));
        let pid = fake_run(&lock, true);
        let mut workers = vec![ProcessIdentity::running(pid as i32).unwrap()];
        collect_descendants(&mut workers, Instant::now() + Duration::from_secs(2)).unwrap();
        assert!(workers.len() > 1);
        assert_eq!(end_run(&lock, pid, Duration::from_millis(300)), Ok(true));
        assert!(!state::held(&lock));
        assert!(all_ended(&workers).unwrap());
        // A run that exits on SIGTERM may orphan its child; retained birth
        // evidence still permits individual-PID cleanup after reparenting.
        let pid = fake_run(&lock, false);
        let mut workers = vec![ProcessIdentity::running(pid as i32).unwrap()];
        collect_descendants(&mut workers, Instant::now() + Duration::from_secs(2)).unwrap();
        assert!(workers.len() > 1);
        assert_eq!(end_run(&lock, pid, KILL_PATIENCE), Ok(false));
        assert!(all_ended(&workers).unwrap());
        fs::remove_file(&lock).unwrap();
    }

    #[test]
    fn stop_leaves_a_foreign_member_of_the_same_group_alive() {
        let lock = std::env::temp_dir().join(format!("aimctl-foreign-{}", std::process::id()));
        let mut command = Command::new("/usr/bin/perl");
        command.args(["-e", "use Fcntl ':flock'; open my $f, '>', $ARGV[0] or die; flock $f, LOCK_EX or die; sleep 600"])
            .arg(&lock);
        unsafe { command.pre_exec(|| {
            if libc::setpgid(0, 0) != 0 { return Err(std::io::Error::last_os_error()); }
            libc::signal(libc::SIGTERM, libc::SIG_IGN);
            Ok(())
        }); }
        let mut run = command.spawn().unwrap();
        let owner = ProcessIdentity::running(run.id() as i32).unwrap();
        let mut peer_command = Command::new("/bin/sleep");
        peer_command.arg("600");
        unsafe { peer_command.pre_exec(move || {
            if libc::setpgid(0, owner.host_pid) != 0 { return Err(std::io::Error::last_os_error()); }
            Ok(())
        }); }
        let mut peer = peer_command.spawn().unwrap();
        let peer_birth = ProcessIdentity::running(peer.id() as i32).unwrap();
        let reaper = std::thread::spawn(move || run.wait());
        let result = end_run_owned(&lock, owner, Duration::from_millis(300));
        let peer_live = same_birth(peer_birth);
        signal_owned(owner, libc::SIGKILL).unwrap();
        reaper.join().unwrap().unwrap();
        peer.kill().unwrap();
        peer.wait().unwrap();
        let _ = fs::remove_file(&lock);
        assert_eq!(result, Ok(true));
        assert_eq!(peer_live, Ok(true));
    }

    #[test]
    fn stale_birth_never_signals_the_current_process() {
        let mut child = spawn("/bin/sleep", &["600"], false);
        let actual = ProcessIdentity::running(child.id() as i32).unwrap();
        let mut stale = actual;
        stale.start_seconds += 1;
        let result = signal_owned(stale, libc::SIGKILL);
        let still_live = same_birth(actual);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(result, Ok(()));
        assert_eq!(still_live, Ok(true));
    }

    #[test]
    fn tail_of_a_log() {
        let path = std::env::temp_dir().join(format!("aimctl-tail-{}", std::process::id()));
        fs::write(&path, "0123456789").unwrap();
        assert_eq!(tail(&path, 4), "6789");
        assert_eq!(tail(&path, 40), "0123456789");
        fs::remove_file(&path).unwrap();
        assert_eq!(tail(&path, 4), "");
    }
}
