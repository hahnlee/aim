//! `/proc/<pid>/smaps` and `smaps_rollup` (proc(5); Linux 6.12's
//! fs/proc/task_mmu.c, `show_smap` and `show_smaps_rollup`), from the page
//! counts Darwin keeps of each VM map entry ([`vmmap::Usage`]).
//!
//! Linux accounts page by page: a page one page table maps is private, one
//! that n map is shared, and 1/n of it is in each mapper's Pss. Darwin
//! counts per entry, so the accounting is approximated:
//!
//! - Whether resident pages are shared follows the entry's share mode: all
//!   private (`SM_PRIVATE`, `SM_LARGE_PAGE`) or all shared (the shared
//!   modes). Copied on write (`SM_COW`), the pages of a top object only
//!   this entry references and those a shadow keeps for it alone are
//!   private, the rest shared.
//! - A shared page's mappers are the references to the entry's objects,
//!   less a file's own (as xnu's `fill_procregioninfo` discounts it). A
//!   reference that is no mapping (a memory entry's, say) counts too, so
//!   Pss of shared memory can be lower than Linux's. With one left the
//!   pages are private.
//! - Darwin counts dirty pages but does not place them: they go to the
//!   private pages first (a copied page is a written one), then to the
//!   shared.
//! - Swap is the pages in Darwin's compressor, as Android's zram holds
//!   them; SwapPss divides them by the mappers of a shared entry.
//! - Anonymous is all of an anonymous private mapping and, of a private
//!   mapping of a file, its private dirty pages (the copies writes made).
//!   Shared anonymous memory and memfds are shmem, as on Linux.
//! - Referenced is the resident pages `mincore` finds referenced.
//! - LazyFree is 0: Darwin's MADV_FREE leaves pages clean, not apart.
//! - Nothing merges pages (KSM) or maps huge pages (THP, hugetlb): 0, and
//!   THPeligible 0.
//! - VmFlags has what the protections, sharing, the stack and mlock tell.

use std::fmt::Write;

use super::mem::PAGE;
use super::procfs::{self, Vma};
use super::vmmap::{SM_COW, SM_EMPTY, SM_LARGE_PAGE, SM_PRIVATE, Usage};

/// Pss is kept in bytes with this many fractional bits, as Linux keeps it.
const PSS_SHIFT: u32 = 12;

/// What memory a mapping holds, as Linux sorts its Pss in the rollup.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Anon,
    File,
    Shmem,
}

/// A mapping's sizes, or the rollup's, in bytes; the Pss sizes and Locked
/// in bytes << `PSS_SHIFT`.
#[derive(Debug, Default, PartialEq)]
struct Stats {
    rss: u64,
    pss: u64,
    pss_dirty: u64,
    pss_anon: u64,
    pss_file: u64,
    pss_shmem: u64,
    shared_clean: u64,
    shared_dirty: u64,
    private_clean: u64,
    private_dirty: u64,
    referenced: u64,
    anonymous: u64,
    swap: u64,
    swap_pss: u64,
    locked: u64,
}

impl Stats {
    fn add(&mut self, s: &Stats) {
        self.rss += s.rss;
        self.pss += s.pss;
        self.pss_dirty += s.pss_dirty;
        self.pss_anon += s.pss_anon;
        self.pss_file += s.pss_file;
        self.pss_shmem += s.pss_shmem;
        self.shared_clean += s.shared_clean;
        self.shared_dirty += s.shared_dirty;
        self.private_clean += s.private_clean;
        self.private_dirty += s.private_dirty;
        self.referenced += s.referenced;
        self.anonymous += s.anonymous;
        self.swap += s.swap;
        self.swap_pss += s.swap_pss;
        self.locked += s.locked;
    }
}

/// The sizes of a mapping with Darwin's counts `u`: of `kind`, mapped
/// private or shared, from a host file (whose pager holds a reference) or
/// not, with `referenced` of its pages referenced.
fn account(u: &Usage, kind: Kind, private_map: bool, host_file: bool, referenced: u64) -> Stats {
    let r = u.resident;
    let (private, mappers) = match u.share_mode {
        SM_PRIVATE | SM_LARGE_PAGE | SM_EMPTY => (r, 1),
        SM_COW => (
            (u.private_resident + u.shared_now_private).min(r),
            u.ref_count.max(2),
        ),
        _ => {
            let m = u.ref_count.saturating_sub(host_file as u32).max(1);
            (if m == 1 { r } else { 0 }, m)
        }
    };
    let mappers = mappers as u64;
    let shared = r - private;
    let dirty = u.dirtied.min(r);
    let private_dirty = dirty.min(private);
    let shared_dirty = dirty - private_dirty;
    let bytes = |pages: u64| pages * PAGE;
    let pss = |private: u64, shared: u64| {
        (bytes(private) << PSS_SHIFT) + (bytes(shared) << PSS_SHIFT) / mappers
    };
    let all = pss(private, shared);
    let anonymous = match kind {
        Kind::Anon => r,
        _ if private_map => private_dirty,
        _ => 0,
    };
    let anon_pss = if kind == Kind::Anon {
        all
    } else {
        (bytes(anonymous) << PSS_SHIFT).min(all)
    };
    let swap_mappers = if matches!(u.share_mode, SM_PRIVATE | SM_LARGE_PAGE | SM_EMPTY | SM_COW) {
        1
    } else {
        mappers
    };
    Stats {
        rss: bytes(r),
        pss: all,
        pss_dirty: pss(private_dirty, shared_dirty),
        pss_anon: anon_pss,
        pss_file: if kind == Kind::File {
            all - anon_pss
        } else {
            0
        },
        pss_shmem: if kind == Kind::Shmem {
            all - anon_pss
        } else {
            0
        },
        shared_clean: bytes(shared - shared_dirty),
        shared_dirty: bytes(shared_dirty),
        private_clean: bytes(private - private_dirty),
        private_dirty: bytes(private_dirty),
        referenced: bytes(referenced.min(r)),
        anonymous: bytes(anonymous),
        swap: bytes(u.swapped_out),
        swap_pss: (bytes(u.swapped_out) << PSS_SHIFT) / swap_mappers,
        locked: if u.wired { all } else { 0 },
    }
}

/// The resident pages of `[start, end)` of this process that `mincore`
/// finds referenced (by this process or another).
fn referenced(start: u64, end: u64) -> u64 {
    let mut vec = vec![0 as libc::c_char; ((end - start) / PAGE) as usize];
    // SAFETY: one byte per page of the range into `vec`.
    let r = unsafe { libc::mincore(start as *const _, (end - start) as usize, vec.as_mut_ptr()) };
    // A range unmapped since the walk has no pages.
    if r != 0 {
        return 0;
    }
    let referenced = libc::MINCORE_REFERENCED | libc::MINCORE_REFERENCED_OTHER;
    vec.iter()
        .filter(|&&b| b as i32 & libc::MINCORE_INCORE != 0 && b as i32 & referenced != 0)
        .count() as u64
}

fn kind(v: &Vma) -> Kind {
    if v.shmem || (!v.file && v.region.shared) {
        Kind::Shmem
    } else if v.file {
        Kind::File
    } else {
        Kind::Anon
    }
}

fn stats(v: &Vma) -> Stats {
    let u = &v.region.usage;
    let referenced = if u.resident > 0 {
        referenced(v.region.start, v.region.end)
    } else {
        0
    };
    account(
        u,
        kind(v),
        !v.region.shared,
        v.region.file.is_some(),
        referenced,
    )
}

/// One line of sizes: the label, then kB right-aligned to column 24.
fn put(out: &mut String, label: &str, bytes: u64) {
    let pad = (label.len() + 1).max(16);
    let _ = writeln!(out, "{label:<pad$}{:>w$} kB", bytes >> 10, w = 24 - pad);
}

/// `__show_smap`: the sizes after a mapping's (or the rollup's) header.
fn put_stats(out: &mut String, s: &Stats, rollup: bool) {
    let pss = |v: u64| v >> PSS_SHIFT;
    put(out, "Rss:", s.rss);
    put(out, "Pss:", pss(s.pss));
    put(out, "Pss_Dirty:", pss(s.pss_dirty));
    if rollup {
        put(out, "Pss_Anon:", pss(s.pss_anon));
        put(out, "Pss_File:", pss(s.pss_file));
        put(out, "Pss_Shmem:", pss(s.pss_shmem));
    }
    put(out, "Shared_Clean:", s.shared_clean);
    put(out, "Shared_Dirty:", s.shared_dirty);
    put(out, "Private_Clean:", s.private_clean);
    put(out, "Private_Dirty:", s.private_dirty);
    put(out, "Referenced:", s.referenced);
    put(out, "Anonymous:", s.anonymous);
    for label in [
        "KSM:",
        "LazyFree:",
        "AnonHugePages:",
        "ShmemPmdMapped:",
        "FilePmdMapped:",
        "Shared_Hugetlb:",
        "Private_Hugetlb:",
    ] {
        put(out, label, 0);
    }
    put(out, "Swap:", s.swap);
    put(out, "SwapPss:", pss(s.swap_pss));
    put(out, "Locked:", pss(s.locked));
}

/// `show_smap_vma_flags`: the two-letter names of the mapping's `VM_*`
/// flags in their bits' order.
fn vm_flags(v: &Vma) -> String {
    let (prot, max, shared) = (v.region.prot, v.region.usage.max_prot, v.region.shared);
    let flags = [
        ("rd", prot & 1 != 0),
        ("wr", prot & 2 != 0),
        ("ex", prot & 4 != 0),
        ("sh", shared),
        ("mr", max & 1 != 0),
        ("mw", max & 2 != 0),
        ("me", max & 4 != 0),
        ("ms", shared),
        ("gd", v.name == "[stack]"),
        ("lo", v.region.usage.wired),
    ];
    let mut out = String::from("VmFlags: ");
    for (name, _) in flags.iter().filter(|(_, set)| *set) {
        out.push_str(name);
        out.push(' ');
    }
    out.push('\n');
    out
}

/// This process's `smaps`, or with `rollup` its `smaps_rollup`.
pub(super) fn smaps(rollup: bool) -> String {
    let vmas = procfs::vmas();
    let all: Vec<Stats> = vmas.iter().map(stats).collect();
    if rollup {
        let lo = vmas.first().map_or(0, |v| v.region.start);
        let hi = vmas.last().map_or(0, |v| v.region.end);
        format_rollup(lo, hi, &all)
    } else {
        vmas.iter()
            .zip(&all)
            .map(|(v, s)| format_vma(v, s))
            .collect()
    }
}

fn format_vma(v: &Vma, s: &Stats) -> String {
    let mut out = v.line.clone();
    put(&mut out, "Size:", v.region.end - v.region.start);
    put(&mut out, "KernelPageSize:", PAGE);
    put(&mut out, "MMUPageSize:", PAGE);
    put_stats(&mut out, s, false);
    let _ = writeln!(out, "THPeligible:    {:>8}", 0);
    out.push_str(&vm_flags(v));
    out
}

/// `show_smaps_rollup`: a header from the first mapping's start to the last
/// one's end, then every mapping's sizes added up.
fn format_rollup(lo: u64, hi: u64, all: &[Stats]) -> String {
    let mut total = Stats::default();
    for s in all {
        total.add(s);
    }
    let mut out = format!("{lo:08x}-{hi:08x} ---p 00000000 00:00 0");
    procfs::push_name(&mut out, "[rollup]");
    out.push('\n');
    put_stats(&mut out, &total, true);
    out
}

#[cfg(test)]
mod tests {
    use super::super::vmmap::Region;
    use super::*;

    const SM_SHARED: u32 = 4;

    fn usage(share_mode: u32, ref_count: u32, resident: u64, dirtied: u64) -> Usage {
        Usage {
            max_prot: 7,
            share_mode,
            ref_count,
            resident,
            dirtied,
            ..Default::default()
        }
    }

    fn vma(start: u64, pages: u64, prot: u32, shared: bool, name: &str, usage: Usage) -> Vma {
        let region = Region {
            start,
            end: start + pages * PAGE,
            prot,
            shared,
            offset: 0,
            file: None,
            usage,
        };
        let mut line = format!(
            "{:08x}-{:08x} rw-p 00000000 00:00 0",
            region.start, region.end
        );
        if !name.is_empty() {
            procfs::push_name(&mut line, name);
        }
        line.push('\n');
        Vma {
            region,
            line,
            name: name.into(),
            file: false,
            shmem: false,
        }
    }

    /// An anonymous private mapping is private and anonymous; its dirty
    /// pages are private dirty, and Pss is Rss.
    #[test]
    fn a_private_anonymous_mapping_is_private() {
        let u = Usage {
            swapped_out: 2,
            ..usage(SM_PRIVATE, 1, 10, 4)
        };
        let s = account(&u, Kind::Anon, true, false, 3);
        assert_eq!(
            (s.rss, s.pss >> PSS_SHIFT, s.pss_anon),
            (10 * PAGE, 10 * PAGE, s.pss)
        );
        assert_eq!(
            (
                s.private_dirty,
                s.private_clean,
                s.shared_clean,
                s.shared_dirty
            ),
            (4 * PAGE, 6 * PAGE, 0, 0)
        );
        assert_eq!((s.anonymous, s.referenced), (10 * PAGE, 3 * PAGE));
        assert_eq!((s.swap, s.swap_pss >> PSS_SHIFT), (2 * PAGE, 2 * PAGE));
    }

    /// After a fork, the pages still shared with the other process count
    /// half; the copies (in the top object) count whole.
    #[test]
    fn copied_on_write_pages_split_into_private_and_shared() {
        let u = Usage {
            private_resident: 3,
            ..usage(SM_COW, 2, 8, 5)
        };
        let s = account(&u, Kind::Anon, true, false, 0);
        assert_eq!((s.private_dirty, s.private_clean), (3 * PAGE, 0));
        assert_eq!((s.shared_dirty, s.shared_clean), (2 * PAGE, 3 * PAGE));
        assert_eq!(s.pss >> PSS_SHIFT, 3 * PAGE + 5 * PAGE / 2);
        assert_eq!(s.pss_dirty >> PSS_SHIFT, 3 * PAGE + 2 * PAGE / 2);
    }

    /// A file three processes map shared: the file's pager reference is
    /// not a mapper, so each has a third of it; none of it is anonymous.
    /// Mapped by one, it is private.
    #[test]
    fn a_shared_file_divides_by_its_mappers() {
        let s = account(&usage(SM_SHARED, 4, 9, 0), Kind::File, false, true, 9);
        assert_eq!(
            (s.shared_clean, s.private_clean, s.anonymous),
            (9 * PAGE, 0, 0)
        );
        assert_eq!(
            (s.pss >> PSS_SHIFT, s.pss_file, s.pss_anon),
            (3 * PAGE, s.pss, 0)
        );
        let one = account(&usage(SM_SHARED, 2, 9, 0), Kind::File, false, true, 0);
        assert_eq!((one.private_clean, one.shared_clean), (9 * PAGE, 0));
    }

    /// A private mapping of a file: its written pages are anonymous
    /// copies, the rest the file's.
    #[test]
    fn a_private_file_mapping_has_its_copies_anonymous() {
        let s = account(&usage(SM_PRIVATE, 1, 6, 2), Kind::File, true, true, 0);
        assert_eq!(
            (
                s.anonymous,
                s.pss_anon >> PSS_SHIFT,
                s.pss_file >> PSS_SHIFT
            ),
            (2 * PAGE, 2 * PAGE, 4 * PAGE)
        );
    }

    /// proc(5)'s layout: the `maps` line, the sizes in kB right-aligned to
    /// column 24, THPeligible, then VmFlags with a space after each.
    #[test]
    fn a_mapping_is_shown_as_linux_shows_it() {
        let u = Usage {
            wired: true,
            ..usage(SM_PRIVATE, 1, 2, 1)
        };
        let v = vma(0x7000_0000, 4, 3, false, "[stack]", u);
        let s = account(&u, kind(&v), true, false, 1);
        let text = format_vma(&v, &s);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            format!(
                "70000000-70010000 rw-p 00000000 00:00 0{}[stack]",
                " ".repeat(34)
            )
        );
        assert_eq!(lines[1], "Size:                 64 kB");
        assert_eq!(lines[2], "KernelPageSize:       16 kB");
        assert_eq!(lines[3], "MMUPageSize:          16 kB");
        assert_eq!(lines[4], "Rss:                  32 kB");
        assert_eq!(lines[5], "Pss:                  32 kB");
        assert_eq!(lines[6], "Pss_Dirty:            16 kB");
        let labels: Vec<&str> = lines[1..]
            .iter()
            .map(|l| l.split(':').next().unwrap())
            .collect();
        assert_eq!(
            labels,
            [
                "Size",
                "KernelPageSize",
                "MMUPageSize",
                "Rss",
                "Pss",
                "Pss_Dirty",
                "Shared_Clean",
                "Shared_Dirty",
                "Private_Clean",
                "Private_Dirty",
                "Referenced",
                "Anonymous",
                "KSM",
                "LazyFree",
                "AnonHugePages",
                "ShmemPmdMapped",
                "FilePmdMapped",
                "Shared_Hugetlb",
                "Private_Hugetlb",
                "Swap",
                "SwapPss",
                "Locked",
                "THPeligible",
                "VmFlags"
            ]
        );
        assert!(lines.contains(&"Private_Hugetlb:       0 kB"));
        assert!(lines.contains(&"Locked:               32 kB"));
        assert_eq!(lines[23], "THPeligible:           0");
        assert_eq!(lines[24], "VmFlags: rd wr mr mw me gd lo ");
        assert!(text.ends_with(" \n"));
    }

    /// The rollup's header spans the mappings and its sizes add up, with
    /// Pss sorted into anonymous, file and shmem.
    #[test]
    fn the_rollup_adds_every_mapping() {
        let a = account(&usage(SM_PRIVATE, 1, 2, 2), Kind::Anon, true, false, 0);
        let f = account(&usage(SM_SHARED, 3, 4, 0), Kind::File, false, true, 0);
        let m = account(&usage(SM_PRIVATE, 1, 1, 1), Kind::Shmem, false, true, 0);
        let text = format_rollup(0x1000_0000, 0x7fff_0000, &[a, f, m]);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            format!(
                "10000000-7fff0000 ---p 00000000 00:00 0{}[rollup]",
                " ".repeat(34)
            )
        );
        assert_eq!(
            &lines[1..7],
            [
                "Rss:                 112 kB",
                "Pss:                  80 kB",
                "Pss_Dirty:            48 kB",
                "Pss_Anon:             32 kB",
                "Pss_File:             32 kB",
                "Pss_Shmem:            16 kB",
            ]
        );
        assert_eq!(lines[7], "Shared_Clean:         64 kB");
        assert_eq!(lines.last(), Some(&"Locked:                0 kB"));
        assert!(!text.contains("VmFlags") && !text.contains("Size:"));
    }

    /// This process's own smaps: every mapping parses, and an anonymous
    /// mapping's Rss grows as its pages are touched.
    #[test]
    fn touched_pages_grow_rss() {
        let len = 64 * PAGE as usize;
        // SAFETY: a fresh anonymous mapping, unmapped below.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        assert_ne!(p, libc::MAP_FAILED);
        let start = p as u64;
        // Darwin may have joined the mapping to a neighbor: the one
        // containing it.
        let rss = || {
            let text = smaps(false);
            let mut lines = text.lines().skip_while(|l| {
                let (lo, hi) = l
                    .split_once(' ')
                    .and_then(|(r, _)| r.split_once('-'))
                    .unwrap_or(("", ""));
                let range = u64::from_str_radix(lo, 16)
                    .ok()
                    .zip(u64::from_str_radix(hi, 16).ok());
                !range.is_some_and(|(lo, hi)| (lo..hi).contains(&start))
            });
            assert!(lines.next().is_some(), "the mapping is listed");
            let rss = lines.find_map(|l| l.strip_prefix("Rss:")).unwrap();
            rss.trim()
                .strip_suffix(" kB")
                .unwrap()
                .parse::<u64>()
                .unwrap()
        };
        let before = rss();
        for i in 0..32 {
            // SAFETY: within the mapping.
            unsafe { *(p as *mut u8).add(i * PAGE as usize) = 1 };
        }
        assert!(rss() >= before + 32 * PAGE / 1024, "before {before} kB");
        let rollup = smaps(true);
        assert!(rollup.lines().nth(1).unwrap().starts_with("Rss:"));
        // SAFETY: the mapping made above.
        unsafe { libc::munmap(p, len) };
    }
}
