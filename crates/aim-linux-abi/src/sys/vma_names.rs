//! Names of anonymous mappings (`prctl(PR_SET_VMA, PR_SET_VMA_ANON_NAME)`,
//! Linux's mm/madvise.c `anon_vma_name`), which `/proc/<pid>/maps` and
//! `smaps` show as `[anon:<name>]` (fs/proc/task_mmu.c `get_vma_name`).
//!
//! Linux keeps a name with each VMA: naming part of a mapping splits it, a
//! split (mprotect, partial munmap) keeps the name on both parts, mremap
//! carries it with the pages, and a new mapping over the range has none.
//! The layer's mappings are Darwin's VM map entries, which do not split
//! where a name ends, so the names are kept here by range and the entries
//! are cut where they change when the maps are read.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::mem::PAGE;
use super::user_memory;
use crate::errno::{EBADF, EFAULT, EINVAL, ENOMEM};

/// `PR_SET_VMA_ANON_NAME`.
const PR_SET_VMA_ANON_NAME: u64 = 0;
/// `ANON_VMA_NAME_MAX_LEN`, with the terminator.
const ANON_VMA_NAME_MAX_LEN: usize = 80;

/// Named ranges by start: their end and name. Ranges never overlap, and
/// adjacent ones with the same name are one (VMAs that merge).
static NAMES: Mutex<BTreeMap<u64, (u64, Arc<str>)>> = Mutex::new(BTreeMap::new());

/// Names `[lo, hi)`, or clears its names with `None`.
fn set(names: &mut BTreeMap<u64, (u64, Arc<str>)>, lo: u64, hi: u64, name: Option<Arc<str>>) {
    if lo >= hi {
        return;
    }
    // The ranges overlapping [lo, hi), cut to what lies outside it.
    let overlapping: Vec<(u64, u64, Arc<str>)> = names
        .range(..hi)
        .rev()
        .take_while(|(_, (end, _))| *end > lo)
        .map(|(&start, (end, n))| (start, *end, n.clone()))
        .collect();
    for (start, end, n) in overlapping {
        names.remove(&start);
        if start < lo {
            names.insert(start, (lo, n.clone()));
        }
        if end > hi {
            names.insert(hi, (end, n));
        }
    }
    let Some(name) = name else { return };
    let (mut lo, mut hi) = (lo, hi);
    // Merged with a neighbour of the same name, as `vma_merge` merges.
    if let Some((&start, (end, n))) = names.range(..lo).next_back()
        && *end == lo
        && **n == *name
    {
        lo = start;
        names.remove(&start);
    }
    if let Some((end, n)) = names.get(&hi)
        && **n == *name
    {
        let end = *end;
        names.remove(&hi);
        hi = end;
    }
    names.insert(lo, (hi, name));
}

/// The names of `[lo, hi)` cut into pieces that each have one name or none,
/// in order.
pub(super) fn pieces(lo: u64, hi: u64) -> Vec<(u64, u64, Option<Arc<str>>)> {
    let names = NAMES.lock().unwrap();
    let mut out = Vec::new();
    let mut cur = lo;
    let first = names
        .range(..lo)
        .next_back()
        .filter(|(_, (end, _))| *end > lo)
        .map(|(&s, _)| s);
    let from = first.unwrap_or(lo);
    for (&start, (end, name)) in names.range(from..hi) {
        let (start, end) = (start.max(lo), (*end).min(hi));
        if start > cur {
            out.push((cur, start, None));
        }
        out.push((start, end, Some(name.clone())));
        cur = end;
    }
    if cur < hi {
        out.push((cur, hi, None));
    }
    out
}

/// A new mapping over `[lo, hi)`, or its unmapping: no name there.
pub(super) fn forget(lo: u64, hi: u64) {
    set(&mut NAMES.lock().unwrap(), lo, hi, None);
}

/// mremap of `[old, old + old_len)` to `[new, new + new_len)`: the pages
/// keep their names, and growth takes the name of the mapping it extends
/// (`vma_expand`). With `keep_old` (MREMAP_DONTUNMAP) the old range stays
/// mapped, and named.
pub(super) fn remap(old: u64, old_len: u64, new: u64, new_len: u64, keep_old: bool) {
    let mut names = NAMES.lock().unwrap();
    let moved: Vec<(u64, u64, Arc<str>)> = {
        let kept = old_len.min(new_len);
        let from = names
            .range(..old)
            .next_back()
            .filter(|(_, (end, _))| *end > old)
            .map_or(old, |(&s, _)| s);
        names
            .range(from..old + kept)
            .map(|(&s, (e, n))| (s.max(old), (*e).min(old + kept), n.clone()))
            .collect()
    };
    let grown = (new_len > old_len)
        .then(|| {
            names
                .range(..old + old_len)
                .next_back()
                .filter(|(_, (end, _))| *end >= old + old_len)
                .map(|(_, (_, n))| n.clone())
        })
        .flatten();
    if !keep_old {
        set(&mut names, old, old + old_len, None);
    }
    set(&mut names, new, new + new_len, None);
    for (s, e, n) in moved {
        set(&mut names, new + (s - old), new + (e - old), Some(n));
    }
    if let Some(n) = grown {
        set(&mut names, new + old_len, new + new_len, Some(n));
    }
}

/// execve in place: the new image has no names.
pub(super) fn exec_reset() {
    NAMES.lock().unwrap().clear();
}

/// Fork: the child's mappings keep their names (`dup_mmap`).
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    let names = NAMES.lock().unwrap();
    w.seq(names.iter(), |w, (&start, (end, name))| {
        w.u64(start);
        w.u64(*end);
        w.str(name);
    });
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    let names = r.seq(|r| {
        let start = r.u64();
        let end = r.u64();
        let name: Arc<str> = r.str().into();
        (start, (end, name))
    });
    *NAMES.lock().unwrap() = names.into_iter().collect();
}

/// `is_valid_name_char`: printable, and none of the characters that would
/// make `maps` ambiguous.
fn valid_name_char(c: u8) -> bool {
    c > 0x1f && c < 0x7f && !b"\\`$[]".contains(&c)
}

/// `prctl(PR_SET_VMA, option, addr, size, arg)` (`prctl_set_vma`,
/// `madvise_set_anon_name`): names the anonymous mappings of the range,
/// or clears their names with a null `arg`.
pub(super) fn prctl_set_vma(option: u64, addr: u64, size: u64, arg: u64) -> i64 {
    if option != PR_SET_VMA_ANON_NAME {
        return -(EINVAL as i64);
    }
    let name: Option<Arc<str>> = if arg == 0 {
        None
    } else {
        // `strndup_user(uname, ANON_VMA_NAME_MAX_LEN)`.
        let bytes = match user_memory::read_cstr(arg, ANON_VMA_NAME_MAX_LEN) {
            Ok(bytes) => bytes,
            Err(e) if e == EFAULT => return -(EFAULT as i64),
            Err(_) => return -(EINVAL as i64),
        };
        if !bytes.iter().all(|&c| valid_name_char(c)) {
            return -(EINVAL as i64);
        }
        Some(String::from_utf8_lossy(&bytes).into())
    };
    if addr & (PAGE - 1) != 0 {
        return -(EINVAL as i64);
    }
    let len = size.wrapping_add(PAGE - 1) & !(PAGE - 1);
    if size != 0 && len == 0 {
        return -(EINVAL as i64);
    }
    let Some(end) = addr.checked_add(len) else {
        return -(EINVAL as i64);
    };
    if end == addr {
        return 0;
    }
    // `madvise_walk_vmas`: each mapping of the range in turn, a file
    // mapping refused, a hole reported once the rest is named.
    let mut cur = addr;
    let mut unmapped = false;
    for vma in super::procfs::vmas_in(addr, end) {
        let (start, stop) = (vma.region.start, vma.region.end);
        if start > cur {
            unmapped = true;
        }
        // `madvise_vma_anon_name`: only anonymous mappings (shared ones
        // included) take a name.
        if vma.file || vma.shmem {
            return -(EBADF as i64);
        }
        set(&mut NAMES.lock().unwrap(), start, stop, name.clone());
        cur = stop;
    }
    if cur < end || unmapped {
        return -(ENOMEM as i64);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(names: &BTreeMap<u64, (u64, Arc<str>)>) -> Vec<(u64, u64, String)> {
        names
            .iter()
            .map(|(&s, (e, n))| (s, *e, n.to_string()))
            .collect()
    }

    #[test]
    fn naming_part_of_a_range_splits_and_like_names_merge() {
        let mut m = BTreeMap::new();
        set(&mut m, 0x1000, 0x5000, Some("a".into()));
        set(&mut m, 0x2000, 0x3000, Some("b".into()));
        assert_eq!(
            named(&m),
            [
                (0x1000, 0x2000, "a".into()),
                (0x2000, 0x3000, "b".into()),
                (0x3000, 0x5000, "a".into())
            ]
        );
        set(&mut m, 0x2000, 0x3000, Some("a".into()));
        assert_eq!(named(&m), [(0x1000, 0x5000, "a".into())]);
        set(&mut m, 0x2000, 0x3000, None);
        assert_eq!(
            named(&m),
            [(0x1000, 0x2000, "a".into()), (0x3000, 0x5000, "a".into())]
        );
    }

    #[test]
    fn only_these_characters_make_a_name() {
        assert!(
            b"dalvik-main space (region space)"
                .iter()
                .all(|&c| valid_name_char(c))
        );
        for c in [b'[', b']', b'$', b'`', b'\\', 0x7f, b'\n'] {
            assert!(!valid_name_char(c), "{c:#x}");
        }
    }
}
