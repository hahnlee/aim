//! Directories vold prepares, as the original's FsCrypt.cpp and
//! VolumeManager.cpp lay them out. /data is a host directory without file
//! encryption, so there are no keys or policies: preparing storage is
//! making the directories with their modes and owners.

use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const AID_ROOT: u32 = 0;
pub const AID_SYSTEM: u32 = 1000;
pub const AID_MEDIA_RW: u32 = 1023;
pub const AID_SHELL: u32 = 2000;
pub const AID_EVERYBODY: u32 = 9997;
pub const AID_MISC: u32 = 9998;
pub const AID_USER_OFFSET: u32 = 100_000;

pub const STORAGE_FLAG_DE: i32 = 1;
pub const STORAGE_FLAG_CE: i32 = 2;

/// fs_prepare_dir: make `path` if needed and give it `mode` and owners.
pub fn prepare_dir(path: &Path, mode: u32, uid: u32, gid: u32) -> io::Result<()> {
    match std::fs::DirBuilder::new().mode(mode & 0o777).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    std::os::unix::fs::chown(path, Some(uid), Some(gid))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

/// A symlink `link` -> `target`, replacing a different one, or the empty
/// directories init.rc makes where vold would mount (`mkdir
/// /mnt/pass_through/0/emulated/0`).
pub fn link(target: &str, link: &Path) -> io::Result<()> {
    if std::fs::read_link(link).is_ok_and(|t| t == Path::new(target)) {
        return Ok(());
    }
    match std::fs::symlink_metadata(link) {
        Ok(m) if m.is_dir() => remove_empty_dirs(link)?,
        Ok(_) => std::fs::remove_file(link)?,
        Err(_) => {}
    }
    std::os::unix::fs::symlink(target, link)
}

/// Remove a tree that holds nothing but directories; fail on anything else.
fn remove_empty_dirs(dir: &Path) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if std::fs::symlink_metadata(&path)?.is_dir() {
            remove_empty_dirs(&path)?;
        }
    }
    std::fs::remove_dir(dir)
}

/// Where a volume's per-user data lives: internal storage (`/data`) or an
/// adopted volume (`/mnt/expand/<uuid>`).
fn volume_root(uuid: Option<&str>) -> PathBuf {
    match uuid.filter(|u| !u.is_empty()) {
        Some(u) => PathBuf::from(format!("/mnt/expand/{u}")),
        None => PathBuf::from("/data"),
    }
}

/// `fscrypt_prepare_user_storage` without keys. Returns the prepared
/// directories.
pub fn prepare_user_storage(uuid: Option<&str>, user: u32, flags: i32) -> io::Result<Vec<PathBuf>> {
    let internal = uuid.is_none_or(str::is_empty);
    let root = volume_root(uuid);
    let mut dirs: Vec<(PathBuf, u32, u32, u32)> = Vec::new();
    if flags & STORAGE_FLAG_DE != 0 {
        if internal {
            dirs.push((
                format!("/data/system/users/{user}").into(),
                0o700,
                AID_SYSTEM,
                AID_SYSTEM,
            ));
            dirs.push((
                format!("/data/misc/profiles/cur/{user}").into(),
                0o771,
                AID_SYSTEM,
                AID_SYSTEM,
            ));
            dirs.push((
                format!("/data/system_de/{user}").into(),
                0o770,
                AID_SYSTEM,
                AID_SYSTEM,
            ));
            dirs.push((
                format!("/data/vendor_de/{user}").into(),
                0o771,
                AID_ROOT,
                AID_ROOT,
            ));
        }
        dirs.push((
            root.join(format!("misc_de/{user}")),
            0o1771,
            AID_SYSTEM,
            AID_MISC,
        ));
        dirs.push((
            root.join(format!("user_de/{user}")),
            0o771,
            AID_SYSTEM,
            AID_SYSTEM,
        ));
    }
    if flags & STORAGE_FLAG_CE != 0 {
        if internal {
            dirs.push((
                format!("/data/system_ce/{user}").into(),
                0o770,
                AID_SYSTEM,
                AID_SYSTEM,
            ));
            dirs.push((
                format!("/data/vendor_ce/{user}").into(),
                0o771,
                AID_ROOT,
                AID_ROOT,
            ));
        }
        dirs.push((
            root.join(format!("media/{user}")),
            0o2770,
            AID_MEDIA_RW,
            AID_MEDIA_RW,
        ));
        dirs.push((
            root.join(format!("misc_ce/{user}")),
            0o1771,
            AID_SYSTEM,
            AID_MISC,
        ));
        // User 0's internal CE app data is /data/data (bound on /data/user/0).
        let user_ce = if internal && user == 0 {
            PathBuf::from("/data/data")
        } else {
            root.join(format!("user/{user}"))
        };
        dirs.push((user_ce, 0o771, AID_SYSTEM, AID_SYSTEM));
    }
    for (path, mode, uid, gid) in &dirs {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        prepare_dir(path, *mode, *uid, *gid)?;
    }
    // FsCrypt.cpp delegates per-user subdirectories to the original helper.
    let status = std::process::Command::new("/system/bin/vold_prepare_subdirs")
        .args([
            "prepare",
            uuid.unwrap_or(""),
            &user.to_string(),
            &flags.to_string(),
        ])
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!("vold_prepare_subdirs: {status}")));
    }
    Ok(dirs.into_iter().map(|d| d.0).collect())
}

/// `fscrypt_init_user0` without keys: `prepare_special_dirs` and user 0's
/// DE storage. The original bind-mounts /data/data onto /data/user/0, a
/// mount /data's shared propagation shows every process and init's data
/// mirror (zygote finds apps' CE storage there). Mounts on the syscall
/// layer are per process, so that bind is an entry of guest-init's path
/// map, and vold checks that it is there.
pub fn init_user0() -> io::Result<Vec<PathBuf>> {
    prepare_dir(Path::new("/data/data"), 0o771, AID_SYSTEM, AID_SYSTEM)?;
    let (data, user0) = (
        std::fs::metadata("/data/data")?,
        std::fs::metadata("/data/user/0")?,
    );
    if (data.dev(), data.ino()) != (user0.dev(), user0.ino()) {
        return Err(io::Error::other(
            "/data/user/0 is not /data/data: the path map has no bind for it",
        ));
    }
    prepare_dir(Path::new("/data/media"), 0o770, AID_MEDIA_RW, AID_MEDIA_RW)?;
    prepare_dir(
        Path::new("/data/media/obb"),
        0o770,
        AID_MEDIA_RW,
        AID_MEDIA_RW,
    )?;
    prepare_user_storage(None, 0, STORAGE_FLAG_DE)
}

/// The views of the emulated volume of `user` that VolumeManager and
/// EmulatedVolume set up with mounts, as symlinks into `/data/media`:
/// - `/storage/emulated` for processes that see the whole /storage;
/// - `/mnt/user/<user>/emulated` and friends, which zygote bind-mounts over
///   /storage for each app (by its mount mode);
/// - `/mnt/user/<user>/self/primary`, the primary volume of that user.
pub fn link_emulated(user: u32) -> io::Result<()> {
    let everybody = user * AID_USER_OFFSET + AID_EVERYBODY;
    link("/data/media", Path::new("/storage/emulated"))?;
    for view in ["user", "pass_through", "installer", "androidwritable"] {
        let dir = PathBuf::from(format!("/mnt/{view}/{user}"));
        std::fs::create_dir_all(&dir)?;
        let owner = if user == 0 { AID_SHELL } else { AID_ROOT };
        prepare_dir(&dir, 0o710, owner, everybody)?;
        link("/data/media", &dir.join("emulated"))?;
    }
    let slf = PathBuf::from(format!("/mnt/user/{user}/self"));
    prepare_dir(&slf, 0o755, AID_ROOT, AID_ROOT)?;
    link(&format!("/storage/emulated/{user}"), &slf.join("primary"))
}

/// The lower (data) path of an app directory named under a volume's
/// visible path, `/storage/emulated/<user>/Android/...`.
pub fn lower_path(path: &str) -> Option<PathBuf> {
    let rest = path.strip_prefix("/storage/emulated/")?;
    Some(PathBuf::from(format!("/data/media/{rest}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_dirs_live_in_data_media() {
        assert_eq!(
            lower_path("/storage/emulated/0/Android/data/com.example"),
            Some(PathBuf::from("/data/media/0/Android/data/com.example"))
        );
        assert_eq!(lower_path("/mnt/expand/x"), None);
    }

    #[test]
    fn prepared_dirs_get_modes() {
        let dir = std::env::temp_dir().join(format!("vold-test-{}", std::process::id()));
        let d = dir.join("x");
        std::fs::create_dir_all(&dir).unwrap();
        // Owner changes need root; keep our own ids.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        prepare_dir(&d, 0o1771, uid, gid).unwrap();
        prepare_dir(&d, 0o770, uid, gid).unwrap();
        let mode = std::fs::metadata(&d).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o770);
        link("/a", &dir.join("l")).unwrap();
        link("/b", &dir.join("l")).unwrap();
        assert_eq!(std::fs::read_link(dir.join("l")).unwrap(), Path::new("/b"));
        // init.rc's empty directories give way; a file in them does not.
        std::fs::create_dir_all(dir.join("m/0")).unwrap();
        link("/c", &dir.join("m")).unwrap();
        assert_eq!(std::fs::read_link(dir.join("m")).unwrap(), Path::new("/c"));
        std::fs::create_dir_all(dir.join("n/0")).unwrap();
        std::fs::write(dir.join("n/0/f"), b"").unwrap();
        assert!(link("/c", &dir.join("n")).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
