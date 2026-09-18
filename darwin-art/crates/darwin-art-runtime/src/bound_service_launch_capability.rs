//! Process-local ownership of prepared bound-service launch capabilities.
//!
//! The C ABI exposes only an opaque integer handle.  The immutable daemon
//! socket is retained with that handle so a capability can never be replayed
//! against another profile daemon.

use darwin_art_profile::BoundServiceProcessResponse;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

pub(crate) const PREPARED: u8 = 0;
pub(crate) const ACTIVATING: u8 = 1;
pub(crate) const ACTIVATED: u8 = 2;
pub(crate) const CANCELLED: u8 = 3;
pub(crate) const RELEASED: u8 = 4;

pub(crate) struct LaunchCapability {
    response: BoundServiceProcessResponse,
    socket: PathBuf,
    phase: AtomicU8,
    unpublished_cleanup: AtomicBool,
}

impl LaunchCapability {
    pub(crate) fn response(&self) -> BoundServiceProcessResponse {
        self.response
    }

    pub(crate) fn socket(&self) -> &Path {
        &self.socket
    }

    pub(crate) fn phase(&self) -> u8 {
        self.phase.load(Ordering::Acquire)
    }

    pub(crate) fn begin_activation(&self) -> bool {
        self.phase
            .compare_exchange(PREPARED, ACTIVATING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub(crate) fn restore_prepared_after_activation_failure(&self) {
        let _ =
            self.phase
                .compare_exchange(ACTIVATING, PREPARED, Ordering::AcqRel, Ordering::Acquire);
    }

    pub(crate) fn finish_activation(&self) -> bool {
        self.phase
            .compare_exchange(ACTIVATING, ACTIVATED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    pub(crate) fn begin_cancellation(&self) -> bool {
        loop {
            let phase = self.phase();
            if phase == RELEASED {
                return false;
            }
            if phase == CANCELLED {
                return true;
            }
            if self
                .phase
                .compare_exchange(phase, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
        }
    }

    pub(crate) fn begin_unpublished_cancellation(&self) -> bool {
        loop {
            let phase = self.phase();
            if phase == RELEASED || phase == ACTIVATING || phase == ACTIVATED {
                return false;
            }
            if phase == CANCELLED {
                return true;
            }
            if self
                .phase
                .compare_exchange(PREPARED, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return true;
            }
        }
    }

    pub(crate) fn mark_unpublished_cleanup(&self) {
        self.unpublished_cleanup.store(true, Ordering::Release);
    }

    pub(crate) fn needs_unpublished_cleanup(&self) -> bool {
        self.unpublished_cleanup.load(Ordering::Acquire)
    }

    pub(crate) fn mark_released(&self) {
        self.phase.store(RELEASED, Ordering::Release);
    }
}

#[derive(Default)]
struct PreparedProcesses {
    next: u64,
    entries: BTreeMap<u64, Arc<LaunchCapability>>,
}

impl PreparedProcesses {
    fn insert(&mut self, socket: &Path, response: BoundServiceProcessResponse) -> Result<u64, ()> {
        self.next = self.next.checked_add(1).ok_or(())?;
        self.entries.insert(
            self.next,
            Arc::new(LaunchCapability {
                response,
                socket: socket.to_path_buf(),
                phase: AtomicU8::new(PREPARED),
                unpublished_cleanup: AtomicBool::new(false),
            }),
        );
        Ok(self.next)
    }

    fn lookup(&self, handle: u64) -> Option<Arc<LaunchCapability>> {
        self.entries.get(&handle).cloned()
    }
}

fn prepared() -> &'static Mutex<PreparedProcesses> {
    static PREPARED: OnceLock<Mutex<PreparedProcesses>> = OnceLock::new();
    PREPARED.get_or_init(|| Mutex::new(PreparedProcesses::default()))
}

pub(crate) fn insert_prepared(
    socket: &Path,
    response: BoundServiceProcessResponse,
) -> Result<u64, ()> {
    prepared().lock().map_err(|_| ())?.insert(socket, response)
}

pub(crate) fn lookup(handle: u64) -> Option<Arc<LaunchCapability>> {
    prepared().lock().ok()?.lookup(handle)
}

pub(crate) fn forget(handle: u64) -> Option<Arc<LaunchCapability>> {
    let capability = prepared().lock().ok()?.entries.remove(&handle);
    if let Some(capability) = &capability {
        capability.mark_released();
    }
    capability
}

pub(crate) fn forget_if_exact(handle: u64, capability: &Arc<LaunchCapability>) -> bool {
    let removed = match prepared().lock() {
        Ok(mut table)
            if table
                .entries
                .get(&handle)
                .is_some_and(|entry| Arc::ptr_eq(entry, capability)) =>
        {
            table.entries.remove(&handle)
        }
        _ => None,
    };
    if let Some(removed) = removed {
        removed.mark_released();
        true
    } else {
        false
    }
}

/// Cancel and, on success, forget one unpublished capability using its own
/// immutable socket.  Failed transport keeps the capability sealed for retry.
pub(crate) fn cancel_unpublished(handle: u64, capability: &Arc<LaunchCapability>) -> i32 {
    if !capability.begin_unpublished_cancellation() {
        return -1;
    }
    capability.mark_unpublished_cleanup();
    if let Err(error) =
        darwin_art_profile::cancel_bound_service_process(capability.socket(), capability.response())
    {
        eprintln!("unpublished bound-service cancellation remains owned: {error}");
        return -2;
    }
    forget_if_exact(handle, capability);
    0
}

pub(crate) fn retry_unpublished(socket: &Path) {
    let pending: Vec<_> = match prepared().lock() {
        Ok(table) => table
            .entries
            .iter()
            .filter(|(_, capability)| {
                capability.needs_unpublished_cleanup() && capability.socket() == socket
            })
            .map(|(handle, capability)| (*handle, Arc::clone(capability)))
            .collect(),
        Err(_) => return,
    };
    for (handle, capability) in pending {
        let _ = cancel_unpublished(handle, &capability);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response() -> BoundServiceProcessResponse {
        BoundServiceProcessResponse {
            pid: 4242,
            start_sequence: 9,
            incarnation: [10, 11],
            activation_token: [0x5a; 16],
        }
    }

    #[test]
    fn capability_exhaustion_never_reuses_a_handle() {
        let mut table = PreparedProcesses {
            next: u64::MAX,
            ..Default::default()
        };
        assert!(table.insert(Path::new("/profile"), response()).is_err());
        assert!(table.entries.is_empty());
    }

    #[test]
    fn socket_is_bound_to_each_capability() {
        let mut table = PreparedProcesses::default();
        let first = table.insert(Path::new("/profile/a"), response()).unwrap();
        let second = table.insert(Path::new("/profile/b"), response()).unwrap();
        assert_eq!(
            table.lookup(first).unwrap().socket(),
            Path::new("/profile/a")
        );
        assert_eq!(
            table.lookup(second).unwrap().socket(),
            Path::new("/profile/b")
        );
    }

    #[test]
    fn cancellation_is_sticky_and_late_activation_cannot_restore_it() {
        let capability = LaunchCapability {
            response: response(),
            socket: PathBuf::from("/profile"),
            phase: AtomicU8::new(PREPARED),
            unpublished_cleanup: AtomicBool::new(false),
        };
        assert!(capability.begin_cancellation());
        assert_eq!(capability.phase(), CANCELLED);
        assert!(!capability.begin_activation());
    }
}
