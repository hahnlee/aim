//! Per-image Android visibility, independent of Darwin's RTLD bit values.
use crate::linker_load_flags::LoadFlags;
use std::sync::Arc;

#[derive(Debug, Default)]
pub struct ImageVisibility {
    flags_1: u64,
    load: Arc<LoadFlags>,
}

impl ImageVisibility {
    pub fn new(flags_1: u64, rtld_global: bool) -> Self {
        Self {
            flags_1,
            // AOSP soinfo::set_dt_flags_1 promotes both GLOBAL and NODELETE.
            load: Arc::new(LoadFlags::new(
                rtld_global || flags_1 & 2 != 0,
                flags_1 & 8 != 0,
            )),
        }
    }
    pub fn promote_global(&self) {
        self.load.promote_global();
    }
    pub fn promote_nodelete(&self) {
        self.load.promote_nodelete();
    }
    /// Effective GLOBAL/NODELETE flags. The linker must separately establish
    /// linked state, original local-group root and balanced open/dependency refs.
    pub fn retention_flags(&self) -> (bool, bool) {
        self.load.snapshot()
    }
    /// AOSP dlsym_linear_lookup only, never dlsym on an ordinary handle or
    /// relocation/global-group construction. Unknown SDK is not an old SDK.
    pub fn linear_lookup_eligible(&self, linked_sdk: Option<i32>) -> Option<bool> {
        if self.load.snapshot().0 {
            Some(true)
        } else {
            linked_sdk.map(|sdk| sdk < 23)
        }
    }
    pub(crate) fn retained_flags(&self) -> Arc<LoadFlags> {
        self.load.clone()
    }
    pub fn in_shared_group(&self, default_parent: bool) -> bool {
        if default_parent {
            self.flags_1 & 2 != 0
        } else {
            self.load.snapshot().0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linear_lookup_uses_linked_sdk_without_changing_other_visibility() {
        let local = ImageVisibility::new(0, false);
        assert_eq!(local.linear_lookup_eligible(Some(22)), Some(true));
        assert_eq!(local.linear_lookup_eligible(Some(23)), Some(false));
        assert_eq!(local.linear_lookup_eligible(Some(36)), Some(false));
        assert_eq!(local.linear_lookup_eligible(None), None);
        assert!(!local.in_shared_group(false));
        local.promote_nodelete();
        assert_eq!(local.linear_lookup_eligible(Some(23)), Some(false));
        local.promote_global();
        assert_eq!(local.linear_lookup_eligible(Some(36)), Some(true));
        assert_eq!(local.linear_lookup_eligible(None), Some(true));
        assert_eq!(
            ImageVisibility::new(2, false).linear_lookup_eligible(None),
            Some(true)
        );
    }
    #[test]
    fn nodelete_promotion_does_not_grant_global_visibility() {
        let nodelete = ImageVisibility::new(8, false);
        assert_eq!(nodelete.retention_flags(), (false, true));
        assert!(!nodelete.in_shared_group(true));
        assert!(!nodelete.in_shared_group(false));
        let local = ImageVisibility::new(1, false);
        assert_eq!(local.retention_flags(), (false, false));
        local.promote_nodelete();
        assert_eq!(local.retention_flags(), (false, true));
        local.promote_global();
        assert_eq!(local.retention_flags(), (true, true));
        assert!(!local.in_shared_group(true));
        assert!(local.in_shared_group(false));
        assert_eq!(
            ImageVisibility::new(2 | 8, false).retention_flags(),
            (true, true)
        );
    }
    #[test]
    fn android_global_groups_are_distinct() {
        let dynamic = ImageVisibility::new(2, false);
        assert!(dynamic.in_shared_group(true));
        assert!(dynamic.in_shared_group(false));
        let runtime = ImageVisibility::new(1, true); // DF_1_NOW isn't GLOBAL.
        assert!(!runtime.in_shared_group(true));
        assert!(runtime.in_shared_group(false));
        let local = ImageVisibility::new(0, false);
        assert!(!local.in_shared_group(false));
        local.promote_global();
        assert!(local.in_shared_group(false));
        assert!(!local.in_shared_group(true));
    }
}
