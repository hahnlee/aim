//! The guest filesystem view init reads its inputs from.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A directory on the host that holds the guest's `/` (an extracted or
/// derived system image). Every path this crate reads is a guest absolute
/// path such as `/system/etc/init/hw/init.rc`.
#[derive(Clone, Debug)]
pub struct ImageRoot {
    root: PathBuf,
}

impl ImageRoot {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Host path of a guest path. Relative guest paths are taken from `/`.
    pub fn host_path(&self, guest: &str) -> PathBuf {
        self.root.join(guest.trim_start_matches('/'))
    }

    pub fn read(&self, guest: &str) -> io::Result<Vec<u8>> {
        fs::read(self.host_path(guest))
    }

    pub fn exists(&self, guest: &str) -> bool {
        self.host_path(guest).exists()
    }

    pub fn is_dir(&self, guest: &str) -> bool {
        self.host_path(guest).is_dir()
    }

    /// Regular files directly in a guest directory, as guest paths, in
    /// unspecified (`readdir`) order; callers sort as init does.
    pub fn regular_files(&self, guest_dir: &str) -> io::Result<Vec<String>> {
        let mut files = Vec::new();
        for entry in fs::read_dir(self.host_path(guest_dir))? {
            let entry = entry?;
            // init checks d_type == DT_REG, which does not follow symlinks.
            if entry.file_type()?.is_file() {
                let name = entry.file_name().to_string_lossy().into_owned();
                files.push(format!("{}/{}", guest_dir.trim_end_matches('/'), name));
            }
        }
        Ok(files)
    }

    /// Subdirectory names of a guest directory (`d_type == DT_DIR`).
    pub fn subdirectories(&self, guest_dir: &str) -> io::Result<Vec<String>> {
        let mut dirs = Vec::new();
        for entry in fs::read_dir(self.host_path(guest_dir))? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                dirs.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        Ok(dirs)
    }
}
