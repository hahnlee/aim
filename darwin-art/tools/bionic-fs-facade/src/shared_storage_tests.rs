//! Shared-storage dispatch through actual Facade operations, with retained
//! immutable/data/storage authorities and no launcher or APK fixtures.
use super::*;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let nonce = crate::test_nonce();
        let path = std::env::temp_dir().join(format!(
            "darwin-shared-storage-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        for name in ["root", "data", "storage"] {
            fs::create_dir(path.join(name)).unwrap();
        }
        fs::create_dir(path.join("root/storage")).unwrap();
        fs::write(path.join("root/immutable"), b"unchanged").unwrap();
        Self(path)
    }
    fn facade(&self) -> Facade {
        let mut private = PrivateDataRoot::open(self.0.join("data")).unwrap();
        let mut storage =
            writable_mount::WritableMount::open(self.0.join("storage"), b"/storage").unwrap();
        let namespace = filesystem_namespace::FilesystemNamespace::with_storage(
            File::open(self.0.join("root")).unwrap(),
            b"/",
            b"/",
            Some(&private),
            Some(&storage),
            None,
            false,
        )
        .unwrap();
        let guest = namespace.guest_root.as_ref().unwrap();
        private.attach_guest_root(guest.clone()).unwrap();
        storage.attach_guest_root(guest.clone()).unwrap();
        let mut facade = Facade::new(File::open(self.0.join("root")).unwrap(), b"/", b"/").unwrap();
        facade.namespace = namespace;
        facade.private_root = Some(private);
        facade.storage_root = Some(storage);
        facade
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn shared_storage_fd_and_path_operations_use_one_retained_mount() {
    let f = Fixture::new();
    let facade = f.facade();
    fs::rename(f.0.join("storage"), f.0.join("retained")).unwrap();
    fs::create_dir(f.0.join("storage")).unwrap();
    assert_eq!(facade.mkdir(b"/storage/emulated", 0o700), 0);
    assert_eq!(facade.mkdir(b"/storage/emulated/0", 0o700), 0);
    assert_eq!(facade.chdir(b"/storage/emulated/0"), 0);
    let fd = facade.open(b"original", O_CREAT | O_EXCL | O_RDWR);
    assert!(fd >= 10_000);
    assert_eq!(
        unsafe { facade.write(fd, b"payload".as_ptr().cast(), 7) },
        7
    );
    assert_eq!(facade.close(fd), 0);
    assert_eq!(facade.rename_path(b"./original", b"renamed"), 0);
    assert_eq!(facade.truncate_path(b"renamed", 4), 0);
    let mut status = AndroidStat::default();
    assert_eq!(unsafe { facade.stat(b"renamed", &mut status, false) }, 0);
    assert_eq!(status.st_size, 4);
    assert_eq!(
        fs::read(f.0.join("retained/emulated/0/renamed")).unwrap(),
        b"payl"
    );
    let mut fs_status = AndroidStatvfs::default();
    assert_eq!(unsafe { facade.statvfs(b".", &mut fs_status) }, 0);
    assert!(facade.pathconf(b".", PATHCONF_MIN) >= 0);
    for _ in 0..2 {
        let stream = facade.opendir(b".");
        assert!(!stream.is_null());
        let mut entries = 0;
        while !facade.readdir(stream).is_null() {
            entries += 1;
        }
        assert_eq!(entries, 3);
        assert_eq!(facade.closedir(stream), 0);
    }
    assert_eq!(facade.rename_path(b"renamed", b"/data/moved"), -1);
    assert!(!f.0.join("data/moved").exists());
    assert!(f.0.join("retained/emulated/0/renamed").is_file());
    assert_eq!(facade.remove_path(b"renamed"), 0);
    assert_eq!(facade.open(b"/immutable", O_WRONLY | O_TRUNC), -1);
    assert_eq!(fs::read(f.0.join("root/immutable")).unwrap(), b"unchanged");
    assert_eq!(fs::read_dir(f.0.join("storage")).unwrap().count(), 0);
    assert_eq!(fs::read_dir(f.0.join("root/storage")).unwrap().count(), 0);
}

#[test]
fn shared_mount_requires_complete_namespace_and_retains_cwd_authority() {
    let f = Fixture::new();
    let storage = writable_mount::WritableMount::open(f.0.join("storage"), b"/storage").unwrap();
    assert!(
        filesystem_namespace::FilesystemNamespace::with_storage(
            File::open(f.0.join("root")).unwrap(),
            b"/system",
            b"/system",
            None,
            Some(&storage),
            None,
            false,
        )
        .is_err()
    );
    let namespace = filesystem_namespace::FilesystemNamespace::with_storage(
        File::open(f.0.join("root")).unwrap(),
        b"/",
        b"/storage",
        None,
        Some(&storage),
        None,
        false,
    )
    .unwrap();
    assert_eq!(namespace.cwd.snapshot().unwrap(), b"/storage");
    assert_eq!(
        namespace.prefix.resolve(b"/", b"/storage").unwrap().kind,
        MountKind::Shared
    );
}

#[test]
fn configured_shared_storage_is_installed_through_process_abi() {
    let f = Fixture::new();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "shared_storage_tests::configured_storage_child",
            "--ignored",
            "--nocapture",
        ])
        .env("DARWIN_ART_STORAGE_TEST_ROOT", &f.0)
        .env("DARWIN_ART_ANDROID_PRIVATE_DATA_ROOT", f.0.join("data"))
        .env(
            "DARWIN_ART_ANDROID_SHARED_STORAGE_ROOT",
            f.0.join("storage"),
        )
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(fs::read(f.0.join("storage/abi-value")).unwrap(), b"ABI");
    assert!(!f.0.join("root/storage/abi-value").exists());
}

#[test]
#[ignore = "spawned by configured_shared_storage_is_installed_through_process_abi"]
fn configured_storage_child() {
    let path =
        PathBuf::from(std::env::var_os("DARWIN_ART_STORAGE_TEST_ROOT").expect("isolated fixture"));
    let root = File::open(path.join("root")).unwrap();
    assert_eq!(
        unsafe {
            darwin_art_bionic_fs_process_install(
                root.as_raw_fd(),
                b"/".as_ptr(),
                1,
                b"/".as_ptr(),
                1,
            )
        },
        PROCESS_OWNER_OK
    );
    drop(root);
    let fd = with_active(-1, |facade| {
        facade.open_with_mode(b"/storage/abi-value", O_CREAT | O_EXCL | O_RDWR, 0o600)
    });
    assert!(fd >= 10_000);
    assert_eq!(
        with_active(-1, |facade| unsafe {
            facade.write(fd, b"ABI".as_ptr().cast(), 3)
        }),
        3
    );
    assert_eq!(with_active(-1, |facade| facade.close(fd)), 0);
    assert_eq!(darwin_art_bionic_fs_process_uninstall(), PROCESS_OWNER_OK);
    assert_eq!(with_active(-1, |_| 0), -1);
}

#[test]
fn writable_package_root_creates_install_session_directories() {
    let f = Fixture::new();
    fs::create_dir(f.0.join("packages")).unwrap();
    let mut private = PrivateDataRoot::open(f.0.join("data")).unwrap();
    let mut packages =
        writable_mount::WritableMount::open(f.0.join("packages"), b"/data/app").unwrap();
    let package_directory = packages.directory().try_clone().unwrap();
    let namespace = filesystem_namespace::FilesystemNamespace::with_storage(
        File::open(f.0.join("root")).unwrap(),
        b"/",
        b"/",
        Some(&private),
        None,
        Some(&package_directory),
        true,
    )
    .unwrap();
    let guest = namespace.guest_root.as_ref().unwrap();
    private.attach_guest_root(guest.clone()).unwrap();
    packages.attach_guest_root(guest.clone()).unwrap();
    let mut facade = Facade::new(File::open(f.0.join("root")).unwrap(), b"/", b"/").unwrap();
    facade.namespace = namespace;
    facade.private_root = Some(private);
    facade.package_root = Some(packages);
    assert_eq!(
        facade.mkdir(b"/data/app/vmdl1.tmp", 0o775),
        0,
        "errno {}",
        Facade::android_errno()
    );
    assert!(f.0.join("packages/vmdl1.tmp").is_dir());
    // PackageInstallerSession names its staged file through /proc/self/fd.
    let fd = facade.open(b"/data/app/vmdl1.tmp/base.apk", O_CREAT | O_EXCL | O_WRONLY);
    assert!(fd >= 10_000, "errno {}", Facade::android_errno());
    let mut target = [0 as c_char; 256];
    let link = format!("/proc/self/fd/{fd}");
    let length = facade.readlink(link.as_bytes(), target.as_mut_ptr(), target.len());
    let target: Vec<u8> = target[..length.max(0) as usize]
        .iter()
        .map(|b| *b as u8)
        .collect();
    assert_eq!(target, b"/data/app/vmdl1.tmp/base.apk");
    assert_eq!(facade.close(fd), 0);
    assert_eq!(
        facade.readlink(link.as_bytes(), [0 as c_char; 8].as_mut_ptr(), 8),
        -1
    );
    assert_eq!(Facade::android_errno(), ANDROID_ENOENT);
}
