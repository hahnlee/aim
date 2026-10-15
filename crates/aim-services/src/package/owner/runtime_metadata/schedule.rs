//! Settings.writeStateForUserAsync debounce and bounded mutation delay.
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Schedule {
    entries: BTreeMap<i32, (Instant, Instant)>,
}
impl Schedule {
    pub fn request(&mut self, user: i32, now: Instant, delay: Duration) {
        let first = self
            .entries
            .get(&user)
            .map(|(first, _)| *first)
            .unwrap_or(now);
        let remaining =
            Duration::from_millis(2000).saturating_sub(now.saturating_duration_since(first));
        self.entries
            .insert(user, (first, now + delay.min(remaining)));
    }
    pub fn next(&self) -> Option<Instant> {
        self.entries.values().map(|(_, deadline)| *deadline).min()
    }
    pub fn due(&self, now: Instant) -> Vec<i32> {
        self.entries
            .iter()
            .filter_map(|(&user, (_, deadline))| (*deadline <= now).then_some(user))
            .collect()
    }
    pub fn remove(&mut self, user: i32) {
        self.entries.remove(&user);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_mutations_never_move_the_first_deadline_beyond_two_seconds() {
        let start = Instant::now();
        let mut schedule = Schedule::default();
        schedule.request(0, start, Duration::from_millis(700));
        assert_eq!(schedule.next(), Some(start + Duration::from_millis(700)));
        schedule.request(
            0,
            start + Duration::from_millis(600),
            Duration::from_millis(1300),
        );
        assert_eq!(schedule.next(), Some(start + Duration::from_millis(1900)));
        schedule.request(
            0,
            start + Duration::from_millis(1800),
            Duration::from_millis(1000),
        );
        assert_eq!(schedule.next(), Some(start + Duration::from_millis(2000)));
        assert!(schedule.due(start + Duration::from_millis(1999)).is_empty());
        assert_eq!(schedule.due(start + Duration::from_millis(2000)), [0]);
        schedule.request(
            0,
            start + Duration::from_millis(2500),
            Duration::from_millis(1000),
        );
        assert_eq!(schedule.next(), Some(start + Duration::from_millis(2500)));
        schedule.remove(0);
        assert!(schedule.next().is_none());
        schedule.request(
            0,
            start + Duration::from_secs(3),
            Duration::from_millis(1000),
        );
        assert_eq!(schedule.next(), Some(start + Duration::from_secs(4)));
    }
}
