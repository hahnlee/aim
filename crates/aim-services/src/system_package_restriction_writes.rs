//! PMS scheduleWritePackageRestrictions / writePendingRestrictions,
//! android-16.0.0_r1 (AOSP, Apache-2.0).
use super::*;
use aim_binder_host::parcel::EX_ILLEGAL_STATE;
use std::{
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub(super) struct Writer {
    wake: mpsc::Sender<Option<Instant>>,
    worker: JoinHandle<()>,
    pub(super) deadline: Option<Instant>,
}
fn illegal(message: impl ToString) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message.to_string())
}
impl Writer {
    pub(super) fn start(
        system: &Arc<System>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
    ) -> Result<Self> {
        let (wake, receiver) = mpsc::channel();
        let system = Arc::downgrade(system);
        let bridge = Arc::downgrade(bridge);
        let worker = std::thread::Builder::new()
            .name("package-restrictions".into())
            .spawn(move || {
                let mut deadline: Option<Instant> = None;
                loop {
                    let received = match deadline {
                        Some(at) => {
                            receiver.recv_timeout(at.saturating_duration_since(Instant::now()))
                        }
                        None => receiver
                            .recv()
                            .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
                    };
                    match received {
                        Ok(at) => deadline = at,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            deadline = None;
                            let (Some(system), Some(bridge)) = (system.upgrade(), bridge.upgrade())
                            else {
                                break;
                            };
                            if let Err(error) =
                                system.write_due_package_restrictions(&bridge, Some(Instant::now()))
                            {
                                eprintln!("Package restrictions write failed: {}", error.message);
                            }
                        }
                    }
                }
            })
            .map_err(illegal)?;
        Ok(Self {
            wake,
            worker,
            deadline: None,
        })
    }
    pub(super) fn schedule(&mut self, now: Instant) -> Result<()> {
        let at = *self.deadline.get_or_insert(now + Duration::from_secs(10));
        self.wake.send(Some(at)).map_err(illegal)
    }
    fn cancel(&mut self) {
        self.deadline = None;
        // Disconnection is reported by schedule or by close's worker join.
        let _ = self.wake.send(None);
    }
    pub(super) fn close(self) -> Result<()> {
        drop(self.wake);
        self.worker
            .join()
            .map_err(|_| illegal("package restrictions worker panicked"))
    }
}
impl PackageBootstrap {
    pub(super) fn cancel_clean_restrictions(&mut self) {
        if self.dirty_restrictions.is_empty() {
            if let Some(writer) = &mut self.restriction_writes {
                writer.cancel();
            }
        }
    }
    pub(super) fn write_dirty_restrictions(
        &mut self,
        disk: &mut crate::package::owner::Store,
    ) -> Result<()> {
        if let Some(writer) = &mut self.restriction_writes {
            writer.cancel();
        }
        if self.dirty_restrictions.is_empty() {
            return Ok(());
        }
        let capture = self
            .queries
            .as_ref()
            .ok_or_else(|| illegal("restriction capture absent"))?;
        let mut errors = Vec::new();
        for user in self.dirty_restrictions.clone() {
            if !capture.state().users.contains_key(&user) {
                self.dirty_restrictions.remove(&user);
                continue;
            }
            match disk.commit_updated_scan_restrictions(capture.scan().owner(), user as u32, false)
            {
                Ok(()) => {
                    self.dirty_restrictions.remove(&user);
                }
                Err(error) => errors.push(format!("user {user}: {error}")),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(illegal(errors.join("; ")))
        }
    }
}
impl System {
    // The timer supplies `now`; None is an unconditional synchronous flush.
    pub(crate) fn write_due_package_restrictions(
        &self,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        now: Option<Instant>,
    ) -> Result<()> {
        let _install = self.package_install_guard();
        let persistence = self
            .package_bootstrap
            .lock()
            .unwrap()
            .current
            .as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .and_then(|current| current.persistence.clone());
        let Some(persistence) = persistence else {
            return Ok(());
        };
        let mut disk = persistence.lock().unwrap();
        let mut state = self.package_bootstrap.lock().unwrap();
        let Some(current) = state
            .current
            .as_mut()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
        else {
            return Ok(());
        };
        if let Some(now) = now {
            if !current
                .restriction_writes
                .as_ref()
                .and_then(|writer| writer.deadline)
                .is_some_and(|at| at <= now)
            {
                return Ok(());
            }
        }
        current.write_dirty_restrictions(&mut disk)
    }
}
