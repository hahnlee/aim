//! AppsFilterImpl's UID-scoped interaction grants and package lifecycle cleanup.
//! Ported from android-16.0.0_r1, Copyright (C) The Android Open Source
//! Project, Apache License 2.0. These grants are memory state, not settings XML.
use std::collections::HashSet;

use super::uid;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImplicitAccess {
    transient: HashSet<(i32, i32)>,
    retained: HashSet<(i32, i32)>,
}

impl ImplicitAccess {
    /// grantImplicitAccess: the recipient gains visibility of the visible UID.
    /// Each retention class deduplicates independently; self-grants do nothing.
    pub fn grant(&mut self, recipient: i32, visible: i32, retain_on_update: bool) -> bool {
        if recipient == visible {
            return false;
        }
        let grants = if retain_on_update {
            &mut self.retained
        } else {
            &mut self.transient
        };
        grants.insert((recipient, visible))
    }

    pub(super) fn transient(&self, recipient: i32, visible: i32) -> bool {
        self.transient.contains(&(recipient, visible))
    }

    pub(super) fn visible(&self, recipient: i32, visible: i32) -> bool {
        self.transient(recipient, visible) || self.retained.contains(&(recipient, visible))
    }

    /// All resolved users must be supplied. A shared app ID loses its grants
    /// too: re-adding surviving members restores static relations, not grants.
    pub fn remove_package(&mut self, app_id: i32, users: &[i32]) {
        self.clear(app_id, users, true);
    }

    /// Replacement always keeps retained grants; its caller decides whether
    /// ordinary grants survive. This is removePackageInternal's replace path.
    pub fn replace_package(&mut self, app_id: i32, users: &[i32], retain_implicit_grants: bool) {
        if !retain_implicit_grants {
            self.clear(app_id, users, false);
        }
    }

    fn clear(&mut self, app_id: i32, users: &[i32], remove_retained: bool) {
        let removed: HashSet<i32> = users.iter().map(|&user| uid(user, app_id)).collect();
        let keep = |&(recipient, visible): &(i32, i32)| {
            !removed.contains(&recipient) && !removed.contains(&visible)
        };
        self.transient.retain(keep);
        if remove_retained {
            self.retained.retain(keep);
        }
    }
}
