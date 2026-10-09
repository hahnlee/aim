//! Linux PTY descriptions backed by the native pair keeper (#1259).
use crate::errno::{self, Errno};
use aim_storage::{
    private_fd::PrivateFd,
    pty_owner::{
        Side,
        transport::{Client, Observation, OwnerConfig, RemoteEndpoint},
    },
};
use std::{
    io,
    os::fd::{AsRawFd, BorrowedFd},
    sync::Arc,
};

fn failure(error: io::Error) -> Errno {
    errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))
}
pub struct Description {
    client: Arc<Client>,
    endpoint: RemoteEndpoint,
}
pub(super) struct Watch {
    pub observation: Observation,
    pub side: Side,
}
impl Watch {
    pub fn save(&self,w:&mut super::fork_state::Writer){
        w.bool(self.side==Side::Slave);
        for fd in [self.observation.data(),self.observation.status.descriptor()]{let fd=fd.as_raw_fd();w.retain_private(fd);w.i32(fd);}
        match self.observation.notification(){Some(fd)=>{let fd=fd.as_raw_fd();w.retain_private(fd);w.i32(fd);},None=>{w.error(errno::EIO);w.i32(-1);}}
    }
    pub fn restore(r:&mut super::fork_state::Reader)->Result<Self,Errno>{
        let side=if r.bool(){Side::Slave}else{Side::Master};
        let fds=[r.i32(),r.i32(),r.i32()];
        let data=duplicate(fds[0])?;
        let status=aim_storage::pty_owner::status::ReadOnly::adopt(duplicate(fds[1])?).map_err(failure)?;
        let notification=duplicate(fds[2])?;
        let observation=Observation::from_parts(data,status,notification).map_err(failure)?;
        super::fdtab::remember_restored_private(&fds);
        Ok(Self{observation,side})
    }
    pub fn events(&self, native:i16)->i16{
        if self.observation.status.retired(){return libc::POLLNVAL;}
        if !self.observation.status.hangup(){return native;}
        native|libc::POLLHUP|if self.side==Side::Slave{libc::POLLIN|libc::POLLOUT|libc::POLLERR}else{0}
    }
}
fn duplicate(fd:i32)->Result<PrivateFd,Errno>{
    PrivateFd::allocate(||{
        use std::os::fd::{FromRawFd,OwnedFd};
        let fd=unsafe{libc::fcntl(fd,libc::F_DUPFD_CLOEXEC,0)};
        if fd<0{Err(io::Error::last_os_error())}else{Ok(unsafe{OwnedFd::from_raw_fd(fd)})}
    }).map_err(failure)
}
impl Description {
    pub fn side(&self) -> Side {
        self.endpoint.side
    }
    pub fn descriptor(&self) -> BorrowedFd<'_> {
        self.endpoint.capability.descriptor()
    }
    pub fn carrier(&self) -> Result<BorrowedFd<'_>, Errno> {
        self.endpoint.capability.carrier().ok_or(errno::EIO)
    }
    pub fn flags(&self) -> u64 {
        self.endpoint.capability.status().flags()
    }
    pub fn set_flags(&self, flags: u64) -> Result<(), Errno> {
        self.client
            .set_flags(&self.endpoint, flags)
            .map_err(failure)
    }
    pub fn slave_lock(&self,set:Option<bool>)->Result<bool,Errno>{self.client.slave_lock(&self.endpoint,set).map_err(failure)}
    pub fn claim_tty(&self,force:bool)->Result<(),Errno>{self.client.claim_tty(&self.endpoint,force).map_err(failure)}
    pub fn tty_session(&self)->Result<(i32,i32),Errno>{self.client.tty_session(&self.endpoint).map_err(failure)}
    pub fn set_foreground(&self,pgrp:i32)->Result<(),Errno>{self.client.set_foreground(&self.endpoint,pgrp).map_err(failure)}
    pub fn detach_tty(&self)->Result<(),Errno>{self.client.detach_tty(&self.endpoint).map_err(failure)}
    pub fn watch(&self) -> Result<Watch, Errno> {
        self.endpoint
            .capability
            .observe()
            .map(|observation| Watch { observation, side:self.side() })
            .map_err(failure)
    }
    pub fn open_slave(self: &Arc<Self>, flags: u64) -> Result<Arc<Self>, Errno> {
        let endpoint = self
            .client
            .open_slave(&self.endpoint, flags)
            .map_err(failure)?;
        Ok(Arc::new(Self {
            client: self.client.clone(),
            endpoint,
        }))
    }
    pub fn read(&self, bytes: &mut [u8]) -> Result<usize, Errno> {
        self.client.read(&self.endpoint, bytes).map_err(failure)
    }
    pub fn write(&self, bytes: &[u8]) -> Result<usize, Errno> {
        self.client.write(&self.endpoint, bytes).map_err(failure)
    }
    fn wait_until(&self, write: bool, deadline: Option<std::time::Instant>) -> Result<bool, Errno> {
        use std::os::fd::{FromRawFd, OwnedFd};
        let observation = self.watch()?.observation;
        let queue = PrivateFd::allocate(|| {
            let fd = unsafe { libc::kqueue() };
            if fd < 0 { Err(io::Error::last_os_error()) } else { Ok(unsafe { OwnedFd::from_raw_fd(fd) }) }
        }).map_err(failure)?;
        let mut changes: [libc::kevent; 3] = unsafe { std::mem::zeroed() };
        changes[0].ident = observation.data().as_raw_fd() as usize;
        changes[0].filter = if write { libc::EVFILT_WRITE } else { libc::EVFILT_READ };
        changes[0].flags = libc::EV_ADD;
        changes[0].fflags = libc::NOTE_LOWAT;
        changes[0].data = 1;
        changes[1].ident = observation.notification().ok_or(errno::EIO)?.as_raw_fd() as usize;
        changes[1].filter = libc::EVFILT_READ;
        changes[1].flags = libc::EV_ADD;
        changes[2].ident = 0;
        changes[2].filter = libc::EVFILT_USER;
        changes[2].flags = libc::EV_ADD | libc::EV_CLEAR;
        if unsafe { libc::kevent(queue.as_raw_fd(), changes.as_ptr(), 3, std::ptr::null_mut(), 0, std::ptr::null()) } < 0 { return Err(errno::last()); }
        let remaining = deadline.map(|end| end.saturating_duration_since(std::time::Instant::now()));
        if remaining.is_some_and(|duration| duration.is_zero()) { return Ok(false); }
        let timeout = remaining.map(|duration| libc::timespec { tv_sec: duration.as_secs() as _, tv_nsec: duration.subsec_nanos() as _ });
        let mut events: [libc::kevent; 2] = unsafe { std::mem::zeroed() };
        let wake = || unsafe {
            let saved = *libc::__error();
            let mut event:libc::kevent=std::mem::zeroed();
            event.filter=libc::EVFILT_USER;event.fflags=libc::NOTE_TRIGGER;
            libc::kevent(queue.as_raw_fd(),&event,1,std::ptr::null_mut(),0,std::ptr::null());
            *libc::__error()=saved;
        };
        let result = super::signal::interruptible(&wake, || {
            let count = unsafe { libc::kevent(queue.as_raw_fd(), std::ptr::null(), 0, events.as_mut_ptr(), 2, timeout.as_ref().map_or(std::ptr::null(), |timeout| timeout)) };
            if count < 0 { return Err(errno::last()); }
            for event in &events[..count as usize] {
                if event.flags & libc::EV_ERROR != 0 { return Err(errno::from_darwin(event.data as i32)); }
                if event.filter==libc::EVFILT_USER { return Err(errno::EINTR); }
            }
            Ok(count != 0)
        });
        result.unwrap_or(Err(errno::EINTR))
    }
    fn wait(&self, write: bool) -> Result<(), Errno> { self.wait_until(write, None).map(|_|()) }
    fn blocking_read(&self, bytes: &mut [u8]) -> Result<usize, Errno> {
        if self.flags() & super::fs::O_NONBLOCK != 0 { return self.read(bytes); }
        let mut minimum = 1;
        let mut interval = std::time::Duration::ZERO;
        let mut canonical = true;
        if self.side() == Side::Slave {
            let mut mode: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(self.descriptor().as_raw_fd(), &mut mode) } < 0 { return Err(errno::last()); }
            canonical = mode.c_lflag & libc::ICANON != 0;
            if !canonical {
                minimum = (mode.c_cc[libc::VMIN] as usize).min(bytes.len());
                interval = std::time::Duration::from_millis(mode.c_cc[libc::VTIME] as u64 * 100);
            }
        }
        let mut deadline = (minimum == 0 && !interval.is_zero()).then(|| std::time::Instant::now() + interval);
        let mut count = 0;
        loop {
            match self.read(&mut bytes[count..]) {
                Ok(n) if n > 0 => {
                    count += n;
                    if canonical || count >= minimum { return Ok(count); }
                    if !interval.is_zero() { deadline = Some(std::time::Instant::now() + interval); }
                }
                Ok(_) if canonical || self.endpoint.capability.status().hangup() => return Ok(count),
                Ok(_) | Err(errno::EAGAIN) => {},
                Err(error) => return if count > 0 { Ok(count) } else { Err(error) },
            }
            if minimum == 0 && interval.is_zero() { return Ok(count); }
            match self.wait_until(false, deadline) {
                Ok(true) => {},
                Ok(false) => return Ok(count),
                Err(error) => return if count > 0 { Ok(count) } else { Err(error) },
            }
        }
    }
    pub(super) fn job_control(&self,write:bool,force:bool)->Result<(),Errno>{
        if self.side()!=Side::Slave{return Ok(());}
        let (sid,foreground)=match self.tty_session(){Ok(state)=>state,Err(errno::ENOTTY)=>return Ok(()),Err(error)=>return Err(error)};
        let pgrp=unsafe{libc::getpgrp()};
        if foreground==pgrp{return Ok(());}
        if unsafe{libc::getsid(0)}!=sid{return Err(errno::ENOTTY);}
        if write&&!force{
            let mut mode:libc::termios=unsafe{std::mem::zeroed()};
            if unsafe{libc::tcgetattr(self.descriptor().as_raw_fd(),&mut mode)}<0{return Err(errno::last());}
            if mode.c_lflag&libc::TOSTOP==0{return Ok(());}
        }
        let signal=if write{22}else{21};
        if super::signal::tty_signal_ignored(signal)?{return if write{Ok(())}else{Err(errno::EIO)};}
        let members=super::pidns::tty_group_members(pgrp,sid)?;
        if super::pidns::tty_group_orphaned(&members,pgrp,sid)?{return Err(errno::EIO);}
        super::signal::tty_signal_group(&members,pgrp,sid,signal)?;
        Err(errno::EINTR)
    }
    pub fn rw(&self, iov: &[libc::iovec], write: bool) -> i64 {
        match self.rw_result(iov,write) { Ok(count)=>count as i64, Err(error)=>-(error as i64) }
    }
    fn rw_result(&self,iov:&[libc::iovec],write:bool)->Result<usize,Errno>{
        let mode=self.flags()&3;
        if !(if write{matches!(mode,1|2)}else{matches!(mode,0|2)}){return Err(errno::EBADF);}
        let length=iov.iter().try_fold(0usize,|sum,part|sum.checked_add(part.iov_len)).filter(|length|*length<=isize::MAX as usize).ok_or(errno::EINVAL)?;
        if length==0{return Ok(0);}
        self.job_control(write,false)?;
        for part in iov{if write{super::user_memory::prepare_read(part.iov_base as u64,part.iov_len)?;}else{super::user_memory::prepare_write(part.iov_base as u64,part.iov_len)?;}}
        if !write{
            let mut bytes=vec![0;length.min(4096)];
            let count=self.blocking_read(&mut bytes)?;
            let mut copied=0;
            for part in iov{let length=part.iov_len.min(count-copied);super::user_memory::write_exact(part.iov_base as u64,&bytes[copied..copied+length])?;copied+=length;if copied==count{break;}}
            return Ok(count);
        }
        let mut total=0;
        for part in iov{
            let mut at=0;
            while at<part.iov_len{
                let length=(part.iov_len-at).min(4096);
                let mut bytes=if write{super::user_memory::read_exact(part.iov_base as u64+at as u64,length)?}else{vec![0;length]};
                let result=loop{
                    let result=if write{self.write(&bytes)}else{self.read(&mut bytes)};
                    match result{Err(errno::EAGAIN)if self.flags()&super::fs::O_NONBLOCK==0=>{
                        if let Err(error)=self.wait(write){break Err(error);}
                    },other=>break other}
                };
                let count=match result{Ok(count)=>count,Err(error)=>return if total>0{Ok(total)}else{Err(error)}};
                if !write{super::user_memory::write_exact(part.iov_base as u64+at as u64,&bytes[..count])?;}
                total+=count;at+=count;
                if count<length||!write{return Ok(total);}
            }
        }
        Ok(total)
    }
}
struct Owner{client:Arc<Client>,_child:Option<std::process::Child>}
static OWNER:std::sync::OnceLock<Result<Owner,Errno>>=std::sync::OnceLock::new();
fn configured_client()->Result<Option<Arc<Client>>,Errno>{
    OWNER.get_or_init(start_owner).as_ref().map(|owner|Some(owner.client.clone())).map_err(|error|*error)
}
fn start_owner()->Result<Owner,Errno>{
    let Some(runtime) = crate::vfs::runtime_dir() else {
        return Err(errno::ENODEV);
    };
    let locator=runtime.join("pty-control-owner");
    if let Some(init)=super::pidns::init_registration().map_err(|error|-error as Errno)?{
        let config=OwnerConfig::read(&locator,init.process).map_err(failure)?;
        return Ok(Owner{client:Arc::new(Client::lookup(&config.endpoint,config.process).map_err(failure)?),_child:None});
    }
    let table=super::cred::publish_current_for_owner(&runtime.join("identity/by-pid"))?;
    if crate::vfs::guest_path_of_host(&locator).is_some(){return Err(errno::EPERM);}
    match std::fs::symlink_metadata(&locator){
        Ok(_)=>{
            let config=OwnerConfig::read_standalone(&locator).map_err(failure)?;
            return Ok(Owner{client:Arc::new(Client::lookup(&config.endpoint,config.process).map_err(failure)?),_child:None});
        },Err(error)if error.kind()==io::ErrorKind::NotFound=>{},Err(error)=>return Err(failure(error)),
    }
    use std::os::unix::fs::{DirBuilderExt,MetadataExt};
    let pairs=runtime.join("pty-pairs");
    match std::fs::DirBuilder::new().mode(0o700).create(&pairs) {
        Ok(()) => {},
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {},
        Err(error) => return Err(failure(error)),
    }
    let metadata=std::fs::symlink_metadata(&pairs).map_err(failure)?;
    if !metadata.is_dir()||metadata.uid()!=unsafe{libc::geteuid()}||metadata.mode()&0o077!=0{return Err(errno::EPERM);}
    let pairs=std::fs::canonicalize(pairs).map_err(failure)?;
    let actor=aim_storage::process_namespace::ProcessIdentity::running(unsafe{libc::getpid()}).map_err(failure)?;
    let name=format!("dev.aim.pty.{}.{}.{}",actor.host_pid,actor.start_seconds,actor.start_microseconds);
    let executable=std::env::current_exe().map_err(failure)?.with_file_name("aim-pty-holder");
    let mut child=std::process::Command::new(executable).arg("--service-name").arg(&name).arg("--runtime").arg(&pairs).arg("--identity-table").arg(&table).arg("--owner-locator").arg(&locator).stdin(std::process::Stdio::null()).spawn().map_err(failure)?;
    let result=(||{
        let process=aim_storage::process_namespace::ProcessIdentity::running(child.id()as i32).map_err(failure)?;
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(5);
        let client=loop{
            if child.try_wait().map_err(failure)?.is_some(){return Err(errno::from_darwin(libc::ECHILD));}
            match Client::lookup(&name,process){Ok(client)=>break Arc::new(client),Err(error)if error.raw_os_error()==Some(libc::ENOENT)=>{
                if std::time::Instant::now()>=deadline{return Err(errno::from_darwin(libc::ETIMEDOUT));}std::thread::sleep(std::time::Duration::from_millis(5));
            },Err(error)=>return Err(failure(error)),}
        };
        loop{match OwnerConfig::read(&locator,process){Ok(config)if config.endpoint==name=>break,Ok(_)=>return Err(errno::EPERM),Err(error)if error.kind()==io::ErrorKind::NotFound=>{if child.try_wait().map_err(failure)?.is_some(){return Err(errno::from_darwin(libc::ECHILD));}if std::time::Instant::now()>=deadline{return Err(errno::from_darwin(libc::ETIMEDOUT));}std::thread::sleep(std::time::Duration::from_millis(5));},Err(error)=>return Err(failure(error))}}
        Ok(client)
    })();
    match result{Ok(client)=>Ok(Owner{client,_child:Some(child)}),Err(error)=>{if child.try_wait().map_err(failure)?.is_none(){child.kill().map_err(failure)?;}child.wait().map_err(failure)?;Err(error)}}
}
pub(super) fn allocate(flags: u64) -> Result<Option<Arc<Description>>, Errno> {
    let Some(client) = configured_client()? else {
        return Ok(None);
    };
    let endpoint = client.allocate(flags).map_err(failure)?;
    client.slave_lock(&endpoint,Some(true)).map_err(failure)?;
    Ok(Some(Arc::new(Description { client, endpoint })))
}
pub(super) fn open_slave(path:&[u8],flags:u64)->Result<Option<Arc<Description>>,Errno>{
    let Some(client)=configured_client()?else{return Ok(None);};
    match client.open_slave_path(path,flags){Ok(endpoint)=>Ok(Some(Arc::new(Description{client,endpoint}))),Err(error)if error.raw_os_error()==Some(libc::ENOENT)=>Ok(None),Err(error)=>Err(failure(error))}
}
pub(super) fn publish(description: Arc<Description>, flags: u64) -> Result<i32, Errno> {
    let result = {
        let _guard = super::fdtab::lifecycle();
        let command = if flags & 0x80000 != 0 { libc::F_DUPFD_CLOEXEC } else { libc::F_DUPFD };
        let fd = unsafe { libc::fcntl(description.descriptor().as_raw_fd(), command, 0) };
        if fd < 0 { return Err(errno::last()); }
        super::fdtab::insert(fd, super::fdtab::Kind::Pty(description.clone()));
        match super::fdtab::publish_guest(fd) {
            Ok(()) => Ok(fd),
            Err(error) => {
                super::fdtab::on_close(fd);
                unsafe { libc::close(fd); }
                Err(error)
            }
        }
    };
    result
}
pub(super) fn inherit_child(pid:i32)->Result<(),Errno>{
    let Some(owner)=OWNER.get()else{return Ok(());};let owner=owner.as_ref().map_err(|error|*error)?;
    let child=aim_storage::process_namespace::ProcessIdentity::running(pid).map_err(failure)?;
    owner.client.inherit_child(child).map_err(failure)
}
pub(super) fn restore(carrier: BorrowedFd<'_>) -> Result<Arc<Description>, Errno> {
    let client = configured_client()?.ok_or(errno::ENODEV)?;
    let carrier = PrivateFd::allocate(|| {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        let fd = unsafe { libc::fcntl(carrier.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        }
    })
    .map_err(failure)?;
    let endpoint = client.import_carrier(carrier).map_err(failure)?;
    Ok(Arc::new(Description { client, endpoint }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::{fd::AsRawFd, unix::fs::DirBuilderExt};
    struct GuestFd(i32);
    impl Drop for GuestFd {
        fn drop(&mut self) { if super::super::fdtab::visible(self.0) { super::super::fdtab::close_owned_guest(self.0,false).unwrap(); } }
    }
    fn publish(description: Arc<Description>) -> GuestFd {
        let _guard=super::super::fdtab::lifecycle();
        let fd=unsafe{libc::fcntl(description.descriptor().as_raw_fd(),libc::F_DUPFD_CLOEXEC,3)};
        assert!(fd>=0);
        super::super::fdtab::insert(fd,super::super::fdtab::Kind::Pty(description));
        super::super::fdtab::publish_guest(fd).unwrap();GuestFd(fd)
    }
    #[test]
    fn actual_typed_dispatch_preserves_output_flags_and_final_close() {
        if super::super::fdtab::isolated_kernel_test("sys::pty_owner::tests::actual_typed_dispatch_preserves_output_flags_and_final_close"){return;}
        let (_guard,root)=crate::vfs::test_view();
        let runtime=root.join("pty-owner");
        std::fs::DirBuilder::new().mode(0o700).create(&runtime).unwrap();
        let runtime=std::fs::canonicalize(runtime).unwrap();
        let name=format!("dev.aim.pty-abi.{}",std::process::id());
        let owner=aim_storage::pty_owner::transport::RunningServer::start(&name,&runtime).unwrap();
        let client=Arc::new(Client::lookup(&name,owner.config.process).unwrap());
        let endpoint=client.allocate(2).unwrap();
        let master_description=Arc::new(Description{client,endpoint});
        let slave_description=master_description.open_slave(2).unwrap();
        let master=publish(master_description.clone());
        let slave=publish(slave_description);
        for request in [0x5401,0x5402,0x5413,0x5414,0x80045430,0x80045439,0x40045431] {
            assert_eq!(super::super::fs::ioctl([master.0 as u64,request,1,0,0,0]),-(errno::EFAULT as i64));
        }
        let input=[b'x';268];
        assert_eq!(super::super::fs::write([slave.0 as u64,input.as_ptr()as u64,input.len()as u64,0,0,0]),268);
        assert_eq!(super::super::fs::fcntl([slave.0 as u64,3,0,0,0,0])&0x800,0);
        assert_eq!(super::super::fs::fcntl([slave.0 as u64,4,0x802,0,0,0]),0);
        assert_ne!(super::super::fs::fcntl([slave.0 as u64,3,0,0,0,0])&0x800,0);
        let duplicate=super::super::fs::dup([slave.0 as u64,0,0,0,0,0]);assert!(duplicate>=0);
        let duplicate=GuestFd(duplicate as i32);
        drop(slave);
        assert!(!master_description.endpoint.capability.status().hangup());
        drop(duplicate);
        let notification=master_description.endpoint.capability.notification().unwrap();
        let mut event=libc::pollfd{fd:notification.as_raw_fd(),events:libc::POLLIN,revents:0};
        assert_eq!(unsafe{libc::poll(&mut event,1,2000)},1);
        assert!(master_description.endpoint.capability.status().hangup());
        let mut output=[0;268];
        assert_eq!(super::super::fs::read([master.0 as u64,output.as_mut_ptr()as u64,output.len()as u64,0,0,0]),268);
        assert_eq!(output,input);
        assert_eq!(super::super::fs::read([master.0 as u64,output.as_mut_ptr()as u64,1,0,0,0]),-(errno::EIO as i64));
        drop(master);
        let endpoint=master_description.client.allocate(2).unwrap();
        let waiting=Arc::new(Description{client:master_description.client.clone(),endpoint});
        let peer=publish(waiting.open_slave(2).unwrap());
        let waiting=publish(waiting);
        let epoll=super::super::epoll::epoll_create1([0;6]);assert!(epoll>=0);let epoll=GuestFd(epoll as i32);
        let event=[1u64,0x1259];
        assert_eq!(super::super::epoll::epoll_ctl([epoll.0 as u64,1,waiting.0 as u64,event.as_ptr()as u64,0,0]),0);
        let slave_event=[1u64,0xdead];
        assert_eq!(super::super::epoll::epoll_ctl([epoll.0 as u64,1,peer.0 as u64,slave_event.as_ptr()as u64,0,0]),0);
        let barrier=Arc::new(std::sync::Barrier::new(2));
        let closing=barrier.clone();
        let closer=std::thread::spawn(move||{closing.wait();drop(peer);});
        super::super::poll::BEFORE_WAIT.with(|hook|*hook.borrow_mut()=Some(Box::new(move||{barrier.wait();})));
        let mut descriptor=libc::pollfd{fd:waiting.0,events:libc::POLLIN,revents:0};
        let timeout=[2i64,0];
        assert_eq!(super::super::poll::ppoll([&mut descriptor as*mut _ as u64,1,timeout.as_ptr()as u64,0,0,0]),1);
        assert_ne!(descriptor.revents&libc::POLLHUP,0);
        closer.join().unwrap();
        let mut event=[0u64;2];
        assert_eq!(super::super::epoll::epoll_pwait([epoll.0 as u64,event.as_mut_ptr()as u64,1,2000,0,0]),1);
        assert_ne!(event[0]&0x10,0);
        assert_eq!(event[1],0x1259,"closed slave watch must not retain or deliver stale OFD");
        drop(epoll);
        drop(waiting);drop(master_description);
        owner.shutdown().unwrap();std::fs::remove_dir(runtime).unwrap();
    }
}
