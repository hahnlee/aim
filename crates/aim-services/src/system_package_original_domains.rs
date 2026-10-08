//! Actual original DVS callback -> validated native domain owner -> durable
//! Settings publication. No remote call is made while the install gate is held.
use super::*;
use crate::package::domain_verification::{self as domain, original_bridge};
use aim_service_aidl::{
    ReadParcelable, dev_aim_server_ipackagebootstrapbridge as bootstrap,
    dev_aim_server_ipackagedomainsettings as leaf,
    dev_aim_server_ipackagedomainsettingschanged as callback,
};
struct State {
    before: domain::State,
    failure: Option<Exception>,
}
pub struct Runtime {
    system: Weak<System>,
    bridge: Arc<crate::package::bootstrap::Bridge>,
    leaf: Strong,
    state: Mutex<State>,
    closed: std::sync::atomic::AtomicBool,
}
fn failure(message: impl Into<String>) -> Exception {
    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message)
}
impl Runtime {
    pub fn reconcile_packages(&self) -> Result<()> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(failure("original domain owner closed"));
        }
        let system = self
            .system
            .upgrade()
            .ok_or_else(|| failure("original domain system stopped"))?;
        system.check_package_bootstrap(&self.bridge)?;
        let mut request = Parcel::new();
        leaf::ReconcilePackages {}.write(&mut request);
        let reply = self
            .leaf
            .transact(leaf::RECONCILE_PACKAGES, &request, false)
            .map_err(|status| failure(format!("domain lifecycle transport: {status}")))?;
        leaf::read_reconcile_packages_reply(&mut reply.reader())
            .map_err(|status| failure(format!("domain lifecycle reply: {status}")))??;
        system.check_package_bootstrap(&self.bridge)
    }
    pub fn close(&self) -> Result<()> {
        if self.closed.swap(true, std::sync::atomic::Ordering::AcqRel) {
            return Ok(());
        }
        let mut request = Parcel::new();
        leaf::Close {}.write(&mut request);
        let reply = self
            .leaf
            .transact(leaf::CLOSE, &request, false)
            .map_err(|status| failure(format!("domain close transport: {status}")))?;
        leaf::read_close_reply(&mut reply.reader())
            .map_err(|status| failure(format!("domain close reply: {status}")))?
    }
    fn apply(&self, bytes: &[u8]) -> Result<()> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(failure("original domain owner closed"));
        }
        let desired = original_bridge::decode(bytes).map_err(failure)?;
        let mut state = self.state.lock().unwrap();
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        let result: Result<()> = (|| {
            let system = self
                .system
                .upgrade()
                .ok_or_else(|| failure("original domain system stopped"))?;
            system.check_package_bootstrap(&self.bridge)?;
            let _install = system.package_install_guard();
            loop {
                let (capture, disk) = {
                    let root = system.package_bootstrap.lock().unwrap();
                    let current = root
                        .current
                        .as_ref()
                        .filter(|current| Arc::ptr_eq(&current.bridge, &self.bridge))
                        .ok_or_else(|| failure("original domain bootstrap replaced"))?;
                    (
                        current
                            .queries
                            .clone()
                            .ok_or_else(|| failure("original domain native capture missing"))?,
                        current
                            .persistence
                            .clone()
                            .ok_or_else(|| failure("original domain disk owner missing"))?,
                    )
                };
                let native = capture
                    .domains()
                    .ok_or_else(|| failure("native domain owner missing"))?;
                let next = original_bridge::merge(native.owner(), &state.before, desired.clone())
                    .map_err(failure)?;
                if next.persisted() == native.owner().persisted() {
                    return Ok(());
                }
                let update = capture.prepare_domain_update(next).map_err(failure)?;
                // A concurrent runtime publication can replace the query base while
                // this callback waits for the disk owner. Reapply the same original
                // delta to the new capture only when no persistence was attempted.
                match system.commit_package_domains_if_current(
                    &self.bridge,
                    update,
                    &mut disk.lock().unwrap(),
                ) {
                    Ok(Some(_)) => return Ok(()),
                    Ok(None) => continue,
                    Err(error) => {
                        return Err(failure(format!(
                            "original domain persistence (committed={}): {error}",
                            error.committed
                        )));
                    }
                }
            }
        })();
        match result {
            Ok(()) => {
                state.before = desired;
                Ok(())
            }
            Err(error) => {
                state.failure = Some(error.clone());
                Err(error)
            }
        }
    }
}
struct Changed(Arc<Runtime>);
impl aim_binder_host::local::Service for Changed {
    fn descriptor(&self) -> &str {
        callback::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> aim_binder_host::local::Reply {
        if call.code != callback::CHANGED {
            return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION);
        }
        if call.sender_euid != 1000 {
            return Err(aim_binder_host::parcel::PERMISSION_DENIED);
        }
        let args = callback::Changed::read(&mut call.data)?;
        if call.data.remaining() != 0 {
            return Err(aim_binder_host::parcel::BAD_VALUE);
        }
        let result = args
            .settings
            .as_deref()
            .ok_or_else(|| failure("original domain state null"))
            .and_then(|bytes| self.0.apply(bytes));
        let mut reply = Parcel::new();
        match result {
            Ok(()) => reply.write_no_exception(),
            Err(error) => reply.write_exception(&error),
        };
        Ok(reply)
    }
}
impl System {
    pub(crate) fn reconcile_original_package_domains(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<()> {
        let runtime = {
            let state = self.package_bootstrap.lock().unwrap();
            let current = state
                .current
                .as_ref()
                .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
                .ok_or_else(|| failure("original domain lifecycle bootstrap replaced"))?;
            current
                .original_domains
                .as_ref()
                .and_then(Weak::upgrade)
                .ok_or_else(|| failure("original domain lifecycle owner unavailable"))?
        };
        runtime.reconcile_packages()
    }
    pub fn initialize_original_domain_settings(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<Arc<Runtime>> {
        self.check_package_bootstrap(bridge)?;
        let capture = self.capture_package_queries()?;
        let before = capture
            .domains()
            .ok_or_else(|| failure("native domain initializer owner absent"))?
            .owner()
            .xml_projection();
        let record = original_bridge::encode(&before).map_err(failure)?;
        let owner =
            self.package_bootstrap_binder_leaf(bridge, bootstrap::GET_PACKAGE_DOMAIN_SETTINGS)?;
        let runtime = Arc::new(Runtime {
            system: Arc::downgrade(self),
            bridge: bridge.clone(),
            leaf: owner,
            state: Mutex::new(State {
                before,
                failure: None,
            }),
            closed: std::sync::atomic::AtomicBool::new(false),
        });
        let callback = self.process.add_service(Arc::new(Changed(runtime.clone())));
        let mut request = Parcel::new();
        leaf::Seed {
            native_settings: Some(record),
            changes: Some(callback),
        }
        .write(&mut request);
        let reply = runtime
            .leaf
            .transact(leaf::SEED, &request, false)
            .map_err(|status| failure(format!("domain seed transport: {status}")))?;
        leaf::read_seed_reply(&mut reply.reader())
            .map_err(|status| failure(format!("domain seed reply: {status}")))??;
        let mut request = Parcel::new();
        leaf::Capture {}.write(&mut request);
        let reply = runtime
            .leaf
            .transact(leaf::CAPTURE, &request, false)
            .map_err(|status| failure(format!("domain export transport: {status}")))?;
        let record = leaf::read_capture_reply(&mut reply.reader())
            .map_err(|status| failure(format!("domain export reply: {status}")))??
            .ok_or_else(|| failure("domain initial export null"))?;
        runtime.apply(&record)?;
        Ok(runtime)
    }
}
