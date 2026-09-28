//! Android APS2 packed RELA decoding.
//!
//! The format is the signed-LEB128 delta stream consumed by Bionic's
//! `for_all_packed_relocs`.  Keep decoding separate from mapped-image writes:
//! malformed input is rejected completely before any relocation is applied.

const APS2_MAGIC: &[u8; 4] = b"APS2";
const GROUPED_BY_INFO: u64 = 1;
const GROUPED_BY_OFFSET_DELTA: u64 = 2;
const GROUPED_BY_ADDEND: u64 = 4;
const GROUP_HAS_ADDEND: u64 = 8;
const KNOWN_GROUP_FLAGS: u64 =
    GROUPED_BY_INFO | GROUPED_BY_OFFSET_DELTA | GROUPED_BY_ADDEND | GROUP_HAS_ADDEND;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PackedRela {
    pub offset: u64,
    pub info: u64,
    pub addend: i64,
}

pub(super) fn decode_aps2_rela(bytes: &[u8]) -> Result<Vec<PackedRela>, &'static str> {
    let payload = bytes
        .strip_prefix(APS2_MAGIC)
        .ok_or("Android packed RELA lacks APS2 magic")?;
    let mut decoder = Sleb128::new(payload);
    let relocation_count = decoder.next_nonnegative("negative packed relocation count")?;
    let capacity = usize::try_from(relocation_count)
        .map_err(|_| "packed relocation count does not fit host")?;
    let mut relocations = Vec::with_capacity(capacity);
    let mut offset = decoder.next_nonnegative("negative initial relocation offset")?;
    let mut info = 0_u64;
    let mut addend = 0_i64;

    while relocations.len() < capacity {
        let group_size = decoder.next_nonnegative("negative relocation group size")?;
        let group_size =
            usize::try_from(group_size).map_err(|_| "relocation group size does not fit host")?;
        if group_size == 0 || group_size > capacity - relocations.len() {
            return Err("invalid relocation group size");
        }
        let group_flags = decoder.next_nonnegative("negative relocation group flags")?;
        if group_flags & !KNOWN_GROUP_FLAGS != 0
            || group_flags & GROUPED_BY_ADDEND != 0 && group_flags & GROUP_HAS_ADDEND == 0
        {
            return Err("invalid relocation group flags");
        }
        let grouped_offset_delta = if group_flags & GROUPED_BY_OFFSET_DELTA != 0 {
            decoder.next()?
        } else {
            0
        };
        if group_flags & GROUPED_BY_INFO != 0 {
            info = decoder.next_nonnegative("negative grouped relocation info")?;
        }
        let has_addend = group_flags & GROUP_HAS_ADDEND != 0;
        let grouped_addend = has_addend && group_flags & GROUPED_BY_ADDEND != 0;
        if grouped_addend {
            addend = addend
                .checked_add(decoder.next()?)
                .ok_or("grouped relocation addend overflow")?;
        } else if !has_addend {
            addend = 0;
        }

        for _ in 0..group_size {
            let delta = if group_flags & GROUPED_BY_OFFSET_DELTA != 0 {
                grouped_offset_delta
            } else {
                decoder.next()?
            };
            offset = offset
                .checked_add_signed(delta)
                .ok_or("relocation offset underflow or overflow")?;
            if group_flags & GROUPED_BY_INFO == 0 {
                info = decoder.next_nonnegative("negative relocation info")?;
            }
            if has_addend && !grouped_addend {
                addend = addend
                    .checked_add(decoder.next()?)
                    .ok_or("relocation addend overflow")?;
            }
            relocations.push(PackedRela {
                offset,
                info,
                addend: if has_addend { addend } else { 0 },
            });
        }
    }
    if !decoder.is_empty() {
        return Err("trailing bytes in Android packed RELA");
    }
    Ok(relocations)
}

struct Sleb128<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Sleb128<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn is_empty(&self) -> bool {
        self.position == self.bytes.len()
    }

    fn next_nonnegative(&mut self, error: &'static str) -> Result<u64, &'static str> {
        u64::try_from(self.next()?).map_err(|_| error)
    }

    fn next(&mut self) -> Result<i64, &'static str> {
        let mut value = 0_u64;
        let mut shift = 0_u32;
        for index in 0..10 {
            let byte = *self
                .bytes
                .get(self.position)
                .ok_or("truncated signed LEB128")?;
            self.position += 1;
            let payload = u64::from(byte & 0x7f);
            if shift == 63 && payload & !1 != 0 {
                return Err("signed LEB128 overflow");
            }
            value |= payload << shift;
            shift += 7;
            if byte & 0x80 == 0 {
                if shift < 64 && byte & 0x40 != 0 {
                    value |= u64::MAX << shift;
                }
                return Ok(value as i64);
            }
            if index == 9 {
                return Err("signed LEB128 exceeds 10 bytes");
            }
        }
        unreachable!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sleb(mut value: i64) -> Vec<u8> {
        let mut bytes = Vec::new();
        loop {
            let byte = (value as u8) & 0x7f;
            let next = value >> 7;
            let done = (next == 0 && byte & 0x40 == 0) || (next == -1 && byte & 0x40 != 0);
            bytes.push(byte | if done { 0 } else { 0x80 });
            value = next;
            if done {
                return bytes;
            }
        }
    }

    #[test]
    fn decodes_grouped_and_delta_addends() {
        let mut bytes = APS2_MAGIC.to_vec();
        for value in [3, 0, 2, 11, 8, 1027, 4, 0, 1, 8, 8, 257, -7] {
            bytes.extend(sleb(value));
        }
        assert_eq!(
            decode_aps2_rela(&bytes),
            Ok(vec![
                PackedRela {
                    offset: 8,
                    info: 1027,
                    addend: 4
                },
                PackedRela {
                    offset: 16,
                    info: 1027,
                    addend: 4
                },
                PackedRela {
                    offset: 24,
                    info: 257,
                    addend: -3
                },
            ])
        );
    }

    #[test]
    fn rejects_bad_magic_and_trailing_data() {
        assert!(decode_aps2_rela(b"APS1").is_err());
        let mut bytes = APS2_MAGIC.to_vec();
        bytes.extend([0, 0, 0]);
        assert!(decode_aps2_rela(&bytes).is_err());
    }

    #[test]
    fn accepts_negative_offset_deltas_between_info_groups() {
        let mut bytes = APS2_MAGIC.to_vec();
        for value in [2, 16, 1, GROUPED_BY_INFO as i64, 257, 8, 1, 1, 513, -4] {
            bytes.extend(sleb(value));
        }
        assert_eq!(
            decode_aps2_rela(&bytes),
            Ok(vec![
                PackedRela {
                    offset: 24,
                    info: 257,
                    addend: 0,
                },
                PackedRela {
                    offset: 20,
                    info: 513,
                    addend: 0,
                },
            ])
        );
    }
}
