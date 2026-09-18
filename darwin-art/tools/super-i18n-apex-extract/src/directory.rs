use crate::{invalid, le16, le64, Result};

/// Parse flat EROFS directory blocks without copying their file contents.
pub fn entries(data: &[u8], block_size: usize) -> Result<Vec<(u64, Vec<u8>)>> {
    if block_size < 12 {
        return Err(invalid("invalid directory block size").into());
    }
    let mut result = Vec::new();
    for block in data.chunks(block_size) {
        if block.len() < 12 {
            return Err(invalid("truncated directory block").into());
        }
        let names = usize::from(le16(block, 8)?);
        if names == 0 || names % 12 != 0 || names > block.len() {
            return Err(invalid("invalid directory table length").into());
        }
        for index in 0..names / 12 {
            let entry = index * 12;
            let start = usize::from(le16(block, entry + 8)?);
            let end = if entry + 12 == names {
                block.len()
            } else {
                usize::from(le16(block, entry + 20)?)
            };
            if start < names || start > end || end > block.len() {
                return Err(invalid("invalid directory name bounds").into());
            }
            let bytes = &block[start..end];
            let name_end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            let name = &bytes[..name_end];
            if name.is_empty()
                || name.len() > 255
                || name.contains(&b'/')
                || bytes[name_end..].iter().any(|&b| b != 0)
            {
                return Err(invalid("invalid directory name").into());
            }
            result.push((le64(block, entry)?, name.to_vec()));
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_name_and_padding() {
        let mut block = vec![0; 32];
        block[0] = 7;
        block[8] = 12;
        block[12..16].copy_from_slice(b"apex");
        assert_eq!(entries(&block, 4096).unwrap(), vec![(7, b"apex".to_vec())]);
    }
    #[test]
    fn rejects_bad_offsets_and_truncation() {
        assert!(entries(&[0; 11], 4096).is_err());
        let mut block = vec![0; 32];
        block[8] = 36;
        assert!(entries(&block, 4096).is_err());
        block[8] = 13;
        assert!(entries(&block, 4096).is_err());
        assert!(entries(&block, 0).is_err());
    }
}
