//! Where a stored (uncompressed) member of a zip file lies: APKs keep
//! their native libraries stored and page-aligned, and the guest linker
//! maps them straight from the APK (`android_dlopen_ext` with a
//! `base.apk!/lib/...` path). Only what finding such a member needs is
//! read: the end of central directory record and the central and local
//! headers (APPNOTE.TXT sections 4.3.6, 4.3.7, 4.3.12, 4.3.16; Zip64 is not
//! read, as APKs do not use it).

/// A member: where its local header is, and its size and method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Member {
    header: u64,
    size: u64,
    stored: bool,
}

fn u16_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as u64)
}

fn u32_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?) as u64)
}

/// The members of a zip file of `size` bytes, sorted by header offset.
/// `read(offset, len)` reads that much of the file.
pub fn members(size: u64, read: impl Fn(u64, usize) -> Option<Vec<u8>>) -> Option<Vec<Member>> {
    // The end record is the last 22 bytes plus a comment of up to 64 KiB.
    let tail_len = size.min(22 + 0xffff);
    let tail = read(size - tail_len, tail_len as usize)?;
    let end = (0..=tail.len().checked_sub(22)?)
        .rev()
        .find(|&i| tail[i..i + 4] == *b"PK\x05\x06")?;
    let count = u16_at(&tail, end + 10)? as usize;
    let dir_len = u32_at(&tail, end + 12)?;
    let dir_at = u32_at(&tail, end + 16)?;
    let dir = read(dir_at, dir_len as usize)?;
    let mut out = Vec::with_capacity(count);
    let mut at = 0;
    for _ in 0..count {
        if dir.get(at..at + 4)? != b"PK\x01\x02" {
            return None;
        }
        out.push(Member {
            stored: u16_at(&dir, at + 10)? == 0,
            size: u32_at(&dir, at + 20)?,
            header: u32_at(&dir, at + 42)?,
        });
        at += 46
            + (u16_at(&dir, at + 28)? + u16_at(&dir, at + 30)? + u16_at(&dir, at + 32)?) as usize;
    }
    out.sort_unstable_by_key(|m| m.header);
    Some(out)
}

/// The data of the stored member that holds file offset `off`, as
/// `(start, len)`.
pub fn stored_at(
    members: &[Member],
    off: u64,
    read: impl Fn(u64, usize) -> Option<Vec<u8>>,
) -> Option<(u64, u64)> {
    let i = members
        .partition_point(|m| m.header <= off)
        .checked_sub(1)?;
    let m = members[i];
    if !m.stored {
        return None;
    }
    let local = read(m.header, 30)?;
    if local[..4] != *b"PK\x03\x04" {
        return None;
    }
    let start = m.header + 30 + u16_at(&local, 26)? + u16_at(&local, 28)?;
    (start <= off && off < start + m.size).then_some((start, m.size))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zip of `(name, data, stored)` members, with `pad` bytes of extra
    /// field in each local header (as zipalign adds).
    fn zip(members: &[(&str, &[u8], bool)], pad: usize) -> Vec<u8> {
        let (mut out, mut dir) = (Vec::new(), Vec::new());
        for &(name, data, stored) in members {
            let header = out.len() as u32;
            let method: u16 = if stored { 0 } else { 8 };
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&[20, 0, 0, 0]);
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 8]);
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&(pad as u16).to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend(std::iter::repeat_n(0, pad));
            out.extend_from_slice(data);
            dir.extend_from_slice(b"PK\x01\x02");
            dir.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
            dir.extend_from_slice(&method.to_le_bytes());
            dir.extend_from_slice(&[0; 8]);
            dir.extend_from_slice(&(data.len() as u32).to_le_bytes());
            dir.extend_from_slice(&(data.len() as u32).to_le_bytes());
            dir.extend_from_slice(&(name.len() as u16).to_le_bytes());
            dir.extend_from_slice(&[0; 12]);
            dir.extend_from_slice(&header.to_le_bytes());
            dir.extend_from_slice(name.as_bytes());
        }
        let dir_at = out.len() as u32;
        out.extend_from_slice(&dir);
        out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        out.extend_from_slice(&(members.len() as u16).to_le_bytes());
        out.extend_from_slice(&(members.len() as u16).to_le_bytes());
        out.extend_from_slice(&(dir.len() as u32).to_le_bytes());
        out.extend_from_slice(&dir_at.to_le_bytes());
        out.extend_from_slice(&3u16.to_le_bytes());
        out.extend_from_slice(b"end");
        out
    }

    #[test]
    fn finds_stored_members() {
        let z = zip(
            &[
                ("classes.dex", b"dex\n035", false),
                ("lib/arm64-v8a/liba.so", b"\x7fELF-a", true),
                ("lib/arm64-v8a/libb.so", b"\x7fELF-bb", true),
            ],
            5,
        );
        let read =
            |off: u64, len: usize| z.get(off as usize..off as usize + len).map(<[u8]>::to_vec);
        let m = members(z.len() as u64, read).unwrap();
        assert_eq!(m.len(), 3);
        let b = z.windows(7).position(|w| w == b"\x7fELF-bb").unwrap() as u64;
        assert_eq!(stored_at(&m, b + 3, read), Some((b, 7)));
        let a = z.windows(6).position(|w| w == b"\x7fELF-a").unwrap() as u64;
        assert_eq!(stored_at(&m, a, read), Some((a, 6)));
        // A compressed member, a local header and the directory hold none.
        assert_eq!(stored_at(&m, 32, read), None);
        assert_eq!(stored_at(&m, a - 2, read), None);
        assert_eq!(stored_at(&m, z.len() as u64 - 30, read), None);
    }
}
