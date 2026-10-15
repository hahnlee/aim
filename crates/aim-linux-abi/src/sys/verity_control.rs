//! Process-local authenticated control worker; prepared VM guards never migrate.
use aim_storage::{fsverity::{Identity,Metadata},verity_control::{Channel,Frame,Operation}};
use std::{io,sync::Arc,os::fd::AsRawFd};
fn error(code:i32)->io::Error{io::Error::from_raw_os_error(code)}
/// Runs on one dedicated native thread. Imported proofs keep hidden descriptors
/// throughout the actual pager prepare/commit/abort lifetime.
pub fn worker(channel:Channel)->io::Result<()>{worker_inner(channel,None)}
pub fn worker_with_store(channel:Channel,store:Arc<aim_storage::fsverity::Store>)->io::Result<()>{worker_inner(channel,Some(store))}
fn worker_inner(mut channel:Channel,store:Option<Arc<aim_storage::fsverity::Store>>)->io::Result<()> {
 let mut pending:Option<(u64,[u8;36],super::verity_pager::PreparedMapsGuard)>=None;
 let mut pending_digest=Vec::new();
 loop {
  let(frame,proof)=match channel.receive(){Ok(value)=>value,Err(failure)if pending.is_none()&&matches!(failure.raw_os_error(),Some(libc::ETIMEDOUT)|Some(libc::EAGAIN))=>continue,Err(failure)=>{if let Some((_,identity,guard))=pending.take(){if let Some(store)=&store{let identity=Identity::from_bytes(&identity)?;match store.lookup(identity){Ok(Some(marker))if marker.descriptor().digest()==pending_digest=>guard.commit()?,Ok(None)=>guard.abort()?,Ok(Some(_))|Err(_)=>{guard.fail_closed()?;return Err(error(libc::EIO));}}}else{guard.abort()?;}}return Err(failure);}};
  let result=match frame.operation {
   Operation::Prepare=>{
    if pending.is_some(){Err(error(libc::EBUSY))}else{
     let identity=Identity::from_bytes(&frame.identity).map_err(|_|error(libc::EINVAL))?;
     let metadata=Arc::new(Metadata::from_private_fd(proof.ok_or_else(||error(libc::EPROTO))?,identity).map_err(|failure|match failure{aim_storage::fsverity::Error::Linux(code)=>error(code),aim_storage::fsverity::Error::Io(error)=>error})?);
     pending_digest=metadata.descriptor().digest();
     match super::verity_pager::prepare_enable(identity,metadata){Ok(guard)=>{pending=Some((frame.transaction,frame.identity,guard));Ok(Operation::Prepared)},Err(failure)=>Err(failure)}
    }
   },
   Operation::Abort|Operation::Commit=>{
    if proof.is_some(){return Err(error(libc::EPROTO));}
    match pending.take(){Some((transaction,identity,guard))if transaction==frame.transaction&&identity==frame.identity=>{
     if frame.operation==Operation::Abort{guard.abort().map(|()|Operation::Aborted)}else{guard.commit().map(|()|Operation::Committed)}
    },Some(previous)=>{pending=Some(previous);Err(error(libc::EPROTO))},None=>Err(error(libc::ENOENT))}
   },
   _=>Err(error(libc::EPROTO))
  };
  let(operation,code)=match result{Ok(operation)=>(operation,0),Err(failure)=>(Operation::Error,failure.raw_os_error().unwrap_or(libc::EIO))};
  channel.send(&Frame{operation,error:code,..frame},None)?;
 }
}
#[cfg(test)]
mod tests {
 use super::*;use std::{fs,os::fd::{AsRawFd,FromRawFd},os::unix::net::UnixListener,process::{Command,Stdio},time::Duration};
 #[test]
 #[ignore="owned real pager subprocess invoked by cross_process_prepare_abort_preserves_cow"]
 fn pager_child(){
  let Some(root)=std::env::args().find_map(|a|a.strip_prefix("--verity-root=").map(std::path::PathBuf::from))else{return;};
  let file=fs::File::open(root.join("data")).unwrap();let memory=unsafe{libc::mmap(std::ptr::null_mut(),32768,libc::PROT_READ|libc::PROT_WRITE,libc::MAP_PRIVATE,file.as_raw_fd(),0)};assert_ne!(memory,libc::MAP_FAILED);unsafe{std::ptr::write_volatile(memory.cast::<u8>(),91)};
  let data=aim_storage::private_fd::PrivateFd::adopt(file.into()).unwrap();
  super::super::verity_pager::track_existing(memory as u64,32768,0,libc::PROT_READ|libc::PROT_WRITE,false,Arc::new(super::super::verity_pager::Source{data,proof:None,description:None,cache:Arc::new(super::super::verity_pager::MappingCache::new(&root.join("cache")).unwrap()),extent:None,derivative:None})).unwrap();
  let registered=std::env::args().any(|a|a=="--registered-control");
  let mut channel=Channel::new(std::os::unix::net::UnixStream::connect(root.join("control")).unwrap(),None,Duration::from_secs(2)).unwrap();
  if registered{let registration=channel.receive().unwrap().0;assert_eq!(registration.operation,Operation::Register);println!("MEMBER:{}",registration.transaction);}
  if std::env::args().any(|a|a=="--unresponsive-control"){std::thread::sleep(Duration::from_secs(2));return;}


  let store=Arc::new(aim_storage::fsverity::Store::new(&root.join("proof"),&root.join("runtime")).unwrap());assert!(worker_with_store(channel,store).is_err());if std::env::args().any(|a|a=="--corrupt-control"){let dirty=super::super::verity_pager::fault_intent(memory as u64,super::super::verity_pager::Access::Read).unwrap();assert!(matches!(super::super::verity_pager::resolve(&dirty),super::super::verity_pager::FaultResult::Bus{..}));}else{assert_eq!(unsafe{std::ptr::read_volatile(memory.cast::<u8>())},91);}if registered&&std::env::args().any(|a|a=="--committed-control"){let intent=super::super::verity_pager::fault_intent(memory as u64+16384,super::super::verity_pager::Access::Read).unwrap();assert!(matches!(super::super::verity_pager::resolve(&intent),super::super::verity_pager::FaultResult::Verified));}
  if std::env::args().any(|a|a=="--corrupt-control"){let intent=super::super::verity_pager::fault_intent(memory as u64+16384,super::super::verity_pager::Access::Read).unwrap();assert!(matches!(super::super::verity_pager::resolve(&intent),super::super::verity_pager::FaultResult::Bus{..}));}else{assert_eq!(unsafe{std::ptr::read_volatile(memory.cast::<u8>().add(16384))},37);}
  super::super::verity_pager::forget(memory as u64,32768).unwrap();unsafe{libc::munmap(memory,32768);}
 }
 #[test]
 fn cross_process_prepare_abort_preserves_cow(){
  let root=std::env::temp_dir().join(format!("aim-control-pager-{}",std::process::id()));fs::create_dir(&root).unwrap();fs::write(root.join("data"),vec![37;32768]).unwrap();let file=fs::File::open(root.join("data")).unwrap();let store=aim_storage::fsverity::Store::new(&root.join("proof"),&root.join("runtime")).unwrap();let admission=store.lock_inode(&file).unwrap();let enable=admission.begin_enable().unwrap();drop(admission);let prepared=enable.build(aim_storage::fsverity::BuildOptions::new(1,4096,vec![],16384,4096).unwrap(),&[],||false).unwrap();let metadata=prepared.metadata_view().unwrap();let identity=Identity::from_fd(std::os::fd::AsFd::as_fd(&file)).unwrap();
  let listener=UnixListener::bind(root.join("control")).unwrap();let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","sys::verity_control::tests::pager_child","--ignored","--nocapture","--skip",&format!("--verity-root={}",root.display())]).stdout(Stdio::null()).spawn().unwrap();let(stream,_)=listener.accept().unwrap();let process=aim_storage::process_namespace::ProcessIdentity::running(child.id()as i32).unwrap();let mut channel=Channel::new(stream,Some(process),Duration::from_secs(2)).unwrap();let proof=aim_storage::private_fd::PrivateFd::allocate(||{let fd=unsafe{libc::fcntl(metadata.backing_descriptor().as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};if fd<0{Err(io::Error::last_os_error())}else{Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(fd)})}}).unwrap();
  let frame=Frame{operation:Operation::Prepare,transaction:7,identity:identity.to_bytes(),generation:0,error:0};channel.send(&frame,Some(&proof)).unwrap();assert_eq!(channel.receive().unwrap().0.operation,Operation::Prepared);channel.send(&Frame{operation:Operation::Abort,..frame},None).unwrap();assert_eq!(channel.receive().unwrap().0.operation,Operation::Aborted);drop(channel);assert!(child.wait().unwrap().success());drop(enable);drop(prepared);drop(metadata);fs::remove_dir_all(root).unwrap();
 }
 #[test]
 fn corrupt_marker_eof_quarantines_clean_pages_and_finishes_worker(){
  let root=std::env::temp_dir().join(format!("aim-control-corrupt-{}",std::process::id()));fs::create_dir(&root).unwrap();fs::write(root.join("data"),vec![37;32768]).unwrap();let file=fs::File::open(root.join("data")).unwrap();let store=aim_storage::fsverity::Store::new(&root.join("proof"),&root.join("runtime")).unwrap();let admission=store.lock_inode(&file).unwrap();let enable=admission.begin_enable().unwrap();drop(admission);let prepared=enable.build(aim_storage::fsverity::BuildOptions::new(1,4096,vec![],16384,4096).unwrap(),&[],||false).unwrap();let metadata=prepared.metadata_view().unwrap();let identity=Identity::from_fd(std::os::fd::AsFd::as_fd(&file)).unwrap();
  let listener=UnixListener::bind(root.join("control")).unwrap();let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","sys::verity_control::tests::pager_child","--ignored","--nocapture","--skip",&format!("--verity-root={}",root.display()),"--skip","--corrupt-control"]).stdout(Stdio::null()).spawn().unwrap();let(stream,_)=listener.accept().unwrap();let process=aim_storage::process_namespace::ProcessIdentity::running(child.id()as i32).unwrap();let mut channel=Channel::new(stream,Some(process),Duration::from_secs(2)).unwrap();let proof=aim_storage::private_fd::PrivateFd::allocate(||{let fd=unsafe{libc::fcntl(metadata.backing_descriptor().as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};if fd<0{Err(io::Error::last_os_error())}else{Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(fd)})}}).unwrap();
  let frame=Frame{operation:Operation::Prepare,transaction:7,identity:identity.to_bytes(),generation:0,error:0};channel.send(&frame,Some(&proof)).unwrap();assert_eq!(channel.receive().unwrap().0.operation,Operation::Prepared);let name:String=identity.to_bytes().iter().map(|b|format!("{b:02x}")).collect();fs::write(root.join("proof").join(name),b"corrupt durable marker").unwrap();drop(channel);let deadline=std::time::Instant::now()+Duration::from_secs(3);while child.try_wait().unwrap().is_none(){if std::time::Instant::now()>deadline{child.kill().unwrap();child.wait().unwrap();panic!("worker did not finish after corrupt marker EOF");}std::thread::sleep(Duration::from_millis(5));}assert!(child.wait().unwrap().success());drop(enable);drop(prepared);drop(metadata);fs::remove_dir_all(root).unwrap();
 }
 #[test]
 fn different_valid_marker_digest_eof_quarantines_clean_pages(){
  let root=std::env::temp_dir().join(format!("aim-control-digest-{}",std::process::id()));fs::create_dir(&root).unwrap();fs::write(root.join("data"),vec![37;32768]).unwrap();let file=fs::File::open(root.join("data")).unwrap();let store=aim_storage::fsverity::Store::new(&root.join("proof"),&root.join("runtime")).unwrap();let admission=store.lock_inode(&file).unwrap();let enable=admission.begin_enable().unwrap();drop(admission);let prepared=enable.build(aim_storage::fsverity::BuildOptions::new(1,4096,vec![],16384,4096).unwrap(),&[],||false).unwrap();let metadata=prepared.metadata_view().unwrap();let identity=Identity::from_fd(std::os::fd::AsFd::as_fd(&file)).unwrap();
  let listener=UnixListener::bind(root.join("control")).unwrap();let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","sys::verity_control::tests::pager_child","--ignored","--nocapture","--skip",&format!("--verity-root={}",root.display()),"--skip","--corrupt-control"]).stdout(Stdio::null()).spawn().unwrap();let(stream,_)=listener.accept().unwrap();let process=aim_storage::process_namespace::ProcessIdentity::running(child.id()as i32).unwrap();let mut channel=Channel::new(stream,Some(process),Duration::from_secs(2)).unwrap();let proof=aim_storage::private_fd::PrivateFd::allocate(||{let fd=unsafe{libc::fcntl(metadata.backing_descriptor().as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};if fd<0{Err(io::Error::last_os_error())}else{Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(fd)})}}).unwrap();
  let frame=Frame{operation:Operation::Prepare,transaction:7,identity:identity.to_bytes(),generation:0,error:0};channel.send(&frame,Some(&proof)).unwrap();assert_eq!(channel.receive().unwrap().0.operation,Operation::Prepared);let different=enable.build(aim_storage::fsverity::BuildOptions::new(1,4096,vec![1],16384,4096).unwrap(),&[],||false).unwrap();let actual=enable.commit(different).unwrap();drop(channel);let deadline=std::time::Instant::now()+Duration::from_secs(3);while child.try_wait().unwrap().is_none(){if std::time::Instant::now()>deadline{child.kill().unwrap();child.wait().unwrap();panic!("worker did not finish after corrupt marker EOF");}std::thread::sleep(Duration::from_millis(5));}assert!(child.wait().unwrap().success());drop(actual);drop(prepared);drop(metadata);fs::remove_dir_all(root).unwrap();
 }
 #[test]
 fn two_registered_pagers_commit_actual_durable_proof_before_late_admission(){
  let root=std::env::temp_dir().join(format!("aimvc-native-{}-{:x}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));fs::create_dir(&root).unwrap();fs::write(root.join("data"),vec![37;32768]).unwrap();let file=fs::File::open(root.join("data")).unwrap();let store=aim_storage::fsverity::Store::new(&root.join("proof"),&root.join("runtime")).unwrap();let admission=store.lock_inode(&file).unwrap();let enable=admission.begin_enable().unwrap();drop(admission);let prepared=enable.build(aim_storage::fsverity::BuildOptions::new(1,4096,vec![],16384,4096).unwrap(),&[],||false).unwrap();let metadata=prepared.metadata_view().unwrap();let identity=Identity::from_fd(std::os::fd::AsFd::as_fd(&file)).unwrap();
  let server=aim_storage::verity_control::Server::bind(&root.join("control"),Duration::from_secs(2)).unwrap();
  struct Children(Vec<std::process::Child>);impl Drop for Children{fn drop(&mut self){for child in &mut self.0{if child.try_wait().ok().flatten().is_none(){let _=child.kill();let _=child.wait();}}}}
  let mut children=Children(Vec::new());for _ in 0..2{children.0.push(Command::new(std::env::current_exe().unwrap()).args(["--exact","sys::verity_control::tests::pager_child","--ignored","--nocapture","--skip",&format!("--verity-root={}",root.display()),"--skip","--registered-control","--skip","--committed-control"]).stdout(Stdio::null()).spawn().unwrap());let(member,_)=server.accept().unwrap();server.coordinator.admission(member,identity.to_bytes()).unwrap().published().unwrap();}
  let proof=aim_storage::private_fd::PrivateFd::allocate(||{let fd=unsafe{libc::fcntl(metadata.backing_descriptor().as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};if fd<0{Err(io::Error::last_os_error())}else{Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(fd)})}}).unwrap();
  let mut transition=server.prepare(identity.to_bytes(),&proof).unwrap();assert!(transition.ready());assert!(store.lookup(identity).unwrap().is_none());
  let late=server.coordinator.register(aim_storage::process_namespace::ProcessIdentity::running(std::process::id()as i32).unwrap()).unwrap();let owner=server.coordinator.clone();let (tx,rx)=std::sync::mpsc::channel();let blocked=std::thread::spawn(move||{let admission=owner.admission(late,identity.to_bytes()).unwrap();tx.send(()).unwrap();drop(admission);});assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
  let durable=enable.commit(prepared).unwrap();transition.published().unwrap();transition.commit().unwrap();rx.recv_timeout(Duration::from_secs(1)).unwrap();blocked.join().unwrap();drop(server);
  for child in &mut children.0{assert!(child.wait().unwrap().success());}assert!(store.lookup(identity).unwrap().is_some());drop(durable);drop(metadata);fs::remove_dir_all(root).unwrap();
 }

 #[test]
 fn remote_issuer_commits_two_actual_pagers(){
  let root=std::env::temp_dir().join(format!("aimvc-remote-{}-{:x}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));fs::create_dir(&root).unwrap();fs::write(root.join("data"),vec![37;32768]).unwrap();let file=fs::File::open(root.join("data")).unwrap();let store=aim_storage::fsverity::Store::new(&root.join("proof"),&root.join("runtime")).unwrap();let admission=store.lock_inode(&file).unwrap();let enable=admission.begin_enable().unwrap();drop(admission);let prepared=enable.build(aim_storage::fsverity::BuildOptions::new(1,4096,vec![],16384,4096).unwrap(),&[],||false).unwrap();let metadata=prepared.metadata_view().unwrap();let identity=Identity::from_fd(std::os::fd::AsFd::as_fd(&file)).unwrap();
  let running=aim_storage::verity_control::RunningServer::start_with_store(&root.join("control"),Duration::from_secs(2),Arc::new(store.clone())).unwrap();let server=running.server.clone();let(issuer,_control)=aim_storage::verity_control::Client::attach(&root.join("control"),running.process,Duration::from_secs(2)).unwrap();
  struct Children(Vec<std::process::Child>);impl Drop for Children{fn drop(&mut self){for child in &mut self.0{if child.try_wait().ok().flatten().is_none(){let _=child.kill();let _=child.wait();}}}}
  let mut readers=Vec::new();let mut children=Children(Vec::new());for _ in 0..2{children.0.push(Command::new(std::env::current_exe().unwrap()).args(["--exact","sys::verity_control::tests::pager_child","--ignored","--nocapture","--skip",&format!("--verity-root={}",root.display()),"--skip","--registered-control","--skip","--committed-control"]).stdout(Stdio::piped()).spawn().unwrap());let mut reader=std::io::BufReader::new(children.0.last_mut().unwrap().stdout.take().unwrap());let member=loop{use std::io::BufRead;let mut line=String::new();assert!(reader.read_line(&mut line).unwrap()>0);if let Some(value)=line.trim().strip_prefix("MEMBER:"){break value.parse::<u64>().unwrap();}};server.coordinator.admission(member,identity.to_bytes()).unwrap().published().unwrap();readers.push(reader);}
  let proof=aim_storage::private_fd::PrivateFd::allocate(||{let fd=unsafe{libc::fcntl(metadata.backing_descriptor().as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};if fd<0{Err(io::Error::last_os_error())}else{Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(fd)})}}).unwrap();
  let transition=issuer.enable_transition(identity.to_bytes(),&proof).unwrap();assert!(store.lookup(identity).unwrap().is_none());
  let late=server.coordinator.register(aim_storage::process_namespace::ProcessIdentity::running(std::process::id()as i32).unwrap()).unwrap();let owner=server.coordinator.clone();let (tx,rx)=std::sync::mpsc::channel();let blocked=std::thread::spawn(move||{let admission=owner.admission(late,identity.to_bytes()).unwrap();tx.send(()).unwrap();drop(admission);});assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
  let durable=enable.commit(prepared).unwrap();transition.published(&durable).unwrap();rx.recv_timeout(Duration::from_secs(1)).unwrap();blocked.join().unwrap();drop(server);running.shutdown().unwrap();
  for child in &mut children.0{assert!(child.wait().unwrap().success());}assert!(store.lookup(identity).unwrap().is_some());drop(durable);drop(metadata);fs::remove_dir_all(root).unwrap();
 }

 #[test]
 fn remote_issuer_postpublication_live_peer_failure_retains_enabled_marker(){
  let root=std::env::temp_dir().join(format!("aim-control-postfailure-{}",std::process::id()));fs::create_dir(&root).unwrap();fs::write(root.join("data"),vec![37;32768]).unwrap();let file=fs::File::open(root.join("data")).unwrap();let store=aim_storage::fsverity::Store::new(&root.join("proof"),&root.join("runtime")).unwrap();let admission=store.lock_inode(&file).unwrap();let enable=admission.begin_enable().unwrap();drop(admission);let prepared=enable.build(aim_storage::fsverity::BuildOptions::new(1,4096,vec![],16384,4096).unwrap(),&[],||false).unwrap();let metadata=prepared.metadata_view().unwrap();let identity=Identity::from_fd(std::os::fd::AsFd::as_fd(&file)).unwrap();
  let running=aim_storage::verity_control::RunningServer::start_with_store(&root.join("control"),Duration::from_secs(2),Arc::new(store.clone())).unwrap();let server=running.server.clone();let(issuer,_control)=aim_storage::verity_control::Client::attach(&root.join("control"),running.process,Duration::from_secs(2)).unwrap();
  struct Children(Vec<std::process::Child>);impl Drop for Children{fn drop(&mut self){for child in &mut self.0{if child.try_wait().ok().flatten().is_none(){let _=child.kill();let _=child.wait();}}}}
  let mut readers=Vec::new();let mut children=Children(Vec::new());for _ in 0..2{children.0.push(Command::new(std::env::current_exe().unwrap()).args(["--exact","sys::verity_control::tests::pager_child","--ignored","--nocapture","--skip",&format!("--verity-root={}",root.display()),"--skip","--registered-control","--skip","--committed-control"]).stdout(Stdio::piped()).spawn().unwrap());let mut reader=std::io::BufReader::new(children.0.last_mut().unwrap().stdout.take().unwrap());let member=loop{use std::io::BufRead;let mut line=String::new();assert!(reader.read_line(&mut line).unwrap()>0);if let Some(value)=line.trim().strip_prefix("MEMBER:"){break value.parse::<u64>().unwrap();}};server.coordinator.admission(member,identity.to_bytes()).unwrap().published().unwrap();readers.push(reader);}
  let proof=aim_storage::private_fd::PrivateFd::allocate(||{let fd=unsafe{libc::fcntl(metadata.backing_descriptor().as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};if fd<0{Err(io::Error::last_os_error())}else{Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(fd)})}}).unwrap();
  let transition=issuer.enable_transition(identity.to_bytes(),&proof).unwrap();assert!(store.lookup(identity).unwrap().is_none());
  let stopped=children.0[1].id()as i32;assert_eq!(unsafe{libc::kill(stopped,libc::SIGSTOP)},0);let live=aim_storage::process_namespace::ProcessIdentity::running(stopped).unwrap();
  let durable=enable.commit(prepared).unwrap();assert!(transition.published(&durable).is_err());assert!(live.is_live());assert!(store.lookup(identity).unwrap().is_some());
  children.0[1].kill().unwrap();children.0[1].wait().unwrap();drop(server);running.shutdown().unwrap();
  assert!(children.0[0].wait().unwrap().success());drop(durable);drop(metadata);fs::remove_dir_all(root).unwrap();
 }
 #[test]
 fn live_unresponsive_second_pager_aborts_first_without_publishing(){
  let root=std::env::temp_dir().join(format!("aim-control-timeout-{}",std::process::id()));fs::create_dir(&root).unwrap();fs::write(root.join("data"),vec![37;32768]).unwrap();let file=fs::File::open(root.join("data")).unwrap();let store=aim_storage::fsverity::Store::new(&root.join("proof"),&root.join("runtime")).unwrap();let admission=store.lock_inode(&file).unwrap();let enable=admission.begin_enable().unwrap();drop(admission);let prepared=enable.build(aim_storage::fsverity::BuildOptions::new(1,4096,vec![],16384,4096).unwrap(),&[],||false).unwrap();let metadata=prepared.metadata_view().unwrap();let identity=Identity::from_fd(std::os::fd::AsFd::as_fd(&file)).unwrap();let server=aim_storage::verity_control::Server::bind(&root.join("control"),Duration::from_millis(100)).unwrap();
  struct Children(Vec<std::process::Child>);impl Drop for Children{fn drop(&mut self){for child in &mut self.0{if child.try_wait().ok().flatten().is_none(){let _=child.kill();let _=child.wait();}}}}
  let mut children=Children(Vec::new());let mut processes=Vec::new();for index in 0..2{let mut command=Command::new(std::env::current_exe().unwrap());command.args(["--exact","sys::verity_control::tests::pager_child","--ignored","--nocapture","--skip",&format!("--verity-root={}",root.display()),"--skip","--registered-control"]);if index==1{command.args(["--skip","--unresponsive-control"]);}children.0.push(command.stdout(Stdio::null()).spawn().unwrap());let(member,process)=server.accept().unwrap();processes.push(process);server.coordinator.admission(member,identity.to_bytes()).unwrap().published().unwrap();}
  let proof=aim_storage::private_fd::PrivateFd::allocate(||{let fd=unsafe{libc::fcntl(metadata.backing_descriptor().as_raw_fd(),libc::F_DUPFD_CLOEXEC,0)};if fd<0{Err(io::Error::last_os_error())}else{Ok(unsafe{std::os::fd::OwnedFd::from_raw_fd(fd)})}}).unwrap();assert!(server.prepare(identity.to_bytes(),&proof).is_err());assert!(processes[1].is_live());assert!(store.lookup(identity).unwrap().is_none());drop(server);assert!(children.0[0].wait().unwrap().success());children.0[1].kill().unwrap();children.0[1].wait().unwrap();assert!(!processes[1].is_live());drop(enable);drop(prepared);drop(metadata);fs::remove_dir_all(root).unwrap();
 }

 #[test]
 fn native_instance_start_admission_disconnect_and_joined_shutdown(){
  let root=std::env::temp_dir().join(format!("aim-control-lifecycle-{}",std::process::id()));std::fs::create_dir(&root).unwrap();let endpoint=root.join("control");let server=aim_storage::verity_control::RunningServer::start(&endpoint,std::time::Duration::from_millis(200)).unwrap();let runtime=Runtime::start(&endpoint,server.process,std::time::Duration::from_millis(200)).unwrap();assert_ne!(unsafe{libc::fcntl(runtime.shutdown.as_raw_fd(),libc::F_GETFD)}&libc::FD_CLOEXEC,0);let admission=runtime.client.admission([4;36]).unwrap();drop(admission);std::thread::sleep(std::time::Duration::from_millis(10));let reservation=server.server.coordinator.reserve([4;36]).unwrap();assert!(reservation.ready());reservation.finish(&std::collections::BTreeSet::new()).unwrap();runtime.shutdown().unwrap();let result=server.shutdown();assert!(result.is_ok(),"coordinator shutdown: {result:?}");assert!(!endpoint.exists());assert!(!endpoint.with_extension("admission").exists());std::fs::remove_dir(root).unwrap();
 }

}

/// Created before guest execution from the native instance's explicit endpoint.
/// Closing its private control socket wakes the worker and aborts pending maps.
pub struct Runtime {pub client:Arc<aim_storage::verity_control::Client>,shutdown:aim_storage::private_fd::PrivateFd,thread:Option<std::thread::JoinHandle<io::Result<()>>>}
impl Runtime {
 pub fn start(path:&std::path::Path,init:aim_storage::process_namespace::ProcessIdentity,timeout:std::time::Duration)->io::Result<Self>{let(client,channel)=aim_storage::verity_control::Client::attach(path,init,timeout)?;let shutdown=channel.shutdown_handle()?;let thread=std::thread::Builder::new().name("verity-control".into()).spawn(move||worker(channel))?;Ok(Self{client:Arc::new(client),shutdown,thread:Some(thread)})}
 pub fn start_with_store(path:&std::path::Path,init:aim_storage::process_namespace::ProcessIdentity,timeout:std::time::Duration,store:Arc<aim_storage::fsverity::Store>)->io::Result<Self>{let(client,channel)=aim_storage::verity_control::Client::attach(path,init,timeout)?;let shutdown=channel.shutdown_handle()?;let thread=std::thread::Builder::new().name("verity-control".into()).spawn(move||worker_with_store(channel,store))?;Ok(Self{client:Arc::new(client),shutdown,thread:Some(thread)})}
 pub fn shutdown(mut self)->io::Result<()>{self.stop()}
 fn stop(&mut self)->io::Result<()>{if self.thread.is_none(){return Ok(());}if unsafe{libc::shutdown(self.shutdown.as_raw_fd(),libc::SHUT_RDWR)}!=0{return Err(io::Error::last_os_error());}if let Some(thread)=self.thread.take(){match thread.join(){Ok(Ok(()))=>Ok(()),Ok(Err(error))if matches!(error.raw_os_error(),Some(libc::EPIPE)|Some(libc::ECONNRESET))=>Ok(()),Ok(Err(error))=>Err(error),Err(_)=>Err(error(libc::EIO))}}else{Ok(())}}
}
impl Drop for Runtime {fn drop(&mut self){if let Err(failure)=self.stop(){eprintln!("verity worker shutdown failed: {failure}");}}}

/// The process startup owner installs this client before any guest thread.
pub fn client()->Option<Arc<aim_storage::verity_control::Client>>{crate::verity_client()}
