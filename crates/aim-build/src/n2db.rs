//! Running ninja files with n2 (evmar/n2, linked into this binary), and
//! reading the inputs it discovered (depfiles) from its deps log, `.n2_db`.

use crate::cargo::{normalize, repository_sources};
use crate::log::Log;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The argv[0] under which this binary runs as n2.
pub const ARGV0: &str = "n2";

/// Builds `targets` (the defaults when empty) of the ninja file in `dir`.
pub fn build(dir: &Path, targets: &[&str], log: &mut Log) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    log.run(
        Command::new(exe)
            .arg0(ARGV0)
            .arg("-C")
            .arg(dir)
            .args(targets),
    )
}

/// The discovered inputs n2 recorded for the builds in `dir` that lie in
/// the repository (outside `target/` and `_build/`).
pub fn repository_deps(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let db = dir.join(".n2_db");
    let bytes = std::fs::read(&db).map_err(|e| format!("{}: {e}", db.display()))?;
    let deps = read(&bytes).map_err(|e| format!("{}: {e}", db.display()))?;
    Ok(repository_sources(
        deps.into_iter().map(|p| normalize(&dir.join(p))),
    ))
}

/// The deps-log format of n2's db.rs (version 1): the signature `n2db`, a
/// u32 version, then records. A record starts with a u16: with the high bit
/// clear it is a path of that length (ids count up from 0); with it set, a
/// build: that many output ids (u24), a u16 count of discovered input ids
/// (u24), and a u64 hash.
fn read(bytes: &[u8]) -> Result<Vec<String>, String> {
    let mut at = 0;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let slice = bytes.get(at..at + n).ok_or("truncated")?;
        at += n;
        Ok(slice)
    };
    if take(4)? != b"n2db" || take(4)? != 1u32.to_le_bytes() {
        return Err("not an n2 deps log (version 1)".into());
    }
    let mut paths = Vec::new();
    let mut deps = std::collections::BTreeSet::new();
    let u24 = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], 0]) as usize;
    loop {
        let Ok(head) = take(2) else { break };
        let head = u16::from_le_bytes([head[0], head[1]]);
        if head & 0x8000 == 0 {
            let name = take(usize::from(head))?;
            paths.push(String::from_utf8_lossy(name).into_owned());
            continue;
        }
        take(3 * usize::from(head & 0x7fff))?;
        let count = take(2)?;
        let count = u16::from_le_bytes([count[0], count[1]]);
        for _ in 0..count {
            deps.insert(u24(take(3)?));
        }
        take(8)?;
    }
    Ok(deps
        .into_iter()
        .filter_map(|id| paths.get(id).cloned())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deps_log() {
        let mut db = b"n2db".to_vec();
        db.extend(1u32.to_le_bytes());
        for name in ["out.o", "a.h", "../b.h"] {
            db.extend((name.len() as u16).to_le_bytes());
            db.extend(name.as_bytes());
        }
        db.extend(0x8001u16.to_le_bytes());
        db.extend([0, 0, 0]);
        db.extend(2u16.to_le_bytes());
        db.extend([1, 0, 0, 2, 0, 0]);
        db.extend(7u64.to_le_bytes());
        assert_eq!(read(&db).unwrap(), ["a.h", "../b.h"]);
        assert!(read(b"n2db").is_err());
    }

    #[test]
    fn normalizes() {
        assert_eq!(normalize(Path::new("/a/b/../c/./d")), Path::new("/a/c/d"));
    }
}
