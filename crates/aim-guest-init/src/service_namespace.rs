//! The original init launch policy is image data, not runtime name dispatch.
use std::{collections::BTreeMap,fs,path::Path};
use aim_storage::{mount_namespace::Namespace,process_namespace::{self,ProcessIdentity}};
use crate::paths::Layout;
const POLICY:&str="vendor/etc/aim/init-mount-namespace-policy.conf";
const CATALOG:&str="init-mount-namespace-owners";
#[derive(Clone,Copy,Debug,PartialEq,Eq)]pub enum Selection{Current,Bootstrap,Default}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]enum Rule{Current,Bootstrap,Default,Ready}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]pub enum LaunchOrigin{Service,Transient,Helper}
pub struct Policy{default:Rule,services:BTreeMap<String,Rule>,executables:BTreeMap<String,Rule>}
impl Policy{
 pub fn parse(text:&str)->Result<Self,String>{
  let mut lines=text.lines().filter(|line|!line.trim().is_empty()&&!line.starts_with('#'));if lines.next()!=Some("AIM_INIT_NAMESPACE_POLICY1"){return Err("invalid service namespace policy header".into());}
  let mut result=Self{default:Rule::Ready,services:BTreeMap::new(),executables:BTreeMap::new()};let(mut source,mut digest,mut default)=(false,false,false);
  let rule=|value:&str|match value{"current"=>Ok(Rule::Current),"bootstrap"=>Ok(Rule::Bootstrap),"default"=>Ok(Rule::Default),"ready"=>Ok(Rule::Ready),_=>Err("invalid service namespace rule".to_owned())};
  for line in lines{let fields=line.split_whitespace().collect::<Vec<_>>();match fields.as_slice(){
   ["source",value]if !source&&value.starts_with("https://android.googlesource.com/")=>source=true,
   ["sha256",value]if !digest&&value.len()==64&&value.bytes().all(|byte|byte.is_ascii_hexdigit())=>digest=true,
   ["default",value]if !default=>{result.default=rule(value)?;default=true;},
   ["service",name,value]=>{if result.services.insert((*name).into(),rule(value)?).is_some(){return Err("duplicate service namespace rule".into());}},
   ["executable",name,value]if name.starts_with('/')=>{if result.executables.insert((*name).into(),rule(value)?).is_some(){return Err("duplicate executable namespace rule".into());}},
   _=>return Err(format!("invalid namespace policy row: {line}")),
  }}
  if !source||!digest||!default{return Err("incomplete service namespace policy provenance".into());}Ok(result)
 }
 fn choose(&self,service:&str,executable:&str,ready:bool)->Selection{
  match self.executables.get(executable).or_else(||self.services.get(service)).copied().unwrap_or(self.default){Rule::Current=>Selection::Current,Rule::Bootstrap=>Selection::Bootstrap,Rule::Default=>Selection::Default,Rule::Ready=>if ready{Selection::Default}else{Selection::Bootstrap}}
 }
}
#[derive(Clone)]pub struct Catalog{pub owner:ProcessIdentity,pub bootstrap:String,pub default:String,pub ready:bool}
impl Catalog{
 pub fn publish(&self,runtime:&Path)->Result<(),String>{let path=runtime.join(CATALOG);let temp=path.with_extension("next");let text=format!("AIM_INIT_MOUNT_OWNERS1\t{}\t{}\t{}\t{}\t{}\t{}\n",self.owner.host_pid,self.owner.start_seconds,self.owner.start_microseconds,self.bootstrap,self.default,u8::from(self.ready));fs::write(&temp,text).and_then(|_|fs::rename(&temp,&path)).map_err(|error|error.to_string())}
 pub fn read(runtime:&Path)->Result<Option<Self>,String>{
  let path=runtime.join(CATALOG);let text=match fs::read_to_string(path){Ok(text)=>text,Err(error)if error.kind()==std::io::ErrorKind::NotFound=>{if runtime.join("identity/by-pid/namespace-init").exists(){return Err("managed namespace owner catalog is absent".into());}return Ok(None);},Err(error)=>return Err(error.to_string())};
  let f=text.trim_end_matches('\n').split('\t').collect::<Vec<_>>();if f.len()!=7||f[0]!="AIM_INIT_MOUNT_OWNERS1"{return Err("invalid namespace owner catalog".into());}
  let value=Self{owner:ProcessIdentity{host_pid:f[1].parse().map_err(|_|"invalid owner PID")?,start_seconds:f[2].parse().map_err(|_|"invalid owner birth")?,start_microseconds:f[3].parse().map_err(|_|"invalid owner birth")?},bootstrap:f[4].into(),default:f[5].into(),ready:match f[6]{"0"=>false,"1"=>true,_=>return Err("invalid default namespace readiness".into())}};
  let init=process_namespace::InitRegistration::read(&runtime.join("identity/by-pid")).map_err(|error|error.to_string())?;if init.process!=value.owner||init.mount_namespace!=value.bootstrap{return Err("namespace catalog belongs to another init".into());}
  Namespace::open(runtime,&value.bootstrap).and_then(|owner|owner.read()).map_err(|error|error.to_string())?;Namespace::open(runtime,&value.default).and_then(|owner|owner.read()).map_err(|error|error.to_string())?;Ok(Some(value))
 }
 pub fn selected(&self,runtime:&Path,selection:Selection)->Result<String,String>{match selection{Selection::Bootstrap=>Ok(self.bootstrap.clone()),Selection::Default=>Ok(self.default.clone()),Selection::Current=>process_namespace::mount_namespace_of(&runtime.join("identity/by-pid"),self.owner).map_err(|error|error.to_string())}}
}
pub fn ready(runtime:&Path)->Result<bool,String>{Ok(Catalog::read(runtime)?.is_none_or(|owner|owner.ready))}
pub fn plan(layout:&Layout,service:&str,executable:&str,origin:LaunchOrigin,previous:Option<Selection>)->Result<(Option<String>,Option<Selection>),String>{
 let Some(owner)=Catalog::read(&layout.runtime)?else{return Ok((None,None));};let policy=Policy::parse(&fs::read_to_string(layout.image.join(POLICY)).map_err(|error|format!("image namespace policy: {error}"))?)?;
 let choice=if origin==LaunchOrigin::Helper{Selection::Current}else{previous.unwrap_or_else(||policy.choose(service,executable,owner.ready))};let keep=(origin==LaunchOrigin::Service&&choice!=Selection::Current).then_some(choice);Ok((Some(owner.selected(&layout.runtime,choice)?),keep))
}
pub fn linker_config_completed(runtime:&Path)->Result<(),String>{if let Some(mut owner)=Catalog::read(runtime)?{let current=owner.selected(runtime,Selection::Current)?;if current==owner.default{owner.ready=true;owner.publish(runtime)?;}}Ok(())}

pub fn admit(runtime:&Path,process:ProcessIdentity,selected:Option<&str>)->Result<(),String>{
 let table=runtime.join("identity/by-pid");let init=process_namespace::InitRegistration::read(&table).map_err(|error|error.to_string())?;
 let current=process_namespace::mount_namespace_of(&table,init.process).map_err(|error|error.to_string())?;let id=selected.unwrap_or(&current);
 Namespace::open(runtime,id).and_then(|owner|owner.read()).map_err(|error|format!("selected child namespace: {error}"))?;
 process_namespace::register_mount_namespace(&table,process,id).map_err(|error|error.to_string())
}

#[cfg(test)]
mod tests{
 use super::*;
 const CONFIG:&str=include_str!("../../../image/vendor/etc/aim/init-mount-namespace-policy.conf");
 fn fixture()->(PathBuf,Layout,Catalog){
  let root=std::env::temp_dir().join(format!("aim-service-namespace-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
  let layout=Layout{image:root.join("image"),data:root.join("data"),runtime:root.join("run")};fs::create_dir_all(layout.runtime.join("identity/by-pid")).unwrap();fs::create_dir_all(layout.image.join("vendor/etc/aim")).unwrap();fs::write(layout.image.join(POLICY),CONFIG).unwrap();
  let owner=ProcessIdentity::running(unsafe{libc::getpid()}).unwrap();let bootstrap=Namespace::open(&layout.runtime,"actual-bootstrap").unwrap();let default=Namespace::open(&layout.runtime,"actual-default").unwrap();
  let base=format!("root\t/\t{}\n",layout.image.display());bootstrap.initialize(&base).unwrap();default.initialize(&base).unwrap();let table=layout.identity_dir().join("by-pid");process_namespace::InitRegistration::register(&table,owner,bootstrap.id()).unwrap();process_namespace::register_mount_namespace(&table,owner,bootstrap.id()).unwrap();
  let catalog=Catalog{owner,bootstrap:bootstrap.id().into(),default:default.id().into(),ready:false};catalog.publish(&layout.runtime).unwrap();(root,layout,catalog)
 }
 use std::path::PathBuf;
 #[test]
 fn strict_image_policy_and_original_selection(){let policy=Policy::parse(CONFIG).unwrap();assert_eq!(policy.choose("servicemanager","/system/bin/servicemanager",false),Selection::Default);assert_eq!(policy.choose("ordinary","/system/bin/ordinary",false),Selection::Bootstrap);assert_eq!(policy.choose("ordinary","/system/bin/ordinary",true),Selection::Default);assert_eq!(policy.choose("any","/system/bin/apexd",true),Selection::Current);assert!(Policy::parse(&format!("{CONFIG}service servicemanager bootstrap\n")).is_err());assert!(Policy::parse("AIM_INIT_NAMESPACE_POLICY1\ndefault ready\n").is_err());}
 #[test]
 fn service_marking_and_transient_linker_helpers_follow_actual_namespace(){
  let(root,layout,catalog)=fixture();let(_,remembered)=plan(&layout,"ordinary","/system/bin/tool",LaunchOrigin::Service,None).unwrap();assert_eq!(remembered,Some(Selection::Bootstrap));let first=plan(&layout,"linkerconfig","/apex/runtime/linkerconfig",LaunchOrigin::Helper,None).unwrap();assert_eq!(first.0.as_deref(),Some(catalog.bootstrap.as_str()));
  process_namespace::register_mount_namespace(&layout.identity_dir().join("by-pid"),catalog.owner,&catalog.default).unwrap();assert!(!ready(&layout.runtime).unwrap(),"entering default is not linkerconfig success");let second=plan(&layout,"linkerconfig","/apex/runtime/linkerconfig",LaunchOrigin::Helper,None).unwrap();assert_eq!(second.0.as_deref(),Some(catalog.default.as_str()));linker_config_completed(&layout.runtime).unwrap();assert!(ready(&layout.runtime).unwrap());assert_eq!(plan(&layout,"ordinary","/system/bin/tool",LaunchOrigin::Service,remembered).unwrap().0.as_deref(),Some(catalog.bootstrap.as_str()));assert_eq!(plan(&layout,"late","/system/bin/tool",LaunchOrigin::Service,None).unwrap().0.as_deref(),Some(catalog.default.as_str()));fs::remove_dir_all(root).unwrap();
 }

 #[test]
 fn updatable_start_queues_without_disable_and_exec_start_never_queues(){
  use crate::{launch::{DryRunLauncher,LinuxRun,LinuxRunOptions},supervisor::{Planner,Supervisor,StartOutcome,template_service,flags}};use std::time::Instant;
  struct Properties;impl aim_android_init::PropertyLookup for Properties{fn property(&self,_:&str)->Option<String>{None}}
  let(root,layout,catalog)=fixture();fs::create_dir_all(layout.image.join("system/bin")).unwrap();fs::create_dir_all(layout.identity_dir()).unwrap();fs::write(layout.image.join("system/bin/updatable-test"),b"actual planner executable").unwrap();
  let planner=Planner{layout:layout.clone(),map:layout.path_map(),ids:aim_android_init::rc::IdResolver::builtin(),vendor_api_level:36,env:vec![],rlimits:vec![],boot_epoch:Instant::now()};
  let mut launcher=DryRunLauncher::new(LinuxRun{binary:PathBuf::from("unused-dryrun"),image:layout.image.clone(),path_map_file:layout.path_map_file(),binder:None,gpu:None,vulkan:None,display:None,trace:false,options:LinuxRunOptions::CONTRACT});
  let mut normal=template_service("updatable-start",vec!["/system/bin/updatable-test".into()]);normal.updatable=true;let mut exec=normal.clone();exec.name="updatable-exec".into();let mut supervisor=Supervisor::new();supervisor.sync(&[normal,exec]);
  for _ in 0..2{assert!(supervisor.start("updatable-start",&mut launcher,&planner,&Properties,Instant::now()).is_err());}
  assert!(supervisor.exec_start("updatable-exec",&mut launcher,&planner,&Properties,Instant::now()).is_err());assert!(launcher.launches.is_empty());assert_eq!(supervisor.record("updatable-start").unwrap().flags&flags::DISABLED,0);
  let delayed=supervisor.take_delayed();assert_eq!(delayed,vec!["updatable-start","updatable-start"]);process_namespace::register_mount_namespace(&layout.identity_dir().join("by-pid"),catalog.owner,&catalog.default).unwrap();linker_config_completed(&layout.runtime).unwrap();
  assert!(matches!(supervisor.start(&delayed[0],&mut launcher,&planner,&Properties,Instant::now()).unwrap(),StartOutcome::Started{..}));assert_eq!(supervisor.start(&delayed[1],&mut launcher,&planner,&Properties,Instant::now()).unwrap(),StartOutcome::AlreadyRunning);assert_eq!(launcher.launches.len(),1);assert_eq!(launcher.launches[0].1.mount_namespace.as_deref(),Some(catalog.default.as_str()));assert!(supervisor.take_delayed().is_empty());fs::remove_dir_all(root).unwrap();
 }
 #[test]
 fn actual_original_vendor_manifest_is_visible_in_selected_default_owner(){
  let(root,layout,catalog)=fixture();let original=aim_paths::original_image();let fragment=original.join("apex/com.android.hardware.power/etc/vintf/android.hardware.power.xml");let bytes=fs::read(&fragment).expect("original Power VINTF input is required, not a skipped gate");let target=layout.image.join("apex/com.android.hardware.power/etc/vintf");fs::create_dir_all(&target).unwrap();fs::write(target.join("android.hardware.power.xml"),&bytes).unwrap();let empty=root.join("bootstrap-apex");fs::create_dir(&empty).unwrap();let bootstrap=Namespace::open(&layout.runtime,&catalog.bootstrap).unwrap();let default=Namespace::open(&layout.runtime,&catalog.default).unwrap();let base=format!("root\t/\t{}\n",layout.image.display());bootstrap.update_base(&format!("{base}ro\t/apex\t{}\n",empty.display())).unwrap();default.update_base(&base).unwrap();
  let(selected,_)=plan(&layout,"servicemanager","/system/bin/servicemanager",LaunchOrigin::Service,None).unwrap();assert_eq!(selected.as_deref(),Some(catalog.default.as_str()));let map=crate::paths::PathMap::parse_file_text(&Namespace::open(&layout.runtime,selected.as_ref().unwrap()).unwrap().read().unwrap().base).unwrap();let visible=map.resolve("/apex/com.android.hardware.power/etc/vintf/android.hardware.power.xml",true).unwrap();assert_eq!(fs::read(visible.host).unwrap(),bytes);let hidden=crate::paths::PathMap::parse_file_text(&bootstrap.read().unwrap().base).unwrap();assert_eq!(fs::read(hidden.resolve("/apex/com.android.hardware.power/etc/vintf/android.hardware.power.xml",true).unwrap().host).unwrap_err().kind(),std::io::ErrorKind::NotFound);fs::remove_dir_all(root).unwrap();
 }
}

#[cfg(test)]
mod child_fixture{
 use super::*;
 use std::{io::{BufRead,Write},process::{Command,Stdio}};
 const CHILD:&str="service_namespace::child_fixture::service_namespace_child";
 #[test]
 #[ignore="real child executed by selected_default_child_reads_original_vintf"]
 fn service_namespace_child(){
  let mut line=String::new();std::io::stdin().read_line(&mut line).unwrap();let runtime=Path::new(line.trim());let process=ProcessIdentity::running(unsafe{libc::getpid()}).unwrap();let id=process_namespace::mount_namespace_of(&runtime.join("identity/by-pid"),process).unwrap();let catalog=Catalog::read(runtime).unwrap().unwrap();assert_eq!(id,catalog.default);let state=Namespace::open(runtime,&id).unwrap().read().unwrap();let map=crate::paths::PathMap::parse_file_text(&state.base).unwrap();let file=map.resolve("/apex/com.android.hardware.power/etc/vintf/android.hardware.power.xml",true).unwrap();let bytes=fs::read(file.host).unwrap();assert!(std::str::from_utf8(&bytes).unwrap().contains("IPower/default"));println!("DEFAULT_CHILD_ACTUAL_VINTF_EXECUTED");std::io::stdout().flush().unwrap();
 }
 #[test]
 fn selected_default_child_reads_original_vintf(){
  use std::path::PathBuf;let root=std::env::temp_dir().join(format!("aim-default-vintf-child-{}",std::process::id()));fs::create_dir(&root).unwrap();struct Directory(PathBuf);impl Drop for Directory{fn drop(&mut self){fs::remove_dir_all(&self.0).unwrap();}}let _directory=Directory(root.clone());
  let layout=Layout{image:root.join("image"),data:root.join("data"),runtime:root.join("run")};fs::create_dir_all(layout.runtime.join("identity/by-pid")).unwrap();fs::create_dir_all(layout.image.join("vendor/etc/aim")).unwrap();fs::write(layout.image.join(POLICY),include_str!("../../../image/vendor/etc/aim/init-mount-namespace-policy.conf")).unwrap();let image=aim_paths::original_image();let fragment=image.join("apex/com.android.hardware.power/etc/vintf/android.hardware.power.xml");assert!(fragment.is_file(),"actual original VINTF input required");let process=ProcessIdentity::running(unsafe{libc::getpid()}).unwrap();let table=layout.identity_dir().join("by-pid");let boot=Namespace::open(&layout.runtime,"bootstrap").unwrap();let default=Namespace::open(&layout.runtime,"default").unwrap();let empty=root.join("empty-apex");fs::create_dir(&empty).unwrap();let base=format!("root\t/\t{}\n",image.display());boot.initialize(&format!("{base}ro\t/apex\t{}\n",empty.display())).unwrap();default.initialize(&base).unwrap();process_namespace::InitRegistration::register(&table,process,boot.id()).unwrap();process_namespace::register_mount_namespace(&table,process,boot.id()).unwrap();Catalog{owner:process,bootstrap:boot.id().into(),default:default.id().into(),ready:false}.publish(&layout.runtime).unwrap();let(selected,_)=plan(&layout,"servicemanager","/system/bin/servicemanager",LaunchOrigin::Service,None).unwrap();
  struct Child(std::process::Child);impl Drop for Child{fn drop(&mut self){if self.0.try_wait().unwrap().is_none(){self.0.kill().unwrap();}self.0.wait().unwrap();}}let mut child=Child(Command::new(std::env::current_exe().unwrap()).args(["--exact",CHILD,"--ignored","--nocapture"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap());let identity=ProcessIdentity::running(child.0.id()as i32).unwrap();admit(&layout.runtime,identity,selected.as_deref()).unwrap();let mut input=child.0.stdin.take().unwrap();writeln!(input,"{}",layout.runtime.display()).unwrap();drop(input);let mut output=std::io::BufReader::new(child.0.stdout.take().unwrap());let mut observed=false;loop{let mut line=String::new();if output.read_line(&mut line).unwrap()==0{break;}if line.contains("DEFAULT_CHILD_ACTUAL_VINTF_EXECUTED"){observed=true;}}assert!(child.0.wait().unwrap().success());assert!(observed,"actual child assertions did not run");
 }
}
