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
        fs::remove_dir_all(&self.0).unwrap();
    }
}

pub struct Boot {
    pub ctl: PathBuf,
    pub data: PathBuf,
    inputs: std::sync::OnceLock<cohort::Cohort>,
}
impl Boot {
    pub fn new(ctl: PathBuf, data: PathBuf) -> Self {
        Self { ctl, data, inputs: std::sync::OnceLock::new() }
    }
    fn inputs(&self) -> &cohort::Cohort {
        self.inputs.get_or_init(|| cohort::load().expect("NOT RUN: explicit pinned M4 cohort required"))
    }
    /// Start a client with the Linux credentials its original daemon requires.
    pub fn client(&self, uid: u32) -> Command {
        self.checked_client(uid).expect("NOT RUN: authenticated original client unavailable")
    }
    pub fn checked_client(&self, uid: u32) -> Result<Command,String> {
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
            .args(["--binder",&format!("dev.aim.guest-init.{guest}.binder")])
            .args(["--identity-text",&format!("uid\t{uid}\ngid\t{uid}\n")]);
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
impl Drop for Boot {
    fn drop(&mut self) {
        if self.inputs.get().is_none() { return; }
        let state = fs::read_to_string(PathBuf::from(format!(
            "{}.aimctl/state",
            self.data.display()
        )))
        .unwrap_or_default();
        let pids: Vec<i32> = state
            .lines()
            .filter_map(|line| {
                line.strip_prefix("pid=")
                    .or_else(|| line.strip_prefix("guest="))
                    .map(|pid| pid.parse().unwrap())
            })
            .collect();
        let inputs = self.inputs.get().unwrap();
        inputs.verify_controller().expect("pinned cleanup controller changed");
        // Cleanup keeps the launch selection even if its manifest was retired.
        run(self.retained_command(inputs).arg("stop"));
        for pid in pids {
            // Signal zero checks only the test's own keeper and guest.
            assert_eq!(
                unsafe { libc::kill(pid, 0) },
                -1,
                "owned process remains: {pid}"
            );
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
        // A stopped keeper's state may remain, but the owned mount must
        // be detached before Data removes the fixture's directory.
        let mounts = run(&mut Command::new("mount")).stdout;
        assert!(!String::from_utf8_lossy(&mounts).contains(self.data.to_str().unwrap()));
    }
}
