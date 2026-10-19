//! `getMemoryInfo` for the app host (`ProcessList.getMemoryInfo` and
//! `updateOomLevels` at `android-16.0.0_r1`): the memory the guest sees
//! (the Mac's, as its `/proc/meminfo` and `sysinfo` report it), and the
//! thresholds the original's OOM levels give for the device's memory and
//! display; and `getProcessMemoryInfo` (`ActivityManagerService` with
//! `Debug.getMemoryInfo`, `android_os_Debug.cpp`, and libmeminfo's
//! `ExtractAndroidHeapStats`): each process's `/proc/<pid>/smaps`, which
//! the agent reads, sorted into the heaps `Debug.MemoryInfo` reports.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::parcel::{EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::WriteParcelable;

use super::{AppHost, PER_USER_RANGE, clock_ms};

/// `ProcessList`'s OOM adjustments and their minimum free memory, low and
/// high, in KiB.
const FOREGROUND_APP_ADJ: i32 = 0;
const VISIBLE_APP_ADJ: i32 = 100;
const SERVICE_ADJ: i32 = 500;
const HOME_APP_ADJ: i32 = 600;
const CACHED_APP_MIN_ADJ: i32 = 900;
const OOM_ADJ: [i32; 6] = [
    FOREGROUND_APP_ADJ,
    VISIBLE_APP_ADJ,
    200,
    250,
    CACHED_APP_MIN_ADJ,
    950,
];
const OOM_MIN_FREE_LOW: [i32; 6] = [12288, 18432, 24576, 36864, 43008, 49152];
const OOM_MIN_FREE_HIGH: [i32; 6] = [73728, 92160, 110592, 129024, 147456, 184320];
/// The agent's `readSmaps(int pid)`.
const READ_SMAPS: u32 = 8;
/// `ActivityManagerConstants.DEFAULT_MEMORY_INFO_THROTTLE_TIME`.
const DEFAULT_MEMORY_INFO_THROTTLE_TIME: i64 = 5 * 60 * 1000;

/// libmeminfo's heaps (`androidprocheaps.h`): the core ones, the others
/// `Debug.MemoryInfo` adds to the unknown, then the dalvik, dex and art
/// sections.
const HEAP_UNKNOWN: usize = 0;
const HEAP_DALVIK: usize = 1;
const HEAP_NATIVE: usize = 2;
const HEAP_DALVIK_OTHER: usize = 3;
const HEAP_STACK: usize = 4;
const HEAP_CURSOR: usize = 5;
const HEAP_ASHMEM: usize = 6;
const HEAP_GL_DEV: usize = 7;
const HEAP_UNKNOWN_DEV: usize = 8;
const HEAP_SO: usize = 9;
const HEAP_JAR: usize = 10;
const HEAP_APK: usize = 11;
const HEAP_TTF: usize = 12;
const HEAP_DEX: usize = 13;
const HEAP_OAT: usize = 14;
const HEAP_ART: usize = 15;
const HEAP_UNKNOWN_MAP: usize = 16;
const HEAP_OTHER_MEMTRACK: usize = 19;
const HEAP_DALVIK_NORMAL: usize = 20;
const HEAP_DALVIK_LARGE: usize = 21;
const HEAP_DALVIK_ZYGOTE: usize = 22;
const HEAP_DALVIK_NON_MOVING: usize = 23;
const HEAP_DALVIK_OTHER_LINEARALLOC: usize = 24;
const HEAP_DALVIK_OTHER_ACCOUNTING: usize = 25;
const HEAP_DALVIK_OTHER_ZYGOTE_CODE_CACHE: usize = 26;
const HEAP_DALVIK_OTHER_APP_CODE_CACHE: usize = 27;
const HEAP_DALVIK_OTHER_COMPILER_METADATA: usize = 28;
const HEAP_DALVIK_OTHER_INDIRECT_REFERENCE_TABLE: usize = 29;
const HEAP_DEX_BOOT_VDEX: usize = 30;
const HEAP_DEX_APP_DEX: usize = 31;
const HEAP_DEX_APP_VDEX: usize = 32;
const HEAP_ART_APP: usize = 33;
const HEAP_ART_BOOT: usize = 34;
const NUM_HEAP: usize = 35;
const NUM_EXCLUSIVE_HEAP: usize = HEAP_OTHER_MEMTRACK + 1;
const NUM_CORE_HEAP: usize = HEAP_NATIVE + 1;

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
        for v in [
            self.advertised_mem,
            self.avail_mem,
            self.total_mem,
            self.threshold,
        ] {
            p.write_i64(v);
        }
        p.write_i32(self.low_memory as i32);
        for v in [
            self.hidden_app_threshold,
            self.secondary_server_threshold,
            self.visible_app_threshold,
            self.foreground_app_threshold,
        ] {
            p.write_i64(v);
        }
    }
}

/// One kind of memory's sizes in `Debug.MemoryInfo` (libmeminfo's
/// `AndroidHeapStats`), in KiB, in the order of its `OFFSET_*`.
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
const NUM_OTHER_KINDS: usize = NUM_HEAP - NUM_CORE_HEAP;

/// `Debug.MemoryInfo`: a process's dalvik, native and other memory, and
/// each other kind's (`otherStats`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcessMemoryInfo {
    dalvik: Stats,
    native: Stats,
    other: Stats,
    has_swapped_out_pss: bool,
    other_stats: [Stats; NUM_OTHER_KINDS],
}

impl Stats {
    /// Adds `values`, in `values()`'s order, as C adds 64-bit counts to
    /// an `int`.
    fn add(&mut self, values: [u64; 9]) {
        let fields = [
            &mut self.pss,
            &mut self.swappable_pss,
            &mut self.rss,
            &mut self.private_dirty,
            &mut self.shared_dirty,
            &mut self.private_clean,
            &mut self.shared_clean,
            &mut self.swapped_out,
            &mut self.swapped_out_pss,
        ];
        for (field, v) in fields.into_iter().zip(values) {
            *field = field.wrapping_add(v as i32);
        }
    }

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

/// A mapping's usage as libmeminfo's `parse_smaps_field` reads it from
/// smaps, in KiB.
#[derive(Default)]
struct Usage {
    rss: u64,
    pss: u64,
    uss: u64,
    swap: u64,
    swap_pss: u64,
    private_clean: u64,
    private_dirty: u64,
    shared_clean: u64,
    shared_dirty: u64,
}

impl Usage {
    /// `parse_smaps_field`: whether `line` (with its newline, as `getline`
    /// reads it) is a field, its first word ending with a colon and
    /// followed by a space, with the counts it keeps.
    fn parse(&mut self, line: &str) -> bool {
        let key = line
            .split(|c: char| c.is_ascii_whitespace())
            .next()
            .unwrap_or_default();
        if key.is_empty() || !key.ends_with(':') || key.len() == line.len() {
            return false;
        }
        let value = strtoull(line[key.len()..].trim_start());
        match key {
            "Pss:" => self.pss = value,
            "Private_Clean:" => {
                self.private_clean = value;
                self.uss += value;
            }
            "Private_Dirty:" => {
                self.private_dirty = value;
                self.uss += value;
            }
            "Shared_Clean:" => self.shared_clean = value,
            "Shared_Dirty:" => self.shared_dirty = value,
            "Swap:" => self.swap = value,
            "SwapPss:" => self.swap_pss = value,
            "Rss:" => self.rss = value,
            _ => {}
        }
        true
    }

    /// The counts `AndroidHeapStats` adds, with `swappable_pss`.
    fn values(&self, swappable_pss: u64) -> [u64; 9] {
        [
            self.pss,
            swappable_pss,
            self.rss,
            self.private_dirty,
            self.shared_dirty,
            self.private_clean,
            self.shared_clean,
            self.swap,
            self.swap_pss,
        ]
    }
}

/// `strtoull(s, nullptr, 10)`: the leading digits, 0 without any, the
/// largest value when they overflow.
fn strtoull(s: &str) -> u64 {
    let digits = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if digits == 0 {
        return 0;
    }
    s[..digits].parse().unwrap_or(u64::MAX)
}

/// A mapping's line, without its newline (`procinfo::ParseMapsFileLine`):
/// its start, end and name; none if the line is not one.
fn map_line(line: &str) -> Option<(u64, u64, &str)> {
    let (start, p) = number(line, 16)?;
    let (end, p) = number(p.strip_prefix('-')?, 16)?;
    let p = pass_space(p)?;
    let perms = p.as_bytes();
    let valid = |at: usize, set: u8| perms.get(at).is_some_and(|&c| c == set || c == b'-');
    if !(valid(0, b'r')
        && valid(1, b'w')
        && valid(2, b'x')
        && matches!(perms.get(3), Some(b'p' | b's')))
    {
        return None;
    }
    // The offset, the device's major:minor and the inode.
    let (_, p) = number(pass_space(&p[4..])?, 16)?;
    let (_, p) = number(pass_space(p)?, 16)?;
    let (_, p) = number(p.strip_prefix(':')?, 16)?;
    let (_, p) = number(pass_space(p)?, 10)?;
    let name = if p.is_empty() { p } else { pass_space(p)? };
    Some((start, end, name))
}

/// The leading digits of `radix` of `p` (one at least) as `strtoull`
/// reads them, and what follows.
fn number(p: &str, radix: u32) -> Option<(u64, &str)> {
    let end = p.find(|c: char| !c.is_digit(radix)).unwrap_or(p.len());
    (end > 0).then(|| {
        (
            u64::from_str_radix(&p[..end], radix).unwrap_or(u64::MAX),
            &p[end..],
        )
    })
}

/// `PassSpace`: past one space or more.
fn pass_space(p: &str) -> Option<&str> {
    p.starts_with(' ').then(|| p.trim_start_matches(' '))
}

/// `ExtractAndroidHeapStatsFromFile` of `smaps`: each heap's sizes and
/// whether a mapping had swapped out PSS; none when a mapping's line does
/// not parse (`ForEachVmaFromFile`).
fn heap_stats(smaps: &str) -> Option<([Stats; NUM_HEAP], bool)> {
    let mut stats = [Stats::default(); NUM_HEAP];
    let mut found_swap_pss = false;
    let (mut prev_end, mut prev_heap) = (0, HEAP_UNKNOWN);
    let mut scan = |(start, end, name): (u64, u64, &str), usage: &Usage| {
        let name = name.strip_suffix(" (deleted)").unwrap_or(name);
        let (which, sub, swappable) = classify(name, start == prev_end && prev_heap == HEAP_SO);
        prev_end = end;
        prev_heap = which;
        if usage.swap_pss > 0 {
            found_swap_pss = true;
        }
        let mut swappable_pss = 0;
        if swappable && usage.pss > 0 {
            let mut sharing_proportion = 0.0f32;
            if usage.shared_clean > 0 || usage.shared_dirty > 0 {
                // An integer division, as the original's.
                sharing_proportion = (usage.pss.wrapping_sub(usage.uss)
                    / (usage.shared_clean + usage.shared_dirty))
                    as f32;
            }
            swappable_pss = (sharing_proportion * usage.shared_clean as f32
                + usage.private_clean as f32) as u64;
        }
        stats[which].add(usage.values(swappable_pss));
        if matches!(which, HEAP_DALVIK | HEAP_DALVIK_OTHER | HEAP_DEX | HEAP_ART) {
            stats[sub].add(usage.values(swappable_pss));
        }
    };
    let mut vma: Option<((u64, u64, &str), Usage)> = None;
    for line in smaps.split_inclusive('\n') {
        if let Some((map, usage)) = &mut vma {
            if usage.parse(line) {
                continue;
            }
            scan(*map, usage);
        }
        vma = Some((
            map_line(line.strip_suffix('\n').unwrap_or(line))?,
            Usage::default(),
        ));
    }
    if let Some((map, usage)) = &vma {
        scan(*map, usage);
    }
    Some((stats, found_swap_pss))
}

/// The heap of a mapping named `name`, its section and whether it is
/// swappable; `after_so` when it starts where a library's mapping ended
/// (the library's bss).
fn classify(name: &str, after_so: bool) -> (usize, usize, bool) {
    let boot = name.contains("@boot") || name.contains("/boot") || name.contains("/apex");
    if name.starts_with("[heap]")
        || name.starts_with("[anon:libc_malloc]")
        || name.starts_with("[anon:scudo:")
        || name.starts_with("[anon:GWP-ASan")
    {
        (HEAP_NATIVE, HEAP_UNKNOWN, false)
    } else if name.starts_with("[stack") || name.starts_with("[anon:stack_and_tls:") {
        (HEAP_STACK, HEAP_UNKNOWN, false)
    } else if name.ends_with(".so") {
        (HEAP_SO, HEAP_UNKNOWN, true)
    } else if name.ends_with(".jar") {
        (HEAP_JAR, HEAP_UNKNOWN, true)
    } else if name.ends_with(".apk") {
        (HEAP_APK, HEAP_UNKNOWN, true)
    } else if name.ends_with(".ttf") {
        (HEAP_TTF, HEAP_UNKNOWN, true)
    } else if name.ends_with(".odex") || (name.len() > 4 && name.contains(".dex")) {
        (HEAP_DEX, HEAP_DEX_APP_DEX, true)
    } else if name.ends_with(".vdex") {
        (
            HEAP_DEX,
            if boot {
                HEAP_DEX_BOOT_VDEX
            } else {
                HEAP_DEX_APP_VDEX
            },
            true,
        )
    } else if name.ends_with(".oat") {
        (HEAP_OAT, HEAP_UNKNOWN, true)
    } else if name.ends_with(".art") || name.ends_with(".art]") {
        (
            HEAP_ART,
            if boot { HEAP_ART_BOOT } else { HEAP_ART_APP },
            true,
        )
    } else if name.contains("kgsl-3d0") {
        (HEAP_GL_DEV, HEAP_UNKNOWN, false)
    } else if name.starts_with("/dev/") {
        if name.starts_with("/dev/ashmem/CursorWindow") {
            (HEAP_CURSOR, HEAP_UNKNOWN, false)
        } else if name.starts_with("/dev/ashmem/jit-zygote-cache") {
            (
                HEAP_DALVIK_OTHER,
                HEAP_DALVIK_OTHER_ZYGOTE_CODE_CACHE,
                false,
            )
        } else if name.starts_with("/dev/ashmem") {
            (HEAP_ASHMEM, HEAP_UNKNOWN, false)
        } else {
            (HEAP_UNKNOWN_DEV, HEAP_UNKNOWN, false)
        }
    } else if name.starts_with("/memfd:jit-cache") {
        (HEAP_DALVIK_OTHER, HEAP_DALVIK_OTHER_APP_CODE_CACHE, false)
    } else if name.starts_with("/memfd:jit-zygote-cache") {
        (
            HEAP_DALVIK_OTHER,
            HEAP_DALVIK_OTHER_ZYGOTE_CODE_CACHE,
            false,
        )
    } else if let Some(dalvik) = name.strip_prefix("[anon:dalvik-") {
        let heap = |sub| (HEAP_DALVIK, sub, false);
        let other = |sub| (HEAP_DALVIK_OTHER, sub, false);
        if dalvik.starts_with("LinearAlloc") {
            other(HEAP_DALVIK_OTHER_LINEARALLOC)
        } else if dalvik.starts_with("alloc space") || dalvik.starts_with("main space") {
            // The regular Dalvik heap.
            heap(HEAP_DALVIK_NORMAL)
        } else if dalvik.starts_with("large object space")
            || dalvik.starts_with("free list large object space")
        {
            heap(HEAP_DALVIK_LARGE)
        } else if dalvik.starts_with("non moving space") {
            heap(HEAP_DALVIK_NON_MOVING)
        } else if dalvik.starts_with("zygote space") {
            heap(HEAP_DALVIK_ZYGOTE)
        } else if dalvik.starts_with("indirect ref") {
            other(HEAP_DALVIK_OTHER_INDIRECT_REFERENCE_TABLE)
        } else if dalvik.starts_with("jit-code-cache") || dalvik.starts_with("data-code-cache") {
            other(HEAP_DALVIK_OTHER_APP_CODE_CACHE)
        } else if dalvik.starts_with("CompilerMetadata") {
            other(HEAP_DALVIK_OTHER_COMPILER_METADATA)
        } else {
            other(HEAP_DALVIK_OTHER_ACCOUNTING)
        }
    } else if name.starts_with("[anon:") {
        (HEAP_UNKNOWN, HEAP_UNKNOWN, false)
    } else if !name.is_empty() {
        (HEAP_UNKNOWN_MAP, HEAP_UNKNOWN, false)
    } else if after_so {
        // A shared library's bss.
        (HEAP_SO, HEAP_UNKNOWN, false)
    } else {
        (HEAP_UNKNOWN, HEAP_UNKNOWN, false)
    }
}

/// `android_os_Debug_getDirtyPagesPid` of a process's `smaps`: its heaps,
/// the exclusive ones added to the unknown; none when it does not parse.
/// The device has no memtrack HAL, so `read_memtrack_memory` fails and
/// the graphics, GL and other memtrack memory stay 0, as the original
/// leaves them.
fn memory_info_of(smaps: &str) -> Option<ProcessMemoryInfo> {
    let (mut stats, found_swap_pss) = heap_stats(smaps)?;
    for i in NUM_CORE_HEAP..NUM_EXCLUSIVE_HEAP {
        let values = stats[i].values().map(|v| v as u64);
        stats[HEAP_UNKNOWN].add(values);
    }
    let mut other_stats = [Stats::default(); NUM_OTHER_KINDS];
    other_stats.copy_from_slice(&stats[NUM_CORE_HEAP..]);
    Some(ProcessMemoryInfo {
        dalvik: stats[HEAP_DALVIK],
        native: stats[HEAP_NATIVE],
        other: stats[HEAP_UNKNOWN],
        has_swapped_out_pss: found_swap_pss,
        other_stats,
    })
}

/// `updateOomLevels`: the minimum free memory of each OOM level, in KiB,
/// for `total_mb` of memory, a display of `width` by `height` pixels and
/// the `config_lowMemoryKillerMinFreeKbytes*` overrides.
fn oom_min_free(
    total_mb: i64,
    width: i32,
    height: i32,
    minfree_adj: i32,
    minfree_abs: i32,
) -> [i32; 6] {
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
    let i = OOM_ADJ
        .iter()
        .position(|a| adjustment <= *a)
        .unwrap_or(OOM_ADJ.len() - 1);
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
            inputs
                .packages
                .framework_integer(name)
                .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, format!("no {name}")))
        };
        let (minfree_adj, minfree_abs) = (
            integer("config_lowMemoryKillerMinFreeKbytesAdjust")?,
            integer("config_lowMemoryKillerMinFreeKbytesAbsolute")?,
        );
        let total = aim_host_memory::total() as i64;
        let display = self.display()?;
        let mode = display.0.active();
        let min_free = oom_min_free(
            total / (1024 * 1024),
            mode.width,
            mode.height,
            minfree_adj,
            minfree_abs,
        );
        let home = mem_level(&min_free, HOME_APP_ADJ);
        let cached = mem_level(&min_free, CACHED_APP_MIN_ADJ);
        let avail = aim_host_memory::available() as i64;
        // `Process.getAdvertisedMem`: `ro.boot.ddr_size`, else the total
        // rounded as storage sizes are.
        let configured = self
            .configured
            .get()
            .and_then(|c| (c.properties)("ro.boot.ddr_size"));
        let advertised = configured
            .as_deref()
            .and_then(parse_size)
            .filter(|s| *s > 0)
            .unwrap_or_else(|| round_storage_size(total));
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

    /// `getProcessMemoryInfo` of `uid` for `pids`: each process's
    /// `Debug.MemoryInfo`, empty for one it may not see, an app process's
    /// last one again within `MEMORY_INFO_THROTTLE_TIME` (no caller is
    /// instrumented from the shell: the app host runs no instrumentation).
    pub(super) fn process_memory_info(
        &self,
        uid: i32,
        pids: Option<Vec<i32>>,
    ) -> Result<Vec<Option<ProcessMemoryInfo>>, Exception> {
        if super::is_isolated(uid) {
            return Err(Exception::security(
                "Isolated process not allowed to call getProcessMemoryInfo",
            ));
        }
        let pids = pids.ok_or_else(|| Exception::new(EX_NULL_POINTER, "pids is null"))?;
        let last_now = clock_ms(libc::CLOCK_UPTIME_RAW) - self.memory_info_throttle_time();
        let all_users =
            self.check_permission(Some("android.permission.INTERACT_ACROSS_USERS_FULL"), uid)? == 0;
        let all_uids = self.get_tasks_allowed(uid)?;
        let mut infos = vec![Some(ProcessMemoryInfo::default()); pids.len()];
        for (i, &pid) in pids.iter().enumerate().rev() {
            let (target, last) = match self.apps.lock().unwrap().values().find(|a| a.pid == pid) {
                Some(a) => (Some(a.uid as i32), a.last_mem_info.clone()),
                None => (None, None),
            };
            if !may_see(uid, target, all_uids, all_users) {
                continue;
            }
            if let Some((_, info)) = last.filter(|(at, _)| *at >= last_now) {
                infos[i] = Some(info);
                continue;
            }
            // `Debug.getMemoryInfo`: empty when the process's smaps
            // cannot be read.
            let info = self
                .read_smaps(pid)?
                .as_deref()
                .and_then(memory_info_of)
                .unwrap_or_default();
            if target.is_some() {
                let now = clock_ms(libc::CLOCK_UPTIME_RAW);
                if let Some(a) = self
                    .apps
                    .lock()
                    .unwrap()
                    .values_mut()
                    .find(|a| a.pid == pid)
                {
                    a.last_mem_info = Some((now, info.clone()));
                }
            }
            infos[i] = Some(info);
        }
        Ok(infos)
    }

    /// `ActivityManagerConstants.MEMORY_INFO_THROTTLE_TIME`: the
    /// `activity_manager_constants` setting's (a `KeyValueListParser`
    /// list), else its default.
    fn memory_info_throttle_time(&self) -> i64 {
        let setting = self.inputs().ok().and_then(|i| {
            i.settings
                .get(super::settings::Table::Global, "activity_manager_constants")
        });
        setting
            .as_deref()
            .and_then(|s| {
                s.split(',')
                    .filter_map(|kv| kv.split_once('='))
                    .find(|(k, _)| k.trim() == "memory_info_throttle_time")
            })
            .and_then(|(_, v)| v.trim().parse().ok())
            .unwrap_or(DEFAULT_MEMORY_INFO_THROTTLE_TIME)
    }

    /// A guest process's `/proc/<pid>/smaps`, which the agent reads as
    /// system_server would; none when it cannot be opened.
    fn read_smaps(&self, pid: i32) -> Result<Option<String>, Exception> {
        let reply = self.call_agent(READ_SMAPS, "readSmaps", |p| p.write_i32(pid))?;
        let mut r = reply.reader();
        let smaps = r.read_exception().ok().and_then(|_| r.read_string16().ok());
        smaps.ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "readSmaps reply"))
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
        let mut info = ProcessMemoryInfo {
            dalvik: stats(10),
            native: stats(20),
            other: stats(30),
            has_swapped_out_pss: true,
            ..Default::default()
        };
        info.other_stats[1] = stats(40);
        let mut p = Parcel::new();
        info.write_to(&mut p);
        let mut r = p.reader();
        let mut ints = |n: usize| (0..n).map(|_| r.read_i32().unwrap()).collect::<Vec<_>>();
        assert_eq!(ints(18), (10..19).chain(20..29).collect::<Vec<_>>());
        assert_eq!(ints(10), [30, 31, 32, 33, 34, 35, 36, 37, 1, 38]);
        assert_eq!(ints(1), [288]);
        let other = ints(288);
        assert_eq!(
            (&other[..9], &other[9..18]),
            (&[0; 9][..], &(40..49).collect::<Vec<_>>()[..])
        );
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

    /// A mapping of smaps: its line and the fields libmeminfo reads.
    fn vma(
        range: &str,
        name: &str,
        rss: u64,
        pss: u64,
        shared_clean: u64,
        private_clean: u64,
        private_dirty: u64,
        swap_pss: u64,
    ) -> String {
        format!(
            "{range} r--p 00000000 fe:00 42{}{name}\nSize:                 64 kB\nKernelPageSize:        4 kB\nRss:                {rss:>4} kB\n\
             Pss:                {pss:>4} kB\nPss_Dirty:             0 kB\nShared_Clean:       {shared_clean:>4} kB\nShared_Dirty:          0 kB\n\
             Private_Clean:      {private_clean:>4} kB\nPrivate_Dirty:      {private_dirty:>4} kB\nSwap:                  0 kB\n\
             SwapPss:            {swap_pss:>4} kB\nVmFlags: rd mr mw me\n",
            if name.is_empty() { "" } else { "        " }
        )
    }

    /// `ExtractAndroidHeapStats`: mappings sorted by name into heaps and
    /// their sections, a library's bss after it, swappable PSS of shared
    /// clean pages by the integer sharing proportion, and swapped out PSS
    /// noted.
    #[test]
    fn heaps_are_sorted_by_mapping_name() {
        let smaps = [
            vma(
                "1000-2000",
                "[anon:dalvik-main space (region space)]",
                40,
                40,
                0,
                0,
                40,
                0,
            ),
            vma("2000-3000", "[anon:dalvik-LinearAlloc]", 8, 8, 0, 0, 8, 0),
            vma("3000-4000", "[anon:scudo:primary]", 20, 20, 0, 0, 20, 3),
            vma("4000-5000", "/system/lib64/libc.so", 30, 12, 24, 6, 0, 0),
            vma("5000-6000", "", 4, 4, 0, 0, 4, 0),
            vma(
                "6000-7000",
                "/system/framework/boot-framework.vdex (deleted)",
                10,
                10,
                0,
                10,
                0,
                0,
            ),
            vma(
                "7000-8000",
                "/dev/ashmem/CursorWindow: x (deleted)",
                2,
                2,
                0,
                0,
                2,
                0,
            ),
            vma("9000-a000", "", 1, 1, 0, 0, 1, 0),
        ]
        .concat();
        let (stats, found_swap_pss) = heap_stats(&smaps).unwrap();
        assert!(found_swap_pss);
        assert_eq!(
            (
                stats[HEAP_DALVIK].pss,
                stats[HEAP_DALVIK_NORMAL].private_dirty
            ),
            (40, 40)
        );
        assert_eq!(
            (
                stats[HEAP_DALVIK_OTHER].pss,
                stats[HEAP_DALVIK_OTHER_LINEARALLOC].pss
            ),
            (8, 8)
        );
        assert_eq!(
            (stats[HEAP_NATIVE].pss, stats[HEAP_NATIVE].swapped_out_pss),
            (20, 3)
        );
        // libc: (12 - 6) / 24 is 0 as integers, so only its private clean
        // pages are swappable; its bss follows it.
        assert_eq!(
            (
                stats[HEAP_SO].pss,
                stats[HEAP_SO].swappable_pss,
                stats[HEAP_SO].rss
            ),
            (16, 6, 34)
        );
        assert_eq!(
            (stats[HEAP_DEX].pss, stats[HEAP_DEX_BOOT_VDEX].swappable_pss),
            (10, 10)
        );
        assert_eq!(stats[HEAP_CURSOR].pss, 2);
        // Not after a library: unknown.
        assert_eq!(stats[HEAP_UNKNOWN].pss, 1);
    }

    /// `getDirtyPagesPid`: the exclusive heaps go to the other memory too,
    /// the sections only to `otherStats`; a line that is neither a field
    /// nor a mapping fails the whole read.
    #[test]
    fn memory_info_adds_the_exclusive_heaps_to_the_other_memory() {
        let smaps = [
            vma("1000-2000", "/system/lib64/libc.so", 30, 12, 24, 6, 0, 0),
            vma("2000-3000", "", 1, 1, 0, 0, 1, 0),
        ]
        .concat();
        let info = memory_info_of(&smaps).unwrap();
        assert_eq!((info.other.pss, info.other.rss), (13, 31));
        assert_eq!(info.other_stats[HEAP_SO - NUM_CORE_HEAP].pss, 13);
        assert!(!info.has_swapped_out_pss);
        assert_eq!(memory_info_of("garbage\n"), None);
        assert_eq!(
            memory_info_of("1000-2000 r--p 00000000 fe:00 42\nRss: 4 kB\n")
                .unwrap()
                .other
                .rss,
            4
        );
    }
}
