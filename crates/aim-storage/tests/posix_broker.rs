//! Actual native-controller/holder process tests for #1189.
use aim_storage::{posix_broker::{Config,Controller},posix_control::{Client,Frame,Operation,Owner,Range},process_namespace::ProcessIdentity,inode_lease::Identity};
use std::{io::{BufRead,BufReader,Write},os::fd::{AsFd,AsRawFd},process::{Child,ChildStdin,ChildStdout,Command,Stdio},time::Duration};
#[test]
#[ignore = "controlled child of controller_owns_two_guest_incarnations_and_real_lock_lifetimes"]
fn posix_broker_guest_child(){
 let stdin=std::io::stdin();let mut lines=stdin.lock().lines();let endpoint=lines.next().unwrap().unwrap();let path=lines.next().unwrap().unwrap();
 let client=Client::lookup(&endpoint).unwrap();let file=std::fs::OpenOptions::new().read(true).write(true).open(path).unwrap();let identity=Identity::from_fd(file.as_fd()).unwrap();
 let mut ticket=0;let mut next=1;let mut pending=None;let mut abandoned=0;
 println!("GUEST READY");std::io::stdout().flush().unwrap();
 for line in lines{let line=line.unwrap();let args=line.split_whitespace().collect::<Vec<_>>();if args[0]=="quit"{break;}
  if args[0]=="exec" {
   // Preserve this actual open descriptor across exec. POSIX ownership is
   // the process incarnation, rather than a particular code epoch.
   assert_eq!(unsafe{libc::fcntl(file.as_raw_fd(),libc::F_SETFD,0)},0);
   println!("GUEST 0 0 0 0 0");std::io::stdout().flush().unwrap();
   use std::os::unix::process::CommandExt;
   panic!("exec failed: {}",Command::new(std::env::current_exe().unwrap()).args(["--exact","posix_broker_guest_child","--ignored","--nocapture"]).exec());
  }
  let mut frame=Frame::new(match args[0]{"register"|"repeat"=>Operation::Register,"set"=>Operation::Set,"get"=>Operation::Get,"wait"=>Operation::Wait,"cancel"=>Operation::Cancel,"close"=>Operation::Close,_=>Operation::Get},next);next+=1;frame.ticket=ticket;
  frame.key=identity.to_bytes();frame.access=libc::O_RDWR;
  if matches!(args[0],"set"|"get"|"wait"){frame.range=Range{kind:args[1].parse().unwrap(),start:args[2].parse().unwrap(),len:args[3].parse().unwrap()};}
  let reply=match args[0]{
   "register"|"repeat"=>{let result=client.call(frame,Some(file.as_fd())).unwrap();ticket=result.ticket;result},
   "wait"=>{pending=Some(client.begin(frame,None).unwrap());Frame::new(Operation::Wait,frame.request)},
   "await"=>pending.take().unwrap().wait(Duration::from_secs(3)).unwrap(),
   "duplicate"=>{frame.operation=Operation::Get;frame.request=pending.as_ref().unwrap().request;client.call(frame,None).unwrap()},
   "reset"=>{next=1;Frame::new(Operation::Get,0)},
   "abandon"=>{abandoned=pending.take().unwrap().request;Frame::new(Operation::Get,frame.request)},
   "cancel"=>{frame.ticket=pending.as_ref().map_or(abandoned,|waiting|waiting.request);client.call(frame,None).unwrap()},
   "pin"=>{let duplicate=file.try_clone().unwrap();let mut byte=0u8;assert_eq!(unsafe{libc::pread(duplicate.as_raw_fd(),(&mut byte as *mut u8).cast(),1,0)},1);drop(duplicate);Frame::new(Operation::Get,frame.request)},
   _=>client.call(frame,None).unwrap()
  };
  println!("GUEST {} {} {} {} {}",reply.errno,reply.ticket,reply.guest_pid,reply.host_pid,reply.range.kind);std::io::stdout().flush().unwrap();
 }
}
struct Guest {child:Child,input:ChildStdin,output:BufReader<ChildStdout>}
impl Guest {
 fn spawn()->Self{let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","posix_broker_guest_child","--ignored","--nocapture"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();Self{input:child.stdin.take().unwrap(),output:BufReader::new(child.stdout.take().unwrap()),child}}
 fn owner(&self,pid:i32)->Owner{Owner{process:ProcessIdentity::running(self.child.id()as i32).unwrap(),guest_pid:pid}}
 fn init(&mut self,endpoint:&str,path:&std::path::Path){writeln!(self.input,"{endpoint}\n{}",path.display()).unwrap();self.input.flush().unwrap();assert_eq!(self.read(),"READY");}
 fn read(&mut self)->String{loop{let mut line=String::new();assert!(self.output.read_line(&mut line).unwrap()>0,"guest unexpectedly exited");if let Some(value)=line.trim().strip_prefix("GUEST "){return value.into();}}}
 fn call(&mut self,command:&str)->Vec<i64>{writeln!(self.input,"{command}").unwrap();self.input.flush().unwrap();self.read().split_whitespace().map(|part|part.parse().unwrap()).collect()}
 fn quit(&mut self){writeln!(self.input,"quit").unwrap();self.input.flush().unwrap();assert!(self.child.wait().unwrap().success());}
}
impl Drop for Guest{fn drop(&mut self){if self.child.try_wait().unwrap().is_none(){self.child.kill().unwrap();}self.child.wait().unwrap();}}
#[test]
fn controller_owns_two_guest_incarnations_and_real_lock_lifetimes(){
 let directory=std::env::temp_dir().join(format!("aim-posix-controller-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));std::fs::create_dir(&directory).unwrap();let path=directory.join("file");std::fs::write(&path,b"real lock bytes").unwrap();
 let endpoint=format!("com.aim.posix-controller-test.{}.{}",std::process::id(),directory.file_name().unwrap().to_string_lossy());
 let controller=Controller::start(Config{endpoint:endpoint.clone(),holder:std::path::PathBuf::from(env!("CARGO_BIN_EXE_aim-lock-holder")),startup_timeout:Duration::from_secs(3)}).unwrap();
 // A real unregistered process cannot turn a claimed PID into ownership.
 let unauthorized=Client::lookup(&endpoint).unwrap();let forged=Frame::new(Operation::Get,1);assert_eq!(unauthorized.call(forged,None).unwrap().errno,libc::EPERM);
 let mut first=Guest::spawn();let mut second=Guest::spawn();let first_owner=first.owner(41001);let second_owner=second.owner(41002);
 let first_holder=controller.register_guest(first_owner).unwrap();let second_holder=controller.register_guest(second_owner).unwrap();assert_ne!(first_holder,second_holder);assert_eq!(controller.register_guest(first_owner).unwrap(),first_holder,"same incarnation rebind keeps locks");
 first.init(&endpoint,&path);second.init(&endpoint,&path);let ticket=first.call("register")[1];second.call("register");
 assert_eq!(first.call("set 1 0 8")[0],0);assert!(matches!(second.call("set 1 0 8")[0]as i32,libc::EACCES|libc::EAGAIN));
 first.call("pin");assert!(matches!(second.call("set 1 0 8")[0]as i32,libc::EACCES|libc::EAGAIN));assert_eq!(first.call("repeat")[1],ticket);assert!(matches!(second.call("set 1 0 8")[0]as i32,libc::EACCES|libc::EAGAIN));
 first.call("exec");first.init(&endpoint,&path);
 assert_eq!(first.owner(41001),first_owner,"actual exec preserves process birth identity");
 assert_eq!(controller.register_guest(first_owner).unwrap(),first_holder);
 assert_eq!(first.call("register")[1],ticket,"actual exec rebind keeps holder inode cache");
 let stale=Owner{process:ProcessIdentity{start_microseconds:first_owner.process.start_microseconds.wrapping_add(1),..first_owner.process},..first_owner};
 assert_eq!(controller.register_guest(stale).unwrap_err().raw_os_error(),Some(libc::ESRCH));
 let observed=second.call("get 1 0 8");assert_eq!(observed[0],0);assert_eq!(observed[2],41001);assert_eq!(observed[3],0,"helper host PID must not escape");
 assert_eq!(second.call("set 1 8 8")[0],0);
 second.call("wait 1 0 8");assert_eq!(second.call("duplicate")[0]as i32,libc::EALREADY,"new client counter must not alias the old wait");
 controller.rebind_guest(second_owner).unwrap();assert_eq!(second.call("await")[0]as i32,libc::EINTR);
 assert!(matches!(first.call("set 1 8 8")[0]as i32,libc::EACCES|libc::EAGAIN),"rebind must preserve the same owner existing locks");
 second.call("reset");
 assert!(matches!(second.call("set 1 0 8")[0]as i32,libc::EACCES|libc::EAGAIN),"rebind cancels waits without releasing other owner's locks");
 second.call("wait 1 0 8");assert_eq!(second.call("cancel")[0],0);assert_eq!(second.call("await")[0]as i32,libc::EINTR);
 second.call("wait 1 0 8");second.call("abandon");assert_eq!(second.call("cancel")[0],0);
 // Darwin may consume a send-once right to a vanished receiver without
 // reporting a send error. Remaining owners must work in either case.
 assert!(matches!(second.call("set 1 0 8")[0]as i32,libc::EACCES|libc::EAGAIN),"lost requester does not revoke other owners");
 assert_eq!(first.call("close")[0],0);assert_eq!(second.call("set 1 0 8")[0],0);second.call("set 2 0 0");
 first.call("register");assert_eq!(first.call("set 1 0 8")[0],0);first.quit();
 let deadline=std::time::Instant::now()+Duration::from_secs(3);while first_holder.is_live(){assert!(std::time::Instant::now()<deadline,"dead guest holder not retired");std::thread::sleep(Duration::from_millis(10));}
 assert_eq!(second.call("set 1 0 8")[0],0,"owner exit releases real POSIX locks");second.quit();controller.shutdown().unwrap();assert!(!first_holder.is_live()&&!second_holder.is_live());std::fs::remove_dir_all(directory).unwrap();
}
