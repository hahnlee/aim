//! Guest fds whose Linux behaviour is more than the Darwin object behind
//! them.
//!
//! Every guest fd is a real host fd, so dup, fork, close-on-exec and
//! descriptor passing keep working. Some need Linux semantics on top: an
//! eventfd is a datagram socketpair carrying counter records, a SEQPACKET
//! socket is a framed stream socket, epoll and inotify are kqueues. Those
//! fds are listed here and have their [`SLOW`] byte set, so the lean path in
//! `trampoline.S` sends read, write, pread, pwrite and close on them to Rust.
//!
//! Sockets carry a marker (the `SO_LINGER` time, meaningless while lingering
//! is off), and memfds marker flags, so a SEQPACKET or datagram socket or a
//! memfd that arrives by `SCM_RIGHTS`, across exec or from the binder driver
//! is recognized by [`adopt`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock,OnceLock};

use super::{epoll, evdev, event, inotify, knob, memfd, net, random, sync_file};

/// Guest fds below this have a byte in [`SLOW`]; the lean path sends larger
/// fds to Rust.
pub const SLOW_FDS: usize = 65536;

/// Read by `trampoline.S`: nonzero sends read/write/pread/pwrite/close on
/// the fd to Rust.
pub static SLOW: [AtomicU8; SLOW_FDS] = [const { AtomicU8::new(0) }; SLOW_FDS];

#[derive(Clone)]
pub enum Kind {
    Regular(Arc<super::regular_file::Description>),
    /// A Linux O_PATH open description; flags do not grant read/write access.
    Path(Arc<PathDescription>),
    ProxyFile,
    Event(Arc<event::EventFd>),
    Timer(Arc<event::TimerFd>),
    Sock(Arc<net::Sock>),
    Epoll(Arc<epoll::Epoll>),
    Memfd(memfd::Key),
    Inotify(Arc<inotify::Inotify>),
    /// A directory stream: host entries read once, or a synthesized
    /// `/proc`/`/sys` directory.
    Dir(Arc<Mutex<super::dir::DirStream>>),
    /// A synthesized kernel file: an unlinked file holding its contents
    /// (`procfs::content_fd`).
    Content,
    /// A kernel file whose writes act (`knob`); also a content file.
    Knob(Arc<knob::Knob>),
    /// An open evdev device (`/dev/input/eventN`).
    Evdev(Arc<evdev::Evdev>),
    /// A fence (`sync_file`); its state is the host socket's.
    SyncFile,
    /// `/dev/random` or `/dev/urandom` open for writing (`random`).
    Random,
    /// A binder device file; the daemon holds its state.
    Binder(aim_binder_host::client::BinderFile),
}

pub(super) struct PathProof(std::os::fd::OwnedFd);
impl PathProof {
    fn new(fd: std::os::fd::OwnedFd) -> Arc<Self> {
        use std::os::fd::{FromRawFd, IntoRawFd};
        Arc::new(Self(unsafe { std::os::fd::OwnedFd::from_raw_fd(hide(fd.into_raw_fd())) }))
    }
}
impl Drop for PathProof { fn drop(&mut self) {
    use std::os::fd::AsRawFd; unhide(self.0.as_raw_fd());
} }

pub struct PathDescription {
    pub flags: u64,
    proof: Mutex<Option<Arc<PathProof>>>,
}
impl PathDescription {
    pub(super) fn new(flags: u64) -> Arc<Self> {
        Arc::new(Self { flags: flags & !super::fs::O_CLOEXEC, proof: Mutex::new(None) })
    }
}

/// Keeps a hidden proof alive through the actual descriptor transfer syscall.
pub struct ExportedFd{pub fd:i32,_proof:Option<Arc<PathProof>>,_pin:Pinned}
pub(super) fn export_fd(fd:i32)->Result<ExportedFd,crate::errno::Errno>{export_pinned(pin_guest(fd)?)}
pub(super) fn export_pinned(pin:Pinned)->Result<ExportedFd,crate::errno::Errno>{
    use std::os::fd::AsRawFd;
    let fd=pin.descriptor().as_raw_fd();
    let Some(Kind::Path(path))=pin.kind()else{return Ok(ExportedFd{fd,_proof:None,_pin:pin})};
    let mut proof=path.proof.lock().unwrap();
    if proof.is_none(){let carrier=super::binder::create_path(fd,path.flags as u32)?;*proof=Some(PathProof::new(carrier));}
    let held=proof.as_ref().unwrap().clone();drop(proof);
    Ok(ExportedFd{fd:held.0.as_raw_fd(),_proof:Some(held),_pin:pin})
}
pub enum ScmExport{Regular(RegularExport),Socket(super::net::SocketExport),Other(ExportedFd)}
pub fn export_scm(fd:i32)->Result<ScmExport,crate::errno::Errno>{
    let pin=pin_guest(fd)?;
    match pin.kind().cloned(){Some(Kind::Regular(description))=>super::regular_file::export(pin,description).map(ScmExport::Regular),Some(Kind::Sock(_))=>super::net::export_socket_pinned(pin).map(ScmExport::Socket),_=>export_pinned(pin).map(ScmExport::Other)}
}

pub(super) fn install_path(fd: i32) -> Result<(), crate::errno::Errno> {
    use std::os::fd::{AsRawFd, BorrowedFd};
    let old_flags = unsafe { libc::fcntl(fd,libc::F_GETFD) };
    if old_flags < 0 { return Err(crate::errno::last()); }
    let adopted = aim_binder_host::path_file::unwrap(unsafe { BorrowedFd::borrow_raw(fd) })
        .map_err(|error| crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?;
    let proof = PathProof::new(adopted.proof.try_clone()
        .map_err(|error| crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?);
    on_close(fd);
    if unsafe { libc::dup2(adopted.backing.as_raw_fd(),fd) } < 0 { return Err(crate::errno::last()); }
    if unsafe { libc::fcntl(fd,libc::F_SETFD,old_flags) } < 0 { return Err(crate::errno::last()); }
    super::fuse_client::adopt(fd)?;
    insert(fd,Kind::Path(Arc::new(PathDescription { flags: adopted.flags as u64, proof:Mutex::new(Some(proof)) })));
    Ok(())
}

/// The imported send right remains owned by the Binder receipt. The target
/// slot stays unpublished until its complete typed description is validated.
pub fn install_fileport(fd:i32,port:aim_binder_host::mach::Port)->Result<(),crate::errno::Errno>{
    use std::os::fd::{AsRawFd,FromRawFd};
    let backing=aim_storage::private_fd::PrivateFd::allocate(||{
        let raw=aim_binder_host::mach::port_to_fd(port).ok_or_else(||std::io::Error::from_raw_os_error(libc::EIO))?;
        Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(raw)})
    }).map_err(|error|crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?;
    let guard=lifecycle();
    if is_hidden(fd){return Err(crate::errno::EBADF);}
    let flags=unsafe{libc::fcntl(fd,libc::F_GETFD)};if flags<0{return Err(crate::errno::last());}
    let retired=get(fd);let retired_fuse=super::fuse_client::get(fd);let posix=guest_close_owner(fd)?;
    let socket=matches!(retired,Some(Kind::Sock(_)));
    let was_visible=visible(fd);if was_visible{withdraw_guest(fd)?;}
    let result=unsafe{libc::dup2(backing.as_raw_fd(),fd)};
    if result>=0{on_close(fd);}else if was_visible{publish_guest(fd)?;}
    let error=if result<0{Some(crate::errno::last())}else if unsafe{libc::fcntl(fd,libc::F_SETFD,flags)}<0{Some(crate::errno::last())}else{None};
    drop(guard);drop(retired);drop(retired_fuse);drop(backing);
    if result>=0&&socket{super::close_effects::note_socket_close();}
    if result>=0{finish_guest_close(posix)?;}
    match error{Some(error)=>Err(error),None=>Ok(())}
}

/// Receipt-owned reserved descriptors may still be unpublished. Hidden storage
/// descriptors never belong to a Binder receipt or a guest close operation.
pub fn close_owned_guest(fd:i32,allow_reserved:bool)->Result<(),crate::errno::Errno>{
    close_owned(fd,allow_reserved,false)
}
pub(super) fn close_guest_cloexec(fd:i32)->Result<(),crate::errno::Errno>{close_owned(fd,false,true)}
fn close_owned(fd:i32,allow_reserved:bool,cloexec_only:bool)->Result<(),crate::errno::Errno>{
    let guard=lifecycle();if is_hidden(fd)||(!allow_reserved&&!visible(fd)){return Err(crate::errno::EBADF);}
    if cloexec_only{let flags=unsafe{libc::fcntl(fd,libc::F_GETFD)};if flags<0{return Err(crate::errno::last());}if flags&libc::FD_CLOEXEC==0{return Ok(());}}
    let retained=get(fd);let file=super::fuse_client::get(fd);let posix=guest_close_owner(fd)?;
    let socket=matches!(retained,Some(Kind::Sock(_)));
    if visible(fd){withdraw_guest(fd)?;}
    on_close(fd);
    let result=unsafe{libc::close(fd)};let error=if result<0{let error=crate::errno::last();if allow_reserved&&error==crate::errno::EBADF{None}else{Some(error)}}else{None};
    drop(guard);
    let flush=if error.is_none(){file.as_ref().map(|file|super::fuse_cache::flush_file(file).and_then(|_|super::fuse_client::flush(file)))}else{None};
    drop(retained);drop(file);
    if result>=0&&socket{super::close_effects::note_socket_close();}
    match error{Some(error)=>Err(error),None=>{finish_guest_close(posix)?;flush.unwrap_or(Ok(()))}}
}
pub(super) type CloseOwner=Option<(std::sync::Arc<super::posix_locks::Client>,aim_storage::inode_lease::Identity)>;
pub(super) fn guest_close_owner(fd:i32)->Result<CloseOwner,crate::errno::Errno>{
    let client=match super::posix_locks::current(){Ok(client)=>client,Err(37)=>return Ok(None),Err(error)=>return Err(error)};
    let identity=match aim_storage::inode_lease::Identity::from_fd(unsafe{std::os::fd::BorrowedFd::borrow_raw(fd)}){
        Ok(identity)=>identity,
        Err(error)if error.raw_os_error()==Some(libc::EBADF)=>return Ok(None),
        Err(error)=>return Err(crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))),
    };
    Ok(Some((client,identity)))
}
pub(super) fn finish_guest_close(owner:CloseOwner)->Result<(),crate::errno::Errno>{
    if let Some((client,identity))=owner{client.close_inode(identity)?;}Ok(())
}

pub struct ForkPrivateFd{held:Option<std::os::fd::OwnedFd>,target:i32}
impl ForkPrivateFd{
    pub fn source(&self)->std::os::fd::BorrowedFd<'_>{use std::os::fd::AsFd;self.held.as_ref().unwrap().as_fd()}
    pub fn target(&self)->i32{self.target}
}
impl Drop for ForkPrivateFd{fn drop(&mut self){
    use std::os::fd::AsRawFd;
    let close=|held:std::os::fd::OwnedFd|{let fd=held.as_raw_fd();drop(held);unhide(fd);super::fd_visibility::private_closed(fd);};
    let held=self.held.take().unwrap();
    if RETIRED.with(|owners|owners.borrow().is_some()){close(held);}else{let _guard=lifecycle();close(held);}
}}
pub(crate) fn fork_writer_receipts()->Result<Vec<ForkPrivateFd>,crate::errno::Errno>{
    let mut receipts=Vec::new();let mut targets=std::collections::HashSet::new();
    for(fd,kind)in fds_where(|kind|matches!(kind,Kind::Regular(_))){
        if !visible(fd){continue;}
        if let Kind::Regular(description)=kind{if let Some(writer)=&description.writer{use std::os::fd::AsRawFd;if targets.insert(writer.descriptor().as_raw_fd()){receipts.push(hold_fork_private(writer.descriptor())?);}}}
    }Ok(receipts)
}
thread_local!{static RESTORED_PRIVATE:std::cell::RefCell<Vec<i32>>=const{std::cell::RefCell::new(Vec::new())};}
pub(crate) fn take_restored_private_targets()->Vec<i32>{RESTORED_PRIVATE.with(|targets|std::mem::take(&mut*targets.borrow_mut()))}
pub(crate) fn close_fork_private(fd:i32)->Result<(),crate::errno::Errno>{
    fn close(fd:i32)->Result<(),crate::errno::Errno>{if visible(fd){return Err(crate::errno::EBADF);}let result=unsafe{libc::close(fd)};if result<0{return Err(crate::errno::last());}unhide(fd);super::fd_visibility::private_closed(fd);Ok(())}
    if RETIRED.with(|owners|owners.borrow().is_some()){return close(fd);}let _guard=lifecycle();close(fd)
}
pub fn hold_fork_private(fd:std::os::fd::BorrowedFd<'_>)->Result<ForkPrivateFd,crate::errno::Errno>{
    if RETIRED.with(|owners|owners.borrow().is_some()){return hold_fork_private_locked(fd);}
    let _guard=lifecycle();hold_fork_private_locked(fd)
}
fn hold_fork_private_locked(fd:std::os::fd::BorrowedFd<'_>)->Result<ForkPrivateFd,crate::errno::Errno>{
    use std::os::fd::{AsRawFd,FromRawFd};
    let target=fd.as_raw_fd();let held=unsafe{libc::fcntl(target,libc::F_DUPFD_CLOEXEC,3)};
    if held<0{return Err(crate::errno::last());}
    // spawn.h accepts only sources below OPEN_MAX; do not relocate these aliases
    // through the general high-slot private registrar.
    if held>=10240{unsafe{libc::close(held);}return Err(crate::errno::from_darwin(libc::EMFILE));}
    if let Err(error)=super::fd_visibility::hide(held){unsafe{libc::close(held);}return Err(error);}
    keep_hidden(held);Ok(ForkPrivateFd{held:Some(unsafe{std::os::fd::OwnedFd::from_raw_fd(held)}),target})
}
pub(crate) fn fork_guest_snapshot()->Result<(Vec<(i32,bool)>,Vec<ForkPrivateFd>),crate::errno::Errno>{
    let mut flags=Vec::new();let mut owners=Vec::new();
    let mut kqueues=kqueue_fds();kqueues.extend(super::wait::pidfd_fds());
    for fd in super::fd_visibility::visible(){
        let value=unsafe{libc::fcntl(fd,libc::F_GETFD)};if value<0{return Err(crate::errno::last());}
        flags.push((fd,value&libc::FD_CLOEXEC!=0));
        if !kqueues.contains(&fd){owners.push(hold_fork_private(unsafe{std::os::fd::BorrowedFd::borrow_raw(fd)})?);}
    }
    Ok((flags,owners))
}

pub type RegularExport=super::regular_file::Export;
pub fn export_regular(fd:i32)->Result<Option<RegularExport>,crate::errno::Errno>{
    let backing=pin_guest(fd)?;
    match backing.kind().cloned(){Some(Kind::Regular(description))=>super::regular_file::export(backing,description).map(Some),_=>Ok(None)}
}
pub fn install_regular(fd:i32,metadata:&aim_binder_host::wire::RegularMetadata,writer:Option<aim_binder_host::regular_file::WriterPort>)->Result<(),crate::errno::Errno>{
    let description=super::regular_file::import(fd,metadata,writer)?;
    insert(fd,Kind::Regular(description));Ok(())
}

static LIFECYCLE:Mutex<()>=Mutex::new(());
thread_local!{static RETIRED:std::cell::RefCell<Option<Vec<Kind>>>=const{std::cell::RefCell::new(None)};}
pub(crate) struct Lifecycle{guard:Option<std::sync::MutexGuard<'static,()>>}
impl Drop for Lifecycle{fn drop(&mut self){
    let retired=RETIRED.with(|owners|owners.borrow_mut().take());
    drop(self.guard.take());
    drop(retired);
}}
pub(crate) fn lifecycle()->Lifecycle{
    let guard=LIFECYCLE.lock().unwrap_or_else(|error|error.into_inner());
    RETIRED.with(|owners|{assert!(owners.borrow().is_none());*owners.borrow_mut()=Some(Vec::new());});
    Lifecycle{guard:Some(guard)}
}
fn retire(kind:Option<Kind>){
    let mut kind=kind;
    RETIRED.with(|owners|{if let Some(retired)=owners.borrow_mut().as_mut(){if let Some(owner)=kind.take(){retired.push(owner);}}});
    drop(kind);
}
struct StorageAllocation{_guard:Lifecycle}
impl aim_storage::private_fd::AllocationGuard for StorageAllocation{}
struct StorageRegistrar;
impl aim_storage::private_fd::Registrar for StorageRegistrar{
    fn enter(&self)->Box<dyn aim_storage::private_fd::AllocationGuard>{Box::new(StorageAllocation{_guard:lifecycle()})}
    fn adopt(&self,descriptor:std::os::fd::OwnedFd)->std::io::Result<std::os::fd::OwnedFd>{
        use std::os::fd::{IntoRawFd,FromRawFd};
        let fd=hide(descriptor.into_raw_fd());
        if let Err(error)=super::fd_visibility::hide(fd){unhide(fd);unsafe{libc::close(fd);}return Err(std::io::Error::from_raw_os_error(error));}
        Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(fd)})
    }
    fn closed(&self,fd:i32){unhide(fd);super::fd_visibility::private_closed(fd);}
}
pub(crate) fn install_storage_registrar()->Result<(),String>{
    static INSTALLED:OnceLock<Result<(),String>>=OnceLock::new();
    INSTALLED.get_or_init(||aim_storage::private_fd::install(Arc::new(StorageRegistrar)).map_err(|_|"storage descriptor registrar already installed".to_owned())).clone()
}
/// A stable host open description and retained Linux owner, captured together
/// under lifecycle admission. Blocking I/O never holds the lifecycle lock.
pub struct Pinned{descriptor:Option<aim_storage::private_fd::PrivateFd>,kind:Option<Kind>,socket_allowed:bool,fuse:Option<Arc<super::fuse_client::Open>>}
impl Pinned{
    pub fn descriptor(&self)->std::os::fd::BorrowedFd<'_>{use std::os::fd::AsFd;self.descriptor.as_ref().unwrap().as_fd()}
    pub fn kind(&self)->Option<&Kind>{self.kind.as_ref()}
    pub fn socket_allowed(&self)->bool{self.socket_allowed}
}
impl Drop for Pinned{fn drop(&mut self){
    use std::os::fd::AsRawFd;
    let fd=self.descriptor().as_raw_fd();
    if let Some(file)=&self.fuse{super::fuse_client::remove_private_alias(fd,file);}
    let (previous,last_socket)={let mut table=TABLE.write().unwrap();let previous=table.remove(&fd);let last=match &self.kind{Some(Kind::Sock(socket))=>!table.values().any(|kind|matches!(kind,Kind::Sock(other) if Arc::ptr_eq(socket,other))),_=>false};(previous,last)};
    retire(previous);drop(self.descriptor.take());
    // A pin can outlive the public alias and discard its actual receive queue.
    if last_socket{super::close_effects::note_socket_close();}
}}

pub fn pin_guest(fd:i32)->Result<Pinned,crate::errno::Errno>{
    use std::os::fd::FromRawFd;
    install_storage_registrar().map_err(|_|crate::errno::EIO)?;
    let mut kind=None;let mut socket_allowed=false;let mut fuse=None;
    let descriptor=aim_storage::private_fd::PrivateFd::allocate(||{
        require_guest_visible(fd).map_err(std::io::Error::from_raw_os_error)?;
        super::net::adopt(fd);
        super::dir::adopt(fd).map_err(std::io::Error::from_raw_os_error)?;
        kind=get(fd);fuse=super::fuse_client::get(fd);
        let mut stat:libc::stat=unsafe{std::mem::zeroed()};
        if unsafe{libc::fstat(fd,&mut stat)}<0{return Err(std::io::Error::last_os_error());}
        socket_allowed=stat.st_mode&libc::S_IFMT==libc::S_IFSOCK&&matches!(kind,None|Some(Kind::Sock(_)))&&!super::fuse_device::is_device(fd)&&super::fuse_client::get(fd).is_none();
        let held=unsafe{libc::fcntl(fd,libc::F_DUPFD_CLOEXEC,0)};
        if held<0{return Err(std::io::Error::last_os_error());}
        Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(held)})
    }).map_err(|error|crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?;
    {use std::os::fd::AsRawFd;if let Some(file)=&fuse{super::fuse_client::install_private_alias(descriptor.as_raw_fd(),file.clone())?;}if let Some(kind)=&kind{insert(descriptor.as_raw_fd(),kind.clone());}}
    Ok(Pinned{descriptor:Some(descriptor),kind,socket_allowed,fuse})
}

static TABLE: LazyLock<RwLock<HashMap<i32, Kind>>> = LazyLock::new(Default::default);

#[cfg(test)]
pub(super) fn isolated_kernel_test(name:&str)->bool{
    const MARKER:&str="isolated-kernel-instance";
    if std::env::args().any(|arg|arg==MARKER){return false;}
    struct Child(std::process::Child);
    impl Drop for Child{fn drop(&mut self){if self.0.try_wait().ok().flatten().is_none(){let _=self.0.kill();}let _=self.0.wait();}}
    let mut child=Child(std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact",name,"--nocapture","--skip",MARKER]).stdout(std::process::Stdio::piped()).spawn().unwrap());
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(20);
    let status=loop{if let Some(status)=child.0.try_wait().unwrap(){break status;}assert!(std::time::Instant::now()<deadline,"kernel fixture timed out: {name}");std::thread::sleep(std::time::Duration::from_millis(10));};
    use std::io::Read;let mut output=String::new();child.0.stdout.take().unwrap().read_to_string(&mut output).unwrap();
    assert!(status.success(),"{name}: {output}");assert!(output.contains("1 passed"),"kernel fixture did not execute: {output}");true
}

fn set_slow(fd: i32, on: bool) {
    if let Some(b) = SLOW.get(fd as usize) {
        b.store(on as u8, Ordering::Relaxed);
    }
}

/// A producer publishes only after the complete open description is installed.
pub fn publish_guest(fd:i32)->Result<(),crate::errno::Errno>{
    if is_hidden(fd)||unsafe{libc::fcntl(fd,libc::F_GETFD)}<0{return Err(crate::errno::EBADF);}
    let typed=get(fd).is_some()||super::fuse_device::is_device(fd)||super::fuse_client::get(fd).is_some()||super::fd_visibility::role(fd)==super::fd_visibility::Role::Typed;
    super::fd_visibility::publish(fd,typed)?;
    set_slow(fd,typed);Ok(())
}
pub fn publish_typed_guest(fd:i32)->Result<(),crate::errno::Errno>{
    if is_hidden(fd)||unsafe{libc::fcntl(fd,libc::F_GETFD)}<0{return Err(crate::errno::EBADF);}
    super::fd_visibility::publish(fd,true)?;set_slow(fd,true);Ok(())
}
pub fn visible(fd:i32)->bool{super::fd_visibility::require(fd).is_ok()}
pub fn require_guest_visible(fd:i32)->Result<(),crate::errno::Errno>{super::fd_visibility::require(fd)}
pub fn withdraw_guest(fd:i32)->Result<(),crate::errno::Errno>{set_slow(fd,true);super::fd_visibility::withdraw(fd).map(|_|())}

pub fn insert(fd: i32, kind: Kind) {
    let previous = TABLE.write().unwrap().insert(fd, kind);
    set_slow(fd, true);
    retire(previous);
}

pub fn get(fd: i32) -> Option<Kind> {
    if SLOW
        .get(fd as usize)
        .is_some_and(|b| b.load(Ordering::Relaxed) == 0)
    {
        return None;
    }
    TABLE.read().unwrap().get(&fd).cloned()
}

/// The guest closed `fd` (or is about to replace it with dup2).
pub fn on_close(fd: i32) {
    super::fuse_client::close(fd);
    if super::fuse_device::is_device(fd){set_slow(fd,false);}
    match get(fd) {
        None => return,
        Some(Kind::Content | Kind::Knob(_)) => super::procfs::recycle(fd),
        Some(_) => {}
    }
    set_slow(fd, true);
    let previous = TABLE.write().unwrap().remove(&fd);
    retire(previous);
}

/// `new` now refers to the same open file as `old`.
pub fn on_dup(old: i32, new: i32) {
    on_close(new);
    super::fuse_client::dup(old,new);
    if super::fuse_device::is_device(old){super::fuse_device::adopt(new);}
    if let Some(k) = get(old) {
        insert(new, k);
    }
}

/// Recognize an fd that arrived from elsewhere (exec, `SCM_RIGHTS`,
/// binder): a socket or a memfd gets its Linux state.
pub fn adopt(fd: i32) {
    if super::fuse_device::adopt(fd){return;}
    if super::fuse_client::adopt(fd)==Ok(true){return;}
    if super::binder::file_class(fd) == Ok(aim_binder_host::proxy_file::CLASS) {
        insert(fd, Kind::ProxyFile);
        return;
    }
    if super::binder::file_class(fd) == Ok(aim_binder_host::path_file::CLASS) {
        if let Err(error) = install_path(fd) { on_close(fd); unsafe { libc::close(fd); } eprintln!("path descriptor adoption failed: errno={error}"); }
        return;
    }
    adopt_untyped(fd);
}

pub(super) fn adopt_received(fd: i32) -> Result<(), i32> {
    if super::fuse_device::adopt(fd){return Ok(());}
    if super::fuse_client::adopt(fd)?{return Ok(());}
    match super::binder::file_class(fd)? {
        0 => adopt_untyped(fd),
        aim_binder_host::proxy_file::CLASS => insert(fd, Kind::ProxyFile),
        aim_binder_host::path_file::CLASS => install_path(fd)?,
        _ => return Err(71),
    }
    Ok(())
}

pub(super) fn refresh_capabilities() -> Result<(), i32> {
    refresh_visible_capability_snapshot(super::fd_visibility::visible(),super::binder::file_class)
}
fn refresh_visible_capability_snapshot(fds:Vec<i32>,mut classify:impl FnMut(i32)->Result<u32,i32>)->Result<(),i32>{
    use std::os::fd::AsRawFd;
    for fd in fds{
        let mut held=match pin_guest(fd){Ok(held)=>held,Err(crate::errno::EBADF)=>continue,Err(error)=>return Err(error)};
        if classify(held.descriptor().as_raw_fd())?!=aim_binder_host::proxy_file::CLASS{continue;}
        held.kind=Some(Kind::ProxyFile);held.socket_allowed=false;
        let _guard=lifecycle();
        if !visible(fd){continue;}
        if !aim_binder_host::proxy_file::same_endpoint(fd,held.descriptor().as_raw_fd()).map_err(|error|crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO)))?{continue;}
        insert(fd,Kind::ProxyFile);publish_guest(fd)?;
    }
    Ok(())
}
#[cfg(test)]
fn refresh_capability_snapshot(
    fds: Vec<i32>,
    mut classify: impl FnMut(i32) -> Result<u32, i32>,
) -> Result<(), i32> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    let pin = |fd| -> Result<Option<OwnedFd>, i32> {
        let held = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
        if held >= 0 {
            return Ok(Some(unsafe { OwnedFd::from_raw_fd(held) }));
        }
        let error = std::io::Error::last_os_error()
            .raw_os_error()
            .unwrap_or(libc::EIO);
        if error == libc::EBADF {
            Ok(None)
        } else {
            Err(error)
        }
    };
    for fd in fds {
        let Some(held) = pin(fd)? else {
            continue;
        };
        if classify(held.as_raw_fd())? != aim_binder_host::proxy_file::CLASS {
            continue;
        }
        // Serialize publication with managed FD-table replacement, then pin the
        // current slot again: classification of its earlier occupant grants no
        // capability to a reused numeric descriptor.
        let mut table = TABLE.write().unwrap();
        let Some(current) = pin(fd)? else {
            continue;
        };
        if !aim_binder_host::proxy_file::same_endpoint(current.as_raw_fd(), held.as_raw_fd())
            .map_err(|error| error.raw_os_error().unwrap_or(libc::EIO))?
        {
            continue;
        }
        let old = table.insert(fd, Kind::ProxyFile);
        set_slow(fd, true);
        drop(table);
        drop(old);
    }
    Ok(())
}

pub(super) fn adopt_untyped(fd: i32) {
    if super::fuse_device::adopt(fd){return;}
    if super::fuse_client::adopt(fd)==Ok(true){return;}
    net::adopt(fd);
    memfd::adopt(fd);
    random::adopt(fd);
}

/// Give the fds inherited across exec their Linux state.
fn adopt_inherited() {
    for fd in super::fd_visibility::visible() { adopt(fd); }
}

/// execve in place: the kept fds this process holds as plain fds (a fork
/// child's evdev fds, say) get their Linux state, as in a new process.
pub fn adopt_plain() {
    for fd in open_fds()
        .into_iter()
        .filter(|&fd| get(fd).is_none() && !is_hidden(fd))
    {
        adopt(fd);
    }
}

/// The open fds of this process.
pub fn open_fds() -> Vec<i32> {
    // SAFETY: sizing call, then a buffer of that size.
    unsafe {
        let pid = libc::getpid();
        let n = libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0);
        if n <= 0 {
            return Vec::new();
        }
        let sz = std::mem::size_of::<libc::proc_fdinfo>();
        let mut v: Vec<libc::proc_fdinfo> = Vec::with_capacity(n as usize / sz + 16);
        let n = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDLISTFDS,
            0,
            v.as_mut_ptr().cast(),
            (v.capacity() * sz) as i32,
        );
        if n <= 0 {
            return Vec::new();
        }
        v.set_len(n as usize / sz);
        v.iter().map(|f| f.proc_fd).collect()
    }
}

/// Link text for `/proc/self/fd/N` of an fd with no path.
pub fn anon_name(fd: i32) -> Option<String> {
    Some(
        match get(fd)? {
            Kind::Regular(_) => return None,
            Kind::Event(_) => "anon_inode:[eventfd]",
            Kind::Timer(_) => "anon_inode:[timerfd]",
            Kind::Epoll(_) => "anon_inode:[eventpoll]",
            Kind::Inotify(_) => "anon_inode:inotify",
            Kind::SyncFile => "anon_inode:sync_file",
            Kind::Evdev(e) => return Some(e.path()),
            Kind::Sock(_)
            | Kind::Dir(_)
            | Kind::Memfd(_)
            | Kind::Path(_)
            | Kind::Content
            | Kind::Knob(_)
            | Kind::Random
            | Kind::ProxyFile
            | Kind::Binder(_) => {
                return None;
            }
        }
        .to_string(),
    )
}

/// The fds of epolls and inotifies: kqueues, which a fork child makes
/// again on the same numbers.
pub fn kqueue_fds() -> Vec<i32> {
    fds_where(|k| matches!(k, Kind::Epoll(_) | Kind::Inotify(_)))
        .into_iter().filter(|(fd,_)|visible(*fd))
        .map(|(fd, _)| fd)
        .collect()
}

/// In a fork child, once the table is restored and before guest code
/// runs: kqueues are not inherited and the child has no threads, so epoll
/// and inotify fds are rebuilt on their numbers (held by placeholders until
/// now) and the timerfd thread restarted.
pub fn after_fork_child() {
    super::display::init();
    super::host_descriptors::init();
    epoll::after_fork_child();
    inotify::after_fork_child();
    event::after_fork_child();
}

pub(crate) fn regular_exec_text()->String{
    TABLE.read().unwrap().iter().filter_map(|(fd,kind)|{
        let Kind::Regular(description)=kind else{return None};
        let flags=unsafe{libc::fcntl(*fd,libc::F_GETFD)};
        if !visible(*fd)||flags<0||flags&libc::FD_CLOEXEC!=0{return None;}
        use std::os::fd::AsRawFd;
        Some(format!("{}\t{}\t{}",fd,description.flags,description.writer.as_ref().map(|writer|writer.descriptor().as_raw_fd()).unwrap_or(-1)))
    }).collect::<Vec<_>>().join("\n")
}
pub(crate) struct ExecWriterFlags(Vec<(std::sync::Arc<super::regular_file::Description>,i32)>);
impl Drop for ExecWriterFlags{
 fn drop(&mut self){for(description,flags)in &self.0{use std::os::fd::AsRawFd;if let Some(writer)=&description.writer{unsafe{libc::fcntl(writer.descriptor().as_raw_fd(),libc::F_SETFD,*flags);}}}}
}
pub(crate) fn prepare_regular_exec()->Result<ExecWriterFlags,crate::errno::Errno>{
    use std::os::fd::AsRawFd;
    let mut changed=ExecWriterFlags(Vec::new());
    let mut writers=std::collections::HashSet::new();
    for(guest,kind)in fds_where(|kind|matches!(kind,Kind::Regular(_))){
        if !visible(guest){continue;}
        let guest_flags=unsafe{libc::fcntl(guest,libc::F_GETFD)};
        if guest_flags<0{return Err(crate::errno::last());}
        if guest_flags&libc::FD_CLOEXEC!=0{continue;}
        let Kind::Regular(description)=kind else{continue};
        if let Some(writer)=&description.writer{let fd=writer.descriptor().as_raw_fd();if !writers.insert(fd){continue;}let flags=unsafe{libc::fcntl(fd,libc::F_GETFD)};if flags<0{return Err(crate::errno::last());}
            if unsafe{libc::fcntl(fd,libc::F_SETFD,flags&!libc::FD_CLOEXEC)}<0{return Err(crate::errno::last());}
            // Retain the original descriptor, not a dup: FD_CLOEXEC is local.
            changed.0.push((description,flags));
        }
    }Ok(changed)
}
pub(crate) fn restore_regular_exec(text:&str)->Result<(),crate::errno::Errno>{
    let mut inherited=Vec::new();
    for line in text.lines(){
        let fields=line.split('\t').collect::<Vec<_>>();if fields.len()!=3{return Err(crate::errno::EINVAL);}
        let fd=fields[0].parse::<i32>().map_err(|_|crate::errno::EINVAL)?;
        let flags=fields[1].parse::<u64>().map_err(|_|crate::errno::EINVAL)?;
        let writer=fields[2].parse::<i32>().map_err(|_|crate::errno::EINVAL)?;
        require_guest_visible(fd)?;
        let description=super::regular_file::restore(fd,flags,(writer>=0).then_some(writer))?;
        insert(fd,Kind::Regular(description));publish_guest(fd)?;
        if writer>=0&&!inherited.contains(&writer){inherited.push(writer);}
    }
    for fd in inherited{let _guard=lifecycle();if unsafe{libc::close(fd)}<0{return Err(crate::errno::last());}unhide(fd);super::fd_visibility::private_closed(fd);}
    Ok(())
}

pub fn parse_socket_receipts(text:&str)->Result<Vec<(i32,aim_storage::socket_inode::Receipt)>,crate::errno::Errno>{
    let mut receipts=Vec::new();if text.is_empty(){return Ok(receipts);}
    for record in text.split(','){
        let(fd,hex)=record.split_once(':').ok_or(crate::errno::EINVAL)?;
        let fds=parse_guest_fds(fd)?;if fds.len()!=1||receipts.iter().any(|(old,_)|*old==fds[0])||hex.len()!=80||!hex.is_ascii(){return Err(crate::errno::EINVAL);}
        let mut bytes=[0;40];for(index,byte)in bytes.iter_mut().enumerate(){*byte=u8::from_str_radix(&hex[index*2..index*2+2],16).map_err(|_|crate::errno::EINVAL)?;}
        let receipt=aim_storage::socket_inode::Receipt::from_bytes(&bytes).map_err(|error|crate::errno::from_darwin(error.raw_os_error().unwrap_or(libc::EINVAL)))?;
        receipts.push((fds[0],receipt));
    }Ok(receipts)
}
pub(crate) fn socket_exec_text()->String{
    fds_where(|kind|matches!(kind,Kind::Sock(_))).into_iter().filter_map(|(fd,kind)|{
        let Kind::Sock(socket)=kind else{return None};let flags=unsafe{libc::fcntl(fd,libc::F_GETFD)};
        if !visible(fd)||flags<0||flags&libc::FD_CLOEXEC!=0{return None;}
        let receipt=(*socket.inode_allocation.lock().unwrap())?;
        Some(format!("{fd}:{}",receipt.to_bytes().iter().map(|byte|format!("{byte:02x}")).collect::<String>()))
    }).collect::<Vec<_>>().join(",")
}
pub fn parse_guest_fds(text:&str)->Result<Vec<i32>,crate::errno::Errno>{
    let mut fds=Vec::new();if text.is_empty(){return Ok(fds);}
    for part in text.split(','){
        if part.is_empty()||!part.bytes().all(|byte|byte.is_ascii_digit()){return Err(crate::errno::EBADF);}
        let fd=part.parse::<i32>().map_err(|_|crate::errno::EBADF)?;
        if fds.contains(&fd){return Err(crate::errno::EBADF);}fds.push(fd);
    }
    Ok(fds)
}
pub(crate) fn bootstrap(inherited:Vec<i32>,receipts:&[(i32,aim_storage::socket_inode::Receipt)])->Result<(),crate::errno::Errno>{
    if receipts.iter().any(|(fd,_)|!inherited.contains(fd)){return Err(crate::errno::EINVAL);}
    for fd in inherited{
        if unsafe{libc::fcntl(fd,libc::F_GETFD)}<0{return Err(crate::errno::EBADF);}
        adopt(fd);super::dir::adopt(fd)?;
        if let Some((_,receipt))=receipts.iter().find(|(target,_)|*target==fd){super::net::install_socket_receipt(fd,*receipt)?;}
        let _guard=lifecycle();publish_guest(fd)?;
    }
    Ok(())
}

/// Set up the table for this process: recognize inherited fds.
pub fn init() {
    super::display::init();
    super::host_descriptors::init();
    sync_file::init();
    adopt_inherited();
}

/// Every fd in the table whose kind matches `f`.
pub fn fds_where(f: impl Fn(&Kind) -> bool) -> Vec<(i32, Kind)> {
    TABLE
        .read()
        .unwrap()
        .iter()
        .filter(|(_, k)| f(k))
        .map(|(fd, k)| (*fd, k.clone()))
        .collect()
}

/// Whether the open file behind `fd` is in non-blocking mode.
pub fn nonblocking(fd: i32) -> bool {
    // SAFETY: plain fcntl.
    unsafe { libc::fcntl(fd, libc::F_GETFL) & libc::O_NONBLOCK != 0 }
}

/// Set O_NONBLOCK and FD_CLOEXEC on a host fd as requested.
pub fn set_flags(fd: i32, nonblock: bool, cloexec: bool) {
    // SAFETY: plain fcntl on our fd.
    unsafe {
        if nonblock {
            let fl = libc::fcntl(fd, libc::F_GETFL);
            libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
        }
        if cloexec {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
    }
}

/// Block until `fd` has one of `events` (host poll bits). Returns 0, or a
/// negative Linux errno (EINTR when a signal arrived).
pub fn wait_for(fd: i32, events: i16) -> i64 {
    let mut p = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    // SAFETY: one pollfd on our stack.
    if unsafe { libc::poll(&mut p, 1, -1) } < 0 {
        return -(crate::errno::last() as i64);
    }
    0
}

static HIDDEN: Mutex<Vec<i32>> = Mutex::new(Vec::new());

/// Move a descriptor the layer keeps for itself (an eventfd's peer end)
/// high up, out of the guest's way, and leave it out of `/proc/self/fd`.
pub fn hide(fd: i32) -> i32 {
    let base = hidden_base();
    // SAFETY: duplicating our own fd, then closing the original.
    let high = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, base) };
    let fd = if high >= 0 {
        unsafe { libc::close(fd) };
        high
    } else {
        fd
    };
    HIDDEN.lock().unwrap().push(fd);
    set_slow(fd,true);
    fd
}

/// Where the layer's own fds go: high up, but below `OPEN_MAX` (10240),
/// the highest fd `posix_spawn` can pass to a fork child
/// (`posix_spawn_file_actions_addinherit_np` refuses the rest with EBADF),
/// and within the descriptor table: F_DUPFD fails above it, as it does at
/// 3/4 of a soft RLIMIT_NOFILE beyond kern.maxfilesperproc.
pub fn hidden_base() -> i32 {
    // <sys/syslimits.h>.
    const OPEN_MAX: i32 = 10240;
    // SAFETY: plain getdtablesize.
    let table = unsafe { libc::getdtablesize() };
    (table.min(OPEN_MAX) * 3 / 4).max(64)
}

/// Leave `fd` (the layer's, kept across exec) out of the guest's view.
pub fn keep_hidden(fd: i32) {
    HIDDEN.lock().unwrap().push(fd);
    set_slow(fd,true);
}

pub fn unhide(fd: i32) {
    HIDDEN.lock().unwrap().retain(|&h| h != fd);
    if !TABLE.read().unwrap().contains_key(&fd) { set_slow(fd,false); }
}

pub fn is_hidden(fd: i32) -> bool {
    HIDDEN.lock().unwrap().contains(&fd)
}

/// Fork: every fd's kind, with fds that share an object (dup'ed ones)
/// sharing it again in the child, and the layer's hidden fds. The fds
/// themselves are inherited. Content, knob, evdev and binder fds stay plain
/// fds: a content file is the parent's to reuse, a knob's action is code,
/// an input device is opened again, and a binder file is the parent's
/// process of the driver.
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    let table = TABLE.read().unwrap();
    let mut objects: Vec<*const ()> = Vec::new();
    let mut entries: Vec<(i32, &Kind, usize, bool)> = Vec::new();
    for (fd, k) in table.iter().filter(|(fd,_)|visible(**fd)) {
        let p = match k {
            Kind::Regular(a) => Arc::as_ptr(a) as *const (),
            Kind::Event(a) => Arc::as_ptr(a) as *const (),
            Kind::Timer(a) => Arc::as_ptr(a) as *const (),
            Kind::Sock(a) => Arc::as_ptr(a) as *const (),
            Kind::Epoll(a) => Arc::as_ptr(a) as *const (),
            Kind::Inotify(a) => Arc::as_ptr(a) as *const (),
            Kind::Dir(a) => Arc::as_ptr(a) as *const (),
            Kind::Memfd(_) | Kind::SyncFile | Kind::Random | Kind::ProxyFile | Kind::Path(_) => std::ptr::null(),
            Kind::Content | Kind::Knob(_) | Kind::Evdev(_) | Kind::Binder(_) => continue,
        };
        let (i, new) = match objects.iter().position(|&o| !p.is_null() && o == p) {
            Some(i) => (i, false),
            None => {
                objects.push(p);
                (objects.len() - 1, true)
            }
        };
        entries.push((*fd, k, i, new));
    }
    // Objects first seen at a lower index come first in the child too.
    entries.sort_by_key(|e| (e.2, !e.3));
    w.seq(entries.into_iter(), |w, (fd, k, i, new)| {
        w.i32(fd);
        w.u64(i as u64);
        w.bool(new);
        if !new {
            return;
        }
        match k {
            Kind::Regular(description) => {w.u32(11);w.u64(description.flags);w.opt(description.writer.as_ref(),|w,writer|{use std::os::fd::AsRawFd;w.i32(writer.descriptor().as_raw_fd());});},
            Kind::Event(e) => {
                w.u32(0);
                event::save_event(e, w);
            }
            Kind::Timer(t) => {
                w.u32(1);
                event::save_timer(t, w);
            }
            Kind::Sock(s) => {
                w.u32(2);
                net::save_sock(s, w);
            }
            Kind::Epoll(e) => {
                w.u32(3);
                epoll::save(e, w);
            }
            Kind::Inotify(i) => {
                w.u32(4);
                inotify::save(i, w);
            }
            Kind::Dir(d) => {
                w.u32(5);
                super::dir::save(&d.lock().unwrap(), w);
            }
            Kind::Memfd(k) => {
                w.u32(6);
                w.u64(k.0);
                w.u64(k.1);
            }
            Kind::SyncFile => w.u32(7),
            Kind::Random => w.u32(8),
            Kind::ProxyFile => w.u32(9),
            Kind::Path(path) => { w.u32(10); w.u64(path.flags); },
            Kind::Content | Kind::Knob(_) | Kind::Evdev(_) | Kind::Binder(_) => unreachable!(),
        }
    });
    // Private targets are installed from the explicit fork receipt before the
    // runtime starts. Parent-only guards and transient pins are not inherited.
    w.seq(std::iter::empty::<i32>(),|w,fd|w.i32(fd));
    w.seq(super::fd_visibility::visible().into_iter(),|w,fd|w.i32(fd));
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    let mut objects: Vec<Kind> = Vec::new();let mut inherited_writers=Vec::new();
    let entries = r.seq(|r| {
        let fd = r.i32();
        let i = r.u64() as usize;
        if !r.bool() {
            return (fd, objects.get(i).cloned());
        }
        let k = match r.u32() {
            0 => Kind::Event(event::load_event(r)),
            1 => Kind::Timer(event::load_timer(r)),
            2 => Kind::Sock(net::load_sock(r)),
            3 => Kind::Epoll(epoll::load(r)),
            4 => Kind::Inotify(inotify::load(r)),
            5 => Kind::Dir(Arc::new(Mutex::new(super::dir::load(r)))),
            7 => Kind::SyncFile,
            8 => Kind::Random,
            9 => Kind::ProxyFile,
            10 => Kind::Path(PathDescription::new(r.u64())),
            11 => {let flags=r.u64();let writer=r.opt(|r|r.i32());if let Some(fd)=writer{if !inherited_writers.contains(&fd){inherited_writers.push(fd);}}match super::regular_file::restore(fd,flags,writer){Ok(description)=>Kind::Regular(description),Err(error)=>{eprintln!("regular descriptor fork restore: errno {error}");r.invalidate();return (fd,None);}}},
            _ => Kind::Memfd((r.u64(), r.u64())),
        };
        objects.push(k.clone());
        (fd, Some(k))
    });
    for (fd, k) in entries {
        if let Some(k) = k {
            insert(fd, k);
        }
    }
    keep_inherited_hidden(r.seq(|r| r.i32()));
    RESTORED_PRIVATE.with(|targets|targets.borrow_mut().extend(inherited_writers));
    let visible=r.seq(|r|r.i32());
    for fd in visible{if let Err(error)=publish_guest(fd){eprintln!("fork descriptor publication: errno {error}");r.invalidate();}}
    for fd in super::fd_visibility::visible(){if super::fuse_device::adopt(fd){continue;}if !is_hidden(fd)&&super::fuse::marker(fd)==Some(super::fuse::FILE_MARKER){if let Err(error)=super::fuse_client::adopt(fd){set_slow(fd,true);eprintln!("inherited FUSE descriptor adoption failed: {error}");}}}
}

/// Fork child: hide the parent's hidden fds that came along, beside this
/// process's own. One that did not (a kqueue, such as the timer kqueue)
/// is a free number the guest may get.
fn keep_inherited_hidden(parent: Vec<i32>) {
    let mut hidden = HIDDEN.lock().unwrap();
    for fd in parent {
        // SAFETY: plain fcntl on a possibly closed fd.
        if !hidden.contains(&fd) && unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0 {
            hidden.push(fd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" {
        fn posix_spawn_file_actions_addinherit_np(
            actions: *mut libc::posix_spawn_file_actions_t,
            fd: libc::c_int,
        ) -> libc::c_int;
    }

    /// A hidden fd goes high even when the soft RLIMIT_NOFILE is above
    /// what the descriptor table can hold, and stays where `posix_spawn`
    /// can pass it to a fork child.
    #[test]
    fn a_hidden_fd_goes_high_and_stays_inheritable() {
        // SAFETY: raising this test process's soft limit to its hard one.
        unsafe {
            let mut lim: libc::rlimit = std::mem::zeroed();
            libc::getrlimit(libc::RLIMIT_NOFILE, &mut lim);
            lim.rlim_cur = lim.rlim_max;
            libc::setrlimit(libc::RLIMIT_NOFILE, &lim);
        }
        // SAFETY: a new kqueue of our own.
        let kq = unsafe { libc::kqueue() };
        let fd = hide(kq);
        assert!(fd >= hidden_base() && fd < 10240, "{fd}");
        // SAFETY: a file action list of our own, destroyed here.
        let r = unsafe {
            let mut actions: libc::posix_spawn_file_actions_t = std::ptr::null_mut();
            libc::posix_spawn_file_actions_init(&mut actions);
            let r = posix_spawn_file_actions_addinherit_np(&mut actions, fd);
            libc::posix_spawn_file_actions_destroy(&mut actions);
            r
        };
        assert_eq!(r, 0, "fd {fd} cannot be passed to a fork child");
        unhide(fd);
        // SAFETY: our fd.
        unsafe { libc::close(fd) };
    }

    /// A fork child hides the parent's hidden fds it inherited, not the
    /// numbers of those it did not, and keeps its own.
    #[test]
    fn a_fork_child_hides_only_inherited_fds() {
        // SAFETY: a new pipe of our own.
        let (own, inherited) = unsafe {
            let mut p = [0; 2];
            libc::pipe(p.as_mut_ptr());
            (p[0], p[1])
        };
        // Not open here, as a parent's kqueue is not in the child: a number
        // above any descriptor table, which no other test's fd can take.
        let gone = i32::MAX;
        keep_hidden(own);
        keep_inherited_hidden(vec![inherited, gone]);
        assert!(is_hidden(own) && is_hidden(inherited) && !is_hidden(gone));
        for fd in [own, inherited] {
            unhide(fd);
            // SAFETY: our fds.
            unsafe { libc::close(fd) };
        }
    }
    #[test]
    fn capability_refresh_pins_snapshot_and_rejects_fd_number_replacement() {
        use std::os::fd::{AsRawFd, IntoRawFd};
        let path =
            std::env::temp_dir().join(format!("aim-capability-refresh-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let (owner, capability, worker) = aim_binder_host::proxy_file::open(file).unwrap();
        let slot = capability.into_raw_fd();
        let replacement = std::fs::File::open("/dev/null").unwrap();
        let (start, ready) = std::sync::mpsc::channel();
        let (done, finished) = std::sync::mpsc::channel();
        let replacing = std::thread::spawn(move || {
            ready.recv().unwrap();
            on_close(slot);
            // dup2 closes/replaces atomically: another parallel test cannot take
            // a temporarily vacant slot belonging to this test.
            assert_eq!(unsafe { libc::dup2(replacement.as_raw_fd(), slot) }, slot);
            done.send(()).unwrap();
        });
        refresh_capability_snapshot(vec![slot], |held| {
            let class = aim_binder_host::proxy_file::registered_class_result(held)
                .map_err(|e| e.raw_os_error().unwrap())
                .unwrap();
            assert_eq!(class, aim_binder_host::proxy_file::CLASS);
            start.send(()).unwrap();
            finished.recv().unwrap();
            Ok(class)
        })
        .unwrap();
        replacing.join().unwrap();
        assert!(get(slot).is_none());
        on_close(slot);
        assert_eq!(unsafe { libc::close(slot) }, 0);
        // Use an impossible descriptor as well: normal enumeration loss is
        // skipped before registry RPC, not converted into a false registry result.
        refresh_capability_snapshot(vec![i32::MAX], |_| {
            panic!("closed snapshot entry classified")
        })
        .unwrap();
        let held = std::fs::File::open("/dev/null").unwrap();
        assert_eq!(
            refresh_capability_snapshot(vec![held.as_raw_fd()], |_| Err(libc::EBADF)),
            Err(libc::EBADF)
        );
        assert_eq!(
            refresh_capability_snapshot(vec![held.as_raw_fd()], |_| Err(libc::EPROTO)),
            Err(libc::EPROTO)
        );
        owner.revoke();
        drop(worker);
        std::fs::remove_file(path).unwrap();
    }
}

#[cfg(test)]
mod publication_tests{
 use super::*;
 use std::{fs::File,io::Write,os::{fd::{AsFd,AsRawFd},unix::process::CommandExt},process::{Command,Stdio}};
 struct Child(std::process::Child);impl Drop for Child{fn drop(&mut self){if self.0.try_wait().ok().flatten().is_none(){let _=self.0.kill();}let _=self.0.wait();}}
 #[test]
 fn exec_writer_carriers_follow_guest_cloexec_and_restore_failure_flags(){
  let(_view,root)=crate::vfs::test_view();install_storage_registrar().unwrap();
  let name=std::ffi::CString::new("/data/exec-writer-flags").unwrap();
  std::fs::write(root.join("data/exec-writer-flags"),b"writer").unwrap();
  let fd=super::super::fs::openat([crate::vfs::LINUX_AT_FDCWD as u64,name.as_ptr()as u64,2,0,0,0])as i32;assert!(fd>=0);
  let Some(Kind::Regular(owner))=get(fd)else{panic!("regular owner missing")};
  let writer=owner.writer.as_ref().unwrap().descriptor().as_raw_fd();
  let original=unsafe{libc::fcntl(writer,libc::F_GETFD)};assert_ne!(original&libc::FD_CLOEXEC,0);
  assert_eq!(super::super::fs::fcntl([fd as u64,2,libc::FD_CLOEXEC as u64,0,0,0]),0);
  let guard=prepare_regular_exec().unwrap();assert_ne!(unsafe{libc::fcntl(writer,libc::F_GETFD)}&libc::FD_CLOEXEC,0);drop(guard);
  assert_eq!(super::super::fs::fcntl([fd as u64,2,0,0,0,0]),0);
  let duplicate=super::super::fs::dup([fd as u64,0,0,0,0,0]);assert!(duplicate>=0);
  let guard=prepare_regular_exec().unwrap();assert_eq!(unsafe{libc::fcntl(writer,libc::F_GETFD)}&libc::FD_CLOEXEC,0);drop(guard);
  assert_eq!(unsafe{libc::fcntl(writer,libc::F_GETFD)},original);
  assert_eq!(super::super::fs::close([duplicate as u64,0,0,0,0,0]),0);assert_eq!(super::super::fs::close([fd as u64,0,0,0,0,0]),0);
 }
 #[test]
 #[ignore="real child exercised by explicit_launcher_inventory_excludes_private_slots_and_pins_reused_fd"]
 fn inherited_child(){
  let mut line=String::new();std::io::stdin().read_line(&mut line).unwrap();let extra=line.trim().parse::<i32>().unwrap();
  let private=File::open("/dev/null").unwrap();let private_fd=private.as_raw_fd();
  install_storage_registrar().unwrap();bootstrap(vec![0,1,2,extra],&[]).unwrap();
  assert!(visible(extra));assert!(!visible(private_fd));
  let mut bytes=[0;32];assert_eq!(super::super::fs::read([private_fd as u64,bytes.as_mut_ptr()as u64,1,0,0,0]),-9);
  let duplicate=super::super::fs::dup([extra as u64,0,0,0,0,0]);assert!(duplicate>=0&&visible(duplicate as i32));
  let pin=pin_guest(extra).unwrap();assert!(is_hidden(pin.descriptor().as_raw_fd()));assert!(!visible(pin.descriptor().as_raw_fd()));
  assert_eq!(super::super::fs::close([extra as u64,0,0,0,0,0]),0);assert!(!visible(extra));
  let replacement=File::open("/dev/zero").unwrap();assert_eq!(unsafe{libc::dup2(replacement.as_raw_fd(),extra)},extra);
  assert_eq!(super::super::fs::read([extra as u64,bytes.as_mut_ptr()as u64,1,0,0,0]),-9);
  let n=unsafe{libc::read(pin.descriptor().as_raw_fd(),bytes.as_mut_ptr().cast(),bytes.len())};assert_eq!(&bytes[..n as usize],b"actual inherited bytes");
  assert_eq!(super::super::fs::close([duplicate as u64,0,0,0,0,0]),0);
  if replacement.as_raw_fd()!=extra{unsafe{libc::close(extra);}}
 }
 #[test]
 fn explicit_launcher_inventory_excludes_private_slots_and_pins_reused_fd(){
  let path=std::env::temp_dir().join(format!("aim-fd-receipt-{}",std::process::id()));std::fs::write(&path,b"actual inherited bytes").unwrap();let file=File::open(&path).unwrap();let fd=file.as_raw_fd();
  let mut command=Command::new(std::env::current_exe().unwrap());command.args(["--exact","sys::fdtab::publication_tests::inherited_child","--ignored","--nocapture"]).stdin(Stdio::piped());
  unsafe{command.pre_exec(move||{if libc::fcntl(fd,libc::F_SETFD,0)<0{Err(std::io::Error::last_os_error())}else{Ok(())}});}
  let mut child=Child(command.spawn().unwrap());writeln!(child.0.stdin.take().unwrap(),"{fd}").unwrap();assert!(child.0.wait().unwrap().success());std::fs::remove_file(path).unwrap();
 }
 #[test]
 fn inherited_inventory_parser_rejects_ambiguous_descriptor_claims(){
  assert_eq!(parse_guest_fds("0,1,2,17").unwrap(),[0,1,2,17]);
  for invalid in ["-1","0,0","0,,2","2147483648"," 2","2x"]{assert_eq!(parse_guest_fds(invalid).unwrap_err(),crate::errno::EBADF);}
 }
}
