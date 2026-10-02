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
