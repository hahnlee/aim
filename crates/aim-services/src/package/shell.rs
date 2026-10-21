//! Original package shell operations invoke the same native AIDL bodies with
//! the authenticated inbound caller. Never forward a local capability remotely.
use aim_binder_host::{
    local::{Call, LocalProcess, Service},
    parcel::{Binder, EX_ILLEGAL_STATE, Exception, Parcel, Reader},
};
use std::{marker::PhantomData, rc::Rc, sync::Arc};

pub struct Context<'a> {
    pub uid: u32,
    pub pid: i32,
    pub command: crate::shell::ShellCommand,
    process: Arc<LocalProcess>,
    public: &'a dyn Service,
    host: Arc<dyn Host>,
    receivers: std::sync::Mutex<std::collections::BTreeMap<u64, Arc<Receiver>>>,
    _synchronous: PhantomData<Rc<()>>,
}
/// What the shell's operations ask of the server whose package manager
/// runs them: the system server's native owners, or the app host's.
pub trait Host: Send + Sync {
    /// The system server's native owners, which the operations that read
    /// its scan and permission state need; none for another host.
    fn system(&self) -> Option<Arc<crate::system::System>> {
        None
    }
    /// `translateUserId`: `ActivityManager.handleIncomingUser` of the
    /// caller, `USER_ALL` as `all_default`.
    fn translate_user(
        &self,
        pid: i32,
        uid: u32,
        user: i32,
        all_default: i32,
        operation: &str,
    ) -> Result<i32, Exception>;
    /// `UserManagerInternal.getUserInfo(user) != null`.
    fn user_exists(&self, user: i32) -> Result<bool, Exception>;
    /// `sys.boot_completed`.
    fn boot_completed(&self) -> Result<bool, Exception>;
    /// Calls `on_death` when the owner of package sessions dies, as a
    /// command running inside it would end with it; the guard clears it.
    fn link_package_owner_death(
        &self,
        on_death: Box<dyn FnOnce() + Send>,
    ) -> Result<OwnerDeath, Exception>;
    /// Whether an installed package of `user` is a system one and an APEX.
    fn uninstall_package_flags(
        &self,
        name: &str,
        user: i32,
    ) -> Result<Option<(bool, bool)>, Exception>;
    #[allow(clippy::too_many_arguments)]
    fn uninstall_apex(
        &self,
        uid: u32,
        pid: i32,
        name: &str,
        version: i64,
        user: i32,
        receiver: super::installer::preapproval::IntentSender,
        flags: i32,
    ) -> Result<(), Exception>;
    /// `InstallLocationUtils.calculateInstalledSize` of the opened APK.
    fn install_size(
        &self,
        uid: u32,
        pid: i32,
        file: aim_binder_host::server::RetainedFd,
        path: &str,
        abi: Option<&str>,
    ) -> Result<i64, Exception>;
    /// `checkAbiArgument`.
    fn validate_install_abi(&self, uid: u32, pid: i32, abi: &str) -> Result<(), Exception>;
    /// `DexoptParams.Builder.setCompilerFilter`'s check.
    fn validate_install_compiler_filter(
        &self,
        uid: u32,
        pid: i32,
        filter: &str,
    ) -> Result<(), Exception>;
    /// `Flags.sdkDependencyInstaller()`.
    fn sdk_dependency_installer_enabled(&self, uid: u32, pid: i32) -> Result<bool, Exception>;
}
/// A link to the package owner's death, cleared when dropped.
pub struct OwnerDeath(#[allow(dead_code)] Box<dyn Send>);
impl OwnerDeath {
    pub fn new(guard: impl Send + 'static) -> Self {
        Self(Box::new(guard))
    }
}
struct BridgeDeath {
    bridge: Arc<super::bootstrap::Bridge>,
    id: u64,
}
impl Drop for BridgeDeath {
    fn drop(&mut self) {
        self.bridge.clear_death(self.id);
    }
}
impl Host for std::sync::Weak<crate::system::System> {
    fn system(&self) -> Option<Arc<crate::system::System>> {
        self.upgrade()
    }
    fn translate_user(
        &self,
        pid: i32,
        uid: u32,
        user: i32,
        all_default: i32,
        operation: &str,
    ) -> Result<i32, Exception> {
        stopped(self)?.shell_handle_incoming_user(pid, uid, user, all_default, operation)
    }
    fn user_exists(&self, user: i32) -> Result<bool, Exception> {
        Ok(stopped(self)?
            .capture_package_queries()?
            .state()
            .users
            .contains_key(&user))
    }
    fn boot_completed(&self) -> Result<bool, Exception> {
        stopped(self)?.shell_boot_completed()
    }
    fn link_package_owner_death(
        &self,
        on_death: Box<dyn FnOnce() + Send>,
    ) -> Result<OwnerDeath, Exception> {
        let bridge = stopped(self)?.package_bootstrap()?;
        let id = bridge.on_death(on_death);
        Ok(OwnerDeath::new(BridgeDeath { bridge, id }))
    }
    fn uninstall_package_flags(
        &self,
        name: &str,
        user: i32,
    ) -> Result<Option<(bool, bool)>, Exception> {
        let capture = stopped(self)?.capture_package_queries()?;
        Ok(uninstall_flags(capture.state(), name, user))
    }
    fn uninstall_apex(
        &self,
        uid: u32,
        pid: i32,
        name: &str,
        version: i64,
        user: i32,
        receiver: super::installer::preapproval::IntentSender,
        flags: i32,
    ) -> Result<(), Exception> {
        stopped(self)?.shell_uninstall_apex(uid, pid, name, version, user, receiver, flags)
    }
    fn install_size(
        &self,
        uid: u32,
        pid: i32,
        file: aim_binder_host::server::RetainedFd,
        path: &str,
        abi: Option<&str>,
    ) -> Result<i64, Exception> {
        stopped(self)?.shell_install_size(uid, pid, file, path, abi)
    }
    fn validate_install_abi(&self, uid: u32, pid: i32, abi: &str) -> Result<(), Exception> {
        stopped(self)?.shell_validate_install_abi(uid, pid, abi)
    }
    fn validate_install_compiler_filter(
        &self,
        uid: u32,
        pid: i32,
        filter: &str,
    ) -> Result<(), Exception> {
        stopped(self)?.shell_validate_install_compiler_filter(uid, pid, filter)
    }
    fn sdk_dependency_installer_enabled(&self, uid: u32, pid: i32) -> Result<bool, Exception> {
        stopped(self)?.shell_dependency_installer_enabled(uid, pid)
    }
}
/// Of a package installed for `user`: whether it is a system one, and an
/// APEX.
pub fn uninstall_flags(state: &super::model::State, name: &str, user: i32) -> Option<(bool, bool)> {
    let package = state.packages.get(name)?;
    package
        .users
        .get(&user)
        .is_some_and(|state| state.installed)
        .then(|| {
            (
                package.is.system,
                package
                    .pkg
                    .as_ref()
                    .is_some_and(|code| code.is2(super::pkg::booleans2::APEX)),
            )
        })
}
fn stopped(
    system: &std::sync::Weak<crate::system::System>,
) -> Result<Arc<crate::system::System>, Exception> {
    system
        .upgrade()
        .ok_or_else(|| failure("shell native system stopped"))
}
impl<'a> Context<'a> {
    pub fn new(
        system: &Arc<crate::system::System>,
        public: &'a dyn Service,
        call: &mut Call<'_>,
    ) -> Result<Self, i32> {
        let process = system.binder_process();
        let command = crate::shell::ShellCommand::read(&process, &mut call.data)?;
        if call.data.remaining() != 0 {
            return Err(aim_binder_host::parcel::BAD_VALUE);
        }
        Ok(Self::with_command(
            Arc::new(Arc::downgrade(system)),
            process,
            public,
            call.sender_euid,
            call.sender_pid,
            command,
        ))
    }
    /// A command already read, of `uid` (of `pid`), in `host`'s process.
    pub fn with_command(
        host: Arc<dyn Host>,
        process: Arc<LocalProcess>,
        public: &'a dyn Service,
        uid: u32,
        pid: i32,
        command: crate::shell::ShellCommand,
    ) -> Self {
        Self {
            uid,
            pid,
            command,
            process,
            public,
            host,
            receivers: Default::default(),
            _synchronous: PhantomData,
        }
    }
    pub fn system(&self) -> Result<Arc<crate::system::System>, Exception> {
        self.host
            .system()
            .ok_or_else(|| failure("shell native system stopped"))
    }
    pub fn capture(&self) -> Result<Arc<super::scan_snapshot::query_state::Capture>, Exception> {
        self.system()?.capture_package_queries()
    }
    pub fn process(&self) -> &Arc<LocalProcess> {
        &self.process
    }
    /// Calls `on_death` when the system server that owns package sessions
    /// dies, as a command running inside it would end with it. Dropping the
    /// link clears it.
    pub fn link_package_owner_death(
        &self,
        on_death: Box<dyn FnOnce() + Send>,
    ) -> Result<OwnerDeath, Exception> {
        self.host.link_package_owner_death(on_death)
    }
    pub fn input_clone(&mut self) -> Result<aim_binder_host::server::RetainedFd, Exception> {
        self.command
            .input()
            .ok_or_else(|| Exception::illegal_argument("shell input descriptor missing"))?
            .try_clone()
            .map_err(|error| failure(format!("shell input descriptor: {error}")))
    }
    pub fn publish_receiver(&self, service: Arc<dyn Service>) -> Result<Binder, Exception> {
        let identity = self
            .process
            .authenticated_inbound_identity()
            .ok_or_else(|| failure("shell receiver outside authenticated inbound call"))?;
        if identity != (self.pid, self.uid) {
            return Err(failure("shell receiver caller differs"));
        }
        let receiver = Arc::new(Receiver {
            inner: std::sync::Mutex::new(Some(service)),
        });
        let binder = self.process.add_service(receiver.clone());
        if let Binder::Local(ptr) = binder {
            self.receivers.lock().unwrap().insert(ptr, receiver);
        }
        Ok(binder)
    }
    pub fn retire_receiver(&self, binder: Binder) {
        if let Binder::Local(ptr) = binder {
            if let Some(receiver) = self.receivers.lock().unwrap().remove(&ptr) {
                receiver.inner.lock().unwrap().take();
            }
        }
    }
    pub fn install_input(
        &mut self,
        path: Option<&str>,
    ) -> Result<Option<aim_binder_host::server::RetainedFd>, Exception> {
        self.command.open_input(&self.process, path)
    }
    pub fn translate_user(
        &self,
        user: i32,
        all_default: i32,
        operation: &str,
    ) -> Result<i32, Exception> {
        self.host
            .translate_user(self.pid, self.uid, user, all_default, operation)
    }
    pub fn user_exists(&self, user: i32) -> Result<bool, Exception> {
        self.host.user_exists(user)
    }
    pub fn boot_completed(&self) -> Result<bool, Exception> {
        self.host.boot_completed()
    }
    pub fn invoke_permission(&self, code: u32, request: Parcel) -> Result<Parcel, Exception> {
        self.system()?
            .shell_permission_call(self.uid, self.pid, code, request)
    }
    pub fn reset_runtime_permissions(&self) -> Result<(), Exception> {
        self.system()?
            .shell_reset_runtime_permissions(self.uid, self.pid)
    }
    pub fn clear_data(
        &self,
        package: &str,
        user: i32,
        cache_only: bool,
    ) -> Result<bool, Exception> {
        self.system()?
            .shell_clear_data(self.uid, self.pid, package, user, cache_only)
    }
    pub fn requested_runtime_permissions(
        &self,
        package: Option<&str>,
        user: i32,
    ) -> Result<Vec<(String, Vec<String>)>, Exception> {
        self.system()?
            .shell_requested_runtime_permissions(self.uid, self.pid, package, user)
    }
    pub fn read_leaf(&self, code: u32, request: Parcel) -> Result<Parcel, Exception> {
        self.system()?
            .shell_read_leaf(self.uid, self.pid, code, request)
    }
    pub fn uninstall_package_flags(
        &self,
        name: &str,
        user: i32,
    ) -> Result<Option<(bool, bool)>, Exception> {
        self.host.uninstall_package_flags(name, user)
    }
    pub fn uninstall_apex(
        &self,
        name: &str,
        version: i64,
        user: i32,
        receiver: super::installer::preapproval::IntentSender,
        flags: i32,
    ) -> Result<(), Exception> {
        self.host
            .uninstall_apex(self.uid, self.pid, name, version, user, receiver, flags)
    }
    pub fn install_size(&mut self, path: &str, abi: Option<&str>) -> Result<i64, Exception> {
        // setParamsSize: an unopened file ends the command.
        let Some(file) = self.install_input(Some(path))? else {
            self.command
                .eprintln(&format!("Error: Can't open file: {path}"));
            return Err(Exception::illegal_argument(format!(
                "Error: Can't open file: {path}"
            )));
        };
        self.host.install_size(self.uid, self.pid, file, path, abi)
    }
    pub fn validate_install_abi(&self, abi: &str) -> Result<(), Exception> {
        self.host.validate_install_abi(self.uid, self.pid, abi)
    }
    pub fn validate_install_compiler_filter(&self, filter: &str) -> Result<(), Exception> {
        self.host
            .validate_install_compiler_filter(self.uid, self.pid, filter)
    }
    pub fn sdk_dependency_installer_enabled(&self) -> Result<bool, Exception> {
        self.host
            .sdk_dependency_installer_enabled(self.uid, self.pid)
    }
    pub fn invoke_public(&self, code: u32, request: Parcel) -> Result<Parcel, Exception> {
        let identity = self
            .process
            .authenticated_inbound_identity()
            .ok_or_else(|| failure("public shell invocation outside authenticated inbound call"))?;
        if identity != (self.pid, self.uid) {
            return Err(failure(
                "public shell caller differs from authenticated inbound identity",
            ));
        }
        if code == crate::shell::SHELL_COMMAND_TRANSACTION {
            return Err(failure("recursive package shell invocation"));
        }
        if !request.files().is_empty() {
            return Err(failure(
                "public shell invocation requires transferred input descriptors",
            ));
        }
        self.public
            .transact(&mut Call {
                code,
                flags: 0,
                sender_pid: self.pid,
                sender_euid: self.uid,
                data: Reader::new(request.data(), request.objects()),
            })
            .map_err(transport)
    }
    pub fn invoke(&self, target: Binder, code: u32, request: Parcel) -> Result<Parcel, Exception> {
        let Binder::Local(ptr) = target else {
            return Err(failure("shell invocation cannot elevate a remote service"));
        };
        let identity = self
            .process
            .authenticated_inbound_identity()
            .ok_or_else(|| failure("shell invocation outside authenticated inbound call"))?;
        if identity != (self.pid, self.uid) {
            return Err(failure(
                "shell caller differs from authenticated inbound identity",
            ));
        }
        self.process
            .local_service(ptr)
            .ok_or_else(|| failure("shell local capability retired"))?
            .transact(code, &request, false)
            .map(|reply| reply.into_parcel())
            .map_err(transport)
    }
    pub fn finish(self, result: i32) {
        for receiver in self.receivers.lock().unwrap().values() {
            receiver.inner.lock().unwrap().take();
        }
        self.command.finish(result)
    }
}
struct Receiver {
    inner: std::sync::Mutex<Option<Arc<dyn Service>>>,
}
impl Service for Receiver {
    fn descriptor(&self) -> &str {
        "android.content.IIntentSender"
    }
    fn accepts_fds(&self) -> bool {
        true
    }
    fn transact(&self, call: &mut Call<'_>) -> aim_binder_host::local::Reply {
        self.inner
            .lock()
            .unwrap()
            .clone()
            .ok_or(aim_binder_host::parcel::DEAD_OBJECT)?
            .transact(call)
    }
}
fn failure(message: impl Into<String>) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}
fn transport(status: i32) -> Exception {
    failure(format!("native shell AIDL transport: {status}"))
}

pub fn dispatch(context: Context<'_>) {
    dispatch_by(context, run)
}
/// Runs the command with `run`, prints what it threw as `exec` does, and
/// sends its result.
pub fn dispatch_by(
    mut context: Context<'_>,
    run: impl FnOnce(&mut Context<'_>) -> Result<Option<i32>, Exception>,
) {
    if !context.command.has_output() {
        return;
    }
    let result = run(&mut context);
    let status = match result {
        Ok(Some(status)) => status,
        Ok(None) => {
            let command = context.command.command().unwrap_or_default().to_owned();
            context
                .command
                .eprintln(&format!("Unknown command: {command}"));
            -1
        }
        Err(error) => {
            let class = match error.code {
                aim_binder_host::parcel::EX_SECURITY => "java.lang.SecurityException",
                aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT => {
                    "java.lang.IllegalArgumentException"
                }
                aim_binder_host::parcel::EX_UNSUPPORTED_OPERATION => {
                    "java.lang.UnsupportedOperationException"
                }
                _ => "java.lang.IllegalStateException",
            };
            context
                .command
                .exception(&format!("{class}: {}", error.message));
            -1
        }
    };
    context.finish(status);
}
fn run(context: &mut Context<'_>) -> Result<Option<i32>, Exception> {
    if let Some(status) = super::shell_user::run(context)? {
        return Ok(Some(status));
    }
    if let Some(status) = super::shell_profile::run(context)? {
        return Ok(Some(status));
    }
    if let Some(status) = super::shell_read::run(context)? {
        return Ok(Some(status));
    }
    if let Some(status) = super::shell_install::run(context)? {
        return Ok(Some(status));
    }
    super::shell_mutation::run(context)
}
