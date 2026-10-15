use super::*;
use std::{fs,io::{BufRead,BufReader,Write},os::unix::fs::DirBuilderExt,process::{Child,Command,Stdio}};
struct Actor { child:Child, output:BufReader<std::process::ChildStdout>, identity:ProcessIdentity }
impl Actor {
 fn new(leader:bool)->Self {
  let script=r#"import os,signal,sys
if sys.argv[1]=='leader': os.setsid()
signal.pthread_sigmask(signal.SIG_BLOCK,{signal.SIGHUP,signal.SIGCONT})
print(os.getpid(),os.getuid(),os.geteuid(),flush=True)
for line in sys.stdin:
 if line.strip()=='exit': break
 if line.strip()=='pending': print(int(bool(signal.sigpending() & {signal.SIGHUP,signal.SIGCONT})),flush=True)
 if line.strip()=='signals':
  signal.alarm(5)
  found=[]
  for _ in range(2):
   found.append(signal.sigwait({signal.SIGHUP,signal.SIGCONT}))
  signal.alarm(0)
  print(','.join(map(str,sorted(found))),flush=True)
"#;
  let mut child=Command::new("python3").args(["-u","-c",script,if leader{"leader"}else{"member"}]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
  let mut output=BufReader::new(child.stdout.take().unwrap());let mut line=String::new();assert!(output.read_line(&mut line).unwrap()>0);
  let fields=line.split_whitespace().map(|s|s.parse::<u32>().unwrap()).collect::<Vec<_>>();assert_eq!(fields[0],child.id());
  assert_eq!(fields[1],unsafe{libc::getuid()});assert_eq!(fields[2],unsafe{libc::geteuid()});
  let identity=ProcessIdentity::running(child.id()as i32).unwrap();println!("ACTUAL_ACTOR {:?} sid={} pgid={} uid={} euid={}",identity,unsafe{libc::getsid(identity.host_pid)},unsafe{libc::getpgid(identity.host_pid)},fields[1],fields[2]);Self{child,output,identity}
 }
 fn command(&mut self,text:&str){writeln!(self.child.stdin.as_mut().unwrap(),"{text}").unwrap();}
 fn exit(&mut self){self.command("exit");assert!(self.child.wait().unwrap().success());assert!(!self.identity.is_live());println!("REAPED {:?}",self.identity);}
}
impl Drop for Actor {fn drop(&mut self){if self.child.try_wait().ok().flatten().is_none(){let _=self.child.kill();}let _=self.child.wait();}}
struct Cleanup(PathBuf);
impl Drop for Cleanup {fn drop(&mut self){fs::remove_dir_all(&self.0).unwrap();}}
struct Fixture { table:PathBuf, pair:Arc<Pair>, sessions:Sessions, _cleanup:Cleanup }
impl Fixture {
 fn new()->Self {
  static NEXT:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
  let root=std::env::temp_dir().join(format!("aim-pty-session-independent-{}-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed)));fs::DirBuilder::new().mode(0o700).create(&root).unwrap();let root=fs::canonicalize(root).unwrap();
  let table=root.join("by-pid");fs::create_dir(&table).unwrap();let pair=Arc::new(Pair::allocate(&root).unwrap());let mut sessions=Sessions::new();sessions.set_table(table.clone());Self{table,pair,sessions,_cleanup:Cleanup(root)}
 }
 fn register(&self,actor:&Actor,cap:u64){
  crate::process_namespace::register_mount_namespace(&self.table,actor.identity,"fixture-actual-child").unwrap();
  fs::write(self.table.join(actor.identity.host_pid.to_string()),format!("uid\t{}\neuid\t{}\ncap_effective\t0x{cap:x}\n",unsafe{libc::getuid()},unsafe{libc::geteuid()})).unwrap();
  let observed=read_credentials(&self.table,actor.identity).unwrap();assert_eq!(observed.uid,unsafe{libc::getuid()});assert_eq!(observed.effective_uid,unsafe{libc::geteuid()});assert_eq!(observed.effective_capabilities,cap);
 }
}
#[test]
fn forced_steal_requires_registered_cap_and_foreign_session_is_rejected(){
 let mut f=Fixture::new();let mut original=Actor::new(true);let mut thief=Actor::new(true);let mut nonleader=Actor::new(false);
 f.register(&original,0);f.register(&thief,0);f.register(&nonleader,1<<21);
 assert_eq!(unsafe{libc::getsid(original.identity.host_pid)},original.identity.host_pid);
 assert!(f.sessions.claim(&f.pair,original.identity,false,false,true).unwrap());
 assert_eq!(f.sessions.claim(&f.pair,thief.identity,true,false,true).unwrap_err().raw_os_error(),Some(libc::EPERM));
 assert_eq!(f.sessions.state(&f.pair,original.identity,false).unwrap().0,original.identity.host_pid);
 assert_eq!(f.sessions.state(&f.pair,thief.identity,false).unwrap_err().raw_os_error(),Some(libc::ENOTTY));
 assert_eq!(f.sessions.claim(&f.pair,nonleader.identity,true,false,true).unwrap_err().raw_os_error(),Some(libc::EPERM));
 f.register(&thief,1<<21);
 assert_eq!(f.sessions.claim(&f.pair,thief.identity,false,false,true).unwrap_err().raw_os_error(),Some(libc::EPERM));
 assert!(f.sessions.claim(&f.pair,thief.identity,true,false,true).unwrap());
 assert_eq!(f.sessions.state(&f.pair,original.identity,false).unwrap_err().raw_os_error(),Some(libc::ENOTTY));
 assert_eq!(f.sessions.state(&f.pair,thief.identity,false).unwrap().0,thief.identity.host_pid);
 original.exit();nonleader.exit();thief.exit();f.sessions.leader_exit(thief.identity.host_pid).unwrap();
}
#[test]
fn leader_exit_releases_binding_and_live_leader_is_not_retired(){
 let mut f=Fixture::new();let mut original=Actor::new(true);let mut successor=Actor::new(true);f.register(&original,0);f.register(&successor,0);
 assert!(f.sessions.claim(&f.pair,original.identity,false,false,true).unwrap());f.sessions.leader_exit(original.identity.host_pid).unwrap();assert_eq!(f.sessions.state(&f.pair,original.identity,false).unwrap().0,original.identity.host_pid);
 original.exit();f.sessions.leader_exit(original.identity.host_pid).unwrap();assert_eq!(f.sessions.state(&f.pair,successor.identity,true).unwrap_err().raw_os_error(),Some(libc::ENOTTY));
 assert!(f.sessions.claim(&f.pair,successor.identity,false,false,true).unwrap());successor.exit();f.sessions.leader_exit(successor.identity.host_pid).unwrap();
}
#[test]
fn actual_owned_leader_detach_delivers_native_hup_and_cont(){
 let mut f=Fixture::new();let mut leader=Actor::new(true);f.register(&leader,0);assert!(f.sessions.claim(&f.pair,leader.identity,false,false,true).unwrap());f.sessions.detach(&f.pair,leader.identity).unwrap();
 leader.command("signals");let mut line=String::new();assert!(leader.output.read_line(&mut line).unwrap()>0);assert_eq!(line.trim(),format!("{},{}",libc::SIGHUP,libc::SIGCONT));assert_eq!(f.sessions.state(&f.pair,leader.identity,false).unwrap_err().raw_os_error(),Some(libc::ENOTTY));leader.exit();
}
#[test]
fn retained_binding_signals_after_credential_directory_teardown_without_adopting_foreign_actor(){
 let mut f=Fixture::new();let mut leader=Actor::new(true);let mut foreign=Actor::new(true);
 f.register(&leader,0);f.register(&foreign,0);
 assert!(f.sessions.claim(&f.pair,leader.identity,false,false,true).unwrap());
 f.sessions.bindings[0].members.push(foreign.identity);
 let mut stale=leader.identity;stale.start_microseconds+=1;f.sessions.bindings[0].members.push(stale);
 fs::remove_dir_all(&f.table).unwrap();
 assert_eq!(f.sessions.claim(&f.pair,foreign.identity,true,false,true).unwrap_err().raw_os_error(),Some(libc::ENOENT));
 f.sessions.master_closed(&f.pair).unwrap();
 leader.command("signals");let mut line=String::new();assert!(leader.output.read_line(&mut line).unwrap()>0);
 assert_eq!(line.trim(),format!("{},{}",libc::SIGHUP,libc::SIGCONT));
 foreign.command("pending");line.clear();assert!(foreign.output.read_line(&mut line).unwrap()>0);assert_eq!(line.trim(),"0");
 assert!(f.sessions.bindings.is_empty());leader.exit();foreign.exit();
}
