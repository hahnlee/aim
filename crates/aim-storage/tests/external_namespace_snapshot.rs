use aim_storage::{mount_namespace::Namespace,posix_broker::{Config,Controller,read_credentials},posix_control::{Client,Frame,Operation},process_namespace::{ProcessIdentity,InitRegistration,InitNamespaceEntry,register_mount_namespace,mount_namespace_of}};
use std::{io::{BufRead,BufReader,Write},os::unix::fs::DirBuilderExt,path::PathBuf,process::{Child,Command,Stdio},time::Duration};
struct Actor(Child,BufReader<std::process::ChildStdout>);
impl Actor {
 fn new()->Self{let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","snapshot_actor","--ignored","--nocapture"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();let output=BufReader::new(child.stdout.take().unwrap());Self(child,output)}
 fn identity(&self)->ProcessIdentity{ProcessIdentity::running(self.0.id() as i32).unwrap()}
 fn attach(&mut self,endpoint:&str)->i32{writeln!(self.0.stdin.as_mut().unwrap(),"{endpoint}").unwrap();let mut line=String::new();loop{line.clear();assert!(self.1.read_line(&mut line).unwrap()>0);if let Some(value)=line.trim().strip_prefix("SNAPSHOT_ATTACH "){return value.parse().unwrap();}}}
 fn reap(&mut self){drop(self.0.stdin.take());assert!(self.0.wait().unwrap().success());}
}
impl Drop for Actor{fn drop(&mut self){if self.0.try_wait().ok().flatten().is_none(){let _=self.0.kill();}let _=self.0.wait();}}
struct Cleanup(PathBuf);
impl Drop for Cleanup{fn drop(&mut self){std::fs::remove_dir_all(&self.0).unwrap();}}
#[test]
#[ignore="real child exercised by authenticated_snapshot_survives_init_namespace_switch"]
fn snapshot_actor(){let mut input=std::io::stdin().lock().lines();let endpoint=input.next().unwrap().unwrap();let response=Client::lookup(&endpoint).unwrap().call(Frame::new(Operation::ExternalAttach,1),None).unwrap();println!("SNAPSHOT_ATTACH {}",response.errno);std::io::stdout().flush().unwrap();assert!(input.next().is_none());}
#[test]
fn authenticated_snapshot_survives_init_namespace_switch(){
 let mut nonce=[0u8;16];assert_eq!(unsafe{libc::getentropy(nonce.as_mut_ptr().cast(),nonce.len())},0);
 let root=std::env::temp_dir().join(format!("aim-snapshot-{}-{}",std::process::id(),nonce.iter().map(|b|format!("{b:02x}")).collect::<String>()));std::fs::DirBuilder::new().mode(0o700).create(&root).unwrap();let root=std::fs::canonicalize(root).unwrap();let _cleanup=Cleanup(root.clone());
 let table=root.join("identity/by-pid");std::fs::create_dir_all(&table).unwrap();
 let bootstrap=Namespace::open(&root,"snapshot-bootstrap").unwrap();bootstrap.initialize(&format!("root\t/\t{}\n",root.display())).unwrap();
 let default=Namespace::open(&root,"snapshot-default").unwrap();default.initialize(&format!("root\t/\t{}\n",root.display())).unwrap();
 let init=ProcessIdentity::running(std::process::id()as i32).unwrap();let registration=InitRegistration::register(&table,init,bootstrap.id()).unwrap();register_mount_namespace(&table,init,bootstrap.id()).unwrap();
 let endpoint=format!("dev.aim.snapshot-test.{}.{}",init.host_pid,init.start_microseconds);
 let controller=Controller::start(Config{endpoint:endpoint.clone(),holder:PathBuf::from(env!("CARGO_BIN_EXE_aim-lock-holder")),startup_timeout:Duration::from_secs(3)}).unwrap();controller.bind_namespace(&table,&registration).unwrap();
 let mut actor=Actor::new();let identity=actor.identity();let retained=InitNamespaceEntry::capture(&table,identity).unwrap();
 std::fs::write(table.join(identity.host_pid.to_string()),"uid\t0\ncap_effective\t0x240000\n").unwrap();
 register_mount_namespace(&table,init,default.id()).unwrap();
 assert_ne!(mount_namespace_of(&table,init).unwrap(),mount_namespace_of(&table,identity).unwrap(),"old early equality branch returns EPERM for this actual ordering");
 assert_eq!(read_credentials(&table,identity).unwrap().effective_capabilities,(1<<21)|(1<<18));
 assert_eq!(retained.id(),bootstrap.id());assert!(!retained.read().unwrap().mounts.is_empty());
 InitNamespaceEntry::retire_dead(&table,init).unwrap();assert!(table.join(format!("{}.init-namespace-entry",identity.host_pid)).exists(),"live actor entry retained");
 assert_eq!(actor.attach(&endpoint),0);actor.reap();controller.guest_exited(aim_storage::posix_control::Owner{process:identity,guest_pid:identity.host_pid}).unwrap();assert!(!table.join(format!("{}.init-namespace-entry",identity.host_pid)).exists(),"native holder reap retires exact dead actor entry");
 let mut denied=Actor::new();let identity=denied.identity();InitNamespaceEntry::capture(&table,identity).unwrap();std::fs::write(table.join(identity.host_pid.to_string()),"uid\t0\ncap_effective\t0x0\n").unwrap();assert_eq!(denied.attach(&endpoint),libc::EPERM);denied.reap();
 let mut admin_only=Actor::new();let identity=admin_only.identity();InitNamespaceEntry::capture(&table,identity).unwrap();std::fs::write(table.join(identity.host_pid.to_string()),"uid\t0\ncap_effective\t0x200000\n").unwrap();assert_eq!(admin_only.attach(&endpoint),libc::EPERM);admin_only.reap();
 let mut stale=Actor::new();let identity=stale.identity();InitNamespaceEntry::capture(&table,identity).unwrap();std::fs::write(table.join(identity.host_pid.to_string()),"uid\t0\ncap_effective\t0x240000\n").unwrap();let entry=table.join(format!("{}.init-namespace-entry",identity.host_pid));let text=std::fs::read_to_string(&entry).unwrap();let mut fields=text.trim().split('\t').map(str::to_string).collect::<Vec<_>>();fields[3]=(identity.start_microseconds+1).to_string();std::fs::write(entry,fields.join("\t")+"\n").unwrap();assert_eq!(stale.attach(&endpoint),libc::EPERM);assert!(table.join(format!("{}.init-namespace-entry",identity.host_pid)).exists(),"different live birth is never removed");stale.reap();
 let mut foreign=Actor::new();let identity=foreign.identity();InitNamespaceEntry::capture(&table,identity).unwrap();std::fs::write(table.join(identity.host_pid.to_string()),"uid\t0\ncap_effective\t0x240000\n").unwrap();register_mount_namespace(&table,identity,bootstrap.id()).unwrap();assert_eq!(foreign.attach(&endpoint),libc::EPERM);foreign.reap();
 let mut other_source=Actor::new();let identity=other_source.identity();InitNamespaceEntry::capture(&table,identity).unwrap();let entry=table.join(format!("{}.init-namespace-entry",identity.host_pid));let text=std::fs::read_to_string(&entry).unwrap();let mut fields=text.trim().split('\t').map(str::to_string).collect::<Vec<_>>();fields[4]=identity.host_pid.to_string();fields[5]=identity.start_seconds.to_string();fields[6]=identity.start_microseconds.to_string();std::fs::write(&entry,fields.join("\t")+"\n").unwrap();std::fs::write(table.join(identity.host_pid.to_string()),"uid\t0\ncap_effective\t0x240000\n").unwrap();assert_eq!(other_source.attach(&endpoint),libc::EPERM);other_source.reap();InitNamespaceEntry::retire_dead(&table,init).unwrap();assert!(entry.exists(),"foreign source owner entry is not retired");
 controller.shutdown().unwrap();
}
