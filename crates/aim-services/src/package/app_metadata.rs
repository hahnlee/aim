//! GET_APP_METADATA native Settings path and read-only Binder FD ownership.
use aim_binder_driver::File;
use aim_binder_host::parcel::Parcel;
use aim_service_aidl::WriteParcelable;
use std::{fs, os::fd::AsFd, path::{Path, PathBuf}, sync::Mutex};
pub struct Descriptor(pub File);
impl WriteParcelable for Descriptor {
    fn write_to(&self, parcel: &mut Parcel) { parcel.write_i32(0); parcel.write_file(self.0.clone()); }
}
pub struct Owner { data: PathBuf, errors: Mutex<Vec<String>> }
impl Owner {
    pub fn new(data: &Path) -> Self { Self { data: data.to_owned(), errors: Mutex::new(Vec::new()) } }
    pub fn take_errors(&self) -> Vec<String> { std::mem::take(&mut *self.errors.lock().unwrap()) }
    pub fn open(&self, path: &str) -> Result<Option<Descriptor>, String> {
        let relative = Path::new(path).strip_prefix("/data").map_err(|_| "app metadata path is outside native Android data")?;
        if relative.components().any(|component| matches!(component, std::path::Component::ParentDir)) { return Err("app metadata path contains parent traversal".into()); }
        let mapped = self.data.join(relative);
        let file = match fs::File::open(&mapped) {
            Ok(file) => file,
            Err(error) => { self.errors.lock().unwrap().push(format!("app metadata open: {error}")); return Ok(None); }
        };
        if file.metadata().map_err(|error| error.to_string())?.is_dir() {
            self.errors.lock().unwrap().push("app metadata open: Is a directory".into()); return Ok(None);
        }
        // file_from_fd duplicates/retains the fileport before this File is closed.
        let file = aim_binder_host::server::file_from_fd(file.as_fd()).ok_or("app metadata Binder fileport export failed")?;
        Ok(Some(Descriptor(file)))
    }
}
impl std::fmt::Debug for Owner { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("AppMetadataFiles").finish_non_exhaustive() } }
impl PartialEq for Owner { fn eq(&self, other: &Self) -> bool { std::ptr::eq(self, other) } }
