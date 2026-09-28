//! Platform security decision required where Linux Binder would invoke its LSM
//! hook. Transaction mechanics never infer authorization from packet fields.

use crate::device::ProcessIdentity;

pub trait ContextManagerPolicy: Send + Sync {
    fn authorize(&self, identity: ProcessIdentity) -> Result<(), i32>;
}

/// Initial Android system identity rule for the Darwin platform boundary.
/// A future SELinux policy engine can replace this through `Device::with_policy`
/// without changing binder_proc, Parcel or transaction ownership.
pub struct AndroidSystemPolicy;

impl ContextManagerPolicy for AndroidSystemPolicy {
    fn authorize(&self, identity: ProcessIdentity) -> Result<(), i32> {
        matches!(identity.euid(), 0 | 1000)
            .then_some(())
            .ok_or(libc::EPERM)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_root_and_android_system_can_own_context_manager() {
        let policy = AndroidSystemPolicy;
        assert_eq!(
            policy.authorize(ProcessIdentity::new(1, 0).unwrap()),
            Ok(())
        );
        assert_eq!(
            policy.authorize(ProcessIdentity::new(2, 1000).unwrap()),
            Ok(())
        );
        assert_eq!(
            policy.authorize(ProcessIdentity::new(3, 10_000).unwrap()),
            Err(libc::EPERM)
        );
    }
}
