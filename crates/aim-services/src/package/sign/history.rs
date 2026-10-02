//! SigningDetails certificate relationship rules at android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
//! These compare identities and capabilities; APK integrity must already
//! be verified separately before an owner authorizes code or UID reuse.
use super::{Lineage, SigningDetails};
use crate::package::settings::Signatures;

pub const INSTALLED_DATA: i32 = 1;
pub const SHARED_USER_ID: i32 = 2;
pub const ROLLBACK: i32 = 8;

#[derive(Clone, Copy)]
pub struct History<'a> {
    current: &'a [Vec<u8>],
    past: Option<&'a Lineage>,
}

impl<'a> History<'a> {
    pub fn verified(details: &'a SigningDetails) -> Self {
        Self {
            current: &details.signatures,
            past: details.past_signing_certificates.as_ref(),
        }
    }

    pub fn saved(details: &'a Signatures) -> Self {
        Self {
            current: &details.signatures,
            past: details.past_signatures.as_ref(),
        }
    }

    fn known(&self, other: &Self) -> bool {
        !self.current.is_empty() && !other.current.is_empty()
    }

    fn exact(&self, other: &Self) -> bool {
        self.current.len() == other.current.len()
            && self.current.iter().all(|s| other.current.contains(s))
            && other.current.iter().all(|s| self.current.contains(s))
    }

    fn past_certificates(&self) -> &[(Vec<u8>, i32)] {
        let past = self.past.map(Vec::as_slice).unwrap_or(&[]);
        // The last lineage entry is the current certificate. Its flags
        // do not restrict the current signer, which gets all capabilities.
        &past[..past.len().saturating_sub(1)]
    }

    fn certificate(&self, certificate: &[u8], flags: i32) -> bool {
        self.past_certificates()
            .iter()
            .any(|(cert, granted)| cert == certificate && (flags & granted) == flags)
            || (self.current.len() == 1 && self.current[0] == certificate)
    }

    pub fn check_capability(&self, old: &Self, flags: i32) -> bool {
        if !self.known(old) {
            return false;
        }
        if old.current.len() > 1 {
            self.exact(old)
        } else {
            self.certificate(&old.current[0], flags)
        }
    }

    pub fn has_ancestor(&self, old: &Self) -> bool {
        self.known(old)
            && old.current.len() == 1
            && self
                .past_certificates()
                .iter()
                .any(|(cert, _)| *cert == old.current[0])
    }

    pub fn has_ancestor_or_self(&self, old: &Self) -> bool {
        self.check_capability(old, 0)
    }

    /// The normal existing-package gate in verifySignatures. The caller
    /// supplies only an owner-authorized rollback. Legacy compatibility/
    /// certificate recovery and shared-user checks are separate gates.
    pub fn allows_update_from(&self, old: &Self, rollback: bool) -> bool {
        self.check_capability(old, INSTALLED_DATA)
            || old.check_capability(self, ROLLBACK)
            || (rollback && old.has_ancestor_or_self(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn capabilities_rotation_multisigners_unknown_and_rollback_follow_the_owner() {
        let a = Signatures {
            signatures: vec![vec![1]],
            ..Default::default()
        };
        let b = Signatures {
            signatures: vec![vec![2]],
            past_signatures: Some(vec![(vec![1], 3), (vec![2], 0)]),
            ..Default::default()
        };
        let revoked = Signatures {
            past_signatures: Some(vec![(vec![1], 0), (vec![2], 0)]),
            ..b.clone()
        };
        let unknown = Signatures::default();
        let multi = Signatures {
            signatures: vec![vec![1], vec![2]],
            ..Default::default()
        };
        let reversed = Signatures {
            signatures: vec![vec![2], vec![1]],
            ..Default::default()
        };
        let old = History::saved(&a);
        let new = History::saved(&b);
        assert!(new.check_capability(&old, INSTALLED_DATA | SHARED_USER_ID));
        assert!(!new.check_capability(&old, ROLLBACK));
        assert!(new.check_capability(&new, 31));
        assert!(new.has_ancestor(&old));
        assert!(!new.has_ancestor(&new));
        assert!(new.has_ancestor_or_self(&new));
        assert!(new.allows_update_from(&old, false));
        let revoked = History::saved(&revoked);
        assert!(!revoked.allows_update_from(&old, false));
        assert!(!old.allows_update_from(&revoked, false));
        assert!(old.allows_update_from(&revoked, true));
        assert!(!new.check_capability(&History::saved(&unknown), 0));
        assert!(History::saved(&multi).check_capability(&History::saved(&reversed), 31));
        assert!(!History::saved(&multi).check_capability(&old, 0));
    }
}
