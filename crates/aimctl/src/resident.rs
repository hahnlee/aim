//! The resident guest: `aimctl run` starts the display server, the shims'
//! keeper in window mode, and guest-init, and stops them when guest-init
//! ends or it is told to stop; `aimctl start` runs it in the background
//! and `aimctl stop` stops it.

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::process::CommandExt;
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

fn inputs() -> Result<(), String> {
    for (path, what) in [
        (aim_paths::derived_image(), "the derived image"),
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
pub fn start(files: &Files, windows: bool) -> Result<ExitCode, String> {
    check_free(files)?;
    inputs()?;
    fs::create_dir_all(files.dir()).map_err(|e| format!("{}: {e}", files.dir().display()))?;
    let log =
        fs::File::create(files.log()).map_err(|e| format!("{}: {e}", files.log().display()))?;
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command.arg("--data").arg(&files.data).arg("run");
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
/// unless this is the user's data directory, whose shims keep theirs (and
/// with them their notification settings).
fn shims_args(files: &Files) -> Vec<OsString> {
    let mut args: Vec<OsString> = vec![
        "shims".into(),
        "--image".into(),
        aim_paths::derived_image().into(),
        "--data".into(),
        files.guest_data().into(),
        "--display".into(),
        files.display().into(),
        "--host".into(),
        program("aim-display").into(),
        "--into".into(),
        files.apps().into(),
        "--watch".into(),
    ];
    if !files.is_default() {
        args.push("--scoped".into());
    }
    args
}

fn guest_init_args(files: &Files) -> Vec<OsString> {
    vec![
        "--image".into(),
        aim_paths::derived_image().into(),
        "--data".into(),
        files.data.clone().into(),
        "--run".into(),
        "--gpu".into(),
        aim_paths::angle().into(),
        "--vulkan".into(),
        aim_paths::moltenvk().into(),
        "--display".into(),
        files.display().into(),
        "--quiet".into(),
    ]
}

/// Children stopped when dropped: SIGTERM, then waited for.
struct Children(Vec<Child>);

impl Drop for Children {
    fn drop(&mut self) {
        for child in &mut self.0 {
            // SAFETY: signals a child we started.
            unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
            let _ = child.wait();
        }
    }
}

fn start_display(files: &Files, windows: bool) -> Result<Child, String> {
    let socket = files.display();
    let _ = fs::remove_file(&socket);
    let mut display = Command::new(program("aim-display"))
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
pub fn run(files: &Files, windows: bool) -> Result<ExitCode, String> {
    fs::create_dir_all(files.dir()).map_err(|e| format!("{}: {e}", files.dir().display()))?;
    let _lock = state::lock(&files.lock())
        .map_err(|_| format!("{}: a guest is already running", files.data.display()))?;
    if state::held(&files.guest_lock()) {
        return Err(format!(
            "{}: its data image is in use by another guest-init",
            files.data.display()
        ));
    }
    inputs()?;
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
    };
    state.write(&files.state())?;
    let result = supervise(files, &mut state);
    let _ = fs::remove_file(files.state());
    let _ = fs::remove_file(files.display());
    result
}

fn supervise(files: &Files, state: &mut State) -> Result<ExitCode, String> {
    let mut children = Children(vec![start_display(files, state.windows)?]);
    if state.windows {
        children.0.push(
            Command::new(program("aim-apps"))
                .args(shims_args(files))
                .spawn()
                .map_err(|e| format!("aim-apps: {e}"))?,
        );
    }
    let mut guest = Command::new(program("guest-init"))
        .args(guest_init_args(files))
        .spawn()
        .map_err(|e| format!("guest-init: {e}"))?;
    state.guest = Some(guest.id());
    state.write(&files.state())?;
    let mut stopping: Option<Instant> = None;
    let status = loop {
        if let Some(status) = guest.try_wait().map_err(|e| e.to_string())? {
            break status;
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
    Ok(if status.success() || stopping.is_some() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// `aimctl stop`: stops the resident guest and waits until it is gone.
pub fn stop(files: &Files) -> Result<ExitCode, String> {
    let Some(state) = state::resident(files)? else {
        println!("not running");
        return Ok(ExitCode::SUCCESS);
    };
    // SAFETY: signals the process holding the data directory's aimctl lock.
    if unsafe { libc::kill(state.pid as i32, libc::SIGTERM) } != 0 {
        return Err(format!(
            "pid {}: {}",
            state.pid,
            std::io::Error::last_os_error()
        ));
    }
    // guest-init's own patience, the children's stop and the detach.
    let deadline = Instant::now() + STOP_PATIENCE + Duration::from_secs(60);
    while state::held(&files.lock()) {
        if Instant::now() > deadline {
            return Err(format!("pid {} did not stop", state.pid));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("stopped");
    Ok(ExitCode::SUCCESS)
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
        let args = guest_init_args(&files);
        let args = strings(&args);
        let after = |flag: &str| args[args.iter().position(|a| *a == flag).unwrap() + 1];
        assert_eq!(after("--data"), "/d");
        assert_eq!(after("--display"), "/d.aimctl/display");
        assert!(args.contains(&"--run"));
        let shims = shims_args(&files);
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
