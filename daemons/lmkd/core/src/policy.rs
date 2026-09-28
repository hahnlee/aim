// SPDX-License-Identifier: Apache-2.0
// The kill order is ported from AOSP lmkd (platform/system/memory/lmkd at
// android-16.0.0_r1: lmkd.cpp, find_and_kill_process, proc_get_heaviest,
// the minfree levels of mp_event_common and the kill counters), Copyright
// (C) 2013 The Android Open Source Project, Apache-2.0.

//! Whom lmkd kills, and when.
//!
//! - **When:** the Mac's pressure level bounds how deep lmkd may go: at
//!   warn it kills cached processes (oom_score_adj >= 900), at critical
//!   down to perceptible ones (>= 200), at normal nothing. Free memory
//!   against ActivityManager's minfree levels (`LMK_TARGET`) can take it
//!   deeper, as the original's minfree mode does: the first level whose
//!   threshold both free and file-backed memory are under names the lowest
//!   oom_score_adj to kill.
//! - **Whom:** one process per kill, the highest oom_score_adj first.
//!   Within one oom_score_adj the least recently prioritized goes first,
//!   except at perceptible and below, where the heaviest does, to free the
//!   most with the fewest perceptible victims.

use std::collections::HashMap;

pub const OOM_SCORE_ADJ_MIN: i32 = -1000;
pub const OOM_SCORE_ADJ_MAX: i32 = 1000;
/// ActivityManager's `PERCEPTIBLE_APP_ADJ`.
pub const PERCEPTIBLE_APP_ADJ: i32 = 200;
/// ActivityManager's `CACHED_APP_MIN_ADJ`.
pub const CACHED_APP_MIN_ADJ: i32 = 900;

/// The host's memory pressure level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Normal,
    Warn,
    Critical,
}

/// The host's memory, in guest pages.
#[derive(Clone, Copy, Debug)]
pub struct Memory {
    pub level: Level,
    pub free: u64,
    pub file: u64,
}

/// One `LMK_TARGET` pair: below `minfree` pages, kill down to `adj`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub minfree: i32,
    pub adj: i32,
}

/// The lowest oom_score_adj to kill at, or None for no kill.
pub fn min_score_adj(mem: &Memory, targets: &[Target]) -> Option<i32> {
    let bound = match mem.level {
        Level::Normal => return None,
        Level::Warn => CACHED_APP_MIN_ADJ,
        Level::Critical => PERCEPTIBLE_APP_ADJ,
    };
    let by_minfree = targets
        .iter()
        .find(|t| mem.free < t.minfree as u64 && mem.file < t.minfree as u64)
        .map(|t| t.adj);
    Some(by_minfree.map_or(bound, |adj| adj.min(bound)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Proc {
    pub pid: i32,
    pub uid: u32,
    pub adj: i32,
    /// When it was last prioritized: larger is more recent.
    seq: u64,
}

/// The processes ActivityManager registered, and the kills so far.
#[derive(Default)]
pub struct Procs {
    procs: HashMap<i32, Proc>,
    seq: u64,
    kills: HashMap<i32, u32>,
}

impl Procs {
    /// Register `pid`, or give it a new oom_score_adj.
    pub fn set(&mut self, pid: i32, uid: u32, adj: i32) {
        self.seq += 1;
        let seq = self.seq;
        self.procs.insert(pid, Proc { pid, uid, adj, seq });
    }

    pub fn remove(&mut self, pid: i32) -> Option<Proc> {
        self.procs.remove(&pid)
    }

    pub fn purge(&mut self) {
        self.procs.clear();
    }

    pub fn len(&self) -> usize {
        self.procs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.procs.is_empty()
    }

    /// The next process to kill at or above `min_adj`, given each one's
    /// resident size in kB (None when unknown).
    pub fn victim(&self, min_adj: i32, rss_kb: impl Fn(i32) -> Option<i64>) -> Option<Proc> {
        let adj = self
            .procs
            .values()
            .filter(|p| p.adj >= min_adj)
            .map(|p| p.adj)
            .max()?;
        let at = self.procs.values().filter(|p| p.adj == adj);
        if adj <= PERCEPTIBLE_APP_ADJ {
            // Ties go to the least recently prioritized.
            at.max_by_key(|p| (rss_kb(p.pid).unwrap_or(0), std::cmp::Reverse(p.seq)))
                .copied()
        } else {
            at.min_by_key(|p| p.seq).copied()
        }
    }

    /// Count a kill at `adj`.
    pub fn killed(&mut self, adj: i32) {
        *self.kills.entry(adj).or_default() += 1;
    }

    /// `LMK_GETKILLCNT`: kills with oom_score_adj in `min..=max`; all
    /// kills when `min` is above [`OOM_SCORE_ADJ_MAX`].
    pub fn kill_count(&self, min: i32, max: i32) -> u32 {
        if min > max {
            return 0;
        }
        let all = min > OOM_SCORE_ADJ_MAX;
        self.kills
            .iter()
            .filter(|(adj, _)| all || (min..=max).contains(*adj))
            .map(|(_, n)| n)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ActivityManager's levels for a 64-bit device with a large screen
    /// (ProcessList.updateOomLevels), in 16 KiB pages.
    const TARGETS: [Target; 6] = [
        Target {
            minfree: 4608,
            adj: 0,
        },
        Target {
            minfree: 5760,
            adj: 100,
        },
        Target {
            minfree: 6912,
            adj: 200,
        },
        Target {
            minfree: 8064,
            adj: 250,
        },
        Target {
            minfree: 13824,
            adj: 900,
        },
        Target {
            minfree: 20160,
            adj: 950,
        },
    ];

    fn mem(level: Level, free: u64, file: u64) -> Memory {
        Memory { level, free, file }
    }

    #[test]
    fn the_level_bounds_the_depth() {
        let plenty = 1 << 20;
        assert_eq!(min_score_adj(&mem(Level::Normal, 0, 0), &TARGETS), None);
        assert_eq!(
            min_score_adj(&mem(Level::Warn, plenty, plenty), &TARGETS),
            Some(900)
        );
        assert_eq!(
            min_score_adj(&mem(Level::Critical, plenty, plenty), &TARGETS),
            Some(200)
        );
        assert_eq!(
            min_score_adj(&mem(Level::Warn, plenty, plenty), &[]),
            Some(900)
        );
    }

    #[test]
    fn minfree_levels_go_deeper() {
        // Under the 250 level (8064 pages) in both free and file memory.
        assert_eq!(
            min_score_adj(&mem(Level::Warn, 8000, 7000), &TARGETS),
            Some(250)
        );
        // Only free memory is low: the page cache can still be dropped.
        assert_eq!(
            min_score_adj(&mem(Level::Warn, 8000, 1 << 20), &TARGETS),
            Some(900)
        );
        // Under the lowest level: down to foreground.
        assert_eq!(
            min_score_adj(&mem(Level::Critical, 100, 100), &TARGETS),
            Some(0)
        );
        // Never shallower than the level allows.
        assert_eq!(
            min_score_adj(&mem(Level::Critical, 15000, 15000), &TARGETS),
            Some(200)
        );
    }

    #[test]
    fn kills_the_highest_adj_then_the_least_recent() {
        let mut p = Procs::default();
        p.set(10, 10010, 900);
        p.set(11, 10011, 950);
        p.set(12, 10012, 900);
        p.set(13, 10013, 0);
        p.set(14, 10014, 999);
        let no_rss = |_| None;
        let order: Vec<i32> = std::iter::from_fn(|| {
            let v = p.victim(900, no_rss)?;
            p.remove(v.pid);
            Some(v.pid)
        })
        .collect();
        assert_eq!(order, [14, 11, 10, 12]);
        assert_eq!(p.victim(900, no_rss), None, "the foreground process stays");
        assert_eq!(p.victim(0, no_rss).map(|v| v.pid), Some(13));
    }

    #[test]
    fn a_new_priority_makes_a_process_recent() {
        let mut p = Procs::default();
        p.set(10, 1, 900);
        p.set(11, 1, 900);
        p.set(10, 1, 900);
        assert_eq!(p.victim(900, |_| None).map(|v| v.pid), Some(11));
        // Moving to a lower adj takes it out of the cached range.
        p.set(11, 1, 100);
        assert_eq!(p.victim(900, |_| None).map(|v| v.pid), Some(10));
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn perceptible_victims_are_the_heaviest() {
        let mut p = Procs::default();
        p.set(20, 1, 200);
        p.set(21, 1, 200);
        p.set(22, 1, 200);
        let rss = |pid| Some(if pid == 21 { 500_000 } else { 1000 });
        assert_eq!(p.victim(200, rss).map(|v| v.pid), Some(21));
        // Equal sizes: the least recently prioritized.
        assert_eq!(p.victim(200, |_| Some(1)).map(|v| v.pid), Some(20));
    }

    #[test]
    fn kill_counts_by_range() {
        let mut p = Procs::default();
        for adj in [900, 950, 950, 200] {
            p.killed(adj);
        }
        assert_eq!(p.kill_count(900, 1000), 3);
        assert_eq!(p.kill_count(0, 899), 1);
        assert_eq!(p.kill_count(OOM_SCORE_ADJ_MAX + 1, 0), 0, "min above max");
        assert_eq!(
            p.kill_count(OOM_SCORE_ADJ_MAX + 1, OOM_SCORE_ADJ_MAX + 1),
            4
        );
    }
}
