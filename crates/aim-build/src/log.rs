//! A node's log (`target/aim-cache/logs/<node>.log`) and the commands it
//! runs.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub struct Log {
    file: File,
    verbose: bool,
}

impl Log {
    pub fn create(path: PathBuf, verbose: bool) -> Result<Log, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let file = File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Log { file, verbose })
    }

    /// Another handle on the same log, for a command run on another thread.
    pub fn share(&self) -> Result<Log, String> {
        let file = self.file.try_clone().map_err(|e| e.to_string())?;
        Ok(Log {
            file,
            verbose: self.verbose,
        })
    }

    pub fn line(&mut self, text: &str) {
        let _ = writeln!(self.file, "{text}");
        if self.verbose {
            eprintln!("{text}");
        }
    }

    fn describe(cmd: &Command) -> String {
        let mut text = cmd.get_program().to_string_lossy().into_owned();
        for arg in cmd.get_args() {
            text.push(' ');
            text.push_str(&arg.to_string_lossy());
        }
        if let Some(dir) = cmd.get_current_dir() {
            text = format!("(cd {} && {text})", dir.display());
        }
        text
    }

    fn sink(&self) -> Result<Stdio, String> {
        if self.verbose {
            return Ok(Stdio::inherit());
        }
        let file = self.file.try_clone().map_err(|e| e.to_string())?;
        Ok(Stdio::from(file))
    }

    /// Runs `cmd` to completion, its output going to the log.
    pub fn run(&mut self, cmd: &mut Command) -> Result<(), String> {
        let text = Self::describe(cmd);
        self.line(&format!("$ {text}"));
        let status = cmd
            .stdin(Stdio::null())
            .stdout(self.sink()?)
            .stderr(self.sink()?)
            .status()
            .map_err(|e| format!("{text}: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("{text}: {status}"))
        }
    }

    /// Runs `cmd` and returns its standard output; errors go to the log.
    pub fn output(&mut self, cmd: &mut Command) -> Result<String, String> {
        let text = Self::describe(cmd);
        self.line(&format!("$ {text}"));
        let out = cmd
            .stdin(Stdio::null())
            .stderr(self.sink()?)
            .output()
            .map_err(|e| format!("{text}: {e}"))?;
        if !out.status.success() {
            return Err(format!("{text}: {}", out.status));
        }
        String::from_utf8(out.stdout).map_err(|e| format!("{text}: {e}"))
    }

    /// The last `lines` lines of the log.
    pub fn tail(path: &Path, lines: usize) -> String {
        let text = fs::read_to_string(path).unwrap_or_default();
        let all: Vec<&str> = text.lines().collect();
        all[all.len().saturating_sub(lines)..].join("\n")
    }
}
