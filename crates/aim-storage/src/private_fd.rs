//! Kernel-owned descriptor lifetime. The ABI registrar owns allocation/fork
//! serialization and visibility; callbacks alone are not a raw syscall gate.
use std::{fs::File,io::{self,Read,Write,Seek,SeekFrom},os::fd::{AsFd,AsRawFd,BorrowedFd,FromRawFd,OwnedFd,RawFd},sync::{Arc,OnceLock}};
pub trait AllocationGuard {}
/// enter must cover host allocation through adoption and close through closed.
/// The guard need not be reentrant: adapters must not allocate another private
/// wrapper inside adopt/closed, and producers must not nest allocate helpers.
pub trait Registrar:Send+Sync {
    fn enter(&self)->Box<dyn AllocationGuard>;
    /// Consumes the actual fd, relocates/hides it, and rolls back on failure.
    fn adopt(&self,descriptor:OwnedFd)->io::Result<OwnedFd>;
    /// Called only after the actual descriptor has closed, under enter's guard.
    fn closed(&self,descriptor:RawFd);
}
static REGISTRAR:OnceLock<Arc<dyn Registrar>>=OnceLock::new();
/// ABI startup must install before any guest-private allocation. Earlier
/// native descriptors deliberately retain their native lifetime semantics.
pub fn install(registrar:Arc<dyn Registrar>)->Result<(),Arc<dyn Registrar>>{REGISTRAR.set(registrar)}
pub struct PrivateFd {descriptor:Option<OwnedFd>,registrar:Option<Arc<dyn Registrar>>}
impl PrivateFd {
    pub fn allocate(create:impl FnOnce()->io::Result<OwnedFd>)->io::Result<Self>{
        let registrar=REGISTRAR.get().cloned();
        let descriptor=if let Some(registrar)=&registrar{let _guard=registrar.enter();registrar.adopt(create()?)?}else{create()?};
        Ok(Self{descriptor:Some(descriptor),registrar})
    }
    /// Existing carriers must already have authenticated ownership. This does
    /// not claim to guard a prior host allocation performed by another caller.
    pub fn adopt(descriptor:OwnedFd)->io::Result<Self>{Self::allocate(||Ok(descriptor))}
    pub fn try_clone(&self)->io::Result<Self>{Self::allocate(||{let fd=unsafe{libc::fcntl(self.as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};if fd<0{Err(io::Error::last_os_error())}else{Ok(unsafe{OwnedFd::from_raw_fd(fd)})}})}
    /// Plain extraction is allowed only for native descriptors that have never
    /// acquired a hidden role. Registered carriers retain this wrapper.
    pub fn into_fd(mut self)->io::Result<OwnedFd>{if self.registrar.is_some(){return Err(io::Error::from_raw_os_error(libc::EPERM));}Ok(self.descriptor.take().unwrap())}
}
impl AsFd for PrivateFd{fn as_fd(&self)->BorrowedFd<'_>{self.descriptor.as_ref().unwrap().as_fd()}}
impl AsRawFd for PrivateFd{fn as_raw_fd(&self)->RawFd{self.as_fd().as_raw_fd()}}
impl Drop for PrivateFd{fn drop(&mut self){if let Some(descriptor)=self.descriptor.take(){if let Some(registrar)=&self.registrar{let _guard=registrar.enter();let fd=descriptor.as_raw_fd();drop(descriptor);registrar.closed(fd);}else{drop(descriptor);}}}}
/// File I/O borrows the wrapped open description without detaching its role.
pub struct PrivateFile {descriptor:PrivateFd}
impl PrivateFile {
    pub fn allocate(create:impl FnOnce()->io::Result<File>)->io::Result<Self>{Ok(Self{descriptor:PrivateFd::allocate(||Ok(create()?.into()))?})}
    pub fn adopt(file:File)->io::Result<Self>{Self::allocate(||Ok(file))}
    pub fn try_clone(&self)->io::Result<Self>{Ok(Self{descriptor:self.descriptor.try_clone()?})}
    pub fn sync_all(&self)->io::Result<()>{if unsafe{libc::fsync(self.as_raw_fd())}<0{Err(io::Error::last_os_error())}else{Ok(())}}
    pub fn into_private_fd(self)->PrivateFd{self.descriptor}
    pub fn into_file(self)->io::Result<File>{Ok(File::from(self.descriptor.into_fd()?))}
    fn borrowed_file(&self)->std::mem::ManuallyDrop<File>{std::mem::ManuallyDrop::new(unsafe{File::from_raw_fd(self.as_raw_fd())})}
}
impl AsFd for PrivateFile{fn as_fd(&self)->BorrowedFd<'_>{self.descriptor.as_fd()}}
impl AsRawFd for PrivateFile{fn as_raw_fd(&self)->RawFd{self.descriptor.as_raw_fd()}}
impl Read for PrivateFile{fn read(&mut self,bytes:&mut[u8])->io::Result<usize>{self.borrowed_file().read(bytes)}}
impl Write for PrivateFile{fn write(&mut self,bytes:&[u8])->io::Result<usize>{self.borrowed_file().write(bytes)}fn flush(&mut self)->io::Result<()>{self.borrowed_file().flush()}}
impl Seek for PrivateFile{fn seek(&mut self,from:SeekFrom)->io::Result<u64>{self.borrowed_file().seek(from)}}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex,atomic::{AtomicBool,Ordering}};
    static GATE:Mutex<()>=Mutex::new(());
    static ENTERED:AtomicBool=AtomicBool::new(false);
    static EVENTS:Mutex<Vec<(char,i32)>>=Mutex::new(Vec::new());
    struct Guard{_guard:std::sync::MutexGuard<'static,()>}
    impl AllocationGuard for Guard {}
    impl Drop for Guard{fn drop(&mut self){ENTERED.store(false,Ordering::SeqCst);}}
    struct Roles;
    impl Registrar for Roles {
        fn enter(&self)->Box<dyn AllocationGuard>{let guard=GATE.lock().unwrap();assert!(!ENTERED.swap(true,Ordering::SeqCst));Box::new(Guard{_guard:guard})}
        fn adopt(&self,fd:OwnedFd)->io::Result<OwnedFd>{
            assert!(ENTERED.load(Ordering::SeqCst));
            let copied=unsafe{libc::fcntl(fd.as_raw_fd(),libc::F_DUPFD_CLOEXEC,64)};
            if copied<0{return Err(io::Error::last_os_error());}
            drop(fd);EVENTS.lock().unwrap().push(('h',copied));Ok(unsafe{OwnedFd::from_raw_fd(copied)})
        }
        fn closed(&self,fd:i32){assert!(ENTERED.load(Ordering::SeqCst));assert_eq!(unsafe{libc::fcntl(fd,libc::F_GETFD)},-1);assert_eq!(io::Error::last_os_error().raw_os_error(),Some(libc::EBADF));EVENTS.lock().unwrap().push(('c',fd));}
    }
    #[test]
    fn registered_descriptor_lifecycle_runs_in_isolated_process(){
        const MARKER:&str="private-fd-isolated-fixture";
        if !std::env::args().any(|arg|arg==MARKER){
            let output=std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","private_fd::tests::registered_descriptor_lifecycle_runs_in_isolated_process","--skip",MARKER,"--nocapture"]).output().unwrap();
            assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
            assert!(String::from_utf8_lossy(&output.stdout).contains("PRIVATE_FD_OWNER_EXECUTED"),"isolated fixture did not execute: {}",String::from_utf8_lossy(&output.stdout));return;
        }
        // A descriptor allocated before registration remains a native owner;
        // ABI startup must install before it opens any guest-private storage.
        let native=PrivateFile::allocate(||File::open(std::env::current_exe().unwrap())).unwrap();
        assert!(install(Arc::new(Roles)).is_ok());assert!(install(Arc::new(Roles)).is_err());
        let fd=PrivateFd::allocate(||{
            assert!(ENTERED.load(Ordering::SeqCst));
            let raw=unsafe{libc::open(c"/dev/null".as_ptr(),libc::O_RDONLY|libc::O_CLOEXEC)};
            if raw<0{Err(io::Error::last_os_error())}else{Ok(unsafe{OwnedFd::from_raw_fd(raw)})}
        }).unwrap();
        let alias=fd.try_clone().unwrap();let first=fd.as_raw_fd();let second=alias.as_raw_fd();assert_ne!(first,second);drop(fd);
        assert_eq!(unsafe{libc::fcntl(second,libc::F_GETFD)},libc::FD_CLOEXEC);
        assert_eq!(alias.into_fd().err().unwrap().raw_os_error(),Some(libc::EPERM));
        assert_eq!(EVENTS.lock().unwrap().as_slice(),&[('h',first),('h',second),('c',first),('c',second)]);
        let descriptor=native.into_file().unwrap();drop(descriptor);
        let path=std::env::temp_dir().join(format!("aim-private-file-{}",std::process::id()));
        let mut file=PrivateFile::allocate(||std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path)).unwrap();
        file.write_all(b"actual private data").unwrap();file.sync_all().unwrap();file.seek(SeekFrom::Start(0)).unwrap();let mut bytes=Vec::new();file.read_to_end(&mut bytes).unwrap();assert_eq!(bytes,b"actual private data");let filefd=file.as_raw_fd();drop(file);
        assert!(EVENTS.lock().unwrap().contains(&('c',filefd)));std::fs::remove_file(path).unwrap();
        let directory=std::env::temp_dir().join(format!("aim-private-writer-{}",std::process::id()));std::fs::create_dir(&directory).unwrap();
        let source=std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(directory.join("source")).unwrap();
        let inode=crate::inode_lease::Inode::open(&directory.join("locks"),source.as_fd()).unwrap();
        let writer=inode.admission().unwrap().writer().unwrap();let alias=writer.try_clone().unwrap();drop(writer);
        assert_eq!(inode.admission().unwrap().exclusive().err().unwrap().raw_os_error(),Some(libc::EBUSY));
        let carrier=alias.into_private_fd();let writer=inode.adopt_private_writer(carrier).unwrap();
        assert_eq!(inode.admission().unwrap().exclusive().err().unwrap().raw_os_error(),Some(libc::EBUSY));drop(writer);
        assert!(inode.admission().unwrap().exclusive().is_ok());drop(inode);drop(source);std::fs::remove_dir_all(directory).unwrap();
        assert!(!ENTERED.load(Ordering::SeqCst));
        println!("PRIVATE_FD_OWNER_EXECUTED");
    }
}
