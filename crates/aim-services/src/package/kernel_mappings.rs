//! Settings' configfs sdcardfs mapping writer, Android 16 r1.
use std::{collections::BTreeMap, fs::{self, OpenOptions}, io::Write, path::{Path, PathBuf}, sync::Mutex};
#[derive(Default)]
struct Package { app: i32, excluded: Option<Vec<i32>> }
pub struct Owner { root: Option<PathBuf>, state: Mutex<BTreeMap<String, Package>> }
impl Owner {
    /// Absence of this actual guest-visible kernel ABI is the original
    /// mKernelMappingFilename=null branch. No substitute files are created.
    pub fn inspect(root: &Path) -> Result<Self, String> {
        let root = match fs::symlink_metadata(root) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Some(root.to_path_buf()),
            Ok(_) => return Err("kernel mapping endpoint is not a directory".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.to_string()),
        };
        Ok(Self { root, state: Mutex::new(BTreeMap::new()) })
    }
    pub fn update(&self, name: &str, app: i32, excluded: &[i32]) -> Result<(), String> {
        let Some(root) = &self.root else { return Ok(()); };
        if name.is_empty() || name.contains('/') || name == "." || name == ".." { return Err("invalid kernel mapping package name".into()); }
        let mut states = self.state.lock().unwrap();
        let first = !states.contains_key(name);
        let directory = root.join(name);
        if first {
            match fs::create_dir(&directory) { Ok(()) => {}, Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}, Err(error) => return Err(error.to_string()) }
        }
        let current = states.entry(name.into()).or_default();
        if current.app != app { integer(&directory.join("appid"), app)?; }
        if first || current.excluded.as_deref() != Some(excluded) {
            for user in excluded {
                if current.excluded.as_ref().is_none_or(|old| !old.contains(user)) { integer(&directory.join("excluded_userids"), *user)?; }
            }
            if let Some(old) = &current.excluded {
                for user in old { if !excluded.contains(user) { integer(&directory.join("clear_userid"), *user)?; } }
            }
            current.excluded = Some(excluded.to_vec());
        }
        // Original KernelPackageState's appId stays its constructor value;
        // preserve that behavior instead of suppressing subsequent writes.
        Ok(())
    }
    pub fn remove_user(&self, user: i32) -> Result<(), String> {
        if let Some(root) = &self.root { integer(&root.join("remove_userid"), user)?; }
        Ok(())
    }
}
fn integer(path: &Path, value: i32) -> Result<(), String> {
    // configfs attribute writes are commands, not normal file replacement.
    // An absent attribute is an incomplete kernel owner, never a created file.
    let mut file = OpenOptions::new().write(true).open(path).map_err(|error| error.to_string())?;
    file.write_all(value.to_string().as_bytes()).map_err(|error| error.to_string())
}
