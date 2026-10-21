//! The guest's virtual memory as Linux counts it against RLIMIT_AS and
//! RLIMIT_DATA (`mm->total_vm`, `mm->data_vm`, `may_expand_vm`).
//!
//! The layer shares the address space with the guest (linux-run, dyld, the
//! heap reference window's free pages, the translation caches), so the host
//! VM map cannot give these totals: they are kept here from the guest's own
//! mappings. A mapping counts from the moment the guest maps it (a page
//! taken from the window included) until it unmaps it. Not counted: the
//! main thread's stack, which the layer reserves whole where Linux counts
//! only the part the stack has grown into, and the vDSO.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// One guest mapping: its end and whether it is a data mapping
/// (`is_data_mapping`: private and writable, not a stack).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Area {
    end: u64,
    shared: bool,
    writable: bool,
    stack: bool,
}

impl Area {
    fn data(&self) -> bool {
        self.writable && !self.shared && !self.stack
    }
}

/// The guest's mappings by start address, and the main executable's data
/// segment (`start_data`, `end_data`) that `brk` counts with the heap.
#[derive(Default)]
pub(super) struct Space {
    areas: BTreeMap<u64, Area>,
    data_segment: u64,
}

/// What a mapping is, as its flags make it.
#[derive(Clone, Copy)]
pub(super) struct Kind {
    pub shared: bool,
    pub writable: bool,
    pub stack: bool,
}

impl Kind {
    pub fn data(self) -> bool {
        self.writable && !self.shared && !self.stack
    }
}

/// The soft and hard limits of RLIMIT_AS and RLIMIT_DATA.
#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub address_space: u64,
    pub data: (u64, u64),
}

impl Space {
    /// Takes `[lo, hi)` out of the areas, splitting those it cuts.
    fn cut(&mut self, lo: u64, hi: u64) {
        let mut tail = None;
        if let Some((_, area)) = self.areas.range_mut(..lo).next_back()
            && area.end > lo
        {
            if area.end > hi {
                tail = Some((hi, *area));
            }
            area.end = lo;
        }
        let inside: Vec<u64> = self.areas.range(lo..hi).map(|(&s, _)| s).collect();
        for start in inside {
            let area = self.areas.remove(&start).unwrap();
            if area.end > hi {
                tail = Some((hi, area));
            }
        }
        if let Some((start, area)) = tail {
            self.areas.insert(start, area);
        }
    }

    /// The guest mapped `[lo, hi)`, replacing what was there.
    pub fn map(&mut self, lo: u64, hi: u64, kind: Kind) {
        if lo >= hi {
            return;
        }
        self.cut(lo, hi);
        self.areas.insert(
            lo,
            Area {
                end: hi,
                shared: kind.shared,
                writable: kind.writable,
                stack: kind.stack,
            },
        );
    }

    /// The guest unmapped `[lo, hi)`.
    pub fn unmap(&mut self, lo: u64, hi: u64) {
        if lo < hi {
            self.cut(lo, hi);
        }
    }

    /// The pieces of the areas inside `[lo, hi)`.
    fn pieces(&self, lo: u64, hi: u64) -> Vec<(u64, u64, Area)> {
        let mut out = Vec::new();
        if let Some((_, area)) = self.areas.range(..lo).next_back()
            && area.end > lo
        {
            out.push((lo, area.end.min(hi), *area));
        }
        for (&start, area) in self.areas.range(lo..hi) {
            out.push((start, area.end.min(hi), *area));
        }
        out
    }

    /// `mprotect` made `[lo, hi)` writable or not.
    pub fn protect(&mut self, lo: u64, hi: u64, writable: bool) {
        for (start, end, area) in self.pieces(lo, hi) {
            self.cut(start, end);
            self.areas.insert(
                start,
                Area {
                    end,
                    writable,
                    ..area
                },
            );
        }
    }

    /// The kind of the mapping at `addr`, if the guest mapped it.
    pub fn kind_at(&self, addr: u64) -> Option<Kind> {
        let (_, area) = self.areas.range(..=addr).next_back()?;
        (area.end > addr).then_some(Kind {
            shared: area.shared,
            writable: area.writable,
            stack: area.stack,
        })
    }

    /// `(total_vm, data_vm)` in bytes, the program break's heap `brk`
    /// bytes counted as data.
    fn totals(&self, brk: u64) -> (u64, u64) {
        let (mut total, mut data) = (brk, brk);
        for (&start, area) in &self.areas {
            total += area.end - start;
            if area.data() {
                data += area.end - start;
            }
        }
        (total, data)
    }

    /// Bytes of `[lo, hi)` already mapped (`count_vma_pages_range`).
    fn mapped_in(&self, lo: u64, hi: u64) -> u64 {
        self.pieces(lo, hi).iter().map(|(s, e, _)| e - s).sum()
    }

    /// `may_expand_vm(mm, flags, len)`.
    pub fn may_expand(&self, limits: Limits, brk: u64, data: bool, len: u64) -> bool {
        let (total, data_vm) = self.totals(brk);
        if total.saturating_add(len) > limits.address_space {
            return false;
        }
        if data && data_vm.saturating_add(len) > limits.data.0 {
            // Valgrind's workaround: a zero soft limit under the hard one.
            return limits.data.0 == 0 && data_vm.saturating_add(len) <= limits.data.1;
        }
        true
    }

    /// `mmap_region`'s check: a fixed mapping is allowed if what it would
    /// unmap of `[lo, lo + len)` makes room.
    pub fn may_map(&self, limits: Limits, brk: u64, lo: u64, len: u64, kind: Kind) -> bool {
        self.may_expand(limits, brk, kind.data(), len)
            || self.may_expand(
                limits,
                brk,
                kind.data(),
                len - self.mapped_in(lo, lo.saturating_add(len)),
            )
    }

    /// `mprotect_fixup`'s check for `[lo, hi)` becoming writable: each
    /// mapping that turns into data must fit RLIMIT_DATA.
    pub fn may_protect(&self, limits: Limits, brk: u64, lo: u64, hi: u64) -> bool {
        let (_, mut data_vm) = self.totals(brk);
        for (start, end, area) in self.pieces(lo, hi) {
            if area.data() || area.shared || area.stack {
                continue;
            }
            data_vm = data_vm.saturating_add(end - start);
            if data_vm > limits.data.0 && !(limits.data.0 == 0 && data_vm <= limits.data.1) {
                return false;
            }
        }
        true
    }

    /// `check_data_rlimit` for a program break moved to `heap` bytes above
    /// its start.
    pub fn brk_fits(&self, limits: Limits, heap: u64) -> bool {
        limits.data.0 == u64::MAX || heap.saturating_add(self.data_segment) <= limits.data.0
    }

    /// The main executable's data segment, as exec sets `start_data` and
    /// `end_data`.
    pub fn set_data_segment(&mut self, bytes: u64) {
        self.data_segment = bytes;
    }
}

static SPACE: Mutex<Option<Space>> = Mutex::new(None);

/// Runs `f` on this process's accounting.
pub(super) fn with<R>(f: impl FnOnce(&mut Space) -> R) -> R {
    let mut space = SPACE.lock().unwrap();
    f(space.get_or_insert_with(Space::default))
}

/// Fork: the child has the parent's mappings.
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    with(|s| {
        w.u64(s.data_segment);
        w.seq(s.areas.iter(), |w, (&start, area)| {
            w.u64(start);
            w.u64(area.end);
            w.bool(area.shared);
            w.bool(area.writable);
            w.bool(area.stack);
        });
    });
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    let data_segment = r.u64();
    let areas = r.seq(|r| {
        let start = r.u64();
        let area = Area {
            end: r.u64(),
            shared: r.bool(),
            writable: r.bool(),
            stack: r.bool(),
        };
        (start, area)
    });
    *SPACE.lock().unwrap() = Some(Space {
        areas: areas.into_iter().collect(),
        data_segment,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: u64 = 16384;
    const PRIVATE_RW: Kind = Kind {
        shared: false,
        writable: true,
        stack: false,
    };
    const PRIVATE_RO: Kind = Kind {
        shared: false,
        writable: false,
        stack: false,
    };
    const SHARED_RW: Kind = Kind {
        shared: true,
        writable: true,
        stack: false,
    };

    fn limits(address_space: u64, data: u64) -> Limits {
        Limits {
            address_space,
            data: (data, data),
        }
    }

    #[test]
    fn mappings_count_until_unmapped_and_fixed_ones_replace() {
        let mut s = Space::default();
        s.map(0x10000, 0x10000 + 4 * PAGE, PRIVATE_RW);
        s.map(0x80000, 0x80000 + 2 * PAGE, SHARED_RW);
        assert_eq!(s.totals(0), (6 * PAGE, 4 * PAGE));
        // MAP_FIXED over the middle replaces two pages.
        s.map(0x10000 + PAGE, 0x10000 + 3 * PAGE, PRIVATE_RO);
        assert_eq!(s.totals(0), (6 * PAGE, 2 * PAGE));
        s.unmap(0x10000, 0x10000 + 2 * PAGE);
        assert_eq!(s.totals(0), (4 * PAGE, PAGE));
        assert_eq!(s.areas.len(), 3);
    }

    #[test]
    fn may_expand_vm_checks_both_limits() {
        let mut s = Space::default();
        s.map(0, 4 * PAGE, PRIVATE_RW);
        let l = limits(8 * PAGE, 5 * PAGE);
        assert!(s.may_expand(l, 0, false, 4 * PAGE));
        assert!(!s.may_expand(l, 0, false, 5 * PAGE));
        assert!(s.may_expand(l, 0, true, PAGE));
        assert!(!s.may_expand(l, 0, true, 2 * PAGE));
        // The heap counts as data.
        assert!(!s.may_expand(l, PAGE, true, PAGE));
        // A zero soft data limit under the hard one (Valgrind).
        let valgrind = Limits {
            address_space: u64::MAX,
            data: (0, 8 * PAGE),
        };
        assert!(s.may_expand(valgrind, 0, true, 4 * PAGE));
        assert!(!s.may_expand(valgrind, 0, true, 5 * PAGE));
    }

    #[test]
    fn a_fixed_mapping_may_reuse_what_it_replaces() {
        let mut s = Space::default();
        s.map(0, 4 * PAGE, PRIVATE_RO);
        let l = limits(4 * PAGE, u64::MAX);
        assert!(!s.may_map(l, 0, 8 * PAGE, PAGE, PRIVATE_RO));
        assert!(s.may_map(l, 0, 2 * PAGE, 2 * PAGE, PRIVATE_RO));
        assert!(!s.may_map(l, 0, 3 * PAGE, 2 * PAGE, PRIVATE_RO));
    }

    #[test]
    fn mprotect_turning_mappings_into_data_is_checked() {
        let mut s = Space::default();
        s.map(0, 4 * PAGE, PRIVATE_RO);
        s.map(8 * PAGE, 10 * PAGE, SHARED_RW);
        let l = limits(u64::MAX, 2 * PAGE);
        assert!(s.may_protect(l, 0, 0, 2 * PAGE));
        assert!(!s.may_protect(l, 0, 0, 3 * PAGE));
        // Shared mappings are never data.
        assert!(s.may_protect(l, 0, 8 * PAGE, 10 * PAGE));
        s.protect(0, 2 * PAGE, true);
        assert_eq!(s.totals(0), (6 * PAGE, 2 * PAGE));
        assert!(!s.may_protect(l, 0, 2 * PAGE, 3 * PAGE));
        assert_eq!(s.kind_at(PAGE).map(|k| k.data()), Some(true));
        assert_eq!(s.kind_at(3 * PAGE).map(|k| k.data()), Some(false));
        assert!(s.kind_at(5 * PAGE).is_none());
    }

    #[test]
    fn brk_counts_the_data_segment() {
        let mut s = Space::default();
        s.set_data_segment(PAGE);
        assert!(s.brk_fits(limits(u64::MAX, 3 * PAGE), 2 * PAGE));
        assert!(!s.brk_fits(limits(u64::MAX, 3 * PAGE), 3 * PAGE));
        assert!(s.brk_fits(limits(u64::MAX, u64::MAX), u64::MAX));
    }
}
