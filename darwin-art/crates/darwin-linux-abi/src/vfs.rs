//! Guest path view: the guest's `/` is a host directory (`--root`), with
//! writable areas mapped over it by a path map (`--path-map`,
//! `docs/guest-init-contract.md` section 2).
//!
//! - The longest matching guest prefix wins, on whole components; the host
//!   device nodes in [`HOST_DEVICES`] take precedence over any `/dev` entry.
//! - Paths are resolved component by component with symlinks interpreted
//!   relative to the guest root, the way a chroot would, so absolute links
//!   inside the image (for example `/bin -> /system/bin`) stay inside it.
//!   Each intermediate result goes through the map again. Links under
//!   `/proc` are synthesized and not followed here.
//! - The inverse map (host path -> guest path) uses the same table.
//! - With a path map, the image root is read-only to the guest.

use std::ffi::{CString, OsStr};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

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
}

struct Mount {
    /// Normalized absolute guest path, no trailing `/`.
    guest: String,
    host: PathBuf,
    area: Area,
}

struct Vfs {
    root: PathBuf,
    /// Longest guest prefix first.
    mounts: Vec<Mount>,
    /// Directory holding the path map (the guest-init runtime directory).
    runtime: Option<PathBuf>,
    cwd: Mutex<String>,
}

static VFS: OnceLock<Vfs> = OnceLock::new();

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
            "rw" => Area::Writable,
            "kernfs" => Area::Kernfs,
            other => return Err(format!("line {}: unknown kind '{other}'", n + 1)),
        };
        let guest = guest.trim_end_matches('/');
        if !guest.starts_with('/') {
            return Err(format!("line {}: guest path must be absolute", n + 1));
        }
        mounts.push(Mount {
            guest: guest.to_string(),
            host: PathBuf::from(host),
            area,
        });
    }
    mounts.sort_by(|a, b| b.guest.len().cmp(&a.guest.len()));
    Ok((root, mounts))
}

/// Initialize from `--root` and, when given, a `--path-map` file (whose
/// `root` line overrides `root`).
pub fn init(root: &Path, map: Option<&Path>) -> Result<(), String> {
    let (map_root, mounts, runtime) = match map {
        Some(map) => {
            let text =
                std::fs::read_to_string(map).map_err(|e| format!("{}: {e}", map.display()))?;
            let (r, m) = parse_map(&text).map_err(|e| format!("{}: {e}", map.display()))?;
            (r, m, map.parent().map(Path::to_path_buf))
        }
        None => (None, Vec::new(), None),
    };
    let root = map_root.as_deref().unwrap_or(root);
    let root = root
        .canonicalize()
        .map_err(|e| format!("--root {}: {e}", root.display()))?;
    let _ = VFS.set(Vfs {
        root,
        mounts,
        runtime,
        cwd: Mutex::new("/".into()),
    });
    Ok(())
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

pub fn cwd() -> String {
    vfs().cwd.lock().unwrap().clone()
}

pub fn set_cwd(guest: String) {
    *vfs().cwd.lock().unwrap() = guest;
}

/// Host path and area of a normalized absolute guest path, without
/// following symlinks.
pub fn lookup(guest: &str) -> (PathBuf, Area) {
    if HOST_DEVICES.contains(&guest) {
        return (PathBuf::from(guest), Area::HostDevice);
    }
    let v = vfs();
    for m in &v.mounts {
        let rest = if guest == m.guest {
            Some("")
        } else {
            guest
                .strip_prefix(m.guest.as_str())
                .and_then(|r| r.strip_prefix('/'))
        };
        if let Some(rest) = rest {
            let host = if rest.is_empty() {
                m.host.clone()
            } else {
                m.host.join(rest)
            };
            return (host, m.area);
        }
    }
    (v.root.join(guest.trim_start_matches('/')), Area::Image)
}

/// Guest path of a host path, if it lies inside the root or a mapped area.
pub fn guest_path_of_host(host: &Path) -> Option<String> {
    let v = VFS.get()?;
    let s = host.to_str()?;
    if HOST_DEVICES.contains(&s) {
        return Some(s.to_string());
    }
    let mut best: Option<(usize, String)> = None;
    for m in &v.mounts {
        if let Ok(rest) = host.strip_prefix(&m.host) {
            let len = m.host.as_os_str().len();
            if best.as_ref().is_none_or(|b| len > b.0) {
                let g = if rest.as_os_str().is_empty() {
                    m.guest.clone()
                } else {
                    format!("{}/{}", m.guest, rest.display())
                };
                best = Some((len, g));
            }
        }
    }
    if let Some((_, g)) = best {
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
        self.area == Area::Image && !vfs().mounts.is_empty()
    }
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
        let (host, _) = lookup(&join_guest(&done));
        let Ok(target) = std::fs::read_link(&host) else {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_parses_and_orders_longest_first() {
        let (root, m) = parse_map(
            "# darwin-guest-init path map v1\nroot\t/\t/img\nrw\t/dev\t/r/dev\nrw\t/apex/apex-info-list.xml\t/r/a.xml\nkernfs\t/proc\t/r/kernfs/proc\n",
        )
        .unwrap();
        assert_eq!(root, Some(PathBuf::from("/img")));
        assert_eq!(m[0].guest, "/apex/apex-info-list.xml");
        assert_eq!(m.last().unwrap().guest, "/dev");
        assert!(parse_map("bogus\t/\t/x\n").is_err());
        assert!(parse_map("rw\t/x\n").is_err());
    }
}
