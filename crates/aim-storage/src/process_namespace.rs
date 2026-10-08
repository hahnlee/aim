//! Authenticated guest process identities shared by native init and syscall owners (#1158).
//! A recorded host process is valid only for its exact kernel start time.
use std::{fs,io,path::{Path,PathBuf},time::{SystemTime,UNIX_EPOCH}};

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub struct ProcessIdentity {pub host_pid:i32,pub start_seconds:u64,pub start_microseconds:u64}
impl ProcessIdentity {
    pub fn running(pid:i32)->io::Result<Self>{
        if pid<=0{return Err(io::Error::from_raw_os_error(libc::ESRCH));}
        let mut info:libc::proc_bsdinfo=unsafe{std::mem::zeroed()};let size=std::mem::size_of_val(&info) as i32;
        if unsafe{libc::proc_pidinfo(pid,libc::PROC_PIDTBSDINFO,1,(&mut info as *mut libc::proc_bsdinfo).cast(),size)}!=size{return Err(io::Error::from_raw_os_error(libc::ESRCH));}
        Ok(Self{host_pid:pid,start_seconds:info.pbi_start_tvsec,start_microseconds:info.pbi_start_tvusec})
    }
    pub fn is_live(self)->bool{Self::running(self.host_pid).is_ok_and(|current|current==self)}
}
#[derive(Clone,Debug)]
pub struct InitRegistration {pub process:ProcessIdentity,pub mount_namespace:String}
impl InitRegistration {
    fn text(&self)->String{format!("AIMNS1\t{}\t{}\t{}\t{}\n",self.process.host_pid,self.process.start_seconds,self.process.start_microseconds,self.mount_namespace)}
    pub fn register(table:&Path,process:ProcessIdentity,mount_namespace:&str)->io::Result<Self>{
        if !process.is_live()||mount_namespace.is_empty()||mount_namespace.contains(['\t','\n','/','\0']){return Err(io::Error::from_raw_os_error(libc::EINVAL));}
        fs::create_dir_all(table)?;let value=Self{process,mount_namespace:mount_namespace.into()};
        atomic_write(&table.join("namespace-init"),value.text().as_bytes())?;Ok(value)
    }
    pub fn read(table:&Path)->io::Result<Self>{
        let text=fs::read_to_string(table.join("namespace-init"))?;let fields:Vec<_>=text.trim_end_matches('\n').split('\t').collect();
        if fields.len()!=5||fields[0]!="AIMNS1"{return Err(io::Error::from_raw_os_error(libc::EPROTO));}
        let number=|at:usize|fields[at].parse::<u64>().map_err(|_|io::Error::from_raw_os_error(libc::EPROTO));
        let host_pid=i32::try_from(number(1)?).map_err(|_|io::Error::from_raw_os_error(libc::EPROTO))?;
        let value=Self{process:ProcessIdentity{host_pid,start_seconds:number(2)?,start_microseconds:number(3)?},mount_namespace:fields[4].into()};
        if !value.process.is_live(){return Err(io::Error::from_raw_os_error(libc::ESRCH));}Ok(value)
    }
    pub fn activate_pid_mapping(&self,table:&Path)->io::Result<()> {
        let current=Self::read(table)?;
        if current.process!=self.process||current.mount_namespace!=self.mount_namespace{return Err(io::Error::from_raw_os_error(libc::ESRCH));}
        init_caught_signals(table,self.process)?;
        atomic_write(&table.join("namespace-pid-ready"),b"AIMNS-PID-READY1\n")
    }
    pub fn remove(&self,table:&Path)->io::Result<()> {
        if Self::read(table).is_ok_and(|current|current.process==self.process){fs::remove_file(table.join("namespace-init"))?;}Ok(())
    }
}
pub fn register_mount_namespace(table:&Path,process:ProcessIdentity,namespace:&str)->io::Result<()> {
    if !process.is_live()||namespace.is_empty()||namespace.contains(['/', '\t','\n','\0']){return Err(io::Error::from_raw_os_error(libc::EINVAL));}
    atomic_write(&table.join(format!("{}.mount-namespace",process.host_pid)),
        format!("AIMPROCNS1\t{}\t{}\t{}\t{}\n",process.host_pid,process.start_seconds,process.start_microseconds,namespace).as_bytes())
}
pub fn mount_namespace_of(table:&Path,process:ProcessIdentity)->io::Result<String> {
    let text=fs::read_to_string(table.join(format!("{}.mount-namespace",process.host_pid)))?;
    let fields:Vec<_>=text.trim_end_matches('\n').split('\t').collect();
    if fields.len()!=5||fields[0]!="AIMPROCNS1"||fields[1].parse()!=Ok(process.host_pid)
        ||fields[2].parse()!=Ok(process.start_seconds)||fields[3].parse()!=Ok(process.start_microseconds)
        ||!process.is_live(){return Err(io::Error::from_raw_os_error(libc::ESRCH));}
    Ok(fields[4].into())
}

pub fn register_init_signals(table:&Path,process:ProcessIdentity,caught:u64)->io::Result<()> {
    if !process.is_live(){return Err(io::Error::from_raw_os_error(libc::ESRCH));}
    atomic_write(&table.join("namespace-init-signals"),format!("AIMINIT-SIGNALS1\t{}\t{}\t{}\t{caught:x}\n",process.host_pid,process.start_seconds,process.start_microseconds).as_bytes())
}
pub fn init_caught_signals(table:&Path,process:ProcessIdentity)->io::Result<u64>{
    let text=fs::read_to_string(table.join("namespace-init-signals"))?;let fields=text.trim_end_matches('\n').split('\t').collect::<Vec<_>>();
    if fields.len()!=5||fields[0]!="AIMINIT-SIGNALS1"||fields[1].parse()!=Ok(process.host_pid)||fields[2].parse()!=Ok(process.start_seconds)||fields[3].parse()!=Ok(process.start_microseconds)||!process.is_live(){return Err(io::Error::from_raw_os_error(libc::ESRCH));}
    u64::from_str_radix(fields[4],16).map_err(|_|io::Error::from_raw_os_error(libc::EPROTO))
}

fn atomic_write(path:&Path,bytes:&[u8])->io::Result<()> {
    use std::io::Write;let nonce=SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let temporary:PathBuf=path.with_extension(format!("{}-{nonce}.tmp",std::process::id()));
    let result=(||{let mut file=fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;file.write_all(bytes)?;file.sync_all()?;fs::rename(&temporary,path)})();
    if result.is_err(){let _=fs::remove_file(temporary);}result
}
#[cfg(test)]
mod tests{
 use super::*;use std::process::{Command,Stdio};
 struct Child(std::process::Child);
 impl Drop for Child{fn drop(&mut self){if self.0.try_wait().ok().flatten().is_none(){let _=self.0.kill();}let _=self.0.wait();}}
 #[test]
 fn actual_process_start_time_rejects_reused_or_dead_init_identity(){
  let root=std::env::temp_dir().join(format!("aim-pid-owner-{}-{}",std::process::id(),SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));fs::create_dir(&root).unwrap();
  let mut child=Child(Command::new("sleep").arg("30").stdout(Stdio::null()).spawn().unwrap());
  let process=ProcessIdentity::running(child.0.id() as i32).unwrap();let registration=InitRegistration::register(&root,process,"namespace-original").unwrap();
  assert_eq!(InitRegistration::read(&root).unwrap().process,process);
  let stale=ProcessIdentity{start_microseconds:process.start_microseconds.wrapping_add(1),..process};
  assert!(!stale.is_live());assert!(InitRegistration::register(&root,stale,"namespace-original").is_err());
  child.0.kill().unwrap();child.0.wait().unwrap();assert!(InitRegistration::read(&root).is_err());
  registration.remove(&root).unwrap();fs::remove_dir_all(root).unwrap();
 }
}

pub fn signal_to_host(sig: i32) -> i32 {
    match sig {
        1 => libc::SIGHUP,
        2 => libc::SIGINT,
        3 => libc::SIGQUIT,
        4 => libc::SIGILL,
        5 => libc::SIGTRAP,
        6 => libc::SIGABRT,
        7 => libc::SIGBUS,
        8 => libc::SIGFPE,
        9 => libc::SIGKILL,
        10 => libc::SIGUSR1,
        11 => libc::SIGSEGV,
        12 => libc::SIGUSR2,
        13 => libc::SIGPIPE,
        14 => libc::SIGALRM,
        15 => libc::SIGTERM,
        17 => libc::SIGCHLD,
        18 => libc::SIGCONT,
        19 => libc::SIGSTOP,
        20 => libc::SIGTSTP,
        21 => libc::SIGTTIN,
        22 => libc::SIGTTOU,
        23 => libc::SIGURG,
        24 => libc::SIGXCPU,
        25 => libc::SIGXFSZ,
        26 => libc::SIGVTALRM,
        27 => libc::SIGPROF,
        28 => libc::SIGWINCH,
        29 => libc::SIGIO,
        31 => libc::SIGSYS,
        _ => 0,
    }
}

pub fn signal_from_host(host:i32)->i32{(1..32).find(|&signal|signal_to_host(signal)==host).unwrap_or(0)}
