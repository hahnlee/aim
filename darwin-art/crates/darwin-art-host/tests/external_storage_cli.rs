//! Exercise the embedded host's real migration CLI, without starting ART/apps.
use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);
const PACKAGE: &str = "com.example.player";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "darwin-external-cli-{}-{time}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        for name in ["storage", "app", "outside"] {
            fs::create_dir(path.join(name)).unwrap();
        }
        Self(path)
    }
    fn destination(&self) -> PathBuf {
        self.0
            .join("storage/emulated/0/Android/data")
            .join(PACKAGE)
            .join("files")
    }
    fn legacy(&self) -> PathBuf {
        self.0.join("app/external")
    }
    fn command(&self, package: &str) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_darwin-art-host"));
        cmd.arg("--prepare-external-storage")
            .arg(self.0.join("storage"))
            .arg(self.0.join("app"))
            .arg(package)
            .env_remove("DARWIN_ART_DEBUG_ATTACH_DELAY_MS")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }
    fn run(&self) -> Output {
        self.command(PACKAGE).output().unwrap()
    }
    fn success(&self) {
        let result = self.run();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            String::from_utf8(result.stdout).unwrap().trim(),
            self.destination().to_str().unwrap()
        );
    }
    fn seed(&self) {
        fs::create_dir(self.legacy()).unwrap();
        fs::write(self.legacy().join("photo"), b"original bytes").unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn moves_without_copying_and_preserves_legacy_producers() {
    let f = Fixture::new();
    f.seed();
    let inode = fs::metadata(f.legacy().join("photo")).unwrap().ino();
    let directory_inode = fs::metadata(f.legacy()).unwrap().ino();
    f.success();
    f.success();
    assert_eq!(fs::read_link(f.legacy()).unwrap(), f.destination());
    assert_eq!(
        fs::metadata(f.destination()).unwrap().ino(),
        directory_inode
    );
    assert_eq!(
        fs::metadata(f.destination().join("photo")).unwrap().ino(),
        inode
    );
    assert_eq!(
        fs::read(f.destination().join("photo")).unwrap(),
        b"original bytes"
    );
    fs::write(f.legacy().join("later"), b"host producer").unwrap();
    assert_eq!(
        fs::read(f.destination().join("later")).unwrap(),
        b"host producer"
    );
}

#[test]
fn fresh_install_and_interrupted_move_are_recoverable() {
    let fresh = Fixture::new();
    fresh.success();
    assert!(fresh.destination().is_dir());
    assert_eq!(fs::read_link(fresh.legacy()).unwrap(), fresh.destination());
    let moved = Fixture::new();
    moved.seed();
    let inode = fs::metadata(moved.legacy()).unwrap().ino();
    fs::create_dir_all(moved.destination().parent().unwrap()).unwrap();
    fs::rename(moved.legacy(), moved.destination()).unwrap();
    moved.success();
    assert_eq!(fs::metadata(moved.legacy()).unwrap().ino(), inode);
    assert_eq!(
        fs::read(moved.legacy().join("photo")).unwrap(),
        b"original bytes"
    );
}

#[test]
fn concurrent_publishers_reuse_one_directory() {
    let f = Fixture::new();
    f.seed();
    let inode = fs::metadata(f.legacy()).unwrap().ino();
    let first = f.command(PACKAGE).spawn().unwrap();
    let second = f.command(PACKAGE).spawn().unwrap();
    for result in [
        first.wait_with_output().unwrap(),
        second.wait_with_output().unwrap(),
    ] {
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    assert_eq!(fs::metadata(f.destination()).unwrap().ino(), inode);
    assert_eq!(fs::read_link(f.legacy()).unwrap(), f.destination());
}

#[test]
fn conflicts_preserve_both_trees_without_merging() {
    let f = Fixture::new();
    f.seed();
    fs::create_dir_all(f.destination()).unwrap();
    fs::write(f.destination().join("other"), b"different").unwrap();
    assert!(!f.run().status.success());
    assert!(
        !fs::symlink_metadata(f.legacy())
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read(f.legacy().join("photo")).unwrap(),
        b"original bytes"
    );
    assert_eq!(
        fs::read(f.destination().join("other")).unwrap(),
        b"different"
    );
    assert!(!f.destination().join("photo").exists());
}

#[test]
fn foreign_links_are_never_followed_or_replaced() {
    let legacy = Fixture::new();
    symlink(legacy.0.join("outside"), legacy.legacy()).unwrap();
    assert!(!legacy.run().status.success());
    assert_eq!(
        fs::read_link(legacy.legacy()).unwrap(),
        legacy.0.join("outside")
    );
    let ancestor = Fixture::new();
    ancestor.seed();
    symlink(
        ancestor.0.join("outside"),
        ancestor.0.join("storage/emulated"),
    )
    .unwrap();
    assert!(!ancestor.run().status.success());
    assert_eq!(fs::read_dir(ancestor.0.join("outside")).unwrap().count(), 0);
    assert!(ancestor.legacy().join("photo").is_file());
    let target = Fixture::new();
    target.seed();
    fs::create_dir_all(target.destination().parent().unwrap()).unwrap();
    symlink(target.0.join("outside"), target.destination()).unwrap();
    assert!(!target.run().status.success());
    assert!(target.legacy().join("photo").is_file());
}

#[test]
fn invalid_package_is_rejected_before_storage_changes() {
    let f = Fixture::new();
    f.seed();
    for package in ["../escape", "com..bad", "", "com.example/other"] {
        assert!(!f.command(package).output().unwrap().status.success());
    }
    assert_eq!(fs::read_dir(f.0.join("storage")).unwrap().count(), 0);
    assert_eq!(
        fs::read(f.legacy().join("photo")).unwrap(),
        b"original bytes"
    );
}
