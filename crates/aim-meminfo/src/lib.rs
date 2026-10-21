//! A process's `/proc/<pid>/smaps` sorted into the heaps
//! `Debug.MemoryInfo` reports, as `android_os_Debug_getDirtyPagesPid`
//! sorts it with libmeminfo's `ExtractAndroidHeapStats` at
//! `android-16.0.0_r1`. The app host's agent sorts it in the guest, where
//! smaps is read, and hands the app host the heaps (`MemoryInfo::ints`),
//! which fit a binder reply as a process's smaps need not.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

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

/// One kind of memory's sizes in `Debug.MemoryInfo` (libmeminfo's
/// `AndroidHeapStats`), in KiB, in the order of its `OFFSET_*`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub pss: i32,
    pub swappable_pss: i32,
    pub rss: i32,
    pub private_dirty: i32,
    pub shared_dirty: i32,
    pub private_clean: i32,
    pub shared_clean: i32,
    pub swapped_out: i32,
    pub swapped_out_pss: i32,
}

/// `Debug.MemoryInfo.NUM_OTHER_STATS + NUM_DVK_STATS`.
pub const NUM_OTHER_KINDS: usize = NUM_HEAP - NUM_CORE_HEAP;

/// `Debug.MemoryInfo`: a process's dalvik, native and other memory, and
/// each other kind's (`otherStats`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemoryInfo {
    pub dalvik: Stats,
    pub native: Stats,
    pub other: Stats,
    pub has_swapped_out_pss: bool,
    pub other_stats: [Stats; NUM_OTHER_KINDS],
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

    fn from_values(v: [i32; 9]) -> Self {
        Stats {
            pss: v[0],
            swappable_pss: v[1],
            rss: v[2],
            private_dirty: v[3],
            shared_dirty: v[4],
            private_clean: v[5],
            shared_clean: v[6],
            swapped_out: v[7],
            swapped_out_pss: v[8],
        }
    }

    pub fn values(&self) -> [i32; 9] {
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
pub fn memory_info_of(smaps: &str) -> Option<MemoryInfo> {
    let (mut stats, found_swap_pss) = heap_stats(smaps)?;
    for i in NUM_CORE_HEAP..NUM_EXCLUSIVE_HEAP {
        let values = stats[i].values().map(|v| v as u64);
        stats[HEAP_UNKNOWN].add(values);
    }
    let mut other_stats = [Stats::default(); NUM_OTHER_KINDS];
    other_stats.copy_from_slice(&stats[NUM_CORE_HEAP..]);
    Some(MemoryInfo {
        dalvik: stats[HEAP_DALVIK],
        native: stats[HEAP_NATIVE],
        other: stats[HEAP_UNKNOWN],
        has_swapped_out_pss: found_swap_pss,
        other_stats,
    })
}

/// The ints of `MemoryInfo::ints`: dalvik's, native's and the other
/// memory's nine, `hasSwappedOutPss`, then `otherStats`.
pub const INTS: usize = 9 * 3 + 1 + 9 * NUM_OTHER_KINDS;

impl MemoryInfo {
    /// Its fields as ints, in `INTS`' order.
    pub fn ints(&self) -> Vec<i32> {
        let mut ints = Vec::with_capacity(INTS);
        for stats in [&self.dalvik, &self.native, &self.other] {
            ints.extend(stats.values());
        }
        ints.push(self.has_swapped_out_pss as i32);
        ints.extend(self.other_stats.iter().flat_map(Stats::values));
        ints
    }

    /// The `MemoryInfo` of `ints`' fields; none unless there are `INTS`.
    pub fn from_ints(ints: &[i32]) -> Option<Self> {
        if ints.len() != INTS {
            return None;
        }
        let stats = |at: usize| Stats::from_values(ints[at..at + 9].try_into().unwrap());
        let mut other_stats = [Stats::default(); NUM_OTHER_KINDS];
        for (i, s) in other_stats.iter_mut().enumerate() {
            *s = stats(28 + 9 * i);
        }
        Some(MemoryInfo {
            dalvik: stats(0),
            native: stats(9),
            other: stats(18),
            has_swapped_out_pss: ints[27] != 0,
            other_stats,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A process of many mappings (a browser's tens of thousands) sorts as
    /// one of a few, and its heaps' ints stay `INTS` whatever its size.
    #[test]
    fn a_large_smaps_sorts_into_ints_of_a_fixed_size() {
        let smaps: String = (0..40_000u64)
            .map(|i| {
                vma(
                    &format!("{:x}-{:x}", 0x1000 * (2 * i + 1), 0x1000 * (2 * i + 2)),
                    "[anon:scudo:primary]",
                    4,
                    2,
                    0,
                    0,
                    2,
                    0,
                )
            })
            .collect();
        assert!(smaps.len() > 10 << 20);
        let info = memory_info_of(&smaps).unwrap();
        assert_eq!((info.native.pss, info.native.rss), (80_000, 160_000));
        let ints = info.ints();
        assert_eq!(ints.len(), INTS);
        assert_eq!(MemoryInfo::from_ints(&ints), Some(info));
        assert_eq!(MemoryInfo::from_ints(&ints[1..]), None);
    }
}
