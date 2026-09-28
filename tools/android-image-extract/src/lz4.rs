//! LZ4 block decoding (EROFS pclusters) and the legacy LZ4 frame format
//! (`lz4 -l`, used by Android ramdisks).
use crate::{Result, invalid, le16, le32};

pub const LEGACY_MAGIC: u32 = 0x184c_2102;
const LEGACY_BLOCK: usize = 8 * 1024 * 1024;

/// Decode one raw LZ4 block, appending exactly `expected` bytes to `out`.
pub fn decode_block_into(input: &[u8], expected: usize, out: &mut Vec<u8>) -> Result<()> {
    decode(input, expected, true, out)
}

/// Decode one raw LZ4 block, appending at most `expected` bytes (exactly, if
/// `exact`) to `out`. The whole input must be consumed.
fn decode(input: &[u8], expected: usize, exact: bool, out: &mut Vec<u8>) -> Result<()> {
    let base = out.len();
    let limit = base + expected;
    let mut cursor = 0usize;
    let length = |cursor: &mut usize, mut value: usize| -> Result<usize> {
        if value == 15 {
            loop {
                let byte = *input
                    .get(*cursor)
                    .ok_or_else(|| invalid("truncated LZ4 length"))?;
                *cursor += 1;
                value += usize::from(byte);
                if byte != 255 {
                    break;
                }
            }
        }
        Ok(value)
    };
    while cursor < input.len() {
        let token = input[cursor];
        cursor += 1;
        let literals = length(&mut cursor, usize::from(token >> 4))?;
        let literal_end = cursor
            .checked_add(literals)
            .filter(|end| *end <= input.len())
            .ok_or_else(|| invalid("LZ4 literals overrun the input"))?;
        if out.len() + literals > limit {
            return Err(invalid("LZ4 literals overrun the output"));
        }
        out.extend_from_slice(&input[cursor..literal_end]);
        cursor = literal_end;
        if cursor == input.len() {
            break;
        }
        let offset = usize::from(le16(input, cursor)?);
        cursor += 2;
        if offset == 0 || offset > out.len() - base {
            return Err(invalid("invalid LZ4 match offset"));
        }
        let matched = length(&mut cursor, usize::from(token & 15))? + 4;
        if out.len() + matched > limit {
            return Err(invalid("LZ4 match overruns the output"));
        }
        let start = out.len() - offset;
        if offset >= matched {
            out.extend_from_within(start..start + matched);
        } else {
            for i in 0..matched {
                let byte = out[start + i];
                out.push(byte);
            }
        }
    }
    if exact && out.len() != limit {
        return Err(invalid(format!(
            "LZ4 block decoded to {} bytes, expected {expected}",
            out.len() - base
        )));
    }
    Ok(())
}

pub fn decode_block(input: &[u8], expected: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(expected);
    decode_block_into(input, expected, &mut out)?;
    Ok(out)
}

/// Decode concatenated legacy LZ4 frames. Each frame is the magic followed by
/// (le32 compressed size, block) pairs; every block but the last decodes to
/// 8 MiB. Trailing zero padding is accepted.
pub fn decode_legacy(input: &[u8], limit: usize) -> Result<Vec<u8>> {
    if le32(input, 0)? != LEGACY_MAGIC {
        return Err(invalid("not a legacy LZ4 stream"));
    }
    let mut out = Vec::new();
    let mut cursor = 4usize;
    while cursor + 4 <= input.len() {
        let size = le32(input, cursor)?;
        cursor += 4;
        if size == LEGACY_MAGIC {
            continue;
        }
        if size == 0 {
            if input[cursor..].iter().all(|b| *b == 0) {
                break;
            }
            return Err(invalid("empty legacy LZ4 block"));
        }
        let block = input
            .get(cursor..cursor + size as usize)
            .ok_or_else(|| invalid("truncated legacy LZ4 block"))?;
        cursor += size as usize;
        // Decoded block sizes are not recorded; each is at most 8 MiB.
        decode(block, LEGACY_BLOCK, false, &mut out)?;
        if out.len() > limit {
            return Err(invalid("legacy LZ4 output exceeds its limit"));
        }
    }
    if cursor != input.len() && !input[cursor..].iter().all(|b| *b == 0) {
        return Err(invalid("trailing bytes after legacy LZ4 stream"));
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Encode `data` as a single literal-only LZ4 sequence.
    pub(crate) fn literal_block(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        if data.len() < 15 {
            out.push((data.len() as u8) << 4);
        } else {
            out.push(0xf0);
            let mut rest = data.len() - 15;
            while rest >= 255 {
                out.push(255);
                rest -= 255;
            }
            out.push(rest as u8);
        }
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn decodes_literals_and_overlapping_matches() {
        assert_eq!(decode_block(b"\x30abc", 3).unwrap(), b"abc");
        // "ab" then a match of 6 at offset 2 (overlapping), then literal "!".
        let block = [0x22, b'a', b'b', 2, 0, 0x10, b'!'];
        assert_eq!(decode_block(&block, 9).unwrap(), b"abababab!");
        let long = vec![7u8; 300];
        assert_eq!(decode_block(&literal_block(&long), 300).unwrap(), long);
    }

    #[test]
    fn rejects_malformed_blocks() {
        assert!(decode_block(b"\x10", 1).is_err());
        assert!(decode_block(b"\x00\x00\x00", 4).is_err());
        assert!(decode_block(b"\x40abcd", 3).is_err());
        assert!(decode_block(b"\x30abc", 4).is_err());
    }

    #[test]
    fn decodes_legacy_frames() {
        let mut stream = LEGACY_MAGIC.to_le_bytes().to_vec();
        for chunk in [&b"first "[..], &b"second"[..]] {
            let block = literal_block(chunk);
            stream.extend_from_slice(&(block.len() as u32).to_le_bytes());
            stream.extend_from_slice(&block);
        }
        stream.extend_from_slice(&[0; 8]);
        assert_eq!(decode_legacy(&stream, 1 << 20).unwrap(), b"first second");
        assert!(decode_legacy(&stream[..stream.len() - 12], 1 << 20).is_err());
        assert!(decode_legacy(b"\0\0\0\0", 1).is_err());
    }
}
