//! Decode validated 64-bit Binder UAPI objects without aligned pointer casts.
//! Values remain sender-scoped claims, not authorized nodes, handles or host FDs.
use crate::objects::{Kind, Object};

#[derive(Debug, PartialEq, Eq)]
pub enum Fields {
    Node {
        weak: bool,
        flags: u32,
        pointer: u64,
        cookie: u64,
    },
    Handle {
        weak: bool,
        flags: u32,
        handle: u32,
        cookie: u64,
    },
    Fd {
        fd: u32,
        cookie: u64,
    },
    Buffer {
        flags: u32,
        pointer: u64,
        length: u64,
        parent: u64,
        parent_offset: u64,
    },
    FdArray {
        count: u64,
        parent: u64,
        parent_offset: u64,
    },
}

impl Object<'_> {
    pub fn fields(&self) -> Fields {
        let bytes = self.bytes();
        // Object construction already checked the full type-dependent size.
        let word = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        let wide = |offset| u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        match self.kind() {
            Kind::Binder | Kind::WeakBinder => Fields::Node {
                weak: self.kind() == Kind::WeakBinder,
                flags: word(4),
                pointer: wide(8),
                cookie: wide(16),
            },
            Kind::Handle | Kind::WeakHandle => Fields::Handle {
                weak: self.kind() == Kind::WeakHandle,
                flags: word(4),
                handle: word(8),
                cookie: wide(16),
            },
            Kind::Fd => Fields::Fd {
                fd: word(8),
                cookie: wide(16),
            },
            Kind::Buffer => Fields::Buffer {
                flags: word(4),
                pointer: wide(8),
                length: wide(16),
                parent: wide(24),
                parent_offset: wide(32),
            },
            Kind::FdArray => Fields::FdArray {
                count: wide(8),
                parent: wide(16),
                parent_offset: wide(24),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn narrow_union_members_ignore_high_padding_without_dereferencing() {
        for kind in [Kind::Handle, Kind::WeakHandle, Kind::Fd] {
            let mut bytes = [0xff; 28];
            bytes[4..8].copy_from_slice(&kind.tag().to_le_bytes());
            bytes[12..16].copy_from_slice(&42u32.to_le_bytes());
            let offsets = 4u64.to_le_bytes();
            let objects = crate::objects::validate(&bytes, &offsets).unwrap();
            let expected = if kind == Kind::Fd {
                Fields::Fd {
                    fd: 42,
                    cookie: u64::MAX,
                }
            } else {
                Fields::Handle {
                    weak: kind == Kind::WeakHandle,
                    flags: u32::MAX,
                    handle: 42,
                    cookie: u64::MAX,
                }
            };
            assert_eq!(objects[0].fields(), expected);
        }
    }
}
