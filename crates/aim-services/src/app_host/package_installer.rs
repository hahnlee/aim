//! PackageInstallerService for the app host (`IPackageInstaller` at
//! `android-16.0.0_r1`, `getPackageInstaller`): the native installer's
//! sessions (`package::installer`, its `Endpoint` and `NativeOwners`), with
//! the app host's owners of what they read: the package model, the
//! device's policy (`createSessionInternal`'s permissions, user
//! restrictions, verifier), `/data/system/install_sessions.xml` and the
//! stage directories `/data/app/vmdl<id>.tmp`, labelled by the agent
//! (`SELinux.restorecon`), and their writes' space (`StorageManager
//! .allocateBytes`). Sessions are made, opened, written, read, sealed,
//! transferred and abandoned, persisted as the original persists them,
//! and their events go to the registered callbacks. Committing: #1995;
//! uninstalling: #1996; reconciling stages and expired sessions at start:
//! #1997; installing existing packages, archiving, install constraints,
//! pre-approval, hard-linked stages, checksums requested of a session,
//! app icons and pre-verified domains: #1757.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, Exception};
use aim_storage::guest_inode::GuestInode;

use super::{AppHost, reply};
use crate::package::installer::endpoint::{self, ArchiveAction, ConstraintReceiver, Endpoint};
use crate::package::installer::native::NativeOwners;
use crate::package::installer::policy::{CallingPermissions, DevicePolicy, UserPolicy};
use crate::package::installer::{
    self as installer, Event, InstallationFile, Record, Session, SessionNode, SessionOperations,
    Sessions, checksums, codec, constraints, existing, preapproval, removal, staged_owner, storage,
};

/// `IAppHostAgent.restorecon(String path)`.
const RESTORECON: u32 = 15;
/// What system_server's `AtomicFile` and `prepareStageDir` make: the
/// system's, `install_sessions.xml` under its umask, a stage `0775`.
const SESSIONS_INODE: GuestInode = GuestInode {
    uid: Some(1000),
    gid: Some(1000),
    mode: Some(0o600),
};
const STAGE_INODE: GuestInode = GuestInode {
    uid: Some(1000),
    gid: Some(1000),
    mode: Some(0o775),
};
/// `PackageManager.INSTALL_ALLOCATE_AGGRESSIVE`, and
/// `StorageManager.FLAG_ALLOCATE_AGGRESSIVE` it translates to.
const INSTALL_ALLOCATE_AGGRESSIVE: i32 = 0x8000;
const FLAG_ALLOCATE_AGGRESSIVE: i32 = 1;
/// `PackageManager.INSTALL_FAILED_INTERNAL_ERROR`.
const INSTALL_FAILED_INTERNAL_ERROR: i32 = -110;
/// `KnownPackages.PACKAGE_VERIFIER`'s intent (`getRequiredButNotReally
/// RequiredVerifiersLPr`).
const ACTION_PACKAGE_NEEDS_VERIFICATION: &str = "android.intent.action.PACKAGE_NEEDS_VERIFICATION";
const PACKAGE_MIME_TYPE: &str = "application/vnd.android.package-archive";
const MATCH_SYSTEM_ONLY: i64 = 0x0010_0000;
const MATCH_DIRECT_BOOT_AWARE: i64 = 0x0008_0000;
const MATCH_DIRECT_BOOT_UNAWARE: i64 = 0x0004_0000;
/// `Flags.rollbackLifetime()`.
const ROLLBACK_LIFETIME: &str = "android.content.pm.rollback_lifetime";

pub struct PackageInstaller {
    host: Arc<AppHost>,
    /// The installer, made when first called (the app host's data and
    /// package model are known by then), with the guards of its workers.
    endpoint: Mutex<Option<Installer>>,
}

struct Installer {
    endpoint: Arc<Endpoint>,
    _callbacks: Option<installer::callbacks::CallbackWorker>,
    _io: Option<installer::file_bridge::IoWorkerGuard>,
}

impl PackageInstaller {
    pub fn new(host: Arc<AppHost>) -> Self {
        PackageInstaller {
            host,
            endpoint: Mutex::new(None),
        }
    }

    fn endpoint(&self) -> Result<Arc<Endpoint>, Exception> {
        let mut installer = self.endpoint.lock().unwrap();
        if let Some(installer) = &*installer {
            return Ok(installer.endpoint.clone());
        }
        let made = open(&self.host)?;
        let endpoint = made.endpoint.clone();
        *installer = Some(made);
        Ok(endpoint)
    }
}

/// The native installer over the app host's owners, its persisted
/// sessions restored (`readSessionsLocked`).
fn open(host: &Arc<AppHost>) -> Result<Installer, Exception> {
    let data = host.data()?.to_path_buf();
    let disk = storage::Store::open(data, SESSIONS_INODE, STAGE_INODE, labeler(host))
        .map_err(storage_exception)?;
    let sessions = Arc::new(Sessions::default());
    let weak = Arc::downgrade(host);
    let source: installer::native::QuerySource =
        Arc::new(move || Ok(upgrade(&weak)?.packages()?.0));
    let weak = Arc::downgrade(host);
    let publisher: installer::native::Publisher =
        Arc::new(move |service| Ok(upgrade(&weak)?.process.add_service(service)));
    let native = NativeOwners::open(
        sessions.clone(),
        source,
        policy_source(host),
        crate::package::system_config::SystemConfig::read(host.root()?, &|name| {
            host.property(name)
        }),
        disk,
        publisher,
        // The registered callbacks hear the events from the owner itself;
        // the system's own listener records finished installs (#1757).
        Arc::new(|_| {}),
        host.process.clone(),
    )?;
    let weak = Arc::downgrade(host);
    let mode: installer::native::WriteModeSource = Arc::new(move || {
        // `PackageInstaller.ENABLE_REVOCABLE_FD`.
        Ok(upgrade(&weak)?.property("fw.revocable_fd").as_deref() == Some("true"))
    });
    native.configure_writer(mode, allocation(host))?;
    let callbacks = native.take_callback_worker();
    let io = native.take_optional_io_worker_guard()?;
    let owners = Arc::new(Owners {
        native,
        host: Arc::downgrade(host),
    });
    Ok(Installer {
        endpoint: Arc::new(Endpoint {
            sessions,
            owners: owners.clone(),
        }),
        _callbacks: callbacks,
        _io: io,
    })
}

fn upgrade(host: &Weak<AppHost>) -> Result<Arc<AppHost>, Exception> {
    host.upgrade()
        .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host stopped"))
}

fn storage_exception(error: storage::Error) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, error.message)
}

/// `SELinux.restorecon` of what the installer made, by the agent, failing
/// as `prepareStageDir` fails.
fn labeler(host: &Arc<AppHost>) -> storage::Labeler {
    let host = Arc::downgrade(host);
    Arc::new(move |_, guest| {
        let fail = |message: String| storage::Error {
            legacy_status: INSTALL_FAILED_INTERNAL_ERROR,
            committed: false,
            message,
        };
        let host = upgrade(&host).map_err(|e| fail(e.message))?;
        let reply = host
            .call_agent(RESTORECON, "restorecon", |p| p.write_string16(Some(guest)))
            .map_err(|e| fail(e.message))?;
        let mut r = reply.reader();
        let restored = r
            .read_exception()
            .ok()
            .and_then(|_| r.read_i32().ok())
            .ok_or_else(|| fail("restorecon reply".into()))?;
        if restored == 0 {
            return Err(fail(format!("Failed to restorecon session dir: {guest}")));
        }
        Ok(())
    })
}

/// `StorageManager.allocateBytes(fd, bytes, translateAllocateFlags(...))`,
/// as system_server asks it: the bytes the file does not have yet made
/// free, then the file allocated to its length (`posix_fallocate`, which
/// the guest's layer makes a preallocation and an extension).
fn allocation(host: &Arc<AppHost>) -> installer::native::AllocationOwner {
    let host = Arc::downgrade(host);
    Arc::new(move |file, bytes, install_flags| {
        let host = upgrade(&host)?;
        let io = |e: std::io::Error| Exception::new(EX_ILLEGAL_STATE, e.to_string());
        let need = bytes - allocated(file).map_err(io)?;
        if need > 0 {
            host.allocate_bytes(None, need, allocate_flags(install_flags), 1000)?;
        }
        fallocate(file, bytes).map_err(io)
    })
}

/// `InstallLocationUtils.translateAllocateFlags`.
fn allocate_flags(install_flags: i32) -> i32 {
    if install_flags & INSTALL_ALLOCATE_AGGRESSIVE != 0 {
        FLAG_ALLOCATE_AGGRESSIVE
    } else {
        0
    }
}

/// `Os.fstat(fd).st_blocks * 512`.
fn allocated(file: &std::fs::File) -> std::io::Result<i64> {
    Ok(std::os::unix::fs::MetadataExt::blocks(&file.metadata()?) as i64 * 512)
}

/// `posix_fallocate(fd, 0, bytes)` as the guest's layer makes it: the
/// space past the end preallocated, the file extended to `bytes`.
fn fallocate(file: &std::fs::File, bytes: i64) -> std::io::Result<()> {
    let size = file.metadata()?.len() as i64;
    if bytes <= size {
        return Ok(());
    }
    let mut store = libc::fstore_t {
        fst_flags: libc::F_ALLOCATEALL,
        fst_posmode: libc::F_PEOFPOSMODE,
        fst_offset: 0,
        fst_length: bytes - size,
        fst_bytesalloc: 0,
    };
    // SAFETY: F_PREALLOCATE with a local fstore; advisory on APFS, as the
    // layer's fallocate makes it.
    unsafe { libc::fcntl(file.as_raw_fd(), libc::F_PREALLOCATE, &mut store) };
    file.set_len(bytes as u64)
}

/// `createSessionInternal`'s view of the device: the caller's permissions,
/// the build, the user's restrictions and organization, an adopted shell,
/// and whether the caller is the required verifier.
fn policy_source(host: &Arc<AppHost>) -> installer::native::PolicySource {
    let weak = Arc::downgrade(host);
    Arc::new(move |uid, _pid, user| {
        let host = upgrade(&weak)?;
        let permissions = {
            let weak = weak.clone();
            CallingPermissions::new(Arc::new(move |name, _pid, uid| {
                Ok(upgrade(&weak)?.check_permission(Some(name), uid)? == 0)
            }))
        };
        let uid_i32 = uid as i32;
        let (state, resolution) = host.packages()?;
        let flag = |name: &str| state.system.flags.iter().any(|(n, on)| n == name && *on);
        let mut users = std::collections::BTreeMap::new();
        // The app host has one user, the system user.
        if user == 0 {
            if host.device_policies()?.owned() {
                return Err(host.unimplemented_named(
                    "package_installer",
                    "a device or profile owner's organization (#1521)",
                ));
            }
            let restrictions = host.user_restrictions()?;
            users.insert(
                0,
                UserPolicy {
                    disallow_install_apps: restrictions.contains("no_install_apps"),
                    disallow_debugging_features: restrictions.contains("no_debugging_features"),
                    organization_managed: false,
                },
            );
        }
        // `PackageManagerServiceUtils.isAdoptedShell`.
        let mut adopted_shell_uids = std::collections::BTreeSet::new();
        if uid != 1000
            && host.check_permission(Some("android.permission.USE_SYSTEM_DATA_LOADERS"), uid_i32)?
                == 0
        {
            adopted_shell_uids.insert(uid);
        }
        let verifiers = resolution
            .query_intent_receivers(
                &crate::package::intent::Intent {
                    action: Some(ACTION_PACKAGE_NEEDS_VERIFICATION.into()),
                    ..Default::default()
                },
                Some(PACKAGE_MIME_TYPE),
                MATCH_SYSTEM_ONLY | MATCH_DIRECT_BOOT_AWARE | MATCH_DIRECT_BOOT_UNAWARE,
                0,
                1000,
            )
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("verifiers: {e:?}")))?;
        let verifier_uid = verifiers
            .iter()
            .filter_map(|v| state.packages.get(v.component().0))
            .any(|ps| ps.app_id == uid_i32 % super::PER_USER_RANGE as i32)
            .then_some(uid);
        Ok(DevicePolicy {
            permissions,
            debuggable: host.property("ro.debuggable").as_deref() == Some("1"),
            // `ApexManager.isApexSupported`: `ApexProperties.updatable()`.
            apex_supported: host.property("ro.apex.updatable").as_deref() == Some("true"),
            rollback_lifetime: flag(ROLLBACK_LIFETIME),
            users,
            adopted_shell_uids,
            verifier_uid,
        })
    })
}

/// The native installer's owners, less what the app host does not own
/// yet, which answers as not implemented (#1757).
struct Owners {
    native: Arc<NativeOwners>,
    host: Weak<AppHost>,
}

impl Owners {
    fn unimplemented(&self, what: &str) -> Exception {
        self.unimplemented_as(what, 1757)
    }

    fn unimplemented_as(&self, what: &str, issue: u32) -> Exception {
        unimplemented(&self.host, what, issue)
    }
}

fn unimplemented(host: &Weak<AppHost>, what: &str, issue: u32) -> Exception {
    match host.upgrade() {
        Some(host) => host.unimplemented_named("package_installer", &format!("{what} (#{issue})")),
        None => Exception::new(EX_ILLEGAL_STATE, "app host stopped"),
    }
}

impl endpoint::Owners for Owners {
    fn uninstall(
        &self,
        _request: removal::Request,
        _receiver: Option<preapproval::IntentSender>,
    ) -> Result<(), Exception> {
        Err(self.unimplemented_as("uninstall", 1996))
    }
    fn archive(&self, _uid: u32, _pid: i32, _action: ArchiveAction) -> Result<(), Exception> {
        Err(self.unimplemented("archiving"))
    }
    fn update_icon(&self, _uid: u32, _id: i32, _body: codec::Object) -> Result<(), Exception> {
        Err(self.unimplemented("a session's app icon"))
    }
    fn install_constraints(
        &self,
        _uid: u32,
        _installer: Option<String>,
        _names: Option<Vec<Option<String>>>,
        _constraints: Option<constraints::Constraints>,
        _receiver: ConstraintReceiver,
        _timeout: i64,
    ) -> Result<(), Exception> {
        Err(self.unimplemented("install constraints"))
    }
    fn install_existing(
        &self,
        _uid: u32,
        _pid: i32,
        _request: existing::Request,
    ) -> Result<(), Exception> {
        Err(self.unimplemented("installExistingPackage"))
    }
    fn permissions_result(&self, _uid: u32, _id: i32, _accepted: bool) -> Result<(), Exception> {
        Err(self.unimplemented("a session's permissions result"))
    }
    fn normalize(
        &self,
        uid: u32,
        pid: i32,
        params: codec::SessionParams,
        installer: Option<String>,
        tag: Option<String>,
        user: i32,
    ) -> Result<(Record, bool), Exception> {
        if params.app_icon.is_some() {
            return Err(self.unimplemented("a session's app icon"));
        }
        self.native
            .normalize(uid, pid, params, installer, tag, user)
    }
    fn staged_status(&self, _id: i32) -> Result<Option<staged_owner::Status>, Exception> {
        Err(self.unimplemented("staged sessions"))
    }
    fn session_created(&self, id: i32, user: u32) -> Result<(), Exception> {
        self.native.session_created(id, user)
    }
    fn prepare_stage(&self, session: &Session, record: &Record) -> Result<(), Exception> {
        self.native.prepare_stage(session, record)
    }
    fn session_operations(&self) -> Option<Arc<dyn SessionOperations>> {
        Some(Arc::new(Operations {
            native: self.native.clone(),
            host: self.host.clone(),
        }))
    }
    fn publish_session(&self, node: Arc<SessionNode>) -> Result<Binder, Exception> {
        self.native.publish_session(node)
    }
    fn notify(&self, event: Event) {
        self.native.notify(event)
    }
    fn enforce_cross_user(&self, uid: u32, user: i32, operation: &str) -> Result<(), Exception> {
        self.native.enforce_cross_user(uid, user, operation)
    }
    fn check_package(&self, uid: u32, package: Option<&str>) -> Result<(), Exception> {
        self.native.check_package(uid, package)
    }
    fn can_query(&self, uid: u32, package: Option<&str>) -> Result<bool, Exception> {
        self.native.can_query(uid, package)
    }
    fn can_read_paths(&self, uid: u32) -> Result<bool, Exception> {
        self.native.can_read_paths(uid)
    }
    fn resolved_path(&self, id: i32) -> Result<Option<String>, Exception> {
        self.native.resolved_path(id)
    }
    fn is_verifier(&self, uid: u32) -> Result<bool, Exception> {
        self.native.is_verifier(uid)
    }
    fn update_label(&self, uid: u32, id: i32, label: Option<String>) -> Result<(), Exception> {
        self.native.update_label(uid, id, label)
    }
    fn register_callback(
        &self,
        uid: u32,
        callback: Option<Binder>,
        user: i32,
    ) -> Result<(), Exception> {
        self.native.register_callback(uid, callback, user)
    }
    fn unregister_callback(&self, callback: Option<Binder>) -> Result<(), Exception> {
        self.native.unregister_callback(callback)
    }
    fn set_service_policy(
        &self,
        uid: u32,
        setting: endpoint::ServiceSetting,
    ) -> Result<(), Exception> {
        self.native.set_service_policy(uid, setting)
    }
    fn abandon_stage(&self, ids: &[i32]) -> Result<(), Exception> {
        self.native.abandon_stage(ids)
    }
}

/// A session's operations: the native installer's on its stage and
/// state, the rest not implemented (#1757).
struct Operations {
    native: Arc<NativeOwners>,
    host: Weak<AppHost>,
}

impl Operations {
    fn unimplemented(&self, what: &str) -> Exception {
        self.unimplemented_as(what, 1757)
    }

    fn unimplemented_as(&self, what: &str, issue: u32) -> Exception {
        unimplemented(&self.host, what, issue)
    }
}

impl SessionOperations for Operations {
    fn commit_session(
        &self,
        _id: i32,
        _uid: u32,
        _receiver: Option<preapproval::IntentSender>,
        _transferred: bool,
    ) -> Result<(), Exception> {
        Err(self.unimplemented_as("committing a session", 1995))
    }
    fn request_preapproval(
        &self,
        _id: i32,
        _uid: u32,
        _body: codec::Object,
    ) -> Result<(), Exception> {
        Err(self.unimplemented("pre-approval"))
    }
    fn stage_hard_link(
        &self,
        _id: i32,
        _uid: u32,
        _target: Option<String>,
    ) -> Result<(), Exception> {
        Err(self.unimplemented("a hard-linked stage"))
    }
    fn set_checksums(
        &self,
        id: i32,
        uid: u32,
        name: Option<String>,
        checksums: Option<Vec<Option<checksums::Checksum>>>,
        signature: Option<Vec<u8>>,
    ) -> Result<(), Exception> {
        self.native
            .set_checksums(id, uid, name, checksums, signature)
    }
    fn request_checksums(
        &self,
        _id: i32,
        _uid: u32,
        _name: Option<String>,
        _optional: i32,
        _required: i32,
        _trusted: checksums::TrustedInstallers,
        _listener: Option<Binder>,
    ) -> Result<(), Exception> {
        Err(self.unimplemented("a session's checksums requested"))
    }
    fn fetch_package_names(&self, _id: i32, _uid: u32) -> Result<Vec<Option<String>>, Exception> {
        Err(self.unimplemented("a session's package names"))
    }
    fn add_file(&self, id: i32, uid: u32, file: InstallationFile) -> Result<(), Exception> {
        self.native.add_file(id, uid, file)
    }
    fn remove_file(
        &self,
        id: i32,
        uid: u32,
        location: i32,
        name: Option<String>,
    ) -> Result<(), Exception> {
        self.native.remove_file(id, uid, location, name)
    }
    fn pre_verified_domains(&self, _id: i32, _uid: u32) -> Result<Option<Vec<String>>, Exception> {
        Err(self.unimplemented("pre-verified domains"))
    }
    fn set_pre_verified_domains(
        &self,
        _id: i32,
        _uid: u32,
        _domains: Option<crate::package::domain_verification::domain_set::DomainSet>,
    ) -> Result<(), Exception> {
        Err(self.unimplemented("pre-verified domains"))
    }
    fn metadata_read(
        &self,
        id: i32,
        uid: u32,
    ) -> Result<Option<aim_binder_driver::File>, Exception> {
        self.native.metadata_read(id, uid)
    }
    fn metadata_write(&self, id: i32, uid: u32) -> Result<aim_binder_driver::File, Exception> {
        self.native.metadata_write(id, uid)
    }
    fn metadata_remove(&self, id: i32) -> Result<(), Exception> {
        self.native.metadata_remove(id)
    }
    fn open_write(
        &self,
        id: i32,
        uid: u32,
        name: Option<String>,
        offset: i64,
        length: i64,
    ) -> Result<aim_binder_driver::File, Exception> {
        self.native.open_write(id, uid, name, offset, length)
    }
    fn write_file(
        &self,
        id: i32,
        uid: u32,
        name: Option<String>,
        offset: i64,
        length: i64,
        fd: Option<u32>,
    ) -> Result<(), Exception> {
        self.native.write_file(id, uid, name, offset, length, fd)
    }
    fn names(&self, id: i32, uid: u32) -> Result<Vec<Option<String>>, Exception> {
        self.native.names(id, uid)
    }
    fn open_read(
        &self,
        id: i32,
        uid: u32,
        name: Option<String>,
    ) -> Result<aim_binder_driver::File, Exception> {
        self.native.open_read(id, uid, name)
    }
    fn remove_split(&self, id: i32, uid: u32, name: Option<String>) -> Result<(), Exception> {
        self.native.remove_split(id, uid, name)
    }
    fn abandon_session(&self, id: i32, uid: u32) -> Result<(), Exception> {
        self.native.abandon_session(id, uid)
    }
    fn data_loader(&self, id: i32, uid: u32) -> Result<Option<codec::Object>, Exception> {
        self.native.data_loader(id, uid)
    }
    fn transfer_session(&self, id: i32, uid: u32, name: Option<String>) -> Result<(), Exception> {
        self.native.transfer_session(id, uid, name)
    }
    fn seal_session(&self, id: i32, uid: u32) -> Result<(), Exception> {
        self.native.seal_session(id, uid)
    }
    fn graph_changed(&self) -> Result<(), Exception> {
        self.native.graph_changed()
    }
}

impl Service for PackageInstaller {
    fn descriptor(&self) -> &str {
        aim_service_aidl::android_content_pm_ipackageinstaller::DESCRIPTOR
    }

    fn accepts_fds(&self) -> bool {
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match self.endpoint() {
            Ok(endpoint) => endpoint.transact(call),
            Err(e) => reply(Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only an aggressive install asks for an aggressive allocation.
    #[test]
    fn allocation_flags_are_translated_as_the_original_translates_them() {
        assert_eq!(allocate_flags(0), 0);
        assert_eq!(allocate_flags(0x2 | 0x10), 0);
        assert_eq!(
            allocate_flags(INSTALL_ALLOCATE_AGGRESSIVE | 0x2),
            FLAG_ALLOCATE_AGGRESSIVE
        );
    }

    /// A write's file is extended to the length asked for, never shrunk,
    /// and keeps what was written.
    #[test]
    fn fallocate_extends_and_never_truncates() {
        let path =
            std::env::temp_dir().join(format!("aim-package-installer-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut &file, b"apk").unwrap();
        fallocate(&file, 1 << 20).unwrap();
        assert_eq!(file.metadata().unwrap().len(), 1 << 20);
        fallocate(&file, 16).unwrap();
        assert_eq!(file.metadata().unwrap().len(), 1 << 20);
        assert_eq!(&std::fs::read(&path).unwrap()[..3], b"apk");
        assert!(allocated(&file).unwrap() >= 0);
        std::fs::remove_file(&path).unwrap();
    }
}
