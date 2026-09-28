//! Owned virtual-memory reservation for one loaded ELF image.
//!
//! All host `mmap`/`mprotect` operations used while staging an image live in
//! this module. The parent loader may copy and later intentionally forget the
//! reservation only after it has transferred the exact pointer/length into
//! `LoadedElf`; on every earlier error this owner unmaps automatically.

use super::{
    LoadError, MAP_ANON, MAP_PRIVATE, PROT_NONE, c_int, mmap, mprotect, munmap, ptr, system_error,
};
use std::ptr::NonNull;

pub(super) struct Mapping {
    pointer: NonNull<u8>,
    length: usize,
}

impl Mapping {
    pub(super) fn reserve(length: usize) -> Result<Self, LoadError> {
        let pointer = unsafe {
            mmap(
                ptr::null_mut(),
                length,
                PROT_NONE,
                MAP_PRIVATE | MAP_ANON,
                -1,
                0,
            )
        };
        if pointer as usize == usize::MAX {
            return Err(system_error("mmap(reserve)"));
        }
        let pointer = NonNull::new(pointer.cast::<u8>()).ok_or(LoadError::System {
            operation: "mmap(reserve)",
            code: 0,
        })?;
        Ok(Self { pointer, length })
    }

    pub(super) fn pointer(&self) -> NonNull<u8> {
        self.pointer
    }

    pub(super) fn length(&self) -> usize {
        self.length
    }

    pub(super) fn protect(
        &self,
        offset: usize,
        length: usize,
        protection: c_int,
    ) -> Result<(), LoadError> {
        let end = offset
            .checked_add(length)
            .ok_or(LoadError::Bounds("mprotect overflow"))?;
        if end > self.length {
            return Err(LoadError::Bounds("mprotect outside reservation"));
        }
        let pointer = unsafe { self.pointer.as_ptr().add(offset) };
        if unsafe { mprotect(pointer.cast(), length, protection) } != 0 {
            return Err(system_error("mprotect(staging)"));
        }
        Ok(())
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe { munmap(self.pointer.as_ptr().cast(), self.length) };
    }
}
