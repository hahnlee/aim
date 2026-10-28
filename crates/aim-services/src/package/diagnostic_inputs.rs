//! Retained native Settings read messages and CompilerStats input owners.
//! CompilerStats at android-16.0.0_r1, Copyright AOSP, Apache License 2.0.
use super::{
    apps_filter::NotModelled,
    model::State,
    owner::recovery::{Event, Report},
    settings::Settings,
};
use std::{
    collections::BTreeMap,
    fmt::Write,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, PartialEq)]
pub struct SettingsMessages {
    text: String,
}
impl SettingsMessages {
    /// Native reader callbacks supply their actual ordered messages. None means
    /// that reader's diagnostics were not retained and is never an empty log.
    pub fn completed(
        report: &Report,
        settings: &Settings,
        parser_messages: Option<&str>,
        related_messages: Option<&str>,
    ) -> Result<Self, String> {
        let mut text = String::new();
        for event in &report.events {
            match event {
                Event::NoStartTag(_) => text.push_str("No start tag found in settings file\n"),
                Event::Failed { message, .. } => {
                    writeln!(text, "Error reading: {message}").unwrap();
                }
                Event::OwnerFailed { message, .. }
                | Event::FatalInput { message, .. }
                | Event::CompletionFailed(message) => return Err(message.clone()),
                _ => {}
            }
        }
        text.push_str(parser_messages.ok_or("native Settings parser messages were not retained")?);
        text.push_str(
            related_messages.ok_or("native related-settings read messages were not retained")?,
        );
        if !report.first_boot {
            writeln!(
                text,
                "Read completed successfully: {} packages, {} shared uids",
                settings.packages.len(),
                settings.shared_users.len()
            )
            .unwrap();
        }
        Ok(Self { text })
    }
    pub fn text(&self) -> &str {
        &self.text
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CompilerRead {
    Absent,
    Main,
    Backup,
    Malformed(String),
}
#[derive(Debug)]
pub struct CompilerStats {
    packages: Mutex<BTreeMap<String, Vec<(String, i64)>>>,
    read: CompilerRead,
    path: PathBuf,
    file_lock: Mutex<()>,
}
impl PartialEq for CompilerStats {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}
impl CompilerStats {
    /// Missing package-cstats.list is the original constructor/readInternal
    /// empty state. Other I/O failures are distinct from this observed absence.
    pub fn read(data: &Path) -> Result<Arc<Self>, String> {
        let main = data.join("system/package-cstats.list");
        let backup = data.join("system/package-cstats.list.bak");
        let input = match fs::read(&backup) {
            Ok(bytes) => Some((CompilerRead::Backup, bytes)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => match fs::read(&main) {
                Ok(bytes) => Some((CompilerRead::Main, bytes)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => return Err(format!("compiler stats main read: {error}")),
            },
            Err(error) => return Err(format!("compiler stats backup read: {error}")),
        };
        let mut packages = BTreeMap::new();
        let read = match input {
            None => CompilerRead::Absent,
            Some((source, bytes)) => {
                // InputStreamReader replaces malformed UTF-8; Java read retains
                // successfully parsed packages when a later line is malformed.
                match parse_stats(&String::from_utf8_lossy(&bytes), &mut packages) {
                    Ok(()) => source,
                    Err(error) => CompilerRead::Malformed(error),
                }
            }
        };
        Ok(Arc::new(Self {
            packages: Mutex::new(packages),
            read,
            path: main,
            file_lock: Mutex::new(()),
        }))
    }
    /// CompilerStats.writeInternal logs I/O failures and lets shutdown continue.
    pub fn write_now(&self) {
        if let Err(error) = self.write() {
            eprintln!("Failed to write compiler stats: {error}");
        }
    }
    fn write(&self) -> io::Result<()> {
        use std::{io::Write as _, os::unix::fs::OpenOptionsExt};
        let _file = self.file_lock.lock().unwrap();
        let mut text = String::from("PACKAGE_MANAGER__COMPILER_STATS__1\n");
        {
            let packages = self.packages.lock().unwrap();
            for (package, times) in packages.iter().filter(|(_, times)| !times.is_empty()) {
                writeln!(text, "{package}").unwrap();
                for (path, time) in times {
                    writeln!(text, "-{path}:{time}").unwrap();
                }
            }
        }
        let backup = super::sibling(&self.path, ".bak");
        match fs::rename(&backup, &self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let new = super::sibling(&self.path, ".new");
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&new)?;
            file.write_all(text.as_bytes())?;
            crate::fsync(&file)?;
            aim_storage::guest_inode::record(
                &new,
                aim_storage::guest_inode::GuestInode {
                    uid: Some(1000),
                    gid: Some(1000),
                    mode: Some(0o600),
                },
            )?;
            fs::rename(&new, &self.path)
        })();
        if result.is_err() {
            // AtomicFile.failWrite leaves the committed main file intact.
            if let Err(error) = fs::remove_file(&new)
                && error.kind() != io::ErrorKind::NotFound
            {
                eprintln!("Failed to remove compiler stats new file: {error}");
            }
        }
        result
    }
    pub fn read_result(&self) -> &CompilerRead {
        &self.read
    }
    /// Called by the actual compiler completion owner with its measured time.
    pub fn record(&self, package: &str, code_path: &str, milliseconds: i64) {
        let name = code_path.rsplit('/').next().unwrap_or(code_path);
        let mut packages = self.packages.lock().unwrap();
        set_time(
            packages.entry(package.into()).or_default(),
            name,
            milliseconds,
        );
    }
    pub fn text(&self, state: &State, package: Option<&str>) -> String {
        let packages = self.packages.lock().unwrap();
        let mut selected: Vec<_> = state
            .packages
            .values()
            .filter(|value| value.pkg.is_some() && package.is_none_or(|name| name == value.name))
            .collect();
        selected.sort_by_key(|package| super::info::java_hash(&package.name));
        let mut text = String::from("Compiler stats:\n");
        for package in selected {
            writeln!(text, "  [{}]", package.name).unwrap();
            match packages
                .get(&package.name)
                .filter(|values| !values.is_empty())
            {
                None => text.push_str("    (No recorded stats)\n"),
                Some(values) => {
                    let mut values = values.iter().collect::<Vec<_>>();
                    values.sort_by_key(|(name, _)| super::info::java_hash(name));
                    for (name, time) in values {
                        writeln!(text, "     {name} - {time}").unwrap();
                    }
                }
            }
        }
        text
    }
}
fn set_time(values: &mut Vec<(String, i64)>, path: &str, time: i64) {
    let path = path.rsplit('/').next().unwrap_or(path);
    if let Some(index) = values.iter().position(|(name, _)| name == path) {
        if time <= 0 {
            values.remove(index);
        } else {
            values[index].1 = time;
        }
    } else if time > 0 {
        values.push((path.into(), time));
    }
}
fn parse_stats(
    text: &str,
    packages: &mut BTreeMap<String, Vec<(String, i64)>>,
) -> Result<(), String> {
    let mut lines = text.lines();
    let header = lines.next().ok_or("No version line found.")?;
    let version = header
        .strip_prefix("PACKAGE_MANAGER__COMPILER_STATS__")
        .ok_or_else(|| format!("Invalid version line: {header}"))?
        .parse::<i32>()
        .map_err(|error| error.to_string())?;
    if version != 1 {
        return Err(format!("Unexpected version: {version}"));
    }
    let mut current: Option<String> = None;
    let mut ignored = Vec::new();
    for line in lines {
        if let Some(line) = line.strip_prefix('-') {
            let (path, time) = line
                .split_once(':')
                .filter(|(path, _)| !path.is_empty())
                .ok_or_else(|| format!("Could not parse data -{line}"))?;
            let time = time.parse::<i64>().map_err(|error| error.to_string())?;
            match &current {
                Some(package) => set_time(packages.entry(package.clone()).or_default(), path, time),
                None => set_time(&mut ignored, path, time),
            }
        } else {
            current = Some(line.into());
            packages.entry(line.into()).or_default();
        }
    }
    Ok(())
}

/// Calls the independent original ART owner over a retained typed bridge. The
/// bridge uses PackageManagerLocal's native facade snapshot, never original PMS.
pub type ArtDump = Arc<dyn Fn(Option<&str>) -> Result<String, String> + Send + Sync>;
pub struct Maintenance {
    pub compiler: Arc<CompilerStats>,
    pub art: ArtDump,
}
impl std::fmt::Debug for Maintenance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MaintenanceDiagnostics")
            .finish_non_exhaustive()
    }
}
impl PartialEq for Maintenance {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}
impl Maintenance {
    pub fn text(
        &self,
        state: &State,
        kind: i32,
        package: Option<&str>,
    ) -> Result<String, NotModelled> {
        match kind {
            1048576 => (self.art)(package)
                .map_err(|_| NotModelled("retained independent ART diagnostics owner failed")),
            2097152 => Ok(self.compiler.text(state, package)),
            _ => Err(NotModelled("unknown maintenance diagnostic input")),
        }
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;
    #[test]
    fn compiler_shutdown_persists_live_times_recovers_backup_and_preserves_failed_commit() {
        let data = std::env::temp_dir().join(format!("compiler-shutdown-{}", std::process::id()));
        fs::create_dir_all(data.join("system")).unwrap();
        let main = data.join("system/package-cstats.list");
        let backup = super::super::sibling(&main, ".bak");
        fs::write(&main, "stale").unwrap();
        fs::write(
            &backup,
            "PACKAGE_MANAGER__COMPILER_STATS__1\np\n-base.apk:4\n",
        )
        .unwrap();
        let owner = CompilerStats::read(&data).unwrap();
        assert_eq!(owner.read_result(), &CompilerRead::Backup);
        owner.record("p", "/data/app/p/base.apk", 17);
        owner.record("p", "/data/app/p/split.apk", 9);
        owner.record("empty", "base.apk", 0);
        owner.write().unwrap();
        assert!(!backup.exists());
        let inode = aim_storage::guest_inode::read(&main).unwrap().unwrap();
        assert_eq!(
            (inode.uid, inode.gid, inode.mode),
            (Some(1000), Some(1000), Some(0o600))
        );
        let committed = fs::read_to_string(&main).unwrap();
        assert_eq!(
            committed,
            "PACKAGE_MANAGER__COMPILER_STATS__1\np\n-base.apk:17\n-split.apk:9\n"
        );
        let reloaded = CompilerStats::read(&data).unwrap();
        assert_eq!(
            reloaded.packages.lock().unwrap().get("p"),
            owner.packages.lock().unwrap().get("p")
        );
        assert!(!reloaded.packages.lock().unwrap().contains_key("empty"));
        owner.record("p", "base.apk", 0);
        let new = super::super::sibling(&main, ".new");
        fs::create_dir(&new).unwrap();
        assert!(owner.write().is_err());
        owner.write_now(); // I/O failure does not abort shutdown.
        assert_eq!(fs::read_to_string(&main).unwrap(), committed);
        fs::remove_dir(&new).unwrap();
        owner.write().unwrap();
        assert_eq!(
            fs::read_to_string(&main).unwrap(),
            "PACKAGE_MANAGER__COMPILER_STATS__1\np\n-split.apk:9\n"
        );
        fs::remove_dir_all(data).unwrap();
    }
}
