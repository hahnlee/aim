//! SigningDetails.mergeLineageWith at android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{History, SigningDetails};
use crate::package::settings::Signatures;
use std::borrow::Cow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MergeRule {
    SelfCapability,
    OtherCapability,
    RestrictedCapability,
}

impl SigningDetails {
    /// Decode saved certificates and reconstruct their SPKI key set.
    /// This does not verify any APK or authorize reuse of its identity.
    pub fn from_saved(saved: &Signatures) -> Result<Self, String> {
        Self::new(
            saved.signatures.clone(),
            saved.scheme_version,
            saved.past_signatures.clone(),
        )
    }

    /// A borrowed result preserves the original's unchanged-instance
    /// signal, including returning the other instance when it supplies
    /// the only lineage. Invalid stored certificates fail explicitly.
    pub fn merge_lineage_with<'a>(
        &'a self,
        other: &'a Self,
        rule: MergeRule,
    ) -> Result<Cow<'a, Self>, String> {
        let has_past = |s: &Self| {
            s.past_signing_certificates
                .as_ref()
                .is_some_and(|p| !p.is_empty())
        };
        if !has_past(self) {
            return Ok(Cow::Borrowed(
                if has_past(other)
                    && History::verified(other).has_ancestor_or_self(&History::verified(self))
                {
                    other
                } else {
                    self
                },
            ));
        }
        if !has_past(other)
            || !History::verified(self).has_common_ancestor(&History::verified(other))
        {
            return Ok(Cow::Borrowed(self));
        }
        let (descendant, ancestor, rule) =
            if History::verified(self).has_ancestor_or_self(&History::verified(other)) {
                (self, other, rule)
            } else {
                let flipped = match rule {
                    MergeRule::SelfCapability => MergeRule::OtherCapability,
                    MergeRule::OtherCapability => MergeRule::SelfCapability,
                    MergeRule::RestrictedCapability => rule,
                };
                (other, self, flipped)
            };
        let d = descendant.past_signing_certificates.as_ref().unwrap();
        let a = ancestor.past_signing_certificates.as_ref().unwrap();
        let Some(common) = d.iter().rposition(|(s, _)| *s == a.last().unwrap().0) else {
            return Ok(Cow::Borrowed(descendant));
        };
        let mut merged: Vec<_> = d[common + 1..].iter().rev().cloned().collect();
        let mut di = common + 1;
        let mut ai = a.len();
        let mut modified = false;
        loop {
            di -= 1;
            ai -= 1;
            let (certificate, flags) = &d[di];
            let (_, other_flags) = &a[ai];
            modified |= flags != other_flags;
            let flags = match rule {
                MergeRule::SelfCapability => *flags,
                MergeRule::OtherCapability => *other_flags,
                MergeRule::RestrictedCapability => flags & other_flags,
            };
            merged.push((certificate.clone(), flags));
            if di == 0 || ai == 0 || d[di - 1].0 != a[ai - 1].0 {
                break;
            }
        }
        if di > 0 && ai > 0 {
            return Ok(Cow::Borrowed(descendant));
        }
        merged.extend(a[..ai].iter().rev().cloned());
        merged.extend(d[..di].iter().rev().cloned());
        if merged.len() == d.len() && !modified {
            return Ok(Cow::Borrowed(descendant));
        }
        merged.reverse();
        Self::new(
            vec![descendant.signatures[0].clone()],
            descendant.scheme_version,
            Some(merged),
        )
        .map(Cow::Owned)
    }
}
