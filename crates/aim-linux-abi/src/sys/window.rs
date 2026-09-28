//! The heap reference window (#161, docs/art-exception-patches.md).
//!
//! macOS maps nothing below 4 GiB, so the ART exception keeps 32-bit heap
//! references as offsets from a 4 GiB-aligned base, and everything ART maps
//! "in the low 4 GiB" goes to `[BASE, BASE + SIZE)` instead. The layer keeps
//! that window for guest mappings placed there on purpose:
//!
//! - At start-up the window is reserved: an inaccessible host mapping
//!   carrying the tag [`TAG`], so no host allocation, loader mapping, thread
//!   stack or translated file can land in it.
//! - Reserved pages are unmapped as far as the guest can tell: they are left
//!   out of the VM map walk (`/proc/self/maps`, mincore), msync and mprotect
//!   fail there with ENOMEM, and a guest mmap whose hint (or
//!   MAP_FIXED_NOREPLACE address) lies on reserved pages gets exactly that
//!   address.
//! - Unmapping guest memory in the window (munmap, a shrinking or moving
//!   mremap) reserves it again.
//!
//! Without a hint, guest mappings never land in the window, as on Linux,
//! where ordinary mappings are placed far above it.

use super::mem::PAGE;
use super::vmmap;
use crate::patch::vm::{VM_FLAGS_FIXED, VM_FLAGS_OVERWRITE, mach_vm_protect, task};

/// `ART_HEAP_REFERENCE_BASE` of the ART exception build.
pub const BASE: u64 = 0x100_0000_0000;
pub const SIZE: u64 = 4 << 30;
const END: u64 = BASE + SIZE;

/// The Darwin VM tag of reserved window pages (an application-specific tag,
/// VM_MEMORY_APPLICATION_SPECIFIC_1 + 3).
pub const TAG: u32 = 243;

unsafe extern "C" {
    fn mach_vm_allocate(task: libc::mach_port_t, address: *mut u64, size: u64, flags: i32) -> i32;
}

/// Reserve `[lo, hi)` (page-aligned, inside the window). With `replace`,
/// whatever is mapped there goes.
fn reserve(lo: u64, hi: u64, replace: bool) -> bool {
    let mut at = lo;
    let flags = VM_FLAGS_FIXED | if replace { VM_FLAGS_OVERWRITE } else { 0 } | (TAG << 24) as i32;
    // SAFETY: allocating (or replacing) guest address space in our own task.
    if unsafe { mach_vm_allocate(task(), &mut at, hi - lo, flags) } != 0 {
        return false;
    }
    // SAFETY: the range we just allocated.
    unsafe { mach_vm_protect(task(), lo, hi - lo, 1, libc::PROT_NONE) == 0 }
}

/// Reserve the whole window; called once per process image, before the
/// program is loaded.
pub fn init() {
    if !reserve(BASE, END, false) {
        crate::diag!("[linux-abi] cannot reserve the heap reference window {BASE:#x}..{END:#x}");
    }
}

fn clip(lo: u64, hi: u64) -> Option<(u64, u64)> {
    let (lo, hi) = (lo.max(BASE), hi.min(END));
    (lo < hi).then_some((lo, hi))
}

/// Whether `[addr, addr+len)` lies inside the window and is all reserved,
/// so a guest mapping may take it.
pub fn is_free(addr: u64, len: u64) -> bool {
    let Some(hi) = addr.checked_add(len) else {
        return false;
    };
    if addr < BASE || hi > END {
        return false;
    }
    let mut cur = addr;
    while cur < hi {
        match vmmap::entry_at(cur) {
            Some(e) if e.tag == TAG && e.start <= cur => cur = e.end,
            _ => return false,
        }
    }
    true
}

/// Whether any page of `[lo, hi)` is reserved (unmapped to the guest).
pub fn touches_reserved(lo: u64, hi: u64) -> bool {
    let Some((lo, hi)) = clip(lo, hi) else {
        return false;
    };
    let mut cur = lo;
    while cur < hi {
        match vmmap::entry_at(cur) {
            Some(e) if e.start < hi => {
                if e.tag == TAG {
                    return true;
                }
                cur = e.end;
            }
            _ => return false,
        }
    }
    false
}

/// Unmap `[addr, addr+len)` for the guest: the part inside the window is
/// reserved again, the rest is deallocated.
pub fn unmap(addr: u64, len: u64) -> i64 {
    let hi = addr.saturating_add(len);
    let Some((wlo, whi)) = clip(addr, hi) else {
        // SAFETY: guest-requested unmap of guest memory.
        return crate::errno::check(unsafe { libc::munmap(addr as *mut _, len as usize) } as i64);
    };
    for (lo, hi) in [(addr, wlo), (whi, hi)] {
        if lo < hi {
            // SAFETY: guest-requested unmap of guest memory outside the window.
            unsafe { libc::munmap(lo as *mut _, (hi - lo) as usize) };
        }
    }
    debug_assert!(wlo % PAGE == 0 && whi % PAGE == 0);
    if reserve(wlo, whi, true) {
        0
    } else {
        -(libc::ENOMEM as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_pages_are_free_to_the_guest_and_come_back_after_unmap() {
        init();
        let at = BASE + (1 << 30);
        assert!(is_free(at, 4 * PAGE));
        assert!(touches_reserved(at, at + PAGE));
        assert!(vmmap::regions(at, at + 4 * PAGE).next().is_none());
        // A guest mapping takes the pages; they are no longer free.
        // SAFETY: mapping over our own reservation.
        let p = unsafe {
            libc::mmap(
                at as *mut _,
                (2 * PAGE) as usize,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_FIXED | libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        assert_eq!(p as u64, at);
        assert!(!is_free(at, 4 * PAGE));
        assert!(!is_free(at + PAGE, PAGE));
        assert!(is_free(at + 2 * PAGE, 2 * PAGE));
        assert!(!touches_reserved(at, at + 2 * PAGE));
        assert_eq!(vmmap::regions(at, at + 4 * PAGE).count(), 1);
        assert_eq!(unmap(at, 2 * PAGE), 0);
        assert!(is_free(at, 4 * PAGE));
        // Outside the window nothing is free.
        assert!(!is_free(BASE - PAGE, 2 * PAGE));
        assert!(!is_free(END - PAGE, 2 * PAGE));

        // Guest mmap: a hint on reserved pages is honoured exactly...
        const RW: u64 = 3;
        const PRIVATE_ANON: u64 = 0x22;
        const NOREPLACE: u64 = 0x10_0000;
        let mmap = |addr, flags| super::super::mem::mmap([addr, 2 * PAGE, RW, flags, u64::MAX, 0]);
        assert_eq!(mmap(at, PRIVATE_ANON), at as i64);
        // ...a taken one is not: MAP_FIXED_NOREPLACE fails, a plain hint goes
        // elsewhere, outside the window.
        assert_eq!(mmap(at, PRIVATE_ANON | NOREPLACE), -(libc::EEXIST as i64));
        let elsewhere = mmap(at, PRIVATE_ANON) as u64;
        assert!(!(BASE..END).contains(&elsewhere));
        // SAFETY: unmapping the mapping made above.
        unsafe { libc::munmap(elsewhere as *mut _, (2 * PAGE) as usize) };
        assert_eq!(
            mmap(at + 2 * PAGE, PRIVATE_ANON | NOREPLACE),
            (at + 2 * PAGE) as i64
        );
        // msync probes see mapped and unmapped pages as on Linux.
        let msync = |addr| super::super::mem::msync([addr, PAGE, 0, 0, 0, 0]);
        assert_eq!(msync(at), 0);
        assert_eq!(msync(at + 4 * PAGE), -(libc::ENOMEM as i64));
        assert_eq!(unmap(at, 4 * PAGE), 0);
        assert_eq!(msync(at), -(libc::ENOMEM as i64));
        assert!(is_free(at, 4 * PAGE));
    }
}
