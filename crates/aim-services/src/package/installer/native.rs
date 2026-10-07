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
    local::{LocalProcess, Service},
    parcel::{Binder, EX_ILLEGAL_STATE, Exception},
};
use std::sync::{Arc, Mutex};
pub type QuerySource = Arc<dyn Fn() -> Result<Arc<State>, Exception> + Send + Sync>;
pub type Publisher = Arc<dyn Fn(Arc<dyn Service>) -> Result<Binder, Exception> + Send + Sync>;
pub type PolicySource = Arc<dyn Fn(u32, i32) -> Result<DevicePolicy, Exception> + Send + Sync>;
pub type Callback = Arc<dyn Fn(Event) + Send + Sync>;
pub struct NativeOwners {
    pub sessions: Arc<Sessions>,
    source: QuerySource,
    resolver: Resolver,
    policy_source: PolicySource,
    service_policy: Mutex<policy::ServicePolicy>,
    silent_policy: Mutex<super::silent::Policy>,
    callbacks: super::callbacks::Registry,
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
        policy_source: PolicySource,
        config: SystemConfig,
        disk: storage::Store,
        publisher: Publisher,
        callback: Callback,
        process: Arc<LocalProcess>,
    ) -> Result<Arc<Self>, Exception> {
        let recovered = disk.recovered().map_err(disk_error)?;
        sessions.restore(recovered)?;
        let weak_owner = Arc::new(Mutex::new(std::sync::Weak::<Self>::new()));
        let weak = weak_owner.clone();
        let visibility: super::callbacks::VisibilitySource = Arc::new(move || {
            let owner = weak.lock().unwrap().upgrade().ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "installer callback owner unavailable")
            })?;
            let state = (owner.source)()?;
            let resolution = owner.resolver.resolution(&state).map_err(|error| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("callback query resolution: {error:?}"),
                )
            })?;
            let sessions = owner.sessions.clone();
            Ok(Arc::new(move |uid, id| {
                let Some((session, record)) = sessions
                    .records()
                    .into_iter()
                    .find(|(session, _)| session.id == id)
                else {
                    return Ok(true);
                };
                if session.installer_uid == uid {
                    return Ok(true);
                }
                policy::can_query(
                    &Query {
                        state: &state,
                        filter: &resolution.apps_filter,
                        calling_uid: uid as i32,
                    },
                    record.params.app_package_name.as_deref(),
                )
            }) as super::callbacks::Visibility)
        });
        let callbacks = super::callbacks::Registry::new(process, visibility)?;
        let owner = Arc::new(Self {
            sessions,
            source,
            resolver: Resolver::default(),
            policy_source,
            service_policy: Mutex::new(Default::default()),
            silent_policy: Mutex::new(Default::default()),
            callbacks,
            config,
            disk: Mutex::new(disk),
            publisher,
            callback,
            errors: Mutex::new(Vec::new()),
        });
        *weak_owner.lock().unwrap() = Arc::downgrade(&owner);
        Ok(owner)
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
    pub fn silent_update_allowed(
        &self,
        installer: Option<&str>,
        package: &str,
        uptime_ms: i64,
    ) -> bool {
        self.silent_policy
            .lock()
            .unwrap()
            .allowed(installer, package, uptime_ms)
    }
    pub fn track_silent_update(&self, installer: Option<&str>, package: &str, uptime_ms: i64) {
        self.silent_policy
            .lock()
            .unwrap()
            .track(installer, package, uptime_ms)
    }
    pub fn take_callback_worker(&self) -> Option<super::callbacks::CallbackWorker> {
        self.callbacks.take_worker()
    }
    pub fn shutdown_callbacks(&self) {
        self.callbacks.shutdown();
    }
    /// Asynchronous callback paths expose failed persistence to the service owner.
    pub fn take_errors(&self) -> Vec<String> {
        {
            let mut errors = std::mem::take(&mut *self.errors.lock().unwrap());
            errors.extend(self.callbacks.take_errors());
            errors
        }
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
                &(self.policy_source)(uid, user)?,
                &mut self.service_policy.lock().unwrap(),
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
        self.callbacks.notify(Event::Created { id, user })?;
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
        if let Err(error) = self.callbacks.notify(event.clone()) {
            self.errors.lock().unwrap().push(error.message);
        }
        (self.callback)(event)
    }
    fn enforce_cross_user(&self, uid: u32, user: i32, operation: &str) -> Result<(), Exception> {
        self.query(uid, |query| {
            policy::cross_user(
                query,
                &(self.policy_source)(uid, user)?,
                user,
                false,
                operation,
            )
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
    fn update_label(&self, uid: u32, id: i32, label: Option<String>) -> Result<(), Exception> {
        if let Some(event) = self.sessions.update_label(id, uid, label)? {
            self.persist()?;
            self.callbacks.notify(event.clone())?;
            (self.callback)(event);
        }
        Ok(())
    }
    fn register_callback(
        &self,
        uid: u32,
        callback: Option<Binder>,
        user: i32,
    ) -> Result<(), Exception> {
        self.enforce_cross_user(uid, user, "registerCallback")?;
        self.callbacks.register(callback, uid, user)
    }
    fn unregister_callback(&self, callback: Option<Binder>) -> Result<(), Exception> {
        self.callbacks.unregister(callback)
    }
    fn set_service_policy(
        &self,
        uid: u32,
        setting: super::endpoint::ServiceSetting,
    ) -> Result<(), Exception> {
        if !matches!(uid, 0 | 1000 | 2000) {
            return Err(Exception::security("Operation not allowed for caller"));
        }
        match &setting {
            super::endpoint::ServiceSetting::Unlimited(installer) => {
                self.silent_policy
                    .lock()
                    .unwrap()
                    .set_unlimited(installer.clone());
                return Ok(());
            }
            super::endpoint::ServiceSetting::Throttle(seconds) => {
                self.silent_policy
                    .lock()
                    .unwrap()
                    .set_throttle_seconds(*seconds);
                return Ok(());
            }
            _ => {}
        }
        let mut policy = self.service_policy.lock().unwrap();
        match setting {
            super::endpoint::ServiceSetting::Unlimited(_)
            | super::endpoint::ServiceSetting::Throttle(_) => unreachable!(),
            super::endpoint::ServiceSetting::Staged(value) => {
                policy.bypass_next_staged_installer_check = value
            }
            super::endpoint::ServiceSetting::Apex(value) => {
                policy.bypass_next_allowed_apex_update_check = value
            }
            super::endpoint::ServiceSetting::Verification(uid) => {
                policy.disable_verification_for_uid = (uid != -1).then_some(uid)
            }
        }
        Ok(())
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
            self.callbacks.notify(Event::Finished {
                id: session.id,
                user: session.user,
                success: false,
            })?;
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
        let mut state = State {
            packages: [(
                "fixture".into(),
                PackageState {
                    name: "fixture".into(),
                    app_id: 10100,
                    pkg: Some(Arc::new(crate::package::pkg::AndroidPackage {
                        package_name: "fixture".into(),
                        uid: 10100,
                        target_sdk_version: 35,
                        ..Default::default()
                    })),
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
        };
        state.packages.insert(
            "android".into(),
            PackageState {
                name: "android".into(),
                app_id: 1000,
                pkg: Some(Arc::new(crate::package::pkg::AndroidPackage {
                    package_name: "android".into(),
                    uid: 1000,
                    target_sdk_version: 35,
                    requested_permissions: vec!["android.permission.QUERY_ALL_PACKAGES".into()],
                    ..Default::default()
                })),
                users: [(0, PackageUserState::default())].into(),
                ..Default::default()
            },
        );
        Arc::new(state)
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
        make_owner_with_policy(data, sessions, nodes, events, Arc::new(|_, _| Ok(device())))
    }
    fn make_owner_with_policy(
        data: &Data,
        sessions: Arc<Sessions>,
        nodes: Arc<Mutex<Vec<Arc<dyn Service>>>>,
        events: Arc<Mutex<Vec<Event>>>,
        policy_source: PolicySource,
    ) -> Arc<NativeOwners> {
        NativeOwners::open(
            sessions,
            Arc::new(|| Ok(state())),
            policy_source,
            SystemConfig::default(),
            disk(data),
            Arc::new(move |node| {
                let mut nodes = nodes.lock().unwrap();
                nodes.push(node);
                Ok(Binder::Local(nodes.len() as u64))
            }),
            Arc::new(move |event| events.lock().unwrap().push(event)),
            LocalProcess::open(
                &aim_binder_driver::Driver::new(),
                aim_binder_driver::Device::Binder,
                aim_binder_driver::Credentials {
                    pid: 77701,
                    euid: 1000,
                    security_context: None,
                },
            ),
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
    #[test]
    fn every_create_reads_current_user_policy_and_owned_oneoff_is_not_rearmed() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let denied = Arc::new(AtomicBool::new(false));
        let reads = Arc::new(AtomicUsize::new(0));
        let flag = denied.clone();
        let count = reads.clone();
        let owner = make_owner_with_policy(
            &data,
            sessions.clone(),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(move |uid, user| {
                assert_eq!((uid, user), (10100, 0));
                count.fetch_add(1, Ordering::SeqCst);
                let mut policy = device();
                policy.users.get_mut(&user).unwrap().disallow_install_apps =
                    flag.load(Ordering::SeqCst);
                Ok(policy)
            }),
        );
        let worker = owner.take_callback_worker().unwrap();
        let endpoint = super::super::endpoint::Endpoint {
            sessions: sessions.clone(),
            owners: owner.clone(),
        };
        let mut parcel = Parcel::new();
        aidl::DisableVerificationForUid { uid: 10100 }.write(&mut parcel);
        let reply = call(
            &endpoint,
            aidl::DISABLE_VERIFICATION_FOR_UID,
            10100,
            &parcel,
        );
        assert!(
            aidl::read_disable_verification_for_uid_reply(&mut Reader::new(
                reply.data(),
                reply.objects()
            ))
            .unwrap()
            .is_err()
        );
        let reply = call(&endpoint, aidl::DISABLE_VERIFICATION_FOR_UID, 1000, &parcel);
        assert!(
            aidl::read_disable_verification_for_uid_reply(&mut Reader::new(
                reply.data(),
                reply.objects()
            ))
            .unwrap()
            .is_ok()
        );
        let mut parcel = Parcel::new();
        aidl::CreateSession {
            params: Some(request()),
            installer_package_name: Some("fixture".into()),
            installer_attribution_tag: None,
            user_id: 0,
        }
        .write(&mut parcel);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &parcel);
        let first =
            aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .unwrap();
        assert!(sessions.snapshot(first).unwrap().parameters.install_flags & 0x80000 != 0);
        denied.store(true, Ordering::SeqCst);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &parcel);
        assert_eq!(
            aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .unwrap_err()
                .code,
            aim_binder_host::parcel::EX_SECURITY
        );
        assert_eq!(sessions.records().len(), 1);
        denied.store(false, Ordering::SeqCst);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &parcel);
        let second =
            aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .unwrap();
        assert_eq!(
            sessions.snapshot(second).unwrap().parameters.install_flags & 0x80000,
            0
        );
        assert_eq!(reads.load(Ordering::SeqCst), 3);
        owner.shutdown_callbacks();
        drop(endpoint);
        drop(owner);
        drop(worker);
    }
    #[test]
    fn callback_endpoint_rejects_trailing_registration_before_retaining_node() {
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let driver = aim_binder_driver::Driver::new();
        let process = LocalProcess::open(
            &driver,
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: 77901,
                euid: 1000,
                security_context: None,
            },
        );
        let owner = NativeOwners::open(
            sessions.clone(),
            Arc::new(|| Ok(state())),
            Arc::new(|_, _| Ok(device())),
            SystemConfig::default(),
            disk(&data),
            Arc::new(|_| panic!("registration does not publish a session")),
            Arc::new(|_| panic!("registration emits no session event")),
            process.clone(),
        )
        .unwrap();
        let worker = owner.take_callback_worker().unwrap();
        let endpoint = super::super::endpoint::Endpoint {
            sessions,
            owners: owner.clone(),
        };
        struct Callback;
        impl Service for Callback {
            fn descriptor(&self) -> &str {
                "android.content.pm.IPackageInstallerCallback"
            }
            fn transact(&self, _: &mut Call<'_>) -> aim_binder_host::local::Reply {
                panic!("no callback queued")
            }
        }
        let binder = process.add_service(Arc::new(Callback));
        let mut parcel = Parcel::new();
        aidl::RegisterCallback {
            callback: Some(binder),
            user_id: 0,
        }
        .write(&mut parcel);
        parcel.write_i32(1);
        assert_eq!(
            endpoint
                .transact(&mut Call {
                    code: aidl::REGISTER_CALLBACK,
                    flags: 0,
                    sender_pid: 1,
                    sender_euid: 10100,
                    data: Reader::new(parcel.data(), parcel.objects())
                })
                .unwrap_err(),
            aim_binder_host::parcel::BAD_VALUE
        );
        assert_eq!(owner.callbacks.len(), 0);
        let mut parcel = Parcel::new();
        aidl::RegisterCallback {
            callback: Some(binder),
            user_id: 0,
        }
        .write(&mut parcel);
        let reply = call(&endpoint, aidl::REGISTER_CALLBACK, 10100, &parcel);
        assert!(
            aidl::read_register_callback_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .is_ok()
        );
        assert_eq!(owner.callbacks.len(), 1);
        let mut parcel = Parcel::new();
        aidl::UnregisterCallback {
            callback: Some(binder),
        }
        .write(&mut parcel);
        let reply = call(&endpoint, aidl::UNREGISTER_CALLBACK, 10100, &parcel);
        assert!(
            aidl::read_unregister_callback_reply(&mut Reader::new(reply.data(), reply.objects()))
                .unwrap()
                .is_ok()
        );
        assert_eq!(owner.callbacks.len(), 0);
        owner.shutdown_callbacks();
        drop(endpoint);
        drop(owner);
        drop(worker);
        driver.release(process.proc_handle());
    }
    #[test]
    fn binder_label_change_persists_without_trimming_and_noop_emits_no_badging() {
        let data = Data::new();
        let sessions = Arc::new(Sessions::default());
        let events = Arc::new(Mutex::new(Vec::new()));
        let owner = make_owner(
            &data,
            sessions.clone(),
            Arc::new(Mutex::new(Vec::new())),
            events.clone(),
        );
        let worker = owner.take_callback_worker().unwrap();
        let endpoint = super::super::endpoint::Endpoint {
            sessions: sessions.clone(),
            owners: owner.clone(),
        };
        let mut parcel = Parcel::new();
        aidl::CreateSession {
            params: Some(request()),
            installer_package_name: Some("fixture".into()),
            installer_attribution_tag: None,
            user_id: 0,
        }
        .write(&mut parcel);
        let reply = call(&endpoint, aidl::CREATE_SESSION, 10100, &parcel);
        let id = aidl::read_create_session_reply(&mut Reader::new(reply.data(), reply.objects()))
            .unwrap()
            .unwrap();
        let original = std::fs::read(data.0.join("system/install_sessions.xml")).unwrap();
        for (uid, label, expected) in [
            (
                10101,
                Some("other".into()),
                aim_binder_host::parcel::EX_SECURITY,
            ),
            (10100, None, aim_binder_host::parcel::EX_NULL_POINTER),
        ] {
            let mut parcel = Parcel::new();
            aidl::UpdateSessionAppLabel {
                session_id: id,
                app_label: label,
            }
            .write(&mut parcel);
            let reply = call(&endpoint, aidl::UPDATE_SESSION_APP_LABEL, uid, &parcel);
            assert_eq!(
                aidl::read_update_session_app_label_reply(&mut Reader::new(
                    reply.data(),
                    reply.objects()
                ))
                .unwrap()
                .unwrap_err()
                .code,
                expected
            );
            assert_eq!(
                std::fs::read(data.0.join("system/install_sessions.xml")).unwrap(),
                original
            );
        }
        let label = "<label & 😀>".repeat(120);
        let mut parcel = Parcel::new();
        aidl::UpdateSessionAppLabel {
            session_id: id,
            app_label: Some(label.clone()),
        }
        .write(&mut parcel);
        parcel.write_i32(123);
        assert_eq!(
            endpoint
                .transact(&mut Call {
                    code: aidl::UPDATE_SESSION_APP_LABEL,
                    flags: 0,
                    sender_pid: 1,
                    sender_euid: 10100,
                    data: Reader::new(parcel.data(), parcel.objects())
                })
                .unwrap_err(),
            aim_binder_host::parcel::BAD_VALUE
        );
        assert_eq!(
            std::fs::read(data.0.join("system/install_sessions.xml")).unwrap(),
            original
        );
        let mut parcel = Parcel::new();
        aidl::UpdateSessionAppLabel {
            session_id: id,
            app_label: Some(label.clone()),
        }
        .write(&mut parcel);
        for _ in 0..2 {
            let reply = call(&endpoint, aidl::UPDATE_SESSION_APP_LABEL, 10100, &parcel);
            assert!(
                aidl::read_update_session_app_label_reply(&mut Reader::new(
                    reply.data(),
                    reply.objects()
                ))
                .unwrap()
                .is_ok()
            );
        }
        assert_eq!(
            events.lock().unwrap().as_slice(),
            &[
                Event::Created { id, user: 0 },
                Event::Badging { id, user: 0 }
            ]
        );
        let records = disk(&data).recovered().unwrap();
        assert_eq!(
            records[0].1.params.app_label.as_deref(),
            Some(label.as_str())
        );
        owner.shutdown_callbacks();
        drop(endpoint);
        drop(owner);
        drop(worker);
    }
    #[test]
    fn binder_silent_policy_setters_enforce_identity_and_update_real_tracker() {
        let data = Data::new();
        let owner = make_owner(
            &data,
            Arc::new(Sessions::default()),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
        );
        let worker = owner.take_callback_worker().unwrap();
        let endpoint = super::super::endpoint::Endpoint {
            sessions: owner.sessions.clone(),
            owners: owner.clone(),
        };
        owner.track_silent_update(Some("installer"), "app", 40_000);
        let mut parcel = Parcel::new();
        aidl::SetAllowUnlimitedSilentUpdates {
            installer_package_name: None,
        }
        .write(&mut parcel);
        let reply = call(
            &endpoint,
            aidl::SET_ALLOW_UNLIMITED_SILENT_UPDATES,
            10100,
            &parcel,
        );
        assert!(
            aidl::read_set_allow_unlimited_silent_updates_reply(&mut Reader::new(
                reply.data(),
                reply.objects()
            ))
            .unwrap()
            .is_err()
        );
        assert!(!owner.silent_update_allowed(Some("installer"), "app", 50_000));
        let reply = call(
            &endpoint,
            aidl::SET_ALLOW_UNLIMITED_SILENT_UPDATES,
            1000,
            &parcel,
        );
        assert!(
            aidl::read_set_allow_unlimited_silent_updates_reply(&mut Reader::new(
                reply.data(),
                reply.objects()
            ))
            .unwrap()
            .is_ok()
        );
        assert!(owner.silent_update_allowed(Some("installer"), "app", 50_000));
        owner.track_silent_update(Some("installer"), "app", 50_000);
        let mut parcel = Parcel::new();
        aidl::SetSilentUpdatesThrottleTime {
            throttle_time_in_seconds: 0,
        }
        .write(&mut parcel);
        parcel.write_i32(1);
        assert_eq!(
            endpoint
                .transact(&mut Call {
                    code: aidl::SET_SILENT_UPDATES_THROTTLE_TIME,
                    flags: 0,
                    sender_pid: 1,
                    sender_euid: 1000,
                    data: Reader::new(parcel.data(), parcel.objects())
                })
                .unwrap_err(),
            aim_binder_host::parcel::BAD_VALUE
        );
        assert!(!owner.silent_update_allowed(Some("installer"), "app", 50_001));
        let mut parcel = Parcel::new();
        aidl::SetSilentUpdatesThrottleTime {
            throttle_time_in_seconds: 0,
        }
        .write(&mut parcel);
        let reply = call(
            &endpoint,
            aidl::SET_SILENT_UPDATES_THROTTLE_TIME,
            2000,
            &parcel,
        );
        assert!(
            aidl::read_set_silent_updates_throttle_time_reply(&mut Reader::new(
                reply.data(),
                reply.objects()
            ))
            .unwrap()
            .is_ok()
        );
        assert!(owner.silent_update_allowed(Some("installer"), "app", 50_001));
        assert!(!owner.silent_update_allowed(Some("installer"), "app", 50_000));
        owner.shutdown_callbacks();
        drop(endpoint);
        drop(owner);
        drop(worker);
    }
}
