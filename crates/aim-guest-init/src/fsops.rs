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
//! - Ownership and modes: the host cannot `chown` to Android ids, so they
//!   are recorded for the syscall layer's `stat` on the host inode of a
//!   writable area ([`crate::guest_inode`]), and for the read-only image and
//!   paths with no host inode in the fs-attrs table.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::paths::{Area, MapEntry, MapKind, PathMap, Resolved};

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

/// A recorded owner and mode; in the fs-attrs table, the line
/// `<guest path>\t<uid|->\t<gid|->\t<octal mode|->`.
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

/// The host mode for a guest `mode`. Every guest uid is the same host user,
/// and the guest mode is the recorded one, so the owner keeps rw(x) on
/// the host: `mkdir /data/user 0511 system system` must not stop root
/// creating `/data/user/0` in it. The layer's `fchmodat` does the same.
fn host_permissions(mode: u32, host: &Path) -> fs::Permissions {
    let owner = if host.is_dir() { 0o700 } else { 0o600 };
    fs::Permissions::from_mode(mode | owner)
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
    /// The cgroup v2 hierarchy (path, mode, uid, gid), a plain directory.
    pub cgroup2: Option<(String, u32, u32, u32)>,
    /// The `--path-map` file processes start with; init's bind mounts
    /// become entries of it.
    path_map_file: Option<PathBuf>,
}

/// Legacy cgroup v1 controllers init.rc still addresses although the
/// image's `cgroups.json` no longer declares them.
pub const LEGACY_CGROUP_ROOTS: &[&str] = &["/dev/memcg", "/dev/stune"];

pub const CGROUP_REASON: &str =
    "cgroup v1 controllers are answered unsupported (ADR 0012 appendix)";

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
            cgroup2: None,
            path_map_file: None,
        }
    }

    /// Where the path map is written when a bind mount changes it.
    pub fn set_path_map_file(&mut self, file: PathBuf) {
        self.path_map_file = Some(file);
    }

    /// `mount none SRC DST bind [rec]`: init mounts in the one namespace
    /// every process shares (the data mirrors zygote's app data isolation
    /// binds from). Between writable areas that is a path-map entry, seen
    /// by every process started from then on; nothing running then does
    /// not see it (no mount propagation). With `rec`, the entries below
    /// SRC come along (the mirror of /data/user gets its /data/user/0).
    fn bind(&mut self, source: &str, target: &str, rec: bool) -> Result<Option<Effect>, String> {
        let from = self.resolve(source, true)?;
        let to = self.resolve(target, true)?;
        if !matches!(from.area, Area::Writable { .. }) || !matches!(to.area, Area::Writable { .. })
        {
            return Ok(None);
        }
        if self.apply && !from.host.is_dir() {
            return Err(format!(
                "mount {source} {target}: {source} is not a directory"
            ));
        }
        let below: Vec<MapEntry> = if rec {
            let prefix = format!("{}/", from.guest);
            self.map
                .entries()
                .iter()
                .filter_map(|e| {
                    let rest = e.guest.strip_prefix(&prefix)?;
                    Some(MapEntry {
                        guest: format!("{}/{rest}", to.guest),
                        ..e.clone()
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        self.map.add(MapEntry {
            guest: to.guest.clone(),
            host: from.host.clone(),
            kind: MapKind::Writable,
        });
        self.map.bind_source(&to.guest,&from.guest);
        for entry in below {
            self.map.add(entry);
        }
        if self.apply
            && let Some(file) = &self.path_map_file
        {
            fs::write(file, self.map.to_file_text()).map_err(|e| e.to_string())?;
        }
        Ok(Some(Effect::Applied(format!(
            "mount {source} {target} bind: path-map entry {} -> {}",
            to.guest,
            from.host.display()
        ))))
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

    /// Records an owner and mode: on the host inode of a writable area (or
    /// an existing kernfs value), else in the fs-attrs table.
    fn record_attrs(
        &mut self,
        resolved: &Resolved,
        uid: Option<u32>,
        gid: Option<u32>,
        mode: Option<u32>,
    ) {
        let record = AttrRecord {
            guest: resolved.guest.clone(),
            uid,
            gid,
            mode,
        };
        if self.apply {
            let on_inode = matches!(resolved.area, Area::Writable { .. } | Area::Kernfs { .. })
                && fs::symlink_metadata(&resolved.host).is_ok()
                && crate::guest_inode::record(
                    &resolved.host,
                    crate::guest_inode::GuestInode { uid, gid, mode },
                )
                .is_ok();
            if !on_inode
                && let Ok(mut file) = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.attrs_file)
            {
                let _ = file.write_all(record.line().as_bytes());
            }
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

    /// `do_mkdir`. The guest mode is recorded; on the host the owner
    /// keeps access (`host_permissions`).
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
                    fs::set_permissions(&resolved.host, host_permissions(mode, &resolved.host))
                        .map_err(|e| format!("fchmodat() failed on {path}: {e}"))?;
                }
                self.record_attrs(
                    &resolved,
                    Some(uid.unwrap_or(0)),
                    Some(gid.unwrap_or(0)),
                    Some(mode),
                );
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
                    self.record_attrs(
                        &resolved,
                        Some(uid.unwrap_or(0)),
                        Some(gid.unwrap_or(0)),
                        Some(mode),
                    );
                    Ok(Effect::NoOp(format!(
                        "mkdir {}: exists in the read-only image (owner/mode recorded)",
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
                    fs::set_permissions(&resolved.host, host_permissions(mode, &resolved.host))
                        .map_err(|e| format!("fchmodat() failed: {e}"))?;
                }
                self.record_attrs(&resolved, None, None, Some(mode));
                Ok(Effect::Applied(format!(
                    "chmod {mode:o} {}",
                    resolved.guest
                )))
            }
            Area::Kernfs { .. } | Area::ReadOnlyImage | Area::HostDevice => {
                self.record_attrs(&resolved, None, None, Some(mode));
                Ok(Effect::Recorded(format!(
                    "chmod {mode:o} {} (not writable here; mode recorded)",
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
        self.record_attrs(&resolved, Some(uid), gid, None);
        Ok(Effect::Recorded(format!(
            "chown {uid}:{} {} (recorded)",
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
        if let Some(kind)=options.iter().find(|option|matches!(option.as_str(),"shared"|"slave"|"private")){
            self.map.propagation(&resolved.guest,kind,options.iter().any(|option|option=="rec"));
            if self.apply&&let Some(file)=&self.path_map_file{fs::write(file,self.map.to_file_text()).map_err(|error|error.to_string())?;}
            return Ok(Effect::Applied(format!("mount propagation {} {kind}",resolved.guest)));
        }

        if options.iter().any(|o| o == "bind" || o == "rbind") || device.starts_with('/') {
            let rec = options.iter().any(|o| o == "rec" || o == "rbind");
            if options.iter().any(|o| o == "bind" || o == "rbind")
                && let Some(effect) = self.bind(device, target, rec)?
            {
                return Ok(effect);
            }
            return Ok(Effect::NoOp(format!(
                "mount {device} {target} ({}): only binds between writable areas become path-map entries",
                options.join(",")
            )));
        }
        // bpffs and the cgroup v2 hierarchy are areas of the path map.
        if matches!(fs_type, "bpf" | "cgroup2") && matches!(resolved.area, Area::Writable { .. }) {
            // A new bpffs root is 01777 (bpf_fill_super), a cgroup2 root 0755.
            let mode = if fs_type == "bpf" { 0o1777 } else { 0o755 };
            self.record_attrs(&resolved, Some(0), Some(0), Some(mode));
            return Ok(Effect::Applied(format!(
                "mount {fs_type} {target}: the path map's {fs_type} area ({})",
                resolved.host.display()
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
    fn init_storage_bind_and_slave_keep_explicit_namespace_relations(){
        let layout=temp_layout("storage-propagation");let mut ops=FsOps::new(layout.path_map(),layout.fs_attrs_file(),true);ops.set_path_map_file(layout.path_map_file());
        ops.mkdir("/mnt/user",Some(0o755),None,None).unwrap();
        ops.mkdir("/mnt/user/0",Some(0o755),None,None).unwrap();
        ops.mount("none","/mnt/user/0","/storage",&["bind".into(),"rec".into()]).unwrap();
        ops.mount("none","none","/storage",&["slave".into(),"rec".into()]).unwrap();
        // Later original init binds reorder map entries but retain all prior
        // namespace relationships and policies.
        ops.mkdir("/data/user_de",Some(0o755),None,None).unwrap();
        ops.mkdir("/data_mirror/metadata-test",Some(0o755),None,None).unwrap();
        ops.mount("none","/data/user_de","/data_mirror/metadata-test",&["bind".into(),"rec".into()]).unwrap();
        let text=fs::read_to_string(layout.path_map_file()).unwrap();
        assert!(text.contains("bind-source\t/storage\t/mnt/user/0\n"));assert!(text.contains("propagation\t/storage\tslave\n"));
        let parsed=crate::paths::PathMap::parse_file_text(&text).unwrap();assert_eq!(parsed.to_file_text(),text);
    }
    #[test]
    fn init_bind_mounts_become_path_map_entries() {
        let layout = temp_layout("bind");
        let mut ops = FsOps::new(layout.path_map(), layout.fs_attrs_file(), true);
        ops.set_path_map_file(layout.path_map_file());
        ops.mkdir("/data/user_de", Some(0o711), None, None).unwrap();
        ops.mkdir("/data/user_de/0", Some(0o771), None, None)
            .unwrap();
        ops.mount("tmpfs", "tmpfs", "/data_mirror", &[]).unwrap();
        ops.mkdir("/data_mirror/data_de", None, None, None).unwrap();
        ops.mkdir("/data_mirror/data_de/null", None, None, None)
            .unwrap();
        let bind = ["bind".to_string(), "rec".to_string()];
        assert!(matches!(
            ops.mount("none", "/data/user_de", "/data_mirror/data_de/null", &bind)
                .unwrap(),
            Effect::Applied(_)
        ));
        let r = ops
            .map
            .resolve("/data_mirror/data_de/null/0", true)
            .unwrap();
        assert_eq!(r.host, layout.data.join("data/user_de/0"));
        // New processes read the entry from the file.
        let text = fs::read_to_string(layout.path_map_file()).unwrap();
        let map = PathMap::parse_file_text(&text).unwrap();
        let r = map.resolve("/data_mirror/data_de/null/0", true).unwrap();
        assert_eq!(r.host, layout.data.join("data/user_de/0"));
        // The mirror of /data/user holds user 0's CE storage, /data/data,
        // as vold's bind propagates to it on Android.
        ops.mkdir("/data/user", Some(0o511), None, None).unwrap();
        ops.mkdir("/data_mirror/data_ce", None, None, None).unwrap();
        ops.mkdir("/data_mirror/data_ce/null", None, None, None)
            .unwrap();
        ops.mount("none", "/data/user", "/data_mirror/data_ce/null", &bind)
            .unwrap();
        let r = ops
            .map
            .resolve("/data_mirror/data_ce/null/0/com.example", true)
            .unwrap();
        assert_eq!(r.host, layout.data.join("data/data/com.example"));
        assert!(layout.data.join("data/user/0").is_dir());
        // A bind out of the read-only image stays unsupported.
        assert!(matches!(
            ops.mount("none", "/system/bin", "/data_mirror/data_de/null", &bind)
                .unwrap(),
            Effect::NoOp(_)
        ));
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
        // Owners of writable files are on their inodes, those of image
        // paths in the table.
        use crate::guest_inode::{GuestInode, read};
        assert_eq!(
            read(&layout.data.join("data/misc")).unwrap(),
            Some(GuestInode {
                uid: Some(1000),
                gid: Some(1000),
                mode: Some(0o1771)
            })
        );
        assert_eq!(
            read(&layout.data.join("data/misc/x")).unwrap(),
            Some(GuestInode {
                uid: Some(1036),
                gid: Some(1036),
                mode: None
            })
        );
        let table = fs::read_to_string(layout.fs_attrs_file()).unwrap();
        assert!(!table.contains("/data/"), "{table}");
        assert!(table.contains("\t0\t0\t755\n"), "{table}");
        assert!(matches!(
            ops.mount("tmpfs", "tmpfs", "/mnt/x", &[]).unwrap(),
            Effect::Applied(_)
        ));
        assert!(layout.runtime.join("mnt/x").is_dir());
        // The cgroup v2 hierarchy and bpffs are areas of the path map.
        for (fs, target) in [("cgroup2", "/sys/fs/cgroup"), ("bpf", "/sys/fs/bpf")] {
            assert!(matches!(
                ops.mount(fs, "none", target, &[]).unwrap(),
                Effect::Applied(_)
            ));
        }
        assert!(matches!(
            ops.mkdir("/sys/fs/bpf/netd_shared", Some(0o755), None, None)
                .unwrap(),
            Effect::Applied(_)
        ));
        assert!(layout.bpf_dir().join("netd_shared").is_dir());
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
