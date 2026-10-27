//! Unified cgroup core on a kernel without v2 resource controllers. CPU,
//! cpuset and I/O are bound to v1. `cgroup.freeze` is deliberately absent:
//! Darwin task suspension also stops the in-process control agent, and
//! thread suspension cannot preserve Linux freezer wakeup/exit semantics.
//! As with CONFIG_CGROUP_FREEZER=n, opening it returns ENOENT.

use crate::cgroup_v1::File;
use std::path::{Path, PathBuf};

pub fn files(root: bool) -> &'static [(&'static str, File, u32)] {
    let files: &'static [(&'static str, File, u32)] = &[
        ("cgroup.events", File::Events, 0o444),
        ("cgroup.procs", File::Procs, 0o644),
        ("cgroup.controllers", File::Controllers, 0o444),
        ("cgroup.subtree_control", File::SubtreeControl, 0o644),
    ];
    if root { &files[1..] } else { files }
}

pub fn file(root: bool, name: &str) -> Option<File> {
    files(root)
        .iter()
        .find(|(n, ..)| *n == name)
        .map(|(_, f, _)| *f)
}

pub fn populate(dir: &Path, root: bool) -> std::io::Result<Vec<(PathBuf, u32)>> {
    let mut made = Vec::new();
    for (name, _, mode) in files(root) {
        let path = dir.join(name);
        // Repeated init mkdir must not reset existing kernel state or owners.
        if !path.exists() {
            std::fs::write(&path, "")?;
            made.push((path, *mode));
        }
    }
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_freezer_or_controllers_are_advertised() {
        let dir = crate::scratch_dir("aim-cgroup2");
        populate(&dir, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("cgroup.controllers")).unwrap(),
            ""
        );
        assert!(!dir.join("cgroup.freeze").exists());
        assert!(!dir.join("cgroup.events").exists());
        assert!(populate(&dir, true).unwrap().is_empty());
        assert_eq!(file(true, "tasks"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
