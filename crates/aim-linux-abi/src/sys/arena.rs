//! The guest's part of the address space.
//!
//! Every mapping the guest can see lies in `[LO, HI)`: its own mmaps, the
//! loaded program and interpreter, the initial stack, the program break,
//! binder buffers, memfd memory and stub islands. The heap reference
//! window (`window`) is inside it too. Host allocations (malloc, host
//! thread stacks, dylibs) are placed by Darwin first-fit from low
//! addresses, and libmalloc's own regions lie between 0x70_0000_0000 and
//! 0x80_0000_0000, so none of them lands here.
//!
//! A guest fork re-creates the guest's mappings in a fresh process at the
//! same addresses (`fork`): the range is known, and the new process's host
//! allocations never collide with it.
//!
//! Darwin's `mmap` takes a non-fixed address as the start of a first-fit
//! search, so a guest mapping without a usable hint is placed with `LO` as
//! its hint. Placing takes [`placing`], so a range unmapped to be mapped
//! again at the same address (`jit`) is not taken meanwhile.

use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::errno::{self, ENOMEM};

/// 512 GiB: above the host's allocations, below the heap reference window.
pub const LO: u64 = 0x80_0000_0000;
/// 8 TiB.
pub const HI: u64 = 0x800_0000_0000;

/// Whether `[addr, addr+len)` lies in the guest range.
pub fn contains(addr: u64, len: u64) -> bool {
    addr >= LO && addr.checked_add(len).is_some_and(|end| end <= HI)
}

/// The placement hint for a non-fixed guest mapping of `len` bytes: the
/// guest's own hint when it lies in the range, else `LO`.
pub fn hint(addr: u64, len: u64) -> u64 {
    if addr != 0 && contains(addr, len) {
        addr
    } else {
        LO
    }
}

static PLACING: RwLock<()> = RwLock::new(());

/// Held while a mapping is placed by a hint.
pub fn placing() -> RwLockReadGuard<'static, ()> {
    PLACING.read().unwrap_or_else(|e| e.into_inner())
}

/// Held while a range is unmapped and mapped again at its address.
pub fn placing_exclusive() -> RwLockWriteGuard<'static, ()> {
    PLACING.write().unwrap_or_else(|e| e.into_inner())
}

/// Map anonymous private memory of `len` bytes in the guest range.
pub fn map_anon(len: u64, prot: i32) -> Result<u64, i64> {
    map(LO, len, prot, libc::MAP_PRIVATE | libc::MAP_ANON, -1, 0)
}

/// `mmap` placed in the guest range, searching from `hint`. `flags` must
/// not contain MAP_FIXED.
pub fn map(hint: u64, len: u64, prot: i32, flags: i32, fd: i32, off: i64) -> Result<u64, i64> {
    let _placing = placing();
    // SAFETY: a fresh mapping; the hint only steers placement.
    let p = unsafe { libc::mmap(hint as *mut _, len as usize, prot, flags, fd, off) };
    if p == libc::MAP_FAILED {
        return Err(-(errno::last() as i64));
    }
    let p = p as u64;
    if !contains(p, len) {
        // SAFETY: unmapping what we just mapped outside the range.
        unsafe { libc::munmap(p as *mut _, len as usize) };
        return Err(-(ENOMEM as i64));
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mappings_land_in_the_range_and_reuse_holes() {
        let a = map_anon(4 * 16384, libc::PROT_READ | libc::PROT_WRITE).unwrap();
        assert!(contains(a, 4 * 16384));
        // SAFETY: our mapping.
        unsafe { libc::munmap(a as *mut _, 4 * 16384) };
        let b = map_anon(16384, libc::PROT_READ).unwrap();
        assert!(b <= a, "{b:#x} is past the hole at {a:#x}");
        // SAFETY: our mapping.
        unsafe { libc::munmap(b as *mut _, 16384) };
        assert_eq!(hint(HI, 16384), LO);
        assert_eq!(hint(0x1_0000_0000, 16384), LO);
        assert_eq!(hint(LO + 16384, 16384), LO + 16384);
    }
}
