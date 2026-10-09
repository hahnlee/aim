//! Programs run in the resident guest, and what its processes use.

use std::io::Read;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::state::{Files, State};

/// The running guest: a program run through linux-run with its root,
/// filesystem view and binder, in its pid namespace.
pub struct Guest {
    pub linux_run: PathBuf,
    pub image: PathBuf,
    pub path_map: PathBuf,
    pub by_pid: PathBuf,
    pub environ: PathBuf,
    pub binder: String,
}

impl Guest {
    /// The guest of `state`, once guest-init has laid out its filesystem.
    pub fn of(files: &Files, state: &State) -> Option<Guest> {
        let guest = state.guest?;
        files.path_map().exists().then(|| Guest {
            linux_run: state.inputs.program("linux-run"),
            image: state.inputs.image.clone(),
            path_map: files.path_map(),
            by_pid: files.by_pid(),
            environ: files.environ(),
            binder: format!("dev.aim.guest-init.{guest}.binder"),
        })
    }

    /// `argv` (a guest program and its arguments) run in the guest with
    /// init's global environment, as adbd's shell inherits it; before
    /// init's first `export`, with linux-run's default one.
    pub fn command(&self, argv: &[&str]) -> Command {
        let mut command = Command::new(&self.linux_run);
        if let Ok(environ) = std::fs::read_to_string(&self.environ) {
            command
                .env_clear()
                .envs(environ.lines().filter_map(|l| l.split_once('=')))
                .arg("--inherit-env");
        }
        command
            .arg("--root")
            .arg(&self.image)
            .arg("--path-map")
            .arg(&self.path_map)
            .args(["--binder", &self.binder])
            .arg("--by-pid")
            .arg(&self.by_pid)
            .arg("--mount-namespace-from-init")
            .args(argv);
        command
    }

    /// `argv`'s exit status and output; its whole process group is killed
    /// after `timeout`.
    pub fn output(&self, argv: &[&str], timeout: Duration) -> Result<(ExitStatus, String), String> {
        let mut child = self
            .command(argv)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|e| format!("linux-run: {e}"))?;
        let mut stdout = child.stdout.take().expect("piped");
        let reader = std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = stdout.read_to_end(&mut out);
            out
        });
        let deadline = Instant::now() + timeout;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                break status;
            }
            if Instant::now() > deadline {
                // SAFETY: the process group of the child we started.
                unsafe { libc::killpg(child.id() as i32, libc::SIGKILL) };
                let _ = child.wait();
                return Err(format!("{}: no answer after {timeout:?}", argv.join(" ")));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let out = String::from_utf8_lossy(&reader.join().unwrap_or_default()).into_owned();
        if status.signal() == Some(libc::SIGKILL) {
            return Err(format!("{}: linux-run was killed", argv.join(" ")));
        }
        Ok((status, out))
    }

    /// Whether Android finished booting (`sys.boot_completed`).
    pub fn booted(&self) -> bool {
        self.output(
            &["/system/bin/getprop", "sys.boot_completed"],
            Duration::from_secs(10),
        )
        .is_ok_and(|(status, out)| status.success() && out.trim() == "1")
    }
}

/// What the guest's processes use.
#[derive(Debug, Default, PartialEq)]
pub struct Usage {
    pub processes: usize,
    pub rss_kib: u64,
    /// Percent of one core, recently (`ps`'s `%cpu`).
    pub cpu: f64,
}

/// The guest's processes in `ps -o pid=,rss=,%cpu=,command=` output:
/// guest-init (`guest`, which also hosts binder) and the linux-run
/// processes on its filesystem view.
pub fn usage(ps: &str, guest: u32, path_map: &str) -> Usage {
    let needle = format!("--path-map {path_map}");
    let mut u = Usage::default();
    for line in ps.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(kib), Some(cpu), Some(program)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let ours =
            pid.parse() == Ok(guest) || (program.ends_with("/linux-run") && line.contains(&needle));
        if !ours {
            continue;
        }
        u.processes += 1;
        u.rss_kib += kib.parse::<u64>().unwrap_or(0);
        u.cpu += cpu.parse::<f64>().unwrap_or(0.0);
    }
    u
}

/// [`usage`] of the running guest.
pub fn measure(files: &Files, guest: u32) -> Usage {
    let ps = Command::new("/bin/ps")
        .args(["-axww", "-o", "pid=,rss=,%cpu=,command="])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    usage(&ps, guest, &files.path_map().to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_uses_resident_selection_and_authenticated_namespace_entry() {
        let root = std::env::temp_dir().join(format!("aimctl-selected-shell-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let files = Files::of(&root.join("data")).unwrap();
        std::fs::create_dir_all(files.path_map().parent().unwrap()).unwrap();
        std::fs::write(files.path_map(), b"fixture").unwrap();
        let selected = crate::inputs::Inputs { image: "/selected-image".into(),
            host_runtime: "/selected-runtime".into(), userdata: "/selected-templates".into() };
        let original = State { pid: 1, guest: Some(42), windows: false, started: 0, inputs: selected };
        let stored = State::parse(&original.to_text()).unwrap();
        let guest = Guest::of(&files, &stored).unwrap();
        let command = guest.command(&["/system/bin/getprop", "sys.boot_completed"]);
        assert_eq!(command.get_program(), "/selected-runtime/linux-run");
        let args = command.get_args().map(|arg| arg.to_string_lossy()).collect::<Vec<_>>();
        assert!(args.windows(2).any(|pair| pair == ["--root", "/selected-image"]));
        assert!(args.windows(2).any(|pair| pair == ["--binder", "dev.aim.guest-init.42.binder"]));
        assert!(args.iter().any(|arg| arg == "--mount-namespace-from-init"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn processes_of_one_guest() {
        let ps = "\
  101  2048   1.5 /t/release/guest-init --image /i --data /d --run
  102 10240  20.0 /t/release/linux-run --root /i --path-map /d/run/path-map /system/bin/logd
  103 20480   0.5 /t/release/linux-run --path-map /d/run/path-map --fork-child 3,4
  104 40960   9.0 /t/release/linux-run --root /i --path-map /other/run/path-map /system/bin/logd
  105   512   0.0 /bin/zsh -c linux-run --path-map /d/run/path-map
";
        assert_eq!(
            usage(ps, 101, "/d/run/path-map"),
            Usage {
                processes: 3,
                rss_kib: 32768,
                cpu: 22.0
            }
        );
        assert_eq!(usage("", 1, "/d"), Usage::default());
    }
}
