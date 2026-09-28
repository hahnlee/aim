//! Preserve the original guest link bytes; never resolve against the host.
use super::*;
use std::os::unix::ffi::OsStrExt;

pub(super) fn extract(fs: &mut Erofs<'_>, inode: &Inode, output: &Path) -> Result<()> {
    if inode.mode & 0xf000 != 0xa000 {
        return Err(invalid("target is not a symbolic link").into());
    }
    let target = fs.flat_data(inode, 4096)?;
    if target.is_empty() || target.contains(&0) {
        return Err(invalid("invalid symbolic link target").into());
    }
    // symlink fails on an existing destination, including dangling links.
    std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(&target), output)?;
    println!(
        "original symlink: {} -> {:?}",
        output.display(),
        std::ffi::OsStr::from_bytes(&target)
    );
    Ok(())
}
