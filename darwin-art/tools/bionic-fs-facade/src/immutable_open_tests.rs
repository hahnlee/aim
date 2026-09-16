//! Regression tests for immutable guest-root opens and Android link semantics.
use super::*;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

const ANDROID_ELOOP: i32 = 40;

unsafe extern "C" {
    fn darwin_art_bionic_errno_load() -> i32;
}

struct TestGuestRoot(PathBuf);

impl TestGuestRoot {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "darwin-art-immutable-open-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn facade(&self) -> Facade {
        Facade::new(File::open(&self.0).unwrap(), b"/", b"/").unwrap()
    }

    fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.0.join(relative)
    }
}

impl Drop for TestGuestRoot {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn read_guest_file(facade: &Facade, path: &[u8]) -> Vec<u8> {
    let fd = facade.open(path, O_RDONLY);
    assert!(fd >= 10_000, "guest open failed for {:?}: fd={fd}", path);
    let mut bytes = Vec::new();
    let mut chunk = [0_u8; 128];
    loop {
        // SAFETY: chunk remains writable for this synchronous facade call.
        let count = unsafe { facade.read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
        assert!(count >= 0, "guest read failed for {:?}", path);
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count as usize]);
    }
    assert_eq!(facade.close(fd), 0);
    bytes
}

fn read_guest_directory(facade: &Facade, path: &[u8]) -> Vec<Vec<u8>> {
    let directory = facade.opendir(path);
    assert!(
        !directory.is_null(),
        "guest directory open failed for {:?}",
        path
    );
    let mut entries = Vec::new();
    loop {
        let entry = facade.readdir(directory);
        if entry.is_null() {
            break;
        }
        // SAFETY: readdir returns the live stream entry until the next call;
        // copy its NUL-terminated name before advancing the stream.
        let entry = unsafe { &*entry };
        let name_length = entry
            .d_name
            .iter()
            .position(|byte| *byte == 0)
            .expect("translated dirent name is NUL terminated");
        entries.push(entry.d_name[..name_length].to_vec());
    }
    assert_eq!(facade.closedir(directory), 0);
    entries.sort();
    entries
}

#[test]
fn immutable_open_resolves_absolute_system_link_to_apex_target() {
    let root = TestGuestRoot::new();
    fs::create_dir_all(root.path("apex/com.android.runtime/lib64")).unwrap();
    fs::write(
        root.path("apex/com.android.runtime/lib64/libart.so"),
        b"apex-runtime",
    )
    .unwrap();
    symlink("/apex/com.android.runtime", root.path("system")).unwrap();

    let facade = root.facade();
    assert_eq!(
        read_guest_file(&facade, b"/system/lib64/libart.so"),
        b"apex-runtime"
    );
}

#[test]
fn immutable_open_resolves_relative_symlink_inside_guest_root() {
    let root = TestGuestRoot::new();
    fs::create_dir_all(root.path("system/bin")).unwrap();
    fs::create_dir_all(root.path("apex/com.android.runtime/bin")).unwrap();
    fs::write(
        root.path("apex/com.android.runtime/bin/runtime"),
        b"relative-target",
    )
    .unwrap();
    symlink(
        "../../apex/com.android.runtime/bin/runtime",
        root.path("system/bin/runtime"),
    )
    .unwrap();

    let facade = root.facade();
    assert_eq!(
        read_guest_file(&facade, b"/system/bin/runtime"),
        b"relative-target"
    );
}

#[test]
fn immutable_open_no_follow_rejects_final_guest_symlink() {
    let root = TestGuestRoot::new();
    fs::create_dir(root.path("system")).unwrap();
    fs::write(root.path("system/target"), b"target").unwrap();
    symlink("target", root.path("system/link")).unwrap();

    let facade = root.facade();
    assert_eq!(facade.open(b"/system/link", O_RDONLY | O_NOFOLLOW), -1);
    // SAFETY: the facade's errno provider is linked into this test binary.
    assert_eq!(unsafe { darwin_art_bionic_errno_load() }, ANDROID_ELOOP);
}

#[test]
fn immutable_open_applies_parent_after_symlink_and_rejects_host_absolute_escape() {
    let root = TestGuestRoot::new();
    fs::create_dir_all(root.path("real/dir")).unwrap();
    fs::write(root.path("real/target"), b"resolved-after-link").unwrap();
    fs::write(root.path("target"), b"lexical-parent-wrong").unwrap();
    symlink("/real/dir", root.path("alias")).unwrap();

    let outside = root.0.with_extension("outside");
    fs::write(&outside, b"host-escape").unwrap();
    symlink(outside.as_os_str(), root.path("escape")).unwrap();

    let facade = root.facade();
    assert_eq!(
        read_guest_file(&facade, b"/alias/../target"),
        b"resolved-after-link"
    );
    assert_eq!(facade.open(b"/escape", O_RDONLY), -1);
    assert_eq!(fs::read(&outside).unwrap(), b"host-escape");
    fs::remove_file(outside).unwrap();
}

#[test]
fn immutable_open_directory_streams_reopen_root_and_relative_dot_independently() {
    let root = TestGuestRoot::new();
    fs::write(root.path("alpha"), b"alpha").unwrap();
    fs::create_dir(root.path("nested")).unwrap();

    let facade = root.facade();
    let expected = vec![
        b".".to_vec(),
        b"..".to_vec(),
        b"alpha".to_vec(),
        b"nested".to_vec(),
    ];
    assert_eq!(read_guest_directory(&facade, b"/"), expected);
    assert_eq!(read_guest_directory(&facade, b"."), expected);
}

#[test]
fn mounted_data_directory_streams_have_independent_offsets() {
    let root = TestGuestRoot::new();
    fs::create_dir(root.path("guest")).unwrap();
    fs::create_dir(root.path("data")).unwrap();
    fs::write(root.path("data/value"), b"mounted").unwrap();
    let private = PrivateDataRoot::open(root.path("data")).unwrap();
    let namespace = filesystem_namespace::FilesystemNamespace::new(
        File::open(root.path("guest")).unwrap(),
        b"/",
        b"/data",
        Some(&private),
    )
    .unwrap();
    let mut facade = Facade::new(File::open(root.path("guest")).unwrap(), b"/", b"/").unwrap();
    facade.namespace = namespace;
    facade.private_root = Some(private);
    let expected = vec![b".".to_vec(), b"..".to_vec(), b"value".to_vec()];
    for path in [b"/data".as_slice(), b"/data", b".", b"."] {
        assert_eq!(read_guest_directory(&facade, path), expected);
    }
    // Direct virtual mount opens also require independent offsets, even when
    // the facade's writable-open route was not involved.
    for _ in 0..2 {
        let file = facade
            .namespace
            .guest_root
            .as_ref()
            .unwrap()
            .open(b"/data")
            .unwrap()
            .node
            .into_file();
        let fd = facade
            .descriptors
            .lock()
            .unwrap()
            .insert(Descriptor::File(file))
            .unwrap();
        let stream = facade.fdopendir(fd);
        assert!(!stream.is_null());
        let mut count = 0;
        while !facade.readdir(stream).is_null() {
            count += 1;
        }
        assert_eq!(count, 3);
        assert_eq!(facade.closedir(stream), 0);
    }
}
