//! `ProcessList.startProcessLocked`'s `mProcessesOnHold` (android-16.0.0_r1):
//! a start asked for before `mProcessesReady` that is not allowed while
//! booting waits, not its caller; `finishBooting` starts it.
use super::{AppHost, STOCK_PM_FLAGS, SYSTEM_PROCESS, SYSTEM_UID, activity_start};
use aim_binder_host::parcel::Exception;

/// `ApplicationInfo.FLAG_PERSISTENT`.
const FLAG_PERSISTENT: i32 = 1 << 3;

/// A process start on hold: what `start_process_for` was asked.
pub(super) struct HeldStart {
    name: String,
    process: String,
    request: Option<activity_start::StartRequest>,
    from_background: bool,
    /// `isAllowedWhileBooting`: a persistent app's start, which only the
    /// app host's package readiness (preceding forks) holds.
    while_booting: bool,
}

impl AppHost {
    /// Starts the process of package `name` for `request`, or for a
    /// service, a provider or a broadcast (no activity). As
    /// `ProcessList.startProcessLocked` decides by `FLAG_FROM_BACKGROUND`,
    /// a start `from_background` (a broadcast's) fails for a bad process,
    /// and any other start makes the process good again and resets its
    /// crash counts. Before `mProcessesReady` it is put on hold unless it
    /// is allowed while booting; the system process runs already.
    pub(super) fn start_process_for(
        &self,
        name: &str,
        process: &str,
        request: Option<activity_start::StartRequest>,
        from_background: bool,
    ) -> Result<(), Exception> {
        if self.processes_ready.get().is_none() {
            let info = self.application_info(name, STOCK_PM_FLAGS, None)?;
            let system = process == SYSTEM_PROCESS
                && info.as_ref().is_some_and(|ai| ai.uid as u32 == SYSTEM_UID);
            if !system {
                let mut held = self.processes_on_hold.lock().unwrap();
                // Readiness is published before either drain takes the hold.
                if self.processes_ready.get().is_none() {
                    // A pending activity resumed again while its start is
                    // held asks for it again: it is the same held process.
                    let same = |h: &HeldStart| {
                        h.name == name
                            && h.process == process
                            && match (&h.request, &request) {
                                (None, None) => true,
                                (Some(a), Some(b)) => a.restart.is_some() && a.restart == b.restart,
                                _ => false,
                            }
                    };
                    if held.iter().any(same) {
                        return Ok(());
                    }
                    held.push(HeldStart {
                        name: name.to_string(),
                        process: process.to_string(),
                        request,
                        from_background,
                        while_booting: info.is_some_and(|ai| ai.flags & FLAG_PERSISTENT != 0),
                    });
                    return Ok(());
                }
            }
        }
        self.start_app_process(name, process, request, from_background, false)
            .map(|_| ())
    }

    /// `systemReady`'s `startPersistentApps` point: the held starts allowed
    /// while booting, in order.
    pub(super) fn start_held_while_booting(&self) {
        let held = {
            let mut held = self.processes_on_hold.lock().unwrap();
            let (now, later) = std::mem::take(&mut *held)
                .into_iter()
                .partition(|start: &HeldStart| start.while_booting);
            *held = later;
            now
        };
        held.into_iter().for_each(|start| self.start_held(start));
    }

    /// `finishBooting` after `PHASE_BOOT_COMPLETED`: the processes on hold
    /// started in order (`HOSTING_TYPE_ON_HOLD`). One whose activity
    /// finished meanwhile still starts, as the original's held
    /// `ProcessRecord` does.
    pub(super) fn start_processes_on_hold(&self) {
        let held = std::mem::take(&mut *self.processes_on_hold.lock().unwrap());
        held.into_iter().for_each(|start| self.start_held(start));
    }

    /// The boot stopped: the held starts fail with `error`.
    pub(super) fn fail_processes_on_hold(&self, error: &Exception) {
        let held = std::mem::take(&mut *self.processes_on_hold.lock().unwrap());
        for start in held {
            let token = start.request.and_then(|r| r.restart);
            self.held_start_failed(&start.process, token, error);
        }
    }

    fn start_held(&self, start: HeldStart) {
        let token = start.request.as_ref().and_then(|r| r.restart);
        let started = self.start_app_process(
            &start.name,
            &start.process,
            start.request,
            start.from_background,
            false,
        );
        if let Err(e) = started {
            self.held_start_failed(&start.process, token, &e);
        }
    }

    /// The activity that waited for a held start that failed goes, as a
    /// new activity's does when its start fails at once.
    fn held_start_failed(&self, process: &str, token: Option<u64>, error: &Exception) {
        eprintln!(
            "app host: unable to start held process {process}: {}",
            error.message
        );
        if let (Some(token), Some(me)) = (token, self.me.upgrade()) {
            me.abandon_activity_start(token);
        }
    }
}
