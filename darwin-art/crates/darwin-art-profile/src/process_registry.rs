//! Profile process-instance and lease lifetime ownership, independent of IPC.
use crate::process_incarnation::ProcessIncarnation;
use crate::{registry::validate_package, ProfileError};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProcessLease {
    pid: u32,
    generation: u64,
    lease: u64,
}

struct ProcessEntry {
    incarnation: ProcessIncarnation,
    package: String,
    android_uid: Option<u32>,
    generation: u64,
    leases: BTreeSet<u64>,
    child_owner: Option<u64>,
}

#[derive(Default)]
pub(crate) struct ProcessRegistry {
    entries: BTreeMap<u32, ProcessEntry>,
    next: u64,
}

impl ProcessRegistry {
    /// Caller has authenticated the peer PID or owns the newly spawned Child.
    /// A child may acquire its socket lease before spawn registration completes.
    pub(crate) fn acquire(
        &mut self,
        pid: u32,
        package: &str,
        incarnation: ProcessIncarnation,
        child_owner: bool,
    ) -> Result<ProcessLease, ProfileError> {
        self.acquire_with_uid(pid, package, incarnation, child_owner, None)
    }

    /// Register a daemon-owned Android process with its execution UID.
    ///
    /// Ordinary package processes may continue deriving their UID from the
    /// installed package record. Isolated processes must retain the UID that
    /// system_server allocated for this exact launch instead.
    pub(crate) fn acquire_with_uid(
        &mut self,
        pid: u32,
        package: &str,
        incarnation: ProcessIncarnation,
        child_owner: bool,
        android_uid: Option<u32>,
    ) -> Result<ProcessLease, ProfileError> {
        validate_package(package)?;
        if pid == 0 || pid > i32::MAX as u32 {
            return Err(ProfileError::Daemon("invalid process PID".into()));
        }
        if let Some(entry) = self
            .entries
            .get(&pid)
            .filter(|entry| entry.incarnation == incarnation)
        {
            if entry.package != package {
                return Err(ProfileError::Daemon(
                    "PID belongs to another package".into(),
                ));
            }
            if child_owner && entry.child_owner.is_some() {
                return Err(ProfileError::Daemon(
                    "process already has a child owner".into(),
                ));
            }
            if android_uid.is_some() && entry.android_uid != android_uid {
                return Err(ProfileError::Daemon(
                    "process Android UID does not match its existing registration".into(),
                ));
            }
        }
        let lease = self
            .next
            .checked_add(1)
            .ok_or_else(|| ProfileError::Daemon("process lease sequence exhausted".into()))?;
        self.next = lease;
        if self
            .entries
            .get(&pid)
            .is_some_and(|entry| entry.incarnation != incarnation)
        {
            self.entries.remove(&pid);
        }
        let entry = self.entries.entry(pid).or_insert_with(|| ProcessEntry {
            incarnation,
            package: package.into(),
            android_uid,
            generation: lease,
            leases: BTreeSet::new(),
            child_owner: None,
        });
        entry.leases.insert(lease);
        if child_owner {
            entry.child_owner = Some(lease);
        }
        Ok(ProcessLease {
            pid,
            generation: entry.generation,
            lease,
        })
    }

    /// Socket EOF releases only its own lease, never a later use of the PID.
    pub(crate) fn release(&mut self, token: ProcessLease) {
        if let Some(entry) = self.entries.get_mut(&token.pid) {
            if entry.generation != token.generation || entry.child_owner == Some(token.lease) {
                return;
            }
            entry.leases.remove(&token.lease);
            if entry.leases.is_empty() {
                self.entries.remove(&token.pid);
            }
        }
    }

    /// Only the matching Child waiter can retire an instance. Child exit is
    /// authoritative even if a supervisor still holds an old socket lease.
    pub(crate) fn child_exited(&mut self, token: ProcessLease) {
        if self.entries.get(&token.pid).is_some_and(|entry| {
            entry.generation == token.generation && entry.child_owner == Some(token.lease)
        }) {
            self.entries.remove(&token.pid);
        }
    }

    pub(crate) fn package(&self, pid: u32, incarnation: ProcessIncarnation) -> Option<&str> {
        self.entries
            .get(&pid)
            .filter(|entry| entry.incarnation == incarnation)
            .map(|entry| entry.package.as_str())
    }

    pub(crate) fn android_uid(&self, pid: u32, incarnation: ProcessIncarnation) -> Option<u32> {
        self.entries
            .get(&pid)
            .filter(|entry| entry.incarnation == incarnation)
            .and_then(|entry| entry.android_uid)
    }

    /// True only for a process whose lease was installed by this daemon's
    /// direct `Child` owner.  A same-user socket lease, including one whose
    /// caller supplied the string `android.system`, is not system authority.
    pub(crate) fn is_child_owner(
        &self,
        pid: u32,
        incarnation: ProcessIncarnation,
        package: &str,
    ) -> bool {
        self.entries.get(&pid).is_some_and(|entry| {
            entry.incarnation == incarnation
                && entry.package == package
                && entry.child_owner.is_some()
        })
    }

    pub(crate) fn contains_package(&self, package: &str) -> bool {
        self.entries.values().any(|entry| entry.package == package)
    }

    /// True when replacing one profile runtime would strand another Android
    /// process generation. The runtime owner itself is deliberately excluded.
    pub(crate) fn contains_package_other_than(&self, package: &str) -> bool {
        self.entries.values().any(|entry| entry.package != package)
    }

    /// A socket-only package registration belongs to a caller-owned process
    /// (for example the legacy host service manager), not to this daemon's
    /// `Child`.  New bound-service launches refuse to overlap one.
    pub(crate) fn contains_unowned_package(&self, package: &str) -> bool {
        self.entries
            .values()
            .any(|entry| entry.package == package && entry.child_owner.is_none())
    }

    pub(crate) fn processes(&self) -> impl Iterator<Item = (u32, &str)> {
        self.entries
            .iter()
            .map(|(pid, entry)| (*pid, entry.package.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reused_same_package_pid_before_old_waiter_completes_is_distinct() {
        let mut registry = ProcessRegistry::default();
        let old_child = registry
            .acquire(42, "android.system", ProcessIncarnation::fixture(1), true)
            .unwrap();
        let old_socket = registry
            .acquire(42, "android.system", ProcessIncarnation::fixture(1), false)
            .unwrap();
        let new_socket = registry
            .acquire(42, "android.system", ProcessIncarnation::fixture(2), false)
            .unwrap();
        let new_child = registry
            .acquire(42, "android.system", ProcessIncarnation::fixture(2), true)
            .unwrap();
        registry.child_exited(old_child);
        registry.release(old_socket);
        assert_eq!(
            registry.package(42, ProcessIncarnation::fixture(2)),
            Some("android.system")
        );
        registry.release(new_socket);
        assert_eq!(
            registry.package(42, ProcessIncarnation::fixture(2)),
            Some("android.system")
        );
        registry.child_exited(new_child);
        assert_eq!(registry.package(42, ProcessIncarnation::fixture(2)), None);
    }

    #[test]
    fn query_cannot_attribute_old_registration_to_a_new_kernel_process() {
        let mut registry = ProcessRegistry::default();
        registry
            .acquire(42, "android.system", ProcessIncarnation::fixture(1), true)
            .unwrap();
        assert_eq!(registry.package(42, ProcessIncarnation::fixture(2)), None);
        assert_eq!(
            registry.package(42, ProcessIncarnation::fixture(1)),
            Some("android.system")
        );
    }

    #[test]
    fn child_early_socket_registration_and_exit_have_one_owner() {
        let mut registry = ProcessRegistry::default();
        let socket = registry
            .acquire(42, "android.system", ProcessIncarnation::fixture(1), false)
            .unwrap();
        let child = registry
            .acquire(42, "android.system", ProcessIncarnation::fixture(1), true)
            .unwrap();
        registry.release(socket);
        assert_eq!(
            registry.package(42, ProcessIncarnation::fixture(1)),
            Some("android.system")
        );
        registry.release(child); // A socket operation cannot retire the child.
        assert!(registry.contains_package("android.system"));
        registry.child_exited(child);
        assert_eq!(registry.processes().count(), 0);
    }

    #[test]
    fn stale_socket_or_child_token_cannot_remove_reused_pid() {
        let mut registry = ProcessRegistry::default();
        let old_child = registry
            .acquire(42, "org.example.one", ProcessIncarnation::fixture(1), true)
            .unwrap();
        let old_socket = registry
            .acquire(42, "org.example.one", ProcessIncarnation::fixture(1), false)
            .unwrap();
        registry.child_exited(old_child);
        let new_socket = registry
            .acquire(42, "org.example.two", ProcessIncarnation::fixture(1), false)
            .unwrap();
        registry.release(old_socket);
        registry.child_exited(old_child);
        assert_eq!(
            registry.package(42, ProcessIncarnation::fixture(1)),
            Some("org.example.two")
        );
        registry.release(new_socket);
        assert_eq!(registry.package(42, ProcessIncarnation::fixture(1)), None);
    }

    #[test]
    fn release_is_per_lease_and_idempotent() {
        let mut registry = ProcessRegistry::default();
        let a = registry
            .acquire(42, "org.example.one", ProcessIncarnation::fixture(1), false)
            .unwrap();
        let b = registry
            .acquire(42, "org.example.one", ProcessIncarnation::fixture(1), false)
            .unwrap();
        registry.release(a);
        registry.release(a);
        assert!(registry.contains_package("org.example.one"));
        registry.child_exited(b); // Not a Child owner.
        assert!(registry.contains_package("org.example.one"));
        registry.release(b);
        assert!(!registry.contains_package("org.example.one"));
    }

    #[test]
    fn registration_conflicts_do_not_replace_existing_owner() {
        let mut registry = ProcessRegistry::default();
        let owner = registry
            .acquire(42, "android.system", ProcessIncarnation::fixture(1), true)
            .unwrap();
        assert!(registry
            .acquire(
                42,
                "org.example.other",
                ProcessIncarnation::fixture(1),
                false
            )
            .is_err());
        assert!(registry
            .acquire(42, "android.system", ProcessIncarnation::fixture(1), true)
            .is_err());
        assert!(registry
            .acquire(0, "android.system", ProcessIncarnation::fixture(1), false)
            .is_err());
        assert_eq!(
            registry.package(42, ProcessIncarnation::fixture(1)),
            Some("android.system")
        );
        registry.child_exited(owner);
    }

    #[test]
    fn replacement_idle_check_excludes_only_the_runtime_owner() {
        let mut registry = ProcessRegistry::default();
        registry
            .acquire(42, "android.system", ProcessIncarnation::fixture(1), true)
            .unwrap();
        assert!(!registry.contains_package_other_than("android.system"));
        registry
            .acquire(43, "org.example.app", ProcessIncarnation::fixture(1), true)
            .unwrap();
        assert!(registry.contains_package_other_than("android.system"));
    }

    #[test]
    fn only_daemon_child_lease_is_system_authority() {
        let mut registry = ProcessRegistry::default();
        let incarnation = ProcessIncarnation::fixture(9);
        let socket = registry
            .acquire(42, "android.system", incarnation, false)
            .unwrap();
        assert!(!registry.is_child_owner(42, incarnation, "android.system"));
        registry.release(socket);

        registry
            .acquire(42, "android.system", incarnation, true)
            .unwrap();
        assert!(registry.is_child_owner(42, incarnation, "android.system"));
        assert!(!registry.is_child_owner(42, incarnation, "org.example.app"));
    }

    #[test]
    fn legacy_socket_only_process_blocks_new_daemon_child_overlap() {
        let mut registry = ProcessRegistry::default();
        let incarnation = ProcessIncarnation::fixture(10);
        let lease = registry
            .acquire(42, "org.example.app", incarnation, false)
            .unwrap();
        assert!(registry.contains_unowned_package("org.example.app"));
        registry.release(lease);
        assert!(!registry.contains_unowned_package("org.example.app"));
    }

    #[test]
    fn daemon_child_retains_isolated_android_uid() {
        let mut registry = ProcessRegistry::default();
        let incarnation = ProcessIncarnation::fixture(11);
        registry
            .acquire_with_uid(42, "org.example.app", incarnation, true, Some(99_042))
            .unwrap();
        assert_eq!(registry.android_uid(42, incarnation), Some(99_042));
        assert!(registry
            .acquire_with_uid(42, "org.example.app", incarnation, false, Some(99_043),)
            .is_err());
    }
}
