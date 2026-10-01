//! Guest path view: the guest's `/` is a host directory (`--root`), with
//! writable areas mapped over it by a path map (`--path-map`,
//! `docs/guest-init-contract.md` section 2).
//!
//! - The longest matching guest prefix wins, on whole components; the host
//!   device nodes in [`HOST_DEVICES`] and the pty slaves `/dev/pts/N`
//!   (`sys::tty`) take precedence over any `/dev` entry.
//! - Paths are resolved component by component with symlinks interpreted
//!   relative to the guest root, the way a chroot would, so absolute links
//!   inside the image (for example `/bin -> /system/bin`) stay inside it.
//!   Each intermediate result goes through the map again. Links under
//!   `/proc` are synthesized and not followed here.
//! - The inverse map (host path -> guest path) uses the same table.
//! - With a path map, the image root is read-only to the guest.
//! - `/dev/input` is the display server's device directory
//!   ([`set_input_dir`], `sys::evdev`).
//! - The process's own mounts (`mount(2)` with `MS_BIND` or `tmpfs`,
//!   `sys/mount.rs`) are entries over the same table. They belong to this
//!   process and its children, as after `unshare(CLONE_NEWNS)`. A mount
//!   hides the older ones at and below its mount point, as mounting over a
//!   directory does (zygote's tmpfs over `/data/user` hides the path map's
//!   `/data/user/0`).

use std::collections::HashMap;
use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};

use crate::errno::{self, Errno};

pub const LINUX_AT_FDCWD: i32 = -100;
const MAX_SYMLINKS: usize = 40;

/// Host device nodes the guest sees at the same path.
pub const HOST_DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/tty",
    "/dev/ptmx",
];

/// Where a guest path lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    /// The image root: read-only when a path map is in use.
    Image,
    /// A writable host directory or file (`rw`).
    Writable,
    /// `/proc` or `/sys`: synthesized; the host tree holds values init wrote.
    Kernfs,
    /// A host device node passed through.
    HostDevice,
    /// `/dev/input`: the display server's input devices.
    Input,
}

struct Mount {
    /// Normalized absolute guest path, no trailing `/`.
    guest: String,
    host: PathBuf,
    area: Area,
    /// What `/proc/mounts` shows; None for a plain directory of the path
    /// map (procfs names those).
    source: Option<String>,
    fstype: Option<String>,
    /// A mount this process made (not a path map entry).
    own: bool,
    /// Mount order: 0 for the path map, then increasing.
    seq: u64,
}

struct Vfs {
    root: PathBuf,
    /// Whether a path map is in use (the image is then read-only).
    mapped: bool,
    /// The device of the root when it is a read-only mount (the image's).
    read_only_dev: Option<i32>,
    /// Longest guest prefix first; among equal prefixes the latest mount.
    mounts: RwLock<Vec<Mount>>,
    /// Directory holding the path map (the guest-init runtime directory).
    runtime: Option<PathBuf>,
    cwd: Mutex<String>,
}

static VFS: OnceLock<Vfs> = OnceLock::new();
static INPUT: OnceLock<PathBuf> = OnceLock::new();

/// Show host directory `dir` as the guest's `/dev/input`. Before [`init`].
pub fn set_input_dir(dir: &Path) {
    let _ = INPUT.set(dir.to_owned());
}

/// The host directory behind `/dev/input`, if any.
pub fn input_dir() -> Option<&'static Path> {
    INPUT.get().map(PathBuf::as_path)
}

/// Parse a path map file: `kind<TAB>guest<TAB>host` lines, kinds `root`,
/// `rw` and `kernfs`; `#` starts a comment line.
fn parse_map(text: &str) -> Result<(Option<PathBuf>, Vec<Mount>), String> {
    let mut root = None;
    let mut mounts = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let [kind, guest, host] = f[..] else {
            return Err(format!("line {}: expected 3 tab-separated fields", n + 1));
        };
        let area = match kind {
            "root" => {
                root = Some(PathBuf::from(host));
                continue;
            }
            "rw" | "cgroup2" | "bpf" => Area::Writable,
            "kernfs" => Area::Kernfs,
            other => return Err(format!("line {}: unknown kind '{other}'", n + 1)),
        };
        let guest = guest.trim_end_matches('/');
        if !guest.starts_with('/') {
            return Err(format!("line {}: guest path must be absolute", n + 1));
        }
        // A cgroup v2 hierarchy is a plain directory: groups can be made
        // and joined, but no controller acts on them.
        let fstype = matches!(kind, "cgroup2" | "bpf").then(|| kind.to_string());
        mounts.push(Mount {
            guest: guest.to_string(),
            host: PathBuf::from(host),
            area,
            source: fstype
                .as_deref()
                .map(|t| if t == "bpf" { "bpf" } else { "none" }.to_string()),
            fstype,
            own: false,
            seq: 0,
        });
    }
    mounts.sort_by(|a, b| b.guest.len().cmp(&a.guest.len()));
    Ok((root, mounts))
}

/// Initialize from `--root` and, when given, a `--path-map` file (whose
/// `root` line overrides `root`).
pub fn init(root: &Path, map: Option<&Path>) -> Result<(), String> {
    let (map_root, mut mounts, runtime) = match map {
        Some(map) => {
            let text =
                std::fs::read_to_string(map).map_err(|e| format!("{}: {e}", map.display()))?;
            let (r, m) = parse_map(&text).map_err(|e| format!("{}: {e}", map.display()))?;
            (r, m, map.parent().map(Path::to_path_buf))
        }
        None => (None, Vec::new(), None),
    };
    if let Some(dir) = input_dir() {
        mounts.push(Mount {
            guest: "/dev/input".into(),
            host: dir.to_owned(),
            area: Area::Input,
            source: None,
            fstype: None,
            own: false,
            seq: 0,
        });
        mounts.sort_by(|a, b| b.guest.len().cmp(&a.guest.len()));
    }
    let root = map_root.as_deref().unwrap_or(root);
    let root = root
        .canonicalize()
        .map_err(|e| format!("--root {}: {e}", root.display()))?;
    let read_only_dev = read_only_dev(&root);
    let _ = VFS.set(Vfs {
        root,
        mapped: !mounts.is_empty(),
        read_only_dev,
        mounts: RwLock::new(mounts),
        runtime,
        cwd: Mutex::new("/".into()),
    });
    Ok(())
}

/// The device of the filesystem holding `root`, if it is mounted read-only.
fn read_only_dev(root: &Path) -> Option<i32> {
    let c = CString::new(root.as_os_str().as_bytes()).ok()?;
    let mut fs: libc::statfs = unsafe { std::mem::zeroed() };
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: NUL-terminated path, local buffers.
    let ok =
        unsafe { libc::statfs(c.as_ptr(), &mut fs) == 0 && libc::stat(c.as_ptr(), &mut st) == 0 };
    (ok && fs.f_flags & libc::MNT_RDONLY as u32 != 0).then_some(st.st_dev)
}

/// Whether host device `dev` is the read-only volume of the root: nothing
/// on it can have been changed by a guest.
pub fn on_read_only_root(dev: i32) -> bool {
    VFS.get().and_then(|v| v.read_only_dev) == Some(dev)
}

fn vfs() -> &'static Vfs {
    VFS.get().expect("vfs not initialized")
}

pub fn root() -> &'static Path {
    &vfs().root
}

/// The guest-init runtime directory (`<runtime>/fs-attrs`, `sockets`,
/// `identity`), when running under a path map.
pub fn runtime_dir() -> Option<&'static Path> {
    VFS.get()?.runtime.as_deref()
}

/// A mount as `/proc/mounts` lists it.
pub struct MountPoint {
    pub guest: String,
    pub area: Area,
    /// None for a plain directory of the path map.
    pub source: Option<String>,
    pub fstype: Option<String>,
}

/// The path map's directory entries (its "mounts"), shortest guest path
/// first (single-file entries are left out), then this process's own
/// mounts in mount order.
pub fn mount_points() -> Vec<MountPoint> {
    let mounts = vfs().mounts.read().unwrap();
    let (mut map, mut own): (Vec<&Mount>, Vec<&Mount>) = mounts
        .iter()
        .filter(|m| m.own || m.host.is_dir())
        .partition(|m| !m.own);
    map.reverse();
    own.reverse();
    map.into_iter()
        .chain(own)
        .map(|m| MountPoint {
            guest: m.guest.clone(),
            area: m.area,
            source: m.source.clone(),
            fstype: m.fstype.clone(),
        })
        .collect()
}

/// The host paths the guest writes to: the path map's writable entries,
/// or the root without a path map. None before [`init`].
pub fn writable_hosts() -> Option<Vec<PathBuf>> {
    let v = VFS.get()?;
    if !v.mapped {
        return Some(vec![v.root.clone()]);
    }
    let mounts = v.mounts.read().unwrap();
    Some(
        mounts
            .iter()
            .filter(|m| !m.own && m.area == Area::Writable)
            .map(|m| m.host.clone())
            .collect(),
    )
}

pub fn cwd() -> String {
    vfs().cwd.lock().unwrap().clone()
}

pub fn set_cwd(guest: String) {
    *vfs().cwd.lock().unwrap() = guest;
}

/// The rest of `path` below the mount point `at`, if it is there.
fn below<'a>(at: &str, path: &'a str) -> Option<&'a str> {
    if path == at {
        Some("")
    } else {
        path.strip_prefix(at).and_then(|r| r.strip_prefix('/'))
    }
}

/// Whether a later mount at or above the mount point of `m` hides it.
fn hidden(mounts: &[Mount], m: &Mount) -> bool {
    mounts
        .iter()
        .any(|n| n.seq > m.seq && below(&n.guest, &m.guest).is_some())
}

/// The mount that holds the normalized guest path, and the path below it:
/// the deepest one not hidden. `mounts` is ordered longest mount point
/// first, the latest first among equal ones.
fn holder<'a, 'p>(mounts: &'a [Mount], guest: &'p str) -> Option<(&'a Mount, &'p str)> {
    mounts
        .iter()
        .find_map(|m| Some((m, below(&m.guest, guest)?)).filter(|(m, _)| !hidden(mounts, m)))
}

/// Host path and area of a normalized absolute guest path, without
/// following symlinks.
pub fn lookup(guest: &str) -> (PathBuf, Area) {
    if HOST_DEVICES.contains(&guest) {
        return (PathBuf::from(guest), Area::HostDevice);
    }
    if let Some(host) = crate::sys::tty::pts_host(guest) {
        return (PathBuf::from(host), Area::HostDevice);
    }
    let v = vfs();
    if let Some((m, rest)) = holder(&v.mounts.read().unwrap(), guest) {
        let host = if rest.is_empty() {
            m.host.clone()
        } else {
            m.host.join(rest)
        };
        return (host, m.area);
    }
    (v.root.join(guest.trim_start_matches('/')), Area::Image)
}

/// The path fs-attrs knows a normalized guest path by, so that a file has
/// one owner and mode through whichever mount it is reached (a bind shows
/// its source's): its path through the path map entry with the shortest
/// host prefix (the area itself rather than a bind into it), or through
/// the image root; in a tmpfs of this process, the host path (private to
/// the process and its children).
pub fn attr_key(guest: &str) -> String {
    if !guest.starts_with('/') {
        return guest.to_string();
    }
    let (host, _) = lookup(guest);
    let v = vfs();
    let mounts = v.mounts.read().unwrap();
    let area = mounts
        .iter()
        .filter(|m| !m.own)
        .filter_map(|m| Some((m, host.strip_prefix(&m.host).ok()?)))
        .min_by_key(|(m, _)| (m.host.as_os_str().len(), m.guest.len()));
    if let Some((m, rest)) = area {
        return if rest.as_os_str().is_empty() {
            m.guest.clone()
        } else {
            format!("{}/{}", m.guest, rest.display())
        };
    }
    match host.strip_prefix(&v.root) {
        Ok(rel) => format!("/{}", rel.display()),
        Err(_) => host.display().to_string(),
    }
}

/// Mount `host` (with `area`) at the normalized guest path `guest`, over
/// whatever is there. `source` and `fstype` are what `/proc/mounts` shows.
pub fn add_mount(guest: &str, host: PathBuf, area: Area, source: &str, fstype: &str) {
    let guest = if guest == "/" {
        guest
    } else {
        guest.trim_end_matches('/')
    };
    let mut mounts = vfs().mounts.write().unwrap();
    let seq = mounts.iter().map(|m| m.seq).max().unwrap_or(0) + 1;
    let at = mounts
        .iter()
        .position(|m| m.guest.len() <= guest.len())
        .unwrap_or(mounts.len());
    mounts.insert(
        at,
        Mount {
            guest: guest.to_string(),
            host,
            area,
            source: Some(source.to_string()),
            fstype: Some(fstype.to_string()),
            own: true,
            seq,
        },
    );
}

/// The filesystem type of the mount that holds the normalized guest path.
pub fn fstype(guest: &str) -> Option<String> {
    let mounts = vfs().mounts.read().unwrap();
    holder(&mounts, guest).and_then(|(m, _)| m.fstype.clone())
}

/// The kernel filesystem `guest` is on, and the path below its root, for
/// the mounts that name one (bpffs, cgroup2).
pub fn fs_path(guest: &str) -> Option<(String, String)> {
    let mounts = vfs().mounts.read().unwrap();
    mounts
        .iter()
        .filter(|m| m.fstype.is_some())
        .filter_map(|m| {
            let rest = guest.strip_prefix(m.guest.as_str())?;
            (rest.is_empty() || rest.starts_with('/')).then_some((m, rest))
        })
        .max_by_key(|(m, _)| m.guest.len())
        .map(|(m, rest)| {
            let rest = if rest.is_empty() { "/" } else { rest };
            (m.fstype.clone().unwrap(), rest.to_string())
        })
}

/// Remove the latest mount this process made at `guest`.
pub fn remove_mount(guest: &str) -> bool {
    let mut mounts = vfs().mounts.write().unwrap();
    match mounts.iter().position(|m| m.guest == guest && m.own) {
        Some(i) => {
            mounts.remove(i);
            true
        }
        None => false,
    }
}

/// Move the latest mount at `from`, and this process's mounts below it, to
/// `to`.
pub fn move_mount(from: &str, to: &str) -> bool {
    let mut mounts = vfs().mounts.write().unwrap();
    let Some(i) = mounts.iter().position(|m| m.guest == from && m.own) else {
        return false;
    };
    let mut moved = vec![mounts.remove(i)];
    let below = format!("{from}/");
    while let Some(j) = mounts
        .iter()
        .position(|m| m.own && m.guest.starts_with(&below))
    {
        moved.push(mounts.remove(j));
    }
    drop(mounts);
    for m in moved {
        let guest = format!("{to}{}", &m.guest[from.len()..]);
        let (source, fstype) = (m.source.unwrap_or_default(), m.fstype.unwrap_or_default());
        add_mount(&guest, m.host, m.area, &source, &fstype);
    }
    true
}

/// This process's own mounts, oldest first, as `--mounts` text for the
/// program it execs: `ro|rw<TAB>guest<TAB>host<TAB>source<TAB>fstype`
/// lines.
pub fn own_mounts_text() -> String {
    let mounts = vfs().mounts.read().unwrap();
    let mut out = String::new();
    let mut own: Vec<&Mount> = mounts.iter().filter(|m| m.own).collect();
    own.reverse();
    for m in own {
        let area = if m.area == Area::Image { "ro" } else { "rw" };
        out.push_str(&format!(
            "{area}\t{}\t{}\t{}\t{}\n",
            m.guest,
            m.host.display(),
            m.source.as_deref().unwrap_or_default(),
            m.fstype.as_deref().unwrap_or_default()
        ));
    }
    out
}

/// Restore the mounts [`own_mounts_text`] described.
pub fn load_own_mounts(text: &str) {
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if let [area, guest, host, source, fstype] = f[..] {
            let area = if area == "ro" {
                Area::Image
            } else {
                Area::Writable
            };
            add_mount(guest, PathBuf::from(host), area, source, fstype);
        }
    }
}

/// Guest path of a host path, if it lies inside the root or a mapped area.
pub fn guest_path_of_host(host: &Path) -> Option<String> {
    let v = VFS.get()?;
    let s = host.to_str()?;
    if HOST_DEVICES.contains(&s) {
        return Some(s.to_string());
    }
    if let Some(guest) = crate::sys::tty::pts_guest(s) {
        return Some(guest);
    }
    // The mount deepest into the host tree (a bind over its source's area);
    // between mounts of the same host directory, the shorter mount point
    // (`/data/user/0`, not its `/data_mirror` copy). Hidden mounts are not
    // reachable.
    let mounts = v.mounts.read().unwrap();
    let mut best: Option<(usize, usize, String)> = None;
    for m in mounts.iter() {
        let Ok(rest) = host.strip_prefix(&m.host) else {
            continue;
        };
        if hidden(&mounts, m) {
            continue;
        }
        let len = m.host.as_os_str().len();
        if best
            .as_ref()
            .is_none_or(|b| len > b.0 || (len == b.0 && m.guest.len() < b.1))
        {
            let g = if rest.as_os_str().is_empty() {
                m.guest.clone()
            } else {
                format!("{}/{}", m.guest, rest.display())
            };
            best = Some((len, m.guest.len(), g));
        }
    }
    if let Some((_, _, g)) = best {
        return Some(g);
    }
    let rel = host.strip_prefix(&v.root).ok()?;
    Some(format!("/{}", rel.display()))
}

/// Guest path of an open directory fd.
fn guest_path_of_fd(fd: i32) -> Result<String, Errno> {
    if let Some(p) = crate::sys::synthesized_dir_path(fd) {
        return Ok(p);
    }
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } < 0 {
        return Err(errno::last());
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let host = Path::new(OsStr::from_bytes(&buf[..len]));
    Ok(guest_path_of_host(host).unwrap_or_else(|| host.display().to_string()))
}

fn join_guest(components: &[Vec<u8>]) -> String {
    let mut guest = String::new();
    for c in components {
        guest.push('/');
        guest.push_str(&String::from_utf8_lossy(c));
    }
    if guest.is_empty() {
        guest.push('/');
    }
    guest
}

pub struct Resolved {
    pub host: CString,
    pub guest: String,
    pub area: Area,
}

impl Resolved {
    /// Whether the guest may not modify this path (the image under a path
    /// map): writes fail with EROFS.
    pub fn read_only(&self) -> bool {
        self.area == Area::Image && vfs().mapped
    }
}

/// Symlink answers for the read-only image (target, or None for anything
/// else), which never change: resolving walks every component, and the
/// image's paths are the most walked.
static IMAGE_LINKS: Mutex<Option<HashMap<PathBuf, Option<PathBuf>>>> = Mutex::new(None);
const IMAGE_LINKS_MAX: usize = 1 << 14;

/// The target of the symlink at `host`, if it is one.
fn link_target(host: PathBuf, area: Area) -> Option<PathBuf> {
    if area != Area::Image || !vfs().mapped {
        return std::fs::read_link(&host).ok();
    }
    if let Some(known) = IMAGE_LINKS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&host))
    {
        return known.clone();
    }
    let target = std::fs::read_link(&host).ok();
    let mut links = IMAGE_LINKS.lock().unwrap();
    let map = links.get_or_insert_with(HashMap::new);
    if map.len() >= IMAGE_LINKS_MAX {
        map.clear();
    }
    map.insert(host, target.clone());
    target
}

/// Resolve a guest path relative to a Linux dirfd into a host path.
pub fn resolve(dirfd: i32, path: &[u8], follow_last: bool) -> Result<Resolved, Errno> {
    if path.is_empty() {
        return Err(errno::ENOENT);
    }
    let base = if path[0] == b'/' {
        String::from("/")
    } else if dirfd == LINUX_AT_FDCWD {
        cwd()
    } else {
        guest_path_of_fd(dirfd)?
    };
    let mut pending: Vec<Vec<u8>> = Vec::new();
    let mut joined = base.into_bytes();
    joined.push(b'/');
    joined.extend_from_slice(path);
    for c in joined.split(|&b| b == b'/').rev() {
        if !c.is_empty() {
            pending.push(c.to_vec());
        }
    }
    let mut done: Vec<Vec<u8>> = Vec::new();
    let mut links = 0;
    while let Some(c) = pending.pop() {
        match c.as_slice() {
            b"." => continue,
            b".." => {
                done.pop();
                continue;
            }
            _ => {}
        }
        done.push(c);
        let is_last = pending.is_empty();
        if is_last && !follow_last {
            break;
        }
        if done[0] == b"proc" {
            continue;
        }
        let (host, area) = lookup(&join_guest(&done));
        let Some(target) = link_target(host, area) else {
            continue;
        };
        links += 1;
        if links > MAX_SYMLINKS {
            return Err(errno::ELOOP);
        }
        done.pop();
        let t = target.as_os_str().as_bytes();
        if t.first() == Some(&b'/') {
            done.clear();
        }
        for part in t.split(|&b| b == b'/').rev() {
            if !part.is_empty() {
                pending.push(part.to_vec());
            }
        }
    }
    let guest = join_guest(&done);
    let (host, area) = lookup(&guest);
    Ok(Resolved {
        host: CString::new(host.as_os_str().as_bytes()).map_err(|_| errno::EINVAL)?,
        guest,
        area,
    })
}

/// The process-wide view the unit tests share (it is initialized once):
/// `/data` and the data mirrors as guest-init maps them. The guard keeps
/// tests that mount from running at the same time.
#[cfg(test)]
pub(crate) fn test_view() -> (std::sync::MutexGuard<'static, ()>, &'static Path) {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    static LOCK: Mutex<()> = Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("vfs-view-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (root, data) = (dir.join("root"), dir.join("data"));
        for d in [&root, &data, &dir.join("run/data_mirror")] {
            std::fs::create_dir_all(d).unwrap();
        }
        let map = dir.join("run/path-map");
        let d = data.display();
        std::fs::write(
            &map,
            format!(
                "rw\t/data\t{d}\n\
                 rw\t/data/user/0\t{d}/data\n\
                 rw\t/data_mirror\t{}\n\
                 rw\t/data_mirror/data_ce/null\t{d}/user\n\
                 rw\t/data_mirror/data_ce/null/0\t{d}/data\n\
                 rw\t/data_mirror/data_de/null\t{d}/user_de\n",
                dir.join("run/data_mirror").display()
            ),
        )
        .unwrap();
        init(&root, Some(&map)).unwrap();
        dir
    });
    (guard, dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_parses_and_orders_longest_first() {
        let (root, m) = parse_map(
            "# aim-guest-init path map v1\nroot\t/\t/img\nrw\t/dev\t/r/dev\nrw\t/apex/apex-info-list.xml\t/r/a.xml\nkernfs\t/proc\t/r/kernfs/proc\n",
        )
        .unwrap();
        assert_eq!(root, Some(PathBuf::from("/img")));
        assert_eq!(m[0].guest, "/apex/apex-info-list.xml");
        assert_eq!(m.last().unwrap().guest, "/dev");
        assert!(parse_map("bogus\t/\t/x\n").is_err());
        assert!(parse_map("rw\t/x\n").is_err());
        let (_, m) = parse_map("cgroup2\t/sys/fs/cgroup\t/r/cgroup\n").unwrap();
        assert_eq!(
            (m[0].area, m[0].fstype.as_deref()),
            (Area::Writable, Some("cgroup2"))
        );
    }

    #[test]
    fn own_mounts_shadow_the_map_until_unmounted() {
        let (_view, dir) = test_view();
        let (data, tmp) = (dir.join("data"), dir.join("tmp"));
        std::fs::create_dir_all(&tmp).unwrap();

        add_mount("/data/app", tmp.clone(), Area::Writable, "tmpfs", "tmpfs");
        assert_eq!(lookup("/data/app/x").0, tmp.join("x"));
        assert_eq!(lookup("/data/other").0, data.join("other"));
        assert!(
            mount_points()
                .iter()
                .any(|m| m.guest == "/data/app" && m.fstype.as_deref() == Some("tmpfs"))
        );
        let text = own_mounts_text();
        assert!(move_mount("/data/app", "/data/moved"));
        assert_eq!(lookup("/data/moved").0, tmp);
        assert!(remove_mount("/data/moved"));
        assert!(!remove_mount("/data/moved"));
        assert!(!remove_mount("/data"), "path map entries are not unmounted");

        load_own_mounts(&text);
        assert_eq!(lookup("/data/app").0, tmp);
        assert!(remove_mount("/data/app"));
    }

    #[test]
    fn a_mount_hides_the_older_ones_below_it() {
        let (_view, dir) = test_view();
        let (data, tmp) = (dir.join("data"), dir.join("hide"));
        std::fs::create_dir_all(&tmp).unwrap();
        // The path map's /data/user/0 is /data/data, and so is its mirror.
        assert_eq!(lookup("/data/user/0/p").0, data.join("data/p"));
        assert_eq!(attr_key("/data/user/0/p"), "/data/data/p");
        assert_eq!(attr_key("/data_mirror/data_ce/null/0/p"), "/data/data/p");
        assert_eq!(attr_key("/data_mirror/data_ce/null"), "/data/user");
        assert_eq!(
            guest_path_of_host(&data.join("data/p")).as_deref(),
            Some("/data/user/0/p")
        );

        add_mount("/data/user", tmp.clone(), Area::Writable, "tmpfs", "tmpfs");
        assert_eq!(lookup("/data/user/0").0, tmp.join("0"));
        assert_eq!(fstype("/data/user/0").as_deref(), Some("tmpfs"));
        assert_eq!(attr_key("/data/user/0"), format!("{}/0", tmp.display()));
        // A mount made later below the tmpfs is seen again.
        add_mount(
            "/data/user/0",
            data.join("data"),
            Area::Writable,
            "/data/data",
            "bind",
        );
        assert_eq!(lookup("/data/user/0/p").0, data.join("data/p"));
        assert!(remove_mount("/data/user/0"));
        assert!(remove_mount("/data/user"));
        assert_eq!(lookup("/data/user/0/p").0, data.join("data/p"));
    }
}
