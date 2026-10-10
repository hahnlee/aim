//! SilentUpdatePolicy, android-16.0.0_r1 (AOSP, Apache License 2.0).
use std::collections::BTreeMap;
const DEFAULT_THROTTLE_MS: i64 = 30_000;
#[derive(Clone, Debug)]
pub struct Policy {
    updates: BTreeMap<(String, String), i64>,
    unlimited: Option<String>,
    throttle_ms: i64,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            updates: BTreeMap::new(),
            unlimited: None,
            throttle_ms: DEFAULT_THROTTLE_MS,
        }
    }
}
impl Policy {
    pub fn allowed(&self, installer: Option<&str>, package: &str, uptime_ms: i64) -> bool {
        let Some(installer) = installer else {
            return true;
        };
        let last = self
            .updates
            .get(&(installer.into(), package.into()))
            .copied()
            .unwrap_or(-1);
        uptime_ms.wrapping_sub(last) > self.throttle_ms
    }
    pub fn track(&mut self, installer: Option<&str>, package: &str, uptime_ms: i64) {
        let Some(installer) = installer else { return };
        if self.unlimited.as_deref() == Some(installer) {
            return;
        }
        self.updates
            .retain(|_, last| uptime_ms.wrapping_sub(*last) <= self.throttle_ms);
        self.updates
            .insert((installer.into(), package.into()), uptime_ms);
    }
    pub fn set_unlimited(&mut self, installer: Option<String>) {
        if installer.is_none() {
            self.updates.clear();
        }
        self.unlimited = installer;
    }
    pub fn set_throttle_seconds(&mut self, seconds: i64) {
        self.throttle_ms = if seconds >= 0 {
            seconds.saturating_mul(1000)
        } else {
            DEFAULT_THROTTLE_MS
        };
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unlimited_skips_tracking_but_does_not_erase_existing_throttle_until_reset() {
        let mut policy = Policy::default();
        policy.track(Some("installer"), "app", 40_000);
        policy.set_unlimited(Some("installer".into()));
        policy.track(Some("installer"), "app", 45_000);
        assert!(!policy.allowed(Some("installer"), "app", 70_000));
        assert!(policy.allowed(Some("installer"), "app", 70_001));
        policy.set_unlimited(None);
        assert!(policy.allowed(Some("installer"), "app", 40_000));
        assert!(policy.allowed(None, "app", 0));
    }
    #[test]
    fn strict_deadline_pruning_negative_reset_and_timeunit_saturation() {
        let mut policy = Policy::default();
        assert!(!policy.allowed(Some("installer"), "missing", 29_999));
        assert!(policy.allowed(Some("installer"), "missing", 30_000));
        policy.track(Some("installer"), "old", 40_000);
        policy.track(Some("installer"), "new", 70_000);
        assert!(
            policy
                .updates
                .contains_key(&("installer".into(), "old".into()))
        );
        policy.track(Some("installer"), "later", 70_001);
        assert!(
            !policy
                .updates
                .contains_key(&("installer".into(), "old".into()))
        );
        policy.set_throttle_seconds(i64::MAX);
        assert_eq!(policy.throttle_ms, i64::MAX);
        policy.set_throttle_seconds(-2);
        assert_eq!(policy.throttle_ms, 30_000);
        policy.set_throttle_seconds(0);
        assert!(!policy.allowed(Some("installer"), "later", 70_001));
        assert!(policy.allowed(Some("installer"), "later", 70_002));
    }
}
