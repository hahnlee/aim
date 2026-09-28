//! Offline payload materialization, not APEX activation or executable policy.
use super::*;
use std::fs::{self, DirBuilder, Permissions};
use std::os::unix::fs::{symlink, DirBuilderExt, PermissionsExt};

impl Ext4Image {
    pub(super) fn extract_tree(&mut self, destination: &Path) -> Result<()> {
        let entries = self.inventory("/")?;
        // Validate the whole layout before creating anything. Links are created
        // last, so Android absolute targets are never used for host writes.
        for (_, inode) in &entries {
            match inode.mode & 0xf000 {
                0x4000 | 0x8000 => {}
                0xa000 => {
                    self.inventory_link(inode)?;
                }
                _ => return Err(invalid("unsupported APEX inode type").into()),
            }
        }
        DirBuilder::new().mode(0o700).create(destination)?;
        for (path, inode) in &entries {
            let output = destination.join(path.trim_start_matches('/'));
            if inode.is_directory() {
                if path != "/" {
                    DirBuilder::new().mode(0o700).create(&output)?;
                }
            } else if inode.is_regular() {
                let bytes = self.read_inode_data(inode, MAX_EXTRACTED_BYTES)?;
                write_new_file(&output, &bytes)?;
                fs::set_permissions(
                    &output,
                    Permissions::from_mode(u32::from(inode.mode & 0o777)),
                )?;
            }
        }
        for (path, inode) in &entries {
            if let Some(target) = self.inventory_link(inode)? {
                symlink(target, destination.join(path.trim_start_matches('/')))?;
            }
        }
        // Keep parents writable until all descendants exist. Do not install
        // guest setuid/setgid bits into the host filesystem.
        for (path, inode) in entries.iter().rev() {
            if inode.is_directory() {
                fs::set_permissions(
                    destination.join(path.trim_start_matches('/')),
                    Permissions::from_mode(u32::from(inode.mode & 0o777)),
                )?;
            }
        }
        Ok(())
    }
}
