#![forbid(unsafe_code)]

//! Stable, pointer-free pieces of the aim boundary.
//!
//! Raw pointers and implementation-specific C++ types belong in
//! `aim-engine-sys`; this crate is safe to depend on from the Rust
//! runtime and provider crates.

use core::fmt;

pub const ABI_VERSION: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AbiHeader {
    pub struct_size: u32,
    pub abi_version: u32,
}

impl AbiHeader {
    pub const fn new(struct_size: usize) -> Self {
        Self {
            struct_size: struct_size as u32,
            abi_version: ABI_VERSION,
        }
    }

    pub const fn accepts(self, minimum_size: usize) -> bool {
        self.abi_version == ABI_VERSION && self.struct_size as usize >= minimum_size
    }
}

/// Validate the flattened `(struct_size, abi_version)` prefix used by the C
/// ABI structs.  The wire layout intentionally stays flat for C callers while
/// the rule itself lives in one Rust crate.
pub const fn accepts_header_fields(
    struct_size: u32,
    abi_version: u32,
    minimum_size: usize,
) -> bool {
    AbiHeader {
        struct_size,
        abi_version,
    }
    .accepts(minimum_size)
}

#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusCode {
    Ok = 0,
    InvalidArgument = -1,
    InvalidState = -2,
    NotFound = -3,
    Busy = -4,
    Unsupported = -5,
    Internal = -6,
}

impl StatusCode {
    pub const fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }
}

#[repr(transparent)]
#[derive(Clone, Copy, Default, Eq, Hash, PartialEq)]
pub struct OpaqueHandle(u64);

impl OpaqueHandle {
    pub const INVALID: Self = Self(0);

    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn is_valid(self) -> bool {
        self.0 != 0
    }
}

impl fmt::Debug for OpaqueHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("OpaqueHandle").field(&self.0).finish()
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallbackResult {
    pub status: i32,
    pub detail: i32,
}

impl CallbackResult {
    pub const fn ok() -> Self {
        Self {
            status: StatusCode::Ok as i32,
            detail: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_rejects_old_or_unknown_layouts() {
        assert!(
            AbiHeader::new(core::mem::size_of::<AbiHeader>())
                .accepts(core::mem::size_of::<AbiHeader>())
        );
        assert!(
            !AbiHeader {
                struct_size: 64,
                abi_version: 2,
            }
            .accepts(8)
        );
        assert!(
            !AbiHeader {
                struct_size: 4,
                abi_version: ABI_VERSION,
            }
            .accepts(8)
        );
    }

    #[test]
    fn opaque_zero_is_the_only_invalid_handle() {
        assert!(!OpaqueHandle::INVALID.is_valid());
        assert!(OpaqueHandle::from_raw(1).is_valid());
    }

    #[test]
    fn c_layout_is_explicit() {
        assert_eq!(core::mem::size_of::<AbiHeader>(), 8);
        assert_eq!(core::mem::size_of::<CallbackResult>(), 8);
        assert_eq!(core::mem::size_of::<OpaqueHandle>(), 8);
    }
}
