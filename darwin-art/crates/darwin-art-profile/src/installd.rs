//! Host side of the Darwin `installd` provider.
//!
//! Android's installd owns the filesystem work PackageManagerService asks
//! for: per-app data directories and installed code directories. Here each
//! application's `/data` is its own host tree (`data/apps/<package>/
//! private-data`) and installed code lives in the profile package store
//! (`packages`, the guest `/data/app`). The system process reaches these
//! operations only through the daemon, authenticated as `android.system`.
//! Policy (which package, when) stays in PackageManagerService.

use crate::registry::validate_package;
use crate::{ProfileError, ProfilePaths};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

// android.os.storage.StorageManager / IInstalld flags.
pub(crate) const FLAG_STORAGE_DE: u32 = 0x1;
pub(crate) const FLAG_STORAGE_CE: u32 = 0x2;
pub(crate) const FLAG_CLEAR_CACHE_ONLY: u32 = 0x10;
pub(crate) const FLAG_CLEAR_CODE_CACHE_ONLY: u32 = 0x20;

// Command 1 was createAppData without the app ID; a stale peer that still
// sends it is rejected rather than misread.
const COMMAND_CREATE_APP_DATA: u8 = 5;
const COMMAND_DESTROY_APP_DATA: u8 = 2;
const COMMAND_CLEAR_APP_DATA: u8 = 3;
const COMMAND_RM_PACKAGE_DIR: u8 = 4;

/// Process.SYSTEM_UID. Packages that share it run in, and keep their data
/// with, the system process (android:sharedUserId="android.uid.system").
pub const SYSTEM_APP_ID: u32 = 1000;
/// The profile tree whose `private-data` is the system process's `/data`.
const SYSTEM_DATA_OWNER: &str = "android.system";

/// One installd request, decoded from the daemon wire payload.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    CreateAppData {
        package: String,
        user: u32,
        flags: u32,
        /// CreateAppDataArgs.appId: selects whose `/data` holds the package.
        app_id: u32,
    },
    DestroyAppData {
        package: String,
        user: u32,
        flags: u32,
    },
    ClearAppData {
        package: String,
        user: u32,
        flags: u32,
    },
    /// `code_path` is the guest path under `/data/app`.
    RmPackageDir { code_path: String },
}

impl Request {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        match self {
            Request::CreateAppData {
                package,
                user,
                flags,
                app_id,
            } => {
                out.push(COMMAND_CREATE_APP_DATA);
                out.extend_from_slice(&user.to_le_bytes());
                out.extend_from_slice(&flags.to_le_bytes());
                out.extend_from_slice(&app_id.to_le_bytes());
                out.extend_from_slice(package.as_bytes());
            }
            Request::DestroyAppData {
                package,
                user,
                flags,
            }
            | Request::ClearAppData {
                package,
                user,
                flags,
            } => {
                out.push(match self {
                    Request::DestroyAppData { .. } => COMMAND_DESTROY_APP_DATA,
                    _ => COMMAND_CLEAR_APP_DATA,
                });
                out.extend_from_slice(&user.to_le_bytes());
                out.extend_from_slice(&flags.to_le_bytes());
                out.extend_from_slice(package.as_bytes());
            }
            Request::RmPackageDir { code_path } => {
                out.push(COMMAND_RM_PACKAGE_DIR);
                out.extend_from_slice(code_path.as_bytes());
            }
        }
        out
    }

    pub(crate) fn decode(payload: &[u8]) -> Result<Self, ProfileError> {
        let invalid = || ProfileError::Daemon("malformed installd request".into());
        let (&command, rest) = payload.split_first().ok_or_else(invalid)?;
        let text = |bytes: &[u8]| {
            std::str::from_utf8(bytes)
                .map(str::to_owned)
                .map_err(|_| invalid())
        };
        match command {
            COMMAND_CREATE_APP_DATA => {
                if rest.len() < 12 {
                    return Err(invalid());
                }
                Ok(Request::CreateAppData {
                    user: u32::from_le_bytes(rest[..4].try_into().unwrap()),
                    flags: u32::from_le_bytes(rest[4..8].try_into().unwrap()),
                    app_id: u32::from_le_bytes(rest[8..12].try_into().unwrap()),
                    package: text(&rest[12..])?,
                })
            }
            COMMAND_DESTROY_APP_DATA | COMMAND_CLEAR_APP_DATA => {
                if rest.len() < 8 {
                    return Err(invalid());
                }
                let user = u32::from_le_bytes(rest[..4].try_into().unwrap());
                let flags = u32::from_le_bytes(rest[4..8].try_into().unwrap());
                let package = text(&rest[8..])?;
                Ok(match command {
                    COMMAND_DESTROY_APP_DATA => Request::DestroyAppData {
                        package,
                        user,
                        flags,
                    },
                    _ => Request::ClearAppData {
                        package,
                        user,
                        flags,
                    },
                })
            }
            COMMAND_RM_PACKAGE_DIR => Ok(Request::RmPackageDir {
                code_path: text(rest)?,
            }),
            _ => Err(invalid()),
        }
    }
}

/// CreateAppDataResult: the CE and DE directory inodes (0 when not created).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct AppDataInodes {
    pub(crate) ce: u64,
    pub(crate) de: u64,
}

impl AppDataInodes {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = self.ce.to_le_bytes().to_vec();
        out.extend_from_slice(&self.de.to_le_bytes());
        out
    }
}

pub(crate) struct Installd<'a> {
    paths: &'a ProfilePaths,
}

impl<'a> Installd<'a> {
    pub(crate) fn new(paths: &'a ProfilePaths) -> Self {
        Self { paths }
    }

    pub(crate) fn execute(&self, request: &Request) -> Result<Vec<u8>, ProfileError> {
        match request {
            Request::CreateAppData {
                package,
                user,
                flags,
                app_id,
            } => self
                .create_app_data(package, *user, *flags, *app_id)
                .map(|inodes| inodes.encode()),
            Request::DestroyAppData {
                package,
                user,
                flags,
            } => self
                .destroy_app_data(package, *user, *flags)
                .map(|()| Vec::new()),
            Request::ClearAppData {
                package,
                user,
                flags,
            } => self
                .clear_app_data(package, *user, *flags)
                .map(|()| Vec::new()),
            Request::RmPackageDir { code_path } => {
                self.rm_package_dir(code_path).map(|()| Vec::new())
            }
        }
    }

    /// The `/data` tree of a profile process: an application's own, or the
    /// system process's (`android.system`).
    fn private_data(&self, owner: &str) -> PathBuf {
        self.paths
            .mount
            .join("data/apps")
            .join(owner)
            .join("private-data")
    }

    /// Whose `/data` holds a package's directories. Android isolates data by
    /// uid; a package that shares the system uid is visible to the system
    /// process, like every other system-uid package.
    fn owner(package: &str, app_id: u32) -> &str {
        if app_id == SYSTEM_APP_ID {
            SYSTEM_DATA_OWNER
        } else {
            package
        }
    }

    /// The owner of a package's existing directories, for requests that carry
    /// no app ID (destroy/clear): the system tree when it holds them.
    fn existing_owner<'p>(&self, package: &'p str, user: u32) -> &'p str {
        let system = self.private_data(SYSTEM_DATA_OWNER);
        let held = |storage: &str| {
            system
                .join(format!("{storage}/{user}"))
                .join(package)
                .exists()
        };
        if held("user") || held("user_de") {
            SYSTEM_DATA_OWNER
        } else {
            package
        }
    }

    fn ce_dir(&self, owner: &str, package: &str, user: u32) -> PathBuf {
        self.private_data(owner)
            .join(format!("user/{user}"))
            .join(package)
    }

    fn de_dir(&self, owner: &str, package: &str, user: u32) -> PathBuf {
        self.private_data(owner)
            .join(format!("user_de/{user}"))
            .join(package)
    }

    fn validate(package: &str, user: u32) -> Result<(), ProfileError> {
        validate_package(package)?;
        // The profile has one Android user; installd's other users do not exist.
        if user != 0 {
            return Err(ProfileError::Daemon(format!("unknown Android user {user}")));
        }
        Ok(())
    }

    /// installd create_app_data: the CE/DE package directories with their
    /// `cache` and `code_cache` children. Existing data is preserved.
    pub(crate) fn create_app_data(
        &self,
        package: &str,
        user: u32,
        flags: u32,
        app_id: u32,
    ) -> Result<AppDataInodes, ProfileError> {
        Self::validate(package, user)?;
        let owner = Self::owner(package, app_id);
        let mut inodes = AppDataInodes::default();
        for (flag, directory, inode) in [
            (
                FLAG_STORAGE_CE,
                self.ce_dir(owner, package, user),
                &mut inodes.ce,
            ),
            (
                FLAG_STORAGE_DE,
                self.de_dir(owner, package, user),
                &mut inodes.de,
            ),
        ] {
            if flags & flag == 0 {
                continue;
            }
            for child in ["cache", "code_cache"] {
                fs::create_dir_all(directory.join(child))?;
            }
            *inode = fs::metadata(&directory)?.ino();
        }
        Ok(inodes)
    }

    /// installd destroy_app_data: remove the selected storage entirely.
    pub(crate) fn destroy_app_data(
        &self,
        package: &str,
        user: u32,
        flags: u32,
    ) -> Result<(), ProfileError> {
        Self::validate(package, user)?;
        let owner = self.existing_owner(package, user);
        for (flag, directory) in [
            (FLAG_STORAGE_CE, self.ce_dir(owner, package, user)),
            (FLAG_STORAGE_DE, self.de_dir(owner, package, user)),
        ] {
            if flags & flag != 0 {
                remove_tree(&directory)?;
            }
        }
        Ok(())
    }

    /// installd clear_app_data: empty the storage, or only its cache or
    /// code_cache, keeping the package directory itself.
    pub(crate) fn clear_app_data(
        &self,
        package: &str,
        user: u32,
        flags: u32,
    ) -> Result<(), ProfileError> {
        Self::validate(package, user)?;
        let owner = self.existing_owner(package, user);
        for (flag, directory) in [
            (FLAG_STORAGE_CE, self.ce_dir(owner, package, user)),
            (FLAG_STORAGE_DE, self.de_dir(owner, package, user)),
        ] {
            if flags & flag == 0 || !directory.exists() {
                continue;
            }
            if flags & FLAG_CLEAR_CACHE_ONLY != 0 {
                empty_directory(&directory.join("cache"))?;
            } else if flags & FLAG_CLEAR_CODE_CACHE_ONLY != 0 {
                empty_directory(&directory.join("code_cache"))?;
            } else {
                empty_directory(&directory)?;
                for child in ["cache", "code_cache"] {
                    fs::create_dir_all(directory.join(child))?;
                }
            }
        }
        Ok(())
    }

    /// installd rm_package_dir: remove one installed code directory from
    /// `/data/app`. Only a directory inside the store may be removed. The
    /// directory moves to the profile's package trash rather than being
    /// destroyed: PackageManagerService removes every code directory it cannot
    /// parse, and an APK the user installed must stay recoverable (ADR 0009).
    pub(crate) fn rm_package_dir(&self, code_path: &str) -> Result<(), ProfileError> {
        let relative = code_path
            .strip_prefix("/data/app/")
            .ok_or_else(|| ProfileError::Daemon("code path is outside /data/app".into()))?;
        let relative = Path::new(relative);
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(ProfileError::Daemon("invalid package code path".into()));
        }
        let source = self.paths.mount.join("packages").join(relative);
        if fs::symlink_metadata(&source).is_err() {
            return Ok(()); // installd treats an absent directory as removed.
        }
        // Moving a directory rewrites its `..` entry; published package
        // directories are read-only.
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::symlink_metadata(&source)?.permissions();
            permissions.set_mode(permissions.mode() | 0o700);
            fs::set_permissions(&source, permissions)?;
        }
        let trash = self.paths.mount.join("system/package-trash");
        fs::create_dir_all(&trash)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let name = relative.to_string_lossy().replace('/', "__");
        fs::rename(&source, trash.join(format!("{stamp}-{name}")))?;
        Ok(())
    }
}

fn remove_tree(path: &Path) -> Result<(), ProfileError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => {
            make_writable(path)?;
            fs::remove_dir_all(path)?;
            Ok(())
        }
        Ok(_) => Ok(fs::remove_file(path)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn empty_directory(path: &Path) -> Result<(), ProfileError> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        remove_tree(&entry?.path())?;
    }
    Ok(())
}

/// Installed payloads are published read-only; deletion needs writable dirs.
fn make_writable(path: &Path) -> Result<(), ProfileError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Ok(());
    }
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o700);
    fs::set_permissions(path, permissions)?;
    for entry in fs::read_dir(path)? {
        make_writable(&entry?.path())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(label: &str) -> ProfilePaths {
        let root =
            std::env::temp_dir().join(format!("darwin-installd-{label}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut paths = ProfilePaths::new(root.clone(), "default").unwrap();
        paths.mount = root.join("mnt");
        fs::create_dir_all(&paths.mount).unwrap();
        paths
    }

    #[test]
    fn request_round_trips() {
        for request in [
            Request::CreateAppData {
                package: "org.example".into(),
                user: 0,
                flags: 3,
                app_id: 10_123,
            },
            Request::DestroyAppData {
                package: "org.example".into(),
                user: 0,
                flags: 2,
            },
            Request::ClearAppData {
                package: "org.example".into(),
                user: 0,
                flags: 0x12,
            },
            Request::RmPackageDir {
                code_path: "/data/app/~~a/org.example-b".into(),
            },
        ] {
            assert_eq!(Request::decode(&request.encode()).unwrap(), request);
        }
        assert!(Request::decode(&[]).is_err());
        assert!(Request::decode(&[9]).is_err());
        assert!(Request::decode(&[1, 0, 0]).is_err());
        // The pre-app-ID createAppData command is refused, not misread.
        let mut stale = vec![1];
        stale.extend_from_slice(&[0; 8]);
        stale.extend_from_slice(b"org.example");
        assert!(Request::decode(&stale).is_err());
    }

    #[test]
    fn creates_preserves_clears_and_destroys_app_data() {
        let paths = fixture("data");
        let installd = Installd::new(&paths);
        let ce = paths
            .mount
            .join("data/apps/org.example/private-data/user/0/org.example");
        let de = paths
            .mount
            .join("data/apps/org.example/private-data/user_de/0/org.example");
        let inodes = installd
            .create_app_data("org.example", 0, FLAG_STORAGE_CE | FLAG_STORAGE_DE, 10_123)
            .unwrap();
        assert!(ce.join("cache").is_dir() && ce.join("code_cache").is_dir());
        assert!(de.join("cache").is_dir());
        assert_eq!(inodes.ce, fs::metadata(&ce).unwrap().ino());
        assert_eq!(inodes.de, fs::metadata(&de).unwrap().ino());
        fs::write(ce.join("files.db"), b"kept").unwrap();
        fs::write(ce.join("cache/tmp"), b"cache").unwrap();
        installd
            .create_app_data("org.example", 0, FLAG_STORAGE_CE, 10_123)
            .unwrap();
        assert_eq!(fs::read(ce.join("files.db")).unwrap(), b"kept");

        installd
            .clear_app_data("org.example", 0, FLAG_STORAGE_CE | FLAG_CLEAR_CACHE_ONLY)
            .unwrap();
        assert!(!ce.join("cache/tmp").exists() && ce.join("files.db").exists());
        installd
            .clear_app_data("org.example", 0, FLAG_STORAGE_CE)
            .unwrap();
        assert!(!ce.join("files.db").exists() && ce.join("cache").is_dir());

        installd
            .destroy_app_data("org.example", 0, FLAG_STORAGE_DE)
            .unwrap();
        assert!(!de.exists() && ce.exists());
        assert!(
            installd
                .create_app_data("org.example", 10, FLAG_STORAGE_CE, 10_123)
                .is_err()
        );
        assert!(
            installd
                .create_app_data("../x", 0, FLAG_STORAGE_CE, 10_123)
                .is_err()
        );
    }

    #[test]
    fn system_uid_packages_keep_data_with_the_system_process() {
        let paths = fixture("system-uid");
        let installd = Installd::new(&paths);
        let system = paths.mount.join("data/apps/android.system/private-data");
        let de = system.join("user_de/0/com.android.providers.settings");
        installd
            .create_app_data(
                "com.android.providers.settings",
                0,
                FLAG_STORAGE_CE | FLAG_STORAGE_DE,
                SYSTEM_APP_ID,
            )
            .unwrap();
        assert!(de.join("cache").is_dir());
        assert!(
            system
                .join("user/0/com.android.providers.settings")
                .is_dir()
        );
        assert!(
            !paths
                .mount
                .join("data/apps/com.android.providers.settings")
                .exists()
        );
        fs::write(de.join("settings.db"), b"db").unwrap();
        installd
            .clear_app_data("com.android.providers.settings", 0, FLAG_STORAGE_DE)
            .unwrap();
        assert!(!de.join("settings.db").exists() && de.join("cache").is_dir());
        installd
            .destroy_app_data("com.android.providers.settings", 0, FLAG_STORAGE_DE)
            .unwrap();
        assert!(!de.exists());
    }

    #[test]
    fn removes_only_store_package_directories() {
        let paths = fixture("code");
        let installd = Installd::new(&paths);
        let code = paths.mount.join("packages/~~a/org.example-b");
        fs::create_dir_all(code.join("lib")).unwrap();
        fs::write(code.join("base.apk"), b"apk").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&code, fs::Permissions::from_mode(0o555)).unwrap();
        installd
            .rm_package_dir("/data/app/~~a/org.example-b")
            .unwrap();
        assert!(!code.exists());
        let trashed: Vec<_> = fs::read_dir(paths.mount.join("system/package-trash"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(trashed.len(), 1);
        assert_eq!(fs::read(trashed[0].join("base.apk")).unwrap(), b"apk");
        installd
            .rm_package_dir("/data/app/~~a/org.example-b")
            .unwrap();
        for rejected in ["/data/app/", "/data/app/../x", "/data/user/0/x", "relative"] {
            assert!(installd.rm_package_dir(rejected).is_err(), "{rejected}");
        }
    }
}
