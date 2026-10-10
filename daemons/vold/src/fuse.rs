//! EmulatedVolume / Utils MountUserFuse, android-16.0.0_r1 (AOSP Apache-2.0).
//! The original MediaProvider owns the daemon. This module owns only mounts.
use crate::storage::{self, AID_EVERYBODY, AID_MEDIA_RW, AID_ROOT, AID_SHELL, AID_USER_OFFSET};
use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

pub struct Mount {
    user: u32,
    mounts: Vec<PathBuf>,
}
impl Mount {
    pub fn new(user: u32) -> Self {
        Self {
            user,
            mounts: Vec::new(),
        }
    }
    pub fn user(&self) -> u32 {
        self.user
    }
    pub fn begin(&mut self) -> io::Result<File> {
        prepare_user_views(self.user)?;
        let fuse = PathBuf::from(format!("/mnt/user/{}/emulated", self.user));
        let pass = PathBuf::from(format!("/mnt/pass_through/{}", self.user));
        let lower = PathBuf::from(format!("/data/media/{}", self.user));
        for (path, gid) in [
            ("Android", AID_MEDIA_RW),
            ("Android/data", 1078),
            ("Android/obb", 1079),
            ("Android/media", AID_MEDIA_RW),
        ] {
            storage::prepare_dir(&lower.join(path), 0o2771, AID_MEDIA_RW, gid)?;
        }
        default_obb_acl(&lower.join("Android/obb"))?;
        // NO default_permissions: original Utils1667 routes permission decisions
        // through MediaProvider instead of checking the restricted lower tree.
        let fd = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_CLOEXEC)
            .open("/dev/fuse")?;
        let options = format!(
            "fd={},rootmode=40000,allow_other,user_id=0,group_id=0,",
            fd.as_raw_fd()
        );
        self.mount(
            "/dev/fuse",
            &fuse,
            Some("fuse"),
            libc::MS_NOSUID | libc::MS_NODEV | libc::MS_NOEXEC | libc::MS_NOATIME | (1 << 25),
            Some(&options),
        )?;
        // This raw bind is exclusively the trusted daemon's lower filesystem.
        self.mount(
            "/data/media",
            &pass.join("emulated"),
            None,
            libc::MS_BIND,
            None,
        )?;
        let source = fs::metadata("/data/media")?;
        let view = fs::metadata(pass.join("emulated"))?;
        if (source.dev(), source.ino(), source.uid(), source.gid())
            != (view.dev(), view.ino(), view.uid(), view.gid())
        {
            return Err(io::Error::from_raw_os_error(libc::EIO));
        }
        log::info!(
            "trusted MediaProvider lower view /mnt/pass_through/{}/emulated bound to /data/media (same guest inode/owner)",
            self.user
        );
        Ok(fd)
    }
    /// Run after onVolumeChecking acknowledged a live original FUSE daemon.
    pub fn finish(&mut self) -> io::Result<()> {
        for name in ["data", "obb"] {
            let source = format!("/data/media/{}/Android/{name}", self.user);
            let target = PathBuf::from(format!(
                "/mnt/user/{}/emulated/{}/Android/{name}",
                self.user, self.user
            ));
            if let Err(error) = self.mount(&source, &target, None, libc::MS_BIND, None) {
                log::error!(
                    "FUSE Android bind {source} -> {}: {error}",
                    target.display()
                );
                return Err(error);
            }
        }
        // Installer and Android-writable zygote modes bind these actual views
        // onto /storage. Clone the ready FUSE tree (including its real nested
        // binds), never a link to /data/media or an empty mount placeholder.
        let source = format!("/mnt/user/{}", self.user);
        for view in ["installer", "androidwritable"] {
            let target = PathBuf::from(format!("/mnt/{view}/{}", self.user));
            self.mount(&source, &target, None, libc::MS_BIND | libc::MS_REC, None)?;
        }
        Ok(())
    }
    fn mount(
        &mut self,
        source: &str,
        target: &Path,
        kind: Option<&str>,
        flags: libc::c_ulong,
        data: Option<&str>,
    ) -> io::Result<()> {
        let source = cstring(source)?;
        let target_c = cstring(
            target
                .to_str()
                .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?,
        )?;
        let kind = kind.map(cstring).transpose()?;
        let data = data.map(cstring).transpose()?;
        loop {
            let result = unsafe {
                libc::mount(
                    source.as_ptr(),
                    target_c.as_ptr(),
                    kind.as_ref().map_or(std::ptr::null(), |kind| kind.as_ptr()),
                    flags,
                    data.as_ref()
                        .map_or(std::ptr::null(), |data| data.as_ptr().cast()),
                )
            };
            if result == 0 {
                self.mounts.push(target.to_owned());
                return Ok(());
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
    }
    pub fn unmount(&mut self) -> io::Result<()> {
        let mut failed = Vec::new();
        let mut first = None;
        for path in self.mounts.iter().rev() {
            let target = cstring(
                path.to_str()
                    .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?,
            )?;
            let result = unsafe { libc::umount2(target.as_ptr(), 8) }; // UMOUNT_NOFOLLOW
            if result != 0 {
                let error = io::Error::last_os_error();
                // Original UnmountUserFuse uses MNT_DETACH on busy FUSE mounts.
                if error.raw_os_error() == Some(libc::EBUSY)
                    && unsafe { libc::umount2(target.as_ptr(), 8 | libc::MNT_DETACH) } == 0
                {
                    continue;
                }
                let error = if error.raw_os_error() == Some(libc::EBUSY) {
                    io::Error::last_os_error()
                } else {
                    error
                };
                failed.push(path.clone());
                if first.is_none() {
                    first = Some(error);
                }
            }
        }
        failed.reverse();
        self.mounts = failed;
        first.map_or(Ok(()), Err)
    }
}
/// The real zygote consumers need their view roots before storage mounts finish.
/// These are initially empty directories; readiness is published only once the
/// original MediaProvider FUSE tree and recursive view mounts actually exist.
pub fn prepare_user_views(user: u32) -> io::Result<()> {
    remove_legacy_raw_views(user)?;
    let everybody = user
        .checked_mul(AID_USER_OFFSET)
        .and_then(|user| user.checked_add(AID_EVERYBODY))
        .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?;
    mount_dir(Path::new("/mnt/user"), 0o750, AID_ROOT, AID_MEDIA_RW)?;
    for view in ["user", "installer", "androidwritable"] {
        if view != "user" {
            mount_dir(
                &PathBuf::from(format!("/mnt/{view}")),
                0o700,
                AID_ROOT,
                AID_ROOT,
            )?;
        }
        let root = PathBuf::from(format!("/mnt/{view}/{user}"));
        mount_dir(
            &root,
            0o710,
            if user == 0 { AID_SHELL } else { AID_ROOT },
            everybody,
        )?;
        mount_dir(&root.join("emulated"), 0o700, AID_ROOT, AID_ROOT)?;
        mount_dir(&root.join("self"), 0o755, AID_ROOT, AID_ROOT)?;
        storage::link(
            &format!("/storage/emulated/{user}"),
            &root.join("self/primary"),
        )?;
    }
    mount_dir(Path::new("/mnt/pass_through"), 0o700, AID_ROOT, AID_ROOT)?;
    let pass = PathBuf::from(format!("/mnt/pass_through/{user}"));
    mount_dir(&pass, 0o710, AID_ROOT, AID_MEDIA_RW)?;
    mount_dir(&pass.join("emulated"), 0o710, AID_ROOT, AID_MEDIA_RW)?;
    mount_dir(&pass.join("self"), 0o710, AID_ROOT, AID_MEDIA_RW)?;
    storage::link(
        &format!("/storage/emulated/{user}"),
        &pass.join("self/primary"),
    )
}

fn remove_legacy_raw_views(user: u32) -> io::Result<()> {
    let mut paths = vec![PathBuf::from("/storage/emulated")];
    for view in ["installer", "androidwritable"] {
        paths.push(PathBuf::from(format!("/mnt/{view}/{user}/emulated")));
    }
    for path in paths {
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                if fs::read_link(&path)? != Path::new("/data/media") {
                    return Err(io::Error::from_raw_os_error(libc::ELOOP));
                }
                fs::remove_file(&path)?;
                storage::prepare_dir(&path, 0o700, AID_ROOT, AID_ROOT)?;
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
fn default_obb_acl(path: &Path) -> io::Result<()> {
    // Original Utils::SetDefaultAcl(mode=02771, media_rw, ext_obb_rw, {}).
    let mut acl = Vec::with_capacity(28);
    acl.extend_from_slice(&2u32.to_le_bytes());
    for (tag, permissions, id) in [(1u16, 7u16, AID_MEDIA_RW), (4, 7, 1079), (32, 1, 0)] {
        acl.extend_from_slice(&tag.to_le_bytes());
        acl.extend_from_slice(&permissions.to_le_bytes());
        acl.extend_from_slice(&id.to_le_bytes());
    }
    let path = cstring(
        path.to_str()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EINVAL))?,
    )?;
    let result = unsafe {
        libc::setxattr(
            path.as_ptr(),
            c"system.posix_acl_default".as_ptr(),
            acl.as_ptr().cast(),
            acl.len(),
            0,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
fn cstring(value: &str) -> io::Result<CString> {
    CString::new(value).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))
}
fn mount_dir(path: &Path, mode: u32, uid: u32, gid: u32) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            // Replace only the previous native vold's explicitly owned raw view.
            if fs::read_link(path)? != Path::new("/data/media") {
                return Err(io::Error::from_raw_os_error(libc::ELOOP));
            }
            fs::remove_file(path)?;
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(io::Error::from_raw_os_error(libc::ENOTDIR));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    storage::prepare_dir(path, mode, uid, gid)
}
