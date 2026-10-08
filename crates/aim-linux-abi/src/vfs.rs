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
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};
use std::sync::atomic::{AtomicBool,Ordering};

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

#[derive(Clone)]
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
    fuse: Option<FuseRoute>,
    shared_fuse: bool,
    bind_source: Option<String>,
    projected_fuse: bool,
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]enum Propagation{Private,Slave,Shared}
static PROPAGATION_RULES:Mutex<Vec<(String,Propagation)>>=Mutex::new(Vec::new());
fn mount_propagation(guest:&str)->Propagation{
    PROPAGATION_RULES.lock().unwrap().iter().filter(|(root,_)|root=="/"||below(root,guest).is_some()).max_by_key(|(root,_)|root.len()).map(|(_,kind)|*kind)
        .unwrap_or(if PRIVATE_MOUNTS.load(Ordering::Acquire){Propagation::Private}else{Propagation::Shared})
}
pub fn set_mount_propagation(guest:&str,flags:u64)->Result<(),Errno>{
    let kind=if flags&(1<<19)!=0{Propagation::Slave}else if flags&(1<<20)!=0{Propagation::Shared}else{Propagation::Private};
    let mut rules=PROPAGATION_RULES.lock().unwrap();rules.retain(|(root,_)|root!=guest&&(!(flags&0x4000!=0)||(guest!="/"&&below(guest,root).is_none())));rules.push((guest.into(),kind));drop(rules);
    refresh_fuse_mounts()
}
static PRIVATE_MOUNTS:AtomicBool=AtomicBool::new(false);
fn fuse_table()->Option<PathBuf>{runtime_dir().map(|runtime|runtime.join("fuse-mounts"))}
struct FuseTableLock(std::fs::File);
impl Drop for FuseTableLock{fn drop(&mut self){unsafe{libc::flock(std::os::fd::AsRawFd::as_raw_fd(&self.0),libc::LOCK_UN);}}}
fn fuse_table_lock()->Result<FuseTableLock,Errno>{
    let path=fuse_table().ok_or(errno::ENODEV)?.with_extension("lock");
    let file=std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).mode(0o600).open(path).map_err(|error|errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?;
    lock_mount_table(std::os::fd::AsRawFd::as_raw_fd(&file))?;
    Ok(FuseTableLock(file))
}
// This lock belongs to the kernel's namespace journal. A guest signal must
// not interrupt it and turn an unrelated path lookup into an I/O error.
fn lock_mount_table(fd: i32) -> Result<(), Errno> {
    loop {
        if unsafe { libc::flock(fd, libc::LOCK_EX) } == 0 {
            return Ok(());
        }
        let error = errno::last();
        if error != errno::EINTR {
            return Err(error);
        }
    }
}
fn fuse_table_lines()->Result<Vec<String>,Errno>{match std::fs::read_to_string(fuse_table().ok_or(errno::ENODEV)?){Ok(text)=>Ok(text.lines().map(str::to_owned).collect()),Err(error)if error.kind()==std::io::ErrorKind::NotFound=>Ok(Vec::new()),Err(_)=>Err(errno::EIO)}}
fn publish_fuse_route(guest:&str,host:&Path,source:&str,route:Option<&FuseRoute>)->Result<(),Errno>{
    if mount_propagation(guest)!=Propagation::Shared{return Ok(());}
    let _lock=fuse_table_lock()?;let mut lines=fuse_table_lines()?;
    lines.retain(|line|line.split('\t').next()!=Some(guest));
    if let Some(route)=route{
        let fields=[guest.to_owned(),host.display().to_string(),source.to_owned(),route.session.display().to_string(),route.relative.clone(),format!("{},{},{},{},{}",route.uid,route.gid,u8::from(route.allow_other),u8::from(route.default_permissions),u8::from(route.read_only))];
        if fields.iter().any(|field|field.contains(['\t','\n'])){return Err(errno::EINVAL);}
        lines.push(fields.join("\t"));
    }
    let path=fuse_table().ok_or(errno::ENODEV)?;let temporary=path.with_extension(format!("{}.new",std::process::id()));
    use std::io::Write as _;
    let mut output=std::fs::OpenOptions::new().create(true).truncate(true).write(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).mode(0o600).open(&temporary).map_err(|_|errno::EIO)?;
    output.write_all(lines.join("\n").as_bytes()).map_err(|_|errno::EIO)?;output.sync_all().map_err(|_|errno::EIO)?;
    std::fs::rename(&temporary,&path).map_err(|_|errno::EIO)?;
    std::fs::File::open(path.parent().ok_or(errno::EIO)?).and_then(|directory|directory.sync_all()).map_err(|_|errno::EIO)
}
fn publish_plain_mount(guest:&str,host:&Path,area:Area,source:&str,fstype:&str,bind_source:Option<&str>)->Result<(),Errno>{
    if mount_propagation(guest)!=Propagation::Shared{return Ok(());}
    let fields=[guest.to_owned(),host.display().to_string(),source.to_owned(),"plain".into(),if area==Area::Image{"ro".into()}else{"rw".into()},fstype.to_owned(),bind_source.unwrap_or("").to_owned()];
    if fields.iter().any(|field|field.contains(['\t','\n'])){return Err(errno::EINVAL);}
    let _lock=fuse_table_lock()?;let mut lines=fuse_table_lines()?;lines.retain(|line|line.split('\t').next()!=Some(guest));lines.push(fields.join("\t"));
    let path=fuse_table().ok_or(errno::ENODEV)?;let temporary=path.with_extension(format!("{}.new",std::process::id()));use std::io::Write as _;
    let mut output=std::fs::OpenOptions::new().create(true).truncate(true).write(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).mode(0o600).open(&temporary).map_err(|_|errno::EIO)?;
    output.write_all(lines.join("\n").as_bytes()).map_err(|_|errno::EIO)?;output.sync_all().map_err(|_|errno::EIO)?;std::fs::rename(&temporary,&path).map_err(|_|errno::EIO)?;
    std::fs::File::open(path.parent().ok_or(errno::EIO)?).and_then(|directory|directory.sync_all()).map_err(|_|errno::EIO)
}
pub fn refresh_fuse_mounts()->Result<(),Errno>{
    if fuse_table().is_none(){return Ok(());}
    let _lock=fuse_table_lock()?;let lines=fuse_table_lines()?;let mut mounts=vfs().mounts.write().unwrap();
    let old=mounts.iter().filter(|mount|mount.shared_fuse).map(|mount|(mount.guest.clone(),mount.seq)).collect::<HashMap<_,_>>();
    mounts.retain(|mount|(!mount.shared_fuse&&!mount.projected_fuse)||mount_propagation(&mount.guest)==Propagation::Private);
    let mut seq=mounts.iter().map(|mount|mount.seq).max().unwrap_or(0);
    for line in lines{
        let fields=line.split('\t').collect::<Vec<_>>();if !matches!(fields.len(),6|7)||!fields[0].starts_with('/'){return Err(errno::EIO);}
        if mount_propagation(fields[0])==Propagation::Private{continue;}
        let mount_seq=match old.get(fields[0]){Some(seq)=>*seq,None=>{seq+=1;seq}};
        if fields.len()==7{
            if fields[3]!="plain"{return Err(errno::EIO);}
            mounts.push(Mount{guest:fields[0].into(),host:PathBuf::from(fields[1]),area:if fields[4]=="ro"{Area::Image}else{Area::Writable},source:Some(fields[2].into()),fstype:Some(fields[5].into()),own:true,seq:mount_seq,fuse:None,shared_fuse:true,bind_source:(!fields[6].is_empty()).then(||fields[6].into()),projected_fuse:false});
        }else{
            let policy=fields[5].split(',').map(|value|value.parse::<u32>().map_err(|_|errno::EIO)).collect::<Result<Vec<_>,_>>()?;if policy.len()!=5{return Err(errno::EIO);}
            mounts.push(Mount{guest:fields[0].into(),host:PathBuf::from(fields[1]),area:Area::Writable,source:Some(fields[2].into()),fstype:Some("fuse".into()),own:true,seq:mount_seq,fuse:Some(FuseRoute{session:PathBuf::from(fields[3]),relative:fields[4].into(),uid:policy[0],gid:policy[1],allow_other:policy[2]!=0,default_permissions:policy[3]!=0,read_only:policy[4]!=0}),shared_fuse:true,bind_source:None,projected_fuse:false});
        }
    }
    // Propagated parent events retain the bind mount's actual source subtree.
    // Iterate to cover installer -> storage aliases; no host path inference.
    for _ in 0..16{
        let aliases=mounts.iter().filter(|mount|mount.bind_source.is_some()&&!hidden(&mounts,mount)).cloned().collect::<Vec<_>>();
        let routes=mounts.iter().filter(|mount|mount.own&&!hidden(&mounts,mount)).cloned().collect::<Vec<_>>();let mut added=false;
        for alias in aliases{let source=alias.bind_source.as_ref().unwrap();if mount_propagation(&alias.guest)==Propagation::Private{continue;}
            for route in &routes{if below(&alias.guest,&route.guest).is_some(){continue;}let Some(relative)=below(source,&route.guest)else{continue;};if relative.is_empty(){continue;}
                let guest=format!("{}/{relative}",alias.guest.trim_end_matches('/'));
                if mounts.iter().any(|mount|mount.guest==guest&&!hidden(&mounts,mount)){continue;}
                // A newer explicit child mount hides inherited events beneath it.
                if mounts.iter().any(|mount|mount.seq>alias.seq&&!mount.projected_fuse&&mount.guest!=alias.guest&&below(&alias.guest,&mount.guest).is_some()&&below(&mount.guest,&guest).is_some()){continue;}
                seq+=1;let mut projected=route.clone();projected.guest=guest;projected.seq=seq;projected.shared_fuse=false;projected.projected_fuse=true;projected.bind_source=None;
                if alias.area==Area::Image{if let Some(route)=projected.fuse.as_mut(){route.read_only=true;}projected.area=Area::Image;}
                mounts.push(projected);added=true;
            }
        }
        if !added{break;}
    }
    mounts.sort_by(|a,b|b.guest.len().cmp(&a.guest.len()).then(b.seq.cmp(&a.seq)));Ok(())
}
pub fn private_mount_namespace()->Result<(),Errno>{
    refresh_fuse_mounts()?;
    // CLONE_NEWNS copies propagation relationships; MS_PRIVATE/MS_SLAVE
    // subsequently changes them at the guest's explicit mount operation.
    if PROPAGATION_RULES.lock().unwrap().is_empty(){PROPAGATION_RULES.lock().unwrap().push(("/".into(),Propagation::Shared));}
    PRIVATE_MOUNTS.store(true,Ordering::Release);Ok(())
}

/// A FUSE mount owns a daemon session transport, never a raw-media path.
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct FuseRoute {pub session:PathBuf,pub relative:String,pub uid:u32,pub gid:u32,pub allow_other:bool,pub default_permissions:bool,pub read_only:bool}
pub fn fuse_route(guest:&str)->Option<FuseRoute>{
    let mounts=vfs().mounts.read().unwrap();let(mount,rest)=holder(&mounts,guest)?;
    let mut route=mount.fuse.clone()?;
    route.relative=if rest.is_empty(){route.relative}else if route.relative.is_empty(){rest.to_owned()}else{format!("{}/{rest}",route.relative)};
    Some(route)
}
pub fn add_fuse_mount(guest:&str,session:PathBuf,host_anchor:PathBuf,source:&str,options:&crate::sys::fuse_mount::Options,read_only:bool)->Result<(),Errno>{
    let route=FuseRoute{session:session.clone(),relative:String::new(),uid:options.uid,gid:options.gid,allow_other:options.allow_other,default_permissions:options.default_permissions,read_only};
    publish_fuse_route(guest,&host_anchor,source,Some(&route))?;
    add_mount(guest,host_anchor,Area::Writable,source,"fuse");
    let mut mounts=vfs().mounts.write().unwrap();
    if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==guest&&mount.own){mount.fuse=Some(route);mount.shared_fuse=mount_propagation(guest)==Propagation::Shared;}
    Ok(())
}
pub fn fuse_route_for_session(session:&Path)->Option<(String,FuseRoute)>{
    let mounts=vfs().mounts.read().unwrap();mounts.iter().filter(|mount|!hidden(&mounts,mount))
        .find_map(|mount|mount.fuse.as_ref().filter(|route|route.session==session).map(|route|(mount.guest.clone(),route.clone())))
}
/// A bind aliases the same FUSE session and mount-relative node tree.
pub fn bind_mount(source_guest:&str,target_guest:&str,host:PathBuf,area:Area,source:&str,fstype:&str)->Result<(),Errno>{
    refresh_fuse_mounts()?;
    let mut route=fuse_route(source_guest);if area==Area::Image{if let Some(route)=&mut route{route.read_only=true;}}if let Some(route)=&route{publish_fuse_route(target_guest,&host,source,Some(route))?;}else{publish_plain_mount(target_guest,&host,area,source,fstype,Some(source_guest))?;}add_mount(target_guest,host,area,source,fstype);
    {let mut mounts=vfs().mounts.write().unwrap();if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==target_guest&&mount.own){mount.bind_source=Some(source_guest.into());mount.shared_fuse=mount_propagation(target_guest)==Propagation::Shared;}}
    if let Some(route)=route{let mut mounts=vfs().mounts.write().unwrap();if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==target_guest&&mount.own){mount.fuse=Some(route);mount.shared_fuse=mount_propagation(target_guest)==Propagation::Shared;}}
    Ok(())
}

/// MS_REC clones the currently visible mount tree, not its host directories.
/// Nonrecursive bind clones only the source mount's selected subtree.
pub fn bind_mount_recursive(source_guest:&str,target_guest:&str,host:PathBuf,area:Area,recursive:bool)->Result<(),Errno>{
    refresh_fuse_mounts()?;
    let children=if recursive{
        let mounts=vfs().mounts.read().unwrap();let mut children=mounts.iter()
            .filter(|mount|mount.guest!=source_guest&&below(source_guest,&mount.guest).is_some()&&!hidden(&mounts,mount))
            .cloned().collect::<Vec<_>>();children.sort_by_key(|mount|(mount.guest.len(),mount.seq));children
    }else{Vec::new()};
    bind_mount(source_guest,target_guest,host,area,source_guest,"bind")?;
    {let mut mounts=vfs().mounts.write().unwrap();if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==target_guest&&mount.own){mount.bind_source=Some(source_guest.into());}}
    for child in children{
        let relative=below(source_guest,&child.guest).ok_or(errno::EINVAL)?;
        let guest=format!("{}/{relative}",target_guest.trim_end_matches('/'));
        let child_area=if area==Area::Image{Area::Image}else{child.area};
        let mut route=child.fuse.clone();if child_area==Area::Image{if let Some(route)=&mut route{route.read_only=true;}}
        if let Some(route)=&route{publish_fuse_route(&guest,&child.host,child.source.as_deref().unwrap_or("fuse"),Some(route))?;}else{publish_plain_mount(&guest,&child.host,child_area,child.source.as_deref().unwrap_or("bind"),child.fstype.as_deref().unwrap_or("bind"),child.bind_source.as_deref())?;}
        add_mount(&guest,child.host,child_area,child.source.as_deref().unwrap_or("bind"),child.fstype.as_deref().unwrap_or("bind"));
        {let mut mounts=vfs().mounts.write().unwrap();if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==guest&&mount.own){mount.fuse=route;mount.shared_fuse=mount_propagation(&guest)==Propagation::Shared;}}

    }
    Ok(())
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
    let mut mounts:Vec<Mount> = Vec::new();
    for (n, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if let ["bind-source",guest,source]=f.as_slice(){
            let mount: &mut Mount=mounts.iter_mut().find(|mount|mount.guest==*guest).ok_or_else(||format!("line {}: bind source has no map owner",n+1))?;
            if !source.starts_with('/')||source.contains(['\n','\t']){return Err(format!("line {}: invalid bind source",n+1));}
            mount.bind_source=Some((*source).into());continue;
        }
        if let ["propagation",guest,kind]=f.as_slice(){
            let propagation=match *kind{"slave"=>Propagation::Slave,"private"=>Propagation::Private,"shared"=>Propagation::Shared,_=>return Err(format!("line {}: invalid propagation",n+1))};
            PROPAGATION_RULES.lock().unwrap().push(((*guest).into(),propagation));continue;
        }
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
            fuse: None,
            shared_fuse: false,
            bind_source: None,
            projected_fuse: false,
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
            fuse: None,
            shared_fuse: false,
            bind_source: None,
            projected_fuse: false,
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

/// Active guest mount roots, including file binds and mapped directory roots.
pub fn is_mountpoint(guest: &str) -> bool {
    let mounts = vfs().mounts.read().unwrap();
    mounts.iter().any(|mount| mount.guest == guest && !hidden(&mounts, mount) && (mount.own || mount.host.is_dir()))
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
            fuse: None,
            shared_fuse: false,
            bind_source: None,
            projected_fuse: false,
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

pub fn remount_fuse(guest:&str,read_only:bool)->Result<bool,Errno>{
    refresh_fuse_mounts()?;
    let mount={let mounts=vfs().mounts.read().unwrap();mounts.iter().find(|mount|mount.guest==guest&&mount.fuse.is_some()).cloned()};
    let Some(mount)=mount else{return Ok(false)};
    let seq=mount.seq;let mut route=mount.fuse.unwrap();route.read_only=read_only;
    publish_fuse_route(guest,&mount.host,mount.source.as_deref().unwrap_or("fuse"),Some(&route))?;
    let mut mounts=vfs().mounts.write().unwrap();if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==guest&&mount.seq==seq){mount.fuse=Some(route);}Ok(true)
}

/// Remove the latest mount this process made at `guest`.
pub fn remove_mount(guest: &str) -> bool {
    if vfs().mounts.read().unwrap().iter().any(|mount|mount.guest==guest&&mount.shared_fuse)&&publish_fuse_route(guest,Path::new(""),"",None).is_err(){return false;}
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
        let route=m.fuse;
        add_mount(&guest, m.host, m.area, &source, &fstype);
        if let Some(route)=route{let mut mounts=vfs().mounts.write().unwrap();if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==guest&&mount.own){mount.fuse=Some(route);}}
    }
    true
}

/// This process's own mounts, oldest first, as `--mounts` text for the
/// program it execs: `ro|rw<TAB>guest<TAB>host<TAB>source<TAB>fstype`
/// lines.
pub fn own_mounts_text() -> String {
    let mounts = vfs().mounts.read().unwrap();
    let mut out = if PRIVATE_MOUNTS.load(Ordering::Acquire){"namespace\tprivate\n".to_owned()}else{String::new()};
    for(root,kind)in PROPAGATION_RULES.lock().unwrap().iter(){out.push_str(&format!("propagation\t{root}\t{}\n",match kind{Propagation::Private=>"private",Propagation::Slave=>"slave",Propagation::Shared=>"shared"}));}
    let mut own: Vec<&Mount> = mounts.iter().filter(|m| m.own).collect();
    own.reverse();
    for m in own {
        let area = if m.area == Area::Image { "ro" } else { "rw" };
        out.push_str(&format!(
            "{area}\t{}\t{}\t{}\t{}",
            m.guest,
            m.host.display(),
            m.source.as_deref().unwrap_or_default(),
            m.fstype.as_deref().unwrap_or_default()
        ));
        if let Some(route)=&m.fuse{out.push_str(&format!("\t{}\t{}\t{},{},{},{},{}",route.session.display(),route.relative,route.uid,route.gid,u8::from(route.allow_other),u8::from(route.default_permissions),u8::from(route.read_only)));}
        out.push('\n');
        if m.shared_fuse{out.push_str(&format!("mount-shared\t{}\n",m.guest));}
        if let Some(source)=&m.bind_source{out.push_str(&format!("bind-source\t{}\t{}\n",m.guest,source));}
    }
    out
}

/// Restore the mounts [`own_mounts_text`] described.
pub fn load_own_mounts(text: &str) {
    if text.lines().any(|line|line=="namespace\tprivate"){PRIVATE_MOUNTS.store(true,Ordering::Release);}
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if let ["propagation",root,kind]=f.as_slice(){let kind=match *kind{"slave"=>Propagation::Slave,"shared"=>Propagation::Shared,_=>Propagation::Private};PROPAGATION_RULES.lock().unwrap().push(((*root).into(),kind));continue;}
        if let ["mount-shared",guest]=f.as_slice(){let mut mounts=vfs().mounts.write().unwrap();if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==*guest&&mount.own){mount.shared_fuse=true;}continue;}
        if let ["bind-source",guest,source]=f.as_slice(){let mut mounts=vfs().mounts.write().unwrap();if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==*guest&&mount.own){mount.bind_source=Some((*source).into());}continue;}
        if f.len()==5||f.len()==8 {
            let(area,guest,host,source,fstype)=(f[0],f[1],f[2],f[3],f[4]);
            let area = if area == "ro" {
                Area::Image
            } else {
                Area::Writable
            };
            add_mount(guest, PathBuf::from(host), area, source, fstype);
            if f.len()==8{let mut mounts=vfs().mounts.write().unwrap();if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==guest&&mount.own){mount.fuse=Some(FuseRoute{session:PathBuf::from(f[5]),relative:f[6].into(),uid:f[7].split(',').next().and_then(|value|value.parse().ok()).unwrap_or(u32::MAX),gid:f[7].split(',').nth(1).and_then(|value|value.parse().ok()).unwrap_or(u32::MAX),allow_other:f[7].split(',').nth(2)==Some("1"),default_permissions:f[7].split(',').nth(3)==Some("1"),read_only:f[7].split(',').nth(4)==Some("1")});}}
        }
    }
}

/// One inverse namespace view for a generated procfs read. Host aliases are
/// resolved once per view; a later read observes mount and symlink changes.
pub(crate) struct HostPathView {
    mounts: Vec<(String, PathBuf)>,
    root: PathBuf,
    canonical_mounts: std::cell::OnceCell<Vec<(String, PathBuf)>>,
    canonical_root: std::cell::OnceCell<PathBuf>,
}

impl HostPathView {
    pub(crate) fn capture() -> Option<Self> {
        let v = VFS.get()?;
        let mounts = v.mounts.read().unwrap();
        Some(Self {
            mounts: mounts.iter().filter(|m| !hidden(&mounts, m))
                .map(|m| (m.guest.clone(), m.host.clone())).collect(),
            root: v.root.clone(),
            canonical_mounts: std::cell::OnceCell::new(),
            canonical_root: std::cell::OnceCell::new(),
        })
    }

    pub(crate) fn guest_path(&self, host: &Path) -> Option<String> {
        let s = host.to_str()?;
        if HOST_DEVICES.contains(&s) { return Some(s.to_owned()); }
        if let Some(guest) = crate::sys::tty::pts_guest(s) { return Some(guest); }
        let mut best: Option<(usize, usize, String)> = None;
        let canonical = self.canonical_mounts.get_or_init(|| self.mounts.iter()
            .filter_map(|(guest, path)| path.canonicalize().ok().map(|path| (guest.clone(), path)))
            .collect());
        for (guest, prefix) in self.mounts.iter().chain(canonical.iter()) {
            let Ok(rest) = host.strip_prefix(prefix) else { continue; };
            let len = prefix.as_os_str().len();
            if best.as_ref().is_none_or(|b| len > b.0 || (len == b.0 && guest.len() < b.1)) {
                let path = if rest.as_os_str().is_empty() { guest.clone() }
                    else { format!("{}/{}", guest, rest.display()) };
                best = Some((len, guest.len(), path));
            }
        }
        if let Some((_, _, guest)) = best { return Some(guest); }
        let root = self.canonical_root.get_or_init(|| self.root.canonicalize().unwrap_or_else(|_| self.root.clone()));
        let rel = host.strip_prefix(root).ok()?;
        Some(format!("/{}", rel.display()))
    }
}

/// Guest path of a host path, if it lies inside the root or a mapped area.
pub fn guest_path_of_host(host: &Path) -> Option<String> {
    HostPathView::capture()?.guest_path(host)
}

/// Guest path of an open directory fd.
fn guest_path_of_fd(fd: i32) -> Result<String, Errno> {
    if let Some(file)=crate::sys::fuse_client::get(fd){return Ok(file.guest.clone());}
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
    resolve_inner(dirfd, path, follow_last, false, |_| Ok(()))
}
/// Resolve from the actual cwd/dirfd and check each searched directory, including
/// directories visited before a symlink or dot-dot changes the resolved path.
pub fn resolve_checked(
    dirfd: i32,
    path: &[u8],
    follow_last: bool,
    check: impl FnMut(&str) -> Result<(), Errno>,
) -> Result<Resolved, Errno> {
    resolve_inner(dirfd, path, follow_last, true, check)
}
fn resolve_inner(
    dirfd: i32,
    path: &[u8],
    follow_last: bool,
    relative_base: bool,
    mut check: impl FnMut(&str) -> Result<(), Errno>,
) -> Result<Resolved, Errno> {
    refresh_fuse_mounts()?;
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
    let mut joined = base.as_bytes().to_vec();
    joined.push(b'/');
    joined.extend_from_slice(path);
    for c in joined.split(|&b| b == b'/').rev() {
        if !c.is_empty() {
            pending.push(c.to_vec());
        }
    }
    let mut done: Vec<Vec<u8>> = Vec::new();
    if relative_base && path[0] != b'/' {
        done = base.split('/').filter(|part| !part.is_empty()).map(|part| part.as_bytes().to_vec()).collect();
        pending = path.split(|&byte| byte == b'/').filter(|part| !part.is_empty()).rev().map(<[u8]>::to_vec).collect();
    }
    let mut links = 0;
    while let Some(c) = pending.pop() {
        check(&join_guest(&done))?;
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
        let current=join_guest(&done);
        if let Some(route)=fuse_route(&current){
            let node=match crate::sys::fuse_client::lookup(&route){Ok(node)=>node,Err(errno::ENOENT)if is_last=>continue,Err(error)=>return Err(error)};
            let _lookup = crate::sys::fuse_client::LookupGuard::transient(&route, node);
            let stat=crate::sys::fuse_client::stat(&route,Some(node),None)?;
            if stat.st_mode&libc::S_IFMT!=libc::S_IFLNK{continue;}
            links+=1;if links>MAX_SYMLINKS{return Err(errno::ELOOP);}done.pop();
            let target=crate::sys::fuse_client::readlink_node(&route,node)?;if target.first()==Some(&b'/'){done.clear();}
            for part in target.split(|byte|*byte==b'/').rev(){if !part.is_empty(){pending.push(part.to_vec());}}continue;
        }
        let (host, area) = lookup(&current);
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
    fn interrupted_mount_journal_lock_waits_for_its_owner() {
        use std::os::fd::AsRawFd;
        let (_view, dir) = test_view();
        let path = dir.join("interrupted-mount-lock");
        let owner = std::fs::OpenOptions::new().create(true).truncate(true)
            .read(true).write(true).open(&path).unwrap();
        let waiter = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
        assert_eq!(unsafe { libc::flock(owner.as_raw_fd(), libc::LOCK_EX) }, 0);
        let mut pipe = [0; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        static SIGNALS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        extern "C" fn caught(_: i32) { SIGNALS.fetch_add(1, Ordering::Relaxed); }
        let child = unsafe { libc::fork() };
        assert!(child >= 0);
        if child == 0 {
            // Only async-signal-safe libc calls after fork. The handler has no
            // SA_RESTART: the first contended flock must return EINTR.
            unsafe {
                libc::close(owner.as_raw_fd());
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = caught as *const () as usize;
                libc::sigemptyset(&mut action.sa_mask);
                if libc::sigaction(libc::SIGUSR1, &action, std::ptr::null_mut()) != 0 {
                    libc::_exit(2);
                }
                libc::write(pipe[1], b"r".as_ptr().cast(), 1);
                let first = libc::flock(waiter.as_raw_fd(), libc::LOCK_EX);
                if first != -1 || errno::last() != errno::EINTR { libc::_exit(3); }
                libc::write(pipe[1], b"i".as_ptr().cast(), 1);
                let result = lock_mount_table(waiter.as_raw_fd());
                libc::_exit(if result.is_ok() && SIGNALS.load(Ordering::Relaxed) == 2 { 0 } else { 4 });
            }
        }
        let mut ready = 0u8;
        assert_eq!(unsafe { libc::read(pipe[0], (&mut ready as *mut u8).cast(), 1) }, 1);
        // Wait until the child is blocked, then deliver an ordinary signal.
        unsafe { libc::usleep(20_000); }
        assert_eq!(unsafe { libc::kill(child, libc::SIGUSR1) }, 0);
        assert_eq!(unsafe { libc::read(pipe[0], (&mut ready as *mut u8).cast(), 1) }, 1);
        assert_eq!(ready, b'i');
        unsafe { libc::usleep(20_000); }
        assert_eq!(unsafe { libc::kill(child, libc::SIGUSR1) }, 0);
        unsafe { libc::usleep(20_000); }
        let mut status = 0;
        assert_eq!(unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) }, 0);
        assert_eq!(unsafe { libc::flock(owner.as_raw_fd(), libc::LOCK_UN) }, 0);
        assert_eq!(unsafe { libc::waitpid(child, &mut status, 0) }, child);
        assert!(libc::WIFEXITED(status));
        assert_eq!(libc::WEXITSTATUS(status), 0);
        unsafe { libc::close(pipe[0]); libc::close(pipe[1]); }
        std::fs::remove_file(path).unwrap();
        assert_eq!(lock_mount_table(-1), Err(errno::EBADF));
    }

    #[test]
    fn canonical_host_aliases_preserve_deepest_shortest_and_hidden_mounts() {
        let (_view, dir) = test_view(); let data = dir.join("data"); let nested = data.join("inverse-alias"); std::fs::create_dir_all(&nested).unwrap();
        let alias = dir.join("inverse-host-alias"); std::os::unix::fs::symlink(&data, &alias).unwrap();
        let canonical = nested.canonicalize().unwrap();
        add_mount("/inverse-parent", data.canonicalize().unwrap(), Area::Writable, "tmpfs", "tmpfs");
        add_mount("/inverse-short", alias.join("inverse-alias"), Area::Writable, "tmpfs", "tmpfs");
        add_mount("/inverse-long-name", alias.join("inverse-alias"), Area::Writable, "tmpfs", "tmpfs");
        assert_eq!(guest_path_of_host(&canonical.join("file")).as_deref(), Some("/inverse-short/file"));
        let read = HostPathView::capture().unwrap();
        assert_eq!(read.guest_path(&canonical.join("file")).as_deref(), Some("/inverse-short/file"));
        let hidden_host = dir.join("inverse-hidden"); std::fs::create_dir_all(&hidden_host).unwrap();
        add_mount("/inverse-short", hidden_host, Area::Writable, "tmpfs", "tmpfs");
        assert_eq!(guest_path_of_host(&canonical.join("file")).as_deref(), Some("/inverse-long-name/file"));
        assert_eq!(read.guest_path(&canonical.join("file")).as_deref(), Some("/inverse-short/file"));
        let next_read = HostPathView::capture().unwrap();
        assert_eq!(next_read.guest_path(&canonical.join("file")).as_deref(), Some("/inverse-long-name/file"));
        assert!(remove_mount("/inverse-short")); assert!(remove_mount("/inverse-short")); assert!(remove_mount("/inverse-long-name")); assert!(remove_mount("/inverse-parent"));
        std::fs::remove_file(alias).unwrap(); std::fs::remove_dir_all(nested).unwrap();
    }
    #[test]
    fn inverse_procfs_read_resolves_aliases_once_and_new_reads_see_retargeting() {
        let (_view, dir) = test_view();
        let before = dir.join("inverse-before");
        let after = dir.join("inverse-after");
        let alias = dir.join("inverse-changing-alias");
        std::fs::create_dir_all(&before).unwrap();
        std::fs::create_dir_all(&after).unwrap();
        std::os::unix::fs::symlink(&before, &alias).unwrap();
        add_mount("/inverse-changing", alias.clone(), Area::Writable, "tmpfs", "tmpfs");
        let read = HostPathView::capture().unwrap();
        let before_file = before.canonicalize().unwrap().join("file");
        assert_eq!(read.guest_path(&before_file).as_deref(), Some("/inverse-changing/file"));
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&after, &alias).unwrap();
        assert_eq!(read.guest_path(&before_file).as_deref(), Some("/inverse-changing/file"));
        assert_eq!(guest_path_of_host(&after.canonicalize().unwrap().join("file")).as_deref(), Some("/inverse-changing/file"));
        assert!(guest_path_of_host(&before_file).is_none());
        assert!(remove_mount("/inverse-changing"));
        std::fs::remove_file(alias).unwrap();
        std::fs::remove_dir(before).unwrap();
        std::fs::remove_dir(after).unwrap();
    }

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

#[cfg(test)]
mod fuse_route_tests {
    use super::*;
    #[test]
    fn init_path_map_alias_receives_late_mount_and_exec_import_does_not_duplicate_shared_entries(){
        let(_guard,root)=test_view();let old_rules=std::mem::take(&mut *PROPAGATION_RULES.lock().unwrap());let parent=root.join("init-map-parent");let lower=root.join("init-map-lower");for path in [&parent,&lower]{std::fs::create_dir_all(path).unwrap();}
        let text=format!("root\t/\t{}\nrw\t/storage/aim-init-alias\t{}\nbind-source\t/storage/aim-init-alias\t/mnt/aim-init-user\npropagation\t/storage/aim-init-alias\tslave\n",root.display(),parent.display());
        let(_,entries)=parse_map(&text).unwrap();assert_eq!(entries[0].bind_source.as_deref(),Some("/mnt/aim-init-user"));vfs().mounts.write().unwrap().extend(entries);
        publish_plain_mount("/mnt/aim-init-user/emulated",&lower,Area::Writable,"lower","bind",None).unwrap();refresh_fuse_mounts().unwrap();assert_eq!(lookup("/storage/aim-init-alias/emulated/0").0,lower.join("0"));
        let inherited=own_mounts_text();load_own_mounts(&inherited);refresh_fuse_mounts().unwrap();
        assert_eq!(vfs().mounts.read().unwrap().iter().filter(|mount|mount.guest=="/mnt/aim-init-user/emulated"&&mount.shared_fuse).count(),1);
        assert_eq!(lookup("/storage/aim-init-alias/emulated/0").0,lower.join("0"));
        publish_fuse_route("/mnt/aim-init-user/emulated",&lower,"",None).unwrap();refresh_fuse_mounts().unwrap();vfs().mounts.write().unwrap().retain(|mount|mount.guest!="/storage/aim-init-alias");*PROPAGATION_RULES.lock().unwrap()=old_rules;
    }
    #[test]
    fn late_plain_pass_through_bind_propagates_actual_lower_into_slave_alias(){
        let(_guard,root)=test_view();let old_private=PRIVATE_MOUNTS.swap(false,Ordering::AcqRel);let old_rules=std::mem::take(&mut *PROPAGATION_RULES.lock().unwrap());
        let parent=root.join("plain-parent");let lower=root.join("plain-lower");std::fs::create_dir_all(&parent).unwrap();std::fs::create_dir_all(lower.join("0/Android/data")).unwrap();
        add_mount("/mnt/aim-pass",parent.clone(),Area::Writable,"parent","bind");
        bind_mount_recursive("/mnt/aim-pass","/storage/aim-pass-slave",parent.clone(),Area::Writable,true).unwrap();
        bind_mount_recursive("/mnt/aim-pass","/storage/aim-pass-private",parent.clone(),Area::Writable,true).unwrap();
        set_mount_propagation("/storage/aim-pass-slave",(1<<19)|0x4000).unwrap();set_mount_propagation("/storage/aim-pass-private",(1<<18)|0x4000).unwrap();
        publish_plain_mount("/mnt/aim-pass/emulated",&lower,Area::Writable,"/data/media","bind",Some("/data/media")).unwrap();refresh_fuse_mounts().unwrap();
        assert_eq!(lookup("/storage/aim-pass-slave/emulated/0/Android/data").0,lower.join("0/Android/data"));
        assert!(fuse_route("/storage/aim-pass-slave/emulated/0/Android/data").is_none());
        assert_ne!(lookup("/storage/aim-pass-private/emulated/0/Android/data").0,lower.join("0/Android/data"));
        publish_fuse_route("/mnt/aim-pass/emulated",&lower,"",None).unwrap();refresh_fuse_mounts().unwrap();
        assert_ne!(lookup("/storage/aim-pass-slave/emulated/0/Android/data").0,lower.join("0/Android/data"));
        for path in ["/storage/aim-pass-private","/storage/aim-pass-slave","/mnt/aim-pass"]{assert!(remove_mount(path));}
        *PROPAGATION_RULES.lock().unwrap()=old_rules;PRIVATE_MOUNTS.store(old_private,Ordering::Release);
    }
    #[test]
    fn early_parent_alias_projects_late_fuse_event_into_slave_but_not_private_view(){
        let(_guard,root)=test_view();let old_private=PRIVATE_MOUNTS.swap(false,Ordering::AcqRel);let old_rules=std::mem::take(&mut *PROPAGATION_RULES.lock().unwrap());
        let parent=root.join("late-source");std::fs::create_dir_all(&parent).unwrap();
        add_mount("/mnt/aim-late-user",parent.clone(),Area::Writable,"user","bind");
        bind_mount_recursive("/mnt/aim-late-user","/mnt/aim-late-installer",parent.clone(),Area::Writable,true).unwrap();
        bind_mount_recursive("/mnt/aim-late-installer","/storage/aim-late-slave",parent.clone(),Area::Writable,true).unwrap();
        bind_mount_recursive("/mnt/aim-late-installer","/storage/aim-late-private",parent.clone(),Area::Writable,true).unwrap();
        set_mount_propagation("/storage/aim-late-slave",(1<<19)|0x4000).unwrap();set_mount_propagation("/storage/aim-late-private",(1<<18)|0x4000).unwrap();
        let route=FuseRoute{session:root.join("late-session.sock"),relative:String::new(),uid:0,gid:0,allow_other:true,default_permissions:false,read_only:false};
        publish_fuse_route("/mnt/aim-late-user/emulated",&parent,"fuse",Some(&route)).unwrap();refresh_fuse_mounts().unwrap();
        assert_eq!(fuse_route("/storage/aim-late-slave/emulated/0/Pictures").unwrap().relative,"0/Pictures");
        assert!(fuse_route("/storage/aim-late-private/emulated/0/Pictures").is_none());
        assert!(own_mounts_text().contains("bind-source\t/mnt/aim-late-installer\t/mnt/aim-late-user"));
        publish_fuse_route("/mnt/aim-late-user/emulated",&parent,"",None).unwrap();refresh_fuse_mounts().unwrap();
        assert!(fuse_route("/storage/aim-late-slave/emulated/0/Pictures").is_none());
        for path in ["/storage/aim-late-private","/storage/aim-late-slave","/mnt/aim-late-installer","/mnt/aim-late-user"]{assert!(remove_mount(path));}
        *PROPAGATION_RULES.lock().unwrap()=old_rules;PRIVATE_MOUNTS.store(old_private,Ordering::Release);
    }
    #[test]
    fn recursive_bind_clones_fuse_and_android_submounts_nonrecursive_excludes_them(){
        let(_guard,root)=test_view();
        let parent=root.join("recursive-source");let anchor=root.join("recursive-fuse-anchor");let android=root.join("recursive-android");
        for path in [&parent,&anchor,&android]{std::fs::create_dir_all(path).unwrap();}
        let options=crate::sys::fuse_mount::parse(b"fd=17,rootmode=40000,user_id=0,group_id=0,allow_other,").unwrap();
        add_mount("/mnt/aim-rec-source",parent.clone(),Area::Writable,"source","bind");
        let session=root.join("recursive-session.sock");
        add_fuse_mount("/mnt/aim-rec-source/emulated",session.clone(),anchor,"fuse",&options,false).unwrap();
        add_mount("/mnt/aim-rec-source/emulated/0/Android/data",android.clone(),Area::Writable,"android-data","bind");
        bind_mount_recursive("/mnt/aim-rec-source","/mnt/aim-nonrec",parent.clone(),Area::Writable,false).unwrap();
        assert!(fuse_route("/mnt/aim-nonrec/emulated/0/Documents").is_none());
        assert_ne!(lookup("/mnt/aim-nonrec/emulated/0/Android/data/pkg").0,android.join("pkg"));
        bind_mount_recursive("/mnt/aim-rec-source","/mnt/aim-rec-target",parent,Area::Writable,true).unwrap();
        let route=fuse_route("/mnt/aim-rec-target/emulated/0/Documents").unwrap();assert_eq!(route.session,session);assert_eq!(route.relative,"0/Documents");assert!(route.allow_other);
        assert_eq!(lookup("/mnt/aim-rec-target/emulated/0/Android/data/pkg").0,android.join("pkg"));
        assert!(fuse_route("/mnt/aim-rec-target/emulated/0/Android/data/pkg").is_none());
        for path in ["/mnt/aim-rec-target/emulated/0/Android/data","/mnt/aim-rec-target/emulated","/mnt/aim-rec-target","/mnt/aim-nonrec","/mnt/aim-rec-source/emulated/0/Android/data","/mnt/aim-rec-source/emulated","/mnt/aim-rec-source"]{assert!(remove_mount(path));}
    }
    #[test]
    fn slave_namespace_imports_future_parent_fuse_routes_without_publishing_child_mounts(){
        let(_guard,root)=test_view();let old_private=PRIVATE_MOUNTS.swap(false,Ordering::AcqRel);
        let old_rules=std::mem::take(&mut *PROPAGATION_RULES.lock().unwrap());
        let parent="/mnt/aim-slave-parent";let child="/mnt/aim-slave-child";
        let anchor=root.join("slave-anchor");std::fs::create_dir_all(&anchor).unwrap();
        let route=FuseRoute{session:root.join("slave-session.sock"),relative:String::new(),uid:0,gid:0,allow_other:true,default_permissions:false,read_only:false};
        private_mount_namespace().unwrap();set_mount_propagation("/",(1<<19)|0x4000).unwrap();
        // Parent publication is represented by its actual shared namespace table.
        PRIVATE_MOUNTS.store(false,Ordering::Release);PROPAGATION_RULES.lock().unwrap().clear();
        publish_fuse_route(parent,&anchor,"fuse",Some(&route)).unwrap();
        PRIVATE_MOUNTS.store(true,Ordering::Release);PROPAGATION_RULES.lock().unwrap().push(("/".into(),Propagation::Slave));
        refresh_fuse_mounts().unwrap();assert_eq!(fuse_route(parent).unwrap().session,route.session);
        bind_mount(parent,child,anchor.clone(),Area::Writable,"fuse","bind").unwrap();
        assert!(fuse_route(child).is_some());assert!(!fuse_table_lines().unwrap().iter().any(|line|line.split('\t').next()==Some(child)));
        PROPAGATION_RULES.lock().unwrap().clear();PRIVATE_MOUNTS.store(false,Ordering::Release);
        publish_fuse_route(parent,&anchor,"",None).unwrap();remove_mount(child);refresh_fuse_mounts().unwrap();
        *PROPAGATION_RULES.lock().unwrap()=old_rules;PRIVATE_MOUNTS.store(old_private,Ordering::Release);
    }
    #[test]
    fn fuse_bind_view_preserves_session_and_relative_tree_without_raw_path_resolution(){
        let(_guard,root)=test_view();let target="/mnt/aim-fuse-test";
        let options=crate::sys::fuse_mount::parse(b"fd=17,rootmode=40000,user_id=0,group_id=0,allow_other").unwrap();
        let anchor=root.join("fuse-anchor");std::fs::create_dir_all(&anchor).unwrap();
        let session=root.join("fuse-session.sock");
        add_fuse_mount(target,session.clone(),anchor.clone(),"fuse",&options,false).unwrap();
        bind_mount("/mnt/aim-fuse-test/user/0","/storage/aim-fuse-test",anchor,Area::Writable,"fuse","bind").unwrap();
        let route=fuse_route("/storage/aim-fuse-test/Documents/a.txt").unwrap();
        assert_eq!(route.session,session);assert_eq!(route.relative,"user/0/Documents/a.txt");assert!(route.allow_other);
        let text=own_mounts_text();assert!(text.contains("fuse-session.sock"));
        assert!(remove_mount("/storage/aim-fuse-test"));assert!(remove_mount(target));
        assert!(fuse_route("/storage/aim-fuse-test/Documents/a.txt").is_none());
    }
}
