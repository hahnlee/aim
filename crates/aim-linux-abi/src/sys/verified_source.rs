//! Stable kernel file reads retain actual inode and fs-verity owners.
use super::{attrs,cred,fdtab,regular_file};
use aim_storage::{fsverity::{Metadata,Identity},private_fd::PrivateFd};
use crate::errno::{self,Errno};
use std::{ffi::CStr,os::fd::{AsFd,AsRawFd,BorrowedFd,FromRawFd},sync::Arc};

pub struct VerifiedSource { data:PrivateFd, description:Option<Arc<regular_file::Description>>, proof:Option<Arc<Metadata>>, identity:Identity }
fn io(error:std::io::Error)->Errno{errno::from_darwin(error.raw_os_error().unwrap_or(libc::EIO))}
impl VerifiedSource {
    pub fn from_descriptor(fd:BorrowedFd<'_>,description:Option<Arc<regular_file::Description>>)->Result<Self,Errno>{
        let data=PrivateFd::allocate(||fd.try_clone_to_owned()).map_err(io)?;
        let identity=Identity::from_fd(data.as_fd()).map_err(io)?;
        if description.as_ref().is_some_and(|description|description.identity!=identity){return Err(errno::EBADF);}
        let proof=description.as_ref().map(|description|description.store.lookup(identity).map(|proof|proof.map(Arc::new)).map_err(|_|errno::EIO)).transpose()?.flatten();
        Ok(Self{data,description,proof,identity})
    }
    pub fn from_proof(data:PrivateFd,proof:Arc<Metadata>)->Result<Self,Errno>{
        let identity=Identity::from_fd(data.as_fd()).map_err(io)?;
        // An empty range still checks the actual source identity and size.
        proof.verify_range(&data,0,0).map_err(|_|errno::EIO)?;
        Ok(Self{data,description:None,proof:Some(proof),identity})
    }
    pub fn from_pinned(held:&fdtab::Pinned)->Result<Self,Errno>{
        let description=match held.kind(){Some(fdtab::Kind::Regular(description))=>Some(description.clone()),_=>None};
        Self::from_descriptor(held.descriptor(),description)
    }
    /// Native loader path after generic guest resolution. Guest pathname policy
    /// is checked against the pinned descriptor rather than host user rights.
    pub fn open(host:&CStr,execute:bool)->Result<Self,Errno>{
        let data=PrivateFd::allocate(||{let fd=unsafe{libc::open(host.as_ptr(),libc::O_RDONLY|libc::O_CLOEXEC)};if fd<0{return Err(std::io::Error::last_os_error());}Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(fd)})}).map_err(io)?;
        let guest=crate::vfs::guest_path_of_host(std::path::Path::new(std::ffi::OsStr::from_bytes(host.to_bytes())));
        if let Some(guest)=guest {
            let id=cred::current();
            let mut at=String::from("/");
            for component in guest.trim_start_matches('/').split('/').filter(|part|!part.is_empty()).collect::<Vec<_>>().split_last().map_or(&[][..],|(_,parents)|parents){
                attrs::search(&at,&id,attrs::FS)?;if at!="/"{at.push('/');}at.push_str(component);
            }
            attrs::search(&at,&id,attrs::FS)?;
            let mut stat:libc::stat=unsafe{std::mem::zeroed()};if unsafe{libc::fstat(data.as_raw_fd(),&mut stat)}<0{return Err(errno::last());}
            attrs::apply(attrs::Host::Fd(data.as_raw_fd()),||guest,&mut stat);
            if stat.st_mode&libc::S_IFMT!=libc::S_IFREG{return Err(errno::EACCES);}
            if attrs::recording()&&!attrs::permits(&stat,if execute{1}else{4},&id,attrs::FS){return Err(errno::EACCES);}
        }
        let description=regular_file::adopt_kernel(data.as_fd(),0)?;
        Self::from_descriptor(data.as_fd(),description)
    }
    pub fn descriptor(&self)->BorrowedFd<'_>{self.data.as_fd()}
    pub fn identity(&self)->Identity{self.identity}
    pub fn len(&self)->Result<u64,Errno>{let mut stat:libc::stat=unsafe{std::mem::zeroed()};if unsafe{libc::fstat(self.data.as_raw_fd(),&mut stat)}<0{return Err(errno::last());}Ok(stat.st_size as u64)}
    pub fn read_at(&self,buffer:&mut[u8],offset:u64)->Result<usize,Errno>{
        if offset>i64::MAX as u64 || offset.checked_add(buffer.len()as u64).is_none(){return Err(errno::EINVAL);}
        if Identity::from_fd(self.data.as_fd()).map_err(io)?!=self.identity{return Err(errno::EIO);}
        if let Some(proof)=&self.proof {
            let bytes=proof.verify_range(&self.data,offset,buffer.len()).map_err(|_|errno::EIO)?;
            buffer[..bytes.len()].copy_from_slice(&bytes);return Ok(bytes.len());
        }
        let size=unsafe{libc::pread(self.data.as_raw_fd(),buffer.as_mut_ptr().cast(),buffer.len(),offset as i64)};
        if size<0{Err(errno::last())}else{Ok(size as usize)}
    }
    pub fn read_exact_at(&self,buffer:&mut[u8],offset:u64)->Result<(),Errno>{
        let mut verified=vec![0;buffer.len()];let mut done=0;
        while done<verified.len(){let count=self.read_at(&mut verified[done..],offset.checked_add(done as u64).ok_or(errno::EINVAL)?)?;if count==0{return Err(errno::EIO);}done+=count;}
        buffer.copy_from_slice(&verified);Ok(())
    }
    pub fn protected(&self)->bool{self.proof.is_some()}
    pub fn mapping_source(&self,extent:Option<(u64,u64)>)->Result<Option<Arc<super::verity_pager::Source>>,Errno>{
        let Some(proof)=&self.proof else{return Ok(None);};
        Ok(Some(Arc::new(super::verity_pager::Source{data:self.data.try_clone().map_err(io)?,proof:Some(proof.clone()),description:self.description.clone(),cache:super::verity_pager::MappingCache::runtime().map_err(io)?,extent,derivative:None})))
    }
}
use std::os::unix::ffi::OsStrExt;

impl AsRawFd for VerifiedSource{fn as_raw_fd(&self)->i32{self.data.as_raw_fd()}}
#[cfg(test)]
mod tests {
    use super::*;use std::{fs::{self,File},os::unix::fs::FileExt};
    #[test]
    fn protected_kernel_reads_keep_buffer_unchanged_on_actual_corruption() {
        let root=std::env::temp_dir().join(format!("aim-verified-source-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));fs::create_dir(&root).unwrap();
        fs::write(root.join("data"),vec![37;8192]).unwrap();let file=File::open(root.join("data")).unwrap();
        let store=Arc::new(aim_storage::fsverity::Store::new(&root.join("proof"),&root.join("runtime")).unwrap());let admission=store.lock_inode(&file).unwrap();let guard=admission.begin_enable().unwrap();drop(admission);
        let prepared=guard.build(aim_storage::fsverity::BuildOptions::new(1,4096,vec![],16384,4096).unwrap(),&[],||false).unwrap();drop(guard.commit(prepared).unwrap());
        let identity=Identity::from_fd(file.as_fd()).unwrap();let proof=Arc::new(store.lookup(identity).unwrap().unwrap());
        let source=VerifiedSource::from_proof(PrivateFd::adopt(file.into()).unwrap(),proof).unwrap();
        let mut buffer=[0;64];source.read_exact_at(&mut buffer,4070).unwrap();assert_eq!(buffer,[37;64]);
        File::options().write(true).open(root.join("data")).unwrap().write_all_at(&[99],4100).unwrap();
        buffer.fill(81);assert_eq!(source.read_exact_at(&mut buffer,4070),Err(errno::EIO));assert_eq!(buffer,[81;64]);
        assert!(source.protected());drop(source);fs::remove_dir_all(root).unwrap();
    }
}
