//! Private file mappings: anonymous memory standing in for them (ELF
//! segments the loader copied and rewrote, executable private mappings of
//! files that are not translated), and the file-backed ones (`mem::mmap`).
//! Linux would show all of them file-backed. Recording what file each came
//! from, and from where, lets `/proc/self/maps` name them and lets
//! MADV_DONTNEED restore the file's contents instead of zeros. A fork
//! child needs the offset of a file-backed one too: its copy of the
//! mapping reports offset 0 into a copy of the file's pages.

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
    note_file(start, len, host, guest, offset, (dev, ino));
}

/// As [`note`], for the file with device and inode `id`.
pub fn note_file(start: u64, len: u64, host: &Path, guest: &str, offset: u64, id: (u64, u64)) {
    let (dev, ino) = id;
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

/// Copies overlapping `[lo, hi)`, clipped to it with adjusted file offsets.
/// Anonymous gaps require no per-page lookup or lock acquisition.
pub fn in_range(lo: u64, hi: u64) -> Vec<(u64, Copy)> {
    pieces(&COPIES.lock().unwrap(), lo, hi)
}

fn pieces(m: &BTreeMap<u64, Copy>, lo: u64, hi: u64) -> Vec<(u64, Copy)> {
    let mut out = Vec::new();
    if lo >= hi {
        return out;
    }
    // Entries never overlap: walking back from hi stops at the first gap
    // below lo, with only one tree search even for a wholly anonymous range.
    for (&start, c) in m.range(..hi).rev() {
        if c.end <= lo {
            break;
        }
        let at = start.max(lo);
        out.push((
            at,
            Copy {
                end: c.end.min(hi),
                offset: c.offset + (at - start),
                ..c.clone()
            },
        ));
    }
    out.reverse();
    out
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

/// Fork: the record goes with the memory it describes.
pub(super) fn fork_save(w: &mut super::fork_state::Writer) {
    let m = COPIES.lock().unwrap();
    w.seq(m.iter(), |w, (s, c)| {
        w.u64(*s);
        w.u64(c.end);
        w.str(&c.guest);
        w.path(&c.host);
        w.u64(c.offset);
        w.u64(c.dev);
        w.u64(c.ino);
    });
}

pub(super) fn fork_restore(r: &mut super::fork_state::Reader) {
    let v = r.seq(|r| {
        (
            r.u64(),
            Copy {
                end: r.u64(),
                guest: r.str(),
                host: r.path(),
                offset: r.u64(),
                dev: r.u64(),
                ino: r.u64(),
            },
        )
    });
    COPIES.lock().unwrap().extend(v);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy(end: u64, offset: u64) -> Copy {
        Copy {
            end,
            offset,
            guest: "/file".into(),
            host: "file".into(),
            dev: 1,
            ino: 2,
        }
    }

    #[test]
    fn range_clips_both_ends_and_skips_anonymous_gaps() {
        let m = BTreeMap::from([
            (10, copy(30, 100)),
            (40, copy(50, 200)),
            (50, copy(80, 300)),
            (90, copy(100, 400)),
        ]);
        let got: Vec<_> = pieces(&m, 20, 60)
            .iter()
            .map(|(s, c)| (*s, c.end, c.offset))
            .collect();
        assert_eq!(got, [(20, 30, 110), (40, 50, 200), (50, 60, 300)]);
        assert_eq!(m[&10].offset, 100);
        assert_eq!(m[&50].end, 80);
    }

    #[test]
    fn range_excludes_touching_endpoints_and_empty_ranges() {
        let m = BTreeMap::from([(10, copy(20, 100)), (30, copy(40, 200))]);
        for (lo, hi) in [(0, 10), (20, 30), (40, u64::MAX), (15, 15), (30, 20)] {
            assert!(pieces(&m, lo, hi).is_empty(), "{lo}..{hi}");
        }
        let got = pieces(&m, 10, 20);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].0, got[0].1.end, got[0].1.offset), (10, 20, 100));
        assert!(pieces(&BTreeMap::new(), 0, u64::MAX).is_empty());
    }
}
