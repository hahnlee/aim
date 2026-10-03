//! Disposable original-runtime fixture support.
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};

pub fn run(command: &mut Command) -> Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}: {}",
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
}
impl Boot {
    /// Start a client with the Linux credentials its original daemon requires.
    pub fn client(&self, uid: u32) -> Command {
        let state = fs::read_to_string(PathBuf::from(format!(
            "{}.aimctl/state",
            self.data.display()
        )))
        .unwrap();
        let guest: u32 = state
            .lines()
            .find_map(|line| line.strip_prefix("guest="))
            .unwrap()
            .parse()
            .unwrap();
        let runtime = PathBuf::from(format!("{}.run", self.data.display()));
        let environ = fs::read_to_string(runtime.join("environ")).unwrap();
        let mut command = Command::new(aim_paths::root().join("target/release/linux-run"));
        command
            .env_clear()
            .envs(environ.lines().filter_map(|line| line.split_once('=')))
            .arg("--inherit-env")
            .arg("--root")
            .arg(aim_paths::derived_image())
            .arg("--path-map")
            .arg(runtime.join("path-map"))
            .arg("--by-pid")
            .arg(runtime.join("identity/by-pid"))
            .args(["--binder", &format!("dev.aim.guest-init.{guest}.binder")])
            .args(["--identity-text", &format!("uid\t{uid}\ngid\t{uid}\n")]);
        command
    }
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(&self.ctl);
        cmd.arg("--data").arg(&self.data);
        cmd
    }
}
impl Drop for Boot {
    fn drop(&mut self) {
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
        run(self.command().arg("stop"));
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
