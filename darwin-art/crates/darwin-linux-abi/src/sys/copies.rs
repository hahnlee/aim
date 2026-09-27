//! Anonymous memory standing in for private file mappings: ELF segments
//! the loader copied and rewrote, and private file mappings of files that
//! are not translated (`mem::mmap`). Linux would show them file-backed.
//! Recording what file each came from lets `/proc/self/maps` name them and
//! lets MADV_DONTNEED restore the file's contents instead of zeros.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Clone)]
pub struct Copy {
    pub end: u64,
    pub guest: String,
    pub host: PathBuf,
    /// File offset of the copy's first byte.
    pub offset: u64,
    pub dev: u64,
    pub ino: u64,
}

static COPIES: Mutex<BTreeMap<u64, Copy>> = Mutex::new(BTreeMap::new());

/// Drop what is recorded for `[lo, hi)`, keeping the parts outside it.
pub fn forget(lo: u64, hi: u64) {
    let mut m = COPIES.lock().unwrap();
    let hit: Vec<u64> = m
        .range(..hi)
        .filter(|(s, c)| **s < hi && c.end > lo)
        .map(|(s, _)| *s)
        .collect();
    for s in hit {
        let c = m.remove(&s).unwrap();
        if s < lo {
            m.insert(
                s,
                Copy {
                    end: lo,
                    ..c.clone()
                },
            );
        }
        if c.end > hi {
            let offset = c.offset + (hi - s);
            m.insert(hi, Copy { offset, ..c });
        }
    }
}

/// `[start, start+len)` now holds a copy of `host` from `offset`.
pub fn note(start: u64, len: u64, host: &Path, guest: &str, offset: u64) {
    let (dev, ino) = std::fs::metadata(host).map_or((0, 0), |m| {
        use std::os::unix::fs::MetadataExt;
        (m.dev() as u32 as u64, m.ino())
    });
    forget(start, start + len);
    COPIES.lock().unwrap().insert(
        start,
        Copy {
            end: start + len,
            guest: guest.to_string(),
            host: host.to_path_buf(),
            offset,
            dev,
            ino,
        },
    );
}

/// The copy containing `addr`, with the file offset of `addr`.
pub fn find(addr: u64) -> Option<(Copy, u64)> {
    let m = COPIES.lock().unwrap();
    let (s, c) = m.range(..=addr).next_back()?;
    (addr < c.end).then(|| (c.clone(), c.offset + (addr - s)))
}

/// mremap moved `[old, old+len)` to `new`.
pub fn moved(old: u64, new: u64, len: u64) {
    let parts: Vec<(u64, Copy)> = {
        let m = COPIES.lock().unwrap();
        m.range(..old + len)
            .filter(|(s, c)| **s < old + len && c.end > old)
            .map(|(s, c)| (*s, c.clone()))
            .collect()
    };
    forget(old, old + len);
    forget(new, new + len);
    let mut m = COPIES.lock().unwrap();
    for (s, c) in parts {
        let lo = s.max(old);
        let hi = c.end.min(old + len);
        m.insert(
            new + (lo - old),
            Copy {
                end: new + (hi - old),
                offset: c.offset + (lo - s),
                ..c
            },
        );
    }
}
