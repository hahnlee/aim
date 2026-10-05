//! ApkSignatureVerifier test override ownership, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::SigningDetails;
use std::sync::{Arc, Mutex};

const DEBUG_ONLY: &str = "This test API is only available on debuggable builds";

impl SigningDetails {
    /// SigningDetails.equals for native verified owners. Current capability flags
    /// do not participate; past certificates retain their order and flags.
    pub fn equals_original(&self, other: &Self) -> bool {
        self.scheme_version == other.scheme_version
            && self.signatures_match(other)
            && match (&self.public_keys, &other.public_keys) {
                (None, None) => true,
                (Some(a), Some(b)) => {
                    a.iter().all(|key| b.contains(key)) && b.iter().all(|key| a.contains(key))
                }
                _ => false,
            }
            && self.past_signing_certificates == other.past_signing_certificates
    }

    fn override_key_matches(&self, other: &Self) -> bool {
        // ArrayMap first compares hash codes. SigningDetails hashes the current
        // Signature array in order although equals compares it as a signer set.
        // Equal owners share all other hash terms, so this term suffices.
        let signature_hash = |details: &Self| {
            if details.unknown {
                return 0i32;
            }
            details.signatures.iter().fold(1i32, |hash, cert| {
                let cert_hash = cert.iter().fold(1i32, |hash, byte| {
                    hash.wrapping_mul(31).wrapping_add(*byte as i8 as i32)
                });
                hash.wrapping_mul(31).wrapping_add(cert_hash)
            })
        };
        self.equals_original(other) && signature_hash(self) == signature_hash(other)
    }
}

#[derive(Clone, Debug)]
pub struct OverrideSnapshot {
    pub version: i64,
    pairs: Vec<(SigningDetails, SigningDetails)>,
}
impl OverrideSnapshot {
    pub fn apply(&self, details: &SigningDetails) -> SigningDetails {
        self.pairs
            .iter()
            .find(|(old, _)| old.override_key_matches(details))
            .map_or_else(|| details.clone(), |(_, new)| new.clone())
    }
}

/// Explicit build policy and service-lifetime state, never a process global.
/// The transport owner must obtain the policy from the pinned image.
pub struct Overrides {
    debuggable: bool,
    current: Mutex<Arc<OverrideSnapshot>>,
}
impl Overrides {
    pub fn new(debuggable: bool) -> Self {
        Self {
            debuggable,
            current: Mutex::new(Arc::new(OverrideSnapshot {
                version: 1,
                pairs: vec![],
            })),
        }
    }
    pub fn snapshot(&self) -> Arc<OverrideSnapshot> {
        self.current.lock().unwrap().clone()
    }
    pub fn apply(&self, details: &SigningDetails) -> SigningDetails {
        self.snapshot().apply(details)
    }
    fn change(
        &self,
        edit: impl FnOnce(&mut Vec<(SigningDetails, SigningDetails)>),
    ) -> Result<i64, String> {
        if !self.debuggable {
            return Err(DEBUG_ONLY.into());
        }
        let mut current = self.current.lock().unwrap();
        let version = current
            .version
            .checked_add(1)
            .ok_or("signing override version exhausted")?;
        let mut pairs = current.pairs.clone();
        edit(&mut pairs);
        *current = Arc::new(OverrideSnapshot { version, pairs });
        Ok(version)
    }
    pub fn add(&self, old: SigningDetails, new: SigningDetails) -> Result<i64, String> {
        self.change(|pairs| {
            if let Some(pair) = pairs
                .iter_mut()
                .find(|(key, _)| key.override_key_matches(&old))
            {
                pair.1 = new;
            } else {
                pairs.push((old, new));
            }
        })
    }
    pub fn remove(&self, old: &SigningDetails) -> Result<i64, String> {
        self.change(|pairs| pairs.retain(|(key, _)| !key.override_key_matches(old)))
    }
    pub fn clear(&self) -> Result<i64, String> {
        self.change(Vec::clear)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn details(cert: u8) -> SigningDetails {
        SigningDetails {
            unknown: false,
            signatures: vec![vec![cert]],
            current_flags: vec![],
            scheme_version: 3,
            public_keys: Some(vec![Some(vec![cert])]),
            past_signing_certificates: None,
        }
    }
    #[test]
    fn lookup_retains_array_map_hash_order_and_past_capabilities() {
        let owner = Overrides::new(true);
        let mut a = details(1);
        a.signatures.push(vec![2]);
        a.public_keys.as_mut().unwrap().push(Some(vec![2]));
        let b = details(3);
        owner.add(a.clone(), b.clone()).unwrap();
        let mut reversed = a.clone();
        reversed.signatures.reverse();
        assert!(a.equals_original(&reversed));
        assert_eq!(owner.apply(&reversed), reversed);
        let mut reordered_keys = a.clone();
        reordered_keys.public_keys.as_mut().unwrap().reverse();
        assert_eq!(owner.apply(&reordered_keys), b);
        let mut past = details(1);
        past.past_signing_certificates = Some(vec![(vec![2], 8), (vec![1], 0)]);
        owner.add(past.clone(), b.clone()).unwrap();
        let mut changed = past.clone();
        changed.past_signing_certificates.as_mut().unwrap()[0].1 = 1;
        assert!(!past.equals_original(&changed));
        assert_eq!(owner.apply(&changed), changed);
        changed = past.clone();
        changed
            .past_signing_certificates
            .as_mut()
            .unwrap()
            .reverse();
        assert!(!past.equals_original(&changed));
        changed = past.clone();
        changed.scheme_version = 2;
        assert!(!past.equals_original(&changed));
    }

    #[test]
    fn overrides_use_original_equality_and_keep_prior_versions_without_chaining() {
        let owner = Overrides::new(true);
        let a = details(1);
        let b = details(2);
        let c = details(3);
        let empty = owner.snapshot();
        owner.add(a.clone(), b.clone()).unwrap();
        let first = owner.snapshot();
        owner.add(b.clone(), c.clone()).unwrap();
        assert_eq!(owner.apply(&a), b);
        let mut equivalent = a.clone();
        equivalent.current_flags = vec![31];
        owner.add(equivalent.clone(), c.clone()).unwrap();
        assert_eq!(owner.apply(&a), c);
        assert_eq!(first.apply(&a), b);
        assert_eq!(empty.apply(&a), a);
        owner.remove(&equivalent).unwrap();
        assert_eq!(owner.apply(&a), a);
        assert_eq!(owner.apply(&b), c);
        owner.clear().unwrap();
        assert_eq!(owner.apply(&b), b);
        let release = Overrides::new(false);
        assert_eq!(release.add(a.clone(), b).unwrap_err(), DEBUG_ONLY);
        assert_eq!(release.remove(&a).unwrap_err(), DEBUG_ONLY);
        assert_eq!(release.clear().unwrap_err(), DEBUG_ONLY);
        assert_eq!(release.snapshot().version, 1);
        let mut exhausted = owner.current.lock().unwrap();
        *exhausted = Arc::new(OverrideSnapshot {
            version: i64::MAX,
            pairs: vec![(a.clone(), c.clone())],
        });
        drop(exhausted);
        assert!(owner.clear().is_err());
        assert_eq!(owner.apply(&a), c);
    }
}
