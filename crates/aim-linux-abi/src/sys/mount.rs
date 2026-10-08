//! Mount namespaces: `unshare(CLONE_NEWNS)`, `mount` and `umount2`.
//!
//! zygote unshares its mount namespace and mounts storage views for each
//! child (`MountEmulatedStorage`, app data isolation): bind mounts and
//! tmpfs. Here a process's mounts are entries of its own path table
//! (`vfs::add_mount`), inherited by fork and carried over exec
//! (`--mounts`), so every process behaves as if it had unshared: nothing
//! propagates to other processes. The other namespaces are answered
//! "unsupported" (ADR 0012 appendix).

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering::Relaxed};

use crate::errno::{EBUSY, EINVAL, ENODEV, ENOENT, ENOTDIR, EPERM};
use crate::vfs::{self, Area, LINUX_AT_FDCWD};

const CLONE_FILES: u64 = 0x400;
const CLONE_FS: u64 = 0x200;
const CLONE_NEWNS: u64 = 0x20000;
const CLONE_SYSVSEM: u64 = 0x40000;

const MS_RDONLY: u64 = 1;
const MS_REMOUNT: u64 = 32;
const MS_BIND: u64 = 4096;
const MS_MOVE: u64 = 8192;
const MS_UNBINDABLE: u64 = 1 << 17;
const MS_PRIVATE: u64 = 1 << 18;
const MS_SLAVE: u64 = 1 << 19;
const MS_SHARED: u64 = 1 << 20;
const PROPAGATION: u64 = MS_UNBINDABLE | MS_PRIVATE | MS_SLAVE | MS_SHARED;

const MNT_DETACH: u64 = 2;
const UMOUNT_NOFOLLOW: u64 = 8;

const CAP_SYS_ADMIN: u32 = 21;

pub fn unshare(a: [u64; 6]) -> i64 {
    // A process's files, cwd, SysV semaphore undo list and mount table are
    // already its own.
    if a[0] & !(CLONE_FILES | CLONE_FS | CLONE_SYSVSEM | CLONE_NEWNS) != 0 {
        return -(EINVAL as i64);
    }
    if a[0] & CLONE_NEWNS != 0 && !super::cred::capable(CAP_SYS_ADMIN) {
        return -(EPERM as i64);
    }
    if a[0]&CLONE_NEWNS!=0 {if let Err(error)=vfs::private_mount_namespace(){return -(error as i64);}}
    0
}

fn resolve(path: u64, follow: bool) -> Result<vfs::Resolved, i64> {
    // SAFETY: guest string.
    let path = unsafe { super::guest_cstr(path) };
    vfs::resolve(LINUX_AT_FDCWD, path, follow).map_err(|e| -(e as i64))
}

fn is_dir(r: &vfs::Resolved) -> Result<bool, i64> {
    if let Some(route)=vfs::fuse_route(&r.guest){let stat=super::fuse_client::stat(&route,None,None).map_err(|error|-(error as i64))?;return Ok(stat.st_mode&libc::S_IFMT==libc::S_IFDIR);}
    // SAFETY: host path, local buffer.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::stat(r.host.as_ptr(), &mut st) } != 0 {
        return Err(-(ENOENT as i64));
    }
    Ok(st.st_mode & libc::S_IFMT == libc::S_IFDIR)
}

/// A fresh host directory for a tmpfs mount, under the runtime directory
/// (per boot) or the host's temporary directory.
fn tmpfs_dir(data: &[u8]) -> Result<PathBuf, i64> {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let base = vfs::runtime_dir()
        .map(|r| r.join("tmpfs"))
        .unwrap_or_else(std::env::temp_dir);
    // SAFETY: trivial.
    let pid = unsafe { libc::getpid() };
    let dir = base.join(format!("{pid}.{}", SEQ.fetch_add(1, Relaxed)));
    std::fs::create_dir_all(&dir).map_err(|_| -(ENOENT as i64))?;
    let mode = String::from_utf8_lossy(data)
        .split(',')
        .find_map(|o| o.strip_prefix("mode="))
        .and_then(|m| u32::from_str_radix(m, 8).ok())
        .unwrap_or(0o1777);
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode & 0o7777));
    Ok(dir)
}

pub fn mount(a: [u64; 6]) -> i64 {
    match do_mount(a) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

fn do_mount(a: [u64; 6]) -> Result<(), i64> {
    let flags = a[3];
    if !super::cred::capable(CAP_SYS_ADMIN) {
        return Err(-(EPERM as i64));
    }
    let target = resolve(a[1], true)?;
    if !is_dir(&target)? && flags & (MS_BIND | MS_MOVE) == 0 {
        return Err(-(ENOTDIR as i64));
    }
    if flags & MS_REMOUNT != 0 {
        if vfs::remount_fuse(&target.guest,flags&MS_RDONLY!=0).map_err(|error|-(error as i64))?{return Ok(());}
        // Mount options (read-only, nosuid, ...) are not enforced.
        return Ok(());
    }
    if flags & PROPAGATION != 0 {
        return vfs::set_mount_propagation(&target.guest,flags).map_err(|error|-(error as i64));
    }
    if flags & MS_BIND != 0 {
        let source = resolve(a[0], true)?;
        if is_dir(&source)? != is_dir(&target)? {
            return Err(-(ENOTDIR as i64));
        }
        let area = if source.area == Area::Image || flags & MS_RDONLY != 0 {
            Area::Image
        } else {
            source.area
        };
        // Linux recursive bind clones visible submounts, including virtual
        // FUSE routes whose nodes do not exist in the host anchor tree.
        let host = PathBuf::from(OsStr::from_bytes(source.host.as_bytes()));
        vfs::bind_mount_recursive(&source.guest,&target.guest,host,area,flags&0x4000!=0).map_err(|error|-(error as i64))?;
        return Ok(());
    }
    if flags & MS_MOVE != 0 {
        let source = resolve(a[0], true)?;
        return if vfs::move_mount(&source.guest, &target.guest) {
            Ok(())
        } else {
            Err(-(EINVAL as i64))
        };
    }
    // SAFETY: guest strings.
    let fstype = unsafe { super::guest_cstr(a[2]) };
    match fstype {
        b"fuse"|b"fuse.media"=>{
            let data=unsafe{super::guest_cstr(a[4])};let options=super::fuse_mount::parse(data).map_err(|error|-(error as i64))?;
            let session=super::fuse::session_for_fd(options.fd).map_err(|error|-(error as i64))?;
            super::fuse::Client::from_key(&session).and_then(|client|client.mount()).map_err(|error|-(error as i64))?;
            let source=unsafe{super::guest_cstr(a[0])};let source=std::str::from_utf8(source).map_err(|_|-(EINVAL as i64))?;
            vfs::add_fuse_mount(&target.guest,session.transport().to_path_buf(),PathBuf::from(OsStr::from_bytes(target.host.as_bytes())),source,&options,flags&MS_RDONLY!=0).map_err(|error|-(error as i64))?;
            match super::fuse_sysfs::mounted(&session){Ok(_)=>Ok(()),Err(error)=>{vfs::remove_mount(&target.guest);Err(-(error as i64))}}
        }
        b"tmpfs" => {
            // SAFETY: guest string (options), may be null.
            let data = unsafe { super::guest_cstr(a[4]) };
            let dir = tmpfs_dir(data)?;
            let area = if flags & MS_RDONLY != 0 {
                Area::Image
            } else {
                Area::Writable
            };
            vfs::add_mount(&target.guest, dir, area, "tmpfs", "tmpfs");
            Ok(())
        }
        _ => Err(-(ENODEV as i64)),
    }
}

pub fn umount2(a: [u64; 6]) -> i64 {
    if a[1] & !(MNT_DETACH | UMOUNT_NOFOLLOW | 1 | 4) != 0 {
        return -(EINVAL as i64);
    }
    if !super::cred::capable(CAP_SYS_ADMIN) {
        return -(EPERM as i64);
    }
    let target = match resolve(a[0], a[1] & UMOUNT_NOFOLLOW == 0) {
        Ok(t) => t,
        Err(e) => return e,
    };
    if vfs::remove_mount(&target.guest) {
        0
    } else if target.guest == "/" {
        -(EBUSY as i64)
    } else {
        -(EINVAL as i64)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use super::*;
    use crate::sys::{attrs, fsops};

    fn p(s: &CString) -> u64 {
        s.as_ptr() as u64
    }

    fn at(path: &str) -> CString {
        CString::new(path).unwrap()
    }

    fn owner(path: &str) -> Option<u32> {
        let r = vfs::resolve(LINUX_AT_FDCWD, path.as_bytes(), true).unwrap();
        attrs::lookup(attrs::Host::Path(&r.host), || r.guest.clone()).uid
    }

    fn host(path: &str) -> PathBuf {
        let r = vfs::resolve(LINUX_AT_FDCWD, path.as_bytes(), true).unwrap();
        PathBuf::from(OsStr::from_bytes(r.host.as_bytes()))
    }

    fn mkdir(path: &str, mode: u64, uid: u64) {
        let c = at(path);
        let fd = LINUX_AT_FDCWD as u64;
        assert_eq!(
            fsops::mkdirat([fd, p(&c), mode, 0, 0, 0]),
            0,
            "mkdir {path}"
        );
        assert_eq!(
            fsops::fchownat([fd, p(&c), uid, uid, 0, 0]),
            0,
            "chown {path}"
        );
    }

    fn bind(source: &str, target: &str) {
        let (s, t) = (at(source), at(target));
        assert_eq!(
            mount([p(&s), p(&t), 0, MS_BIND | 0x4000, 0, 0]),
            0,
            "bind {target}"
        );
    }

    /// Zygote's `isolateAppData` for one app of user 0, as the original
    /// runs it in the app's child (Zygote.cpp, Android 16), after installd
    /// made the app's data directories.
    #[test]
    fn zygote_isolates_app_data() {
        let (_view, _) = vfs::test_view();
        const APP: u64 = 10123;
        // installd: create_app_data through /data/data (a directory).
        // init.rc and vold made these.
        for dir in ["/data/data", "/data/user", "/data/user_de/0"] {
            std::fs::create_dir_all(host(dir)).unwrap();
        }
        mkdir("/data/data/com.example", 0o700, APP);
        mkdir("/data/user_de/0/com.example", 0o700, APP);
        mkdir("/data/data/com.other", 0o700, APP + 1);
        // The same directory, whichever mount reaches it.
        assert_eq!(owner("/data/user/0/com.example"), Some(APP as u32));
        assert_eq!(
            owner("/data_mirror/data_ce/null/0/com.example"),
            Some(APP as u32)
        );

        assert_eq!(unshare([CLONE_NEWNS, 0, 0, 0, 0, 0]), 0);
        let (tmpfs, opts) = (at("tmpfs"), at("uid=0,gid=0,mode=0751"));
        for dir in ["/data/data", "/data/user", "/data/user_de"] {
            let t = at(dir);
            let flags = 0x2 | 0x4 | 0x8; // MS_NOSUID | MS_NODEV | MS_NOEXEC
            assert_eq!(mount([p(&tmpfs), p(&t), p(&tmpfs), flags, p(&opts), 0]), 0);
        }
        // The tmpfs over /data/user hides the path map's /data/user/0.
        let (target, link) = (at("/data/data"), at("/data/user/0"));
        let fd = LINUX_AT_FDCWD as u64;
        assert_eq!(fsops::symlinkat([p(&target), fd, p(&link), 0, 0, 0]), 0);
        mkdir("/data/user_de/0", 0o711, 0);
        // DE, then CE (getAppDataDirName finds the directory by name).
        mkdir("/data/user_de/0/com.example", 0o700, 0);
        bind(
            "/data_mirror/data_de/null/0/com.example",
            "/data/user_de/0/com.example",
        );
        assert!(host("/data_mirror/data_ce/null/0/com.example").is_dir());
        mkdir("/data/data/com.example", 0o700, 0);
        bind(
            "/data_mirror/data_ce/null/0/com.example",
            "/data/data/com.example",
        );

        // The app: its data directory is ApplicationInfo.dataDir, and its
        // databases are SQLite files created there.
        let data = host("/data_mirror/data_ce/null/0/com.example");
        assert_eq!(host("/data/user/0/com.example"), data);
        assert_eq!(owner("/data/user/0/com.example"), Some(APP as u32));
        assert_eq!(owner("/data/user_de/0/com.example"), Some(APP as u32));
        mkdir("/data/user/0/com.example/databases", 0o771, APP);
        let db = at("/data/user/0/com.example/databases/contacts2.db");
        let flags = 0o2 | 0o100 | 0o2000000; // O_RDWR | O_CREAT | O_CLOEXEC
        let fd = super::super::fs::openat([fd, p(&db), flags, 0o660, 0, 0]);
        assert!(fd >= 0, "open: {fd}");
        // SAFETY: the fd just opened.
        unsafe { libc::close(fd as i32) };
        assert!(data.join("databases/contacts2.db").is_file());
        // Other apps' directories are hidden.
        assert!(!host("/data/user/0/com.other").exists());
        assert!(!host("/data/data/com.other").exists());

        for dir in [
            "/data/data/com.example",
            "/data/user_de/0/com.example",
            "/data/user_de",
            "/data/user",
            "/data/data",
        ] {
            let t = at(dir);
            assert_eq!(umount2([p(&t), MNT_DETACH, 0, 0, 0, 0]), 0, "umount {dir}");
        }
        // The stubs zygote gave root's owner did not change the app's.
        assert_eq!(owner("/data/data/com.example"), Some(APP as u32));
        assert_eq!(owner("/data/user/0/com.example"), Some(APP as u32));
    }
}
