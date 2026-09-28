//! Content hashes: of files (with a stat cache, so an unchanged file is not
//! read again) and of fetched trees.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            return Ok(hex(&hasher.finalize()));
        }
        hasher.update(&buffer[..n]);
    }
}

/// What a file's content hash was recorded against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileState {
    pub size: u64,
    pub mtime_ns: i128,
    pub hash: String,
}

/// The hash of a missing input.
pub const MISSING: &str = "missing";

/// Hashes `path`, reusing `known` when size and mtime still match.
pub fn file_state(path: &Path, known: Option<&FileState>) -> FileState {
    let Ok(metadata) = fs::metadata(path) else {
        return FileState {
            size: 0,
            mtime_ns: 0,
            hash: MISSING.into(),
        };
    };
    let size = metadata.size();
    let mtime_ns = i128::from(metadata.mtime()) * 1_000_000_000 + i128::from(metadata.mtime_nsec());
    if let Some(known) = known
        && known.size == size
        && known.mtime_ns == mtime_ns
        && known.hash != MISSING
    {
        return known.clone();
    }
    let hash = sha256_file(path).unwrap_or_else(|_| MISSING.into());
    FileState {
        size,
        mtime_ns,
        hash,
    }
}

/// The content hash of a fetched tree, as the source locks record it:
/// sha256 over the `shasum -a 256` lines (`<sha256>  ./<path>`) of every
/// regular file except `.fetched` markers, sorted by path.
pub fn tree_hash(dir: &Path) -> io::Result<String> {
    let mut files = Vec::new();
    walk_files(dir, dir, &mut files)?;
    let mut lines: Vec<(String, PathBuf)> = files
        .into_iter()
        .map(|rel| (format!("./{}", rel.display()), rel))
        .collect();
    lines.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut text = String::new();
    for (name, rel) in lines {
        text.push_str(&format!("{}  {name}\n", sha256_file(&dir.join(rel))?));
    }
    Ok(sha256(text.as_bytes()))
}

fn walk_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            walk_files(root, &path, out)?;
        } else if kind.is_file() && entry.file_name() != ".fetched" {
            out.push(path.strip_prefix(root).unwrap().to_path_buf());
        }
    }
    Ok(())
}

/// Every regular file under `dir` (following no links), sorted.
pub fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let _ = walk_files(dir, dir, &mut files);
    let mut files: Vec<PathBuf> = files.into_iter().map(|rel| dir.join(rel)).collect();
    files.sort();
    files
}

/// A combined key over labelled parts.
#[derive(Default)]
pub struct KeyHasher(Sha256);

impl KeyHasher {
    pub fn part(&mut self, label: &str, value: &str) {
        self.0
            .update(format!("{}:{label}{}:{value}\n", label.len(), value.len()));
    }

    pub fn finish(self) -> String {
        hex(&self.0.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_hash_is_shasum_over_sorted_paths() {
        let dir = std::env::temp_dir().join(format!("aim-tree-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::write(dir.join("b/c"), "c").unwrap();
        std::fs::write(dir.join("a"), "a").unwrap();
        std::fs::write(dir.join(".fetched"), "").unwrap();
        let lines = format!("{}  ./a\n{}  ./b/c\n", sha256(b"a"), sha256(b"c"));
        assert_eq!(tree_hash(&dir).unwrap(), sha256(lines.as_bytes()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn file_state_reuses_a_matching_hash() {
        let path = std::env::temp_dir().join(format!("aim-state-{}", std::process::id()));
        std::fs::write(&path, "x").unwrap();
        let state = file_state(&path, None);
        assert_eq!(state.hash, sha256(b"x"));
        let fake = FileState {
            hash: "recorded".into(),
            ..state.clone()
        };
        assert_eq!(file_state(&path, Some(&fake)).hash, "recorded");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(file_state(&path, Some(&fake)).hash, MISSING);
    }
}
