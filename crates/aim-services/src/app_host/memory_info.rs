//! `getMemoryInfo` for the app host (`ProcessList.getMemoryInfo` and
//! `updateOomLevels` at `android-16.0.0_r1`): the memory the guest sees
//! (the Mac's, as its `/proc/meminfo` and `sysinfo` report it), and the
//! thresholds the original's OOM levels give for the device's memory and
//! display; and `getProcessMemoryInfo`'s checks of which processes a
//! caller may see.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::parcel::{EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::WriteParcelable;

use super::{AppHost, PER_USER_RANGE};

/// `ProcessList`'s OOM adjustments and their minimum free memory, low and
/// high, in KiB.
const FOREGROUND_APP_ADJ: i32 = 0;
const VISIBLE_APP_ADJ: i32 = 100;
const SERVICE_ADJ: i32 = 500;
const HOME_APP_ADJ: i32 = 600;
const CACHED_APP_MIN_ADJ: i32 = 900;
const OOM_ADJ: [i32; 6] = [FOREGROUND_APP_ADJ, VISIBLE_APP_ADJ, 200, 250, CACHED_APP_MIN_ADJ, 950];
const OOM_MIN_FREE_LOW: [i32; 6] = [12288, 18432, 24576, 36864, 43008, 49152];
const OOM_MIN_FREE_HIGH: [i32; 6] = [73728, 92160, 110592, 129024, 147456, 184320];

/// `ActivityManager.MemoryInfo`.
#[derive(Debug, PartialEq)]
pub struct MemoryInfo {
    advertised_mem: i64,
    avail_mem: i64,
    total_mem: i64,
    threshold: i64,
    low_memory: bool,
    hidden_app_threshold: i64,
    secondary_server_threshold: i64,
    visible_app_threshold: i64,
    foreground_app_threshold: i64,
}

impl WriteParcelable for MemoryInfo {
    fn write_to(&self, p: &mut Parcel) {
        for v in [self.advertised_mem, self.avail_mem, self.total_mem, self.threshold] {
            p.write_i64(v);
        }
        p.write_i32(self.low_memory as i32);
        for v in [self.hidden_app_threshold, self.secondary_server_threshold, self.visible_app_threshold, self.foreground_app_threshold] {
            p.write_i64(v);
        }
    }
}

/// One kind of memory's sizes in `Debug.MemoryInfo`, in KiB, in the
/// order of its `OFFSET_*`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Stats {
    pss: i32,
    swappable_pss: i32,
    rss: i32,
    private_dirty: i32,
    shared_dirty: i32,
    private_clean: i32,
    shared_clean: i32,
    swapped_out: i32,
    swapped_out_pss: i32,
}

/// `Debug.MemoryInfo.NUM_OTHER_STATS + NUM_DVK_STATS`.
const NUM_OTHER_KINDS: usize = 17 + 15;

/// `Debug.MemoryInfo`: a process's dalvik, native and other memory, and
/// each other kind's (`otherStats`).
#[derive(Debug, Default, PartialEq)]
pub struct ProcessMemoryInfo {
    dalvik: Stats,
    native: Stats,
    other: Stats,
    has_swapped_out_pss: bool,
    other_stats: [Stats; NUM_OTHER_KINDS],
}

impl Stats {
    fn values(&self) -> [i32; 9] {
        [
            self.pss,
            self.swappable_pss,
            self.rss,
            self.private_dirty,
            self.shared_dirty,
            self.private_clean,
            self.shared_clean,
            self.swapped_out,
            self.swapped_out_pss,
        ]
    }
}

impl WriteParcelable for ProcessMemoryInfo {
    fn write_to(&self, p: &mut Parcel) {
        for v in self.dalvik.values().into_iter().chain(self.native.values()) {
            p.write_i32(v);
        }
        // `hasSwappedOutPss` before the other memory's swapped out PSS.
        let other = self.other.values();
        for v in &other[..8] {
            p.write_i32(*v);
        }
        p.write_i32(self.has_swapped_out_pss as i32);
        p.write_i32(other[8]);
        let stats: Vec<i32> = self.other_stats.iter().flat_map(Stats::values).collect();
        aim_service_aidl::write_int_array(p, Some(&stats));
    }
}

/// Whether `getProcessMemoryInfo` measures a process of `target` (none
/// for a pid of no app's process) for `uid`: its own uid's, else only with
/// `all_uids` (`isGetTasksAllowed`), and another user's only with
/// `all_users` (INTERACT_ACROSS_USERS_FULL).
fn may_see(uid: i32, target: Option<i32>, all_uids: bool, all_users: bool) -> bool {
    let user = |uid: i32| uid / PER_USER_RANGE as i32;
    let target_user = target.map_or(-1, user);
    target == Some(uid) || all_uids && (all_users || target_user == user(uid))
}

/// `updateOomLevels`: the minimum free memory of each OOM level, in KiB,
/// for `total_mb` of memory, a display of `width` by `height` pixels and
/// the `config_lowMemoryKillerMinFreeKbytes*` overrides.
fn oom_min_free(total_mb: i64, width: i32, height: i32, minfree_adj: i32, minfree_abs: i32) -> [i32; 6] {
    let scale_mem = (total_mb - 350) as f32 / (700 - 350) as f32;
    let (min_size, max_size) = (480 * 800, 1280 * 800);
    let scale_disp = ((width * height) as f32 - min_size as f32) / (max_size - min_size) as f32;
    let scale = scale_mem.max(scale_disp).clamp(0.0, 1.0);
    let mut min_free = [0i32; 6];
    for i in 0..OOM_ADJ.len() {
        let low = OOM_MIN_FREE_LOW[i];
        let mut high = OOM_MIN_FREE_HIGH[i];
        // 64-bit: more for the cached processes.
        if i == 4 {
            high = high * 3 / 2;
        } else if i == 5 {
            high = high * 7 / 4;
        }
        min_free[i] = (low as f32 + (high - low) as f32 * scale) as i32;
    }
    let last = OOM_ADJ.len() - 1;
    if minfree_abs >= 0 {
        let top = min_free[last];
        for v in &mut min_free {
            *v = (minfree_abs as f32 * *v as f32 / top as f32) as i32;
        }
    }
    if minfree_adj != 0 {
        let top = min_free[last];
        for v in &mut min_free {
            *v = (*v + (minfree_adj as f32 * *v as f32 / top as f32) as i32).max(0);
        }
    }
    min_free
}

/// `getMemLevel`: the minimum free memory of the first level at or above
/// `adjustment`, in bytes.
fn mem_level(min_free: &[i32; 6], adjustment: i32) -> i64 {
    let i = OOM_ADJ.iter().position(|a| adjustment <= *a).unwrap_or(OOM_ADJ.len() - 1);
    min_free[i] as i64 * 1024
}

/// `FileUtils.parseSize`: a size such as `8GB` or `512MiB`, in bytes.
fn parse_size(s: &str) -> Option<i64> {
    let s = s.trim();
    let (sign, s) = match s.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, s.strip_prefix('+').unwrap_or(s)),
    };
    let digits = s.find(|c: char| !c.is_ascii_digit()).filter(|&i| i > 0)?;
    let value = sign * s[..digits].parse::<i64>().ok()?;
    let unit: i64 = match s[digits..].trim().to_ascii_uppercase().as_str() {
        "B" => 1,
        "K" | "KB" => 1000,
        "M" | "MB" => 1000 * 1000,
        "G" | "GB" => 1000 * 1000 * 1000,
        "KI" | "KIB" => 1 << 10,
        "MI" | "MIB" => 1 << 20,
        "GI" | "GIB" => 1 << 30,
        _ => return None,
    };
    Some(value * unit)
}

/// `FileUtils.roundStorageSize`: up to a power of two of a power of 1000.
fn round_storage_size(size: i64) -> i64 {
    let (mut val, mut pow) = (1i64, 1i64);
    while val * pow < size {
        val <<= 1;
        if val > 512 {
            val = 1;
            pow *= 1000;
        }
    }
    val * pow
}

impl AppHost {
    /// `ProcessList.getMemoryInfo`.
    pub(super) fn memory_info(&self) -> Result<MemoryInfo, Exception> {
        let inputs = self.inputs()?;
        let integer = |name: &str| {
            inputs.packages.framework_integer(name).ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, format!("no {name}")))
        };
        let (minfree_adj, minfree_abs) =
            (integer("config_lowMemoryKillerMinFreeKbytesAdjust")?, integer("config_lowMemoryKillerMinFreeKbytesAbsolute")?);
        let total = aim_host_memory::total() as i64;
        let display = self.display()?;
        let mode = display.0.active();
        let min_free = oom_min_free(total / (1024 * 1024), mode.width, mode.height, minfree_adj, minfree_abs);
        let home = mem_level(&min_free, HOME_APP_ADJ);
        let cached = mem_level(&min_free, CACHED_APP_MIN_ADJ);
        let avail = aim_host_memory::available() as i64;
        // `Process.getAdvertisedMem`: `ro.boot.ddr_size`, else the total
        // rounded as storage sizes are.
        let configured = self.configured.get().and_then(|c| (c.properties)("ro.boot.ddr_size"));
        let advertised = configured.as_deref().and_then(parse_size).filter(|s| *s > 0).unwrap_or_else(|| round_storage_size(total));
        Ok(MemoryInfo {
            advertised_mem: advertised,
            avail_mem: avail,
            total_mem: total,
            threshold: home,
            low_memory: avail < home + (cached - home) / 2,
            hidden_app_threshold: cached,
            secondary_server_threshold: mem_level(&min_free, SERVICE_ADJ),
            visible_app_threshold: mem_level(&min_free, VISIBLE_APP_ADJ),
            foreground_app_threshold: mem_level(&min_free, FOREGROUND_APP_ADJ),
        })
    }

    /// `getProcessMemoryInfo` of `uid` for `pids`: an empty
    /// `Debug.MemoryInfo` for each process it may not see. Measuring one
    /// it may see (`Debug.getMemoryInfo`) takes the guest's
    /// `/proc/<pid>/smaps`, which only a guest process can read (#1543).
    pub(super) fn process_memory_info(&self, uid: i32, pids: Option<Vec<i32>>) -> Result<Vec<Option<ProcessMemoryInfo>>, Exception> {
        if super::is_isolated(uid) {
            return Err(Exception::security("Isolated process not allowed to call getProcessMemoryInfo"));
        }
        let all_users = self.check_permission(Some("android.permission.INTERACT_ACROSS_USERS_FULL"), uid)? == 0;
        let all_uids = self.get_tasks_allowed(uid)?;
        let pids = pids.ok_or_else(|| Exception::new(EX_NULL_POINTER, "pids is null"))?;
        let apps = self.apps.lock().unwrap();
        let target = |pid: i32| apps.values().find(|a| a.pid == pid).map(|a| a.uid as i32);
        if pids.iter().any(|&pid| may_see(uid, target(pid), all_uids, all_users)) {
            return Err(self.unimplemented_named("activity", "getProcessMemoryInfo of a process its caller may see (#1543)"));
        }
        Ok(pids.iter().map(|_| Some(ProcessMemoryInfo::default())).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device with memory to spare and a large display has the high
    /// levels, the cached ones raised for 64 bits.
    #[test]
    fn oom_levels_scale_to_the_device() {
        let levels = oom_min_free(16 * 1024, 1080, 1920, 0, -1);
        assert_eq!(levels, [73728, 92160, 110592, 129024, 221184, 322560]);
        assert_eq!(mem_level(&levels, HOME_APP_ADJ), 221184 * 1024);
        assert_eq!(mem_level(&levels, 1001), 322560 * 1024);
        let small = oom_min_free(300, 320, 480, 0, -1);
        assert_eq!(small[0], 12288);
        let absolute = oom_min_free(16 * 1024, 1080, 1920, 0, 322560 * 2);
        assert_eq!(absolute[0], 73728 * 2);
    }

    /// `Debug.MemoryInfo.writeToParcel`: dalvik's, native's, the other
    /// memory's (with `hasSwappedOutPss` before its swapped out PSS), then
    /// `otherStats`, 32 kinds of 9.
    #[test]
    fn process_memory_info_is_written_as_the_original_writes_it() {
        let stats = |base: i32| Stats {
            pss: base,
            swappable_pss: base + 1,
            rss: base + 2,
            private_dirty: base + 3,
            shared_dirty: base + 4,
            private_clean: base + 5,
            shared_clean: base + 6,
            swapped_out: base + 7,
            swapped_out_pss: base + 8,
        };
        let mut info = ProcessMemoryInfo { dalvik: stats(10), native: stats(20), other: stats(30), has_swapped_out_pss: true, ..Default::default() };
        info.other_stats[1] = stats(40);
        let mut p = Parcel::new();
        info.write_to(&mut p);
        let mut r = p.reader();
        let mut ints = |n: usize| (0..n).map(|_| r.read_i32().unwrap()).collect::<Vec<_>>();
        assert_eq!(ints(18), (10..19).chain(20..29).collect::<Vec<_>>());
        assert_eq!(ints(10), [30, 31, 32, 33, 34, 35, 36, 37, 1, 38]);
        assert_eq!(ints(1), [288]);
        let other = ints(288);
        assert_eq!((&other[..9], &other[9..18]), (&[0; 9][..], &(40..49).collect::<Vec<_>>()[..]));
        assert_eq!(r.remaining(), 0);
    }

    /// A caller sees its own uid's processes; another uid's of its user
    /// with REAL_GET_TASKS, of another user (or a pid of no process) also
    /// with INTERACT_ACROSS_USERS_FULL.
    #[test]
    fn callers_see_processes_as_the_original_lets_them() {
        assert!(may_see(10100, Some(10100), false, false));
        assert!(!may_see(10100, Some(10101), false, true));
        assert!(may_see(10100, Some(10101), true, false));
        assert!(!may_see(10100, Some(1010101), true, false));
        assert!(may_see(10100, Some(1010101), true, true));
        assert!(!may_see(10100, None, true, false));
        assert!(may_see(10100, None, true, true));
    }

    #[test]
    fn sizes_parse_and_round_as_the_original() {
        assert_eq!(parse_size("8GB"), Some(8_000_000_000));
        assert_eq!(parse_size(" 512mib "), Some(512 << 20));
        assert_eq!(parse_size("0KB"), Some(0));
        assert_eq!(parse_size("12"), None);
        assert_eq!(round_storage_size(17_179_869_184), 32_000_000_000);
        assert_eq!(round_storage_size(7_900_000_000), 8_000_000_000);
    }
}
