//! Shared mount namespace generation journal (#1158). Mutations are serialized
//! under a distinct lock inode so atomic state replacement never drops the lock.
use std::{fs,io::{self,Read,Write},os::{fd::AsRawFd,unix::fs::OpenOptionsExt},path::{Path,PathBuf}};
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct State {pub generation:u64,pub base:String,pub events:Vec<String>,pub mounts:Vec<MountRecord>}
mod records;
pub use records::MountRecord;
#[derive(Clone,Debug)]
pub struct Namespace {runtime:PathBuf,id:String}
pub struct Generation {address:*mut std::sync::atomic::AtomicU64}
unsafe impl Send for Generation{} unsafe impl Sync for Generation{}
impl Generation {pub fn current(&self)->u64{unsafe{(*self.address).load(std::sync::atomic::Ordering::Acquire)}}}
impl Drop for Generation{fn drop(&mut self){unsafe{libc::munmap(self.address.cast(),4096);}}}
impl Namespace {
 pub fn open(runtime:&Path,id:&str)->io::Result<Self>{if id.is_empty()||id.contains(['/', '\0','\n','\t']){return Err(io::Error::from_raw_os_error(libc::EINVAL));}Ok(Self{runtime:runtime.into(),id:id.into()})}
 pub fn id(&self)->&str{&self.id}
 pub fn runtime(&self)->&Path{&self.runtime}
 pub fn generation(&self)->io::Result<Generation>{
  let file=fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(self.directory().join(format!("{}.generation",self.id)))?;
  file.set_len(4096)?;
  let address=unsafe{libc::mmap(std::ptr::null_mut(),4096,libc::PROT_READ|libc::PROT_WRITE,libc::MAP_SHARED,file.as_raw_fd(),0)};
  if address==libc::MAP_FAILED{return Err(io::Error::last_os_error());}Ok(Generation{address:address.cast()})
 }
 fn publish_generation(&self,generation:u64)->io::Result<()>{
  let page=self.generation()?;unsafe{(*page.address).store(generation,std::sync::atomic::Ordering::Release);}
  if unsafe{libc::msync(page.address.cast(),4096,libc::MS_SYNC)}<0{return Err(io::Error::last_os_error());}Ok(())
 }
 fn directory(&self)->PathBuf{self.runtime.join("mount-namespaces")}
 fn locked<T>(&self,action:impl FnOnce()->io::Result<T>)->io::Result<T>{
  fs::create_dir_all(self.directory())?;
  let file=fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(self.directory().join(format!("{}.lock",self.id)))?;
  loop{if unsafe{libc::flock(file.as_raw_fd(),libc::LOCK_EX)}==0{break;}let error=io::Error::last_os_error();if error.kind()!=io::ErrorKind::Interrupted{return Err(error);}}
  action()
 }
 fn path(&self)->PathBuf{self.directory().join(format!("{}.state",self.id))}
 fn read_unlocked(&self)->io::Result<State>{
  let mut bytes=Vec::new();fs::File::open(self.path())?.read_to_end(&mut bytes)?;
  let mut at=0usize;
  let take=|at:&mut usize,n:usize|->io::Result<&[u8]>{let end=at.checked_add(n).ok_or_else(protocol)?;let value=bytes.get(*at..end).ok_or_else(protocol)?;*at=end;Ok(value)};
  if take(&mut at,8)?!=b"AIMMNT3\0"{return Err(protocol());}
  let generation=u64::from_le_bytes(take(&mut at,8)?.try_into().unwrap());
  let text=|at:&mut usize|->io::Result<String>{let n=u64::from_le_bytes(take(at,8)?.try_into().unwrap());let n=usize::try_from(n).map_err(|_|protocol())?;String::from_utf8(take(at,n)?.to_vec()).map_err(|_|protocol())};
  let base=text(&mut at)?;let count=u64::from_le_bytes(take(&mut at,8)?.try_into().unwrap());
  if count>bytes.len() as u64/8{return Err(protocol());}let mut events=Vec::new();for _ in 0..count{events.push(text(&mut at)?);}
  let count=u64::from_le_bytes(take(&mut at,8)?.try_into().unwrap());if count>bytes.len()as u64/32{return Err(protocol());}
  let mut mounts=Vec::new();for _ in 0..count{let id=u64::from_le_bytes(take(&mut at,8)?.try_into().unwrap());let origin=u64::from_le_bytes(take(&mut at,8)?.try_into().unwrap());let parent=u64::from_le_bytes(take(&mut at,8)?.try_into().unwrap());let shared=u64::from_le_bytes(take(&mut at,8)?.try_into().unwrap());let master=u64::from_le_bytes(take(&mut at,8)?.try_into().unwrap());mounts.push(MountRecord{id,origin,parent,shared,master,kind:text(&mut at)?,guest:text(&mut at)?,host:text(&mut at)?,source:text(&mut at)?,fstype:text(&mut at)?,root:text(&mut at)?,own:take(&mut at,1)?[0]!=0});}
  if at!=bytes.len()||generation==0{return Err(protocol());}records::validate(&mounts)?;Ok(State{generation,base,events,mounts})
 }
 fn write_unlocked(&self,state:&State)->io::Result<()>{
  let mut bytes=b"AIMMNT3\0".to_vec();bytes.extend(state.generation.to_le_bytes());
  fn text(bytes:&mut Vec<u8>,value:&str){bytes.extend((value.len() as u64).to_le_bytes());bytes.extend(value.as_bytes());}
  text(&mut bytes,&state.base);bytes.extend((state.events.len() as u64).to_le_bytes());for event in &state.events{text(&mut bytes,event);}
  bytes.extend((state.mounts.len()as u64).to_le_bytes());for mount in &state.mounts{bytes.extend(mount.id.to_le_bytes());bytes.extend(mount.origin.to_le_bytes());bytes.extend(mount.parent.to_le_bytes());bytes.extend(mount.shared.to_le_bytes());bytes.extend(mount.master.to_le_bytes());for value in [&mount.kind,&mount.guest,&mount.host,&mount.source,&mount.fstype,&mount.root]{text(&mut bytes,value);}bytes.push(u8::from(mount.own));}
  let temporary=self.path().with_extension(format!("{}.tmp",std::process::id()));
  let result=(||{let mut file=fs::OpenOptions::new().write(true).create_new(true).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC).open(&temporary)?;file.write_all(&bytes)?;file.sync_all()?;fs::rename(&temporary,self.path())?;fs::File::open(self.directory())?.sync_all()?;self.publish_generation(state.generation)})();
  if result.is_err(){let _=fs::remove_file(temporary);}result
 }
 pub fn initialize(&self,base:&str)->io::Result<State>{self.locked(||match self.read_unlocked(){Ok(state)=>{self.publish_generation(state.generation)?;Ok(state)},Err(error)if error.kind()==io::ErrorKind::NotFound=>{let mounts=records::initialize(self,base)?;let state=State{generation:1,base:base.into(),events:Vec::new(),mounts};self.write_unlocked(&state)?;Ok(state)},Err(error)=>Err(error)})}
 pub fn read(&self)->io::Result<State>{self.locked(||self.read_unlocked())}
 pub fn append(&self,event:&str)->io::Result<State>{self.locked(||{let mut state=self.read_unlocked()?;state.generation=state.generation.checked_add(1).ok_or_else(protocol)?;records::apply(self,&mut state.mounts,event)?;state.events.push(event.into());self.write_unlocked(&state)?;Ok(state)})}
 /// Native init changes its actual mapped roots while preserving guest mutations.
 pub fn update_base(&self,base:&str)->io::Result<State>{self.locked(||{let mut state=self.read_unlocked()?;if state.base!=base{records::replace_base(self,&mut state.mounts,&state.base,base)?;state.base=base.into();state.generation=state.generation.checked_add(1).ok_or_else(protocol)?;self.write_unlocked(&state)?;}Ok(state)})}
 pub fn bind_base(&self,base:&str,target:&str)->io::Result<State>{self.locked(||{let mut state=self.read_unlocked()?;records::bind_base(self,&mut state.mounts,&state.base,base,target)?;state.base=base.into();state.generation=state.generation.checked_add(1).ok_or_else(protocol)?;self.write_unlocked(&state)?;Ok(state)})}
 pub fn propagated_group(&self,source:u64,parent_group:u64)->io::Result<u64>{records::propagated_group(self,source,parent_group)}
 pub fn sync_projections(&self,live:&[MountRecord])->io::Result<State>{self.locked(||{let mut state=self.read_unlocked()?;if records::sync_projections(self,&mut state.mounts,live)?{state.generation=state.generation.checked_add(1).ok_or_else(protocol)?;self.write_unlocked(&state)?;}Ok(state)})}
 pub fn clone_to(&self,id:&str)->io::Result<Self>{let mut state=self.read()?;records::clone_mounts(self,&mut state.mounts)?;let target=Self::open(&self.runtime,id)?;target.locked(||{if target.path().exists(){return Err(io::Error::from_raw_os_error(libc::EEXIST));}target.write_unlocked(&state)})?;Ok(target)}
}
fn protocol()->io::Error{io::Error::from_raw_os_error(libc::EPROTO)}
#[cfg(test)]
mod tests {
 use super::*;
 #[test]
 fn different_children_of_shared_parent_are_not_peers_but_propagated_copy_is(){
  let root=std::env::temp_dir().join(format!("aim-mount-peer-{}",std::process::id()));let _=fs::remove_dir_all(&root);fs::create_dir_all(root.join("image")).unwrap();fs::create_dir_all(root.join("one")).unwrap();fs::create_dir_all(root.join("two")).unwrap();
  let first=Namespace::open(&root,"source").unwrap();first.initialize(&format!("root\t/\t{}\npropagation\t/\tshared\n",root.join("image").display())).unwrap();let second=first.clone_to("receiver").unwrap();
  for name in ["one","two"]{first.append(&format!("rw\t/{name}\t{}\ttmpfs\ttmpfs\n",root.join(name).display())).unwrap();}
  let state=first.read().unwrap();let parent=state.mounts.iter().find(|mount|mount.guest=="/").unwrap();let one=state.mounts.iter().find(|mount|mount.guest=="/one").unwrap();let two=state.mounts.iter().find(|mount|mount.guest=="/two").unwrap();
  assert_ne!(one.shared,parent.shared);assert_ne!(two.shared,parent.shared);assert_ne!(one.shared,two.shared);assert_ne!(one.shared,0);
  let mut incoming=one.clone();incoming.id=0;incoming.parent=0;incoming.origin=one.id;incoming.kind="projected".into();let copy=second.sync_projections(&[incoming]).unwrap();let copy=copy.mounts.iter().find(|mount|mount.guest=="/one").unwrap();assert_ne!(copy.id,one.id);assert_eq!(copy.shared,one.shared);
  fs::remove_dir_all(root).unwrap();
 }
 #[test]
 fn mount_objects_keep_ids_until_unmount_and_namespace_clone_allocates_new_objects(){
  let root=std::env::temp_dir().join(format!("aim-mount-identities-{}",std::process::id()));let _=fs::remove_dir_all(&root);fs::create_dir_all(root.join("image")).unwrap();fs::create_dir_all(root.join("source")).unwrap();
  let owner=Namespace::open(&root,"first").unwrap();let base=format!("root\t/\t{}\nrw\t/apex\t{}\npropagation\t/\tshared\npropagation\t/apex\tprivate\n",root.join("image").display(),root.join("source").display());let first=owner.initialize(&base).unwrap();
  assert_eq!(first,owner.read().unwrap());let cloned=owner.clone_to("second").unwrap().read().unwrap();
  assert!(cloned.mounts.iter().all(|mount|!first.mounts.iter().any(|old|old.id==mount.id)));
  assert_eq!(cloned.mounts[0].id,cloned.mounts[0].parent);assert_eq!(cloned.mounts[0].shared,first.mounts[0].shared);assert_eq!(cloned.mounts[1].shared,0);
  owner.append(&format!("rw\t/attached\t{}\ttmpfs\ttmpfs\n",root.join("source").display())).unwrap();let id=owner.read().unwrap().mounts.iter().find(|mount|mount.guest=="/attached").unwrap().id;
  owner.append("move\t/attached\t/moved").unwrap();assert_eq!(owner.read().unwrap().mounts.iter().find(|mount|mount.guest=="/moved").unwrap().id,id);
  owner.append("remove\t/moved").unwrap();assert!(!owner.read().unwrap().mounts.iter().any(|mount|mount.id==id));
  owner.append(&format!("rw\t/moved\t{}\ttmpfs\ttmpfs\n",root.join("source").display())).unwrap();assert_ne!(owner.read().unwrap().mounts.iter().find(|mount|mount.guest=="/moved").unwrap().id,id);fs::remove_dir_all(root).unwrap();
 }
 struct Child(std::process::Child);
 impl Drop for Child{fn drop(&mut self){if self.0.try_wait().ok().flatten().is_none(){let _=self.0.kill();}let _=self.0.wait();}}
 #[test]
 #[ignore="subprocess helper exercised by cross_process_commit_updates_existing_generation_mapping"]
 fn namespace_child(){
  use std::io::BufRead;
  let mut input=io::BufReader::new(io::stdin());let mut root=String::new();let mut id=String::new();input.read_line(&mut root).unwrap();input.read_line(&mut id).unwrap();
  Namespace::open(Path::new(root.trim()),id.trim()).unwrap().append("rw\t/child-real-mount\t/actual-child-host\ttmpfs\ttmpfs\n").unwrap();
 }
 #[test]
 fn cross_process_commit_updates_existing_generation_mapping(){
  let root=std::env::temp_dir().join(format!("aim-mount-child-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));fs::create_dir(&root).unwrap();
  let owner=Namespace::open(&root,"shared-child").unwrap();owner.initialize("root\t/\t/actual-root\n").unwrap();let generation=owner.generation().unwrap();let before=generation.current();
  let mut child=Child(std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","mount_namespace::tests::namespace_child","--ignored","--nocapture"]).stdin(std::process::Stdio::piped()).spawn().unwrap());
  writeln!(child.0.stdin.take().unwrap(),"{}\n{}",root.display(),owner.id()).unwrap();assert!(child.0.wait().unwrap().success());
  assert!(generation.current()>before);assert_eq!(owner.read().unwrap().events.len(),1);drop(generation);fs::remove_dir_all(root).unwrap();
 }
 #[test]
 fn shared_generation_and_private_clone_preserve_actual_mutation_order(){
  let root=std::env::temp_dir().join(format!("aim-mount-owner-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));fs::create_dir(&root).unwrap();
  let first=Namespace::open(&root,"initial").unwrap();first.initialize("root\t/\t/image\n").unwrap();
  let second=Namespace::open(&root,"initial").unwrap();first.append("mount\t/a\tactual-first").unwrap();second.append("mount\t/b\tactual-second").unwrap();
  assert_eq!(first.read().unwrap().events,vec!["mount\t/a\tactual-first","mount\t/b\tactual-second"]);
  let private=first.clone_to("private").unwrap();first.append("remove\t/a").unwrap();private.append("mount\t/c\tactual-private").unwrap();
  assert_eq!(first.read().unwrap().events.len(),3);assert_eq!(private.read().unwrap().events.len(),3);assert_ne!(first.read().unwrap().events,private.read().unwrap().events);
  first.update_base("root\t/\t/new-root\n").unwrap();assert_eq!(second.read().unwrap().base,"root\t/\t/new-root\n");assert_eq!(private.read().unwrap().base,"root\t/\t/image\n");
  fs::remove_dir_all(root).unwrap();
 }
}
