//! Read-only typed payload inventory. Never follows symlinks or copies files.
use super::*;
use std::collections::{HashSet, VecDeque};

fn components(path: &str) -> Result<Vec<&str>> {
    if !path.starts_with('/') || path.contains("//") {
        return Err(invalid("inventory path must be absolute and normalized").into());
    }
    let parts: Vec<_> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.iter().any(|part| matches!(*part, "." | "..")) {
        return Err(invalid("invalid inventory path component").into());
    }
    Ok(parts)
}

impl Ext4Image {
    pub(super) fn inventory_link(&mut self, inode: &Inode) -> Result<Option<String>> {
        if inode.mode & 0xf000 != 0xa000 {
            return Ok(None);
        }
        let bytes = if inode.flags & EXT4_EXTENTS_FL == 0 && inode.size <= 60 {
            inode.extent_root[..inode.size as usize].to_vec()
        } else {
            self.read_inode_data(inode, 4096)?
        };
        let target = String::from_utf8(bytes)?;
        if target.is_empty() || target.contains('\0') {
            return Err(invalid("invalid APEX symlink target").into());
        }
        // Preserve Android targets verbatim; never resolve against host paths.
        Ok(Some(target))
    }
    pub(super) fn inventory(&mut self, path: &str) -> Result<Vec<(String, Inode)>> {
        let parts = components(path)?;
        let mut number = 2;
        let mut inode = self.read_inode(number)?;
        for part in &parts {
            number = self.find_directory_entry(&inode, part.as_bytes())?;
            inode = self.read_inode(number)?;
        }
        let start = format!("/{}", parts.join("/"));
        let mut pending = VecDeque::from([(start, number, inode, 0usize)]);
        let mut directories = HashSet::new();
        let mut result = Vec::new();
        while let Some((path, number, inode, depth)) = pending.pop_front() {
            if result.len() + pending.len() >= 100_000 || depth > 64 {
                return Err(invalid("APEX inventory entry/depth limit exceeded").into());
            }
            if inode.is_directory() {
                if !directories.insert(number) {
                    return Err(invalid("repeated/cyclic APEX directory inode").into());
                }
                for name in self.list_directory(&path)? {
                    if result.len() + pending.len() + 1 >= 100_000 {
                        return Err(invalid("APEX inventory entry limit exceeded").into());
                    }
                    // Do not trust directory bytes as a host path component.
                    if name.is_empty() || name.contains('/') || name.contains('\0') {
                        return Err(invalid("invalid APEX inventory entry name").into());
                    }
                    let child_number = self.find_directory_entry(&inode, name.as_bytes())?;
                    let child = self.read_inode(child_number)?;
                    let child_path = format!("{}/{}", path.trim_end_matches('/'), name);
                    pending.push_back((child_path, child_number, child, depth + 1));
                }
            }
            result.push((path, inode));
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalized_paths_only() {
        assert_eq!(components("/").unwrap(), Vec::<&str>::new());
        assert_eq!(components("/etc/config").unwrap(), vec!["etc", "config"]);
        for path in ["relative", "/a//b", "/a/../b", "/./etc"] {
            assert!(components(path).is_err());
        }
    }
}
