//! Pinned BatteryStatsImpl: wake timer pools, screen-off/background bases and PID spans.
use super::super::power_service::Attribution;
use super::*;

#[derive(Debug, Default)]
struct WakeUid {
    names: NamedTimers<[Option<DualTimer>; 3]>,
    aggregate: DualTimer,
    background: TimeBase,
    foreground: bool,
    pids: BTreeMap<i32, (i32, i64, i64)>,
}

#[derive(Debug, Default)]
pub(super) struct WakeStats {
    battery: TimeBase,
    off: TimeBase,
    screen_off: bool,
    uids: BTreeMap<i32, WakeUid>,
    mapped: BTreeMap<i32, (i32, usize)>,
}

impl WakeStats {
    fn base(&self, kind: usize) -> &TimeBase {
        if kind == 0 { &self.off } else { &self.battery }
    }

    fn refresh(&mut self, kind: usize, now: i64) {
        let rt = self.base(kind).realtime(now * 1000);
        let count = self
            .uids
            .values()
            .flat_map(|u| u.names.timers.values())
            .filter(|ts| ts[kind].as_ref().is_some_and(|t| t.timer.timer.nesting > 0))
            .count();
        for t in self
            .uids
            .values_mut()
            .flat_map(|u| u.names.timers.values_mut())
            .filter_map(|ts| ts[kind].as_mut())
        {
            t.timer.timer.time_stopped_in(rt, count);
        }
    }

    fn background(u: &mut WakeUid, running: bool, now: i64) {
        if let Some(rt) = u
            .background
            .set_running(running && !u.foreground, now * 1000)
        {
            for t in u
                .names
                .timers
                .values_mut()
                .filter_map(|ts| ts[0].as_mut())
                .chain(std::iter::once(&mut u.aggregate))
            {
                if u.background.running {
                    t.background.time_started(rt);
                } else {
                    t.background.time_stopped_in(rt, 0);
                }
            }
        }
    }

    pub(super) fn bases(&mut self, battery: bool, now: i64) {
        let off = battery && self.screen_off;
        for kind in 0..3 {
            if self.base(kind).running != if kind == 0 { off } else { battery } {
                self.refresh(kind, now);
            }
        }
        let changes = [
            self.off.set_running(off, now * 1000),
            self.battery.set_running(battery, now * 1000),
        ];
        for (kind, change) in [changes[0], changes[1], changes[1]].into_iter().enumerate() {
            if let Some(rt) = change {
                for u in self.uids.values_mut() {
                    for t in u
                        .names
                        .timers
                        .values_mut()
                        .filter_map(|ts| ts[kind].as_mut())
                        .chain((kind == 0).then_some(&mut u.aggregate))
                    {
                        if if kind == 0 { off } else { battery } {
                            t.timer.time_started(rt);
                        } else {
                            t.timer.time_stopped_in(rt, 0);
                        }
                    }
                }
            }
        }
        for u in self.uids.values_mut() {
            Self::background(u, off, now);
        }
    }

    pub(super) fn process(&mut self, uid: i32, background: bool, now: i64) {
        let u = self.uids.entry(uid).or_default();
        u.foreground = !background;
        Self::background(u, self.off.running, now);
    }

    fn note(&mut self, start: bool, uid: i32, a: &Attribution, now: i64, limit: usize) {
        let Some(kind) = a.kind else { return };
        if uid < 0 {
            return;
        }
        let u = self.uids.entry(uid).or_default();
        let boundary = u.names.timer(Some(&a.tag), start, limit).map(|timers| {
            let t = timers[kind].get_or_insert_with(DualTimer::default);
            t.timer.timer.nesting == if start { 0 } else { 1 }
        });
        if boundary == Some(true) {
            self.refresh(kind, now);
        }
        let base = if kind == 0 { &self.off } else { &self.battery };
        let u = self.uids.get_mut(&uid).unwrap();
        if boundary.is_some() {
            let key = if u.names.timers.contains_key(&a.tag) {
                a.tag.as_str()
            } else {
                OVERFLOW_NAME
            };
            let t = u.names.timers.get_mut(key).unwrap()[kind].as_mut().unwrap();
            if start {
                t.timer.start(base, now);
                if kind == 0 {
                    t.background.start(&u.background, now);
                }
            } else {
                t.timer.stop(base, now);
                if kind == 0 {
                    t.background.stop(&u.background, now);
                }
            }
        }
        if kind == 0 {
            if start {
                u.aggregate.timer.start(base, now);
                u.aggregate.background.start(&u.background, now);
            } else {
                u.aggregate.timer.stop(base, now);
                u.aggregate.background.stop(&u.background, now);
            }
            if a.pid >= 0 {
                let p = u.pids.entry(a.pid).or_default();
                if start {
                    if p.0 == 0 {
                        p.1 = now;
                    }
                    p.0 += 1;
                } else if p.0 > 0 {
                    p.0 -= 1;
                    if p.0 == 0 {
                        p.2 += now - p.1;
                        p.1 = 0;
                    }
                }
            }
        }
    }

    fn change(
        &mut self,
        old: Option<&Attribution>,
        new: Option<&Attribution>,
        map: &dyn Fn(i32) -> i32,
        now: i64,
        limit: usize,
    ) {
        let diff = old.zip(new).is_some_and(|(a, b)| {
            a.source.is_some() && b.source.is_some() && a.kind.is_some() && b.kind.is_some()
        });
        let changes = if diff {
            [(true, new, old), (false, old, new)]
        } else {
            [(false, old, None), (true, new, None)]
        };
        for (start, a, other) in changes {
            let Some(a) = a.filter(|a| a.kind.is_some()) else {
                continue;
            };
            for uid in a.entries(other).into_iter().filter(|u| *u >= 0) {
                let entry = self.mapped.entry(uid).or_insert_with(|| (map(uid), 0));
                let mapped = entry.0;
                if start {
                    entry.1 += 1;
                } else {
                    entry.1 = entry.1.saturating_sub(1);
                    if entry.1 == 0 {
                        self.mapped.remove(&uid);
                    }
                }
                self.note(start, mapped, a, now, limit);
            }
        }
    }
}

impl BatteryStats {
    #[cfg(test)]
    pub(in crate::app_host) fn assert_sync_cleanup_for_test(&self) {
        let s = self.stats.lock().unwrap();
        assert!(s.wakes.mapped.is_empty());
        for u in s.wakes.uids.values() {
            assert_eq!(u.aggregate.timer.timer.nesting, 0);
            assert_eq!(u.aggregate.background.timer.nesting, 0);
            assert!(u.names.active_overflow.is_empty());
            assert!(u.pids.values().all(|p| p.0 == 0));
            assert!(
                u.names
                    .timers
                    .values()
                    .flatten()
                    .flatten()
                    .all(|t| t.timer.timer.nesting == 0 && t.background.timer.nesting == 0)
            );
        }
        for u in s.uids.values() {
            assert!(u.sync.active_overflow.is_empty());
            assert!(
                u.sync
                    .timers
                    .values()
                    .all(|t| t.timer.timer.nesting == 0 && t.background.timer.nesting == 0)
            );
        }
    }

    #[cfg(test)]
    pub(in crate::app_host) fn assert_sync_nesting_for_test(&self, uid: i32, name: &str, n: i32) {
        let s = self.stats.lock().unwrap();
        let timer = &s.uids[&uid].sync.timers[name];
        assert_eq!(timer.timer.timer.nesting, n);
        assert_eq!(timer.background.timer.nesting, n);
    }

    pub(in crate::app_host) fn change_wake(
        &self,
        old: Option<&Attribution>,
        new: Option<&Attribution>,
    ) -> Result<(), Exception> {
        let apply = |s: &mut Stats, map: &dyn Fn(i32) -> i32, now| {
            s.wakes.change(old, new, map, now, self.sync_name_limit)
        };
        if new.is_none() {
            // Stops use retained UID mappings even after package/owner teardown.
            let now = elapsed_realtime();
            let mut stats = self.stats.lock().unwrap();
            Ok(apply(&mut stats, &|u| u, now))
        } else {
            self.note(1000, apply)
        }
    }

    #[cfg(test)]
    pub(in crate::app_host) fn wake_nesting(&self, uid: i32, tag: &str) -> i32 {
        self.stats
            .lock()
            .unwrap()
            .wakes
            .uids
            .get(&uid)
            .and_then(|u| u.names.timers.get(tag))
            .and_then(|ts| ts[0].as_ref())
            .map_or(0, |t| t.timer.timer.nesting)
    }

    pub(in crate::app_host) fn wake_screen(&self, on: bool) {
        let mut s = self.stats.lock().unwrap();
        s.wakes.screen_off = !on;
        let battery = s.on_battery_time_base.running;
        s.wakes.bases(battery, elapsed_realtime());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn attribution(uid: i32, name: &str, kind: usize) -> Attribution {
        Attribution {
            uid,
            pid: 71,
            package: "p".into(),
            tag: name.into(),
            history: None,
            source: None,
            kind: Some(kind),
        }
    }
    fn timer<'a>(w: &'a WakeStats, uid: i32, name: &str, kind: usize) -> &'a DualTimer {
        w.uids[&uid].names.timers[name][kind].as_ref().unwrap()
    }
    #[test]
    fn wake_pools_screen_off_background_and_pid_spans_follow_bases() {
        let mut w = WakeStats::default();
        w.bases(true, 0);
        w.process(10001, true, 0);
        let a = attribution(10001, "a", 0);
        let b = attribution(10002, "b", 0);
        w.change(None, Some(&a), &|u| u, 0, 200);
        w.screen_off = true;
        w.bases(true, 1000);
        w.change(None, Some(&b), &|u| u, 2000, 200);
        w.change(None, Some(&a), &|u| u, 2500, 200);
        w.change(Some(&a), None, &|u| u, 3000, 200);
        w.process(10001, false, 3500);
        w.bases(false, 4000);
        w.change(Some(&a), None, &|u| u, 5000, 200);
        w.change(Some(&b), None, &|u| u, 6000, 200);
        let t = timer(&w, 10001, "a", 0);
        assert_eq!(t.timer.timer.total_time_us, 2_000_000); // 1s alone + 2s/2
        assert_eq!(t.timer.total_duration_ms, 3000); // unpooled duration
        assert_eq!(t.background.timer.total_time_us, 2_500_000);
        assert_eq!(
            timer(&w, 10002, "b", 0).timer.timer.total_time_us,
            1_000_000
        );
        assert_eq!(
            w.uids[&10001].aggregate.timer.timer.total_time_us,
            3_000_000
        );
        assert_eq!(w.uids[&10001].pids[&71], (0, 0, 5000)); // elapsed, including plugged/screen-on
        assert!(w.mapped.is_empty());
    }
    #[test]
    fn full_and_draw_pools_overflow_and_mapping_survive_owner_disappearance() {
        let mut w = WakeStats::default();
        w.bases(true, 0);
        let a = attribution(99001, "a", 1);
        let b = attribution(10002, "b", 1);
        let draw = attribution(10003, "draw", 2);
        w.change(
            None,
            Some(&a),
            &|u| if u == 99001 { 10001 } else { u },
            0,
            1,
        );
        w.change(None, Some(&b), &|u| u, 1000, 1);
        w.change(None, Some(&draw), &|u| u, 1000, 1);
        w.change(Some(&a), None, &|u| u, 3000, 1);
        w.change(Some(&b), None, &|u| u, 3000, 1);
        w.change(Some(&draw), None, &|u| u, 3000, 1);
        assert_eq!(
            timer(&w, 10001, "a", 1).timer.timer.total_time_us,
            2_000_000
        );
        assert_eq!(
            timer(&w, 10002, "b", 1).timer.timer.total_time_us,
            1_000_000
        );
        assert_eq!(
            timer(&w, 10003, "draw", 2).timer.timer.total_time_us,
            2_000_000
        );
        let extra = attribution(10001, "extra", 1);
        w.change(None, Some(&extra), &|u| u, 4000, 1);
        w.change(Some(&extra), None, &|u| u, 6000, 1);
        assert_eq!(
            timer(&w, 10001, OVERFLOW_NAME, 1).timer.timer.total_time_us,
            2_000_000
        );
        assert!(w.uids[&10001].names.active_overflow.is_empty());
    }
}
