//! Integration checks for the facade's directory authority, not APK acceptance.
use super::*;
use std::os::unix::fs::MetadataExt;

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let nonce = crate::test_nonce();
        let path = std::env::temp_dir().join(format!(
            "darwin-private-authority-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        // Only the unique directory created by this test is removed.
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn data_open_keeps_original_directory_after_host_path_replacement() {
    let temporary = TestDirectory::new();
    let original = temporary.0.join("root");
    let moved = temporary.0.join("retained");
    fs::create_dir(&original).unwrap();
    fs::write(original.join("value"), b"original").unwrap();
    let mut facade = Facade::new(File::open("/").unwrap(), b"/", b"/").unwrap();
    facade.private_root = Some(private_data::PrivateDataRoot::open(original.clone()).unwrap());

    fs::rename(&original, &moved).unwrap();
    fs::create_dir(&original).unwrap();
    fs::write(original.join("value"), b"replaced").unwrap();

    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(moved.join("value"), fs::Permissions::from_mode(0)).unwrap();
    fs::set_permissions(original.join("value"), fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(facade.chmod(b"/data/value", 0o640), 0);
    assert_eq!(
        fs::metadata(moved.join("value")).unwrap().mode() & 0o777,
        0o640
    );
    assert_eq!(
        fs::metadata(original.join("value")).unwrap().mode() & 0o777,
        0o600
    );

    let retained = facade
        .open_private_relative(b"", O_RDONLY | O_CLOEXEC | O_DIRECTORY, 0)
        .unwrap();
    assert_eq!(
        retained.metadata().unwrap().ino(),
        fs::metadata(&moved).unwrap().ino()
    );
    assert_ne!(
        retained.metadata().unwrap().ino(),
        fs::metadata(&original).unwrap().ino()
    );
    assert!(
        facade
            .open_private_relative(b"", O_WRONLY | O_TRUNC, 0)
            .is_err()
    );

    let fd = facade.open(b"/data/value", O_RDONLY);
    assert!(fd >= 0);
    let mut bytes = [0_u8; 8];
    // SAFETY: bytes is writable for its exact capacity and fd is live.
    assert_eq!(
        unsafe { facade.read(fd, bytes.as_mut_ptr().cast(), bytes.len()) },
        8
    );
    assert_eq!(&bytes, b"original");
    assert_eq!(facade.close(fd), 0);

    assert_eq!(facade.truncate_path(b"/data/value", 3), 0);
    assert_eq!(fs::read(moved.join("value")).unwrap(), b"ori");
    assert_eq!(fs::read(original.join("value")).unwrap(), b"replaced");
    assert_eq!(facade.mkdir(b"/data/created", 0o700), 0);
    assert!(moved.join("created").is_dir());
    assert!(!original.join("created").exists());
    assert_eq!(facade.rename_path(b"/data/value", b"/data/renamed"), 0);
    assert_eq!(fs::read(moved.join("renamed")).unwrap(), b"ori");
    assert_eq!(facade.remove_path(b"/data/renamed"), 0);
    assert_eq!(facade.remove_path(b"/data/created"), 0);
    assert!(original.join("value").is_file());
    assert_eq!(
        facade.seed_private_directory(b"/data/user/0/example/files"),
        0
    );
    assert_eq!(
        facade.seed_private_directory(b"/data/user/0/example/files"),
        0
    );
    assert!(moved.join("user/0/example/files").is_dir());
    assert!(!original.join("user").exists());
}

#[test]
fn private_mutations_do_not_follow_host_symlink_parents() {
    let temporary = TestDirectory::new();
    let data = temporary.0.join("data");
    let outside = temporary.0.join("outside");
    fs::create_dir(&data).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret"), b"unchanged").unwrap();
    fs::write(data.join("source"), b"owned").unwrap();
    std::os::unix::fs::symlink(&outside, data.join("escape")).unwrap();
    let mut facade = Facade::new(File::open("/").unwrap(), b"/", b"/").unwrap();
    facade.private_root = Some(private_data::PrivateDataRoot::open(data.clone()).unwrap());
    let original_mode = fs::metadata(outside.join("secret")).unwrap().mode();
    assert_eq!(facade.chmod(b"/data/escape/secret", 0o777), -1);
    assert_eq!(
        fs::metadata(outside.join("secret")).unwrap().mode(),
        original_mode
    );
    assert_eq!(facade.mkdir(b"/data/escape/new", 0o700), -1);
    assert_eq!(facade.seed_private_directory(b"/data/escape/new/deep"), -1);
    assert_eq!(facade.truncate_path(b"/data/escape/secret", 0), -1);
    assert_eq!(facade.remove_path(b"/data/escape/secret"), -1);
    assert_eq!(
        facade.rename_path(b"/data/source", b"/data/escape/secret"),
        -1
    );
    assert_eq!(
        facade.rename_path(b"/data/escape/secret", b"/data/stolen"),
        -1
    );
    assert_eq!(fs::read(outside.join("secret")).unwrap(), b"unchanged");
    assert!(!outside.join("new").exists());
    assert_eq!(fs::read(data.join("source")).unwrap(), b"owned");
    assert_eq!(facade.remove_path(b"/data/escape"), 0);
    assert!(outside.is_dir());
}

#[test]
fn configured_mutations_follow_guest_links_and_enforce_mount_permissions() {
    use darwin_art_fs_broker::guest_path::GuestRoot;
    let temporary = TestDirectory::new();
    let system = temporary.0.join("guest");
    let data = temporary.0.join("data");
    fs::create_dir_all(system.join("system")).unwrap();
    fs::create_dir_all(data.join("real/nested")).unwrap();
    fs::write(data.join("real/source"), b"owned").unwrap();
    fs::write(system.join("system/immutable"), b"unchanged").unwrap();
    std::os::unix::fs::symlink("/data/real", data.join("absolute")).unwrap();
    std::os::unix::fs::symlink("real", data.join("relative")).unwrap();
    std::os::unix::fs::symlink("real/nested", data.join("deep")).unwrap();
    std::os::unix::fs::symlink("/system", data.join("system-link")).unwrap();
    let mut guest = GuestRoot::from_directory(File::open(&system).unwrap()).unwrap();
    guest
        .mount_directory(b"/data", File::open(&data).unwrap())
        .unwrap();
    let guest = Arc::new(guest);
    let mut private = private_data::PrivateDataRoot::open(data.clone()).unwrap();
    private.attach_guest_root(guest.clone()).unwrap();
    let mut facade = Facade::new(File::open(&system).unwrap(), b"/", b"/").unwrap();
    facade.namespace.guest_root = Some(guest);
    facade.private_root = Some(private);
    assert_eq!(facade.chmod(b"/data/absolute/source", 0o640), 0);
    assert_eq!(
        fs::metadata(data.join("real/source")).unwrap().mode() & 0o777,
        0o640
    );
    assert_eq!(facade.chmod(b"/data/system-link/immutable", 0o777), -1);
    fs::write(data.join("real/open-source"), b"runtime-path").unwrap();
    std::os::unix::fs::symlink("/data/real/open-source", data.join("final-file")).unwrap();
    let opened = facade.open(b"/data/deep/../open-source", O_RDONLY);
    assert!(opened >= 10_000);
    let mut bytes = [0u8; 32];
    assert_eq!(
        unsafe { facade.read(opened, bytes.as_mut_ptr().cast(), bytes.len()) },
        12
    );
    assert_eq!(&bytes[..12], b"runtime-path");
    assert_eq!(facade.close(opened), 0);
    assert_eq!(facade.truncate_path(b"/data/final-file", 7), 0);
    assert_eq!(fs::read(data.join("real/open-source")).unwrap(), b"runtime");
    assert_eq!(facade.open(b"/data/final-file", O_RDONLY | O_NOFOLLOW), -1);
    let directory = facade.open(b"/data/absolute/", O_RDONLY | O_DIRECTORY | O_NOFOLLOW);
    assert!(directory >= 10_000);
    assert_eq!(facade.close(directory), 0);
    assert_eq!(facade.mkdir(b"/data/absolute/new", 0o700), 0);
    assert!(data.join("real/new").is_dir());
    assert_eq!(
        facade.rename_path(b"/data/relative/source", b"/data/absolute/renamed"),
        0
    );
    assert_eq!(facade.remove_path(b"/data/relative/renamed"), 0);
    // Resolve the link before '..', not lexical normalization of the input.
    assert_eq!(facade.mkdir(b"/data/deep/../sibling", 0o700), 0);
    assert!(data.join("real/sibling").is_dir());
    assert!(!data.join("sibling").exists());
    assert_eq!(facade.mkdir(b"/data/system-link/new", 0o700), -1);
    assert_eq!(facade.remove_path(b"/data/system-link/immutable"), -1);
    assert_eq!(
        facade.rename_path(b"/data/absolute/new", b"/data/system-link/new"),
        -1
    );
    assert_eq!(
        fs::read(system.join("system/immutable")).unwrap(),
        b"unchanged"
    );
    assert!(!system.join("system/new").exists());
    assert_eq!(
        facade.seed_private_directory(b"/data/absolute/seeded/deep"),
        0
    );
    assert!(data.join("real/seeded/deep").is_dir());
    assert_eq!(
        facade.seed_private_directory(b"/data/system-link/new/deep"),
        -1
    );
    assert!(!system.join("system/new").exists());
    fs::write(data.join("file-not-directory"), b"keep").unwrap();
    assert_eq!(
        facade.seed_private_directory(b"/data/file-not-directory/deep"),
        -1
    );
    assert_eq!(fs::read(data.join("file-not-directory")).unwrap(), b"keep");
    let mut wrong = private_data::PrivateDataRoot::open(system.clone()).unwrap();
    assert!(
        wrong
            .attach_guest_root(facade.namespace.guest_root.as_ref().unwrap().clone())
            .is_err()
    );

    let retained = temporary.0.join("retained-data");
    fs::rename(&data, &retained).unwrap();
    fs::create_dir_all(data.join("real")).unwrap();
    assert_eq!(facade.mkdir(b"/data/absolute/retained", 0o700), 0);
    assert!(retained.join("real/retained").is_dir());
    assert!(!data.join("real/retained").exists());
}

#[test]
fn private_data_keeps_linux_user_extended_attributes() {
    let temporary = TestDirectory::new();
    let root = temporary.0.join("root");
    fs::create_dir_all(root.join("user/0")).unwrap();
    let mut facade = Facade::new(File::open("/").unwrap(), b"/", b"/").unwrap();
    facade.private_root = Some(private_data::PrivateDataRoot::open(root.clone()).unwrap());
    let errno = || Facade::android_errno();

    // UserDataPreparer: a missing user.serial is ENODATA, then it is set.
    let mut value = [0u8; 16];
    assert_eq!(
        facade.get_xattr(b"/data/user/0", b"user.serial", &mut value, false),
        -1
    );
    assert_eq!(errno(), 61);
    assert_eq!(
        facade.set_xattr(b"/data/user/0", b"user.serial", b"0", 1, false),
        0
    );
    assert_eq!(
        facade.set_xattr(b"/data/user/0", b"user.serial", b"1", 1, false),
        -1
    );
    assert_eq!(errno(), 17); // XATTR_CREATE on an existing attribute
    assert_eq!(
        facade.get_xattr(b"/data/user/0", b"user.serial", &mut [], false),
        1
    );
    assert_eq!(
        facade.get_xattr(b"/data/user/0", b"user.serial", &mut value, false),
        1
    );
    assert_eq!(&value[..1], b"0");
    let mut names = [0u8; 64];
    let listed = facade.list_xattr(b"/data/user/0", &mut names, false);
    assert_eq!(&names[..listed as usize], b"user.serial\0");
    assert_eq!(
        facade.remove_xattr(b"/data/user/0", b"user.serial", false),
        0
    );
    assert_eq!(
        facade.get_xattr(b"/data/user/0", b"user.serial", &mut value, false),
        -1
    );
    assert_eq!(errno(), 61);

    // Only the user namespace exists; a missing path is ENOENT.
    assert_eq!(
        facade.set_xattr(b"/data/user/0", b"security.selinux", b"x", 0, false),
        -1
    );
    assert_eq!(errno(), 95);
    assert_eq!(
        facade.get_xattr(b"/data/user/9", b"user.serial", &mut value, false),
        -1
    );
    assert_eq!(errno(), 2);
}
