//! The macOS disk image tools, run as the user (no admin rights):
//! `diskutil image` creates, converts and attaches images, `newfs_apfs -e`
//! makes a case-sensitive APFS volume on a blank one, `diskutil eject`
//! detaches, and `hdiutil info` lists what is attached.

use std::ffi::CStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

/// An attached image, as `hdiutil info` lists it.
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

fn run(command: &mut Command) -> Result<Output, String> {
    let output = command
        .output()
        .map_err(|e| format!("{:?}: {e}", command.get_program()))?;
    if output.status.success() {
        Ok(output)
    } else {
        let mut text = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !stdout.trim().is_empty() {
            text = format!("{} {text}", stdout.trim());
        }
        let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy()).collect();
        Err(format!(
            "{} {}: {text}",
            command.get_program().to_string_lossy(),
            args.join(" ")
        ))
    }
}

/// Parses `hdiutil info`: blocks of `key : value` lines, then the image's
/// devices as `device<TAB>hint<TAB>mount point` lines.
pub fn parse_info(text: &str) -> Vec<Attached> {
    let mut out = Vec::new();
    for block in text
        .split("================================================")
        .skip(1)
    {
        let mut attached = Attached {
            image: PathBuf::new(),
            shadow: None,
            writable: false,
            device: String::new(),
            devices: Vec::new(),
            mounts: Vec::new(),
        };
        for line in block.lines() {
            if line.starts_with("/dev/") {
                let mut fields = line.split('\t');
                let device = fields.next().unwrap_or("").trim();
                if attached.device.is_empty() {
                    attached.device = device.to_string();
                }
                attached.devices.push(device.to_string());
                if let Some(mount) = fields.nth(1).map(str::trim).filter(|m| !m.is_empty()) {
                    attached.mounts.push(PathBuf::from(mount));
                }
            } else if let Some((key, value)) = line.split_once(':') {
                let value = value.trim();
                match key.trim() {
                    "image-path" => attached.image = PathBuf::from(value),
                    "shadow-path" if value != "<none>" => {
                        attached.shadow = Some(PathBuf::from(value))
                    }
                    "writeable" => attached.writable = value.eq_ignore_ascii_case("true"),
                    _ => {}
                }
            }
        }
        if !attached.device.is_empty() {
            out.push(attached);
        }
    }
    out
}

/// Every attached image.
pub fn attached() -> Result<Vec<Attached>, String> {
    let output = run(Command::new("hdiutil").arg("info"))?;
    Ok(parse_info(&String::from_utf8_lossy(&output.stdout)))
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

/// Attaches `image` by its real path, which is what `hdiutil info` then
/// lists ([`attachments_of`]); returns its device.
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

/// Detaches the image of `device`. A volume in use is retried for
/// `patience` (the Mac's security agent scans freshly written files for a
/// while), then unmounted by force.
pub fn detach(device: &str, patience: Duration) -> Result<(), String> {
    let deadline = Instant::now() + patience;
    loop {
        match run(Command::new("diskutil").args(["eject", device])) {
            Ok(_) => return Ok(()),
            Err(e) if Instant::now() < deadline => {
                if !e.contains("could not be unmounted") && !e.contains("dissented") {
                    return Err(e);
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(e) => {
                eprintln!("aim-storage: {e}; unmounting by force");
                run(Command::new("diskutil").args(["unmountDisk", "force", device]))?;
                run(Command::new("diskutil").args(["eject", device]))?;
                return Ok(());
            }
        }
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
    let formatted = run(Command::new("newfs_apfs").args(["-e", "-v", volume, &raw]));
    let detached = detach(&device, Duration::from_secs(10));
    formatted.and(detached).map(|_| ())
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
    run(Command::new("diskutil")
        .args(["image", "create", "from", "--format", format])
        .arg(source)
        .arg(destination))
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

/// The size of the filesystem holding `path` (for APFS, its container's).
pub fn capacity(path: &Path) -> Option<u64> {
    let fs = statfs(path)?;
    Some(fs.f_blocks * fs.f_bsize as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_lists_images_devices_and_mounts() {
        let text = "framework       : 704\n\
            images          : 2\n\
            ================================================\n\
            image-path      : /a/sys.dmg\n\
            shadow-path     : /a/d.shadow\n\
            writeable       : false\n\
            /dev/disk8\t\t\n\
            /dev/disk9\tEF57347C-0000-11AA-AA11-00306543ECAC\t\n\
            /dev/disk9s1\t41504653-0000-11AA-AA11-00306543ECAC\t/a/mnt one\n\
            ================================================\n\
            image-path      : /a/data.asif\n\
            shadow-path     : <none>\n\
            writeable       : TRUE\n\
            /dev/disk4\t\t\n";
        let got = parse_info(text);
        assert_eq!(
            got,
            [
                Attached {
                    image: "/a/sys.dmg".into(),
                    shadow: Some("/a/d.shadow".into()),
                    writable: false,
                    device: "/dev/disk8".into(),
                    devices: vec![
                        "/dev/disk8".into(),
                        "/dev/disk9".into(),
                        "/dev/disk9s1".into()
                    ],
                    mounts: vec!["/a/mnt one".into()],
                },
                Attached {
                    image: "/a/data.asif".into(),
                    shadow: None,
                    writable: true,
                    device: "/dev/disk4".into(),
                    devices: vec!["/dev/disk4".into()],
                    mounts: Vec::new(),
                },
            ]
        );
    }
}
