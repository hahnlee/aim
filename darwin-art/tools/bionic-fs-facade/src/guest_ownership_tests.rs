//! Android owner and group of private `/data` nodes through actual Facade
//! operations, with trusted credentials supplied directly and no launcher.
use super::guest_ownership::{GuestOwner, ProcessCredentials};
use super::*;
use std::os::unix::fs::PermissionsExt;

const ANDROID_EPERM: i32 = 1;
const APP_UID: u32 = 10_123;
const SYSTEM_UID: u32 = 1000;
const PACKAGE_INFO_GID: u32 = 1032;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let nonce = crate::test_nonce();
        let path = std::env::temp_dir().join(format!(
            "darwin-guest-ownership-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        for name in ["root", "data", "data/system"] {
            fs::create_dir(path.join(name)).unwrap();
        }
        fs::write(path.join("root/immutable"), b"image").unwrap();
        Self(path)
    }

    fn facade(&self, uid: Option<u32>) -> Facade {
        let mut private = PrivateDataRoot::open(self.0.join("data")).unwrap();
        let namespace = filesystem_namespace::FilesystemNamespace::with_storage(
            File::open(self.0.join("root")).unwrap(),
            b"/",
            b"/",
            Some(&private),
            None,
            None,
            false,
        )
        .unwrap();
        private
            .attach_guest_root(namespace.guest_root.as_ref().unwrap().clone())
            .unwrap();
        let mut facade = Facade::new(File::open(self.0.join("root")).unwrap(), b"/", b"/").unwrap();
        facade.namespace = namespace;
        facade.private_root = Some(private);
        facade.credentials = uid.map(ProcessCredentials::for_uid);
        facade
    }

    fn create(&self, facade: &Facade, path: &[u8]) -> c_int {
        let fd = facade.open_with_mode(path, O_CREAT | O_RDWR, 0o640);
        assert!(fd >= 0);
        fd
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn path_owner(facade: &Facade, path: &[u8]) -> (u32, u32) {
    let mut status = AndroidStat::default();
    assert_eq!(unsafe { facade.stat(path, &mut status, false) }, 0);
    (status.st_uid, status.st_gid)
}

fn fd_owner(facade: &Facade, fd: c_int) -> (u32, u32) {
    let mut status = AndroidStat::default();
    assert_eq!(unsafe { facade.fstat(fd, &mut status) }, 0);
    (status.st_uid, status.st_gid)
}

fn errno() -> i32 {
    unsafe { darwin_art_bionic_errno_load() }
}

#[test]
fn unowned_private_nodes_belong_to_the_process_uid() {
    let f = Fixture::new();
    let facade = f.facade(Some(APP_UID));
    let fd = f.create(&facade, b"/data/value");
    assert_eq!(path_owner(&facade, b"/data/value"), (APP_UID, APP_UID));
    assert_eq!(path_owner(&facade, b"/data/system"), (APP_UID, APP_UID));
    assert_eq!(fd_owner(&facade, fd), (APP_UID, APP_UID));
    let mut status = AndroidStat::default();
    assert_eq!(
        unsafe { facade.fstatat(AT_FDCWD, b"/data/value", &mut status, 0) },
        0
    );
    assert_eq!((status.st_uid, status.st_gid), (APP_UID, APP_UID));
    assert_eq!(facade.close(fd), 0);

    // The immutable image keeps the metadata it reported before.
    let host = fs::metadata(f.0.join("root/immutable")).unwrap();
    assert_eq!(path_owner(&facade, b"/immutable"), (host.uid(), host.gid()));
}

#[test]
fn system_chown_is_stored_and_reported_to_every_process() {
    let f = Fixture::new();
    let system = f.facade(Some(SYSTEM_UID));
    let fd = f.create(&system, b"/data/system/packages.list");
    assert_eq!(system.fchown(fd, SYSTEM_UID, PACKAGE_INFO_GID), 0);
    assert_eq!(fd_owner(&system, fd), (SYSTEM_UID, PACKAGE_INFO_GID));
    assert_eq!(
        path_owner(&system, b"/data/system/packages.list"),
        (SYSTEM_UID, PACKAGE_INFO_GID)
    );
    // CAP_CHOWN hands a node to an app.
    assert_eq!(system.fchown(fd, APP_UID, u32::MAX), 0);
    assert_eq!(fd_owner(&system, fd), (APP_UID, PACKAGE_INFO_GID));
    assert_eq!(system.close(fd), 0);

    // Another process reads the stored owner, not its own default.
    let app = f.facade(Some(APP_UID));
    assert_eq!(
        path_owner(&app, b"/data/system/packages.list"),
        (APP_UID, PACKAGE_INFO_GID)
    );
    // The owner record is host-private: the guest sees no user attribute.
    let mut names = [0_u8; 64];
    assert_eq!(
        app.list_xattr(b"/data/system/packages.list", &mut names, false),
        0
    );
    assert_eq!(
        app.get_xattr(
            b"/data/system/packages.list",
            b"dev.darwinart.owner",
            &mut names,
            false
        ),
        -1
    );
}

#[test]
fn app_may_only_choose_its_own_groups() {
    let f = Fixture::new();
    let app = f.facade(Some(APP_UID));
    let fd = f.create(&app, b"/data/value");
    assert_eq!(app.fchown(fd, SYSTEM_UID, u32::MAX), -1);
    assert_eq!(errno(), ANDROID_EPERM);
    assert_eq!(app.fchown(fd, u32::MAX, PACKAGE_INFO_GID), -1);
    assert_eq!(errno(), ANDROID_EPERM);
    assert_eq!(fd_owner(&app, fd), (APP_UID, APP_UID));
    // Restating its own uid and gid is allowed and stored.
    assert_eq!(app.fchown(fd, APP_UID, APP_UID), 0);
    assert_eq!(app.close(fd), 0);

    // A node the system server owns is not the app's to change.
    let system = f.facade(Some(SYSTEM_UID));
    let fd = f.create(&system, b"/data/system/owned");
    assert_eq!(system.fchown(fd, SYSTEM_UID, SYSTEM_UID), 0);
    assert_eq!(system.close(fd), 0);
    let fd = app.open(b"/data/system/owned", O_RDONLY);
    assert!(fd >= 0);
    assert_eq!(app.fchown(fd, u32::MAX, APP_UID), -1);
    assert_eq!(errno(), ANDROID_EPERM);
    assert_eq!(fd_owner(&app, fd), (SYSTEM_UID, SYSTEM_UID));
    assert_eq!(app.close(fd), 0);
}

#[test]
fn chown_clears_set_id_bits_as_linux_does() {
    let f = Fixture::new();
    let system = f.facade(Some(SYSTEM_UID));
    let fd = f.create(&system, b"/data/tool");
    let host = f.0.join("data/tool");
    fs::set_permissions(&host, fs::Permissions::from_mode(0o6750)).unwrap();
    assert_eq!(system.fchown(fd, APP_UID, APP_UID), 0);
    assert_eq!(fs::metadata(&host).unwrap().mode() & 0o7777, 0o750);
    assert_eq!(system.close(fd), 0);

    // An owner in the inode's group keeps a non-executable S_ISGID.
    let app = f.facade(Some(APP_UID));
    fs::set_permissions(&host, fs::Permissions::from_mode(0o6640)).unwrap();
    let fd = app.open(b"/data/tool", O_RDONLY);
    assert!(fd >= 0);
    assert_eq!(app.fchown(fd, u32::MAX, u32::MAX), 0);
    assert_eq!(fs::metadata(&host).unwrap().mode() & 0o7777, 0o2640);
    assert_eq!(app.close(fd), 0);
}

#[test]
fn without_trusted_credentials_host_owner_and_read_only_ownership_remain() {
    let f = Fixture::new();
    let facade = f.facade(None);
    let fd = f.create(&facade, b"/data/value");
    let host = fs::metadata(f.0.join("data/value")).unwrap();
    assert_eq!(fd_owner(&facade, fd), (host.uid(), host.gid()));
    assert_eq!(
        path_owner(&facade, b"/data/value"),
        (host.uid(), host.gid())
    );
    assert_eq!(facade.fchown(fd, SYSTEM_UID, SYSTEM_UID), -1);
    assert_eq!(errno(), ANDROID_EROFS);
    assert_eq!(facade.close(fd), 0);
}

#[test]
fn overlay_nodes_store_the_authorized_owner() {
    let mut facade = Facade::new(File::open("/").unwrap(), b"/", b"/").unwrap();
    facade.credentials = Some(ProcessCredentials::for_uid(SYSTEM_UID));
    let node = Arc::new(Mutex::new(OverlayFile {
        inode: 99,
        mode: ANDROID_S_IFREG | 0o4750,
        data: Vec::new(),
        owner: facade.overlay_creator(),
    }));
    let fd = facade
        .descriptors
        .lock()
        .unwrap()
        .insert_with_flags(
            Descriptor::Overlay(OverlayDescriptor {
                node: node.clone(),
                offset: 0,
                readable: true,
                writable: true,
            }),
            false,
        )
        .unwrap();
    assert_eq!(fd_owner(&facade, fd), (SYSTEM_UID, SYSTEM_UID));
    assert_eq!(facade.fchown(fd, APP_UID, PACKAGE_INFO_GID), 0);
    assert_eq!(fd_owner(&facade, fd), (APP_UID, PACKAGE_INFO_GID));
    assert_eq!(
        node.lock().unwrap().owner,
        Some(GuestOwner {
            uid: APP_UID,
            gid: PACKAGE_INFO_GID
        })
    );
    assert_eq!(node.lock().unwrap().mode, ANDROID_S_IFREG | 0o750);

    facade.credentials = Some(ProcessCredentials::for_uid(APP_UID));
    assert_eq!(facade.fchown(fd, SYSTEM_UID, u32::MAX), -1);
    assert_eq!(errno(), ANDROID_EPERM);
    assert_eq!(facade.close(fd), 0);
}
