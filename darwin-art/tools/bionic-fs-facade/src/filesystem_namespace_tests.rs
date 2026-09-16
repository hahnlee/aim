use super::*;
use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("darwin-namespace-{}-{nonce}", std::process::id()));
        fs::create_dir(&path).unwrap();
        fs::create_dir(path.join("root")).unwrap();
        fs::create_dir(path.join("private")).unwrap();
        Self(path)
    }
    fn root(&self) -> File {
        File::open(self.0.join("root")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn initial_cwd_resolves_links_before_parent_components() {
    let f = Fixture::new();
    fs::create_dir_all(f.0.join("root/real/nested")).unwrap();
    symlink("/real/nested", f.0.join("root/link")).unwrap();
    let ns = FilesystemNamespace::new(f.root(), b"/", b"/link/..", None).unwrap();
    assert_eq!(ns.cwd.snapshot().unwrap(), b"/real");
    assert_eq!(
        ns.cwd.metadata().unwrap().ino(),
        fs::metadata(f.0.join("root/real")).unwrap().ino()
    );
}

#[test]
fn initial_cwd_uses_retained_private_mount_not_root_shadow() {
    let f = Fixture::new();
    fs::create_dir_all(f.0.join("root/data/work")).unwrap();
    fs::create_dir(f.0.join("private/work")).unwrap();
    let private = PrivateDataRoot::open(f.0.join("private")).unwrap();
    fs::rename(f.0.join("private"), f.0.join("retained")).unwrap();
    fs::create_dir_all(f.0.join("private/work")).unwrap();
    let ns = FilesystemNamespace::new(f.root(), b"/", b"/data/work", Some(&private)).unwrap();
    assert_eq!(ns.cwd.snapshot().unwrap(), b"/data/work");
    assert_eq!(
        ns.cwd.metadata().unwrap().ino(),
        fs::metadata(f.0.join("retained/work")).unwrap().ino()
    );
    assert_ne!(
        ns.cwd.metadata().unwrap().ino(),
        fs::metadata(f.0.join("root/data/work")).unwrap().ino()
    );
    assert!(FilesystemNamespace::new(f.root(), b"/", b"/data/work", None).is_err());
}

#[test]
fn invalid_cwd_does_not_publish_or_consume_other_authorities() {
    let f = Fixture::new();
    fs::write(f.0.join("root/file"), b"x").unwrap();
    for cwd in [b"relative".as_slice(), b"/missing", b"/file", b"/nul\0"] {
        assert!(FilesystemNamespace::new(f.root(), b"/", cwd, None).is_err());
    }
    assert!(FilesystemNamespace::new(f.root(), b"/system", b"/data", None).is_err());
    let ns = FilesystemNamespace::new(f.root(), b"/system", b"/system", None).unwrap();
    assert_eq!(ns.cwd.snapshot().unwrap(), b"/system");
}
