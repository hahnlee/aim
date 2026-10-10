//! Original web-instant Settings watcher publishes immutable native captures.
use super::*;
use crate::package::web_instant_state::{Owner, Snapshot, Changed};
use aim_service_aidl::{dev_aim_server_ipackagebootstrapbridge as bootstrap, dev_aim_server_iwebinstantappsstate as leaf};

pub struct Runtime { owner: Strong, policy: Arc<Owner>, callback: Binder, started: Mutex<Option<Result<()>>>, closed: std::sync::atomic::AtomicBool }
impl Runtime {
    pub(crate) fn close(&self) -> Result<()> {
        if self.closed.swap(true, std::sync::atomic::Ordering::AcqRel) { return Ok(()); }
        if self.started.lock().unwrap().is_none() { return Ok(()); }
        let mut request = Parcel::new(); leaf::Stop {}.write(&mut request);
        let reply = self.owner.transact(leaf::STOP, &request, false).map_err(|status| error(format!("web watcher stop: {status}")))?;
        let mut reader = reply.reader();
        leaf::read_stop_reply(&mut reader).map_err(|status| error(format!("web watcher stop reply: {status}")))??;
        if reader.remaining() != 0 { return Err(error("web watcher stop trailing bytes")); }
        Ok(())
    }
}
fn error(message: impl Into<String>) -> Exception { Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, message) }
impl System {
    fn publish_web_instant_policy(&self, bridge: &Arc<crate::package::bootstrap::Bridge>, snapshot: Snapshot) -> Result<()> {
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| error("web policy bootstrap changed"))?;
        let base = current.queries.as_ref().ok_or_else(|| error("web policy capture unavailable"))?;
        let update = base.prepare_web_instant_policy(snapshot).map_err(error)?;
        current.publish_snapshot(update.store); current.queries = Some(update.capture.clone());
        if let Some(page) = &current.version_page { page.publish(update.capture.scan().version()); }
        state.version = update.capture.scan().version();
        Ok(())
    }
    /// Called once in PMS systemReady, after AMS and providers are available.
    pub fn start_web_instant_policy(self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>) -> Result<()> {
        let runtime = self.initialize_web_instant_policy(bridge)?;
        let mut started = runtime.started.lock().unwrap();
        if let Some(result) = started.as_ref() { return result.clone(); }
        if runtime.closed.load(std::sync::atomic::Ordering::Acquire) { return Err(error("web watcher closed")); }
        let result = (|| {
            let mut request = Parcel::new(); leaf::Start { callback: Some(runtime.callback) }.write(&mut request);
            // Original systemReady registers both observers, then co.onChange.
            let reply = runtime.owner.transact(leaf::START, &request, false).map_err(|status| error(format!("web watcher start: {status}")))?;
            let mut reader = reply.reader();
            leaf::read_start_reply(&mut reader).map_err(|status| error(format!("web watcher start reply: {status}")))??;
            if reader.remaining() != 0 { return Err(error("web watcher start trailing bytes")); }
            self.check_package_bootstrap(bridge)
        })();
        // Failed starts are retained and teardown still unregisters any observer.
        *started = Some(result.clone());
        result
    }
    pub fn initialize_web_instant_policy(self: &Arc<Self>, bridge: &Arc<crate::package::bootstrap::Bridge>) -> Result<Arc<Runtime>> {
        if let Some(runtime) = self.package_bootstrap.lock().unwrap().current.as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge)).and_then(|current| current.web_policy.clone()) {
            return Ok(runtime);
        }
        let node = self.package_bootstrap_binder_leaf(bridge, bootstrap::GET_WEB_INSTANT_APPS_STATE)?;
        let mut request = Parcel::new(); leaf::Capture {}.write(&mut request);
        let reply = node.transact(leaf::CAPTURE, &request, false).map_err(|status| error(format!("web policy capture: {status}")))?;
        let mut reader = reply.reader();
        let bytes = leaf::read_capture_reply(&mut reader).map_err(|status| error(format!("web policy record: {status}")))??
            .ok_or_else(|| error("web policy record absent"))?;
        if reader.remaining() != 0 { return Err(error("web policy capture trailing bytes")); }
        let snapshot = Snapshot::read(&bytes).map_err(error)?;
        self.publish_web_instant_policy(bridge, snapshot)?;
        let system = Arc::downgrade(self); let attached = bridge.clone();
        let policy = Owner::new(&bytes, Arc::new(move |snapshot| {
            system.upgrade().ok_or_else(|| error("web policy system stopped"))?
                .publish_web_instant_policy(&attached, snapshot)
        })).map_err(error)?;
        let callback = self.process.add_service(Arc::new(Changed { owner: Arc::downgrade(&policy) }));
        let runtime = Arc::new(Runtime { owner: node, policy, callback, started: Mutex::new(None), closed: std::sync::atomic::AtomicBool::new(false) });
        self.check_package_bootstrap(bridge)?;
        let mut state = self.package_bootstrap.lock().unwrap();
        let current = state.current.as_mut().filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .ok_or_else(|| error("web watcher bootstrap changed"))?;
        if current.web_policy.is_some() { return Err(error("web watcher initialized concurrently")); }
        current.web_policy = Some(runtime.clone());
        Ok(runtime)
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if let Err(error) = self.close() { eprintln!("Web policy watcher teardown failed: {}", error.message); }
    }
}
