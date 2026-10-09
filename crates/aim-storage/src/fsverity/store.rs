use super::{tree,Descriptor,Error,Result,EIO,EINVAL,ENODATA,EOVERFLOW};
use sha2::{Digest,Sha256};
use std::{fs::{self,File},os::unix::fs::OpenOptionsExt,path::{Path,PathBuf}};
use crate::inode_lease::{Identity,Inode,WriterLease,ExclusiveLease,EnableSlot};
use std::os::fd::AsFd;
use crate::private_fd::{PrivateFd,PrivateFile};
const HEADER:u64=512;
const MAGIC:&[u8;8]=b"AIMVRT02";

pub struct Metadata{file:PrivateFile,identity:Identity,descriptor:Descriptor,tree_len:u64,signature_len:u64}
impl Metadata {
 /// Import the actual private proof capability. The binding names the data
 /// inode; the proof blob itself has a different, independently owned inode.
 pub fn from_private_fd(descriptor:PrivateFd,identity:Identity)->Result<Self>{
  use std::os::fd::AsRawFd;
  let file=PrivateFile::from_private_fd(descriptor);
  if tree::stat(&file)?.st_mode&libc::S_IFMT!=libc::S_IFREG{return Err(Error::Linux(EINVAL));}
  let flags=unsafe{libc::fcntl(file.as_raw_fd(),libc::F_GETFL)};
  if flags<0{return Err(std::io::Error::last_os_error().into());}
  if flags&libc::O_ACCMODE!=libc::O_RDONLY||flags&libc::O_EVTONLY!=0{return Err(Error::Linux(EINVAL));}
  Self::from_file(file,&binding(identity))
 }
 fn open(path:&Path,binding:&[u8;36])->Result<Self>{
  let file=PrivateFile::allocate(||File::options().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(path))?;
  Self::from_file(file,binding)
 }
 fn from_file(file:PrivateFile,binding:&[u8;36])->Result<Self>{
  let mut header=[0;HEADER as usize];tree::read_exact(&file,&mut header,0)?;
  if &header[..8]!=MAGIC||&header[8..44]!=binding||header[44..48].iter().any(|byte|*byte!=0)||header[416..].iter().any(|byte|*byte!=0)||Sha256::digest(&header[..384]).as_slice()!=&header[384..416]{return Err(Error::Linux(EIO));}
  let descriptor=Descriptor::from_bytes(header[48..304].try_into().unwrap())?;
  let tree_len=u64::from_le_bytes(header[304..312].try_into().unwrap());let signature_len=u64::from_le_bytes(header[312..320].try_into().unwrap());
  let expected=descriptor.levels()?.iter().try_fold(0u64,|sum,(_,length)|sum.checked_add(*length).ok_or(Error::Linux(EOVERFLOW)))?;
  let digest=descriptor.digest();
  if tree_len!=expected||signature_len>16128||header[320..320+digest.len()]!=digest||header[320+digest.len()..384].iter().any(|byte|*byte!=0)||tree::stat(&file)?.st_size as u64!=HEADER.checked_add(tree_len).and_then(|n|n.checked_add(signature_len)).ok_or(Error::Linux(EOVERFLOW))?{return Err(Error::Linux(EIO));}
  Ok(Self{file,identity:Identity::from_bytes(binding)?,descriptor,tree_len,signature_len})
 }
 pub fn descriptor(&self)->&Descriptor{&self.descriptor}
 /// The ABI owner registers this descriptor as hidden for Metadata's lifetime.
 pub fn backing_descriptor(&self)->std::os::fd::BorrowedFd<'_>{self.file.as_fd()}
 /// Header parsing and measurement are independent of data/tree length.
 pub fn measure(&self,capacity:usize)->Result<(u16,Vec<u8>)>{let digest=self.descriptor.digest();if capacity<digest.len(){return Err(Error::Linux(EOVERFLOW));}Ok((u16::from(self.descriptor.options().algorithm),digest))}
 pub fn read_metadata(&self,kind:u64,offset:u64,length:usize)->Result<Vec<u8>>{
  offset.checked_add(length as u64).ok_or(Error::Linux(EINVAL))?;
  let(start,size)=match kind{1=>(HEADER,self.tree_len),2=>(48,256),3 if self.signature_len>0=>(HEADER+self.tree_len,self.signature_len),3=>return Err(Error::Linux(ENODATA)),_=>return Err(Error::Linux(EINVAL))};
  if offset>=size{return Ok(Vec::new());}let length=(size-offset).min(length as u64).min(i32::MAX as u64) as usize;
  let mut output=Vec::new();output.try_reserve_exact(length).map_err(|_|Error::Linux(12))?;output.resize(length,0);tree::read_exact(&self.file,&mut output,start+offset)?;Ok(output)
 }
 /// Copy only bytes whose complete Merkle path was authenticated, never reread.
 pub fn verify_range(&self,data:&impl AsFd,offset:u64,length:usize)->Result<Vec<u8>>{
  if Identity::from_fd(data.as_fd())?!=self.identity{return Err(Error::Linux(EIO));}
  let size=self.descriptor.data_size();if tree::stat(data)?.st_size as u64!=size{return Err(Error::Linux(EIO));}
  let end=offset.checked_add(length as u64).ok_or(Error::Linux(EOVERFLOW))?.min(size);if offset>=end{return Ok(Vec::new());}
  let options=self.descriptor.options();let block=options.block_size as u64;let fanout=options.block_size/options.digest_size();let layout=self.descriptor.levels()?;
  let mut output=Vec::new();output.try_reserve_exact((end-offset) as usize).map_err(|_|Error::Linux(12))?;
  let mut bytes=vec![0;options.block_size];let mut node=vec![0;options.block_size];
  for index in offset/block..end.div_ceil(block){
   bytes.fill(0);let start=index*block;let available=(size-start).min(block) as usize;tree::read_exact(data,&mut bytes[..available],start)?;
   let mut digest=options.hash(&bytes);let mut at=index;
   for(level_offset,_)in &layout{
    tree::read_exact(&self.file,&mut node,HEADER+level_offset+(at/fanout as u64)*block)?;
    let entry=(at%fanout as u64)as usize*options.digest_size();if node[entry..entry+digest.len()]!=digest{return Err(Error::Linux(EIO));}
    digest=options.hash(&node);at/=fanout as u64;
   }
   if digest!=self.descriptor.root(){return Err(Error::Linux(EIO));}
   let lo=offset.saturating_sub(start) as usize;let hi=(end-start).min(block) as usize;output.extend_from_slice(&bytes[lo..hi]);
  }
  Ok(output)
 }
}

#[derive(Clone)]
pub struct Store{directory:PathBuf,runtime:PathBuf}
impl Store{
 pub fn new(directory:&Path,runtime:&Path)->Result<Self>{
  fs::create_dir_all(directory)?;
  if !fs::symlink_metadata(directory)?.is_dir(){return Err(Error::Linux(EINVAL));}
  Ok(Self{directory:directory.into(),runtime:runtime.into()})
 }
 pub fn lock_inode(&self,data:&impl AsFd)->Result<Admission>{
  let metadata=tree::stat(data)?;if metadata.st_mode&libc::S_IFMT==libc::S_IFDIR{return Err(Error::Linux(21));}if metadata.st_mode&libc::S_IFMT!=libc::S_IFREG{return Err(Error::Linux(EINVAL));}
  let inode=Inode::open(&self.runtime,data.as_fd())?;let admission=inode.admission()?;
  Ok(Admission{store:self.clone(),inode,admission})
 }
 fn path(&self,identity:Identity)->PathBuf{self.directory.join(hex(&binding(identity)))}
 pub fn lookup(&self,identity:Identity)->Result<Option<Metadata>>{
  match Metadata::open(&self.path(identity),&binding(identity)){Ok(metadata)=>Ok(Some(metadata)),Err(Error::Io(error))if error.kind()==std::io::ErrorKind::NotFound=>Ok(None),Err(error)=>Err(error)}
 }
 /// Publish the already validated, immutable prepared inode without copying
 /// or reconstructing any descriptor/tree/signature bytes.
 fn publish_with_sync(&self,identity:Identity,prepared:&Prepared,sync:impl FnOnce(&Path)->std::io::Result<()>)->Result<Metadata>{
  let view=prepared.metadata_view()?;
  if view.identity!=identity{return Err(Error::Linux(EIO));}
  let blob=Identity::from_fd(prepared.file.as_fd())?;
  let target=self.path(identity);let mut published=false;
  let result=(||{
   match fs::hard_link(&prepared._temporary.0,&target){Ok(())=>published=true,Err(error)if error.kind()==std::io::ErrorKind::AlreadyExists=>return Err(Error::Linux(17)),Err(error)=>return Err(error.into())}
   let metadata=Metadata::open(&target,&binding(identity))?;
   if Identity::from_fd(metadata.backing_descriptor())?!=blob{return Err(Error::Linux(EIO));}
   sync(&self.directory)?;
   Ok(metadata)
  })();
  if result.is_err()&&published{fs::remove_file(&target)?;PrivateFile::allocate(||File::open(&self.directory))?.sync_all()?;}
  result
 }

}
fn nonce()->Result<String>{
 static NEXT:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
 Ok(format!("{}-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_|Error::Linux(EIO))?.as_nanos(),NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed)))
}
fn hex(bytes:&[u8])->String{bytes.iter().map(|byte|format!("{byte:02x}")).collect()}
fn binding(identity:Identity)->[u8;36]{identity.to_bytes()}

pub struct Admission{store:Store,inode:Inode,admission:crate::inode_lease::Admission}
pub struct EnableGuard{store:Store,inode:Inode,_slot:EnableSlot,_writers:ExclusiveLease}
pub struct WriterExclusion{inode:Inode,writers:ExclusiveLease}
struct Temporary(PathBuf);
impl Drop for Temporary{fn drop(&mut self){if let Err(error)=fs::remove_file(&self.0){if error.kind()!=std::io::ErrorKind::NotFound{eprintln!("fs-verity temporary cleanup: {error}");}}}}
pub struct Prepared{file:PrivateFile,_temporary:Temporary,descriptor:Descriptor,identity:Identity}
impl Prepared{
 pub fn descriptor(&self)->&Descriptor{&self.descriptor}
 pub fn backing_descriptor(&self)->std::os::fd::BorrowedFd<'_>{self.file.as_fd()}
 /// Complete read-only proof before publication, kept alive by its real inode.
 pub fn metadata_view(&self)->Result<Metadata>{Metadata::from_file(self.file.try_clone()?,&binding(self.identity))}
}
impl Admission{
 pub fn identity(&self)->Identity{self.inode.identity()}
 pub fn enabled(&self)->Result<Option<Metadata>>{self.store.lookup(self.identity())}
 pub fn writer_lease(&self)->Result<WriterLease>{
  if self.enabled()?.is_some(){return Err(Error::Linux(1));}
  self.admission.writer().map_err(|error|if error.raw_os_error()==Some(libc::EBUSY){Error::Linux(26)}else{error.into()})
 }
 pub fn begin_enable(&self)->Result<EnableGuard>{
  self.begin_enable_excluded(self.exclude_writers()?)
 }
 pub fn exclude_writers(&self)->Result<WriterExclusion>{
  use std::os::fd::AsRawFd;
  let flags=unsafe{libc::fcntl(self.inode.source().as_raw_fd(),libc::F_GETFL)};
  if flags<0{return Err(std::io::Error::last_os_error().into());}
  if flags&libc::O_ACCMODE!=libc::O_RDONLY{return Err(Error::Linux(26));}
  let writers=self.admission.deny_writers().map_err(|error|if error.raw_os_error()==Some(libc::EBUSY){Error::Linux(26)}else{error.into()})?;
  Ok(WriterExclusion{inode:self.inode.clone(),writers})
 }
 pub fn begin_enable_excluded(&self,exclusion:WriterExclusion)->Result<EnableGuard>{
  if exclusion.inode.identity()!=self.identity()||exclusion.inode.directory()!=self.inode.directory(){return Err(Error::Linux(EINVAL));}
  if self.enabled()?.is_some(){return Err(Error::Linux(17));}
  let slot=self.admission.enable_slot().map_err(|error|if error.raw_os_error()==Some(libc::EBUSY){Error::Linux(16)}else{error.into()})?;
  Ok(EnableGuard{store:self.store.clone(),inode:self.inode.clone(),_slot:slot,_writers:exclusion.writers})
 }
}
impl EnableGuard{
 pub fn identity(&self)->Identity{self.inode.identity()}
 pub fn build(&self,options:super::BuildOptions,signature:&[u8],interrupted:impl FnMut()->bool)->Result<Prepared>{
  if signature.len()>16128{return Err(Error::Linux(super::EMSGSIZE));}
  let path=self.store.directory.join(format!("build-{}.tmp",nonce()?));
  let file=PrivateFile::allocate(||File::options().create_new(true).read(true).write(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(&path))?;
  let temporary=Temporary(path);
  let source=self.inode.source();
  let before=tree::stat(&source)?;
  let descriptor=super::build(&source,&file,HEADER,options,interrupted)?;
  let after=tree::stat(&source)?;
  if (before.st_size,before.st_mtime,before.st_mtime_nsec,before.st_ctime,before.st_ctime_nsec)!=(after.st_size,after.st_mtime,after.st_mtime_nsec,after.st_ctime,after.st_ctime_nsec){
   return Err(Error::Linux(EIO));
  }
  let layout=descriptor.levels()?;let tree_len=layout.iter().try_fold(0u64,|sum,(_,length)|sum.checked_add(*length).ok_or(Error::Linux(EOVERFLOW)))?;
  let mut header=[0;HEADER as usize];header[..8].copy_from_slice(MAGIC);header[8..44].copy_from_slice(&binding(self.identity()));header[48..304].copy_from_slice(descriptor.bytes());header[304..312].copy_from_slice(&tree_len.to_le_bytes());header[312..320].copy_from_slice(&(signature.len()as u64).to_le_bytes());
  let digest=descriptor.digest();header[320..320+digest.len()].copy_from_slice(&digest);let checksum=Sha256::digest(&header[..384]);header[384..416].copy_from_slice(&checksum);
  tree::write_all(&file,&header,0)?;tree::write_all(&file,signature,HEADER+tree_len)?;file.sync_all()?;
  let blob=Identity::from_fd(file.as_fd())?;drop(file);
  let metadata=Metadata::open(&temporary.0,&binding(self.identity()))?;
  if Identity::from_fd(metadata.backing_descriptor())?!=blob{return Err(Error::Linux(EIO));}
  Ok(Prepared{file:metadata.file,_temporary:temporary,descriptor,identity:self.identity()})
 }
 pub fn commit(self,prepared:Prepared)->Result<Metadata>{
  let _admission=self.inode.admission()?;
  if prepared.identity!=self.identity()||Identity::from_fd(self.inode.source())?!=self.identity(){return Err(Error::Linux(EIO));}
  let source=self.inode.source();
  if tree::stat(&source)?.st_size as u64!=prepared.descriptor.data_size(){return Err(Error::Linux(EIO));}
  self.store.publish_with_sync(self.identity(),&prepared,|directory|PrivateFile::allocate(||File::open(directory))?.sync_all())
 }
}

#[cfg(test)]
mod tests{
 use super::*;
 use std::os::unix::fs::FileExt;
 struct Data(PathBuf);
 impl Data{fn new()->Self{static NEXT:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);let path=std::env::temp_dir().join(format!("aim-verity-store-{}-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed)));fs::create_dir(&path).unwrap();Self(path)}fn store(&self)->Store{Store::new(&self.0.join("persistent"),&self.0.join("runtime")).unwrap()}fn data(&self)->File{File::open(self.0.join("data")).unwrap()}}
 impl Drop for Data{fn drop(&mut self){fs::remove_dir_all(&self.0).unwrap();}}
 fn proof(store:&Store,data:&File)->Metadata{
  let admission=store.lock_inode(data).unwrap();let guard=admission.begin_enable().unwrap();drop(admission);
  let prepared=guard.build(super::super::BuildOptions::new(1,4096,vec![9],16384,4096).unwrap(),&[],||false).unwrap();guard.commit(prepared).unwrap()
 }
 #[test]
 fn private_metadata_import_retains_actual_readonly_proof_and_rejects_invalid_capabilities(){
  let files=Data::new();fs::write(files.0.join("data"),vec![9;8193]).unwrap();let data=files.data();let store=files.store();
  let admission=store.lock_inode(&data).unwrap();let guard=admission.begin_enable().unwrap();drop(admission);
  let prepared=guard.build(super::super::BuildOptions::new(1,4096,vec![],4096,4096).unwrap(),b"admitted signature",||false).unwrap();let view=prepared.metadata_view().unwrap();
  let identity=guard.identity();let imported=Metadata::from_private_fd(view.file.try_clone().unwrap().into_private_fd(),identity).unwrap();
  assert_ne!(Identity::from_fd(imported.backing_descriptor()).unwrap(),identity,"proof and data inode identities are distinct");
  assert_eq!(imported.measure(64).unwrap(),view.measure(64).unwrap());assert_eq!(imported.read_metadata(3,0,100).unwrap(),b"admitted signature");assert_eq!(imported.verify_range(&data,4093,17).unwrap(),vec![9;17]);
  let writable=PrivateFd::allocate(||Ok(File::options().read(true).write(true).open(&prepared._temporary.0)?.into())).unwrap();
  assert!(matches!(Metadata::from_private_fd(writable,identity),Err(Error::Linux(EINVAL))));
  let mut wrong=identity;wrong.generation=wrong.generation.wrapping_add(1);
  assert!(matches!(Metadata::from_private_fd(view.file.try_clone().unwrap().into_private_fd(),wrong),Err(Error::Linux(EIO))));
  let path=files.0.join("truncated-proof");fs::write(&path,b"AIMVRT02").unwrap();let truncated=PrivateFd::allocate(||Ok(File::open(path)?.into())).unwrap();
  assert!(matches!(Metadata::from_private_fd(truncated,identity),Err(Error::Linux(EIO))));
  drop(prepared);drop(view);assert_eq!(imported.verify_range(&data,0,1).unwrap(),[9]);
 }
 #[test]
 fn prepared_readonly_metadata_is_complete_and_commit_publishes_the_same_blob(){
  use std::os::fd::AsRawFd;
  let files=Data::new();let bytes=(0..8193).map(|index|(index%251)as u8).collect::<Vec<_>>();fs::write(files.0.join("data"),&bytes).unwrap();let data=files.data();let store=files.store();
  let admission=store.lock_inode(&data).unwrap();let guard=admission.begin_enable().unwrap();drop(admission);
  let prepared=guard.build(super::super::BuildOptions::new(1,4096,vec![9],4096,4096).unwrap(),b"admitted signature",||false).unwrap();
  let view=prepared.metadata_view().unwrap();assert!(store.lookup(guard.identity()).unwrap().is_none(),"a proof view is not ENABLE publication");
  assert_eq!(view.read_metadata(2,0,256).unwrap(),prepared.descriptor().bytes());assert_eq!(view.read_metadata(3,0,100).unwrap(),b"admitted signature");
  assert_eq!(view.verify_range(&data,4087,4106).unwrap(),bytes[4087..8193]);
  let measured=view.measure(64).unwrap();let blob=Identity::from_fd(view.backing_descriptor()).unwrap();
  let length=tree::stat(&view.file).unwrap().st_size as usize;let mut before=vec![0;length];tree::read_exact(&view.file,&mut before,0).unwrap();
  for fd in [prepared.backing_descriptor(),view.backing_descriptor()]{
   assert_eq!(unsafe{libc::fcntl(fd.as_raw_fd(),libc::F_GETFL)}&libc::O_ACCMODE,libc::O_RDONLY);
   assert_eq!(unsafe{libc::pwrite(fd.as_raw_fd(),b"X".as_ptr().cast(),1,0)},-1);assert_eq!(std::io::Error::last_os_error().raw_os_error(),Some(libc::EBADF));
  }
  let committed=guard.commit(prepared).unwrap();assert_eq!(Identity::from_fd(committed.backing_descriptor()).unwrap(),blob);
  let mut after=vec![0;length];tree::read_exact(&committed.file,&mut after,0).unwrap();assert_eq!(before,after);
  assert_eq!(committed.measure(64).unwrap(),measured);assert_eq!(committed.verify_range(&data,4087,4106).unwrap(),bytes[4087..8193]);assert_eq!(view.measure(64).unwrap(),measured);
  assert_eq!(fs::read_dir(&store.directory).unwrap().count(),1,"only the final immutable blob remains");
 }
 #[test]
 fn abort_removes_private_prepared_path_without_revoking_retained_proof(){
  let files=Data::new();fs::write(files.0.join("data"),b"private proof").unwrap();let data=files.data();let store=files.store();
  let admission=store.lock_inode(&data).unwrap();let guard=admission.begin_enable().unwrap();drop(admission);
  let prepared=guard.build(super::super::BuildOptions::new(1,4096,vec![],4096,4096).unwrap(),&[],||false).unwrap();let view=prepared.metadata_view().unwrap();
  assert_eq!(fs::read_dir(&store.directory).unwrap().count(),1);drop(prepared);
  assert_eq!(fs::read_dir(&store.directory).unwrap().count(),0);assert!(store.lookup(guard.identity()).unwrap().is_none());
  assert_eq!(view.verify_range(&data,0,13).unwrap(),b"private proof");assert_eq!(view.read_metadata(2,0,256).unwrap(),view.descriptor().bytes());drop(guard);
  let admission=store.lock_inode(&data).unwrap();assert!(admission.begin_enable().is_ok(),"aborted proof releases both exclusive owners");
 }
 #[test]
 fn persistent_inode_binding_and_verified_range_survive_rename_and_restart(){
  let files=Data::new();let bytes=(0..4096*129+17).map(|i|(i%251)as u8).collect::<Vec<_>>();fs::write(files.0.join("data"),&bytes).unwrap();let data=files.data();let identity=Identity::from_fd(data.as_fd()).unwrap();let store=files.store();let metadata=proof(&store,&data);
  assert_eq!(metadata.verify_range(&data,4091,8211).unwrap(),bytes[4091..4091+8211]);assert!(matches!(metadata.measure(1),Err(Error::Linux(EOVERFLOW))));
  let foreign=files.0.join("foreign");fs::write(&foreign,&bytes).unwrap();
  assert!(matches!(metadata.verify_range(&File::open(foreign).unwrap(),0,1),Err(Error::Linux(EIO))));
  let digest=metadata.measure(64).unwrap();fs::rename(files.0.join("data"),files.0.join("renamed")).unwrap();fs::hard_link(files.0.join("renamed"),files.0.join("alias")).unwrap();
  let alias=File::open(files.0.join("alias")).unwrap();assert_eq!(Identity::from_fd(alias.as_fd()).unwrap(),identity);
  drop(metadata);drop(store);fs::remove_dir_all(files.0.join("runtime")).unwrap();let restarted=files.store();assert_eq!(restarted.lookup(identity).unwrap().unwrap().measure(64).unwrap(),digest);
  fs::write(files.0.join("data"),b"new inode").unwrap();let replacement=files.data();assert!(restarted.lookup(Identity::from_fd(replacement.as_fd()).unwrap()).unwrap().is_none());
  let mut changed=identity;changed.generation=changed.generation.wrapping_add(1);assert!(restarted.lookup(changed).unwrap().is_none());
 }
 #[test]
 fn corruption_truncation_and_metadata_ranges_are_real_errors(){
  let files=Data::new();fs::write(files.0.join("data"),vec![7;8193]).unwrap();let data=files.data();let store=files.store();let identity=Identity::from_fd(data.as_fd()).unwrap();let metadata=proof(&store,&data);
  assert_eq!(metadata.read_metadata(2,0,1024).unwrap(),metadata.descriptor().bytes());assert!(metadata.read_metadata(1,u64::MAX,1).is_err());assert!(matches!(metadata.read_metadata(3,0,1),Err(Error::Linux(ENODATA))));assert!(metadata.read_metadata(2,256,10).unwrap().is_empty());
  let corrupt=File::options().write(true).open(files.0.join("data")).unwrap();corrupt.write_at(&[8],4097).unwrap();assert!(matches!(metadata.verify_range(&data,4096,1),Err(Error::Linux(EIO))));assert_eq!(metadata.verify_range(&data,0,1).unwrap(),[7]);
  let tree=File::options().write(true).open(store.path(identity)).unwrap();tree.write_at(&[255],HEADER).unwrap();assert!(matches!(metadata.verify_range(&data,0,1),Err(Error::Linux(EIO))));
  // Measurement reads only the fixed header, irrespective of corrupted tree pages.
  assert!(store.lookup(identity).unwrap().unwrap().measure(64).is_ok());
  tree.set_len(HEADER).unwrap();assert!(matches!(store.lookup(identity),Err(Error::Linux(EIO))));
  tree.write_at(&[1],40).unwrap();assert!(store.lookup(identity).is_err());
 }
 #[test]
 fn interrupted_build_removes_its_owned_temporary_file(){
  let files=Data::new();fs::write(files.0.join("data"),vec![1;8193]).unwrap();let data=files.data();let store=files.store();
  let admission=store.lock_inode(&data).unwrap();let guard=admission.begin_enable().unwrap();drop(admission);
  let result=guard.build(super::super::BuildOptions::new(1,4096,vec![],4096,4096).unwrap(),&[],||true);
  assert!(matches!(result,Err(Error::Linux(4))));assert_eq!(fs::read_dir(&store.directory).unwrap().count(),0);
  assert!(store.lookup(guard.identity()).unwrap().is_none());
  let mut shortened=false;
  let result=guard.build(super::super::BuildOptions::new(1,4096,vec![],4096,4096).unwrap(),&[],||{
   if !shortened{File::options().write(true).open(files.0.join("data")).unwrap().set_len(0).unwrap();shortened=true;}false
  });
  assert!(matches!(result,Err(Error::Linux(EIO))));assert_eq!(fs::read_dir(&store.directory).unwrap().count(),0);
 }
 #[test]
 fn failed_directory_commit_rolls_back_enabled_record(){
  let files=Data::new();fs::write(files.0.join("data"),b"unchanged original").unwrap();let data=files.data();let store=files.store();
  let admission=store.lock_inode(&data).unwrap();let guard=admission.begin_enable().unwrap();drop(admission);
  let prepared=guard.build(super::super::BuildOptions::new(1,4096,vec![],4096,4096).unwrap(),b"stored signature",||false).unwrap();
  let result=store.publish_with_sync(guard.identity(),&prepared,|_|Err(std::io::Error::from_raw_os_error(libc::EIO)));
  assert!(matches!(result,Err(Error::Io(_))));assert!(store.lookup(guard.identity()).unwrap().is_none());
  assert_eq!(fs::read_dir(&store.directory).unwrap().count(),1,"only the still-owned Prepared build remains");assert_eq!(fs::read(files.0.join("data")).unwrap(),b"unchanged original");
  let metadata=guard.commit(prepared).unwrap();assert_eq!(metadata.read_metadata(3,0,100).unwrap(),b"stored signature");
 }
 #[test]
 fn admission_distinguishes_writers_enablers_and_enabled_state(){
  let files=Data::new();fs::write(files.0.join("data"),b"content").unwrap();let data=files.data();let store=files.store();
  let writable=File::options().read(true).write(true).open(files.0.join("data")).unwrap();let admission=store.lock_inode(&writable).unwrap();let writer=admission.writer_lease().unwrap();drop(admission);
  let admission=store.lock_inode(&data).unwrap();assert!(matches!(admission.begin_enable(),Err(Error::Linux(26))));drop(writer);
  let guard=admission.begin_enable().unwrap();drop(admission);
  let other=store.lock_inode(&data).unwrap();assert!(matches!(other.begin_enable(),Err(Error::Linux(16))));assert!(matches!(other.writer_lease(),Err(Error::Linux(26))));drop(other);
  let prepared=guard.build(super::super::BuildOptions::new(1,4096,vec![],4096,4096).unwrap(),&[],||false).unwrap();guard.commit(prepared).unwrap();
  let admission=store.lock_inode(&data).unwrap();assert!(matches!(admission.begin_enable(),Err(Error::Linux(17))));assert!(matches!(admission.writer_lease(),Err(Error::Linux(1))));
 }
 #[test]
 fn exclusion_cannot_authorize_a_different_inode_or_lease_domain(){
  let files=Data::new();fs::write(files.0.join("data"),b"content").unwrap();fs::write(files.0.join("other"),b"content").unwrap();
  let data=files.data();let other=File::open(files.0.join("other")).unwrap();let store=files.store();
  let admission=store.lock_inode(&data).unwrap();let exclusion=admission.exclude_writers().unwrap();drop(admission);
  let wrong=store.lock_inode(&other).unwrap();assert!(matches!(wrong.begin_enable_excluded(exclusion),Err(Error::Linux(EINVAL))));drop(wrong);
  let admission=store.lock_inode(&data).unwrap();let exclusion=admission.exclude_writers().unwrap();drop(admission);
  let foreign=Store::new(&files.0.join("foreign-proof"),&files.0.join("foreign-runtime")).unwrap();
  let wrong=foreign.lock_inode(&data).unwrap();assert!(matches!(wrong.begin_enable_excluded(exclusion),Err(Error::Linux(EINVAL))));
 }
}
