//! The macOS disk image tools, run as the user (no admin rights):
//! `diskutil image` creates, converts and attaches images, `newfs_apfs -e`
//! makes a case-sensitive APFS volume on a blank one, `diskutil eject`
//! detaches, and the IORegistry lists what is attached.

use std::ffi::CStr;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

mod registry;

/// An attached image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attached {
    pub image: PathBuf,
    pub shadow: Option<PathBuf>,
    pub writable: bool,
    /// The image's own device (`/dev/diskN`): ejecting it detaches the
    /// image with every volume on it.
    pub device: String,
    /// Every device of the image: its own, the APFS container's, the
    /// volumes'.
    pub devices: Vec<String>,
    pub mounts: Vec<PathBuf>,
}

/// How long a disk image tool may take. A hung storagekitd leaves them
/// waiting forever (#425); a boot fails instead.
const LIMIT: Duration = Duration::from_secs(120);
/// How long converting a whole image may take.
const CONVERT_LIMIT: Duration = Duration::from_secs(30 * 60);
/// A newly attached blank device may still be unavailable to the formatter.
const FORMAT_LIMIT: Duration = Duration::from_secs(20);

fn run(command: &mut Command) -> Result<Output, String> {
    run_within(command, LIMIT)
}

fn run_within(command: &mut Command, limit: Duration) -> Result<Output, String> {
    run_until(command, Instant::now() + limit)
}

fn run_until(command: &mut Command, deadline: Instant) -> Result<Output, String> {
    let output = run_output_until(command, deadline)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(command_error(command, &output))
    }
}

fn command_error(command: &Command, output: &Output) -> String {
    let mut text = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.trim().is_empty() {
        text = format!("{} {text}", stdout.trim());
    }
    let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy()).collect();
    format!(
        "{} {}: {text}",
        command.get_program().to_string_lossy(),
        args.join(" ")
    )
}

fn run_output_until(command: &mut Command, deadline: Instant) -> Result<Output, String> {
    let limit = deadline.saturating_duration_since(Instant::now());
    if limit.is_zero() {
        return Err("disk command cleanup deadline reached".into());
    }
    let describe = |command: &Command| {
        let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy()).collect();
        format!(
            "{} {}",
            command.get_program().to_string_lossy(),
            args.join(" ")
        )
    };
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{:?}: {e}", command.get_program()))?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            bytes
        })
    };
    let stdout = drain(child.stdout.take().map(|p| Box::new(p) as _));
    let stderr = drain(child.stderr.take().map(|p| Box::new(p) as _));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{}: no answer in {} s (a hung storagekitd? #425)",
                    describe(command),
                    limit.as_secs()
                ));
            }
            Err(e) => return Err(format!("{}: {e}", describe(command))),
        }
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// Every attached image, from the IORegistry: an `AppleDiskImageDevice`
/// per attachment, with the image's and shadow's URLs, and the IOMedia of
/// its disk, container and volumes below it; their mounts are the
/// kernel's. Not `hdiutil info`, which, while another image attaches or
/// detaches, succeeds with some attached images or their devices left out
/// (#653).
pub fn attached() -> Result<Vec<Attached>, String> {
    let mounts = mounts()?;
    let mut out = Vec::new();
    for device in registry::Object::matching(c"AppleDiskImageDevice")? {
        let mut media = Vec::new();
        device.media(&mut media);
        let Some((first, writable)) = media.first().map(|(d, w)| (d.clone(), *w)) else {
            // Still attaching, or detaching: no disk yet, or any more.
            continue;
        };
        let devices: Vec<String> = media.into_iter().map(|(d, _)| d).collect();
        let Some(image) = device.path(c"DiskImageURL") else {
            continue;
        };
        out.push(Attached {
            image,
            shadow: device.path(c"ShadowURL"),
            writable,
            device: first,
            mounts: mounts
                .iter()
                .filter(|(from, _)| devices.contains(from))
                .map(|(_, on)| on.clone())
                .collect(),
            devices,
        });
    }
    Ok(out)
}

/// The kernel's mounts, as (device, mount point).
fn mounts() -> Result<Vec<(String, PathBuf)>, String> {
    // SAFETY: a null buffer asks for the count.
    let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if count < 0 {
        return Err(format!("getfsstat: {}", std::io::Error::last_os_error()));
    }
    // Room for mounts made in between. SAFETY: statfs is plain data.
    let mut fs: Vec<libc::statfs> = vec![unsafe { std::mem::zeroed() }; count as usize + 16];
    let size = (fs.len() * std::mem::size_of::<libc::statfs>()) as libc::c_int;
    // SAFETY: a buffer of `size` bytes.
    let count = unsafe { libc::getfsstat(fs.as_mut_ptr(), size, libc::MNT_NOWAIT) };
    if count < 0 {
        return Err(format!("getfsstat: {}", std::io::Error::last_os_error()));
    }
    fs.truncate(count as usize);
    Ok(fs
        .iter()
        .map(|fs| {
            // SAFETY: getfsstat NUL-terminates both names.
            let (from, on) = unsafe {
                (
                    CStr::from_ptr(fs.f_mntfromname.as_ptr()),
                    CStr::from_ptr(fs.f_mntonname.as_ptr()),
                )
            };
            (
                from.to_string_lossy().into_owned(),
                PathBuf::from(std::ffi::OsStr::from_bytes(on.to_bytes())),
            )
        })
        .collect())
}

/// The attachments of `image` (its real path).
pub fn attachments_of(image: &Path) -> Result<Vec<Attached>, String> {
    let image = std::fs::canonicalize(image).map_err(|e| format!("{}: {e}", image.display()))?;
    Ok(attached()?
        .into_iter()
        .filter(|a| a.image == image)
        .collect())
}

/// How to attach an image.
#[derive(Clone, Copy, Debug, Default)]
pub struct Attach<'a> {
    pub read_only: bool,
    /// Writes go to this file; the image itself is never written.
    pub shadow: Option<&'a Path>,
    /// Where to mount its volume, hidden from the Finder; None attaches
    /// the device only.
    pub mount: Option<&'a Path>,
}

/// Attaches `image` by its real path, which is what [`attached`] then
/// lists; returns its device.
pub fn attach(image: &Path, how: Attach) -> Result<String, String> {
    let image = &std::fs::canonicalize(image).map_err(|e| format!("{}: {e}", image.display()))?;
    let mut command = Command::new("diskutil");
    command.args(["image", "attach"]);
    if how.read_only {
        command.arg("--readOnly");
    }
    match how.mount {
        Some(mount) => {
            command.args(["--nobrowse", "--mountPoint"]).arg(mount);
        }
        None => {
            command.arg("--noMount");
        }
    }
    command.arg(image);
    if let Some(shadow) = how.shadow {
        command.arg("--shadow").arg(shadow);
    }
    let output = run(&mut command)?;
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .find(|w| w.starts_with("/dev/disk"))
        .map(str::to_string)
        .ok_or_else(|| format!("{}: attached without a device", image.display()))
}

/// Detaches the image of `device`. Busy volumes are retried for `patience`.
/// A dissenter is an owner, not permission to force its volume away (#1094).
pub fn detach(device: &str, patience: Duration) -> Result<(), String> {
    detach_with(device, patience, || {
        run(Command::new("diskutil").args(["eject", device])).map(|_| ())
    })
}

/// Normal detach sharing the caller's absolute cleanup deadline.
pub fn detach_until(device: &str, deadline: Instant) -> Result<(), String> {
    loop {
        if Instant::now() >= deadline {
            return Err(format!("normal detach {device}: cleanup deadline reached"));
        }
        match run_until(Command::new("diskutil").args(["eject", device]), deadline) {
            Ok(_) => return Ok(()),
            Err(error) => {
                if !(error.contains("dissented") || error.contains("could not be unmounted"))
                    || Instant::now() >= deadline
                {
                    return Err(format!("refusing to force detach {device}: {error}"));
                }
                std::thread::sleep(
                    Duration::from_millis(500)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        }
    }
}

fn detach_with(
    device: &str,
    patience: Duration,
    eject: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    detach_with_clock(device, patience, eject, Instant::now, std::thread::sleep)
}

fn detach_with_clock(
    device: &str,
    patience: Duration,
    mut eject: impl FnMut() -> Result<(), String>,
    mut now: impl FnMut() -> Instant,
    mut sleep: impl FnMut(Duration),
) -> Result<(), String> {
    let deadline = now() + patience;
    loop {
        match eject() {
            Ok(()) => return Ok(()),
            Err(e) => {
                let busy = e.contains("dissented") || e.contains("could not be unmounted");
                if !busy || now() >= deadline {
                    return Err(format!("refusing to force detach {device}: {e}"));
                }
                sleep(Duration::from_millis(500).min(deadline.saturating_duration_since(now())));
            }
        }
    }
}

#[cfg(test)]
mod detach_tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn transient_dissenter_is_retried_without_force() {
        let mut calls = 0;
        let start = Instant::now();
        let elapsed = Cell::new(Duration::ZERO);
        let result = detach_with_clock(
            "fixture-device",
            Duration::from_secs(10),
            || {
                calls += 1;
                if calls == 1 {
                    Err("Unmount was dissented by PID 42".into())
                } else {
                    Ok(())
                }
            },
            || start + elapsed.get(),
            |delay| elapsed.set(elapsed.get() + delay),
        );
        assert!(result.is_ok());
        assert_eq!(calls, 2);
        assert_eq!(elapsed.get(), Duration::from_millis(500));
    }

    #[test]
    fn persistent_dissenter_expires_with_final_diagnostic() {
        let mut calls = 0;
        let start = Instant::now();
        let elapsed = Cell::new(Duration::ZERO);
        let result = detach_with_clock(
            "fixture-device",
            Duration::from_millis(750),
            || {
                calls += 1;
                Err(format!("Unmount was dissented by PID {}", 40 + calls))
            },
            || start + elapsed.get(),
            |delay| elapsed.set(elapsed.get() + delay),
        );
        assert_eq!(calls, 3);
        assert_eq!(elapsed.get(), Duration::from_millis(750));
        let error = result.unwrap_err();
        assert!(error.contains("refusing to force detach"));
        assert!(error.contains("PID 43"));
    }

    #[test]
    fn zero_patience_dissenter_is_not_retried() {
        let mut calls = 0;
        let result = detach_with("fixture-device", Duration::ZERO, || {
            calls += 1;
            Err("Unmount was dissented by PID 42".into())
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn unrelated_eject_error_is_not_retried_or_forced() {
        let mut calls = 0;
        assert!(
            detach_with("fixture-device", Duration::from_secs(20), || {
                calls += 1;
                Err("permission denied".into())
            })
            .is_err()
        );
        assert_eq!(calls, 1);
    }
}

/// Creates a blank sparse image (ASIF) of up to `size` (diskutil's size
/// syntax, such as `32g`, or bytes) with one case-sensitive APFS volume.
pub fn create_case_sensitive(image: &Path, size: &str, volume: &str) -> Result<(), String> {
    run(Command::new("diskutil")
        .args([
            "image", "create", "blank", "--format", "ASIF", "--fs", "None",
        ])
        .args(["--size", size])
        .arg(image))?;
    let device = attach(image, Attach::default())?;
    let raw = device.replacen("/dev/disk", "/dev/rdisk", 1);
    let formatted = format_apfs(&raw, volume);
    let detached = detach(&device, Duration::from_secs(10));
    match (formatted, detached) {
        (Err(format), Err(detach)) => Err(format!("{format}; cleanup failed: {detach}")),
        (format, detach) => format.and(detach),
    }
}

fn format_apfs(raw: &str, volume: &str) -> Result<(), String> {
    format_with_clock(
        Command::new("newfs_apfs")
            .args(["-e", "-v", volume, raw])
            .env("LC_ALL", "C"),
        raw,
        FORMAT_LIMIT,
        run_output_until,
        Instant::now,
        std::thread::sleep,
    )
}

fn format_with_clock(
    command: &mut Command,
    raw: &str,
    patience: Duration,
    mut format: impl FnMut(&mut Command, Instant) -> Result<Output, String>,
    mut now: impl FnMut() -> Instant,
    mut sleep: impl FnMut(Duration),
) -> Result<(), String> {
    let start = now();
    let deadline = start + patience;
    let mut attempts = 0;
    loop {
        attempts += 1;
        let result = format(command, deadline);
        let error = match result {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(output) => {
                let error = command_error(command, &output);
                // Retry only the formatter's device-open diagnostic, without
                // format progress. Persistent permission failures still fail.
                let stderr = String::from_utf8_lossy(&output.stderr);
                let unavailable = output.status.code().is_some()
                    && output.stdout.is_empty()
                    && ["Operation not permitted", "Resource busy"]
                        .iter()
                        .any(|reason| stderr.trim() == format!("newfs_apfs: {raw}: {reason}"));
                if unavailable && now() < deadline {
                    sleep(
                        Duration::from_millis(200).min(deadline.saturating_duration_since(now())),
                    );
                    if now() < deadline {
                        continue;
                    }
                }
                error
            }
            Err(error) => error,
        };
        return Err(format!(
            "format {raw} failed after {attempts} attempt(s) in {:.1} s: {error}",
            now().saturating_duration_since(start).as_secs_f64()
        ));
    }
}

#[cfg(test)]
mod format_tests {
    use super::*;
    use std::cell::Cell;
    use std::os::unix::process::ExitStatusExt;

    fn output(stderr: &str) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(if stderr.is_empty() { 0 } else { 256 }),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    fn command() -> Command {
        let mut command = Command::new("newfs_apfs");
        command.args(["-e", "-v", "aim-data", "/dev/rdisk-fixture"]);
        command
    }

    #[test]
    fn device_open_eperm_and_ebusy_recover_with_one_deadline() {
        for reason in ["Operation not permitted", "Resource busy"] {
            let start = Instant::now();
            let elapsed = Cell::new(Duration::ZERO);
            let patience = Duration::from_secs(1);
            let mut calls = 0;
            format_with_clock(
                &mut command(),
                "/dev/rdisk-fixture",
                patience,
                |_, deadline| {
                    assert_eq!(deadline, start + patience);
                    calls += 1;
                    if calls == 3 {
                        Ok(output(""))
                    } else {
                        Ok(output(&format!(
                            "newfs_apfs: /dev/rdisk-fixture: {reason}\n"
                        )))
                    }
                },
                || start + elapsed.get(),
                |delay| elapsed.set(elapsed.get() + delay),
            )
            .unwrap();
            assert_eq!(calls, 3);
            assert_eq!(elapsed.get(), Duration::from_millis(400));
        }
    }

    #[test]
    fn persistent_permission_failure_is_bounded_and_preserves_the_error() {
        let start = Instant::now();
        let elapsed = Cell::new(Duration::ZERO);
        let mut calls = 0;
        let error = format_with_clock(
            &mut command(),
            "/dev/rdisk-fixture",
            Duration::from_millis(750),
            |_, _| {
                calls += 1;
                Ok(output(
                    "newfs_apfs: /dev/rdisk-fixture: Operation not permitted\n",
                ))
            },
            || start + elapsed.get(),
            |delay| elapsed.set(elapsed.get() + delay),
        )
        .unwrap_err();
        assert_eq!(calls, 4);
        assert_eq!(elapsed.get(), Duration::from_millis(750));
        assert!(error.contains("4 attempt(s) in 0.8 s"), "{error}");
        assert!(
            error.ends_with("newfs_apfs: /dev/rdisk-fixture: Operation not permitted"),
            "{error}"
        );
    }

    #[test]
    fn unrelated_errors_and_partial_formatting_are_not_retried() {
        let errors = [
            output("newfs_apfs: /dev/rdisk-fixture: Permission denied"),
            output("newfs_apfs: /dev/rdisk-other: Operation not permitted"),
            output("newfs_apfs: /dev/rdisk-fixture: Input/output error"),
            Output {
                stdout: b"Creating a new APFS Container\n".to_vec(),
                ..output("newfs_apfs: /dev/rdisk-fixture: Operation not permitted")
            },
            Output {
                status: std::process::ExitStatus::from_raw(libc::SIGKILL),
                ..output("newfs_apfs: /dev/rdisk-fixture: Operation not permitted")
            },
        ];
        for output in errors {
            let mut calls = 0;
            let error = format_with_clock(
                &mut command(),
                "/dev/rdisk-fixture",
                FORMAT_LIMIT,
                |_, _| {
                    calls += 1;
                    Ok(output.clone())
                },
                Instant::now,
                |_| panic!("must not wait on an unrelated failure"),
            )
            .unwrap_err();
            assert_eq!(calls, 1);
            assert!(error.contains(String::from_utf8_lossy(&output.stderr).as_ref()));
        }
    }

    #[test]
    fn command_failure_and_timeout_are_not_retried() {
        let mut calls = 0;
        let error = format_with_clock(
            &mut command(),
            "/dev/rdisk-fixture",
            FORMAT_LIMIT,
            |_, _| {
                calls += 1;
                Err("no answer (Operation not permitted)".into())
            },
            Instant::now,
            |_| panic!("must not retry a failed command"),
        )
        .unwrap_err();
        assert_eq!(calls, 1);
        assert!(error.ends_with("no answer (Operation not permitted)"));
    }

    #[test]
    fn time_spent_in_the_formatter_consumes_the_retry_budget() {
        let start = Instant::now();
        let elapsed = Cell::new(Duration::ZERO);
        let mut calls = 0;
        let error = format_with_clock(
            &mut command(),
            "/dev/rdisk-fixture",
            Duration::from_secs(1),
            |_, deadline| {
                assert_eq!(deadline, start + Duration::from_secs(1));
                elapsed.set(Duration::from_secs(1));
                calls += 1;
                Ok(output("newfs_apfs: /dev/rdisk-fixture: Resource busy"))
            },
            || start + elapsed.get(),
            |_| panic!("must not wait beyond the deadline"),
        )
        .unwrap_err();
        assert_eq!(calls, 1);
        assert!(error.contains("1 attempt(s) in 1.0 s"));
    }
}

/// Grows or shrinks the detached image `image` to `bytes`, with the
/// filesystem on it; its data is kept.
pub fn resize(image: &Path, bytes: u64) -> Result<(), String> {
    run(Command::new("diskutil")
        .args(["image", "resize", "--size"])
        .arg(bytes.to_string())
        .arg(image))
    .map(|_| ())
}

/// The format of the image file `image` (`UDRO`, `ULFO`, ...), as
/// `diskutil image info` names it.
pub fn format_of(image: &Path) -> Result<String, String> {
    let output = run(Command::new("diskutil").args(["image", "info"]).arg(image))?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|l| l.trim().strip_prefix("Image Format: "))
        .map(|f| f.trim().to_string())
        .ok_or_else(|| format!("{}: diskutil image info names no format", image.display()))
}

/// Writes `source` (an image) as a new image `destination` in `format`
/// (`ULFO`, `UDZO`, `ULMO`, `ASIF`...).
pub fn convert(source: &Path, destination: &Path, format: &str) -> Result<(), String> {
    run_within(
        Command::new("diskutil")
            .args(["image", "create", "from", "--format", format])
            .arg(source)
            .arg(destination),
        CONVERT_LIMIT,
    )
    .map(|_| ())
}

fn statfs(path: &Path) -> Option<libc::statfs> {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: NUL-terminated path, local buffer.
    (unsafe { libc::statfs(c.as_ptr(), &mut fs) } == 0).then_some(fs)
}

/// The mount point of the filesystem holding `path`, and its device.
pub fn mount_of(path: &Path) -> Option<(PathBuf, String)> {
    let fs = statfs(path)?;
    // SAFETY: statfs NUL-terminates both names.
    let (on, from) = unsafe {
        (
            CStr::from_ptr(fs.f_mntonname.as_ptr()),
            CStr::from_ptr(fs.f_mntfromname.as_ptr()),
        )
    };
    Some((
        PathBuf::from(on.to_string_lossy().into_owned()),
        from.to_string_lossy().into_owned(),
    ))
}

/// Whether a volume is mounted exactly at `path`.
pub fn is_mount_point(path: &Path) -> bool {
    let Ok(real) = std::fs::canonicalize(path) else {
        return false;
    };
    mount_of(&real).is_some_and(|(on, _)| on == real)
}

/// Bytes the file at `path` occupies on the host (a sparse image grows
/// and, where the volume inside returns space, shrinks).
pub fn allocated(path: &Path) -> std::io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path)?;
    if !meta.is_dir() {
        return Ok(meta.blocks() * 512);
    }
    let mut total = 0;
    for entry in std::fs::read_dir(path)? {
        total += allocated(&entry?.path())?;
    }
    Ok(total)
}

/// Bytes the files of the volume mounted at `mount` use. Not statfs's
/// blocks less free ones: APFS on a sparse image counts the host volume's
/// free space as its own, so that difference includes the host's use.
pub fn used(mount: &Path) -> Option<u64> {
    let c = std::ffi::CString::new(mount.as_os_str().as_encoded_bytes()).ok()?;
    let mut list = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: 0,
        volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_SPACEUSED,
        dirattr: 0,
        fileattr: 0,
        forkattr: 0,
    };
    #[repr(C, packed(4))]
    struct Reply {
        length: u32,
        used: libc::off_t,
    }
    let mut reply = Reply { length: 0, used: 0 };
    // SAFETY: NUL-terminated path, local attribute list and reply buffer.
    let status = unsafe {
        libc::getattrlist(
            c.as_ptr(),
            (&raw mut list).cast(),
            (&raw mut reply).cast(),
            std::mem::size_of::<Reply>(),
            0,
        )
    };
    (status == 0).then_some(reply.used as u64)
}

/// The size of the disk the filesystem holding `path` is on: the
/// outermost whole disk under its volume, through APFS containers and
/// their physical stores (for a data image's volume, the attached image).
pub fn block_device_size(path: &Path) -> Option<u64> {
    let (_, device) = mount_of(path)?;
    let name = std::ffi::CString::new(device.strip_prefix("/dev/")?).ok()?;
    registry::Object::bsd_media(&name)?.whole_disk_size()
}

/// The size of the filesystem holding `path` (for APFS, its container's).
pub fn capacity(path: &Path) -> Option<u64> {
    let fs = statfs(path)?;
    Some(fs.f_blocks * fs.f_bsize as u64)
}
