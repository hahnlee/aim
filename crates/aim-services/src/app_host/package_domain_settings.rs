//! `IPackageDomainSettings` for the app host: the domain verification
//! settings owner system_server's `NativeDomainSettings` serves to the
//! native package boot (`android-16.0.0_r1`), to the system uid only. The
//! app host's DomainVerificationService is the native domain owner (its
//! `addPackage`, `migrateState` and `clearPackage` ported in
//! `package::domain_verification::owner`), so the settings it reads at
//! `seed`, attaches the admitted packages to and writes back are the
//! current native capture's own: `capture` writes that capture's
//! projection, and `reconcilePackages` takes the capture of the expected
//! version and reports its settings to the native owner's
//! `IPackageDomainSettingsChanged`, off the caller's thread as the
//! original's `scheduleWrite` posts it.

use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::dev_aim_server_ipackagedomainsettings as leaf;
use aim_service_aidl::dev_aim_server_ipackagedomainsettingschanged as changed;

use super::{AppHost, reply};
use crate::package::domain_verification::original_bridge;
use crate::package::scan_snapshot::query_state::Capture;
use crate::system::ServiceOwner;

const SYSTEM_UID: u32 = 1000;

#[derive(Default)]
struct State {
    changes: Option<Arc<ServiceOwner>>,
    seeded: bool,
    closed: bool,
    lifecycle_version: i64,
}

pub struct DomainSettings {
    host: Arc<AppHost>,
    state: Arc<Mutex<State>>,
}

fn illegal_state(message: impl Into<String>) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}

impl DomainSettings {
    pub fn new(host: Arc<AppHost>) -> Self {
        DomainSettings {
            host,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    fn seed(&self, settings: Option<Vec<u8>>, changes: Option<Binder>) -> Result<(), Exception> {
        let mut state = self.state.lock().unwrap();
        if state.seeded || state.closed {
            return Err(illegal_state(
                "original domain settings already seeded or closed",
            ));
        }
        let changes = match changes {
            None => return Err(Exception::new(EX_NULL_POINTER, "changes")),
            Some(Binder::Handle(handle)) => {
                ServiceOwner::Remote(Arc::new(self.host.process.strong(handle)))
            }
            Some(Binder::Local(ptr)) => ServiceOwner::Local(
                self.host
                    .process
                    .local_service(ptr)
                    .ok_or_else(|| illegal_state("domain settings listener gone"))?,
            ),
        };
        let snapshot = capture(&self.host)?;
        // `readSettings` of the native record; its packages are the
        // snapshot's, attached as the native owner attached them.
        settings
            .ok_or_else(|| "null settings".to_string())
            .and_then(|bytes| original_bridge::decode(&bytes))
            .map_err(|e| illegal_state(format!("original domain settings import failed: {e}")))?;
        state.changes = Some(Arc::new(changes));
        state.lifecycle_version = version(&snapshot)?;
        state.seeded = true;
        Ok(())
    }

    fn capture(&self) -> Result<Vec<u8>, Exception> {
        let state = self.state.lock().unwrap();
        if !state.seeded || state.closed {
            return Err(illegal_state(
                "original domain settings not seeded or closed",
            ));
        }
        drop(state);
        capture_state(&*capture(&self.host)?)
    }

    fn reconcile_packages(&self, expected_version: i64) -> Result<bool, Exception> {
        let mut state = self.state.lock().unwrap();
        if !state.seeded || state.closed {
            return Err(illegal_state(
                "original domain settings not seeded or closed",
            ));
        }
        let next = capture(&self.host)?;
        if version(&next)? != expected_version {
            return Ok(false);
        }
        if expected_version < state.lifecycle_version {
            return Err(illegal_state("domain lifecycle generation regressed"));
        }
        state.lifecycle_version = expected_version;
        drop(state);
        self.schedule_write()?;
        Ok(true)
    }

    fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.changes = None;
    }

    /// `scheduleWrite`: the settings as they are when the write runs, to
    /// the native owner, unless closed meanwhile.
    fn schedule_write(&self) -> Result<(), Exception> {
        let (host, state) = (self.host.clone(), self.state.clone());
        std::thread::Builder::new()
            .name("domain settings".into())
            .spawn(move || {
                // The record is made under the lock, and sent outside it.
                let (record, target, version) = {
                    let state = state.lock().unwrap();
                    if state.closed {
                        return;
                    }
                    let record = match capture(&host).and_then(|c| capture_state(&c)) {
                        Ok(record) => record,
                        Err(e) => return eprintln!("app host: domain settings write: {e:?}"),
                    };
                    let Some(target) = state.changes.clone() else {
                        return;
                    };
                    (record, target, state.lifecycle_version)
                };
                let mut p = Parcel::new();
                changed::Changed {
                    settings: Some(record),
                    lifecycle_version: version,
                }
                .write(&mut p);
                match target.transact(changed::CHANGED, &p, false) {
                    Ok(reply) => {
                        if let Err(e) = changed::read_changed_reply(&mut reply.reader())
                            .map_err(|s| illegal_state(format!("changed reply: {s}")))
                            .and_then(|r| r)
                        {
                            eprintln!("app host: domain settings write: {e:?}");
                        }
                    }
                    Err(status) => eprintln!("app host: domain settings write: {status}"),
                }
            })
            .map(|_| ())
            .map_err(|_| illegal_state("domain persistence Handler stopped"))
    }
}

/// The native package boot's current capture, the original's
/// `computers.get()`.
fn capture(host: &AppHost) -> Result<Arc<Capture>, Exception> {
    let system = host
        .system
        .get()
        .ok_or_else(|| illegal_state("native package owner unavailable"))?;
    let bridge = system.package_bootstrap()?;
    system.capture_package_queries_for(&bridge)
}

/// `domainCaptureVersion`.
fn version(capture: &Capture) -> Result<i64, Exception> {
    i64::try_from(capture.scan().version())
        .map_err(|_| illegal_state("domain lifecycle version exhausted"))
}

/// `captureState`: `<packages>` with the settings `writeSettings` writes.
fn capture_state(capture: &Capture) -> Result<Vec<u8>, Exception> {
    let domains = capture
        .domains()
        .ok_or_else(|| illegal_state("native domain owner absent"))?;
    original_bridge::encode(&domains.owner().xml_projection())
        .map_err(|e| illegal_state(format!("original domain settings export failed: {e}")))
}

impl Service for DomainSettings {
    fn descriptor(&self) -> &str {
        leaf::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // `enforce`.
        let system = call.sender_euid == SYSTEM_UID;
        let caller = || {
            if system {
                Ok(())
            } else {
                Err(Exception::security("Native domain settings owner required"))
            }
        };
        let mut p = Parcel::new();
        match call.code {
            leaf::SEED => {
                let a = leaf::Seed::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    caller()
                        .and_then(|()| self.seed(a.native_settings, a.changes))
                        .map(|()| {
                            leaf::write_seed_reply(&mut p);
                            p
                        }),
                )
            }
            leaf::CAPTURE => {
                leaf::Capture::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().and_then(|()| self.capture()).map(|record| {
                    leaf::write_capture_reply(&mut p, &Some(record));
                    p
                }))
            }
            leaf::RECONCILE_PACKAGES => {
                let a = leaf::ReconcilePackages::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    caller()
                        .and_then(|()| self.reconcile_packages(a.expected_version))
                        .map(|applied| {
                            leaf::write_reconcile_packages_reply(&mut p, applied);
                            p
                        }),
                )
            }
            leaf::CLOSE => {
                leaf::Close::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(caller().map(|()| {
                    self.close();
                    leaf::write_close_reply(&mut p);
                    p
                }))
            }
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::EX_SECURITY;

    fn node(euid: u32) -> aim_binder_host::local::LocalService {
        let process = aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid,
                security_context: None,
            },
        );
        let host = AppHost::new(process.clone());
        let Binder::Local(ptr) = process.add_service(Arc::new(DomainSettings::new(host))) else {
            unreachable!()
        };
        process.local_service(ptr).unwrap()
    }

    fn code(node: &aim_binder_host::local::LocalService, code: u32, p: &Parcel) -> Option<i32> {
        let reply = node.transact(code, p, false).unwrap();
        reply
            .reader()
            .read_exception()
            .unwrap()
            .err()
            .map(|e| e.code)
    }

    /// Only the system uid is served, and nothing is read before a seed or
    /// after a close.
    #[test]
    fn settings_are_read_only_between_seed_and_close() {
        let mut capture = Parcel::new();
        leaf::Capture {}.write(&mut capture);
        assert_eq!(
            code(&node(10000), leaf::CAPTURE, &capture),
            Some(EX_SECURITY)
        );
        let node = node(SYSTEM_UID);
        assert_eq!(code(&node, leaf::CAPTURE, &capture), Some(EX_ILLEGAL_STATE));
        let mut reconcile = Parcel::new();
        leaf::ReconcilePackages {
            expected_version: 1,
        }
        .write(&mut reconcile);
        assert_eq!(
            code(&node, leaf::RECONCILE_PACKAGES, &reconcile),
            Some(EX_ILLEGAL_STATE)
        );
        let mut close = Parcel::new();
        leaf::Close {}.write(&mut close);
        assert_eq!(code(&node, leaf::CLOSE, &close), None);
        let mut seed = Parcel::new();
        leaf::Seed {
            native_settings: Some(original_bridge::encode(&Default::default()).unwrap()),
            changes: None,
        }
        .write(&mut seed);
        assert_eq!(code(&node, leaf::SEED, &seed), Some(EX_ILLEGAL_STATE));
    }
}
