//! The guest's tzdata (`/apex/com.android.tzdata`), as bionic and libcore
//! resolve zone names in it.

/// Where the image has it.
pub const PATH: &str = "/apex/com.android.tzdata/etc/tz/tzdata";

/// Whether bionic's tzdata file (`ZoneInfoDb`: a 12-byte version, the
/// index, data and final offsets, then 52-byte entries of a 40-byte name
/// and three integers) has a zone called `name`.
pub fn has(tzdata: &[u8], name: &str) -> bool {
    let be = |at: usize| -> Option<usize> {
        Some(u32::from_be_bytes(tzdata.get(at..at + 4)?.try_into().ok()?) as usize)
    };
    if !tzdata.starts_with(b"tzdata") || name.len() >= 40 {
        return false;
    }
    let (Some(index), Some(data)) = (be(12), be(16)) else {
        return false;
    };
    let Some(entries) = tzdata.get(index..data.min(tzdata.len())) else {
        return false;
    };
    entries.chunks_exact(52).any(|entry| {
        let stored = &entry[..40];
        let end = stored.iter().position(|&b| b == 0).unwrap_or(40);
        &stored[..end] == name.as_bytes()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tzdata(names: &[&str]) -> Vec<u8> {
        let mut out = b"tzdata2025a\0".to_vec();
        let index = 24u32;
        let data = index + 52 * names.len() as u32;
        for offset in [index, data, data] {
            out.extend(offset.to_be_bytes());
        }
        for name in names {
            let mut entry = [0u8; 52];
            entry[..name.len()].copy_from_slice(name.as_bytes());
            out.extend(entry);
        }
        out
    }

    #[test]
    fn tzdata_index_lookup() {
        let data = tzdata(&[
            "Africa/Abidjan",
            "Asia/Seoul",
            "America/Argentina/ComodRivadavia",
        ]);
        assert!(has(&data, "Asia/Seoul"));
        assert!(has(&data, "America/Argentina/ComodRivadavia"));
        assert!(!has(&data, "Asia/Seo"));
        assert!(!has(&data, "Mars/Olympus"));
        assert!(!has(b"nottzdata", "Asia/Seoul"));
        assert!(!has(&data[..30], "Asia/Seoul"));
    }
}
