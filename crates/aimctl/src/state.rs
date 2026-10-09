//! Where a resident guest keeps its state, and that state.
//!
//! Beside the data directory `DATA` (the mount point of its data image,
//! `DATA.asif`, whose user holds `DATA.lock`; docs/storage.md) lies
//! `DATA.aimctl/`:
//!
//! ```text
//! lock        held by `aimctl run` while the guest is resident
//! state       its pid, guest-init's pid, the mode and when it started
//! log         the output of the resident guest's processes
//! display     the display server's socket, capture.bmp its captures
//! apps/       the launcher apps' shims (window mode)
//! ```

use std::fs::{self, File};
use std::io::ErrorKind;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

/// The files of the resident guest of one data directory.
#[derive(Debug)]
pub struct Files {
    /// The data directory, with its parent resolved (the guest's path maps
    /// compare real host paths).
    pub data: PathBuf,
    dir: PathBuf,
}

/// `~/Library/Application Support/aim/data`.
pub fn default_data() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").ok_or("no HOME")?;
    Ok(Path::new(&home).join("Library/Application Support/aim/data"))
}

/// `dir` with `suffix` appended to its name.
fn beside(dir: &Path, suffix: &str) -> PathBuf {
    let mut name = dir.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    dir.with_file_name(name)
}

impl Files {
    /// The files of `data`; its parent is created if missing.
    pub fn of(data: &Path) -> Result<Files, String> {
        let (Some(parent), Some(name)) = (data.parent(), data.file_name()) else {
            return Err(format!("{}: not a data directory path", data.display()));
        };
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        let parent = fs::canonicalize(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        let data = parent.join(name);
        let dir = beside(&data, ".aimctl");
        Ok(Files { data, dir })
    }

    /// Whether this is the user's own data directory, `default_data`.
    pub fn is_default(&self) -> bool {
        default_data().is_ok_and(|d| self.is_of(&d))
    }

    /// Whether these are the files of `data` (its parent resolved as
    /// `of` does, without creating it).
    pub fn is_of(&self, data: &Path) -> bool {
        let (Some(parent), Some(name)) = (data.parent(), data.file_name()) else {
            return false;
        };
        fs::canonicalize(parent).is_ok_and(|p| p.join(name) == self.data)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }
    pub fn lock(&self) -> PathBuf {
        self.dir.join("lock")
    }
    pub fn state(&self) -> PathBuf {
        self.dir.join("state")
    }
    pub fn log(&self) -> PathBuf {
        self.dir.join("log")
    }
    pub fn display(&self) -> PathBuf {
        self.dir.join("display")
    }
    pub fn capture(&self) -> PathBuf {
        self.dir.join("capture.bmp")
    }
    pub fn apps(&self) -> PathBuf {
        self.dir.join("apps")
    }
    /// The lock guest-init holds while the data image is attached.
    pub fn guest_lock(&self) -> PathBuf {
        beside(&self.data, ".lock")
    }
    /// The guest's `/data`.
    pub fn guest_data(&self) -> PathBuf {
        self.data.join("data")
    }
    /// guest-init's runtime directory, `DATA.run`.
    fn runtime(&self) -> PathBuf {
        aim_storage::data::runtime_of(&self.data)
    }
    /// The running guest's filesystem view.
    pub fn path_map(&self) -> PathBuf {
        self.runtime().join("path-map")
    }
    /// The running guest's process table: a process that names it joins
    /// the guest's pid namespace.
    pub fn by_pid(&self) -> PathBuf {
        self.runtime().join("identity/by-pid")
    }
    /// init's global environment, which the guest's shells inherit.
    pub fn environ(&self) -> PathBuf {
        self.runtime().join("environ")
    }
}

/// A resident guest, as its `state` file records it.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    /// `aimctl run`.
    pub pid: u32,
    /// guest-init, once started.
    pub guest: Option<u32>,
    pub windows: bool,
    /// Seconds since the Unix epoch.
    pub started: u64,
    pub inputs: crate::inputs::Inputs,
}

impl State {
    pub fn to_text(&self) -> String {
        format!(
            "pid={}\nguest={}\nmode={}\nstarted={}\n{}",
            self.pid,
            self.guest.map_or(String::new(), |p| p.to_string()),
            if self.windows { "windows" } else { "device" },
            self.started,
            self.inputs.to_text()
        )
    }

    pub fn parse(text: &str) -> Result<State, String> {
        let field = |key: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
                .ok_or(format!("state: no {key}"))
        };
        let number = |key: &str| {
            field(key)?
                .parse::<u64>()
                .map_err(|_| format!("state: bad {key}"))
        };
        let guest = field("guest")?;
        Ok(State {
            pid: number("pid")? as u32,
            guest: match guest {
                "" => None,
                g => Some(g.parse().map_err(|_| "state: bad guest".to_string())?),
            },
            windows: match field("mode")? {
                "windows" => true,
                "device" => false,
                _ => return Err("state: bad mode".into()),
            },
            started: number("started")?,
            inputs: crate::inputs::Inputs::parse(text)?,
        })
    }

    /// Writes the state file at `path`, replacing it whole.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        let tmp = path.with_extension("new");
        fs::write(&tmp, self.to_text())
            .and_then(|()| fs::rename(&tmp, path))
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn read(path: &Path) -> Result<Option<State>, String> {
        match fs::read_to_string(path) {
            Ok(text) => State::parse(&text).map(Some),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

/// An exclusive lock on `path`, held until dropped; an error when another
/// process holds it.
pub fn lock(path: &Path) -> Result<File, String> {
    let file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    // SAFETY: flock on an open descriptor.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(format!("{}: held by another process", path.display()));
    }
    Ok(file)
}

/// Whether some process holds the lock on `path`.
pub fn held(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    // SAFETY: flock on an open descriptor; closing it releases the lock.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) != 0 }
}

/// The resident guest of `files`, if there is one: `aimctl run` holds the
/// lock. A state left by one that died is not one.
pub fn resident(files: &Files) -> Result<Option<State>, String> {
    if !held(&files.lock()) {
        return Ok(None);
    }
    match State::read(&files.state())? {
        Some(state) => Ok(Some(state)),
        None => Err(format!(
            "{}: the guest is starting; try again",
            files.data.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("aimctl-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::canonicalize(dir).unwrap()
    }

    #[test]
    fn files_lie_beside_the_data_directory() {
        let root = scratch("files");
        let files = Files::of(&root.join("new/data")).unwrap();
        assert!(root.join("new").is_dir());
        assert_eq!(files.data, root.join("new/data"));
        assert_eq!(files.dir(), root.join("new/data.aimctl"));
        assert_eq!(files.state(), root.join("new/data.aimctl/state"));
        assert_eq!(files.guest_lock(), root.join("new/data.lock"));
        assert_eq!(files.path_map(), root.join("new/data.run/path-map"));
        assert_eq!(files.by_pid(), root.join("new/data.run/identity/by-pid"));
        assert_eq!(files.environ(), root.join("new/data.run/environ"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn state_round_trip() {
        for state in [
            State {
                pid: 12,
                guest: Some(34),
                windows: true,
                started: 1_790_000_000,
                inputs: crate::inputs::Inputs::default(),
            },
            State {
                pid: 12,
                guest: None,
                windows: false,
                started: 5,
                inputs: crate::inputs::Inputs::default(),
            },
        ] {
            assert_eq!(State::parse(&state.to_text()).unwrap(), state);
        }
        assert!(State::parse("pid=1\nguest=\nmode=phone\nstarted=0\n").is_err());
        assert!(State::parse("pid=1\nmode=device\nstarted=0\n").is_err());
        assert!(State::parse("pid=x\nguest=\nmode=device\nstarted=0\n").is_err());
    }

    #[test]
    fn state_file_replaced_and_missing() {
        let root = scratch("state");
        let path = root.join("state");
        assert_eq!(State::read(&path).unwrap(), None);
        let state = State {
            pid: 1,
            guest: None,
            windows: false,
            started: 2,
            inputs: crate::inputs::Inputs::default(),
        };
        state.write(&path).unwrap();
        let later = State {
            guest: Some(3),
            ..state
        };
        later.write(&path).unwrap();
        assert_eq!(State::read(&path).unwrap(), Some(later));
        assert!(!root.join("state.new").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn residency_follows_the_lock() {
        let root = scratch("lock");
        let files = Files::of(&root.join("data")).unwrap();
        fs::create_dir_all(files.dir()).unwrap();
        let state = State {
            pid: 1,
            guest: Some(2),
            windows: false,
            started: 3,
            inputs: crate::inputs::Inputs::default(),
        };
        // A state left behind without its holder.
        state.write(&files.state()).unwrap();
        assert!(!held(&files.lock()));
        assert_eq!(resident(&files).unwrap(), None);
        let holder = lock(&files.lock()).unwrap();
        assert!(held(&files.lock()));
        assert!(lock(&files.lock()).is_err());
        assert_eq!(resident(&files).unwrap(), Some(state));
        fs::remove_file(files.state()).unwrap();
        assert!(resident(&files).is_err());
        drop(holder);
        assert!(!held(&files.lock()));
        fs::remove_dir_all(root).unwrap();
    }
}
