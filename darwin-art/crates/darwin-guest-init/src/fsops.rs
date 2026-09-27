//! Filesystem builtins (`mkdir`, `chmod`, `chown`, `write`, `copy`,
//! `symlink`, `rm`, `rmdir`, `mount`, `wait`) against the guest view of
//! [`crate::paths`].
//!
//! - Writable areas: the change is made on the host.
//! - `/proc` and `/sys`: the value is recorded in the kernfs tree for the
//!   syscall layer's emulation; nothing on the host kernel changes.
//! - The read-only image: nothing is written; init's error is returned
//!   where the path is missing (`EROFS` / `ENOENT`), a no-op where it
//!   already has the requested shape.
//! - Ownership and modes are also recorded in the fs-attrs table, since the
//!   host cannot `chown` to Android ids; the syscall layer reports them from
//!   `stat`.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::paths::{Area, PathMap, Resolved};

/// What one command did (or, in a dry run, would do).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// A process was started.
    Launch { service: String, pid: u32 },
    /// A change made in a writable area, or a state change init owns.
    Applied(String),
    /// A value recorded for the syscall layer (kernfs, fs-attrs, identity
    /// inputs, environment).
    Recorded(String),
    /// Deliberately not performed; the text says why.
    NoOp(String),
    /// Handled by the action engine itself (`trigger`, `setprop`,
    /// `wait_for_prop`, init's queued builtins).
    Engine(String),
    /// Not performed because of `--only`.
    Skipped(String),
}

impl Effect {
    pub fn kind(&self) -> &'static str {
        match self {
            Effect::Launch { .. } => "launch",
            Effect::Applied(_) => "applied",
            Effect::Recorded(_) => "recorded",
            Effect::NoOp(_) => "no-op",
            Effect::Engine(_) => "engine",
            Effect::Skipped(_) => "skipped",
        }
    }

    pub fn text(&self) -> String {
        match self {
            Effect::Launch { service, pid } => format!("started '{service}' (pid {pid})"),
            Effect::Applied(t)
            | Effect::Recorded(t)
            | Effect::NoOp(t)
            | Effect::Engine(t)
            | Effect::Skipped(t) => t.clone(),
        }
    }
}

/// One fs-attrs entry: `<guest path>\t<uid|->\t<gid|->\t<octal mode|->`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttrRecord {
    pub guest: String,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub mode: Option<u32>,
}

impl AttrRecord {
    pub fn line(&self) -> String {
        let field = |v: Option<u32>| v.map_or("-".to_string(), |v| v.to_string());
        format!(
            "{}\t{}\t{}\t{}\n",
            self.guest,
            field(self.uid),
            field(self.gid),
            self.mode.map_or("-".to_string(), |m| format!("{m:o}"))
        )
    }
}

/// `strtoul(s, 0, 8)` as the builtins parse modes.
pub fn parse_mode(text: &str) -> Result<u32, String> {
    u32::from_str_radix(text, 8).map_err(|_| format!("invalid mode '{text}'"))
}

pub struct FsOps {
    pub map: PathMap,
    /// Perform changes; a dry run only describes them.
    pub apply: bool,
    attrs_file: PathBuf,
    kernfs_values: Vec<(String, String)>,
    attrs: Vec<AttrRecord>,
    /// Mount points of kernel filesystems that are not mounted here
    /// (cgroup controllers, configfs, functionfs, ...), with the reason.
    unmounted: Vec<(String, String)>,
}

/// Legacy cgroup v1 controllers init.rc still addresses although the
/// image's `cgroups.json` no longer declares them.
pub const LEGACY_CGROUP_ROOTS: &[&str] = &["/dev/memcg", "/dev/stune"];

pub const CGROUP_REASON: &str = "cgroups are answered unsupported (ADR 0012 appendix)";

type FsResult = Result<Effect, String>;

impl FsOps {
    pub fn new(map: PathMap, attrs_file: PathBuf, apply: bool) -> Self {
        Self {
            map,
            apply,
            attrs_file,
            kernfs_values: Vec::new(),
            attrs: Vec::new(),
            unmounted: Vec::new(),
        }
    }

    /// A kernel filesystem mount point that stays unmounted (a cgroup
    /// controller from `cgroups.json`, or the target of a `mount` of a
    /// kernel filesystem): commands on its files are no-ops with `reason`
    /// rather than init's ENOENT errors on a kernel without it.
    pub fn add_unmounted_root(&mut self, guest: &str, reason: &str) {
        let guest = guest.trim_end_matches('/').to_string();
        if !guest.is_empty() && !self.unmounted.iter().any(|(root, _)| *root == guest) {
            self.unmounted.push((guest, reason.to_string()));
        }
    }

    pub fn unmounted_roots(&self) -> &[(String, String)] {
        &self.unmounted
    }

    fn cgroup_noop(&self, verb: &str, guest: &str) -> Option<Effect> {
        let normalized = guest.trim_end_matches('/');
        let (root, reason) = self.unmounted.iter().find(|(root, _)| {
            normalized == root.as_str()
                || normalized
                    .strip_prefix(root.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })?;
        Some(Effect::NoOp(format!(
            "{verb} {guest}: {root} is not mounted ({reason})"
        )))
    }

    /// Values written to `/proc` and `/sys`, in order.
    pub fn kernfs_values(&self) -> &[(String, String)] {
        &self.kernfs_values
    }

    pub fn attrs(&self) -> &[AttrRecord] {
        &self.attrs
    }

    fn resolve(&self, guest: &str, follow_last: bool) -> Result<Resolved, String> {
        self.map.resolve(guest, follow_last)
    }

    pub fn record_attrs(&mut self, record: AttrRecord) {
        if self.apply
            && let Ok(mut file) = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.attrs_file)
        {
            let _ = file.write_all(record.line().as_bytes());
        }
        self.attrs.push(record);
    }

    fn record_kernfs(&mut self, resolved: &Resolved, content: &str) -> FsResult {
        self.kernfs_values
            .push((resolved.guest.clone(), content.to_string()));
        if self.apply {
            if let Some(parent) = resolved.host.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(&resolved.host, content).map_err(|e| e.to_string())?;
        }
        Ok(Effect::Recorded(format!(
            "{} = {:?} (kernfs value for the syscall layer's {} emulation)",
            resolved.guest,
            content,
            if resolved.guest.starts_with("/proc") {
                "/proc"
            } else {
                "/sys"
            }
        )))
    }

    /// `do_mkdir`.
    pub fn mkdir(
        &mut self,
        path: &str,
        mode: Option<u32>,
        uid: Option<u32>,
        gid: Option<u32>,
    ) -> FsResult {
        if let Some(effect) = self.cgroup_noop("mkdir", path) {
            return Ok(effect);
        }
        let mode = mode.unwrap_or(0o755);
        let resolved = self.resolve(path, true)?;
        match &resolved.area {
            Area::Writable { .. } => {
                if self.apply {
                    match fs::create_dir(&resolved.host) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                            if !resolved.host.is_dir() {
                                return Err(format!("mkdir() failed on {path}: File exists"));
                            }
                        }
                        Err(e) => return Err(format!("mkdir() failed on {path}: {e}")),
                    }
                    fs::set_permissions(&resolved.host, fs::Permissions::from_mode(mode))
                        .map_err(|e| format!("fchmodat() failed on {path}: {e}"))?;
                }
                self.record_attrs(AttrRecord {
                    guest: resolved.guest.clone(),
                    uid: Some(uid.unwrap_or(0)),
                    gid: Some(gid.unwrap_or(0)),
                    mode: Some(mode),
                });
                Ok(Effect::Applied(format!(
                    "mkdir {} {:o} -> {}",
                    resolved.guest,
                    mode,
                    resolved.host.display()
                )))
            }
            Area::Kernfs { .. } => {
                if self.apply {
                    fs::create_dir_all(&resolved.host).map_err(|e| e.to_string())?;
                }
                Ok(Effect::Recorded(format!(
                    "mkdir {} (kernfs directory recorded; e.g. cgroup nodes are answered unsupported)",
                    resolved.guest
                )))
            }
            Area::HostDevice => Err(format!("mkdir() failed on {path}: File exists")),
            Area::ReadOnlyImage => {
                if resolved.host.is_dir() {
                    self.record_attrs(AttrRecord {
                        guest: resolved.guest.clone(),
                        uid: Some(uid.unwrap_or(0)),
                        gid: Some(gid.unwrap_or(0)),
                        mode: Some(mode),
                    });
                    Ok(Effect::NoOp(format!(
                        "mkdir {}: exists in the read-only image (owner/mode recorded in fs-attrs)",
                        resolved.guest
                    )))
                } else {
                    Err(format!(
                        "mkdir() failed on {path}: Read-only file system (not a writable area of the derived image)"
                    ))
                }
            }
        }
    }

    /// `do_chmod`.
    pub fn chmod(&mut self, mode: u32, path: &str) -> FsResult {
        if let Some(effect) = self.cgroup_noop("chmod", path) {
            return Ok(effect);
        }
        let resolved = self.resolve(path, true)?;
        match &resolved.area {
            Area::Writable { .. } => {
                if self.apply {
                    fs::set_permissions(&resolved.host, fs::Permissions::from_mode(mode))
                        .map_err(|e| format!("fchmodat() failed: {e}"))?;
                }
                self.record_attrs(AttrRecord {
                    guest: resolved.guest.clone(),
                    uid: None,
                    gid: None,
                    mode: Some(mode),
                });
                Ok(Effect::Applied(format!(
                    "chmod {mode:o} {}",
                    resolved.guest
                )))
            }
            Area::Kernfs { .. } | Area::ReadOnlyImage | Area::HostDevice => {
                self.record_attrs(AttrRecord {
                    guest: resolved.guest.clone(),
                    uid: None,
                    gid: None,
                    mode: Some(mode),
                });
                Ok(Effect::Recorded(format!(
                    "chmod {mode:o} {} (not writable here; mode recorded in fs-attrs)",
                    resolved.guest
                )))
            }
        }
    }

    /// `do_chown`: the host cannot chown to Android ids, so the owner is
    /// recorded for `stat`.
    pub fn chown(&mut self, uid: u32, gid: Option<u32>, path: &str) -> FsResult {
        if let Some(effect) = self.cgroup_noop("chown", path) {
            return Ok(effect);
        }
        let resolved = self.resolve(path, true)?;
        if self.apply
            && matches!(resolved.area, Area::Writable { .. })
            && fs::symlink_metadata(&resolved.host).is_err()
        {
            return Err(format!(
                "lchown() failed on {path}: No such file or directory"
            ));
        }
        self.record_attrs(AttrRecord {
            guest: resolved.guest.clone(),
            uid: Some(uid),
            gid,
            mode: None,
        });
        Ok(Effect::Recorded(format!(
            "chown {uid}:{} {} (fs-attrs)",
            gid.map_or("-".to_string(), |g| g.to_string()),
            resolved.guest
        )))
    }

    /// `do_write` (`WriteFile`: O_CREAT|O_TRUNC|O_NOFOLLOW, mode 0600).
    pub fn write(&mut self, path: &str, content: &str) -> FsResult {
        if let Some(effect) = self.cgroup_noop("write", path) {
            return Ok(effect);
        }
        let resolved = self.resolve(path, true)?;
        match &resolved.area {
            Area::Writable { .. } => {
                if self.apply {
                    let existed = resolved.host.exists();
                    fs::write(&resolved.host, content)
                        .map_err(|e| format!("Unable to write to file '{path}': {e}"))?;
                    if !existed {
                        let _ =
                            fs::set_permissions(&resolved.host, fs::Permissions::from_mode(0o600));
                    }
                }
                Ok(Effect::Applied(format!(
                    "write {} ({} bytes)",
                    resolved.guest,
                    content.len()
                )))
            }
            Area::Kernfs { .. } => self.record_kernfs(&resolved, content),
            Area::HostDevice => Ok(Effect::NoOp(format!(
                "write {}: host device sink",
                resolved.guest
            ))),
            Area::ReadOnlyImage => Err(format!(
                "Unable to write to file '{path}': Read-only file system"
            )),
        }
    }

    fn read_source(&self, source: &str) -> Result<(Resolved, Option<Vec<u8>>), String> {
        let resolved = self.resolve(source, true)?;
        let bytes = match resolved.area {
            // /proc and /sys contents come from the syscall layer, not from
            // this tree.
            Area::Kernfs { .. } | Area::HostDevice => None,
            _ => Some(
                fs::read(&resolved.host)
                    .map_err(|e| format!("Could not read input file '{source}': {e}"))?,
            ),
        };
        Ok((resolved, bytes))
    }

    /// `do_copy`.
    pub fn copy(&mut self, source: &str, target: &str) -> FsResult {
        if let Some(effect) = self.cgroup_noop("copy", target) {
            return Ok(effect);
        }
        let target_resolved = self.resolve(target, true)?;
        if target_resolved.area == Area::HostDevice {
            return Ok(Effect::NoOp(format!(
                "copy {source} {target}: entropy/sink host device; the host pool is already seeded"
            )));
        }
        if !self.apply && !matches!(self.resolve(source, true)?.area, Area::ReadOnlyImage) {
            return Ok(Effect::Applied(format!("copy {source} -> {target}")));
        }
        let (_, bytes) = self.read_source(source)?;
        let Some(bytes) = bytes else {
            return Ok(Effect::NoOp(format!(
                "copy {source} {target}: the source is a /proc or /sys file the syscall layer synthesizes"
            )));
        };
        self.write(target, &String::from_utf8_lossy(&bytes))
    }

    /// `do_copy_per_line`.
    pub fn copy_per_line(&mut self, source: &str, target: &str) -> FsResult {
        if let Some(effect) = self.cgroup_noop("copy_per_line", target) {
            return Ok(effect);
        }
        let (_, bytes) = match self.read_source(source) {
            Ok(v) => v,
            Err(e) if !self.apply => {
                return Ok(Effect::Applied(format!(
                    "copy_per_line {source} -> {target} ({e})"
                )));
            }
            Err(e) => return Err(e),
        };
        let Some(bytes) = bytes else {
            return Ok(Effect::NoOp(format!(
                "copy_per_line {source}: synthesized source"
            )));
        };
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let mut last = Effect::NoOp(format!("copy_per_line {source}: empty"));
        for line in text.lines() {
            last = self.write(target, line)?;
        }
        Ok(last)
    }

    /// `do_symlink` (EEXIST is not reported).
    pub fn symlink(&mut self, target: &str, link: &str) -> FsResult {
        if let Some(effect) = self.cgroup_noop("symlink", link) {
            return Ok(effect);
        }
        let resolved = self.resolve(link, false)?;
        match &resolved.area {
            Area::Writable { .. } => {
                if self.apply {
                    match std::os::unix::fs::symlink(target, &resolved.host) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(e) => {
                            return Err(format!(
                                "symlink() failed to create {link} -> {target}: {e}"
                            ));
                        }
                    }
                }
                Ok(Effect::Applied(format!(
                    "symlink {} -> {target}",
                    resolved.guest
                )))
            }
            _ if fs::symlink_metadata(&resolved.host).is_ok() => Ok(Effect::NoOp(format!(
                "symlink {}: exists in the read-only image",
                resolved.guest
            ))),
            _ => Err(format!(
                "symlink() failed to create {link} -> {target}: Read-only file system"
            )),
        }
    }

    /// `do_rm` / `do_rmdir`.
    pub fn remove(&mut self, path: &str, directory: bool) -> FsResult {
        if let Some(effect) = self.cgroup_noop("remove", path) {
            return Ok(effect);
        }
        let resolved = self.resolve(path, false)?;
        let verb = if directory { "rmdir" } else { "unlink" };
        match &resolved.area {
            Area::Writable { .. } => {
                if self.apply {
                    let result = if directory {
                        fs::remove_dir(&resolved.host)
                    } else {
                        fs::remove_file(&resolved.host)
                    };
                    result.map_err(|e| format!("{verb}() failed: {e}"))?;
                }
                Ok(Effect::Applied(format!("{verb} {}", resolved.guest)))
            }
            _ => Err(format!("{verb}() failed: Read-only file system")),
        }
    }

    /// `do_mount`: only a tmpfs on a writable area has an effect (the
    /// directory is the mount); everything else is explained.
    pub fn mount(
        &mut self,
        fs_type: &str,
        device: &str,
        target: &str,
        options: &[String],
    ) -> FsResult {
        let resolved = self.resolve(target, true)?;
        if options.iter().any(|o| o == "bind" || o == "rbind") || device.starts_with('/') {
            return Ok(Effect::NoOp(format!(
                "mount {device} {target} ({}): bind mounts need mount namespaces; one namespace with a fixed path map",
                options.join(",")
            )));
        }
        let reason = match fs_type {
            "tmpfs" => match resolved.area {
                Area::Writable { .. } => {
                    if self.apply {
                        fs::create_dir_all(&resolved.host).map_err(|e| e.to_string())?;
                    }
                    return Ok(Effect::Applied(format!(
                        "mount tmpfs {target}: backed by the writable directory {}",
                        resolved.host.display()
                    )));
                }
                _ => {
                    "a tmpfs over a read-only image directory would hide the pre-flattened content; not emulated"
                }
            },
            "cgroup" | "cgroup2" => CGROUP_REASON,
            "binder" | "binderfs" => "binder devices come from the syscall layer's binder driver",
            "proc" | "sysfs" => "/proc and /sys are synthesized by the syscall layer",
            "selinuxfs" | "securityfs" => "SELinux is permissive; no security filesystem",
            "tracefs" | "debugfs" | "pstore" | "bpf" | "configfs" | "functionfs" => {
                "kernel filesystem not emulated (tracing, pstore, eBPF, sdcardfs config and USB gadget are absent)"
            }
            "fuse" => "FUSE mounts are made by the MediaProvider path, not by init",
            _ => "block-device filesystems are pre-extracted into the derived image",
        };
        if !matches!(fs_type, "proc" | "sysfs" | "fuse")
            && self.map.lookup(target).1 != Area::ReadOnlyImage
        {
            self.add_unmounted_root(target, &format!("{fs_type}: {reason}"));
        }
        Ok(Effect::NoOp(format!(
            "mount {fs_type} {device} {target}: {reason}"
        )))
    }

    /// `do_wait`: ueventd's device nodes do not exist; a path in the guest
    /// view either exists already or never will.
    pub fn wait(&mut self, path: &str) -> FsResult {
        let resolved = self.resolve(path, true)?;
        if resolved.area == Area::HostDevice || resolved.host.exists() {
            Ok(Effect::Applied(format!("wait {path}: present")))
        } else {
            Ok(Effect::NoOp(format!(
                "wait {path}: no device nodes appear (the ueventd role creates none); not waiting"
            )))
        }
    }

    /// `hostname` / `domainname` write `/proc/sys/kernel/*`.
    pub fn kernel_value(&mut self, guest: &str, value: &str) -> FsResult {
        let resolved = self.resolve(guest, true)?;
        self.record_kernfs(&resolved, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Layout;

    fn temp_layout(tag: &str) -> Layout {
        let root = std::env::temp_dir().join(format!("dgi-fsops-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let image = root.join("image");
        fs::create_dir_all(image.join("system/bin")).unwrap();
        std::os::unix::fs::symlink("/system/bin", image.join("bin")).unwrap();
        let layout = Layout::new(image, root.join("data"), Some(root.join("run")));
        layout.prepare().unwrap();
        layout
    }

    #[test]
    fn commands_act_on_mapped_areas() {
        let layout = temp_layout("areas");
        let mut ops = FsOps::new(layout.path_map(), layout.fs_attrs_file(), true);
        assert!(matches!(
            ops.mkdir("/data/misc", Some(0o1771), Some(1000), Some(1000))
                .unwrap(),
            Effect::Applied(_)
        ));
        let meta = fs::metadata(layout.data.join("data/misc")).unwrap();
        assert_eq!(meta.permissions().mode() & 0o7777, 0o1771);
        ops.write("/data/misc/x", "hello").unwrap();
        assert_eq!(
            fs::read_to_string(layout.data.join("data/misc/x")).unwrap(),
            "hello"
        );
        ops.symlink("/proc/self/fd/0", "/dev/stdin").unwrap();
        assert_eq!(
            fs::read_link(layout.runtime.join("dev/stdin")).unwrap(),
            PathBuf::from("/proc/self/fd/0")
        );
        // Symlinks resolve relative to the guest root, through the image.
        assert!(matches!(
            ops.mkdir("/bin", None, None, None).unwrap(),
            Effect::NoOp(_)
        ));
        assert!(ops.mkdir("/system/newdir", None, None, None).is_err());
        assert!(ops.write("/system/x", "1").is_err());
        let recorded = ops.write("/proc/sys/vm/overcommit_memory", "1").unwrap();
        assert!(matches!(recorded, Effect::Recorded(_)));
        assert_eq!(
            fs::read_to_string(layout.kernfs_dir().join("proc/sys/vm/overcommit_memory")).unwrap(),
            "1"
        );
        ops.chown(1036, Some(1036), "/data/misc/x").unwrap();
        let attrs = fs::read_to_string(layout.fs_attrs_file()).unwrap();
        assert!(attrs.contains("/data/misc\t1000\t1000\t1771\n"), "{attrs}");
        assert!(attrs.contains("/data/misc/x\t1036\t1036\t-\n"), "{attrs}");
        assert!(matches!(
            ops.mount("tmpfs", "tmpfs", "/mnt/x", &[]).unwrap(),
            Effect::Applied(_)
        ));
        assert!(layout.runtime.join("mnt/x").is_dir());
        assert!(matches!(
            ops.mount("cgroup2", "none", "/sys/fs/cgroup", &[]).unwrap(),
            Effect::NoOp(_)
        ));
        ops.mount("configfs", "none", "/config", &[]).unwrap();
        assert!(matches!(
            ops.mkdir("/config/sdcardfs/extensions/1055", None, None, None)
                .unwrap(),
            Effect::NoOp(_)
        ));
        ops.add_unmounted_root("/dev/cpuctl", CGROUP_REASON);
        assert!(matches!(
            ops.write("/dev/cpuctl/top-app/cpu.shares", "1024").unwrap(),
            Effect::NoOp(_)
        ));
        assert!(!layout.runtime.join("dev/cpuctl").exists());
        assert!(matches!(
            ops.copy("/proc/cmdline", "/dev/urandom").unwrap(),
            Effect::NoOp(_)
        ));
        ops.remove("/data/misc/x", false).unwrap();
        assert!(!layout.data.join("data/misc/x").exists());
        let _ = fs::remove_dir_all(layout.data.parent().unwrap());
    }
}
