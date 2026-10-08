//! One actual native package constructor epoch, driven by generated BootSession AIDL.
use super::{boot_captures::Captures, bootstrap::Bridge, early_user_operations::Endpoint};
use crate::system::{
    System,
    package_boot_scan::{RawScanned, Scanned},
    package_runtime::Runtime,
};
use aim_binder_host::{
    local::{Call, LocalProcess, Reply, Service},
    parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel, UNKNOWN_TRANSACTION},
};
use aim_service_aidl::{WriteParcelable, dev_aim_server_ipackagebootsession as api};
use std::sync::{
    Arc, Mutex, Weak,
    atomic::{AtomicBool, Ordering},
};
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Early,
    RawScanning,
    RawScanned,
    Scanning,
    Scanned,
    Constructing,
    Ready,
    Failed,
    Closed,
}
struct State {
    phase: Phase,
    raw: Option<Arc<RawScanned>>,
    scanned: Option<Scanned>,
    runtime: Option<Arc<Runtime>>,
    services: Option<(Binder, Binder)>,
}
pub struct Session {
    system: Weak<System>,
    bridge: Arc<Bridge>,
    process: Arc<LocalProcess>,
    early: Arc<Endpoint>,
    user_operations: Binder,
    internal_host: Binder,
    factory_test: bool,
    captures: Captures,
    raw_metadata_leases: Mutex<Vec<Weak<super::scan_snapshot::endpoint::Endpoint>>>,
    closed: AtomicBool,
    state: Mutex<State>,
}
fn illegal(message: impl Into<String>) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}
impl Session {
    pub fn new(
        system: &Arc<System>,
        bridge: Arc<Bridge>,
        early: Arc<Endpoint>,
        user_operations: Binder,
        internal_host: Binder,
        factory_test: bool,
    ) -> Arc<Self> {
        let process = system.binder_process();
        let session = Arc::new(Self {
            system: Arc::downgrade(system),
            bridge: bridge.clone(),
            process: process.clone(),
            early,
            user_operations,
            internal_host,
            factory_test,
            captures: Captures::new(system, bridge.clone()),
            raw_metadata_leases: Mutex::new(Vec::new()),
            closed: AtomicBool::new(false),
            state: Mutex::new(State {
                phase: Phase::Early,
                raw: None,
                scanned: None,
                runtime: None,
                services: None,
            }),
        });
        // System.attach_package_bootstrap owns the single death registration
        // for this node reference and closes this epoch's Session on death.
        session
    }
    fn system(&self) -> Result<Arc<System>, Exception> {
        if self.closed.load(Ordering::Acquire) {
            return Err(illegal("native boot session closed"));
        }
        let system = self
            .system
            .upgrade()
            .ok_or_else(|| illegal("native boot System stopped"))?;
        system.check_package_bootstrap(&self.bridge)?;
        Ok(system)
    }
    fn require_scan(&self) -> Result<Arc<System>, Exception> {
        let system = self.system()?;
        let state = self.state.lock().unwrap();
        if !matches!(
            state.phase,
            Phase::Scanned | Phase::Constructing | Phase::Ready
        ) {
            return Err(illegal("native initial scan has not completed"));
        }
        Ok(system)
    }
    fn require_runtime(&self) -> Result<(Arc<System>, Arc<Runtime>), Exception> {
        let system = self.system()?;
        let state = self.state.lock().unwrap();
        if state.phase != Phase::Ready {
            return Err(illegal("native package construction has not completed"));
        }
        let runtime = state
            .runtime
            .clone()
            .ok_or_else(|| illegal("native runtime owner unavailable"))?;
        Ok((system, runtime))
    }
    fn require_raw(&self) -> Result<Arc<RawScanned>, Exception> {
        self.system()?;
        let state = self.state.lock().unwrap();
        if !matches!(state.phase, Phase::RawScanned | Phase::Scanning) {
            return Err(illegal(
                "native raw scan unavailable outside initial projection",
            ));
        }
        state
            .raw
            .clone()
            .ok_or_else(|| illegal("actual raw scan owner absent"))
    }
    fn fail_and_close(&self, mut error: Exception) -> Exception {
        {
            let mut state = self.state.lock().unwrap();
            if !self.closed.load(Ordering::Acquire) {
                state.phase = Phase::Failed;
            }
        }
        if let Err(cleanup) = self.close() {
            error
                .message
                .push_str(&format!("; boot cleanup: {}", cleanup.message));
        }
        error
    }
    fn finish_raw_scan(&self) -> Result<(), Exception> {
        let system = self.system()?;
        {
            let mut state = self.state.lock().unwrap();
            if state.phase != Phase::Early {
                return Err(illegal("raw scan requires untouched early session"));
            }
            state.phase = Phase::RawScanning;
        }
        let begin = match system.prepare_native_boot_session_scan(
            &self.bridge,
            self.factory_test,
            &self.early,
        ) {
            Ok(begin) => begin,
            Err(error) => return Err(self.fail_and_close(error)),
        };
        if let Err(error) = self.system() {
            drop(begin);
            return Err(self.fail_and_close(error));
        }
        match system.finish_native_boot_session_raw_scan(&self.bridge, begin, &self.early) {
            Ok(raw) => {
                self.system()?;
                let raw = Arc::new(raw);
                let mut state = self.state.lock().unwrap();
                if state.phase != Phase::RawScanning {
                    drop(state);
                    drop(raw);
                    return Err(illegal("raw scan epoch changed"));
                }
                state.raw = Some(raw);
                state.phase = Phase::RawScanned;
                Ok(())
            }
            Err(error) => Err(self.fail_and_close(error)),
        }
    }
    fn finish_scan(&self) -> Result<(), Exception> {
        let system = self.system()?;
        let raw = {
            let mut state = self.state.lock().unwrap();
            if state.phase != Phase::RawScanned {
                return Err(illegal(
                    "initial projection requires completed raw scan exactly once",
                ));
            }
            state.phase = Phase::Scanning;
            state
                .raw
                .clone()
                .ok_or_else(|| illegal("actual raw scan owner absent"))?
        };
        match system.finish_native_boot_session_initial_scan(&self.bridge, raw, &self.early) {
            Ok(scanned) => {
                self.system()?;
                let mut state = self.state.lock().unwrap();
                if state.phase != Phase::Scanning {
                    drop(state);
                    drop(scanned);
                    return Err(illegal("initial projection epoch changed"));
                }
                state.scanned = Some(scanned);
                state.phase = Phase::Scanned;
                let raw = state.raw.take();
                drop(state);
                drop(raw);
                Ok(())
            }
            Err(error) => Err(self.fail_and_close(error)),
        }
    }
    fn finish_construction(&self) -> Result<(), Exception> {
        let system = self.system()?;
        let scanned = {
            let mut state = self.state.lock().unwrap();
            if state.phase != Phase::Scanned {
                return Err(illegal(
                    "construction requires the completed initial scan exactly once",
                ));
            }
            state.phase = Phase::Constructing;
            state
                .scanned
                .take()
                .ok_or_else(|| illegal("actual scanned owner absent"))?
        };
        let result = system.finish_native_boot_session_construction(
            &self.bridge,
            scanned,
            &self.early,
            self.user_operations,
            self.factory_test,
        );
        match result {
            Ok(runtime) => {
                let runtime = Arc::new(runtime);
                self.system()?;
                let services = match system.publish_native_package_services(&self.bridge, &runtime)
                {
                    Ok(services) => services,
                    Err(mut error) => {
                        let mut state = self.state.lock().unwrap();
                        if !self.closed.load(Ordering::Acquire) {
                            state.phase = Phase::Failed;
                        }
                        drop(state);
                        if let Err(cleanup) = self.close() {
                            error.message.push_str(&format!(
                                "; boot publication cleanup: {}",
                                cleanup.message
                            ));
                        }
                        drop(runtime);
                        return Err(error);
                    }
                };
                let mut state = self.state.lock().unwrap();
                if state.phase != Phase::Constructing {
                    drop(state);
                    drop(runtime);
                    return Err(illegal("construction epoch changed"));
                }
                state.runtime = Some(runtime);
                state.services = Some(services);
                state.phase = Phase::Ready;
                Ok(())
            }
            Err(mut error) => {
                let mut state = self.state.lock().unwrap();
                if !self.closed.load(Ordering::Acquire) {
                    state.phase = Phase::Failed;
                }
                drop(state);
                if let Err(cleanup) = self.close() {
                    error
                        .message
                        .push_str(&format!("; construction cleanup: {}", cleanup.message));
                }
                Err(error)
            }
        }
    }
    fn current_bootstrap_state(&self) -> Result<Vec<u8>, Exception> {
        let system = self.require_scan()?;
        let capture = system.capture_package_queries()?;
        let version = i64::try_from(capture.scan().version())
            .map_err(|_| illegal("boot version exceeds Java long"))?;
        self.captures.bootstrap_state(version)
    }
    fn service_binder(&self, native: bool) -> Result<Binder, Exception> {
        self.require_runtime()?;
        let state = self.state.lock().unwrap();
        let (public, package_native) = state
            .services
            .ok_or_else(|| illegal("actual package service binders unavailable"))?;
        Ok(if native { package_native } else { public })
    }
    /// Invalidate endpoints first, then release/join Runtime guards outside all
    /// session/System locks. The original bridge's death retires this epoch too.
    pub fn close(&self) -> Result<(), Exception> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.early.close();
        self.captures.close();
        for endpoint in self.raw_metadata_leases.lock().unwrap().drain(..).filter_map(|lease|lease.upgrade()){endpoint.close_lease();}
        let (raw, scanned, runtime) = {
            let mut state = self.state.lock().unwrap();
            state.phase = Phase::Closed;
            state.services = None;
            (state.raw.take(), state.scanned.take(), state.runtime.take())
        };
        let retired = if let Some(system) = self.system.upgrade() {
            if system.check_package_bootstrap(&self.bridge).is_ok() {
                system.detach_package_bootstrap(&self.bridge)
            } else {
                Ok(())
            }
        } else {
            Ok(())
        };
        drop(runtime);
        drop(raw);
        drop(scanned);
        retired
    }
}
struct VersionDescriptor(aim_binder_driver::File);
impl WriteParcelable for VersionDescriptor {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(0);
        p.write_file(self.0.clone());
    }
}
impl Service for Session {
    fn descriptor(&self) -> &str {
        api::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if !api::METHODS.iter().any(|(code, _)| *code == call.code) {
            return Err(UNKNOWN_TRANSACTION);
        }
        let start = call.data.position();
        call.data.enforce_interface(api::DESCRIPTOR)?;
        call.data.set_position(start);
        if call.sender_euid != crate::SYSTEM_UID {
            let mut reply = Parcel::new();
            reply.write_exception(&Exception::security(
                "native boot session serves system UID only",
            ));
            return Ok(reply);
        }
        enum Action {
            Internal,
            Users,
            DexoptCompletion,
            RawScan,
            RawInstant(i32),
            RawPackage(Option<String>, i32, i32, bool),
            RawUid(i32, i32),
            Scan,
            Construct,
            Snapshot,
            InitialMetadata,
            InitialKnown(i32,i32),
            InitialDefinitions,
            InitialCrossUser,
            InitialRuntime(i32),
            InitialRuntimeVersion(i32),
            InitialUpgrading,
            InitialAdmissions,
            Version,
            Public,
            Native,
            State,
            Historical(i64),
            Name(i64, Option<String>, i32, i32),
            Filter(i64, Option<String>, i32, i32),
            Lifecycle(i32),
            Metrics(Option<Vec<u8>>),
            Wait,
            Close,
        }
        let action = match call.code {
            api::GET_INTERNAL_HOST => {
                api::GetInternalHost::read(&mut call.data)?;
                Action::Internal
            }
            api::GET_DEXOPT_COMPLETION => {
                api::GetDexoptCompletion::read(&mut call.data)?;
                Action::DexoptCompletion
            }
            api::GET_USER_OPERATIONS => {
                api::GetUserOperations::read(&mut call.data)?;
                Action::Users
            }
            api::FINISH_RAW_SCAN => {
                api::FinishRawScan::read(&mut call.data)?;
                Action::RawScan
            }
            api::GET_INITIAL_INSTANT_APP_PACKAGE_NAME => {
                let args = api::GetInitialInstantAppPackageName::read(&mut call.data)?;
                Action::RawInstant(args.uid)
            }
            api::FILTER_INITIAL_PACKAGE_ACCESS => {
                let args = api::FilterInitialPackageAccess::read(&mut call.data)?;
                Action::RawPackage(
                    args.package_name,
                    args.calling_uid,
                    args.user_id,
                    args.filter_uninstalled,
                )
            }
            api::FILTER_INITIAL_UID_ACCESS => {
                let args = api::FilterInitialUidAccess::read(&mut call.data)?;
                Action::RawUid(args.uid, args.calling_uid)
            }
            api::FINISH_INITIAL_SCAN => {
                api::FinishInitialScan::read(&mut call.data)?;
                Action::Scan
            }
            api::FINISH_CONSTRUCTION => {
                api::FinishConstruction::read(&mut call.data)?;
                Action::Construct
            }
            api::GET_INITIAL_KNOWN_PACKAGE_NAMES=>{
                let args=api::GetInitialKnownPackageNames::read(&mut call.data)?;
                Action::InitialKnown(args.kind,args.user_id)
            }
            api::GET_INITIAL_LEGACY_PERMISSION_DEFINITIONS_RECORD=>{
                api::GetInitialLegacyPermissionDefinitionsRecord::read(&mut call.data)?;
                Action::InitialDefinitions
            }
            api::GET_INITIAL_CROSS_USER_SUSPENSIONS=>{
                api::GetInitialCrossUserSuspensions::read(&mut call.data)?;
                Action::InitialCrossUser
            }
            api::GET_INITIAL_LEGACY_RUNTIME_PERMISSIONS_STATE_RECORD=>{
                let args=api::GetInitialLegacyRuntimePermissionsStateRecord::read(&mut call.data)?;
                Action::InitialRuntime(args.user_id)
            }
            api::GET_INITIAL_LEGACY_PERMISSIONS_VERSION=>{
                let args=api::GetInitialLegacyPermissionsVersion::read(&mut call.data)?;
                Action::InitialRuntimeVersion(args.user_id)
            }
            api::GET_INITIAL_DEVICE_UPGRADING=>{
                api::GetInitialDeviceUpgrading::read(&mut call.data)?;
                Action::InitialUpgrading
            }
            api::GET_INITIAL_PERMISSION_ADMISSIONS_RECORD=>{
                api::GetInitialPermissionAdmissionsRecord::read(&mut call.data)?;
                Action::InitialAdmissions
            }
            api::GET_INITIAL_METADATA_SNAPSHOT=>{
                api::GetInitialMetadataSnapshot::read(&mut call.data)?;
                Action::InitialMetadata
            }
            api::GET_SNAPSHOT => {
                api::GetSnapshot::read(&mut call.data)?;
                Action::Snapshot
            }
            api::GET_VERSION_PAGE => {
                api::GetVersionPage::read(&mut call.data)?;
                Action::Version
            }
            api::GET_PACKAGE_SERVICE => {
                api::GetPackageService::read(&mut call.data)?;
                Action::Public
            }
            api::GET_PACKAGE_NATIVE_SERVICE => {
                api::GetPackageNativeService::read(&mut call.data)?;
                Action::Native
            }
            api::GET_BOOTSTRAP_STATE => {
                api::GetBootstrapState::read(&mut call.data)?;
                Action::State
            }
            api::GET_BOOTSTRAP_STATE_FOR_VERSION => {
                let args = api::GetBootstrapStateForVersion::read(&mut call.data)?;
                Action::Historical(args.version)
            }
            api::FILTER_PACKAGE_NAME => {
                let args = api::FilterPackageName::read(&mut call.data)?;
                Action::Name(
                    args.version,
                    args.package_name,
                    args.calling_uid,
                    args.user_id,
                )
            }
            api::SHOULD_FILTER => {
                let args = api::ShouldFilter::read(&mut call.data)?;
                Action::Filter(
                    args.version,
                    args.package_name,
                    args.calling_uid,
                    args.user_id,
                )
            }
            api::RUN_LIFECYCLE => {
                let args = api::RunLifecycle::read(&mut call.data)?;
                Action::Lifecycle(args.stage)
            }
            api::UPDATE_METRICS => {
                let args = api::UpdateMetrics::read(&mut call.data)?;
                Action::Metrics(args.metrics_record)
            }
            api::WAIT_FOR_APP_DATA_PREPARED => {
                api::WaitForAppDataPrepared::read(&mut call.data)?;
                Action::Wait
            }
            api::CLOSE => {
                api::Close::read(&mut call.data)?;
                Action::Close
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        if call.data.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        let mut reply = Parcel::new();
        let result: Result<(), Exception> = (|| {
            match action {
                Action::Internal => {
                    self.system()?;
                    api::write_get_internal_host_reply(&mut reply, Some(self.internal_host));
                }
                Action::DexoptCompletion => {
                    let system = self.require_scan()?;
                    let binder = system.package_dexopt_completion(&self.bridge)?;
                    api::write_get_dexopt_completion_reply(&mut reply, Some(binder));
                }
                Action::Users => {
                    self.system()?;
                    api::write_get_user_operations_reply(&mut reply, Some(self.user_operations));
                }
                Action::RawScan => {
                    self.finish_raw_scan()?;
                    api::write_finish_raw_scan_reply(&mut reply);
                }
                Action::RawInstant(uid) => {
                    api::write_get_initial_instant_app_package_name_reply(
                        &mut reply,
                        &self.require_raw()?.initial_instant(uid)?,
                    );
                }
                Action::RawPackage(name, uid, user, filter) => {
                    api::write_filter_initial_package_access_reply(
                        &mut reply,
                        self.require_raw()?
                            .filter_package(name.as_deref(), uid, user, filter)?,
                    );
                }
                Action::RawUid(uid, caller) => {
                    api::write_filter_initial_uid_access_reply(
                        &mut reply,
                        self.require_raw()?.filter_uid(uid, caller)?,
                    );
                }
                Action::Scan => {
                    self.finish_scan()?;
                    api::write_finish_initial_scan_reply(&mut reply);
                }
                Action::Construct => {
                    self.finish_construction()?;
                    api::write_finish_construction_reply(&mut reply);
                }
                Action::InitialKnown(kind,user)=>{
                    let names=self.require_raw()?.known_package_names(kind,user)?;
                    api::write_get_initial_known_package_names_reply(&mut reply,&Some(names));
                }
                Action::InitialDefinitions=>{
                    let bytes=self.require_raw()?.legacy_permission_definitions_record()?;
                    api::write_get_initial_legacy_permission_definitions_record_reply(&mut reply,&Some(bytes));
                }
                Action::InitialCrossUser=>{
                    let enabled=self.require_raw()?.cross_user_suspensions()?;
                    api::write_get_initial_cross_user_suspensions_reply(&mut reply,enabled);
                }
                Action::InitialRuntime(user)=>{
                    let bytes=self.require_raw()?.legacy_runtime_permissions_state_record(user)?;
                    api::write_get_initial_legacy_runtime_permissions_state_record_reply(&mut reply,&Some(bytes));
                }
                Action::InitialRuntimeVersion(user)=>{
                    let version=self.require_raw()?.legacy_permissions_version(user)?;
                    api::write_get_initial_legacy_permissions_version_reply(&mut reply,version);
                }
                Action::InitialUpgrading=>{
                    let upgrading=self.require_raw()?.device_upgrading()?;
                    api::write_get_initial_device_upgrading_reply(&mut reply,upgrading);
                }
                Action::InitialAdmissions=>{
                    let bytes=self.require_raw()?.permission_admissions_record()?;
                    api::write_get_initial_permission_admissions_record_reply(&mut reply,&Some(bytes));
                }
                Action::InitialMetadata=>{
                    let raw=self.require_raw()?;
                    let snapshot=raw.metadata_snapshot()?;
                    let endpoint=Arc::new(super::scan_snapshot::endpoint::Endpoint::raw_metadata(snapshot));
                    let mut leases=self.raw_metadata_leases.lock().unwrap();
                    if self.closed.load(Ordering::Acquire){return Err(illegal("native raw metadata epoch closed"));}
                    leases.retain(|lease|lease.strong_count()!=0);leases.push(Arc::downgrade(&endpoint));
                    let binder=self.process.add_service(endpoint);
                    api::write_get_initial_metadata_snapshot_reply(&mut reply,Some(binder));
                }
                Action::Snapshot => {
                    self.require_scan()?;
                    api::write_get_snapshot_reply(&mut reply, Some(self.captures.snapshot()?));
                }
                Action::Version => {
                    let system = self.require_scan()?;
                    let file = VersionDescriptor(system.package_state_version_page()?);
                    api::write_get_version_page_reply(&mut reply, Some(&file));
                }
                Action::Public => {
                    api::write_get_package_service_reply(
                        &mut reply,
                        Some(self.service_binder(false)?),
                    );
                }
                Action::Native => {
                    api::write_get_package_native_service_reply(
                        &mut reply,
                        Some(self.service_binder(true)?),
                    );
                }
                Action::State => {
                    api::write_get_bootstrap_state_reply(
                        &mut reply,
                        &Some(self.current_bootstrap_state()?),
                    );
                }
                Action::Historical(version) => {
                    self.require_scan()?;
                    api::write_get_bootstrap_state_for_version_reply(
                        &mut reply,
                        &Some(self.captures.bootstrap_state(version)?),
                    );
                }
                Action::Name(version, name, uid, user) => {
                    self.require_scan()?;
                    let name = name.ok_or_else(|| {
                        Exception::new(
                            aim_binder_host::parcel::EX_NULL_POINTER,
                            "packageName is null",
                        )
                    })?;
                    api::write_filter_package_name_reply(
                        &mut reply,
                        &self
                            .captures
                            .filter_package_name(version, &name, uid, user)?,
                    );
                }
                Action::Filter(version, name, uid, user) => {
                    self.require_scan()?;
                    let name = name.ok_or_else(|| {
                        Exception::new(
                            aim_binder_host::parcel::EX_NULL_POINTER,
                            "packageName is null",
                        )
                    })?;
                    api::write_should_filter_reply(
                        &mut reply,
                        self.captures.should_filter(version, &name, uid, user)?,
                    );
                }
                Action::Lifecycle(stage) => {
                    let (system, _runtime) = self.require_runtime()?;
                    system.run_native_package_lifecycle(stage)?;
                    api::write_run_lifecycle_reply(&mut reply);
                }
                Action::Metrics(record) => {
                    let (system, _runtime) = self.require_runtime()?;
                    system
                        .update_native_package_metrics(&record.ok_or_else(|| {
                            Exception::illegal_argument("metrics record is null")
                        })?)?;
                    api::write_update_metrics_reply(&mut reply);
                }
                Action::Wait => {
                    let (system, _runtime) = self.require_runtime()?;
                    system.wait_native_package_app_data()?;
                    api::write_wait_for_app_data_prepared_reply(&mut reply);
                }
                Action::Close => {
                    self.close()?;
                    api::write_close_reply(&mut reply);
                }
            }
            Ok(())
        })();
        if let Err(error) = result {
            reply = Parcel::new();
            reply.write_exception(&error);
        }
        Ok(reply)
    }
}

#[cfg(test)]
impl Session {
    pub(crate) fn epoch_fixture_set_scanned(&self,scanned:Scanned,metadata:&Arc<super::scan_snapshot::endpoint::Endpoint>){
        let mut state=self.state.lock().unwrap();
        assert!(state.phase==Phase::Early);
        state.scanned=Some(scanned);state.phase=Phase::Scanned;
        self.raw_metadata_leases.lock().unwrap().push(Arc::downgrade(metadata));
    }
    pub(crate) fn epoch_fixture_is_closed_and_empty(&self)->bool{
        let state=self.state.lock().unwrap();
        self.closed.load(Ordering::Acquire)&&state.phase==Phase::Closed&&state.raw.is_none()&&state.scanned.is_none()&&state.runtime.is_none()
    }
}
