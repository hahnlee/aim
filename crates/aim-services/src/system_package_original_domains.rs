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
    receipts: original_bridge::AdmissionReceipts,
    pending: bool,
    failure: Option<Exception>,
}
pub struct Runtime {
    system: Weak<System>,
    bridge: Arc<crate::package::bootstrap::Bridge>,
    leaf: Strong,
    state: Mutex<State>,
    lifecycle: Mutex<()>,
    lifecycle_done: std::sync::Condvar,
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
        let _operation = self.lifecycle.lock().unwrap();
        loop {
            system.check_package_bootstrap(&self.bridge)?;
            let capture = system.capture_package_queries()?;
            let native = capture
                .domains()
                .ok_or_else(|| failure("native domain lifecycle owner absent"))?;
            let generation = capture.scan().version();
            let previous = {
                let mut state = self.state.lock().unwrap();
                if let Some(error) = &state.failure {
                    return Err(error.clone());
                }
                let previous = state.receipts.clone();
                state
                    .receipts
                    .record(generation, native.owner())
                    .map_err(failure)?;
                state.pending = true;
                previous
            };
            // No state/publication/disk/install lock crosses this original RPC.
            // A queued XML callback waits for the typed admission result below.
            let result: Result<bool> = (|| {
                let mut request = Parcel::new();
                leaf::ReconcilePackages {
                    expected_version: i64::try_from(generation)
                        .map_err(|_| failure("domain lifecycle version exhausted"))?,
                }
                .write(&mut request);
                let reply = self
                    .leaf
                    .transact(leaf::RECONCILE_PACKAGES, &request, false)
                    .map_err(|status| failure(format!("domain lifecycle transport: {status}")))?;
                leaf::read_reconcile_packages_reply(&mut reply.reader())
                    .map_err(|status| failure(format!("domain lifecycle reply: {status}")))?
            })();
            {
                let mut state = self.state.lock().unwrap();
                if !matches!(result, Ok(true)) {
                    state.receipts = previous;
                }
                if let Err(error) = &result {
                    state.failure = Some(error.clone());
                }
                state.pending = false;
                self.lifecycle_done.notify_all();
            }
            match result {
                Ok(true) => return system.check_package_bootstrap(&self.bridge),
                Ok(false) => continue,
                Err(error) => return Err(error),
            }
        }
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
    fn apply(&self, bytes: &[u8], lifecycle_version: u64) -> Result<()> {
        if self.closed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(failure("original domain owner closed"));
        }
        let desired = original_bridge::decode(bytes).map_err(failure)?;
        let mut state = self.state.lock().unwrap();
        while state.pending {
            state = self.lifecycle_done.wait(state).unwrap();
        }
        if let Some(error) = &state.failure {
            return Err(error.clone());
        }
        let result: Result<()> = (|| {
            state
                .receipts
                .validate_version(lifecycle_version)
                .map_err(failure)?;
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
                let next = original_bridge::merge_with_receipts(
                    native.owner(),
                    &state.before,
                    desired.clone(),
                    &state.receipts,
                )
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
                let current = self
                    .system
                    .upgrade()
                    .ok_or_else(|| failure("original domain system stopped"))?
                    .capture_package_queries()?;
                state
                    .receipts
                    .acknowledge(
                        lifecycle_version,
                        &desired,
                        current
                            .domains()
                            .ok_or_else(|| failure("native domain acknowledgement owner absent"))?
                            .owner(),
                    )
                    .map_err(failure)?;
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
            .and_then(|bytes| {
                self.0.apply(
                    bytes,
                    u64::try_from(args.lifecycle_version)
                        .map_err(|_| failure("negative original domain lifecycle version"))?,
                )
            });
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
                pending: false,
                receipts: {
                    let mut receipts = original_bridge::AdmissionReceipts::default();
                    receipts
                        .record(capture.scan().version(), capture.domains().unwrap().owner())
                        .map_err(failure)?;
                    receipts
                },
                failure: None,
            }),
            lifecycle: Mutex::new(()),
            lifecycle_done: std::sync::Condvar::new(),
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
        runtime.apply(&record, capture.scan().version())?;
        Ok(runtime)
    }
}
