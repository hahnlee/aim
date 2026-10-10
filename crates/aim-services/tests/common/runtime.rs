//! Disposable original-runtime fixture support.
#[path = "cohort.rs"]
pub mod cohort;
#[path = "../../../aim-build/src/nodes/binder_ready.rs"]
mod binder_ready;

struct QueryChild(std::process::Child);
impl Drop for QueryChild {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) { let _ = self.0.kill(); }
        let _ = self.0.wait();
    }
}

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

/// Captured EOF stdin/stdout/stderr for the archive oracle only; not a streaming caller.
#[track_caller]
pub fn run_with_status_receipt(command: &mut Command) -> Output {
    let output = match scoped_output(command, std::time::Duration::from_secs(60)) {
        Ok(output) => output,
        Err(failure) => {
            use std::os::unix::process::ExitStatusExt;
            let receipt = std::env::temp_dir().join(format!("aim-archive-capture-{}-{}", std::process::id(), failure.pid));
            let record = format!("reason={} pid={} raw={:?} code={:?} signal={:?} stdout_eof={} stderr_eof={} stdout_bytes={} stderr_bytes={}\n",
                failure.reason, failure.pid, failure.status.map(|s| s.into_raw()), failure.status.and_then(|s| s.code()),
                failure.status.and_then(|s| s.signal()), failure.stdout_eof, failure.stderr_eof, failure.stdout.len(), failure.stderr.len());
            let saved = (|| -> std::io::Result<()> {
                std::fs::create_dir(&receipt)?;
                std::fs::write(receipt.join("stdout.partial"), &failure.stdout)?;
                std::fs::write(receipt.join("stderr.partial"), &failure.stderr)?;
                std::fs::write(receipt.join("receipt.txt"), &record)
            })();
            eprintln!("fixture-child failure {record}partial_receipt={receipt:?} persistence={saved:?}");
            panic!("archive child capture failed: {}", failure.reason);
        }
    };
    assert!(output.status.success(), "{:?}: {}: {}", command.get_program(), output.status,
        String::from_utf8_lossy(&output.stderr));
    output
}
#[derive(Debug)]
struct CaptureFailure {
    reason: String,
    pid: u32,
    status: Option<std::process::ExitStatus>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_eof: bool,
    stderr_eof: bool,
}
fn scoped_output(command: &mut Command, eof_limit: std::time::Duration) -> Result<Output, CaptureFailure> {
    use std::{io::Read, os::{fd::AsRawFd, unix::process::ExitStatusExt}, process::Stdio, time::{Duration, Instant}};
    const OUTPUT_LIMIT: usize = 1024 * 1024;
    let mut child = QueryChild(command.stdin(Stdio::null()).stdout(Stdio::piped())
        .stderr(Stdio::piped()).spawn().unwrap());
    let pid = child.0.id();
    eprintln!("fixture-child phase=spawn pid={pid} program={:?} args={:?}",
        command.get_program(), command.get_args().collect::<Vec<_>>());
    let mut stdout = child.0.stdout.take().unwrap();
    let mut stderr = child.0.stderr.take().unwrap();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let (mut out_eof, mut err_eof) = (false, false);
    let mut status = None;
    let mut failure = None;
    let mut eof_deadline = None;
    for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            failure = Some(format!("pipe configuration: {}", std::io::Error::last_os_error()));
            break;
        }
    }
    let drain = |pipe: &mut dyn Read, bytes: &mut Vec<u8>, other: usize| -> Result<bool, String> {
        let mut buffer = [0u8; 8192];
        for _ in 0..16 {
            let room = OUTPUT_LIMIT - bytes.len() - other;
            let capacity = buffer.len().min(room + 1);
            match pipe.read(&mut buffer[..capacity]) {
                Ok(0) => return Ok(true),
                Ok(n) => {
                    bytes.extend_from_slice(&buffer[..n.min(room)]);
                    if n > room { return Err("combined output exceeds 1MiB diagnostic budget".into()); }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(format!("output read: {error}")),
            }
        }
        Ok(false)
    };
    loop {
        if failure.is_none() && !out_eof {
            match drain(&mut stdout, &mut out, err.len()) { Ok(eof) => out_eof = eof, Err(error) => failure = Some(error) }
        }
        if failure.is_none() && !err_eof {
            match drain(&mut stderr, &mut err, out.len()) { Ok(eof) => err_eof = eof, Err(error) => failure = Some(error) }
        }
        if status.is_none() {
            match child.0.try_wait() {
                Ok(Some(done)) => {
                    eprintln!("fixture-child phase=wait pid={pid} raw={} code={:?} signal={:?}", done.into_raw(), done.code(), done.signal());
                    status = Some(done);
                    eof_deadline = Some(Instant::now() + eof_limit);
                }
                Ok(None) => {}
                Err(error) => failure = Some(format!("wait: {error}")),
            }
        }
        if failure.is_none() && status.is_some() && out_eof && err_eof { break; }
        if eof_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            failure = Some(format!("output EOF exceeded {eof_limit:?}"));
        }
        if let Some(mut reason) = failure {
            if status.is_none() {
                if let Err(error) = child.0.kill() { reason.push_str(&format!("; held child kill: {error}")); }
                match child.0.wait() {
                    Ok(done) => status = Some(done),
                    Err(error) => reason.push_str(&format!("; held child reap: {error}")),
                }
            }
            eprintln!("fixture-child capture-error pid={pid} reason={reason} raw={:?} stdout_bytes={} stderr_bytes={} stdout_eof={out_eof} stderr_eof={err_eof}",
                status.map(|s| s.into_raw()), out.len(), err.len());
            return Err(CaptureFailure { reason, pid, status, stdout: out, stderr: err, stdout_eof: out_eof, stderr_eof: err_eof });
        }
        // Active-child lifetime remains under the enclosing held body supervisor's 900s.
        std::thread::sleep(Duration::from_millis(10));
    }
    eprintln!("fixture-child phase=output-eof pid={pid} stdout_bytes={} stderr_bytes={}", out.len(), err.len());
    Ok(Output { status: status.unwrap(), stdout: out, stderr: err })
}

#[track_caller]
pub fn run(command: &mut Command) -> Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{:?}: {}: {}",
        command.get_program(),
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

pub struct Data(pub PathBuf);
impl Drop for Data {
    fn drop(&mut self) {
        let marker=self.0.join(".aim-cleanup-failed");
        if marker.exists() {
            cleanup_error(&self.0,"recorded cleanup failure");return;
        }
        let result=mounted_under(&self.0).and_then(|mounted|{
            if mounted { return Err("fixture data still contains an actual mount".into()); }
            fs::remove_dir_all(&self.0).map_err(|error|error.to_string())
        });
        if let Err(error)=result {
            if let Err(record)=fs::write(&marker,&error) {
                eprintln!("cleanup failure receipt write failed: {record}");
            }
            cleanup_error(&self.0,&error);
        }
    }
}
fn cleanup_error(data:&std::path::Path,error:&str) {
    eprintln!("fixture cleanup failed; data preserved at {}: {error}",data.display());
    if !std::thread::panicking() { panic!("fixture cleanup failed: {error}"); }
}
fn mounted_under(data:&std::path::Path)->Result<bool,String> {
    use std::os::unix::ffi::OsStrExt;
    let root=fs::canonicalize(data).map_err(|error|error.to_string())?;
    let count=unsafe{libc::getfsstat(std::ptr::null_mut(),0,libc::MNT_NOWAIT)};
    if count<0{return Err(std::io::Error::last_os_error().to_string());}
    let mut entries=vec![unsafe{std::mem::zeroed::<libc::statfs>()};count as usize+64];
    let size=i32::try_from(entries.len()*std::mem::size_of::<libc::statfs>()).map_err(|error|error.to_string())?;
    let read=unsafe{libc::getfsstat(entries.as_mut_ptr(),size,libc::MNT_NOWAIT)};
    if read<0{return Err(std::io::Error::last_os_error().to_string());}
    if read as usize>=entries.len(){return Err("mount snapshot overflow".into());}
    Ok(entries[..read as usize].iter().any(|entry|{
        let name=unsafe{std::ffi::CStr::from_ptr(entry.f_mntonname.as_ptr())};
        std::path::Path::new(std::ffi::OsStr::from_bytes(name.to_bytes())).starts_with(&root)
    }))
}
struct CleanupChild(std::process::Child);
impl Drop for CleanupChild {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(),Ok(Some(_))){return;}
        let _=self.0.kill();let _=self.0.wait();
    }
}
fn bounded_cleanup(command:&mut Command, directory:&std::path::Path, limit:std::time::Duration)->Result<(),String> {
    fs::create_dir(directory).map_err(|error|format!("cleanup output: {error}"))?;
    let stdout=directory.join("stdout");let stderr=directory.join("stderr");
    command.stdout(fs::File::create(&stdout).map_err(|error|error.to_string())?)
        .stderr(fs::File::create(&stderr).map_err(|error|error.to_string())?);
    let mut child=CleanupChild(command.spawn().map_err(|error|error.to_string())?);
    let deadline=std::time::Instant::now()+limit;
    let status=loop {
        if let Some(status)=child.0.try_wait().map_err(|error|error.to_string())?{break Some(status);}
        if std::time::Instant::now()>=deadline {
            child.0.kill().map_err(|error|error.to_string())?;
            let terminated=child.0.wait().map_err(|error|error.to_string())?;
            fs::write(directory.join("terminated-status"),format!("{terminated:?}\n")).map_err(|error|error.to_string())?;
            break None;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    fs::write(directory.join("status"),format!("{status:?}\n")).map_err(|error|error.to_string())?;
    if status.is_some_and(|status|status.success()){Ok(())}
    else {Err(format!("cleanup status {status:?}; stderr: {}; raw output: {}",
        String::from_utf8_lossy(&fs::read(stderr).map_err(|error|error.to_string())?),directory.display()))}
}

struct CredentialLauncher { path: PathBuf, sha256: String }

fn append_credentials(command: &mut Command, uid: u32, proof: &str) {
    command.args(["--identity-text", "uid\t0\ngid\t0\n"]);
    if uid != 0 {
        command.arg("/data/local/tmp/aim-fixture-credentials/credential-drop")
            .arg(uid.to_string()).arg(proof);
    }
}
fn validate_launcher(launcher: &CredentialLauncher) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::symlink_metadata(&launcher.path).map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o7777 != 0o555 {
        return Err("staged credential launcher type/mode changed".into());
    }
    let bytes = fs::read(&launcher.path).map_err(|e| e.to_string())?;
    if format!("{:x}", Sha256::digest(&bytes)) != launcher.sha256
        || aim_storage::guest_inode::read(&launcher.path).map_err(|e| e.to_string())?
            != Some(aim_storage::guest_inode::GuestInode { uid: Some(0), gid: Some(0), mode: Some(0o555) }) {
        return Err("staged credential launcher binding changed".into());
    }
    Ok(())
}

pub struct Boot {
    pub ctl: PathBuf,
    pub data: PathBuf,
    inputs: std::sync::OnceLock<cohort::Cohort>,
    launcher: std::sync::OnceLock<Result<CredentialLauncher, String>>,
}
impl Boot {
    pub fn new(ctl: PathBuf, data: PathBuf) -> Self {
        Self { ctl, data, inputs: std::sync::OnceLock::new(), launcher: std::sync::OnceLock::new() }
    }
    fn inputs(&self) -> &cohort::Cohort {
        self.inputs.get_or_init(|| cohort::load().expect("NOT RUN: explicit pinned M4 cohort required"))
    }
    /// Start a client with the Linux credentials its original daemon requires.
    pub fn client(&self, uid: u32) -> Command {
        self.checked_client(uid).expect("NOT RUN: authenticated original client unavailable")
    }
    pub fn client_with_binder(&self, uid: u32, binder: &str) -> Command {
        self.checked_client_with_binder(uid, Some(binder))
            .expect("NOT RUN: authenticated original client unavailable")
    }
    pub fn checked_client(&self, uid: u32) -> Result<Command,String> {
        self.checked_client_with_binder(uid, None)
    }
    fn checked_client_with_binder(&self, uid: u32, binder: Option<&str>) -> Result<Command,String> {
        let state = fs::read_to_string(PathBuf::from(format!("{}.aimctl/state",self.data.display())))
            .map_err(|e|format!("original boot state: {e}"))?;
        let guest: u32 = state.lines().find_map(|line|line.strip_prefix("guest="))
            .ok_or("original guest PID unavailable")?.parse().map_err(|e|format!("original guest PID: {e}"))?;
        let runtime = PathBuf::from(format!("{}.run", self.data.display()));
        let environ = fs::read_to_string(runtime.join("environ")).map_err(|e|format!("original environment: {e}"))?;
        let inputs = self.inputs.get().ok_or("original boot has no retained input owner")?;
        inputs.revalidate(false)?;
        let mut command = Command::new(&inputs.runtime["linux-run"].path);
        command.env_clear().envs(environ.lines().filter_map(|line|line.split_once('=')))
            .arg("--inherit-env").arg("--root").arg(inputs.image(cohort::Variant::Original))
            .arg("--mount-namespace-from-init").arg("--path-map").arg(runtime.join("path-map"))
            .arg("--by-pid").arg(runtime.join("identity/by-pid"))
            .args(["--binder", binder.unwrap_or(&format!("dev.aim.guest-init.{guest}.binder"))]);
        if uid != 0 { self.prepare_credential_launcher(inputs)?; }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let ticket = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        append_credentials(&mut command, uid,
            &format!("/data/local/tmp/aim-fixture-credentials/proof-{}-{ticket}-{uid}", std::process::id()));
        inputs.revalidate(false)?;
        Ok(command)
    }
    /// Wait for the launched init's authenticated Binder owner and actual Android boot.
    pub fn wait_ready(&self, limit: std::time::Duration) -> Result<(), String> {
        use std::{io::Read, time::{Duration, Instant}};
        let state = fs::read_to_string(format!("{}.aimctl/state", self.data.display()))
            .map_err(|error| format!("boot state: {error}"))?;
        let pid: i32 = state.lines().find_map(|line| line.strip_prefix("guest="))
            .ok_or("boot init PID missing")?.parse().map_err(|error| format!("boot init PID: {error}"))?;
        let init = aim_storage::process_namespace::ProcessIdentity::running(pid)
            .map_err(|error| format!("boot init birth: {error}"))?;
        let runtime = aim_storage::data::runtime_of(&self.data);
        let name = format!("dev.aim.guest-init.{pid}.binder");
        let deadline = Instant::now() + limit;
        let mut query = 0;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { return Err(format!("Android boot incomplete after {limit:?}")); }
            if binder_ready::poll(&runtime, init, &name, remaining.as_millis().min(20) as u32)? {
                let output = format!("{}.ready-{query}.stdout", self.data.display());
                let errors = format!("{}.ready-{query}.stderr", self.data.display());
                query += 1;
                let stdout = fs::File::create(&output).map_err(|error| error.to_string())?;
                let stderr = fs::File::create(&errors).map_err(|error| error.to_string())?;
                let mut child = QueryChild(self.command().args(["shell", "getprop", "sys.boot_completed"])
                    .stdout(stdout).stderr(stderr).spawn().map_err(|error| error.to_string())?);
                let query_deadline = Instant::now() + remaining.min(Duration::from_secs(15));
                let status = loop {
                    if let Some(status) = child.0.try_wait().map_err(|error| error.to_string())? { break status; }
                    if Instant::now() >= query_deadline { return Err("boot property query timed out".into()); }
                    std::thread::sleep(Duration::from_millis(10));
                };
                let read = |path: &str| -> Result<String, String> {
                    let mut bytes = Vec::new();
                    fs::File::open(path).map_err(|error| error.to_string())?.take(65537)
                        .read_to_end(&mut bytes).map_err(|error| error.to_string())?;
                    if bytes.len() > 65536 { return Err("boot property output exceeds 64KiB".into()); }
                    String::from_utf8(bytes).map_err(|error| error.to_string())
                };
                if !status.success() { return Err(format!("boot property query {status}: {}", read(&errors)?)); }
                if read(&output)?.trim() == "1" {
                    if !init.is_live() { return Err("boot init exited during property query".into()); }
                    return Ok(());
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    fn prepare_credential_launcher(&self, inputs: &cohort::Cohort) -> Result<(), String> {
        let launcher = self.launcher.get_or_init(|| {
            use sha2::{Digest, Sha256};
            use std::io::Write;
            use std::os::unix::fs::PermissionsExt;
            let source = aim_paths::root().join("crates/aim-services/tests/fixtures/credential_drop_launcher.c");
            let source_bytes = fs::read(&source).map_err(|e| e.to_string())?;
            let source_sha = format!("{:x}", Sha256::digest(&source_bytes));
            let compiler = &inputs.tools["ndk_clang35"];
            inputs.revalidate(false)?;
            // A sibling of the boot directory, inside the owning fixture Data.
            let build = self.data.parent().ok_or("credential fixture parent missing")?
                .join(format!("{}.credential-helper", self.data.file_name().ok_or("credential fixture name missing")?.to_string_lossy()));
            fs::create_dir(&build).map_err(|e| format!("private credential build: {e}"))?;
            let output = build.join("credential-drop");
            let stdout = build.join("compiler.stdout");
            let stderr = build.join("compiler.stderr");
            let mut command = Command::new(&compiler.path);
            command.args(["-Wall", "-Wextra", "-Werror", "-O2"]).arg(&source).arg("-o").arg(&output)
                .stdout(fs::File::create(&stdout).map_err(|e| e.to_string())?)
                .stderr(fs::File::create(&stderr).map_err(|e| e.to_string())?);
            struct Compiler(std::process::Child);
            impl Drop for Compiler {
                fn drop(&mut self) {
                    if matches!(self.0.try_wait(), Ok(Some(_))) { return; }
                    let _ = self.0.kill(); let _ = self.0.wait();
                }
            }
            let mut child = Compiler(command.spawn().map_err(|e| e.to_string())?);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            let status = loop {
                if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? { break Some(status); }
                if std::time::Instant::now() >= deadline {
                    child.0.kill().map_err(|e| e.to_string())?;
                    child.0.wait().map_err(|e| e.to_string())?;
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            };
            let stderr_bytes = fs::read(&stderr).map_err(|e| e.to_string())?;
            if !status.is_some_and(|status| status.success()) {
                return Err(format!("credential compiler {status:?}: {} (output preserved at {})",
                    String::from_utf8_lossy(&stderr_bytes), build.display()));
            }
            inputs.revalidate(false)?;
            if fs::read(&source).map_err(|e| e.to_string())? != source_bytes {
                return Err("credential launcher source changed during build".into());
            }
            let bytes = fs::read(&output).map_err(|e| e.to_string())?;
            if bytes.len() < 20 || &bytes[..4] != b"\x7fELF"
                || bytes[4] != 2 || bytes[5] != 1 || u16::from_le_bytes([bytes[18], bytes[19]]) != 183 {
                return Err("credential launcher is not a real AArch64 ELF".into());
            }
            let helper_sha = format!("{:x}", Sha256::digest(&bytes));
            let directory = self.data.join("data/local/tmp/aim-fixture-credentials");
            fs::create_dir(&directory).map_err(|e| format!("exclusive credential stage: {e}"))?;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
            aim_storage::guest_inode::record(&directory, aim_storage::guest_inode::GuestInode {
                uid: Some(0), gid: Some(0), mode: Some(0o755),
            }).map_err(|e| e.to_string())?;
            let staged = directory.join("credential-drop");
            let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&staged).map_err(|e| e.to_string())?;
            file.write_all(&bytes).and_then(|()| file.sync_all()).map_err(|e| e.to_string())?;
            file.set_permissions(fs::Permissions::from_mode(0o555)).map_err(|e| e.to_string())?;
            aim_storage::guest_inode::record(&staged, aim_storage::guest_inode::GuestInode {
                uid: Some(0), gid: Some(0), mode: Some(0o555),
            }).map_err(|e| e.to_string())?;
            fs::write(build.join("provenance.tsv"), format!("compiler\t{}\t{}\nsource\t{}\nhelper\t{}\n",
                compiler.path.display(), compiler.sha256, source_sha, helper_sha)).map_err(|e| e.to_string())?;
            Ok(CredentialLauncher { path: staged, sha256: helper_sha })
        }).as_ref().map_err(Clone::clone)?;
        validate_launcher(launcher)
    }
    pub fn start_command(&self) -> Command {
        let inputs = self.inputs();
        inputs.revalidate(true).expect("NOT RUN: launch input drift");
        let mut cmd = self.command();
        cmd.arg("--image").arg(inputs.image(cohort::Variant::Original))
            .arg("--host-runtime").arg(inputs.runtime_root())
            .arg("--userdata").arg(&inputs.userdata);
        cmd
    }
    pub fn command(&self) -> Command {
        let inputs = self.inputs();
        inputs.revalidate(false).expect("NOT RUN: pinned cohort binding changed");
        self.retained_command(inputs)
    }
    fn retained_command(&self, inputs: &cohort::Cohort) -> Command {
        let mut cmd = Command::new(&inputs.tools["aimctl"].path);
        cmd.arg("--data").arg(&self.data);
        cmd
    }

}
impl Boot {
    fn cleanup(&self)->Result<(),String> {
        let Some(inputs)=self.inputs.get() else{return Ok(());};
        inputs.verify_controller()?;
        static NEXT:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
        let ticket=NEXT.fetch_add(1,std::sync::atomic::Ordering::Relaxed);
        let parent=self.data.parent().ok_or("cleanup fixture parent missing")?;
        let logs=parent.join(format!(".aim-cleanup-stop-{}-{ticket}",std::process::id()));
        // Only this directly spawned controller child is signalled on timeout.
        // The controller retains responsibility for its authenticated boot owners.
        let state=fs::read_to_string(PathBuf::from(format!("{}.aimctl/state",self.data.display())));
        bounded_cleanup(self.retained_command(inputs).arg("stop"),&logs,std::time::Duration::from_secs(60))?;
        let state=match state {
            Ok(state)=>state,Err(error) if error.kind()==std::io::ErrorKind::NotFound=>String::new(),
            Err(error)=>return Err(error.to_string()),
        };
        for field in state.lines().filter_map(|line|line.strip_prefix("pid=").or_else(||line.strip_prefix("guest="))) {
            if field.is_empty(){continue;}
            let pid=field.parse::<i32>().map_err(|error|format!("cleanup recorded PID: {error}"))?;
            if pid<=0{return Err("invalid recorded cleanup PID".into());}
            // Liveness check only; never signal a PID learned from a text record.
            if unsafe{libc::kill(pid,0)}==0{return Err(format!("recorded process still exists: {pid}"));}
            if std::io::Error::last_os_error().raw_os_error()!=Some(libc::ESRCH) {
                return Err(format!("recorded process status unknown: {pid}"));
            }
        }
        if mounted_under(&self.data)?{return Err("boot data is still mounted".into());}
        Ok(())
    }
}
impl Drop for Boot {
    fn drop(&mut self) {
        if let Err(error)=self.cleanup() {
            if let Some(parent)=self.data.parent(){
                if let Err(record)=fs::write(parent.join(".aim-cleanup-failed"),&error){
                    eprintln!("cleanup failure receipt write failed: {record}");
                }
            }
            cleanup_error(&self.data,&error);
        }
    }
}
