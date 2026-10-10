//! Native commit queue and mandatory original policy inputs (pinned PIS).
use super::preapproval::IntentSender;
use aim_binder_host::parcel::Exception;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
pub struct Policy {
    pub require_mutable_receiver: bool,
    pub secure_frp: bool,
    pub secure_frp_install_allowed: bool,
}
pub type PolicySource = Arc<dyn Fn(u32, IntentSender) -> Result<Policy, Exception> + Send + Sync>;
pub struct Worker {
    queue: mpsc::Sender<Option<i32>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    pub fn start(
        run: Arc<dyn Fn(i32) -> Result<(), Exception> + Send + Sync>,
        failure: Arc<dyn Fn(String) + Send + Sync>,
    ) -> (mpsc::Sender<Option<i32>>, Self) {
        let (queue, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = thread::spawn(move || {
            while let Ok(Some(id)) = receiver.recv() {
                if flag.load(Ordering::Acquire) {
                    break;
                }
                if let Err(error) = run(id) {
                    failure(error.message);
                }
            }
        });
        (
            queue.clone(),
            Self {
                queue,
                stop,
                thread: Some(thread),
            },
        )
    }
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.queue.send(None);
        if let Some(thread) = self.thread.take() {
            if thread.thread().id() != thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}

/// Image/service inputs for the source-defined confirmation branches. Package
/// identity, permissions, update source and throttle remain native owner checks.
pub struct ConfirmationPolicy {
    pub installer_package: String,
    pub device_owner_or_affiliated: bool,
    pub emergency_install: bool,
    pub install_disabled: bool,
    pub dependency_installer_enabled: bool,
    pub update_ownership_enabled: bool,
    pub silent_target_allowed: bool,
    pub has_device_admin_receiver: bool,
    pub is_sdk_or_static_library: bool,
    pub uptime_millis: i64,
}
pub type ConfirmationPolicySource = Arc<
    dyn Fn(&super::Session, &super::Record) -> Result<ConfirmationPolicy, Exception> + Send + Sync,
>;
