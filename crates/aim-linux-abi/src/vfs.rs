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
pub struct MountOrigin{pub namespace:String,pub id:u64,pub parent_shared:u64,pub shared:u64,pub master:u64}
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
    origin:Option<MountOrigin>,
}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]enum Propagation{Private,Slave,Shared}
static PROPAGATION_RULES:Mutex<Vec<(String,Propagation)>>=Mutex::new(Vec::new());
fn mount_propagation(guest:&str)->Propagation{
    PROPAGATION_RULES.lock().unwrap().iter().filter(|(root,_)|root=="/"||below(root,guest).is_some()).max_by_key(|(root,_)|root.len()).map(|(_,kind)|*kind)
        .unwrap_or(if PRIVATE_MOUNTS.load(Ordering::Acquire){Propagation::Private}else{Propagation::Shared})
}
pub fn set_mount_propagation(guest:&str,flags:u64)->Result<(),Errno>{
    if !REPLAYING_NAMESPACE.with(std::cell::Cell::get)&&mount_namespace_id().is_some(){return namespace_event(&format!("propagation-flags\t{guest}\t{flags}"));}
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
fn published_mount(guest:&str,host:&Path)->Result<MountOrigin,Errno>{
    let owner=NAMESPACE_VIEW.lock().unwrap().as_ref().map(|view|view.owner.clone()).ok_or(errno::ENODEV)?;
    let state=owner.read().map_err(namespace_error)?;let record=state.mounts.iter().filter(|mount|mount.guest==guest&&Path::new(&mount.host)==host).max_by_key(|mount|mount.id).ok_or(errno::ENOENT)?;
    let parent=state.mounts.iter().find(|mount|mount.id==record.parent).ok_or(errno::EIO)?;
    Ok(MountOrigin{namespace:owner.id().into(),id:record.id,parent_shared:parent.shared,shared:record.shared,master:record.master})
}
fn journal_fields(line:&str)->Result<(Vec<&str>,Option<MountOrigin>),Errno>{
    let mut fields=line.split('\t').collect::<Vec<_>>();if !matches!(fields.len(),11|12){return Err(errno::EIO);}
    let mut number=||fields.pop().unwrap().parse::<u64>().map_err(|_|errno::EIO);
    let master=number()?;let shared=number()?;let parent_shared=number()?;let id=number()?;let namespace=fields.pop().unwrap();
    if id==0||[id,shared,master,parent_shared].iter().any(|id|*id>i32::MAX as u64){return Err(errno::EIO);}
    Ok((fields,Some(MountOrigin{namespace:namespace.into(),id,parent_shared,shared,master})))
}
fn origin_visible(id:&str,receiving:&[aim_storage::mount_namespace::MountRecord],guest:&str,origin:&Option<MountOrigin>)->Result<bool,Errno>{
    let Some(origin)=origin else{return Err(errno::EIO);};if origin.namespace==id{return Ok(true);}
    let target=receiving.iter().filter(|mount|mount.guest!=guest&&(mount.guest=="/"||below(&mount.guest,guest).is_some())).max_by_key(|mount|(mount.guest.len(),mount.id)).ok_or(errno::EIO)?;
    Ok(origin.parent_shared!=0&&(target.shared==origin.parent_shared||target.master==origin.parent_shared))
}
fn publish_fuse_route(guest:&str,host:&Path,source:&str,route:Option<&FuseRoute>)->Result<(),Errno>{
    if mount_propagation(guest)!=Propagation::Shared{return Ok(());}
    let _lock=fuse_table_lock()?;let mut lines=fuse_table_lines()?;
    lines.retain(|line|line.split('\t').next()!=Some(guest));
    if let Some(route)=route{
        let origin=published_mount(guest,host)?;let fields=[guest.to_owned(),host.display().to_string(),source.to_owned(),route.session.display().to_string(),route.relative.clone(),format!("{},{},{},{},{}",route.uid,route.gid,u8::from(route.allow_other),u8::from(route.default_permissions),u8::from(route.read_only)),origin.namespace,origin.id.to_string(),origin.parent_shared.to_string(),origin.shared.to_string(),origin.master.to_string()];
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
    let origin=published_mount(guest,host)?;
    let fields=[guest.to_owned(),host.display().to_string(),source.to_owned(),"plain".into(),match area{Area::Image=>"ro",Area::Writable=>"rw",Area::Kernfs=>"kernfs",Area::HostDevice=>"host-device",Area::Input=>"input"}.into(),fstype.to_owned(),bind_source.unwrap_or("").to_owned(),origin.namespace,origin.id.to_string(),origin.parent_shared.to_string(),origin.shared.to_string(),origin.master.to_string()];
    if fields.iter().any(|field|field.contains(['\t','\n'])){return Err(errno::EINVAL);}
    let _lock=fuse_table_lock()?;let mut lines=fuse_table_lines()?;lines.retain(|line|line.split('\t').next()!=Some(guest));lines.push(fields.join("\t"));
    let path=fuse_table().ok_or(errno::ENODEV)?;let temporary=path.with_extension(format!("{}.new",std::process::id()));use std::io::Write as _;
    let mut output=std::fs::OpenOptions::new().create(true).truncate(true).write(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).mode(0o600).open(&temporary).map_err(|_|errno::EIO)?;
    output.write_all(lines.join("\n").as_bytes()).map_err(|_|errno::EIO)?;output.sync_all().map_err(|_|errno::EIO)?;std::fs::rename(&temporary,&path).map_err(|_|errno::EIO)?;
    std::fs::File::open(path.parent().ok_or(errno::EIO)?).and_then(|directory|directory.sync_all()).map_err(|_|errno::EIO)
}
pub fn refresh_fuse_mounts()->Result<(),Errno>{
    refresh_namespace()?;
    if fuse_table().is_none(){return Ok(());}
    let namespace=NAMESPACE_VIEW.lock().unwrap().as_ref().map(|view|view.owner.clone()).ok_or(errno::ENODEV)?;
    let receiving=NAMESPACE_VIEW.lock().unwrap().as_ref().map(|view|view.records.clone()).ok_or(errno::ENODEV)?;
    let _lock=fuse_table_lock()?;let lines=fuse_table_lines()?;let mut mounts=vfs().mounts.write().unwrap();
    let old=mounts.iter().filter(|mount|mount.shared_fuse).map(|mount|(mount.guest.clone(),mount.seq)).collect::<HashMap<_,_>>();
    mounts.retain(|mount|(!mount.shared_fuse&&!mount.projected_fuse)||mount_propagation(&mount.guest)==Propagation::Private);
    let mut seq=mounts.iter().map(|mount|mount.seq).max().unwrap_or(0);
    for line in lines{
        let(fields,origin)=journal_fields(&line)?;if !fields[0].starts_with('/'){return Err(errno::EIO);}
        if mount_propagation(fields[0])==Propagation::Private||!origin_visible(namespace.id(),receiving.as_slice(),fields[0],&origin)?{continue;}
        let mount_seq=match old.get(fields[0]){Some(seq)=>*seq,None=>{seq+=1;seq}};
        if fields.len()==7{
            if fields[3]!="plain"{return Err(errno::EIO);}
            mounts.push(Mount{guest:fields[0].into(),host:PathBuf::from(fields[1]),area:match fields[4]{"ro"=>Area::Image,"rw"=>Area::Writable,"kernfs"=>Area::Kernfs,"host-device"=>Area::HostDevice,"input"=>Area::Input,_=>return Err(errno::EIO)},source:Some(fields[2].into()),fstype:Some(fields[5].into()),own:true,seq:mount_seq,fuse:None,shared_fuse:true,bind_source:(!fields[6].is_empty()).then(||fields[6].into()),projected_fuse:false,origin:origin.clone()});
        }else{
            let policy=fields[5].split(',').map(|value|value.parse::<u32>().map_err(|_|errno::EIO)).collect::<Result<Vec<_>,_>>()?;if policy.len()!=5{return Err(errno::EIO);}
            mounts.push(Mount{guest:fields[0].into(),host:PathBuf::from(fields[1]),area:Area::Writable,source:Some(fields[2].into()),fstype:Some("fuse".into()),own:true,seq:mount_seq,fuse:Some(FuseRoute{session:PathBuf::from(fields[3]),relative:fields[4].into(),uid:policy[0],gid:policy[1],allow_other:policy[2]!=0,default_permissions:policy[3]!=0,read_only:policy[4]!=0}),shared_fuse:true,bind_source:None,projected_fuse:false,origin:origin.clone()});
        }
    }
    project_mounts(&mut mounts,mount_propagation);
    mounts.sort_by(|a,b|b.guest.len().cmp(&a.guest.len()).then(b.seq.cmp(&a.seq)));Ok(())
}
fn project_mounts(mounts:&mut Vec<Mount>,propagation:impl Fn(&str)->Propagation){
    let mut seq=mounts.iter().map(|mount|mount.seq).max().unwrap_or(0);
    // Propagated parent events retain the bind mount's actual source subtree.
    // Iterate to cover installer -> storage aliases; no host path inference.
    for _ in 0..16{
        let aliases=mounts.iter().filter(|mount|mount.bind_source.is_some()&&!hidden(&mounts,mount)).cloned().collect::<Vec<_>>();
        let routes=mounts.iter().filter(|mount|mount.own&&!hidden(&mounts,mount)).cloned().collect::<Vec<_>>();let mut added=false;
        for alias in aliases{let source=alias.bind_source.as_ref().unwrap();if propagation(&alias.guest)==Propagation::Private{continue;}
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
}
pub fn private_mount_namespace()->Result<(),Errno>{
    refresh_fuse_mounts()?;
    // CLONE_NEWNS copies propagation relationships; MS_PRIVATE/MS_SLAVE
    // subsequently changes them at the guest's explicit mount operation.
    if PROPAGATION_RULES.lock().unwrap().is_empty(){PROPAGATION_RULES.lock().unwrap().push(("/".into(),Propagation::Shared));}
    let inherited=NAMESPACE_VIEW.lock().unwrap().as_ref().map(|view|view.owner.clone());
    if let Some(owner)=inherited {
        let id=format!("ns-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_|errno::EIO)?.as_nanos());
        let owner=owner.clone_to(&id).map_err(namespace_error)?;
        let mut inheritance="namespace\tprivate\n".to_owned();
        for(root,kind)in PROPAGATION_RULES.lock().unwrap().iter(){inheritance.push_str(&format!("propagation\t{root}\t{}\n",match kind{Propagation::Private=>"private",Propagation::Slave=>"slave",Propagation::Shared=>"shared"}));}
        let state=owner.append(&inheritance).map_err(namespace_error)?;
        let generation=state.generation;
        *NAMESPACE_VIEW.lock().unwrap()=Some(NamespaceView{page:owner.generation().map_err(namespace_error)?,owner,generation,records:std::sync::Arc::new(state.mounts.clone())});
        publish_process_namespace()?;
    }
    PRIVATE_MOUNTS.store(true,Ordering::Release);Ok(())
}

struct NamespaceView {owner:aim_storage::mount_namespace::Namespace,generation:u64,page:aim_storage::mount_namespace::Generation,records:std::sync::Arc<Vec<aim_storage::mount_namespace::MountRecord>>}
static NAMESPACE_VIEW:Mutex<Option<NamespaceView>>=Mutex::new(None);
static NAMESPACE_REFRESH:Mutex<()>=Mutex::new(());
thread_local! {static REPLAYING_NAMESPACE:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};}
fn namespace_error(error:std::io::Error)->Errno{errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))}
fn namespace_event(event:&str)->Result<(),Errno>{
    if REPLAYING_NAMESPACE.with(std::cell::Cell::get){return Ok(());}
    let owner=NAMESPACE_VIEW.lock().unwrap().as_ref().map(|view|view.owner.clone());
    if let Some(owner)=owner{owner.append(event).map_err(namespace_error)?;refresh_namespace()?;}Ok(())
}
fn apply_namespace_state(state:aim_storage::mount_namespace::State)->Result<(),Errno>{
    struct Replay;impl Drop for Replay{fn drop(&mut self){REPLAYING_NAMESPACE.with(|value|value.set(false));}}
    REPLAYING_NAMESPACE.with(|value|value.set(true));let _replay=Replay;
    PROPAGATION_RULES.lock().unwrap().clear();
    PRIVATE_MOUNTS.store(false,Ordering::Release);
    let(_,base)=parse_map(&state.base).map_err(|_|errno::EIO)?;
    *PROPAGATION_RULES.lock().unwrap()=map_propagation(&state.base)?;
    *vfs().mounts.write().unwrap()=base;
    for event in state.events {
        if let Some(guest)=event.strip_prefix("remove\t") { remove_mount(guest)?; }
        else if let Some(rest)=event.strip_prefix("propagation-flags\t") {
            let (guest,flags)=rest.rsplit_once('\t').ok_or(errno::EIO)?;set_mount_propagation(guest,flags.parse().map_err(|_|errno::EIO)?)?;
        } else if let Some(rest)=event.strip_prefix("move\t") {
            let(from,to)=rest.split_once('\t').ok_or(errno::EIO)?;move_mount(from,to)?;
        } else if let Some(rest)=event.strip_prefix("policy\t") {
            let fields=rest.split('\t').collect::<Vec<_>>();if fields.len()!=7{return Err(errno::EINVAL);}
            let mut mounts=vfs().mounts.write().unwrap();let mount=mounts.iter_mut().find(|mount|mount.guest==fields[0]&&mount.own).ok_or(errno::ENOENT)?;
            mount.bind_source=(!fields[1].is_empty()).then(||fields[1].into());
            if !fields[2].is_empty(){let policy=fields[4].split(',').map(str::parse::<u32>).collect::<Result<Vec<_>,_>>().map_err(|_|errno::EINVAL)?;if policy.len()!=5{return Err(errno::EINVAL);}mount.fuse=Some(FuseRoute{session:fields[2].into(),relative:fields[3].into(),uid:policy[0],gid:policy[1],allow_other:policy[2]!=0,default_permissions:policy[3]!=0,read_only:policy[4]!=0});}
            mount.shared_fuse=fields[5]=="1";mount.projected_fuse=fields[6]=="1";
        } else { load_own_mounts(&event)?; }
    }
    Ok(())
}
fn publish_mount_policy(guest:&str)->Result<(),Errno>{
    if REPLAYING_NAMESPACE.with(std::cell::Cell::get)||mount_namespace_id().is_none(){return Ok(());}
    let mount=vfs().mounts.read().unwrap().iter().find(|mount|mount.guest==guest&&mount.own).cloned().ok_or(errno::ENOENT)?;
    let(route,relative,policy)=mount.fuse.as_ref().map(|route|(route.session.display().to_string(),route.relative.clone(),format!("{},{},{},{},{}",route.uid,route.gid,u8::from(route.allow_other),u8::from(route.default_permissions),u8::from(route.read_only)))).unwrap_or_default();
    namespace_event(&format!("policy\t{}\t{}\t{}\t{}\t{}\t{}\t{}",mount.guest,mount.bind_source.as_deref().unwrap_or(""),route,relative,policy,u8::from(mount.shared_fuse),u8::from(mount.projected_fuse)))
}

fn refresh_namespace()->Result<(),Errno>{
    if REPLAYING_NAMESPACE.with(std::cell::Cell::get){return Ok(());}
    let _refresh=NAMESPACE_REFRESH.lock().unwrap();
    let owner=NAMESPACE_VIEW.lock().unwrap().as_ref().and_then(|view|(view.page.current()!=view.generation).then(||(view.owner.clone(),view.generation)));
    if let Some((owner,generation))=owner {
        let state=owner.read().map_err(namespace_error)?;
        if state.generation!=generation {let next=state.generation;let records=std::sync::Arc::new(state.mounts.clone());apply_namespace_state(state)?;if let Some(view)=NAMESPACE_VIEW.lock().unwrap().as_mut(){if view.owner.id()==owner.id(){view.generation=next;view.records=records;}}}
    }Ok(())
}
pub fn mount_namespace_id()->Option<String>{NAMESPACE_VIEW.lock().unwrap().as_ref().map(|view|view.owner.id().to_owned())}

pub(crate) fn publish_process_namespace()->Result<(),Errno>{
    if let(Some(id),Some(table))=(mount_namespace_id(),crate::sys::cred::by_pid_dir()){
        let process=aim_storage::process_namespace::ProcessIdentity::running(unsafe{libc::getpid()}).map_err(namespace_error)?;
        aim_storage::process_namespace::register_mount_namespace(table,process,&id).map_err(namespace_error)?;
    }Ok(())
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
    let route=FuseRoute{session,relative:String::new(),uid:options.uid,gid:options.gid,allow_other:options.allow_other,default_permissions:options.default_permissions,read_only};
    install_mount(guest,host_anchor,Area::Writable,source,"fuse",Some(route),None)
}
pub fn fuse_route_for_session(session:&Path)->Option<(String,FuseRoute)>{
    let mounts=vfs().mounts.read().unwrap();mounts.iter().filter(|mount|!hidden(&mounts,mount))
        .find_map(|mount|mount.fuse.as_ref().filter(|route|route.session==session).map(|route|(mount.guest.clone(),route.clone())))
}
/// A bind aliases the same FUSE session and mount-relative node tree.
pub fn bind_mount(source_guest:&str,target_guest:&str,host:PathBuf,area:Area,source:&str,fstype:&str)->Result<(),Errno>{
    refresh_fuse_mounts()?;
    let mut route=fuse_route(source_guest);
    if area==Area::Image{if let Some(route)=&mut route{route.read_only=true;}}
    install_mount(target_guest,host,area,source,fstype,route,Some(source_guest.into()))?;
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
        install_mount(&guest,child.host,child_area,child.source.as_deref().unwrap_or("bind"),child.fstype.as_deref().unwrap_or("bind"),route,child.bind_source)?;

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
/// `ro`, `rw` and `kernfs`; `#` starts a comment line.
fn map_propagation(text:&str)->Result<Vec<(String,Propagation)>,Errno>{
    text.lines().filter_map(|line|line.strip_prefix("propagation\t")).map(|line|{
        let(guest,kind)=line.split_once('\t').ok_or(errno::EINVAL)?;
        let kind=match kind{"private"=>Propagation::Private,"slave"=>Propagation::Slave,"shared"=>Propagation::Shared,_=>return Err(errno::EINVAL)};
        Ok((guest.into(),kind))
    }).collect()
}

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
        if let ["propagation",_guest,kind]=f.as_slice(){
            let propagation=match *kind{"slave"=>Propagation::Slave,"private"=>Propagation::Private,"shared"=>Propagation::Shared,_=>return Err(format!("line {}: invalid propagation",n+1))};
            let _ = propagation;
            continue;
        }
        let [kind, guest, host] = f[..] else {
            return Err(format!("line {}: expected 3 tab-separated fields", n + 1));
        };
        let area = match kind {
            "root" => {
                root = Some(PathBuf::from(host));
                continue;
            }
            "ro" => Area::Image,
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
            origin:None,
        });
    }
    mounts.sort_by(|a, b| b.guest.len().cmp(&a.guest.len()));
    Ok((root, mounts))
}

/// Native fork startup precedes VM restore. Inherit only the actual XNU
/// parent's authenticated current namespace, never the init origin fallback.
pub(crate) fn inherit_fork_namespace(map:Option<&Path>)->Result<(),String>{
    let Some(runtime)=map.and_then(Path::parent)else{return Ok(());};
    let table=runtime.join("identity/by-pid");
    match std::fs::symlink_metadata(table.join("namespace-init")){
        Err(error)if error.kind()==std::io::ErrorKind::NotFound=>return Ok(()),
        Err(error)=>return Err(error.to_string()),Ok(_)=>{},
    }
    aim_storage::process_namespace::InitRegistration::read(&table).map_err(|error|error.to_string())?;
    let child=aim_storage::process_namespace::ProcessIdentity::running(unsafe{libc::getpid()}).map_err(|error|error.to_string())?;
    match std::fs::symlink_metadata(table.join(format!("{}.mount-namespace",child.host_pid))){
        Ok(_)=>return aim_storage::process_namespace::mount_namespace_of(&table,child).map(|_|()).map_err(|error|error.to_string()),
        Err(error)if error.kind()==std::io::ErrorKind::NotFound=>{},Err(error)=>return Err(error.to_string()),
    }
    let parent=aim_storage::process_namespace::ProcessIdentity::running(unsafe{libc::getppid()}).map_err(|error|error.to_string())?;
    let namespace=aim_storage::process_namespace::mount_namespace_of(&table,parent).map_err(|error|error.to_string())?;
    aim_storage::process_namespace::register_mount_namespace(&table,child,&namespace).map_err(|error|error.to_string())
}

/// Initialize from `--root` and, when given, a `--path-map` file (whose
/// `root` line overrides `root`).
pub fn init(root: &Path, map: Option<&Path>) -> Result<(), String> {
    let (map_root, mut mounts, mut runtime) = match map {
        Some(map) => {
            let text =
                std::fs::read_to_string(map).map_err(|e| format!("{}: {e}", map.display()))?;
            let (r, m) = parse_map(&text).map_err(|e| format!("{}: {e}", map.display()))?;
            *PROPAGATION_RULES.lock().unwrap()=map_propagation(&text).map_err(|error|format!("mount propagation: errno {error}"))?;
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
            origin:None,
        });
        mounts.sort_by(|a, b| b.guest.len().cmp(&a.guest.len()));
    }
    let root = map_root.as_deref().unwrap_or(root);
    let root = root
        .canonicalize()
        .map_err(|e| format!("--root {}: {e}", root.display()))?;
    if runtime.is_none(){
        let process=aim_storage::process_namespace::ProcessIdentity::running(unsafe{libc::getpid()}).map_err(|error|error.to_string())?;
        let directory=std::env::temp_dir().join(format!("aim-mount-{}-{}-{}",process.host_pid,process.start_seconds,process.start_microseconds));
        std::fs::create_dir_all(&directory).map_err(|error|error.to_string())?;
        runtime=Some(directory);
    }
    let read_only_dev = read_only_dev(&root);
    let _ = VFS.set(Vfs {
        root,
        mapped: !mounts.is_empty(),
        read_only_dev,
        mounts: RwLock::new(mounts),
        runtime,
        cwd: Mutex::new("/".into()),
    });
    if let Some(runtime)=runtime_dir() {
        let table=crate::sys::cred::by_pid_dir().cloned().unwrap_or_else(||runtime.join("identity/by-pid"));
        if let Ok(_init)=aim_storage::process_namespace::InitRegistration::read(&table){
            let process=aim_storage::process_namespace::ProcessIdentity::running(unsafe{libc::getpid()}).map_err(|error|error.to_string())?;
            let id=aim_storage::process_namespace::mount_namespace_of(&table,process).map_err(|error|format!("current process mount owner: {error}"))?;
            let owner=aim_storage::mount_namespace::Namespace::open(runtime,&id).map_err(|error|error.to_string())?;
            *NAMESPACE_VIEW.lock().unwrap()=Some(NamespaceView{page:owner.generation().map_err(|error|error.to_string())?,owner,generation:0,records:Default::default()});
            refresh_namespace().map_err(|error|format!("mount namespace: errno {error}"))?;
        } else if !matches!(std::fs::symlink_metadata(table.join("namespace-init")),Err(error)if error.kind()==std::io::ErrorKind::NotFound){return Err("invalid native init registration".into());}
        else{
            let process=aim_storage::process_namespace::ProcessIdentity::running(unsafe{libc::getpid()}).map_err(|error|error.to_string())?;
            let id=format!("standalone-{}-{}-{}",process.host_pid,process.start_seconds,process.start_microseconds);
            let owner=aim_storage::mount_namespace::Namespace::open(runtime,&id).map_err(|error|error.to_string())?;
            let mut base=match map{Some(path)=>std::fs::read_to_string(path).map_err(|error|error.to_string())?,None=>String::new()};
            if !base.lines().any(|line|line.starts_with("root\t")){base=format!("root\t/\t{}\n{base}",vfs().root.display());}
            if !base.lines().any(|line|line.starts_with("propagation\t/\t")){base.push_str("propagation\t/\tshared\n");}
            owner.initialize(&base).map_err(|error|error.to_string())?;
            *NAMESPACE_VIEW.lock().unwrap()=Some(NamespaceView{page:owner.generation().map_err(|error|error.to_string())?,owner,generation:0,records:Default::default()});
            refresh_namespace().map_err(|error|format!("standalone mount namespace: errno {error}"))?;
        }
    }
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
    pub id:u64,
    pub parent:u64,
    pub shared:u64,
    pub master:u64,
    pub root:String,
    pub host:PathBuf,
    pub origin:Option<MountOrigin>,
    pub guest: String,
    pub area: Area,
    /// None for a plain directory of the path map.
    pub source: Option<String>,
    pub fstype: Option<String>,
}

/// The path map's directory entries (its "mounts"), shortest guest path
/// first (single-file entries are left out), then this process's own
/// mounts in mount order.
fn record_points(records:&[aim_storage::mount_namespace::MountRecord])->Vec<MountPoint>{
    records.iter().map(|record|MountPoint{id:record.id,parent:record.parent,shared:record.shared,master:record.master,root:record.root.clone(),host:PathBuf::from(&record.host),origin:None,guest:record.guest.clone(),area:match record.kind.as_str(){"root"|"ro"=>Area::Image,"kernfs"=>Area::Kernfs,"input"=>Area::Input,"host-device"=>Area::HostDevice,_=>Area::Writable},source:if record.kind=="root"{Some("/dev/root".into())}else if record.source.is_empty(){None}else{Some(record.source.clone())},fstype:if record.kind=="root"{Some("erofs".into())}else if matches!(record.kind.as_str(),"bpf"|"cgroup2"){Some(record.kind.clone())}else if record.fstype.is_empty(){None}else{Some(record.fstype.clone())}}).collect()
}
pub fn mount_points() -> Result<Vec<MountPoint>,Errno> {let id=mount_namespace_id().ok_or(errno::ENODEV)?;namespace_mount_points(&id)}

/// Read another authenticated process's actual mount object inventory.
pub fn namespace_mount_points(id:&str)->Result<Vec<MountPoint>,Errno>{
    let runtime=NAMESPACE_VIEW.lock().unwrap().as_ref().map(|view|view.owner.runtime().to_path_buf()).or_else(||runtime_dir().map(Path::to_path_buf)).ok_or(errno::ENODEV)?;
    let owner=aim_storage::mount_namespace::Namespace::open(&runtime,id).map_err(namespace_error)?;
    let backend=namespace_backend_points(id)?;
    let receiving=owner.read().map_err(namespace_error)?;
    let mut projected=Vec::new();
    for point in backend{
        let(mut origin,mut shared,mut master)=(0,0,0);
        if let Some(source)=&point.origin{
            let receiving_parent=receiving.mounts.iter().filter(|candidate|candidate.guest!=point.guest&&(candidate.guest=="/"||below(&candidate.guest,&point.guest).is_some())).max_by_key(|candidate|(candidate.guest.len(),candidate.id)).ok_or(errno::EIO)?;
            origin=source.id;
            if source.namespace==id||receiving_parent.shared!=0&&receiving_parent.shared==source.parent_shared{shared=source.shared;master=source.master;}
            else if source.parent_shared!=0&&receiving_parent.master==source.parent_shared{master=source.shared;if receiving_parent.shared!=0{shared=owner.propagated_group(source.id,receiving_parent.shared).map_err(namespace_error)?;}}
            else{continue;}
        }
        projected.push(aim_storage::mount_namespace::MountRecord{id:0,origin,parent:0,shared,master,kind:"projected".into(),guest:point.guest.clone(),host:point.host.display().to_string(),source:point.source.clone().unwrap_or_default(),fstype:point.fstype.clone().unwrap_or_default(),root:point.root.clone(),own:true});
    }
    let state=owner.sync_projections(&projected).map_err(namespace_error)?;
    Ok(record_points(&state.mounts))
}
fn namespace_backend_points(id:&str)->Result<Vec<MountPoint>,Errno>{
    let runtime=NAMESPACE_VIEW.lock().unwrap().as_ref().map(|view|view.owner.runtime().to_path_buf()).or_else(||runtime_dir().map(Path::to_path_buf)).ok_or(errno::ENODEV)?;
    let owner=aim_storage::mount_namespace::Namespace::open(&runtime,id).map_err(namespace_error)?;
    let state=owner.read().map_err(namespace_error)?;
    let receiving=state.clone();
    let(_,mut mounts)=parse_map(&state.base).map_err(|_|errno::EIO)?;
    let mut rules=map_propagation(&state.base)?;
    let mut private=false;
    for event in state.events {
        if let Some(guest)=event.strip_prefix("remove\t"){
            if let Some(index)=mounts.iter().position(|mount|mount.own&&mount.guest==guest){mounts.remove(index);}
        }else if let Some(rest)=event.strip_prefix("move\t"){
            let(from,to)=rest.split_once('\t').ok_or(errno::EINVAL)?;
            for mount in mounts.iter_mut().filter(|mount|mount.own&&(mount.guest==from||below(from,&mount.guest).is_some())){mount.guest=format!("{to}{}",&mount.guest[from.len()..]);}
        }else if let Some(rest)=event.strip_prefix("propagation-flags\t"){
            let(guest,flags)=rest.rsplit_once('\t').ok_or(errno::EINVAL)?;
            let flags=flags.parse::<u64>().map_err(|_|errno::EINVAL)?;
            let kind=if flags&(1<<19)!=0{Propagation::Slave}else if flags&(1<<20)!=0{Propagation::Shared}else{Propagation::Private};
            rules.retain(|(root,_)|root!=guest&&(flags&0x4000==0||guest!="/"&&below(guest,root).is_none()));rules.push((guest.into(),kind));
        }else if event.starts_with("policy\t"){continue;}
        else {
            for line in event.lines(){
                if line=="namespace\tprivate"{private=true;continue;}
                if let Some(rest)=line.strip_prefix("propagation\t"){let(guest,kind)=rest.split_once('\t').ok_or(errno::EINVAL)?;rules.push((guest.into(),match kind{"private"=>Propagation::Private,"slave"=>Propagation::Slave,"shared"=>Propagation::Shared,_=>return Err(errno::EINVAL)}));continue;}
                if let Some(guest)=line.strip_prefix("mount-shared\t"){if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==guest&&mount.own){mount.shared_fuse=true;}continue;}
                let fields=line.split('\t').collect::<Vec<_>>();if fields.len()==3&&fields[0]=="bind-source"{if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==fields[1]&&mount.own){mount.bind_source=Some(fields[2].into());}continue;}if !matches!(fields.len(),5|8){return Err(errno::EINVAL);}
                let area=match fields[0]{"ro"=>Area::Image,"rw"=>Area::Writable,"kernfs"=>Area::Kernfs,"host-device"=>Area::HostDevice,"input"=>Area::Input,_=>return Err(errno::EINVAL)};
                let seq=mounts.iter().map(|mount|mount.seq).max().unwrap_or(0)+1;
                let mount=Mount{guest:fields[1].into(),host:fields[2].into(),area,source:Some(fields[3].into()),fstype:Some(fields[4].into()),own:true,seq,fuse:None,shared_fuse:false,bind_source:None,projected_fuse:false,origin:None};
                let at=mounts.iter().position(|known|known.guest.len()<=mount.guest.len()).unwrap_or(mounts.len());mounts.insert(at,mount);
            }
        }
    }
    let propagation=|guest:&str|rules.iter().filter(|(root,_)|root=="/"||below(root,guest).is_some()).max_by_key(|(root,_)|root.len()).map(|(_,kind)|*kind).unwrap_or(if private{Propagation::Private}else{Propagation::Shared});
    mounts.retain(|mount|!mount.shared_fuse||propagation(&mount.guest)==Propagation::Private);
    for line in fuse_table_lines()?{
        let(fields,origin)=journal_fields(&line)?;
        if propagation(fields[0])==Propagation::Private||!origin_visible(id,&receiving.mounts,fields[0],&origin)?{continue;}
        let(area,fstype)=if fields.len()==7{(match fields[4]{"ro"=>Area::Image,"rw"=>Area::Writable,"kernfs"=>Area::Kernfs,"host-device"=>Area::HostDevice,"input"=>Area::Input,_=>return Err(errno::EIO)},fields[5])}else{(Area::Writable,"fuse")};
        let seq=mounts.iter().map(|mount|mount.seq).max().unwrap_or(0)+1;
        mounts.push(Mount{guest:fields[0].into(),host:fields[1].into(),area,source:Some(fields[2].into()),fstype:Some(fstype.into()),own:true,seq,fuse:None,shared_fuse:true,bind_source:if fields.len()==7&&!fields[6].is_empty(){Some(fields[6].into())}else{None},projected_fuse:false,origin:origin.clone()});
    }
    project_mounts(&mut mounts,propagation);
    mounts.sort_by(|a,b|b.guest.len().cmp(&a.guest.len()).then(b.seq.cmp(&a.seq)));
    let(mut map,mut own):(Vec<_>,Vec<_>)=mounts.iter().filter(|mount|mount.own||mount.host.is_dir()).partition(|mount|!mount.own);map.reverse();own.reverse();
    Ok(map.into_iter().chain(own).map(|mount|MountPoint{id:0,parent:0,shared:0,master:0,root:"/".into(),host:mount.host.clone(),origin:mount.origin.clone(),guest:mount.guest.clone(),area:mount.area,source:mount.source.clone(),fstype:mount.fstype.clone()}).collect())
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
pub fn add_mount(guest: &str, host: PathBuf, area: Area, source: &str, fstype: &str) -> Result<(),Errno> {
    install_mount(guest,host,area,source,fstype,None,None)
}
fn install_mount(guest:&str,host:PathBuf,area:Area,source:&str,fstype:&str,route:Option<FuseRoute>,bind_source:Option<String>)->Result<(),Errno>{
    if !REPLAYING_NAMESPACE.with(std::cell::Cell::get)&&mount_namespace_id().is_some(){
        let kind=match area{Area::Image=>"ro",Area::Writable=>"rw",Area::Kernfs=>"kernfs",Area::HostDevice=>"host-device",Area::Input=>"input"};
        let shared=mount_propagation(guest)==Propagation::Shared;
        let mut event=format!("{kind}\t{guest}\t{}\t{source}\t{fstype}",host.display());
        if let Some(route)=&route{event.push_str(&format!("\t{}\t{}\t{},{},{},{},{}",route.session.display(),route.relative,route.uid,route.gid,u8::from(route.allow_other),u8::from(route.default_permissions),u8::from(route.read_only)));}
        event.push('\n');if shared{event.push_str(&format!("mount-shared\t{guest}\n"));}
        if let Some(source)=&bind_source{event.push_str(&format!("bind-source\t{guest}\t{source}\n"));}
        namespace_event(&event)?;
        if let Some(route)=&route{publish_fuse_route(guest,&host,source,Some(route))?;}else{publish_plain_mount(guest,&host,area,source,fstype,bind_source.as_deref())?;}
        return Ok(());
    }
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
            fuse: route.clone(),
            shared_fuse: (route.is_some()||bind_source.is_some())&&mount_propagation(guest)==Propagation::Shared,
            bind_source: bind_source.clone(),
            projected_fuse: false,
            origin:None,
        },
    );
    drop(mounts);
    if !REPLAYING_NAMESPACE.with(std::cell::Cell::get){
        if let Some(route)=&route{let mount=vfs().mounts.read().unwrap().iter().find(|mount|mount.guest==guest&&mount.own).cloned().ok_or(errno::ENOENT)?;publish_fuse_route(guest,&mount.host,source,Some(route))?;}
        else if bind_source.is_some(){let mount=vfs().mounts.read().unwrap().iter().find(|mount|mount.guest==guest&&mount.own).cloned().ok_or(errno::ENOENT)?;publish_plain_mount(guest,&mount.host,area,source,fstype,bind_source.as_deref())?;}
    }
    Ok(())
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
    let mut mounts=vfs().mounts.write().unwrap();if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==guest&&mount.seq==seq){mount.fuse=Some(route);}drop(mounts);publish_mount_policy(guest)?;Ok(true)
}

/// Remove the latest mount this process made at `guest`.
pub fn remove_mount(guest: &str) -> Result<bool,Errno> {
    refresh_namespace()?;
    if !REPLAYING_NAMESPACE.with(std::cell::Cell::get)&&mount_namespace_id().is_some(){
        if !vfs().mounts.read().unwrap().iter().any(|mount|mount.guest==guest&&mount.own){return Ok(false);}
        namespace_event(&format!("remove\t{guest}"))?;
        publish_fuse_route(guest,Path::new(""),"",None)?;return Ok(true);
    }
    if !REPLAYING_NAMESPACE.with(std::cell::Cell::get)&&vfs().mounts.read().unwrap().iter().any(|mount|mount.guest==guest&&mount.shared_fuse){publish_fuse_route(guest,Path::new(""),"",None)?;}
    let mut mounts = vfs().mounts.write().unwrap();
    match mounts.iter().position(|m| m.guest == guest && m.own) {
        Some(i) => {
            mounts.remove(i);
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Move the latest mount at `from`, and this process's mounts below it, to
/// `to`.
pub fn move_mount(from: &str, to: &str) -> Result<bool,Errno> {
    refresh_namespace()?;
    if !REPLAYING_NAMESPACE.with(std::cell::Cell::get)&&mount_namespace_id().is_some(){
        let moved=vfs().mounts.read().unwrap().iter().filter(|mount|mount.own&&(mount.guest==from||below(from,&mount.guest).is_some())).cloned().collect::<Vec<_>>();
        if !moved.iter().any(|mount|mount.guest==from){return Ok(false);}
        namespace_event(&format!("move\t{from}\t{to}"))?;
        for mount in moved{
            if mount.shared_fuse{publish_fuse_route(&mount.guest,Path::new(""),"",None)?;}
            let guest=format!("{to}{}",&mount.guest[from.len()..]);
            if let Some(route)=&mount.fuse{publish_fuse_route(&guest,&mount.host,mount.source.as_deref().unwrap_or("fuse"),Some(route))?;}
            else{publish_plain_mount(&guest,&mount.host,mount.area,mount.source.as_deref().unwrap_or(""),mount.fstype.as_deref().unwrap_or(""),mount.bind_source.as_deref())?;}
        }
        return Ok(true);
    }
    let mut mounts = vfs().mounts.write().unwrap();
    let Some(i) = mounts.iter().position(|m| m.guest == from && m.own) else {
        return Ok(false);
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
        let shared=m.shared_fuse;let projected=m.projected_fuse;
        install_mount(&guest,m.host,m.area,&source,&fstype,m.fuse,m.bind_source)?;
        let mut mounts=vfs().mounts.write().unwrap();if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==guest&&mount.own){mount.shared_fuse=shared;mount.projected_fuse=projected;}
    }
    Ok(true)
}

/// This process's own mounts, oldest first, as `--mounts` text for the
/// program it execs: `ro|rw<TAB>guest<TAB>host<TAB>source<TAB>fstype`
/// lines.
pub(crate) fn fork_mounts_text(private:bool)->Result<String,Errno>{
    let text=own_mounts_text();if !private{return Ok(text);}
    let owner=NAMESPACE_VIEW.lock().unwrap().as_ref().map(|view|view.owner.clone()).ok_or(errno::ENODEV)?;
    let id=format!("fork-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_|errno::EIO)?.as_nanos());
    let child=owner.clone_to(&id).map_err(namespace_error)?;child.append("namespace\tprivate\n").map_err(namespace_error)?;
    Ok(text.replace(&format!("namespace-owner\t{}\n",owner.id()),&format!("namespace-owner\t{id}\n")))
}
pub(crate) fn publish_fork_mounts(pid:i32,text:&str)->Result<(),Errno>{
    let Some(table)=crate::sys::cred::by_pid_dir()else{return Ok(());};
    let Some(id)=text.lines().find_map(|line|line.strip_prefix("namespace-owner\t"))else{return Err(errno::ENODEV);};
    let process=aim_storage::process_namespace::ProcessIdentity::running(pid).map_err(namespace_error)?;
    aim_storage::process_namespace::register_mount_namespace(table,process,id).map_err(namespace_error)
}

pub fn own_mounts_text() -> String {
    let mounts = vfs().mounts.read().unwrap();
    let namespace=mount_namespace_id();
    let mut out = if PRIVATE_MOUNTS.load(Ordering::Acquire){"namespace\tprivate\n".to_owned()}else{String::new()};
    if let Some(id)=namespace{out.push_str(&format!("namespace-owner\t{id}\n"));if let Some(runtime)=runtime_dir(){out.push_str(&format!("namespace-runtime\t{}\n",runtime.display()));}}
    for(root,kind)in PROPAGATION_RULES.lock().unwrap().iter(){out.push_str(&format!("propagation\t{root}\t{}\n",match kind{Propagation::Private=>"private",Propagation::Slave=>"slave",Propagation::Shared=>"shared"}));}
    let mut own: Vec<&Mount> = mounts.iter().filter(|m| m.own).collect();
    own.reverse();
    for m in own {
        let area = match m.area{Area::Image=>"ro",Area::Writable=>"rw",Area::Kernfs=>"kernfs",Area::HostDevice=>"host-device",Area::Input=>"input"};
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
        if m.projected_fuse{out.push_str(&format!("mount-projected\t{}\n",m.guest));}
        if let Some(source)=&m.bind_source{out.push_str(&format!("bind-source\t{}\t{}\n",m.guest,source));}
    }
    out
}

/// Restore the mounts [`own_mounts_text`] described.
pub fn load_own_mounts(text: &str) -> Result<(),Errno> {
    if !REPLAYING_NAMESPACE.with(std::cell::Cell::get) {
        if let Some(id)=text.lines().find_map(|line|line.strip_prefix("namespace-owner\t")){
            if let Some(runtime)=text.lines().find_map(|line|line.strip_prefix("namespace-runtime\t")).map(PathBuf::from).or_else(||runtime_dir().map(Path::to_path_buf)){
                let owner=aim_storage::mount_namespace::Namespace::open(&runtime,id).map_err(namespace_error)?;
                let page=owner.generation().map_err(namespace_error)?;
                *NAMESPACE_VIEW.lock().unwrap()=Some(NamespaceView{owner,generation:0,page,records:Default::default()});
                refresh_namespace()?;
                publish_process_namespace()?;
                return Ok(());
            }
        }
    }
    if text.lines().any(|line|line=="namespace\tprivate"){PRIVATE_MOUNTS.store(true,Ordering::Release);}
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if let ["propagation",root,kind]=f.as_slice(){let kind=match *kind{"slave"=>Propagation::Slave,"shared"=>Propagation::Shared,_=>Propagation::Private};PROPAGATION_RULES.lock().unwrap().push(((*root).into(),kind));continue;}
        if let ["mount-shared",guest]=f.as_slice(){let mut mounts=vfs().mounts.write().unwrap();if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==*guest&&mount.own){mount.shared_fuse=true;}continue;}
        if let ["mount-projected",guest]=f.as_slice(){let mut mounts=vfs().mounts.write().unwrap();if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==*guest&&mount.own){mount.projected_fuse=true;}continue;}
        if let ["bind-source",guest,source]=f.as_slice(){let mut mounts=vfs().mounts.write().unwrap();if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==*guest&&mount.own){mount.bind_source=Some((*source).into());}continue;}
        if f.len()==5||f.len()==8 {
            let(area,guest,host,source,fstype)=(f[0],f[1],f[2],f[3],f[4]);
            let area = match area {"ro"=>Area::Image,"rw"=>Area::Writable,"kernfs"=>Area::Kernfs,"host-device"=>Area::HostDevice,"input"=>Area::Input,_=>return Err(errno::EINVAL)};
            add_mount(guest, PathBuf::from(host), area, source, fstype)?;
            if f.len()==8{let policy=f[7].split(',').map(str::parse::<u32>).collect::<Result<Vec<_>,_>>().map_err(|_|errno::EINVAL)?;if policy.len()!=5||policy[2..].iter().any(|value|*value>1){return Err(errno::EINVAL);}let mut mounts=vfs().mounts.write().unwrap();if let Some(mount)=mounts.iter_mut().find(|mount|mount.guest==guest&&mount.own){mount.fuse=Some(FuseRoute{session:PathBuf::from(f[5]),relative:f[6].into(),uid:policy[0],gid:policy[1],allow_other:policy[2]!=0,default_permissions:policy[3]!=0,read_only:policy[4]!=0});}}
        }
    }
    Ok(())
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
        // The implicit root competes with bind aliases of the same host tree;
        // a later alias cannot rename ordinary paths already rooted at `/`.
        let root = self.canonical_root.get_or_init(|| self.root.canonicalize().unwrap_or_else(|_| self.root.clone()));
        let roots = [("/".to_owned(), self.root.clone()), ("/".to_owned(), root.clone())];
        for (guest, prefix) in self.mounts.iter().chain(canonical.iter()).chain(roots.iter()) {
            let Ok(rest) = host.strip_prefix(prefix) else { continue; };
            let len = prefix.as_os_str().len();
            if best.as_ref().is_none_or(|b| len > b.0 || (len == b.0 && guest.len() < b.1)) {
                let path = if rest.as_os_str().is_empty() { guest.clone() }
                    else { format!("{}/{}", guest.trim_end_matches('/'), rest.display()) };
                best = Some((len, guest.len(), path));
            }
        }
        best.map(|(_, _, guest)| guest)
    }
}

/// Guest path of a host path, if it lies inside the root or a mapped area.
pub fn guest_path_of_host(host: &Path) -> Option<String> {
    HostPathView::capture()?.guest_path(host)
}

/// Guest path of an open directory fd.
fn guest_path_of_fd(fd: i32) -> Result<String, Errno> {
    use std::os::fd::AsRawFd;
    let pin = crate::sys::fdtab::pin_guest(fd)?;
    let fd = pin.descriptor().as_raw_fd();
    if let Some(file)=crate::sys::fuse_client::get(fd) {
        if !file.directory { return Err(errno::ENOTDIR); }
        return Ok(file.guest.clone());
    }
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut stat) } < 0 { return Err(errno::last()); }
    if stat.st_mode & libc::S_IFMT != libc::S_IFDIR { return Err(errno::ENOTDIR); }
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
        let proofs = dir.join("fs-verity");
        std::fs::create_dir(&proofs).unwrap();
        let proofs = std::fs::canonicalize(proofs).unwrap();
        let mut locator = b"AIMVRTROOT01\0".to_vec();
        locator.extend_from_slice(proofs.as_os_str().as_bytes());
        std::fs::write(dir.join("run/fs-verity-root"), locator).unwrap();
        init(&root, Some(&map)).unwrap();
        dir
    });
    (guard, dir)
}

#[cfg(test)]
mod tests {
    #[test]
    fn relative_dirfd_requires_a_published_directory_and_absolute_path_ignores_it() {
        const MARKER:&str="aim-dirfd-publication-owned-child";
        if !std::env::args().any(|argument|argument==MARKER){
            let mut child=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","vfs::tests::relative_dirfd_requires_a_published_directory_and_absolute_path_ignores_it","--nocapture","--skip",MARKER]).stdout(std::process::Stdio::piped()).spawn().unwrap();
            let deadline=std::time::Instant::now()+std::time::Duration::from_secs(10);
            let status=loop{if let Some(status)=child.try_wait().unwrap(){break status;}if std::time::Instant::now()>=deadline{child.kill().unwrap();child.wait().unwrap();panic!("owned dirfd fixture exceeded deadline");}std::thread::sleep(std::time::Duration::from_millis(5));};
            use std::io::Read;let mut output=String::new();child.stdout.take().unwrap().read_to_string(&mut output).unwrap();
            assert!(status.success(),"{output}");assert!(output.contains("DIRFD_PUBLICATION_EXECUTED"));return;
        }
        println!("DIRFD_PUBLICATION_EXECUTED");
        use std::os::fd::AsRawFd;
        let (_view, root) = super::test_view();
        let dir = std::fs::File::open(root.join("data")).unwrap();
        assert!(matches!(super::resolve(dir.as_raw_fd(), b"item", false), Err(crate::errno::EBADF)));
        crate::sys::fdtab::publish_guest(dir.as_raw_fd()).unwrap();
        assert_eq!(super::resolve(dir.as_raw_fd(), b"item", false).unwrap().guest, "/data/item");
        crate::sys::fdtab::withdraw_guest(dir.as_raw_fd()).unwrap();
        let path = root.join("data/plain"); std::fs::write(&path, b"data").unwrap();
        let file = std::fs::File::open(path).unwrap();
        crate::sys::fdtab::publish_guest(file.as_raw_fd()).unwrap();
        assert!(matches!(super::resolve(file.as_raw_fd(), b"item", false), Err(crate::errno::ENOTDIR)));
        assert_eq!(super::resolve(file.as_raw_fd(), b"/data/item", false).unwrap().guest, "/data/item");
        crate::sys::fdtab::withdraw_guest(file.as_raw_fd()).unwrap();
    }
    use super::*;

    #[test]
    #[ignore="real upstream mutation helper exercised by namespace_process_probe"]
    fn namespace_upstream_probe(){
        use std::io::BufRead;
        let mut input=std::io::BufReader::new(std::io::stdin());let mut lines=Vec::new();
        for _ in 0..4{let mut line=String::new();input.read_line(&mut line).unwrap();lines.push(line.trim_end().to_owned());}
        init(Path::new(&lines[0]),Some(Path::new(&lines[1]))).unwrap();
        load_own_mounts(&format!("namespace-owner\t{}\n",lines[2])).unwrap();
        add_mount("/data/live-upstream-owner",PathBuf::from(&lines[3]),Area::Writable,"actual-upstream-child","tmpfs").unwrap();
    }

    #[test]
    #[ignore="subprocess helper exercised by separate_process_mount_owner_preserves_shared_and_private_state"]
    fn namespace_process_probe(){
        use std::io::BufRead;
        let mut input=std::io::BufReader::new(std::io::stdin());
        let mut lines=Vec::new();for _ in 0..4{let mut line=String::new();input.read_line(&mut line).unwrap();lines.push(line.trim_end().to_owned());}
        init(Path::new(&lines[0]),Some(Path::new(&lines[1]))).unwrap();
        load_own_mounts(&format!("namespace-owner\t{}\n",lines[2])).unwrap();
        let host=PathBuf::from(&lines[3]);
        add_mount("/data/live-shared-owner",host.clone(),Area::Writable,"actual-child","tmpfs").unwrap();
        let inherited=own_mounts_text();
        private_mount_namespace().unwrap();
        set_mount_propagation("/",(1<<19)|0x4000).unwrap();
        use std::io::Write;
        struct Child(std::process::Child);
        impl Drop for Child{fn drop(&mut self){if self.0.try_wait().ok().flatten().is_none(){let _=self.0.kill();}let _=self.0.wait();}}
        let mut upstream=Child(std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","vfs::tests::namespace_upstream_probe","--ignored","--nocapture"]).stdin(std::process::Stdio::piped()).spawn().unwrap());
        writeln!(upstream.0.stdin.take().unwrap(),"{}\n{}\n{}\n{}",lines[0],lines[1],lines[2],lines[3]).unwrap();
        assert!(upstream.0.wait().unwrap().success());refresh_fuse_mounts().unwrap();
        assert_eq!(lookup("/data/live-upstream-owner/file").0,host.join("file"));
        add_mount("/data/live-slave-only",host.clone(),Area::Writable,"actual-slave-child","tmpfs").unwrap();
        set_mount_propagation("/",(1<<18)|0x4000).unwrap();
        add_mount("/data/live-private-owner",host.clone(),Area::Writable,"actual-private-child","tmpfs").unwrap();
        refresh_fuse_mounts().unwrap();
        assert_eq!(lookup("/data/live-private-owner/file").0,host.join("file"));
        // Exec restores the shared owner, rather than a stale mount snapshot.
        load_own_mounts(&inherited).unwrap();refresh_fuse_mounts().unwrap();
        assert_ne!(lookup("/data/live-private-owner/file").0,host.join("file"));
        assert_eq!(lookup("/data/live-shared-owner/file").0,host.join("file"));
    }

    #[test]
    fn separate_process_mount_owner_preserves_shared_and_private_state(){
        use std::io::Write;
        struct Child(std::process::Child);
        impl Drop for Child{fn drop(&mut self){if self.0.try_wait().ok().flatten().is_none(){let _=self.0.kill();}let _=self.0.wait();}}
        let (_guard,root)=test_view();let runtime=runtime_dir().unwrap();
        let old_rules=PROPAGATION_RULES.lock().unwrap().clone();let old_private=PRIVATE_MOUNTS.load(Ordering::Acquire);
        let old_view=NAMESPACE_VIEW.lock().unwrap().take();
        let table=fuse_table().unwrap();let old_table=std::fs::read(&table).ok();
        let id=format!("live-process-{}",std::process::id());
        let owner=aim_storage::mount_namespace::Namespace::open(runtime,&id).unwrap();
        owner.initialize(&format!("root\t/\t{}\npropagation\t/\tshared\n{}",crate::vfs::root().display(),std::fs::read_to_string(runtime.join("path-map")).unwrap())).unwrap();
        owner.append("propagation-flags\t/\t1064960").unwrap();
        *NAMESPACE_VIEW.lock().unwrap()=Some(NamespaceView{page:owner.generation().unwrap(),owner:owner.clone(),generation:0,records:Default::default()});refresh_namespace().unwrap();
        let host=root.join("live-process-host");std::fs::create_dir_all(&host).unwrap();
        let mut child=Child(std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","vfs::tests::namespace_process_probe","--ignored","--nocapture"]).stdin(std::process::Stdio::piped()).spawn().unwrap());
        writeln!(child.0.stdin.take().unwrap(),"{}\n{}\n{}\n{}",crate::vfs::root().display(),runtime.join("path-map").display(),id,host.display()).unwrap();
        assert!(child.0.wait().unwrap().success());
        refresh_fuse_mounts().unwrap();
        assert_eq!(lookup("/data/live-shared-owner/file").0,host.join("file"));
        assert_ne!(lookup("/data/live-private-owner/file").0,host.join("file"));
        assert_ne!(lookup("/data/live-slave-only/file").0,host.join("file"));
        assert_eq!(lookup("/data/live-upstream-owner/file").0,host.join("file"));
        let inventory=namespace_mount_points(&id).unwrap();
        assert!(inventory.iter().any(|mount|mount.guest=="/data/live-shared-owner"));
        assert!(!inventory.iter().any(|mount|mount.guest=="/data/live-private-owner"));
        *NAMESPACE_VIEW.lock().unwrap()=old_view;
        *PROPAGATION_RULES.lock().unwrap()=old_rules;PRIVATE_MOUNTS.store(old_private,Ordering::Release);
        let (_,base)=parse_map(&std::fs::read_to_string(runtime.join("path-map")).unwrap()).unwrap();*vfs().mounts.write().unwrap()=base;
        if let Some(bytes)=old_table{std::fs::write(table,bytes).unwrap();}else{let _=std::fs::remove_file(table);}
        std::fs::remove_dir_all(host).unwrap();
    }

    #[test]
    fn shared_namespace_generation_refreshes_mounts_and_unshare_keeps_private_owner() {
        let (_guard,root)=test_view();
        let runtime=runtime_dir().unwrap();
        let id=format!("test-namespace-{}",std::process::id());
        let owner=aim_storage::mount_namespace::Namespace::open(runtime,&id).unwrap();
        owner.initialize(&format!("root\t/\t{}\npropagation\t/\tshared\n{}",crate::vfs::root().display(),std::fs::read_to_string(runtime.join("path-map")).unwrap())).unwrap();
        let old_rules=PROPAGATION_RULES.lock().unwrap().clone();let old_private=PRIVATE_MOUNTS.load(Ordering::Acquire);
        let previous=NAMESPACE_VIEW.lock().unwrap().take();
        *NAMESPACE_VIEW.lock().unwrap()=Some(NamespaceView{page:owner.generation().unwrap(),owner:owner.clone(),generation:0,records:Default::default()});
        refresh_namespace().unwrap();
        set_mount_propagation("/",(1<<18)|0x4000).unwrap();
        let host=root.join("shared-real-mount");std::fs::create_dir_all(&host).unwrap();
        add_mount("/data/shared-owner-test",host.clone(),Area::Writable,"tmpfs","tmpfs").unwrap();
        assert_eq!(lookup("/data/shared-owner-test/file").0,host.join("file"));
        let external=aim_storage::mount_namespace::Namespace::open(runtime,&id).unwrap();
        external.append("remove\t/data/shared-owner-test").unwrap();
        refresh_fuse_mounts().unwrap();assert_ne!(lookup("/data/shared-owner-test/file").0,host.join("file"));
        let inherited=own_mounts_text();
        private_mount_namespace().unwrap();let private=mount_namespace_id().unwrap();assert_ne!(private,id);
        add_mount("/data/private-owner-test",host.clone(),Area::Writable,"tmpfs","tmpfs").unwrap();
        load_own_mounts(&inherited).unwrap();assert_eq!(mount_namespace_id().as_deref(),Some(id.as_str()));
        assert_ne!(lookup("/data/private-owner-test/file").0,host.join("file"));
        let clone=aim_storage::mount_namespace::Namespace::open(runtime,&private).unwrap();assert!(clone.read().unwrap().events.iter().any(|event|event.contains("private-owner-test")));
        *NAMESPACE_VIEW.lock().unwrap()=previous;
        *PROPAGATION_RULES.lock().unwrap()=old_rules;PRIVATE_MOUNTS.store(old_private,Ordering::Release);
        let (_,base)=parse_map(&std::fs::read_to_string(runtime.join("path-map")).unwrap()).unwrap();*vfs().mounts.write().unwrap()=base;
        std::fs::remove_dir_all(host).unwrap();
    }

    #[test]
    fn interrupted_mount_journal_lock_waits_for_its_owner() {
        const CHILD: &str = "__isolated_mount_lock_fork";
        if !std::env::args().any(|argument| argument == CHILD) {
            use std::os::unix::process::CommandExt;
            struct ChildGroup(std::process::Child);
            impl Drop for ChildGroup {
                fn drop(&mut self) {
                    if self.0.try_wait().ok().flatten().is_none() {
                        unsafe { libc::killpg(self.0.id() as i32, libc::SIGKILL); }
                        let _ = self.0.wait();
                    }
                }
            }
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "vfs::tests::interrupted_mount_journal_lock_waits_for_its_owner", "--skip", CHILD, "--nocapture"])
                .process_group(0).spawn().unwrap();
            let mut child = ChildGroup(child);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                if let Some(status) = child.0.try_wait().unwrap() {
                    assert!(status.success(), "isolated raw-fork fixture: {status}");
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    unsafe { libc::killpg(child.0.id() as i32, libc::SIGKILL); }
                    child.0.wait().unwrap();
                    panic!("isolated raw-fork fixture exceeded ten seconds");
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            return;
        }
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
    fn inverse_root_bind_keeps_library_path_and_more_specific_mount() {
        use std::os::fd::{AsRawFd, IntoRawFd};
        let (_view, dir) = test_view();
        let root = vfs().root.clone();
        let library = root.join("system/lib64/inverse-root-library.so");
        std::fs::create_dir_all(library.parent().unwrap()).unwrap();
        std::fs::write(&library, b"actual library path owner").unwrap();
        let file = std::fs::File::open(&library).unwrap();
        let mut path = [0u8; libc::PATH_MAX as usize];
        assert_eq!(unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, path.as_mut_ptr()) }, 0);
        let host = PathBuf::from(std::ffi::OsStr::from_bytes(&path[..path.iter().position(|byte| *byte == 0).unwrap()]));
        assert_eq!(guest_path_of_host(&host).as_deref(), Some("/system/lib64/inverse-root-library.so"));
        let alias = "/inverse-reboot-root/mount_tmp";
        bind_mount_recursive("/", alias, root.clone(), Area::Image, false).unwrap();
        assert_eq!(lookup(&format!("{alias}/system/lib64/inverse-root-library.so")).0, library);
        assert_eq!(guest_path_of_host(&host).as_deref(), Some("/system/lib64/inverse-root-library.so"));
        assert_eq!(guest_path_of_host(&root).as_deref(), Some("/"));
        let fd = file.into_raw_fd();
        crate::sys::fdtab::publish_guest(fd).unwrap();
        let link = CString::new(format!("/proc/self/fd/{fd}")).unwrap();
        let mut answer = [0u8; libc::PATH_MAX as usize];
        let mut context: crate::context::GuestContext = unsafe { std::mem::zeroed() };
        context.x[8] = 78;
        context.x[..4].copy_from_slice(&[LINUX_AT_FDCWD as u64, link.as_ptr() as u64, answer.as_mut_ptr() as u64, answer.len() as u64]);
        crate::sys::dispatch(&mut context);
        let count = context.x[0] as i64;
        assert!(count > 0, "actual proc-fd readlink: {count}");
        assert_eq!(&answer[..count as usize], b"/system/lib64/inverse-root-library.so");
        context.x[8] = 57;
        context.x[0] = fd as u64;
        crate::sys::dispatch(&mut context);
        assert_eq!(context.x[0] as i64, 0);
        let nested = dir.join("inverse-specific-library");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("library.so"), b"actual nested mount").unwrap();
        add_mount("/system/lib64/inverse-specific", nested.clone(), Area::Writable, "tmpfs", "tmpfs").unwrap();
        assert_eq!(guest_path_of_host(&nested.canonicalize().unwrap().join("library.so")).as_deref(), Some("/system/lib64/inverse-specific/library.so"));
        assert!(remove_mount("/system/lib64/inverse-specific").unwrap());
        assert!(remove_mount(alias).unwrap());
        std::fs::remove_file(library).unwrap();
        std::fs::remove_dir_all(nested).unwrap();
    }

    #[test]
    fn canonical_host_aliases_preserve_deepest_shortest_and_hidden_mounts() {
        let (_view, dir) = test_view(); let data = dir.join("data"); let nested = data.join("inverse-alias"); std::fs::create_dir_all(&nested).unwrap();
        let alias = dir.join("inverse-host-alias"); std::os::unix::fs::symlink(&data, &alias).unwrap();
        let canonical = nested.canonicalize().unwrap();
        add_mount("/inverse-parent", data.canonicalize().unwrap(), Area::Writable, "tmpfs", "tmpfs").unwrap();
        add_mount("/inverse-short", alias.join("inverse-alias"), Area::Writable, "tmpfs", "tmpfs").unwrap();
        add_mount("/inverse-long-name", alias.join("inverse-alias"), Area::Writable, "tmpfs", "tmpfs").unwrap();
        assert_eq!(guest_path_of_host(&canonical.join("file")).as_deref(), Some("/inverse-short/file"));
        let read = HostPathView::capture().unwrap();
        assert_eq!(read.guest_path(&canonical.join("file")).as_deref(), Some("/inverse-short/file"));
        let hidden_host = dir.join("inverse-hidden"); std::fs::create_dir_all(&hidden_host).unwrap();
        add_mount("/inverse-short", hidden_host, Area::Writable, "tmpfs", "tmpfs").unwrap();
        assert_eq!(guest_path_of_host(&canonical.join("file")).as_deref(), Some("/inverse-long-name/file"));
        assert_eq!(read.guest_path(&canonical.join("file")).as_deref(), Some("/inverse-short/file"));
        let next_read = HostPathView::capture().unwrap();
        assert_eq!(next_read.guest_path(&canonical.join("file")).as_deref(), Some("/inverse-long-name/file"));
        assert!(remove_mount("/inverse-short").unwrap()); assert!(remove_mount("/inverse-short").unwrap()); assert!(remove_mount("/inverse-long-name").unwrap()); assert!(remove_mount("/inverse-parent").unwrap());
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
        add_mount("/inverse-changing", alias.clone(), Area::Writable, "tmpfs", "tmpfs").unwrap();
        let read = HostPathView::capture().unwrap();
        let before_file = before.canonicalize().unwrap().join("file");
        assert_eq!(read.guest_path(&before_file).as_deref(), Some("/inverse-changing/file"));
        std::fs::remove_file(&alias).unwrap();
        std::os::unix::fs::symlink(&after, &alias).unwrap();
        assert_eq!(read.guest_path(&before_file).as_deref(), Some("/inverse-changing/file"));
        assert_eq!(guest_path_of_host(&after.canonicalize().unwrap().join("file")).as_deref(), Some("/inverse-changing/file"));
        assert!(guest_path_of_host(&before_file).is_none());
        assert!(remove_mount("/inverse-changing").unwrap());
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

        add_mount("/data/app", tmp.clone(), Area::Writable, "tmpfs", "tmpfs").unwrap();
        assert_eq!(lookup("/data/app/x").0, tmp.join("x"));
        assert_eq!(lookup("/data/other").0, data.join("other"));
        assert!(
            mount_points().unwrap()
                .iter()
                .any(|m| m.guest == "/data/app" && m.fstype.as_deref() == Some("tmpfs"))
        );
        // Test explicit mount reconstruction, not a shared namespace handle
        // (which must observe its live removals rather than rewind them).
        let text=own_mounts_text().lines().filter(|line|!line.starts_with("namespace-owner\t")&&!line.starts_with("namespace-runtime\t")).collect::<Vec<_>>().join("\n");
        assert!(move_mount("/data/app", "/data/moved").unwrap());
        assert_eq!(lookup("/data/moved").0, tmp);
        assert!(remove_mount("/data/moved").unwrap());
        assert!(!remove_mount("/data/moved").unwrap());
        assert!(!remove_mount("/data").unwrap(), "path map entries are not unmounted");

        load_own_mounts(&text).unwrap();
        assert_eq!(lookup("/data/app").0, tmp);
        assert!(remove_mount("/data/app").unwrap());
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

        add_mount("/data/user", tmp.clone(), Area::Writable, "tmpfs", "tmpfs").unwrap();
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
        ).unwrap();
        assert_eq!(lookup("/data/user/0/p").0, data.join("data/p"));
        assert!(remove_mount("/data/user/0").unwrap());
        assert!(remove_mount("/data/user").unwrap());
        assert_eq!(lookup("/data/user/0/p").0, data.join("data/p"));
    }
}

#[cfg(test)]
mod fuse_route_tests {
    use super::*;
    fn actual_fuse_publication(guest:&str,host:&Path,source:&str,route:Option<&FuseRoute>)->Result<(),Errno>{
        match route{Some(route)=>install_mount(guest,host.into(),Area::Writable,source,"fuse",Some(route.clone()),None),None=>remove_mount(guest).map(|_|())}
    }
    fn actual_plain_publication(guest:&str,host:&Path,area:Area,source:&str,fstype:&str,bind:Option<&str>)->Result<(),Errno>{install_mount(guest,host.into(),area,source,fstype,None,bind.map(str::to_owned))}

    #[test]
    fn init_path_map_alias_receives_late_mount_and_exec_import_does_not_duplicate_shared_entries(){
        if crate::sys::fdtab::isolated_kernel_test("vfs::fuse_route_tests::init_path_map_alias_receives_late_mount_and_exec_import_does_not_duplicate_shared_entries"){return;}
        let(_guard,root)=test_view();let old_private=PRIVATE_MOUNTS.swap(false,Ordering::AcqRel);let old_rules=std::mem::take(&mut *PROPAGATION_RULES.lock().unwrap());let parent=root.join("init-map-parent");let lower=root.join("init-map-lower");for path in [&parent,&lower]{std::fs::create_dir_all(path).unwrap();}
        let text=format!("root\t/\t{}\nrw\t/storage/aim-init-alias\t{}\nbind-source\t/storage/aim-init-alias\t/mnt/aim-init-user\npropagation\t/storage/aim-init-alias\tslave\n",root.display(),parent.display());
        let(_,entries)=parse_map(&text).unwrap();assert_eq!(entries[0].bind_source.as_deref(),Some("/mnt/aim-init-user"));
        add_mount("/mnt/aim-init-user",parent.clone(),Area::Writable,"parent","bind").unwrap();
        let owner=NAMESPACE_VIEW.lock().unwrap().as_ref().unwrap().owner.clone();let prior=owner.read().unwrap();owner.update_base(&format!("{}{}",prior.base,text.lines().filter(|line|!line.starts_with("root\t")).collect::<Vec<_>>().join("\n"))).unwrap();refresh_namespace().unwrap();
        actual_plain_publication("/mnt/aim-init-user/emulated",&lower,Area::Writable,"lower","bind",None).unwrap();refresh_fuse_mounts().unwrap();assert_eq!(lookup("/storage/aim-init-alias/emulated/0").0,lower.join("0"));
        let inherited=own_mounts_text();load_own_mounts(&inherited).unwrap();refresh_fuse_mounts().unwrap();
        assert_eq!(vfs().mounts.read().unwrap().iter().filter(|mount|mount.guest=="/mnt/aim-init-user/emulated"&&mount.shared_fuse).count(),1);
        assert_eq!(lookup("/storage/aim-init-alias/emulated/0").0,lower.join("0"));
        // An imported projection must remain an inherited event, not turn
        // into an explicit child mount hiding the next parent event.
        let retargeted=root.join("init-map-retargeted");std::fs::create_dir_all(&retargeted).unwrap();
        actual_plain_publication("/mnt/aim-init-user/emulated",&retargeted,Area::Writable,"retargeted","bind",None).unwrap();refresh_fuse_mounts().unwrap();
        assert_eq!(lookup("/storage/aim-init-alias/emulated/0").0,retargeted.join("0"));
        assert_eq!(vfs().mounts.read().unwrap().iter().filter(|mount|mount.guest=="/storage/aim-init-alias/emulated"&&mount.projected_fuse).count(),1);
        actual_fuse_publication("/mnt/aim-init-user/emulated",&lower,"",None).unwrap();refresh_fuse_mounts().unwrap();*PROPAGATION_RULES.lock().unwrap()=old_rules;PRIVATE_MOUNTS.store(old_private,Ordering::Release);
    }
    #[test]
    fn late_plain_pass_through_bind_propagates_actual_lower_into_slave_alias(){
        if crate::sys::fdtab::isolated_kernel_test("vfs::fuse_route_tests::late_plain_pass_through_bind_propagates_actual_lower_into_slave_alias"){return;}
        let(_guard,root)=test_view();let old_private=PRIVATE_MOUNTS.swap(false,Ordering::AcqRel);let old_rules=std::mem::take(&mut *PROPAGATION_RULES.lock().unwrap());
        let parent=root.join("plain-parent");let lower=root.join("plain-lower");std::fs::create_dir_all(&parent).unwrap();std::fs::create_dir_all(lower.join("0/Android/data")).unwrap();
        add_mount("/mnt/aim-pass",parent.clone(),Area::Writable,"parent","bind").unwrap();
        bind_mount_recursive("/mnt/aim-pass","/storage/aim-pass-slave",parent.clone(),Area::Writable,true).unwrap();
        bind_mount_recursive("/mnt/aim-pass","/storage/aim-pass-private",parent.clone(),Area::Writable,true).unwrap();
        set_mount_propagation("/storage/aim-pass-slave",(1<<19)|0x4000).unwrap();set_mount_propagation("/storage/aim-pass-private",(1<<18)|0x4000).unwrap();
        actual_plain_publication("/mnt/aim-pass/emulated",&lower,Area::Writable,"/data/media","bind",Some("/data/media")).unwrap();refresh_fuse_mounts().unwrap();
        assert_eq!(lookup("/storage/aim-pass-slave/emulated/0/Android/data").0,lower.join("0/Android/data"));
        assert!(fuse_route("/storage/aim-pass-slave/emulated/0/Android/data").is_none());
        assert_ne!(lookup("/storage/aim-pass-private/emulated/0/Android/data").0,lower.join("0/Android/data"));
        actual_fuse_publication("/mnt/aim-pass/emulated",&lower,"",None).unwrap();refresh_fuse_mounts().unwrap();
        assert_ne!(lookup("/storage/aim-pass-slave/emulated/0/Android/data").0,lower.join("0/Android/data"));
        for path in ["/storage/aim-pass-private","/storage/aim-pass-slave","/mnt/aim-pass"]{assert!(remove_mount(path).unwrap());}
        *PROPAGATION_RULES.lock().unwrap()=old_rules;PRIVATE_MOUNTS.store(old_private,Ordering::Release);
    }
    #[test]
    fn early_parent_alias_projects_late_fuse_event_into_slave_but_not_private_view(){
        if crate::sys::fdtab::isolated_kernel_test("vfs::fuse_route_tests::early_parent_alias_projects_late_fuse_event_into_slave_but_not_private_view"){return;}
        let(_guard,root)=test_view();let old_private=PRIVATE_MOUNTS.swap(false,Ordering::AcqRel);let old_rules=std::mem::take(&mut *PROPAGATION_RULES.lock().unwrap());
        let parent=root.join("late-source");std::fs::create_dir_all(&parent).unwrap();
        add_mount("/mnt/aim-late-user",parent.clone(),Area::Writable,"user","bind").unwrap();
        bind_mount_recursive("/mnt/aim-late-user","/mnt/aim-late-installer",parent.clone(),Area::Writable,true).unwrap();
        bind_mount_recursive("/mnt/aim-late-installer","/storage/aim-late-slave",parent.clone(),Area::Writable,true).unwrap();
        bind_mount_recursive("/mnt/aim-late-installer","/storage/aim-late-private",parent.clone(),Area::Writable,true).unwrap();
        set_mount_propagation("/storage/aim-late-slave",(1<<19)|0x4000).unwrap();set_mount_propagation("/storage/aim-late-private",(1<<18)|0x4000).unwrap();
        let route=FuseRoute{session:root.join("late-session.sock"),relative:String::new(),uid:0,gid:0,allow_other:true,default_permissions:false,read_only:false};
        actual_fuse_publication("/mnt/aim-late-user/emulated",&parent,"fuse",Some(&route)).unwrap();refresh_fuse_mounts().unwrap();
        assert_eq!(fuse_route("/storage/aim-late-slave/emulated/0/Pictures").unwrap().relative,"0/Pictures");
        assert!(fuse_route("/storage/aim-late-private/emulated/0/Pictures").is_none());
        assert!(own_mounts_text().contains("bind-source\t/mnt/aim-late-installer\t/mnt/aim-late-user"));
        actual_fuse_publication("/mnt/aim-late-user/emulated",&parent,"",None).unwrap();refresh_fuse_mounts().unwrap();
        assert!(fuse_route("/storage/aim-late-slave/emulated/0/Pictures").is_none());
        for path in ["/storage/aim-late-private","/storage/aim-late-slave","/mnt/aim-late-installer","/mnt/aim-late-user"]{assert!(remove_mount(path).unwrap());}
        *PROPAGATION_RULES.lock().unwrap()=old_rules;PRIVATE_MOUNTS.store(old_private,Ordering::Release);
    }
    #[test]
    fn recursive_bind_clones_fuse_and_android_submounts_nonrecursive_excludes_them(){
        if crate::sys::fdtab::isolated_kernel_test("vfs::fuse_route_tests::recursive_bind_clones_fuse_and_android_submounts_nonrecursive_excludes_them"){return;}
        let(_guard,root)=test_view();
        let parent=root.join("recursive-source");let anchor=root.join("recursive-fuse-anchor");let android=root.join("recursive-android");
        for path in [&parent,&anchor,&android]{std::fs::create_dir_all(path).unwrap();}
        let options=crate::sys::fuse_mount::parse(b"fd=17,rootmode=40000,user_id=0,group_id=0,allow_other,").unwrap();
        add_mount("/mnt/aim-rec-source",parent.clone(),Area::Writable,"source","bind").unwrap();
        let session=root.join("recursive-session.sock");
        add_fuse_mount("/mnt/aim-rec-source/emulated",session.clone(),anchor,"fuse",&options,false).unwrap();
        add_mount("/mnt/aim-rec-source/emulated/0/Android/data",android.clone(),Area::Writable,"android-data","bind").unwrap();
        bind_mount_recursive("/mnt/aim-rec-source","/mnt/aim-nonrec",parent.clone(),Area::Writable,false).unwrap();
        assert!(fuse_route("/mnt/aim-nonrec/emulated/0/Documents").is_none());
        assert_ne!(lookup("/mnt/aim-nonrec/emulated/0/Android/data/pkg").0,android.join("pkg"));
        bind_mount_recursive("/mnt/aim-rec-source","/mnt/aim-rec-target",parent,Area::Writable,true).unwrap();
        let route=fuse_route("/mnt/aim-rec-target/emulated/0/Documents").unwrap();assert_eq!(route.session,session);assert_eq!(route.relative,"0/Documents");assert!(route.allow_other);
        assert_eq!(lookup("/mnt/aim-rec-target/emulated/0/Android/data/pkg").0,android.join("pkg"));
        assert!(fuse_route("/mnt/aim-rec-target/emulated/0/Android/data/pkg").is_none());
        for path in ["/mnt/aim-rec-target/emulated/0/Android/data","/mnt/aim-rec-target/emulated","/mnt/aim-rec-target","/mnt/aim-nonrec","/mnt/aim-rec-source/emulated/0/Android/data","/mnt/aim-rec-source/emulated","/mnt/aim-rec-source"]{assert!(remove_mount(path).unwrap());}
    }
    #[test]
    fn slave_namespace_imports_future_parent_fuse_routes_without_publishing_child_mounts(){
        if crate::sys::fdtab::isolated_kernel_test("vfs::fuse_route_tests::slave_namespace_imports_future_parent_fuse_routes_without_publishing_child_mounts"){return;}
        let(_guard,root)=test_view();let old_private=PRIVATE_MOUNTS.swap(false,Ordering::AcqRel);
        let old_rules=std::mem::take(&mut *PROPAGATION_RULES.lock().unwrap());
        let parent="/mnt/aim-slave-parent";let child="/mnt/aim-slave-child";
        let anchor=root.join("slave-anchor");std::fs::create_dir_all(&anchor).unwrap();
        let route=FuseRoute{session:root.join("slave-session.sock"),relative:String::new(),uid:0,gid:0,allow_other:true,default_permissions:false,read_only:false};
        let upstream=NAMESPACE_VIEW.lock().unwrap().as_ref().unwrap().owner.clone();
        private_mount_namespace().unwrap();set_mount_propagation("/",(1<<19)|0x4000).unwrap();
        // Parent publication is represented by its actual shared namespace table.
        let slave=NAMESPACE_VIEW.lock().unwrap().take().unwrap();
        *NAMESPACE_VIEW.lock().unwrap()=Some(NamespaceView{page:upstream.generation().unwrap(),owner:upstream.clone(),generation:0,records:Default::default()});refresh_namespace().unwrap();
        actual_fuse_publication(parent,&anchor,"fuse",Some(&route)).unwrap();
        *NAMESPACE_VIEW.lock().unwrap()=Some(slave);refresh_namespace().unwrap();
        refresh_fuse_mounts().unwrap();assert_eq!(fuse_route(parent).unwrap().session,route.session);
        bind_mount(parent,child,anchor.clone(),Area::Writable,"fuse","bind").unwrap();
        assert!(fuse_route(child).is_some());assert!(!fuse_table_lines().unwrap().iter().any(|line|line.split('\t').next()==Some(child)));
        remove_mount(child).unwrap();let slave=NAMESPACE_VIEW.lock().unwrap().take().unwrap();
        *NAMESPACE_VIEW.lock().unwrap()=Some(NamespaceView{page:upstream.generation().unwrap(),owner:upstream,generation:0,records:Default::default()});refresh_namespace().unwrap();actual_fuse_publication(parent,&anchor,"",None).unwrap();
        *NAMESPACE_VIEW.lock().unwrap()=Some(slave);refresh_namespace().unwrap();refresh_fuse_mounts().unwrap();
        *PROPAGATION_RULES.lock().unwrap()=old_rules;PRIVATE_MOUNTS.store(old_private,Ordering::Release);
    }
    #[test]
    fn fuse_bind_view_preserves_session_and_relative_tree_without_raw_path_resolution(){
        if crate::sys::fdtab::isolated_kernel_test("vfs::fuse_route_tests::fuse_bind_view_preserves_session_and_relative_tree_without_raw_path_resolution"){return;}
        let(_guard,root)=test_view();let target="/mnt/aim-fuse-test";
        let options=crate::sys::fuse_mount::parse(b"fd=17,rootmode=40000,user_id=0,group_id=0,allow_other").unwrap();
        let anchor=root.join("fuse-anchor");std::fs::create_dir_all(&anchor).unwrap();
        let session=root.join("fuse-session.sock");
        add_fuse_mount(target,session.clone(),anchor.clone(),"fuse",&options,false).unwrap();
        bind_mount("/mnt/aim-fuse-test/user/0","/storage/aim-fuse-test",anchor,Area::Writable,"fuse","bind").unwrap();
        let route=fuse_route("/storage/aim-fuse-test/Documents/a.txt").unwrap();
        assert_eq!(route.session,session);assert_eq!(route.relative,"user/0/Documents/a.txt");assert!(route.allow_other);
        let text=own_mounts_text();assert!(text.contains("fuse-session.sock"));
        assert!(remove_mount("/storage/aim-fuse-test").unwrap());assert!(remove_mount(target).unwrap());
        assert!(fuse_route("/storage/aim-fuse-test/Documents/a.txt").is_none());
    }
}
