//! Sticky cancellation and one-shot activation for one daemon-owned child.
//!
//! This is a host mechanism only.  Android service policy decides whether a
//! prepared child should be activated; this object merely owns the startup
//! capability and makes cancellation win over any later activation attempt.

use crate::ProfileError;
use crate::process_start_gate::StartGate;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

struct State {
    gate: Option<StartGate>,
    terminal: bool,
}

pub(crate) struct BoundServiceChildControl {
    state: Mutex<State>,
    cancelled: AtomicBool,
}

impl BoundServiceChildControl {
    pub(crate) fn new(gate: Option<StartGate>) -> Self {
        Self {
            state: Mutex::new(State {
                gate,
                terminal: false,
            }),
            cancelled: AtomicBool::new(false),
        }
    }

    /// Release the startup gate exactly once.  Taking the gate before the
    /// write leaves the descriptor's final drop outside the control mutex.
    pub(crate) fn activate(&self) -> Result<(), ProfileError> {
        let gate = {
            let mut state = self.lock()?;
            if state.terminal || self.cancelled.load(Ordering::Acquire) {
                return Err(invalid_activation());
            }
            state.terminal = true;
            state.gate.take().ok_or_else(invalid_activation)?
        };
        match gate.release() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.cancelled.store(true, Ordering::Release);
                Err(ProfileError::Daemon(format!(
                    "bound-service activation: {error}"
                )))
            }
        }
    }

    /// Cancel this child.  The cancellation bit is sticky even after
    /// activation, so the supervisor can terminate a live owned child too.
    pub(crate) fn cancel(&self) {
        let gate = {
            let Ok(mut state) = self.state.lock() else {
                self.cancelled.store(true, Ordering::Release);
                return;
            };
            state.terminal = true;
            self.cancelled.store(true, Ordering::Release);
            state.gate.take()
        };
        drop(gate);
    }

    /// Expire a pending startup gate.  Kept separate from `cancel` so the
    /// supervisor's policy is explicit at its activation deadline.
    pub(crate) fn expire(&self) {
        self.cancel();
    }

    pub(crate) fn expired_waiting(&self) -> bool {
        let gate = {
            let Ok(mut state) = self.state.lock() else {
                self.cancelled.store(true, Ordering::Release);
                return false;
            };
            if state.terminal || state.gate.is_none() {
                return false;
            }
            state.terminal = true;
            self.cancelled.store(true, Ordering::Release);
            state.gate.take()
        };
        drop(gate);
        true
    }

    pub(crate) fn waiting(&self) -> bool {
        self.state
            .lock()
            .map(|state| !state.terminal && state.gate.is_some())
            .unwrap_or(false)
    }

    /// Atomic-only status used by the child supervisor's polling loop.
    pub(crate) fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, State>, ProfileError> {
        self.state
            .lock()
            .map_err(|_| ProfileError::Daemon("bound-service activation lock poisoned".into()))
    }
}

impl Drop for BoundServiceChildControl {
    fn drop(&mut self) {
        let gate = self
            .state
            .get_mut()
            .ok()
            .and_then(|state| state.gate.take());
        drop(gate);
    }
}

fn invalid_activation() -> ProfileError {
    ProfileError::Daemon(
        "invalid bound-service activation: already cancelled, activated, or expired".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, BufRead, BufReader, Write};
    use std::process::{Child, Command, Stdio};

    struct Reap(Child);

    impl Drop for Reap {
        fn drop(&mut self) {
            if !matches!(self.0.try_wait(), Ok(Some(_))) {
                let _ = self.0.kill();
            }
            let _ = self.0.wait();
        }
    }

    fn gate() -> StartGate {
        let mut command = Command::new("/bin/true");
        StartGate::prepare(&mut command).unwrap()
    }

    #[test]
    #[ignore = "subprocess fixture for bound-service activation"]
    fn child_fixture() {
        println!("BEFORE");
        io::stdout().flush().unwrap();
        // SAFETY: this isolated subprocess runs before any application
        // threads and owns the inherited startup descriptor exclusively.
        let result = unsafe { crate::wait_for_process_registration() };
        println!("AFTER {}", result.is_ok());
        io::stdout().flush().unwrap();
        assert!(result.is_ok());
    }

    #[test]
    fn cancellation_is_sticky_before_activation() {
        let control = BoundServiceChildControl::new(Some(gate()));
        assert!(control.waiting());
        control.cancel();
        assert!(control.cancelled());
        assert!(control.activate().is_err());
        assert!(!control.waiting());
    }

    #[test]
    fn activation_consumes_gate_but_late_cancel_is_sticky() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "bound_service_child_control::tests::child_fixture",
                "--ignored",
                "--nocapture",
            ])
            .stdout(Stdio::piped());
        let gate = StartGate::prepare(&mut command).unwrap();
        let mut child = Reap(command.spawn().unwrap());
        let stdout = child.0.stdout.take().unwrap();
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line.trim() == "BEFORE" {
                break;
            }
        }

        let control = BoundServiceChildControl::new(Some(gate));
        assert!(control.activate().is_ok());
        assert!(!control.cancelled());
        line.clear();
        loop {
            line.clear();
            assert!(reader.read_line(&mut line).unwrap() > 0);
            if line.trim() == "AFTER true" {
                break;
            }
        }
        assert!(child.0.wait().unwrap().success());
        control.cancel();
        assert!(control.cancelled());
        assert!(control.activate().is_err());
    }

    #[test]
    fn expiry_cancels_only_a_waiting_gate() {
        let control = BoundServiceChildControl::new(Some(gate()));
        assert!(control.expired_waiting());
        assert!(control.cancelled());
        assert!(!control.expired_waiting());
    }
}
