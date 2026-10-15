//! Original ART consumer of the native true-mode typed Binder proxy capability.
use aim_binder_driver::{Credentials, Device, GuestProcess};
use aim_binder_host::{local::LocalProcess, parcel::Binder};
use aim_services::package::{
    installer::{
        self,
        native::NativeOwners,
        policy::{DevicePolicy, UserPolicy},
    },
    model::{PackageState, PackageUserState, State, User},
    pkg::AndroidPackage,
    system_config::SystemConfig,
};
use std::{
    fs,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
mod common {
    pub mod java;
    pub mod runtime;
}
#[path = "../../aim-build/src/nodes/binder_ready.rs"]
mod binder_ready;
use common::runtime::{Boot, Data, run};

// SET_CONTEXT_MGR_EXT reads only its inline object; no guest pointers or files.
struct Inline;
impl GuestProcess for Inline {
    fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), i32> {
        Err(libc::EFAULT)
    }
    fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), i32> {
        Err(libc::EFAULT)
    }
    fn get_file(&mut self, _: u32) -> Result<aim_binder_driver::File, i32> {
        Err(libc::EBADF)
    }
    fn install_file(&mut self, _: aim_binder_driver::File) -> Result<u32, i32> {
        Err(libc::EBADF)
    }
    fn close_fd(&mut self, _: u32) {
        unreachable!()
    }
}
struct FixtureData {inner:std::mem::ManuallyDrop<Data>,failed:Arc<std::sync::atomic::AtomicBool>}
impl std::ops::Deref for FixtureData {type Target=Data;fn deref(&self)->&Data{&self.inner}}
impl FixtureData {fn finish(&self)->std::io::Result<()> {
    fs::remove_dir_all(&self.inner.0).inspect_err(|_|self.failed.store(true,std::sync::atomic::Ordering::Release))
}}
impl Drop for FixtureData {fn drop(&mut self){
    if std::thread::panicking()||self.failed.load(std::sync::atomic::Ordering::Acquire){eprintln!("preserved failed owned fixture {}",self.inner.0.display());return;}
    if self.inner.0.exists(){eprintln!("preserved unfinished owned fixture {}",self.inner.0.display());}
}}
struct BootStop {boot:Arc<Boot>,failed:Arc<std::sync::atomic::AtomicBool>,done:std::cell::Cell<bool>}
impl BootStop {fn finish(&self)->std::io::Result<()> {
    let output=capture_client(self.boot.command().arg("stop"),self.boot.data.parent().unwrap(),"boot-stop",Duration::from_secs(60))?;
    if !output.status.success(){return Err(std::io::Error::other(format!("owned boot stop {}: {}",output.status,String::from_utf8_lossy(&output.stderr))));}
    self.done.set(true);Ok(())
}}
impl Drop for BootStop {fn drop(&mut self){
    if self.done.get(){return;}
    let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||self.finish()));
    if !matches!(result,Ok(Ok(()))){self.failed.store(true,std::sync::atomic::Ordering::Release);eprintln!("owned boot stop failed during cleanup: {result:?}; preserved failure and data");}
}}
struct NativeStop {owners:Arc<NativeOwners>,process:Arc<LocalProcess>,failed:Arc<std::sync::atomic::AtomicBool>,done:std::cell::Cell<bool>}
impl NativeStop {fn finish(&self)->Result<(),String> {
    let external=self.owners.close_external().map_err(|error|format!("native installer close: {error:?}"));
    self.owners.shutdown_callbacks();self.owners.shutdown_io();
    let process=self.process.shutdown().map_err(|error|format!("native Binder shutdown: {error}"));
    external?;process?;self.done.set(true);Ok(())
}}
impl Drop for NativeStop {fn drop(&mut self){
    if self.done.get(){return;}
    let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||self.finish()));
    if !matches!(result,Ok(Ok(()))){self.failed.store(true,std::sync::atomic::Ordering::Release);eprintln!("native owner cleanup failed: {result:?}; preserved failure and data");}
}}
fn isolated_client(boot: &Boot, name: &str) -> Command {
    let original = boot.client(0);
    let mut client = Command::new(original.get_program());
    client.env_clear();
    for (key, value) in original.get_envs() {
        if let Some(value) = value {
            client.env(key, value);
        }
    }
    let mut args = original.get_args();
    while let Some(arg) = args.next() {
        if arg == "--binder" {
            args.next();
            client.arg(arg).arg(name);
        } else {
            client.arg(arg);
        }
    }
    client
}
struct ClientGuard(Option<Child>);
impl Drop for ClientGuard {
    fn drop(&mut self) {
        let Some(child)=self.0.as_mut() else{return};
        if matches!(child.try_wait(),Ok(Some(_))) { let _=child.wait(); return; }
        unsafe { libc::kill(child.id() as i32,libc::SIGTERM); }
        let grace=Instant::now()+Duration::from_secs(1);
        while Instant::now()<grace {
            if matches!(child.try_wait(),Ok(Some(_))) { let _=child.wait(); return; }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _=child.kill(); let _=child.wait();
    }
}
fn capture_client(command:&mut Command,dir:&std::path::Path,label:&str,limit:Duration)->std::io::Result<std::process::Output> {
    use std::io::Read;
    let out=dir.join(format!("{label}.stdout"));let err=dir.join(format!("{label}.stderr"));
    let child=command.stdout(fs::File::create(&out)?).stderr(fs::File::create(&err)?).spawn()?;
    let mut guard=ClientGuard(Some(child));let pid=guard.0.as_ref().unwrap().id();let deadline=Instant::now()+limit;
    let status=loop {
        if let Some(status)=guard.0.as_mut().unwrap().try_wait()? {break status;}
        if Instant::now()>=deadline {return Err(std::io::Error::new(std::io::ErrorKind::TimedOut,format!("original client PID{pid} exceeded {limit:?}; output files {} / {}",out.display(),err.display())));}
        std::thread::sleep(Duration::from_millis(10));
    };
    guard.0.as_mut().unwrap().wait()?;
    let read=|path:&std::path::Path|->std::io::Result<Vec<u8>> {
        let mut bytes=Vec::new();fs::File::open(path)?.take(65537).read_to_end(&mut bytes)?;
        if bytes.len()>65536 {return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,format!("original client output exceeds64KiB: {}",path.display())));}
        Ok(bytes)
    };
    let stdout=read(&out);let stderr=read(&err);
    if !status.success() && (stdout.is_err() || stderr.is_err()) {
        return Err(std::io::Error::other(format!("original client PID{pid} {status}; stdout:{stdout:?}; stderr:{stderr:?}")));
    }
    Ok(std::process::Output{status,stdout:stdout?,stderr:stderr?})
}
fn original_client(command:&mut Command,dir:&std::path::Path)->std::process::Output {
    capture_client(command,dir,"proxy-client",Duration::from_secs(60)).expect("original proxy capture")
}
const PERMISSION_FRAME_MAX: usize = 65536;
fn permission_io_error(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
}
fn permission_nonblocking(fd: i32) -> std::io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
fn permission_wait(fd: i32, events: i16, deadline: Instant) -> std::io::Result<()> {
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() { return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "permission channel deadline")); }
        let mut poll = libc::pollfd { fd, events, revents: 0 };
        let count = unsafe { libc::poll(&mut poll, 1, remaining.as_millis().clamp(1, i32::MAX as u128) as i32) };
        if count < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted { continue; }
            return Err(error);
        }
        if count > 0 {
            if poll.revents & libc::POLLNVAL != 0 { return Err(permission_io_error("permission descriptor invalid")); }
            return Ok(());
        }
    }
}
fn permission_copy(fd: i32, bytes: &mut [u8], write: bool, deadline: Instant) -> std::io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        permission_wait(fd, if write { libc::POLLOUT } else { libc::POLLIN }, deadline)?;
        let count = unsafe { if write { libc::write(fd, bytes[offset..].as_ptr().cast(), bytes.len()-offset) }
            else { libc::read(fd, bytes[offset..].as_mut_ptr().cast(), bytes.len()-offset) } };
        if count < 0 {
            let error = std::io::Error::last_os_error();
            if matches!(error.kind(), std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock) { continue; }
            return Err(error);
        }
        if count == 0 { return Err(permission_io_error("permission channel EOF")); }
        offset += count as usize;
    }
    Ok(())
}
fn permission_idle(fd: i32) -> std::io::Result<()> {
    let mut byte = [0u8];
    let count = unsafe { libc::read(fd, byte.as_mut_ptr().cast(), 1) };
    if count >= 0 { return Err(permission_io_error(if count == 0 {"permission channel unexpectedly closed"} else {"extra permission response bytes"})); }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::WouldBlock { Ok(()) } else { Err(error) }
}
fn permission_send(fd: i32, payload: &[u8], deadline: Instant) -> std::io::Result<()> {
    if !(10..=PERMISSION_FRAME_MAX).contains(&payload.len()) { return Err(permission_io_error("permission frame length")); }
    let mut bytes = (payload.len() as u32).to_be_bytes().to_vec(); bytes.extend(payload);
    permission_copy(fd, &mut bytes, true, deadline)
}
fn permission_receive(fd: i32, log: &mut fs::File, deadline: Instant) -> std::io::Result<Vec<u8>> {
    use std::io::Write;
    let mut length = [0;4]; permission_copy(fd, &mut length, false, deadline)?; log.write_all(&length)?;
    let length = u32::from_be_bytes(length) as usize;
    if !(10..=PERMISSION_FRAME_MAX).contains(&length) { return Err(permission_io_error("permission reply frame length")); }
    if log.metadata()?.len().saturating_add(length as u64) > PERMISSION_FRAME_MAX as u64 { return Err(permission_io_error("permission transcript exceeds64KiB")); }
    let mut bytes = vec![0;length]; permission_copy(fd, &mut bytes, false, deadline)?; log.write_all(&bytes)?; log.flush()?;
    Ok(bytes)
}
struct PermissionFrame<'a> { bytes: &'a [u8] }
impl<'a> PermissionFrame<'a> {
    fn take(&mut self, count: usize) -> std::io::Result<&'a [u8]> {
        if count > self.bytes.len() { return Err(permission_io_error("permission reply truncated")); }
        let (value, tail) = self.bytes.split_at(count); self.bytes = tail; Ok(value)
    }
    fn int(&mut self) -> std::io::Result<i32> { Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap())) }
    fn long(&mut self) -> std::io::Result<u64> { Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap())) }
    fn string(&mut self) -> std::io::Result<String> {
        let count = self.int()?; if !(0..=60000).contains(&count) { return Err(permission_io_error("permission string length")); }
        String::from_utf8(self.take(count as usize)?.to_vec()).map_err(|e|permission_io_error(e.to_string()))
    }
    fn header(&mut self, kind: u8, sequence: u64) -> std::io::Result<()> {
        if self.take(1)? != [1] { return Err(permission_io_error("permission protocol version")); }
        let actual_kind = self.take(1)?[0]; let actual_sequence = self.long()?;
        if actual_sequence != sequence { return Err(permission_io_error("permission sequence differs")); }
        if actual_kind == 6 { let message = self.string()?; self.end()?; return Err(permission_io_error(format!("original permission exception: {message}"))); }
        if actual_kind != kind { return Err(permission_io_error("permission response kind")); }
        Ok(())
    }
    fn end(&self) -> std::io::Result<()> { if self.bytes.is_empty() { Ok(()) } else { Err(permission_io_error("permission reply tail")) } }
}
fn permission_result(bytes: &[u8], sequence: u64, reader: i32, permission: &str, pid: i32, uid: i32) -> std::io::Result<i32> {
    let mut frame=PermissionFrame{bytes};frame.header(3,sequence)?;
    if frame.int()?!=reader || frame.int()?!=1000 || frame.string()?!=permission || frame.int()?!=pid || frame.int()?!=uid {return Err(permission_io_error("permission response identity/subject differs"));}
    let status=frame.int()?;if ![0,-1].contains(&status){return Err(permission_io_error("permission result invalid"));}frame.end()?;Ok(status)
}
fn permission_request(kind: u8, sequence: u64) -> Vec<u8> { let mut bytes=vec![1,kind]; bytes.extend(sequence.to_be_bytes()); bytes }
fn permission_string(bytes: &mut Vec<u8>, value: &str) -> std::io::Result<()> {
    if value.len()>60000 { return Err(permission_io_error("permission string exceeds bound")); }
    bytes.extend((value.len() as u32).to_be_bytes()); bytes.extend(value.as_bytes()); Ok(())
}
struct PermissionOracle {
    guard: Arc<std::sync::Mutex<ClientGuard>>,
    birth: aim_storage::process_namespace::ProcessIdentity,
    input: Option<std::process::ChildStdin>,
    output: std::process::ChildStdout,
    transcript: fs::File,
    stderr: std::path::PathBuf,
    sequence: u64,
    completed: Vec<(u64,i32)>,
    terminal: bool,
}
impl PermissionOracle {
    fn start(boot: &Boot) -> std::io::Result<Self> {
        use std::os::fd::AsRawFd;
        let deadline=Instant::now()+Duration::from_secs(60);
        let root=boot.data.parent().unwrap(); let stderr=root.join("permission-resident.stderr");
        let mut command=boot.checked_client(0).map_err(permission_io_error)?;
        command.args(["/data/local/tmp/credential-drop","1000","/data/local/tmp/permission-resident-proof",
            "/system/bin/app_process","-Djava.class.path=/data/local/tmp/proxy-oracle.dex","/system/bin","InstallerProxyOracle","permission-server"]);
        let child=command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(fs::File::create(&stderr)?).spawn()?;
        let mut guard=ClientGuard(Some(child)); let child=guard.0.as_mut().unwrap();
        let birth=aim_storage::process_namespace::ProcessIdentity::running(child.id() as i32)?;
        let input=child.stdin.take().ok_or_else(||permission_io_error("permission stdin missing"))?;
        let output=child.stdout.take().ok_or_else(||permission_io_error("permission stdout missing"))?;
        permission_nonblocking(input.as_raw_fd())?; permission_nonblocking(output.as_raw_fd())?;
        let mut owner=Self {guard:Arc::new(std::sync::Mutex::new(guard)),birth,input:Some(input),output,transcript:fs::File::create(root.join("permission-resident.stdout"))?,stderr,sequence:0,completed:Vec::new(),terminal:false};
        let bytes=permission_receive(owner.output.as_raw_fd(), &mut owner.transcript, deadline)?;
        let mut frame=PermissionFrame{bytes:&bytes}; frame.header(1,0)?;
        if frame.int()? != birth.host_pid || frame.int()? != 1000 || frame.int()? != birth.host_pid || frame.int()? != 1000 {
            return Err(permission_io_error("permission READY actual identity differs"));
        }
        frame.end()?; permission_idle(owner.output.as_raw_fd())?;
        let proof=fs::read_to_string(boot.data.join("data/local/tmp/permission-resident-proof"))?;
        let post=format!("post pid={} uid=1000 euid=1000 suid=1000 gid=1000 egid=1000 sgid=1000 groups=0 caps=0",birth.host_pid);
        if !proof.lines().any(|line|line==post) || !birth.is_live() { return Err(permission_io_error("permission READY credential/birth proof differs")); }
        owner.stderr_exact("PERMISSION_ENTERED_MAIN\n")?;
        if Instant::now()>=deadline { return Err(std::io::Error::new(std::io::ErrorKind::TimedOut,"permission READY validation deadline")); }
        println!("actual original permission READY pid={} uid=1000 groups=0 caps=0 preparation_ms={}",birth.host_pid,Duration::from_secs(60).saturating_sub(deadline.saturating_duration_since(Instant::now())).as_millis());
        Ok(owner)
    }
    fn stderr_exact(&self, expected: &str) -> std::io::Result<()> {
        use std::io::Read;
        let mut bytes=Vec::new();fs::File::open(&self.stderr)?.take(65537).read_to_end(&mut bytes)?;
        if bytes.len()>65536 || bytes!=expected.as_bytes() { return Err(permission_io_error(format!("permission phase receipt differs: {}",String::from_utf8_lossy(&bytes)))); }
        Ok(())
    }
    fn phase_receipt(&self) -> String {
        let mut result=String::from("PERMISSION_ENTERED_MAIN\n");
        for (seq,status) in &self.completed {
            result.push_str(&format!("PERMISSION_REQUEST sequence={seq}\n"));
            for phase in ["main","getService","checkPermission"] {result.push_str(&format!("PERMISSION_PHASE {phase} pid={} uid=1000\n",self.birth.host_pid));}
            result.push_str(&format!("PERMISSION_PHASE reply pid={} uid=1000 status={status}\n",self.birth.host_pid));
        }
        result
    }
    fn check(&mut self, permission: &str, pid: i32, uid: i32, deadline: Instant) -> std::io::Result<bool> {
        use std::os::fd::AsRawFd;
        if self.terminal || !self.birth.is_live() { return Err(permission_io_error("permission owner terminal/expired")); }
        let result=(|| {
            permission_idle(self.output.as_raw_fd())?;
            self.sequence=self.sequence.checked_add(1).ok_or_else(||permission_io_error("permission sequence overflow"))?;
            let mut request=permission_request(2,self.sequence);permission_string(&mut request,permission)?;request.extend(pid.to_be_bytes());request.extend(uid.to_be_bytes());
            permission_send(self.input.as_ref().ok_or_else(||permission_io_error("permission input closed"))?.as_raw_fd(),&request,deadline)?;
            let bytes=permission_receive(self.output.as_raw_fd(),&mut self.transcript,deadline)?;
            let status=permission_result(&bytes,self.sequence,self.birth.host_pid,permission,pid,uid)?;
            permission_idle(self.output.as_raw_fd())?;self.completed.push((self.sequence,status));self.stderr_exact(&self.phase_receipt())?;
            if Instant::now()>=deadline{return Err(std::io::Error::new(std::io::ErrorKind::TimedOut,"permission validation deadline"));}
            Ok(status==0)
        })();if result.is_err(){self.terminal=true;}result
    }
    fn shutdown(&mut self, deadline: Instant) -> std::io::Result<()> {
        use std::os::fd::AsRawFd;
        if self.terminal {return Err(permission_io_error("permission owner terminal"));}
        self.sequence+=1;
        permission_send(self.input.as_ref().ok_or_else(||permission_io_error("permission input closed"))?.as_raw_fd(),&permission_request(4,self.sequence),deadline)?;
        let bytes=permission_receive(self.output.as_raw_fd(),&mut self.transcript,deadline)?;let mut frame=PermissionFrame{bytes:&bytes};frame.header(5,self.sequence)?;
        if frame.int()?!=self.birth.host_pid || frame.int()?!=1000 {return Err(permission_io_error("permission BYE identity differs"));}frame.end()?;drop(self.input.take());
        loop {if let Some(status)=self.guard.lock().unwrap().0.as_mut().ok_or_else(||permission_io_error("permission child already stopped"))?.try_wait()? {if !status.success(){return Err(permission_io_error(format!("permission exit {status}")));}break;}
            if Instant::now()>=deadline{return Err(std::io::Error::new(std::io::ErrorKind::TimedOut,"permission shutdown deadline"));}std::thread::sleep(Duration::from_millis(5));}
        let mut extra=[0];let count=unsafe{libc::read(self.output.as_raw_fd(),extra.as_mut_ptr().cast(),1)};if count!=0{return Err(permission_io_error("permission BYE extra bytes or missing EOF"));}
        self.stderr_exact(&self.phase_receipt())?;self.guard.lock().unwrap().0.as_mut().ok_or_else(||permission_io_error("permission child missing"))?.wait()?;self.guard.lock().unwrap().0=None;self.terminal=true;
        println!("actual original permission shutdown pid={} reaped=true",self.birth.host_pid);Ok(())
    }
}
struct PermissionOwner {
    oracle: std::sync::Mutex<PermissionOracle>,
    child: Arc<std::sync::Mutex<ClientGuard>>,
    terminal: std::sync::atomic::AtomicBool,
    proxy_deadline: std::sync::OnceLock<Instant>,
}
impl PermissionOwner {
    fn new(oracle: PermissionOracle) -> Self { Self { child:oracle.guard.clone(),oracle:std::sync::Mutex::new(oracle),terminal:std::sync::atomic::AtomicBool::new(false),proxy_deadline:std::sync::OnceLock::new() } }
    fn force_stop(&self) -> std::io::Result<()> {
        self.terminal.store(true,std::sync::atomic::Ordering::Release);
        let mut held=self.child.lock().unwrap_or_else(|poison|poison.into_inner());
        if let Some(child)=held.0.as_mut() {
            if child.try_wait()?.is_none() {child.kill()?;}
            child.wait()?;
        }
        held.0=None;Ok(())
    }
    fn finish(&self) -> std::io::Result<()> {
        self.terminal.store(true,std::sync::atomic::Ordering::Release);
        let deadline=Instant::now()+Duration::from_secs(5);
        let result=loop {match self.oracle.try_lock() {
            Ok(mut oracle)=>break oracle.shutdown(deadline),
            Err(std::sync::TryLockError::Poisoned(_))=>break Err(permission_io_error("permission owner poisoned")),
            Err(std::sync::TryLockError::WouldBlock)=>{if Instant::now()>=deadline {break Err(std::io::Error::new(std::io::ErrorKind::TimedOut,"permission cleanup queue deadline"));}std::thread::sleep(Duration::from_millis(2));}
        }};
        if let Err(error)=result {let forced=self.force_stop();return Err(std::io::Error::other(format!("permission normal shutdown failed: {error}; forced cleanup: {forced:?}")));}
        Ok(())
    }
}
// Declared before NativeStop and after BootStop: unwind closes native receivers,
// then this independently held real child, even if policy Arcs remain retained.
struct PermissionStop {owner:Arc<PermissionOwner>,failed:Arc<std::sync::atomic::AtomicBool>,done:std::cell::Cell<bool>}
impl PermissionStop {
    fn finish(&self)->std::io::Result<()> {let result=self.owner.finish();self.done.set(true);if result.is_err(){self.failed.store(true,std::sync::atomic::Ordering::Release);}result}
}
impl Drop for PermissionStop {fn drop(&mut self){if self.done.get(){return;}let result=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||self.finish()));if !matches!(result,Ok(Ok(()))) {self.failed.store(true,std::sync::atomic::Ordering::Release);let forced=self.owner.force_stop();eprintln!("permission cleanup failed: {result:?}; forced actual child: {forced:?}");}}}
fn original_permission(owner: &Arc<PermissionOwner>, permission: &str, pid: i32, uid: i32)
    -> Result<bool, aim_binder_host::parcel::Exception> {
    let failure=|error:std::io::Error|aim_binder_host::parcel::Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,error.to_string());
    let proxy=match owner.proxy_deadline.get() {Some(deadline)=>*deadline,None=>{owner.terminal.store(true,std::sync::atomic::Ordering::Release);return Err(failure(permission_io_error("proxy deadline not installed")));}};
    let deadline=(Instant::now()+Duration::from_secs(15)).min(proxy);
    loop {
        if owner.terminal.load(std::sync::atomic::Ordering::Acquire) {return Err(failure(permission_io_error("permission channel terminal")));}
        if Instant::now()>=deadline {owner.terminal.store(true,std::sync::atomic::Ordering::Release);return Err(failure(std::io::Error::new(std::io::ErrorKind::TimedOut,"permission owner queue/proxy deadline")));}
        match owner.oracle.try_lock() {
            Ok(mut oracle)=>{let result=oracle.check(permission,pid,uid,deadline);match result {
                Err(error)=>{owner.terminal.store(true,std::sync::atomic::Ordering::Release);return Err(failure(error));}
                Ok(value)=>{if owner.terminal.load(std::sync::atomic::Ordering::Acquire){return Err(failure(permission_io_error("permission channel failed/terminal")));}return Ok(value);}
            }}
            Err(std::sync::TryLockError::Poisoned(_))=>{owner.terminal.store(true,std::sync::atomic::Ordering::Release);return Err(failure(permission_io_error("permission owner poisoned")));}
            Err(std::sync::TryLockError::WouldBlock)=>std::thread::sleep(Duration::from_millis(2)),
        }
    }
}

fn compile_credential_launcher(data:&Data,repo:&std::path::Path)->std::path::PathBuf {
    let launcher=data.0.join("credential-drop");
    let clang=aim_paths::ndk_clang(35).expect("NOT RUN: pinned NDK clang missing");
    let built=capture_client(Command::new(clang).args(["-Wall","-Wextra","-Werror","-O2"]).arg(repo.join("crates/aim-services/tests/fixtures/credential_drop_launcher.c")).arg("-o").arg(&launcher),&data.0,"launcher-build",Duration::from_secs(30)).unwrap();
    assert!(built.status.success(),"{}",String::from_utf8_lossy(&built.stderr));
    launcher
}
#[test]
#[ignore="host-only: pinned NDK compiler required"]
fn authored_credential_drop_launcher_compiles(){
    let dir=std::env::temp_dir().join(format!("aim-cred-build-{}",std::process::id()));fs::create_dir(&dir).unwrap();let data=Data(dir);
    let path=compile_credential_launcher(&data,&aim_paths::root());let bytes=fs::read(path).unwrap();assert_eq!(&bytes[..4],b"\x7fELF");assert_eq!(u16::from_le_bytes(bytes[18..20].try_into().unwrap()),183);
}
fn compile_oracle(data: &Data, repo: &std::path::Path) -> std::path::PathBuf {
    let java = aim_paths::fetched().join("java");
    let jdk = java.join("temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let classes = data.0.join("classes");
    let stubs = data.0.join("stubs");
    let dex = data.0.join("dex");
    for path in [&classes, &stubs, &dex] {
        fs::create_dir(path).unwrap();
    }
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(common::java::sources(
            &repo.join("java/device-services/stubs"),
        )));
    let api = data.0.join("api/android");
    let app_api = api.join("app");
    fs::create_dir_all(&app_api).unwrap();
    fs::write(app_api.join("ActivityManager.java"), "package android.app; public class ActivityManager { public static IActivityManager getService() { throw new RuntimeException(\"compile-only\"); } }").unwrap();
    fs::write(app_api.join("IActivityManager.java"), "package android.app; public interface IActivityManager { int checkPermission(String permission,int pid,int uid) throws android.os.RemoteException; }").unwrap();
    fs::create_dir_all(api.join("os")).unwrap();
    fs::create_dir_all(api.join("system")).unwrap();
    fs::create_dir_all(api.join("content/pm")).unwrap();
    fs::write(api.join("content/pm/PackageInstaller.java"), "package android.content.pm; public final class PackageInstaller { public static class SessionParams implements android.os.Parcelable { public long sizeBytes; public String appPackageName; public SessionParams(int mode) { throw new RuntimeException(); } public void writeToParcel(android.os.Parcel out, int flags) { throw new RuntimeException(); } public int describeContents() { throw new RuntimeException(); } } }").unwrap();
    let internal = data.0.join("api/com/android/internal/os");
    fs::create_dir_all(&internal).unwrap();
    fs::write(internal.join("BinderInternal.java"), "package com.android.internal.os; public final class BinderInternal { public static android.os.IBinder getContextObject() { throw new RuntimeException(); } }").unwrap();
    let mut pfd = fs::read_to_string(repo.join("java/device-services/stubs/android/os/ParcelFileDescriptor.java")).unwrap();
    if !pfd.contains("class AutoCloseOutputStream") {
        pfd = pfd.replace("    public static class AutoCloseInputStream", "    public static class AutoCloseOutputStream extends java.io.FileOutputStream { public AutoCloseOutputStream(ParcelFileDescriptor fd) { super(fd.getFileDescriptor()); } }\n    public static class AutoCloseInputStream");
    }
    for (signature, declaration) in [
        ("int getFd()", "    public int getFd() { throw new RuntimeException(); }\n"),
        ("int detachFd()", "    public int detachFd() { throw new RuntimeException(); }\n"),
        ("ParcelFileDescriptor adoptFd(", "    public static ParcelFileDescriptor adoptFd(int fd) { throw new RuntimeException(); }\n"),
    ] {
        if !pfd.contains(signature) { pfd = pfd.replace("    public FileDescriptor getFileDescriptor()", &format!("{declaration}    public FileDescriptor getFileDescriptor()")); }
    }
    fs::write(api.join("os/ParcelFileDescriptor.java"), pfd).unwrap();
    fs::write(
        api.join("system/StructStat.java"),
        "package android.system; public final class StructStat { public long st_size; }",
    )
    .unwrap();
    fs::write(api.join("system/ErrnoException.java"), "package android.system; public final class ErrnoException extends Exception { public final int errno; public ErrnoException(String functionName, int errno) { this.errno = errno; } }").unwrap();
    fs::write(api.join("system/StructMsghdr.java"), "package android.system; public final class StructMsghdr { public StructCmsghdr[] msg_control; public int msg_flags; public StructMsghdr(java.net.SocketAddress name, java.nio.ByteBuffer[] iov, StructCmsghdr[] control, int flags) { throw new RuntimeException(); } }").unwrap();
    fs::write(api.join("system/StructCmsghdr.java"), "package android.system; public final class StructCmsghdr { public int cmsg_level, cmsg_type; public byte[] cmsg_data; public StructCmsghdr(int level, int type, byte[] value) { throw new RuntimeException(); } }").unwrap();
    fs::write(api.join("system/Os.java"), "package android.system; public final class Os { public static void socketpair(int domain,int type,int protocol,java.io.FileDescriptor a,java.io.FileDescriptor b) throws ErrnoException { throw new RuntimeException(); } public static int sendmsg(java.io.FileDescriptor fd,StructMsghdr msg,int flags) throws ErrnoException,java.net.SocketException { throw new RuntimeException(); } public static int recvmsg(java.io.FileDescriptor fd,StructMsghdr msg,int flags) throws ErrnoException,java.net.SocketException { throw new RuntimeException(); } public static int fcntlInt(java.io.FileDescriptor fd,int cmd,int arg) throws ErrnoException { throw new RuntimeException(); } public static void execv(String path,String[] argv) throws ErrnoException { throw new RuntimeException(); } public static java.io.FileDescriptor dup(java.io.FileDescriptor fd) throws ErrnoException { throw new RuntimeException(); } public static void close(java.io.FileDescriptor fd) throws ErrnoException { throw new RuntimeException(); } public static void fsync(java.io.FileDescriptor fd) throws ErrnoException { throw new RuntimeException(); } public static long lseek(java.io.FileDescriptor fd,long offset,int whence) throws ErrnoException { throw new RuntimeException(); } public static StructStat fstat(java.io.FileDescriptor fd) throws ErrnoException { throw new RuntimeException(); } public static int write(java.io.FileDescriptor fd,byte[] b,int off,int len) throws ErrnoException { throw new RuntimeException(); } public static int pread(java.io.FileDescriptor fd,byte[] b,int off,int len,long pos) throws ErrnoException { throw new RuntimeException(); } }").unwrap();
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .arg("-classpath")
        .arg(&stubs)
        .args(common::java::sources(&data.0.join("api"))));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .arg(repo.join("crates/aim-services/tests/fixtures/InstallerProxyOracle.java")));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(java.join("build-tools-36.0.0/android-16/lib/d8.jar"))
        .args([
            "com.android.tools.r8.D8",
            "--release",
            "--min-api",
            "36",
            "--lib",
        ])
        .arg(&jdk)
        .arg("--classpath")
        .arg(&stubs)
        .arg("--output")
        .arg(&dex)
        .args(
            fs::read_dir(&classes)
                .unwrap()
                .map(|entry| entry.unwrap().path()),
        ));
    dex
}

#[test]
#[ignore = "host-only: requires pinned original image, JDK and d8"]
fn permission_oracle_compiles_and_links_against_original_framework() {
    use aim_android_image::{classpath::{self, BOOTCLASSPATH}, linkage::ClassPath};
    let directory = std::env::temp_dir().join(format!("aim-permission-link-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let data = Data(directory);
    let dex = compile_oracle(&data, &aim_paths::root());
    let image = aim_paths::original_image();
    let jars = classpath::jars(&image,"bootclasspath.pb",BOOTCLASSPATH).unwrap();
    let bytes = fs::read(dex.join("classes.dex")).unwrap();
    let missing = ClassPath::read(&image,&jars).unwrap().unresolved(&bytes).unwrap();
    assert!(missing.is_empty(), "original framework linkage: {missing:?}");
}

#[test]
#[ignore = "requires rebuilt wire-v2 runtime, pinned image, aimctl, JDK and d8; explicit true-mode test owner"]
fn original_art_consumes_native_installer_proxy_capability() {
    let directory = std::env::temp_dir().join(format!("ap-{}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let data = FixtureData {inner:std::mem::ManuallyDrop::new(Data(directory)),failed:Arc::new(std::sync::atomic::AtomicBool::new(false))};
    let repo = aim_paths::root();
    let launcher=compile_credential_launcher(&data,&repo);
    let dex = compile_oracle(&data, &repo);
    common::java::check_linkage(&dex.join("classes.dex"), &[]).unwrap();
    let boot = Arc::new(Boot::new(repo.join("target/release/aimctl"), data.0.join("g")));
    use std::os::unix::ffi::OsStrExt;
    let canonical=fs::canonicalize(&data.0).unwrap().join("g.aimctl");
    let address:libc::sockaddr_un=unsafe{std::mem::zeroed()};
    for socket in ["display","display.input/event0","display.input/event1","display.input/event2"] {
        let path=canonical.join(socket);assert!(path.as_os_str().as_bytes().len()<address.sun_path.len()-1,"NOT RUN: fixture socket exceeds SUN_LEN: {}",path.display());
    }
    let boot_stop=BootStop {boot:boot.clone(),failed:data.failed.clone(),done:std::cell::Cell::new(false)};
    let started=capture_client(boot.start_command().args(["start","--windows"]),&data.0,"boot-start",Duration::from_secs(270)).unwrap();
    assert!(started.status.success(),"{} {}",started.status,String::from_utf8_lossy(&started.stderr));
    let deadline=Instant::now()+Duration::from_secs(300);
    let state=fs::read_to_string(format!("{}.aimctl/state",boot.data.display())).unwrap();
    let pid:i32=state.lines().find_map(|line|line.strip_prefix("guest=")).expect("actual init PID missing").parse().unwrap();
    let init=aim_storage::process_namespace::ProcessIdentity::running(pid).unwrap();
    let runtime=aim_storage::data::runtime_of(&boot.data);let name=format!("dev.aim.guest-init.{pid}.binder");
    let mut count=0;
    loop {
        assert!(Instant::now()<deadline,"proxy oracle boot incomplete");let remaining=deadline.saturating_duration_since(Instant::now());
        if binder_ready::poll(&runtime,init,&name,remaining.as_millis().min(20) as u32).unwrap() {
            let output=capture_client(boot.command().args(["shell","getprop","sys.boot_completed"]),&data.0,&format!("ready-{count}"),remaining.min(Duration::from_secs(15))).unwrap();
            assert!(output.status.success(),"{} {}",output.status,String::from_utf8_lossy(&output.stderr));
            if String::from_utf8_lossy(&output.stdout).trim()=="1" {break;}
            count+=1;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    fs::copy(
        dex.join("classes.dex"),
        boot.data.join("data/local/tmp/proxy-oracle.dex"),
    )
    .unwrap();
    fs::copy(&launcher,boot.data.join("data/local/tmp/credential-drop")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(boot.data.join("data/local/tmp/credential-drop"),fs::Permissions::from_mode(0o755)).unwrap();
    let name = format!("dev.aim.proxy-oracle.{}", std::process::id());
    let server = aim_binder_host::server::Server::start(&name).unwrap();
    let process = LocalProcess::open(
        server.driver(),
        Device::Binder,
        Credentials {
            pid: std::process::id() as i32,
            euid: 1000,
            security_context: None,
        },
    );
    let mut state = State::default();
    state.users.insert(
        0,
        User {
            id: 0,
            ..Default::default()
        },
    );
    state.packages.insert(
        "fixture".into(),
        PackageState {
            name: "fixture".into(),
            app_id: 10100,
            pkg: Some(Arc::new(AndroidPackage {
                package_name: "fixture".into(),
                uid: 10100,
                target_sdk_version: 35,
                ..Default::default()
            })),
            users: [(0, PackageUserState::default())].into(),
            ..Default::default()
        },
    );
    let state = Arc::new(state);
    let sessions = Arc::new(installer::Sessions::default());
    let native_data = data.0.join("native");
    for directory in ["system", "app", "app-staging"] {
        fs::create_dir_all(native_data.join(directory)).unwrap();
    }
    let inode = aim_storage::guest_inode::GuestInode {
        uid: Some(1000),
        gid: Some(1000),
        mode: Some(0o600),
    };
    let disk = installer::storage::Store::open(
        native_data.clone(),
        inode,
        aim_storage::guest_inode::GuestInode {
            mode: Some(0o775),
            ..inode
        },
        Arc::new(|path, guest| {
            use std::os::unix::ffi::OsStrExt;
            let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            let label = if guest.starts_with("/data/app/") {
                c"u:object_r:apk_tmp_file:s0"
            } else {
                c"u:object_r:system_data_file:s0"
            };
            if unsafe {
                libc::setxattr(
                    path.as_ptr(),
                    c"dev.aim.xattr.security.selinux".as_ptr(),
                    label.as_ptr().cast(),
                    label.to_bytes_with_nul().len(),
                    0,
                    libc::XATTR_NOFOLLOW,
                )
            } != 0
            {
                return Err(installer::storage::Error {
                    legacy_status: -110,
                    committed: false,
                    message: std::io::Error::last_os_error().to_string(),
                });
            }
            Ok(())
        }),
    )
    .unwrap();
    let publisher = process.clone();
    let permission_owner = Arc::new(PermissionOwner::new(PermissionOracle::start(&boot).expect("original permission startup60/READY")));
    let permission_stop=PermissionStop {owner:permission_owner.clone(),failed:data.failed.clone(),done:std::cell::Cell::new(false)};
    let permission_reader = permission_owner.clone();
    let calling_identities=Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_callers=calling_identities.clone();
    let owners = NativeOwners::open(
        sessions.clone(),
        Arc::new(move || Ok(state.clone())),
        Arc::new(move |caller_uid, caller_pid, _user| {
            captured_callers.lock().unwrap().push((caller_uid,caller_pid));
            println!("actual native Binder caller uid={caller_uid} pid={caller_pid}");
            let retained = permission_reader.clone();
            Ok(DevicePolicy {
                permissions: aim_services::package::installer::policy::CallingPermissions::new(Arc::new(move |permission, pid, uid| {
                    let pid = if pid == 0 {
                        if caller_pid <= 0 || uid != caller_uid as i32 {
                            return Err(aim_binder_host::parcel::Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,"authenticated caller PID unavailable"));
                        }
                        caller_pid
                    } else { pid };
                    original_permission(&retained, permission, pid, uid)
                })),
                debuggable: false,
                apex_supported: false,
                rollback_lifetime: true,
                users: [(
                    0,
                    UserPolicy {
                        disallow_install_apps: false,
                        disallow_debugging_features: false,
                        organization_managed: false,
                    },
                )]
                .into(),
                adopted_shell_uids: Default::default(),
                verifier_uid: None,
            })
        }),
        SystemConfig::default(),
        disk,
        Arc::new(move |node| Ok(publisher.add_service(node))),
        Arc::new(|_| {}),
        process.clone(),
    )
    .unwrap();
    let native_stop=NativeStop {owners:owners.clone(),process:process.clone(),failed:data.failed.clone(),done:std::cell::Cell::new(false)};
    let callback = owners.take_callback_worker().unwrap();
    owners
        .configure_writer(
            Arc::new(|| Ok(true)),
            Arc::new(|_, _, _| panic!("negative length must not allocate")),
        )
        .unwrap();
    let io = owners.take_io_worker_guard().unwrap();
    let installer = process.add_service(Arc::new(installer::endpoint::Endpoint {
        sessions: sessions.clone(),
        owners: owners.clone(),
    }));
    let Binder::Local(ptr) = installer else {
        unreachable!()
    };
    use aim_binder_driver::uapi::*;
    let mut object = FlatBinderObject {
        kind: BINDER_TYPE_BINDER,
        flags: FLAT_BINDER_FLAG_ACCEPTS_FDS,
        binder: ptr,
        cookie: ptr,
    }
    .encode();
    server
        .driver()
        .ioctl(
            process.proc_handle(),
            1,
            BINDER_SET_CONTEXT_MGR_EXT,
            &mut object,
            &mut Inline,
        )
        .unwrap();
    process.start();
    use aim_service_aidl::{
        android_content_pm_ipackageinstaller as api,
        android_content_pm_ipackageinstallersession as session,
    };
    permission_owner.proxy_deadline.set(Instant::now()+Duration::from_secs(60)).expect("proxy deadline set twice");
    let output = original_client(
        isolated_client(&boot, &name)
            .args([
                "/data/local/tmp/credential-drop", "10100", "/data/local/tmp/proxy-proof",
                "/system/bin/app_process",
                "-Djava.class.path=/data/local/tmp/proxy-oracle.dex",
                "/system/bin",
                "InstallerProxyOracle",
            ])
            .args(
                [
                    api::CREATE_SESSION,
                    api::OPEN_SESSION,
                    session::OPEN_WRITE,
                    session::ABANDON,
                    session::OPEN_READ,
                ]
                .map(|code| code.to_string()),
            ),
        &data.0,
    );
    assert!(
        output.status.success(),
        "{}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains(
        "PROXY original AutoCloseOutputStream Binder typed fd dup seek fsync fstat revoke"
    ));
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("PROXY original SCM_RIGHTS retransmit exec classification revoke")
    );
    assert!(
        sessions
            .records()
            .iter()
            .all(|(session, _)| session.destroyed)
    );
    let callers=calling_identities.lock().unwrap();
    assert!(!callers.is_empty(),"native Binder caller identities not captured");
    assert!(callers.iter().all(|(uid,pid)|*uid==10100 && *pid>0),"actual app Binder identities: {callers:?}");
    drop(callers);
    let proof=fs::read_to_string(boot.data.join("data/local/tmp/proxy-proof")).unwrap();
    assert!(proof.contains("uid=10100 euid=10100 suid=10100 gid=10100 egid=10100 sgid=10100 groups=0 caps=0"),"{proof}");
    let pid:i32=proof.lines().find(|line|line.starts_with("post ")).and_then(|line|line.split_whitespace().find_map(|part|part.strip_prefix("pid="))).expect("post-drop PID proof missing").parse().unwrap();
    assert!(calling_identities.lock().unwrap().iter().all(|(_,caller_pid)|*caller_pid==pid),"native Binder PID differs from actual dropped process {pid}");
    println!("actual credential proof {proof}");
    owners.shutdown_callbacks();
    owners.shutdown_io();
    drop(io);
    drop(callback);
    assert!(owners.take_errors().is_empty());
    native_stop.finish().expect("actual native owner shutdown");
    permission_stop.finish().expect("actual permission owner BYE/reap");
    boot_stop.finish().expect("actual original boot stop");
    drop(native_stop);
    drop(owners);
    drop(process);
    drop(boot_stop);
    drop(boot);
    data.finish().expect("actual owned fixture data removal");
}
#[test]
fn file_capture_finishes_while_an_owned_descendant_retains_the_writer() {
    let root=std::env::temp_dir().join(format!("aim-proxy-inherited-{}",std::process::id()));fs::create_dir(&root).unwrap();let data=Data(root);let pid_file=data.0.join("writer.pid");
    let started=Instant::now();
    let output=capture_client(Command::new("/bin/sh").args(["-c","/bin/sleep 20 & echo $! > \"$1\"; printf inherited-writer","fixture"]).arg(&pid_file),&data.0,"inherited",Duration::from_secs(2)).unwrap();
    let pid:i32=fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
    let identity=aim_storage::process_namespace::ProcessIdentity::running(pid).unwrap();
    struct Descendant(aim_storage::process_namespace::ProcessIdentity);
    impl Drop for Descendant { fn drop(&mut self){
        if !self.0.is_live(){return;}unsafe{libc::kill(self.0.host_pid,libc::SIGTERM);}
        let deadline=Instant::now()+Duration::from_secs(1);while self.0.is_live() && Instant::now()<deadline {std::thread::sleep(Duration::from_millis(10));}
        if self.0.is_live(){unsafe{libc::kill(self.0.host_pid,libc::SIGKILL);}}
    } }
    let owned=Descendant(identity);
    assert!(identity.is_live(),"actual descendant still holds inherited output");
    assert!(started.elapsed()<Duration::from_secs(2));assert!(output.status.success());assert_eq!(output.stdout,b"inherited-writer");
    drop(owned);let deadline=Instant::now()+Duration::from_secs(2);while identity.is_live() && Instant::now()<deadline {std::thread::sleep(Duration::from_millis(10));}
    assert!(!identity.is_live(),"owned descendant not cleaned");
}
#[test]
fn file_capture_preserves_error_status_and_bounds_timeout_and_output() {
    let root=std::env::temp_dir().join(format!("aim-proxy-status-{}",std::process::id()));fs::create_dir(&root).unwrap();let data=Data(root);
    let result=capture_client(Command::new("/bin/sh").args(["-c","printf real-error >&2; exit 7"]),&data.0,"status",Duration::from_secs(2)).unwrap();
    assert_eq!(result.status.code(),Some(7));assert_eq!(result.stderr,b"real-error");
    let result=capture_client(Command::new("/usr/bin/head").args(["-c","65537","/dev/zero"]),&data.0,"oversized",Duration::from_secs(2));assert_eq!(result.unwrap_err().kind(),std::io::ErrorKind::InvalidData);
    let pid_file=data.0.join("timeout.pid");let result=capture_client(Command::new("/bin/sh").args(["-c","echo $$ > \"$1\"; exec /bin/sleep 20","fixture"]).arg(&pid_file),&data.0,"timeout",Duration::from_millis(100));assert_eq!(result.unwrap_err().kind(),std::io::ErrorKind::TimedOut);
    let pid:i32=fs::read_to_string(pid_file).unwrap().trim().parse().unwrap();let mut status=0;assert_eq!(unsafe{libc::waitpid(pid,&mut status,libc::WNOHANG)},-1);assert_eq!(std::io::Error::last_os_error().raw_os_error(),Some(libc::ECHILD));
    assert_eq!(capture_client(&mut Command::new(data.0.join("absent")),&data.0,"spawn-error",Duration::from_secs(2)).unwrap_err().kind(),std::io::ErrorKind::NotFound);
}
#[test]
fn fixture_lifecycle_preserves_original_unwind_without_data_double_panic() {
    let dir=std::env::temp_dir().join(format!("aim-proxy-unwind-{}",std::process::id()));fs::create_dir(&dir).unwrap();fs::write(dir.join("failure-evidence"),b"original failure").unwrap();
    let caught=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{let _owned=FixtureData{inner:std::mem::ManuallyDrop::new(Data(dir.clone())),failed:Arc::new(std::sync::atomic::AtomicBool::new(false))};panic!("original fixture error");}));
    assert_eq!(caught.unwrap_err().downcast_ref::<&str>(),Some(&"original fixture error"));assert_eq!(fs::read(dir.join("failure-evidence")).unwrap(),b"original failure");fs::remove_dir_all(dir).unwrap();
}
#[test]
fn fixture_lifecycle_shutdown_releases_actual_native_publisher_cycle() {
    use aim_binder_host::local::{Service,Call,Reply};
    use std::sync::atomic::{AtomicBool,Ordering};
    struct Retained {process:Arc<LocalProcess>,dropped:Arc<AtomicBool>}
    impl Service for Retained {fn descriptor(&self)->&str{"fixture.retained"}fn transact(&self,_:&mut Call<'_>)->Reply{Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION)}}
    impl Drop for Retained {fn drop(&mut self){assert!(Arc::strong_count(&self.process)>0);self.dropped.store(true,Ordering::Release);}}
    let name=format!("dev.aim.proxy-lifetime.{}",std::process::id());let server=aim_binder_host::server::Server::start(&name).unwrap();
    let process=LocalProcess::open(server.driver(),Device::Binder,Credentials{pid:std::process::id()as i32,euid:1000,security_context:None});
    let dropped=Arc::new(AtomicBool::new(false));process.add_service(Arc::new(Retained{process:process.clone(),dropped:dropped.clone()}));process.start();
    assert!(!dropped.load(Ordering::Acquire));process.shutdown().unwrap();assert!(dropped.load(Ordering::Acquire));assert_eq!(Arc::strong_count(&process),1);
}

#[test]
fn permission_protocol_rejects_version_sequence_error_utf8_and_tail() {
    let good=permission_request(3,7);
    let mut frame=PermissionFrame{bytes:&good};frame.header(3,7).unwrap();frame.end().unwrap();
    for bytes in [&good[..9], &[2,3,0,0,0,0,0,0,0,7], &[1,4,0,0,0,0,0,0,0,7], &[1,3,0,0,0,0,0,0,0,8]] {
        assert!(PermissionFrame{bytes}.header(3,7).is_err());
    }
    let mut error=permission_request(6,7);permission_string(&mut error,"actual owner exception").unwrap();
    assert!(PermissionFrame{bytes:&error}.header(3,7).unwrap_err().to_string().contains("actual owner exception"));
    let mut bytes=vec![0,0,0,1,0xff];assert!(PermissionFrame{bytes:&bytes}.string().is_err());
    bytes=vec![0xff,0xff,0xff,0xff];assert!(PermissionFrame{bytes:&bytes}.string().is_err());
    assert!(PermissionFrame{bytes:&[1]}.end().is_err());
}
#[test]
fn permission_channel_bounds_partial_io_timeout_and_extra_eof() {
    use std::os::{fd::AsRawFd,unix::net::UnixStream};
    use std::io::Write;
    let directory=std::env::temp_dir().join(format!("aim-permission-channel-{}",std::process::id()));fs::create_dir(&directory).unwrap();let data=Data(directory);
    let (reader,mut writer)=UnixStream::pair().unwrap();permission_nonblocking(reader.as_raw_fd()).unwrap();
    let mut log=fs::File::create(data.0.join("frames")).unwrap();
    let payload=permission_request(3,9);let mut wire=(payload.len() as u32).to_be_bytes().to_vec();wire.extend(&payload);
    let thread=std::thread::spawn(move||{for byte in wire {writer.write_all(&[byte]).unwrap();std::thread::sleep(Duration::from_millis(1));}writer});
    let got=permission_receive(reader.as_raw_fd(),&mut log,Instant::now()+Duration::from_secs(1)).unwrap();assert_eq!(got,payload);
    let mut writer=thread.join().unwrap();permission_idle(reader.as_raw_fd()).unwrap();
    let error=permission_receive(reader.as_raw_fd(),&mut log,Instant::now()+Duration::from_millis(30)).unwrap_err();assert_eq!(error.kind(),std::io::ErrorKind::TimedOut);
    writer.write_all(&[7]).unwrap();assert!(permission_idle(reader.as_raw_fd()).unwrap_err().to_string().contains("extra"));
    drop(writer);assert!(permission_idle(reader.as_raw_fd()).unwrap_err().to_string().contains("closed"));
    let (reader,mut writer)=UnixStream::pair().unwrap();permission_nonblocking(reader.as_raw_fd()).unwrap();writer.write_all(&[0,1,0,1]).unwrap();
    assert!(permission_receive(reader.as_raw_fd(),&mut log,Instant::now()+Duration::from_secs(1)).unwrap_err().to_string().contains("frame length"));
    let (reader,mut writer)=UnixStream::pair().unwrap();permission_nonblocking(reader.as_raw_fd()).unwrap();writer.write_all(&[0,0,0,10,1]).unwrap();drop(writer);
    assert!(permission_receive(reader.as_raw_fd(),&mut log,Instant::now()+Duration::from_secs(1)).unwrap_err().to_string().contains("EOF"));
}
#[test]
fn permission_owned_child_timeout_reaps_real_birth() {
    use std::os::fd::AsRawFd;
    let directory=std::env::temp_dir().join(format!("aim-permission-owned-child-{}",std::process::id()));fs::create_dir(&directory).unwrap();let data=Data(directory);
    let mut child=Command::new("/bin/sh").args(["-c","read line"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let birth=aim_storage::process_namespace::ProcessIdentity::running(child.id() as i32).unwrap();let output=child.stdout.take().unwrap();permission_nonblocking(output.as_raw_fd()).unwrap();let guard=ClientGuard(Some(child));
    let mut file=fs::File::create(data.0.join("output")).unwrap();assert_eq!(permission_receive(output.as_raw_fd(),&mut file,Instant::now()+Duration::from_millis(30)).unwrap_err().kind(),std::io::ErrorKind::TimedOut);
    drop(guard);assert!(!birth.is_live(),"actual owned reader birth was not reaped");
}
#[test]
fn permission_reply_preserves_actual_subject_identity_and_status() {
    let response=|reader:i32,permission:&str,pid:i32,uid:i32,status:i32| {
        let mut bytes=permission_request(3,1);bytes.extend(reader.to_be_bytes());bytes.extend(1000i32.to_be_bytes());permission_string(&mut bytes,permission).unwrap();bytes.extend(pid.to_be_bytes());bytes.extend(uid.to_be_bytes());bytes.extend(status.to_be_bytes());bytes
    };
    for status in [0,-1] {assert_eq!(permission_result(&response(41,"permission",42,10100,status),1,41,"permission",42,10100).unwrap(),status);}
    for bytes in [response(40,"permission",42,10100,0),response(41,"other",42,10100,0),response(41,"permission",43,10100,0),response(41,"permission",42,1000,0),response(41,"permission",42,10100,1)] {assert!(permission_result(&bytes,1,41,"permission",42,10100).is_err());}
    let mut extra=response(41,"permission",42,10100,0);extra.push(0);assert!(permission_result(&extra,1,41,"permission",42,10100).is_err());
}
#[test]
fn permission_partial_write_obeys_the_same_absolute_deadline() {
    use std::os::{fd::AsRawFd,unix::net::UnixStream};
    let (writer,_reader)=UnixStream::pair().unwrap();permission_nonblocking(writer.as_raw_fd()).unwrap();
    let size=1024i32;assert_eq!(unsafe{libc::setsockopt(writer.as_raw_fd(),libc::SOL_SOCKET,libc::SO_SNDBUF,(&size as *const i32).cast(),4)},0);
    let bytes=vec![1;PERMISSION_FRAME_MAX];let deadline=Instant::now()+Duration::from_millis(30);
    let error=permission_send(writer.as_raw_fd(),&bytes,deadline).unwrap_err();assert_eq!(error.kind(),std::io::ErrorKind::TimedOut);
    assert!(Instant::now()<deadline+Duration::from_millis(250));
}
fn permission_test_owned_reader(root: &std::path::Path) -> Arc<PermissionOwner> {
    let mut child=Command::new("/bin/sh").args(["-c","read line"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let birth=aim_storage::process_namespace::ProcessIdentity::running(child.id() as i32).unwrap();
    let input=child.stdin.take();let output=child.stdout.take().unwrap();
    Arc::new(PermissionOwner::new(PermissionOracle{guard:Arc::new(std::sync::Mutex::new(ClientGuard(Some(child)))),birth,input,output,transcript:fs::File::create(root.join("host-reader.stdout")).unwrap(),stderr:root.join("host-reader.stderr"),sequence:0,completed:Vec::new(),terminal:false}))
}
#[test]
fn permission_queue_uses_proxy_deadline_and_quarantines_followup() {
    let directory=std::env::temp_dir().join(format!("aim-permission-queue-{}",std::process::id()));fs::create_dir(&directory).unwrap();let data=Data(directory);
    let owner=permission_test_owned_reader(&data.0);owner.proxy_deadline.set(Instant::now()+Duration::from_millis(30)).unwrap();
    let lock=owner.oracle.lock().unwrap();let queued=owner.clone();let started=Instant::now();
    let thread=std::thread::spawn(move||original_permission(&queued,"unused",1,1));
    assert!(thread.join().unwrap().is_err());assert!(started.elapsed()<Duration::from_millis(500));
    assert!(owner.terminal.load(std::sync::atomic::Ordering::Acquire));drop(lock);
    assert!(original_permission(&owner,"unused",1,1).is_err());owner.force_stop().unwrap();
}
#[test]
fn permission_failure_guard_reaps_even_with_retained_policy_and_busy_oracle() {
    let directory=std::env::temp_dir().join(format!("aim-permission-cleanup-{}",std::process::id()));fs::create_dir(&directory).unwrap();let data=Data(directory);
    let owner=permission_test_owned_reader(&data.0);let retained_policy=owner.clone();let lock=owner.oracle.lock().unwrap();let birth=lock.birth;
    // Force cleanup uses the separately owned Child handle, never metadata PID
    // or the busy per-request mutex retained by a policy callback.
    let failed=Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop=PermissionStop{owner:owner.clone(),failed:failed.clone(),done:std::cell::Cell::new(false)};
    drop(stop);
    assert!(failed.load(std::sync::atomic::Ordering::Acquire),"failed normal cleanup must remain a fixture failure");
    assert!(!birth.is_live());assert!(owner.child.lock().unwrap().0.is_none());
    assert!(retained_policy.terminal.load(std::sync::atomic::Ordering::Acquire));drop(lock);
}
