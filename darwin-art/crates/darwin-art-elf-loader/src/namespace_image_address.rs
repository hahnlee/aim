//! Original-image address membership, matching Bionic find_containing_library.
//! Reservation holes, adjacent images and retained dependencies are not callers
//! of this selected image. No memory at the supplied address is dereferenced.
use super::GlobalElfImage;

fn virtual_address(address: usize, base: usize, size: usize, minimum: u64) -> Option<u64> {
    // Android arm64 permits a top-byte tag on data pointers.
    #[cfg(target_arch = "aarch64")]
    let address = address & 0x00ff_ffff_ffff_ffff;
    let offset = address.checked_sub(base)?;
    if offset >= size {
        return None;
    }
    minimum.checked_add(offset as u64)
}

fn segment_contains(address: u64, start: u64, size: u64) -> bool {
    address
        .checked_sub(start)
        .is_some_and(|offset| offset < size)
}

fn executable_segment_range(
    address: usize,
    base: usize,
    mapping_size: usize,
    minimum_page: u64,
    loads: impl IntoIterator<Item = (u64, u64, u32)>,
) -> Option<std::ops::Range<usize>> {
    let virtual_address = virtual_address(address, base, mapping_size, minimum_page)?;
    let mapping_end = base.checked_add(mapping_size)?;
    loads.into_iter().find_map(|(start, size, flags)| {
        if flags & crate::PF_X == 0 || !segment_contains(virtual_address, start, size) {
            return None;
        }
        // The parser validates this arithmetic, but keep the public address
        // projection independently checked at the FFI boundary.
        let start_offset = start.checked_sub(minimum_page)?;
        let host_start = base.checked_add(usize::try_from(start_offset).ok()?)?;
        let host_end = host_start.checked_add(usize::try_from(size).ok()?)?;
        (host_start >= base && host_end <= mapping_end).then_some(host_start..host_end)
    })
}

impl GlobalElfImage {
    /// Actual PT_LOAD memory coverage in this original mapped image, not the
    /// graph's union or page-rounded mapping. This does not grant visibility.
    pub fn contains_address(&self, address: usize) -> bool {
        let image = self.mapping.image();
        let Some(vaddr) = virtual_address(
            address,
            image.mapping.as_ptr() as usize,
            image.mapping_size,
            image.minimum_page,
        ) else {
            return false;
        };
        image.loads.iter().any(|load| {
            load.kind == crate::PT_LOAD
                && segment_contains(vaddr, load.virtual_address, load.memory_size)
        })
    }

    /// Return the exact host half-open range of the executable PT_LOAD that
    /// contains `address`. This is selected-image identity only; no mapping
    /// lookup or OS VM inspection is performed.
    pub(crate) fn executable_range_containing(
        &self,
        address: usize,
    ) -> Option<std::ops::Range<usize>> {
        let image = self.mapping.image();
        executable_segment_range(
            address,
            image.mapping.as_ptr() as usize,
            image.mapping_size,
            image.minimum_page,
            image
                .loads
                .iter()
                .filter(|load| load.kind == crate::PT_LOAD)
                .map(|load| (load.virtual_address, load.memory_size, load.flags)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn load_membership_excludes_rounding_holes_and_end() {
        for (address, expected) in [
            (0xfff, false),
            (0x1000, true),
            (0x10ff, true),
            (0x1100, false),
            (0x1fff, false),
            (0x2000, true),
            (0x2010, false),
            (0x3000, false),
        ] {
            let found = virtual_address(address, 0x1000, 0x2000, 0x4000).is_some_and(|v| {
                segment_contains(v, 0x4000, 0x100) || segment_contains(v, 0x5000, 0x10)
            });
            assert_eq!(found, expected);
        }
        assert_eq!(virtual_address(1, 0, 2, u64::MAX), None);
        assert!(!segment_contains(9, 10, 5));
        assert!(!segment_contains(10, 10, 0));
    }

    #[test]
    fn executable_range_excludes_data_holes_and_mapping_edges() {
        let loads = [
            (0x4000, 0x100, crate::PF_R | crate::PF_X),
            (0x5000, 0x100, crate::PF_R),
        ];
        let range = |address| executable_segment_range(address, 0x1000, 0x2000, 0x4000, loads);
        assert_eq!(range(0x1000), Some(0x1000..0x1100));
        assert_eq!(range(0x10ff), Some(0x1000..0x1100));
        assert_eq!(range(0x1100), None);
        assert_eq!(range(0x2000), None);
        assert_eq!(range(0x2100), None);
    }

    #[test]
    fn executable_range_rejects_checked_projection_overflow() {
        let loads = [(0x4000, 8, crate::PF_X)];
        assert_eq!(
            executable_segment_range(usize::MAX - 3, usize::MAX - 3, 4, 0x4000, loads),
            None
        );
        assert_eq!(
            executable_segment_range(0x1000, 0x1000, 0x1000, 0x5000, loads),
            None
        );
    }
    #[test]
    #[cfg(target_arch = "aarch64")]
    fn arm64_data_pointer_tag_does_not_change_membership() {
        assert_eq!(
            virtual_address(0xab00_0000_0000_1010, 0x1000, 0x100, 0),
            Some(0x10)
        );
    }
}
