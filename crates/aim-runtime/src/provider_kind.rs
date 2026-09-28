//! Closed provider-kind vocabulary at the native ABI boundary.
//!
//! The C ABI carries a raw `u32`, but the runtime lease state machine never
//! accepts an arbitrary number. Keeping this conversion separate from the
//! mutex/condition-variable implementation prevents provider manifest edits
//! from changing the concurrency algorithm.

/// Provider identifiers accepted by the Rust ownership boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ProviderKind {
    Filesystem = 1,
    Network = 2,
    Stdio = 3,
    Ioctl = 4,
    Strftime = 5,
    Sendfile = 6,
    Vm = 7,
}

impl ProviderKind {
    pub const fn raw(self) -> u32 {
        self as u32
    }

    pub(crate) const fn index(self) -> usize {
        self as usize
    }
}

impl TryFrom<u32> for ProviderKind {
    type Error = ();

    fn try_from(raw: u32) -> Result<Self, Self::Error> {
        match raw {
            1 => Ok(Self::Filesystem),
            2 => Ok(Self::Network),
            3 => Ok(Self::Stdio),
            4 => Ok(Self::Ioctl),
            5 => Ok(Self::Strftime),
            6 => Ok(Self::Sendfile),
            7 => Ok(Self::Vm),
            _ => Err(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ProviderKind;

    #[test]
    fn abi_vocabulary_is_closed() {
        assert_eq!(ProviderKind::Filesystem.raw(), 1);
        assert_eq!(ProviderKind::Sendfile.raw(), 6);
        assert!(ProviderKind::try_from(0).is_err());
        assert_eq!(ProviderKind::Vm.raw(), 7);
        assert!(ProviderKind::try_from(8).is_err());
    }
}
