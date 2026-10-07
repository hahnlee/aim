//! Concrete native policy, persistence and stage owners for install sessions.
use super::{
    Event, Record, Session, SessionNode, Sessions,
    codec::SessionParams,
    endpoint::Owners,
    policy::{self, DevicePolicy},
    storage,
};
use crate::package::{model::State, query::Query, resolve::Resolver, system_config::SystemConfig};
use aim_binder_host::{
    local::Service,
    parcel::{Binder, EX_ILLEGAL_STATE, Exception},
};
use std::sync::{Arc, Mutex};
pub type QuerySource = Arc<dyn Fn() -> Result<Arc<State>, Exception> + Send + Sync>;
pub type Publisher = Arc<dyn Fn(Arc<dyn Service>) -> Result<Binder, Exception> + Send + Sync>;
pub type Callback = Arc<dyn Fn(Event) + Send + Sync>;
pub struct NativeOwners {
    pub sessions: Arc<Sessions>,
    source: QuerySource,
    resolver: Resolver,
    device: Mutex<DevicePolicy>,
    config: SystemConfig,
    disk: Mutex<storage::Store>,
    publisher: Publisher,
    callback: Callback,
    errors: Mutex<Vec<String>>,
}
fn disk_error(error: storage::Error) -> Exception {
    Exception::new(
        EX_ILLEGAL_STATE,
        format!(
            "Native installer persistence: committed={}: {}",
            error.committed, error.message
        ),
    )
}
impl NativeOwners {
    pub fn open(
        sessions: Arc<Sessions>,
        source: QuerySource,
        device: DevicePolicy,
        config: SystemConfig,
        disk: storage::Store,
        publisher: Publisher,
        callback: Callback,
    ) -> Result<Arc<Self>, Exception> {
        let recovered = disk.recovered().map_err(disk_error)?;
        sessions.restore(recovered)?;
        Ok(Arc::new(Self {
            sessions,
            source,
            resolver: Resolver::default(),
            device: Mutex::new(device),
            config,
            disk: Mutex::new(disk),
            publisher,
            callback,
            errors: Mutex::new(Vec::new()),
        }))
    }
    fn query<T>(
        &self,
        uid: u32,
        action: impl FnOnce(&Query<'_>) -> Result<T, Exception>,
    ) -> Result<T, Exception> {
        let state = (self.source)()?;
        let resolution = self.resolver.resolution(&state).map_err(|error| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("native installer query resolution: {error:?}"),
            )
        })?;
        action(&Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: uid as i32,
        })
    }
    fn persist(&self) -> Result<(), Exception> {
        let records = self.sessions.records();
        self.disk
            .lock()
            .unwrap()
            .write(&records)
            .map_err(disk_error)
    }
    /// Asynchronous callback paths expose failed persistence to the service owner.
    pub fn take_errors(&self) -> Vec<String> {
        std::mem::take(&mut *self.errors.lock().unwrap())
    }
}
impl Owners for NativeOwners {
    fn normalize(
        &self,
        uid: u32,
        params: SessionParams,
        installer: Option<String>,
        tag: Option<String>,
        user: i32,
    ) -> Result<(Record, bool), Exception> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.to_string()))?
            .as_millis() as i64;
        let result = self.query(uid, |query| {
            policy::normalize(
                query,
                &mut self.device.lock().unwrap(),
                &self.config,
                params,
                installer,
                tag,
                user,
                now,
            )
        })?;
        self.disk
            .lock()
            .unwrap()
            .validate_storage(&result.0.params)
            .map_err(disk_error)?;
        Ok(result)
    }
    fn session_created(&self, id: i32, user: u32) -> Result<(), Exception> {
        self.persist()?;
        (self.callback)(Event::Created { id, user });
        Ok(())
    }
    fn prepare_stage(&self, session: &Session, record: &Record) -> Result<(), Exception> {
        self.disk
            .lock()
            .unwrap()
            .prepare_stage(session, record)
            .map_err(disk_error)
    }
    fn publish_session(&self, node: Arc<SessionNode>) -> Result<Binder, Exception> {
        (self.publisher)(node)
    }
    fn notify(&self, event: Event) {
        if let Err(error) = self.persist() {
            eprintln!(
                "native installer callback persistence failed: {}",
                error.message
            );
            self.errors.lock().unwrap().push(error.message);
        }
        (self.callback)(event)
    }
    fn enforce_cross_user(&self, uid: u32, user: i32, operation: &str) -> Result<(), Exception> {
        self.query(uid, |query| {
            policy::cross_user(query, &self.device.lock().unwrap(), user, false, operation)
        })
    }
    fn check_package(&self, uid: u32, package: Option<&str>) -> Result<(), Exception> {
        self.query(uid, |query| policy::check_package(query, package))
    }
    fn can_query(&self, uid: u32, package: Option<&str>) -> Result<bool, Exception> {
        self.query(uid, |query| policy::can_query(query, package))
    }
    fn can_read_paths(&self, uid: u32) -> Result<bool, Exception> {
        self.query(uid, |query| {
            policy::permission(query, "android.permission.READ_INSTALLED_SESSION_PATHS")
        })
    }
    fn resolved_path(&self, id: i32) -> Result<Option<String>, Exception> {
        let (session, record) = self
            .sessions
            .records()
            .into_iter()
            .find(|(session, _)| session.id == id)
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Unknown session"))?;
        self.disk
            .lock()
            .unwrap()
            .resolved_path(&session, &record)
            .map_err(disk_error)
    }
    fn is_verifier(&self, uid: u32) -> Result<bool, Exception> {
        self.query(uid, |query| {
            policy::permission(query, "android.permission.PACKAGE_VERIFICATION_AGENT")
        })
    }
    fn abandon_stage(&self, ids: &[i32]) -> Result<(), Exception> {
        let records = self.sessions.records();
        let selected: Vec<_> = records
            .iter()
            .filter(|(session, _)| ids.contains(&session.id))
            .cloned()
            .collect();
        self.disk
            .lock()
            .unwrap()
            .abandon(&selected)
            .map_err(disk_error)?;
        self.persist()?;
        for (session, _) in selected {
            (self.callback)(Event::Finished {
                id: session.id,
                user: session.user,
                success: false,
            })
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::model::{PackageState, PackageUserState, User};
    use aim_binder_host::{
        local::Call,
        parcel::{Parcel, Reader},
    };
    use aim_service_aidl::android_content_pm_ipackageinstaller as aidl;
    use std::collections::BTreeSet;
    struct Data(std::path::PathBuf);
    impl Data {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "aim-native-installer-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            for name in ["system", "app", "app-staging"] {
                std::fs::create_dir_all(path.join(name)).unwrap();
            }
            Self(path)
        }
    }
    impl Drop for Data {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn state() -> Arc<State> {
        Arc::new(State {
            packages: [(
                "fixture".into(),
                PackageState {
                    name: "fixture".into(),
                    app_id: 10100,
                    users: [(
                        0,
                        PackageUserState {
                            granted_permissions: vec![
                                "android.permission.INTERACT_ACROSS_USERS".into(),
                            ],
                            ..Default::default()
                        },
                    )]
                    .into(),
                    ..Default::default()
                },
            )]
            .into(),
            users: [
                (
                    0,
                    User {
                        id: 0,
                        ..Default::default()
                    },
                ),
                (
                    10,
                    User {
                        id: 10,
                        ..Default::default()
                    },
                ),
            ]
            .into(),
            ..Default::default()
        })
    }
    fn device() -> DevicePolicy {
        DevicePolicy {
            debuggable: false,
            apex_supported: false,
            rollback_lifetime: true,
            users: [
                (
                    0,
                    policy::UserPolicy {
                        disallow_install_apps: false,
                        disallow_debugging_features: false,
                        organization_managed: false,
                    },
                ),
                (
                    10,
                    policy::UserPolicy {
                        disallow_install_apps: false,
                        disallow_debugging_features: false,
                        organization_managed: false,
                    },
                ),
            ]
            .into(),
            adopted_shell_uids: BTreeSet::new(),
            verifier_uid: None,
            disable_verification_for_uid: None,
            bypass_next_staged_installer_check: false,
            bypass_next_allowed_apex_update_check: false,
        }
    }
    fn disk(data: &Data) -> storage::Store {
        let inode = aim_storage::guest_inode::GuestInode {
            uid: Some(1000),
            gid: Some(1000),
            mode: Some(0o600),
        };
        storage::Store::open(
            data.0.clone(),
            inode,
            aim_storage::guest_inode::GuestInode {
                mode: Some(0o775),
                ..inode
            },
            Arc::new(|path, guest| {
                use std::os::unix::ffi::OsStrExt;
                let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
                let label = if guest.starts_with("/data/app/") {
                    b"u:object_r:apk_tmp_file:s0\0".as_slice()
                } else {
                    b"u:object_r:system_data_file:s0\0".as_slice()
                };
                if unsafe {
                    libc::setxattr(
                        path.as_ptr(),
                        c"dev.aim.xattr.security.selinux".as_ptr(),
                        label.as_ptr().cast(),
                        label.len(),
                        0,
                        libc::XATTR_NOFOLLOW,
                    )
                } != 0
                {
                    return Err(storage::Error {
                        committed: false,
                        message: std::io::Error::last_os_error().to_string(),
                    });
                }
                Ok(())
            }),
        )
        .unwrap()
    }
    fn make_owner(
        data: &Data,
        sessions: Arc<Sessions>,
        nodes: Arc<Mutex<Vec<Arc<dyn Service>>>>,
        events: Arc<Mutex<Vec<Event>>>,
    ) -> Arc<NativeOwners> {
        NativeOwners::open(
            sessions,
            Arc::new(|| Ok(state())),
            device(),
            SystemConfig::default(),
            disk(data),
            Arc::new(move |node| {
                let mut nodes = nodes.lock().unwrap();
                nodes.push(node);
                Ok(Binder::Local(nodes.len() as u64))
            }),
            Arc::new(move |event| events.lock().unwrap().push(event)),
        )
        .unwrap()
    }
    fn request() -> SessionParams {
        SessionParams {
            mode: 1,
            size_bytes: -1,
            originating_uid: -1,
            required_installed_version_code: -1,
            unarchive_id: -1,
            auto_install_dependencies_enabled: true,
            install_flags: 0x00400000,
            app_package_name: Some("fixture".into()),
            ..Default::default()
        }
    }
    fn call(endpoint: &dyn Service, code: u32, uid: u32, parcel: &Parcel) -> Parcel {
        endpoint
            .transact(&mut Call {
                code,
                flags: 0,
                sender_pid: 77,
                sender_euid: uid,
                data: Reader::new(parcel.data(), parcel.objects()),
            })
            .unwrap()
    }
    #[test]
    fn native_binder_create_prepare_reopen_abandon_has_real_files_and_identity() {
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let nodes = Arc::new(Mutex::new(Vec::new()));
        let events = Arc::new(Mutex::new(Vec::new()));
        let owner = make_owner(&data, sessions.clone(), nodes.clone(), events.clone());
        let endpoint = super::super::endpoint::Endpoint {
            sessions: sessions.clone(),
            owners: owner.clone(),
        };
        let mut parcel = Parcel::new();
        aidl::CreateSession {
            params: Some(request()),
            installer_package_name: Some("fixture".into()),
            installer_attribution_tag: Some("tag".into()),
            user_id: 0,
        }
        .write(&mut parcel);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &parcel);
        let id = aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
            .unwrap()
            .unwrap();
        assert!(data.0.join("system/install_sessions.xml").is_file());
        assert!(!data.0.join(format!("app/vmdl{id}.tmp")).exists());
        assert_eq!(
            events.lock().unwrap().as_slice(),
            &[Event::Created { id, user: 0 }]
        );
        let mut parcel = Parcel::new();
        aidl::OpenSession { session_id: id }.write(&mut parcel);
        let reply = call(&endpoint, aidl::OPEN_SESSION, 10101, &parcel);
        assert!(
            aidl::read_open_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .is_err()
        );
        assert!(!data.0.join(format!("app/vmdl{id}.tmp")).exists());
        let reply = call(&endpoint, aidl::OPEN_SESSION, 10100, &parcel);
        assert!(
            aidl::read_open_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .unwrap()
                .is_some()
        );
        let stage = data.0.join(format!("app/vmdl{id}.tmp"));
        assert!(stage.is_dir());
        assert_eq!(
            aim_storage::guest_inode::read(&stage)
                .unwrap()
                .unwrap()
                .mode,
            Some(0o775)
        );
        std::fs::write(
            stage.join("base.apk"),
            b"stage inventory only; commit is unsupported",
        )
        .unwrap();
        let mut parcel = Parcel::new();
        aidl::GetSessionInfo { session_id: id }.write(&mut parcel);
        let reply = call(&endpoint, aidl::GET_SESSION_INFO, 1000, &parcel);
        let info = aidl::read_get_session_info_reply::<super::super::codec::SessionInfo>(
            &mut Reader::new(reply.data(), reply.objects()),
        )
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(
            info.resolved_base_code_path,
            Some(format!("/data/app/vmdl{id}.tmp/base.apk"))
        );
        assert_eq!(info.installer_uid, 10100);
        drop(endpoint);
        drop(owner);
        drop(sessions);
        let recovered = Arc::new(Sessions::default());
        let owner = make_owner(&data, recovered.clone(), nodes, events);
        let endpoint = super::super::endpoint::Endpoint {
            sessions: recovered.clone(),
            owners: owner,
        };
        assert!(recovered.snapshot(id).unwrap().prepared);
        assert_eq!(recovered.snapshot(id).unwrap().active_count, 0);
        let mut parcel = Parcel::new();
        aidl::AbandonSession { session_id: id }.write(&mut parcel);
        let reply = call(&endpoint, aidl::ABANDON_SESSION, 10100, &parcel);
        assert!(
            aidl::read_abandon_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .is_ok()
        );
        assert!(!stage.exists());
        assert!(disk(&data).recovered().unwrap().is_empty());
    }
    #[test]
    fn caller_cannot_claim_other_installer_or_use_partial_cross_user_permission() {
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let owner = make_owner(
            &data,
            sessions.clone(),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
        );
        let endpoint = super::super::endpoint::Endpoint {
            sessions: sessions.clone(),
            owners: owner,
        };
        for (installer, user) in [("other", 0), ("fixture", 10)] {
            let mut parcel = Parcel::new();
            aidl::CreateSession {
                params: Some(request()),
                installer_package_name: Some(installer.into()),
                installer_attribution_tag: None,
                user_id: user,
            }
            .write(&mut parcel);
            let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &parcel);
            assert_eq!(
                aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                    .unwrap()
                    .unwrap_err()
                    .code,
                aim_binder_host::parcel::EX_SECURITY
            );
            assert!(sessions.records().is_empty());
        }
        assert!(!data.0.join("system/install_sessions.xml").exists());
    }
}
