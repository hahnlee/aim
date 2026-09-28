//! The guest filesystem view: the read-only derived image plus the
//! writable areas init and the services write to.
//!
//! The mapping table is the contract with the syscall layer
//! (`docs/guest-init-contract.md`, "Path map"): `linux-run --path-map FILE`
//! must resolve guest paths through the same table, so what init creates at
//! `/dev/socket/logd` is what `logd` finds there.
//!
//! | Guest | Host | Kind | Lifetime |
//! | --- | --- | --- | --- |
//! | `/dev` | `<runtime>/dev` | writable | per boot (tmpfs on a device) |
//! | `/mnt`, `/tmp`, `/storage`, `/config`, `/data_mirror`, `/linkerconfig` | `<runtime>/<name>` | writable | per boot |
//! | `/apex/apex-info-list.xml` | `<runtime>/apex/apex-info-list.xml` | writable | per boot |
//! | `/data`, `/metadata`, `/cache` | `<data>/<name>` | writable | persistent |
//! | `/data/user/0` | `<data>/data/data` | writable | persistent (vold's bind of `/data/data`) |
//! | `/proc`, `/sys` | `<runtime>/kernfs/{proc,sys}` | kernfs | per boot: values init wrote |
//! | `/sys/fs/cgroup` | `<runtime>/cgroup` | cgroup2 | per boot: a plain directory tree |
//! | `/sys/fs/bpf` | `<runtime>/bpf` | bpf | per boot: pinned eBPF objects |
//! | everything else | `<image>/...` | read-only image | |
//!
//! Host device nodes (`/dev/null`, `/dev/zero`, `/dev/random`,
//! `/dev/urandom`, `/dev/tty`) keep the syscall layer's passthrough and take
//! precedence over the `/dev` entry.

use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// Guest paths the syscall layer passes through to the host device node.
pub const HOST_DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/tty",
];

/// Marks a runtime directory this crate created, so a new boot may wipe it.
const RUNTIME_MARKER: &str = ".aim-guest-init-runtime";

const MAX_SYMLINKS: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapKind {
    /// Reads and writes go to the host directory or file.
    Writable,
    /// `/proc` and `/sys`: the syscall layer synthesizes them; this tree
    /// only holds the values init wrote, for the layer to report.
    Kernfs,
    /// The cgroup v2 hierarchy: a writable directory whose groups no
    /// controller acts on. zygote cannot start a process without making
    /// its group (`createProcessGroup`).
    Cgroup2,
    /// bpffs: where the syscall layer's eBPF maps and programs are pinned
    /// (hard links to their object files).
    Bpf,
}

impl MapKind {
    pub fn keyword(self) -> &'static str {
        match self {
            MapKind::Writable => "rw",
            MapKind::Kernfs => "kernfs",
            MapKind::Cgroup2 => "cgroup2",
            MapKind::Bpf => "bpf",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapEntry {
    /// Normalized absolute guest path (no trailing `/`).
    pub guest: String,
    pub host: PathBuf,
    pub kind: MapKind,
}

/// Where a resolved guest path lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Area {
    Writable {
        prefix: String,
    },
    Kernfs {
        prefix: String,
    },
    /// A host device node passed through by the syscall layer.
    HostDevice,
    ReadOnlyImage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// The guest path after symlink resolution.
    pub guest: String,
    pub host: PathBuf,
    pub area: Area,
}

/// Guest path → host path table (longest prefix wins) over the image root.
#[derive(Clone, Debug)]
pub struct PathMap {
    image: PathBuf,
    entries: Vec<MapEntry>,
}

impl PathMap {
    pub fn new(image: impl Into<PathBuf>, mut entries: Vec<MapEntry>) -> Self {
        entries.sort_by(|a, b| {
            b.guest
                .len()
                .cmp(&a.guest.len())
                .then(a.guest.cmp(&b.guest))
        });
        Self {
            image: image.into(),
            entries,
        }
    }

    pub fn image(&self) -> &Path {
        &self.image
    }

    /// Add `entry`, replacing one at the same guest path.
    pub fn add(&mut self, entry: MapEntry) {
        self.entries.retain(|e| e.guest != entry.guest);
        let mut entries = std::mem::take(&mut self.entries);
        entries.push(entry);
        *self = Self::new(std::mem::take(&mut self.image), entries);
    }

    pub fn entries(&self) -> &[MapEntry] {
        &self.entries
    }

    /// Host path and area of a normalized guest path, without following
    /// symlinks.
    pub fn lookup(&self, guest: &str) -> (PathBuf, Area) {
        if HOST_DEVICES.contains(&guest) {
            return (PathBuf::from(guest), Area::HostDevice);
        }
        for entry in &self.entries {
            let rest = if guest == entry.guest {
                Some("")
            } else {
                guest
                    .strip_prefix(entry.guest.as_str())
                    .and_then(|r| r.strip_prefix('/'))
            };
            if let Some(rest) = rest {
                let host = if rest.is_empty() {
                    entry.host.clone()
                } else {
                    entry.host.join(rest)
                };
                let area = match entry.kind {
                    MapKind::Writable | MapKind::Cgroup2 | MapKind::Bpf => Area::Writable {
                        prefix: entry.guest.clone(),
                    },
                    MapKind::Kernfs => Area::Kernfs {
                        prefix: entry.guest.clone(),
                    },
                };
                return (host, area);
            }
        }
        (
            self.image.join(guest.trim_start_matches('/')),
            Area::ReadOnlyImage,
        )
    }

    /// Resolves a guest path component by component, interpreting symlinks
    /// relative to the guest root (as `linux-run`'s vfs does), through the
    /// map. `/proc/self/...` links are not followed.
    pub fn resolve(&self, guest: &str, follow_last: bool) -> Result<Resolved, String> {
        if !guest.starts_with('/') {
            return Err(format!("{guest}: not an absolute guest path"));
        }
        let mut pending: Vec<String> = guest
            .split('/')
            .filter(|c| !c.is_empty())
            .rev()
            .map(str::to_string)
            .collect();
        let mut done: Vec<String> = Vec::new();
        let mut links = 0;
        while let Some(component) = pending.pop() {
            match component.as_str() {
                "." => continue,
                ".." => {
                    done.pop();
                    continue;
                }
                _ => {}
            }
            done.push(component);
            if pending.is_empty() && !follow_last {
                break;
            }
            let current = join_guest(&done);
            if current.starts_with("/proc/") || current == "/proc" {
                continue;
            }
            let (host, _) = self.lookup(&current);
            let Ok(target) = fs::read_link(&host) else {
                continue;
            };
            links += 1;
            if links > MAX_SYMLINKS {
                return Err(format!("{guest}: too many levels of symbolic links"));
            }
            done.pop();
            let target = String::from_utf8_lossy(target.as_os_str().as_bytes()).into_owned();
            if target.starts_with('/') {
                done.clear();
            }
            for part in target.split('/').rev() {
                if !part.is_empty() {
                    pending.push(part.to_string());
                }
            }
        }
        let guest = join_guest(&done);
        let (host, area) = self.lookup(&guest);
        Ok(Resolved { guest, host, area })
    }

    /// The table in the `--path-map` file format (see the contract).
    pub fn to_file_text(&self) -> String {
        let mut out = String::from("# aim-guest-init path map v1\n");
        out.push_str(&format!("root\t/\t{}\n", self.image.display()));
        for entry in &self.entries {
            out.push_str(&format!(
                "{}\t{}\t{}\n",
                entry.kind.keyword(),
                entry.guest,
                entry.host.display()
            ));
        }
        out
    }

    /// Parses the `--path-map` format back (for the contract's round trip).
    pub fn parse_file_text(text: &str) -> Result<Self, String> {
        let mut image = None;
        let mut entries = Vec::new();
        for (number, line) in text.lines().enumerate() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split('\t').collect();
            let [kind, guest, host] = fields[..] else {
                return Err(format!(
                    "line {}: expected 3 tab-separated fields",
                    number + 1
                ));
            };
            match kind {
                "root" => image = Some(PathBuf::from(host)),
                "rw" | "kernfs" | "cgroup2" | "bpf" => entries.push(MapEntry {
                    guest: guest.to_string(),
                    host: PathBuf::from(host),
                    kind: match kind {
                        "rw" => MapKind::Writable,
                        "kernfs" => MapKind::Kernfs,
                        "cgroup2" => MapKind::Cgroup2,
                        _ => MapKind::Bpf,
                    },
                }),
                other => return Err(format!("line {}: unknown kind '{other}'", number + 1)),
            }
        }
        let image = image.ok_or("missing root line")?;
        Ok(Self::new(image, entries))
    }
}

fn join_guest(components: &[String]) -> String {
    if components.is_empty() {
        return "/".to_string();
    }
    let mut out = String::new();
    for component in components {
        out.push('/');
        out.push_str(component);
    }
    out
}

/// Host directories of one boot.
#[derive(Clone, Debug)]
pub struct Layout {
    /// The derived image root (read-only).
    pub image: PathBuf,
    /// Persistent writable root (`/data`, `/metadata`, `/cache`).
    pub data: PathBuf,
    /// Per-boot runtime directory (`/dev`, sockets, identities, logs).
    pub runtime: PathBuf,
}

/// Per-boot guest directories backed by `<runtime>/<name>`.
const RUNTIME_DIRS: &[&str] = &[
    "dev",
    "mnt",
    "tmp",
    "storage",
    "config",
    "data_mirror",
    "linkerconfig",
];
/// Persistent guest directories backed by `<data>/<name>`.
const DATA_DIRS: &[&str] = &["data", "metadata", "cache"];

impl Layout {
    /// `runtime` defaults to `<data>/run`.
    pub fn new(image: PathBuf, data: PathBuf, runtime: Option<PathBuf>) -> Self {
        let runtime = runtime.unwrap_or_else(|| data.join("run"));
        Self {
            image,
            data,
            runtime,
        }
    }

    pub fn dev_dir(&self) -> PathBuf {
        self.runtime.join("dev")
    }
    /// Host directory the guest sees as `/dev/__properties__`.
    pub fn properties_dir(&self) -> PathBuf {
        self.dev_dir().join("__properties__")
    }
    /// Host directory the guest sees as `/dev/socket`.
    pub fn socket_dir(&self) -> PathBuf {
        self.dev_dir().join("socket")
    }
    pub fn kernfs_dir(&self) -> PathBuf {
        self.runtime.join("kernfs")
    }
    /// Host directory the guest sees as the cgroup v2 hierarchy.
    pub fn cgroup_dir(&self) -> PathBuf {
        self.runtime.join("cgroup")
    }
    /// Host directory the guest sees as bpffs.
    pub fn bpf_dir(&self) -> PathBuf {
        self.runtime.join("bpf")
    }
    pub fn identity_dir(&self) -> PathBuf {
        self.runtime.join("identity")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.runtime.join("logs")
    }
    pub fn path_map_file(&self) -> PathBuf {
        self.runtime.join("path-map")
    }
    pub fn fs_attrs_file(&self) -> PathBuf {
        self.runtime.join("fs-attrs")
    }
    pub fn sockets_file(&self) -> PathBuf {
        self.runtime.join("sockets")
    }
    pub fn apex_info_list(&self) -> PathBuf {
        self.runtime.join("apex").join("apex-info-list.xml")
    }

    pub fn path_map(&self) -> PathMap {
        let mut entries = Vec::new();
        for name in RUNTIME_DIRS {
            entries.push(MapEntry {
                guest: format!("/{name}"),
                host: self.runtime.join(name),
                kind: MapKind::Writable,
            });
        }
        entries.push(MapEntry {
            guest: "/apex/apex-info-list.xml".to_string(),
            host: self.apex_info_list(),
            kind: MapKind::Writable,
        });
        for name in DATA_DIRS {
            entries.push(MapEntry {
                guest: format!("/{name}"),
                host: self.data.join(name),
                kind: MapKind::Writable,
            });
        }
        // vold binds /data/data onto /data/user/0 (prepare_special_dirs), a
        // mount /data's shared propagation shows every process, and init's
        // data mirror of /data/user; the syscall layer's mounts are per
        // process, so it is an entry here.
        entries.push(MapEntry {
            guest: "/data/user/0".to_string(),
            host: self.data.join("data/data"),
            kind: MapKind::Writable,
        });
        for name in ["proc", "sys"] {
            entries.push(MapEntry {
                guest: format!("/{name}"),
                host: self.kernfs_dir().join(name),
                kind: MapKind::Kernfs,
            });
        }
        entries.push(MapEntry {
            guest: "/sys/fs/cgroup".to_string(),
            host: self.cgroup_dir(),
            kind: MapKind::Cgroup2,
        });
        entries.push(MapEntry {
            guest: "/sys/fs/bpf".to_string(),
            host: self.bpf_dir(),
            kind: MapKind::Bpf,
        });
        PathMap::new(self.image.clone(), entries)
    }

    /// Creates the directories. The runtime directory is wiped first when a
    /// previous boot created it; a non-empty foreign directory is refused.
    pub fn prepare(&self) -> io::Result<()> {
        if self.runtime.exists() {
            let ours = self.runtime.join(RUNTIME_MARKER).exists();
            let empty = fs::read_dir(&self.runtime)?.next().is_none();
            if !ours && !empty {
                return Err(io::Error::other(format!(
                    "{}: exists and was not created by guest-init; refusing to wipe it",
                    self.runtime.display()
                )));
            }
            if ours {
                make_writable_recursive(&self.runtime);
                fs::remove_dir_all(&self.runtime)?;
            }
        }
        fs::create_dir_all(&self.runtime)?;
        fs::write(self.runtime.join(RUNTIME_MARKER), b"")?;
        for name in RUNTIME_DIRS {
            fs::create_dir_all(self.runtime.join(name))?;
        }
        for name in DATA_DIRS {
            fs::create_dir_all(self.data.join(name))?;
        }
        // The mount point of /data/user/0, a symlink to /data/data in
        // earlier layouts (#221).
        let user0 = self.data.join("data/user/0");
        if fs::symlink_metadata(&user0).is_ok_and(|m| m.file_type().is_symlink()) {
            fs::remove_file(&user0)?;
        }
        fs::create_dir_all(&user0)?;
        for dir in [
            self.properties_dir(),
            self.socket_dir(),
            self.kernfs_dir().join("proc"),
            self.kernfs_dir().join("sys"),
            self.cgroup_dir(),
            self.bpf_dir(),
            self.identity_dir().join("by-pid"),
            self.logs_dir(),
            self.runtime.join("apex"),
        ] {
            fs::create_dir_all(dir)?;
        }
        // /dev/kmsg: a regular file stands in for the kernel log device
        // until the syscall layer emulates it; writes append there.
        fs::write(self.dev_dir().join("kmsg"), b"")?;
        Ok(())
    }
}

fn make_writable_recursive(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    let mode = metadata.permissions().mode();
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o700));
    if metadata.is_dir()
        && let Ok(read) = fs::read_dir(path)
    {
        for entry in read.flatten() {
            make_writable_recursive(&entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> PathMap {
        Layout::new("/img".into(), "/w".into(), Some("/r".into())).path_map()
    }

    #[test]
    fn longest_prefix_wins() {
        let map = map();
        assert_eq!(
            map.lookup("/apex/apex-info-list.xml").0,
            PathBuf::from("/r/apex/apex-info-list.xml")
        );
        assert_eq!(
            map.lookup("/apex/com.android.art/lib64").0,
            PathBuf::from("/img/apex/com.android.art/lib64")
        );
        assert_eq!(
            map.lookup("/dev/socket/logd"),
            (
                PathBuf::from("/r/dev/socket/logd"),
                Area::Writable {
                    prefix: "/dev".into()
                }
            )
        );
        assert_eq!(map.lookup("/dev/null").1, Area::HostDevice);
        assert_eq!(map.lookup("/device").1, Area::ReadOnlyImage);
        assert_eq!(map.lookup("/data").0, PathBuf::from("/w/data"));
        assert!(matches!(
            map.lookup("/proc/sys/vm/x").1,
            Area::Kernfs { .. }
        ));
    }

    #[test]
    fn file_format_round_trips() {
        let map = map();
        let parsed = PathMap::parse_file_text(&map.to_file_text()).unwrap();
        assert_eq!(parsed.entries(), map.entries());
        assert_eq!(parsed.image(), map.image());
    }
}
