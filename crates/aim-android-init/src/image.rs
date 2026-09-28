//! The guest filesystem view init reads its inputs from.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Symlinks followed in one lookup, as Linux's `MAXSYMLINKS`.
const MAX_SYMLINKS: usize = 40;

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
    ///
    /// Symlinks in the image are followed component by component relative
    /// to the guest root, as for a process whose root is the image: a GSI's
    /// `/system_ext -> /system/system_ext` stays inside the image instead of
    /// naming the Mac's own `/system`.
    pub fn host_path(&self, guest: &str) -> PathBuf {
        let mut pending: Vec<String> = components(guest).rev().collect();
        let mut done: Vec<String> = Vec::new();
        let mut links = 0;
        while let Some(component) = pending.pop() {
            match component.as_str() {
                "." => continue,
                ".." => {
                    done.pop();
                    continue;
                }
                _ => done.push(component),
            }
            let host = self.join(&done);
            let Ok(target) = fs::read_link(&host) else {
                continue;
            };
            links += 1;
            if links > MAX_SYMLINKS {
                // The host call on the link reports the loop.
                return host;
            }
            done.pop();
            let target = target.to_string_lossy();
            if target.starts_with('/') {
                done.clear();
            }
            pending.extend(components(&target).rev());
        }
        self.join(&done)
    }

    fn join(&self, components: &[String]) -> PathBuf {
        let mut path = self.root.clone();
        path.extend(components);
        path
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

fn components(path: &str) -> impl DoubleEndedIterator<Item = String> + '_ {
    path.split('/')
        .filter(|c| !c.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn symlinks_resolve_inside_the_image() {
        let root = std::env::temp_dir().join(format!("image-root-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("system/system_ext/etc/init")).unwrap();
        fs::create_dir_all(root.join("system/product/etc")).unwrap();
        fs::write(root.join("system/system_ext/etc/init/a.rc"), "on boot\n").unwrap();
        fs::write(root.join("system/product/etc/build.prop"), "ro.x=1\n").unwrap();
        // A GSI's partition links: absolute, and relative through `..`.
        symlink("/system/system_ext", root.join("system_ext")).unwrap();
        symlink("../system/product", root.join("product")).unwrap();
        symlink("/loop", root.join("loop")).unwrap();
        let image = ImageRoot::new(&root);

        assert_eq!(
            image.host_path("/system_ext/etc/init/a.rc"),
            root.join("system/system_ext/etc/init/a.rc")
        );
        assert_eq!(image.read("/product/etc/build.prop").unwrap(), b"ro.x=1\n");
        assert!(image.is_dir("/system_ext/etc/init"));
        assert_eq!(
            image.regular_files("/system_ext/etc/init").unwrap(),
            ["/system_ext/etc/init/a.rc"]
        );
        assert_eq!(image.subdirectories("/product").unwrap(), ["etc"]);
        assert!(!image.exists("/system_ext/missing"));
        assert!(!image.exists("/loop/x"));
        fs::remove_dir_all(&root).unwrap();
    }
}
