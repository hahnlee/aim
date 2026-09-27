//! The translation cache: translated files keyed by the sha256 of the
//! original plus the translator version (ADR 0012 decision 8).
//!
//! Layout of a cache directory (for example
//! `~/Library/Caches/DarwinART/translated`):
//!
//! ```text
//! <dir>/<sha256>-v<VERSION>/meta   what the translator found (key=value lines)
//! <dir>/<sha256>-v<VERSION>/elf    the translated file (only when translated)
//! <dir>/index/<sha256 of host path> -> "<dev>:<ino>:<size>:<mtime> <sha256>"
//! ```
//!
//! - An entry is staged in `<dir>/.tmp-*`, made read-only, and published with
//!   one `rename`, so readers see a complete entry or none. It is never
//!   modified afterwards. A version bump changes every key, which
//!   invalidates all older entries at once.
//! - The index lets the runtime find an original's entry without hashing it:
//!   one `readlink` per ELF open. Index links are replaced atomically
//!   (`rename` of a new symlink) when a file at a path changes, and are
//!   checked against the file's current stat, so a stale link is a miss.

use std::ffi::CString;
use std::fs;
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::xlate::{self, Outcome, Output, Report};

/// Identity of an original file as the index records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileStat {
    pub dev: u64,
    pub ino: u64,
    pub size: u64,
    pub mtime_s: i64,
    pub mtime_ns: i64,
}

impl FileStat {
    pub fn from_libc(st: &libc::stat) -> FileStat {
        FileStat {
            dev: st.st_dev as u32 as u64,
            ino: st.st_ino,
            size: st.st_size as u64,
            mtime_s: st.st_mtime,
            mtime_ns: st.st_mtime_nsec,
        }
    }

    pub fn of_path(p: &Path) -> io::Result<FileStat> {
        let c = CString::new(p.as_os_str().as_bytes())?;
        // SAFETY: stat into a local buffer.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::stat(c.as_ptr(), &mut st) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(FileStat::from_libc(&st))
    }

    pub fn of_fd(fd: i32) -> io::Result<(FileStat, libc::stat)> {
        // SAFETY: fstat into a local buffer.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd, &mut st) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((FileStat::from_libc(&st), st))
    }

    fn tag(&self) -> String {
        format!(
            "{}:{}:{}:{}.{:09}",
            self.dev, self.ino, self.size, self.mtime_s, self.mtime_ns
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryKind {
    Translated(PathBuf),
    Identity,
    Unsupported(String),
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub key: String,
    pub kind: EntryKind,
    pub ctr_el0: u32,
}

#[derive(Clone, Debug)]
pub struct Cache {
    dir: PathBuf,
}

/// What [`Cache::translate_file`] did.
#[derive(Debug)]
pub struct FileResult {
    pub key: String,
    /// False when the entry already existed (another path, or a rerun).
    pub translated_now: bool,
    pub kind: &'static str,
    pub report: Option<Report>,
}

static STAGE_SEQ: AtomicU64 = AtomicU64::new(0);

fn meta_text(key: &str, sha: &str, size: u64, outcome: &Outcome, r: &Report) -> String {
    let mut s = String::new();
    let mut kv = |k: &str, v: String| {
        s.push_str(k);
        s.push('=');
        s.push_str(&v);
        s.push('\n');
    };
    kv("version", xlate::VERSION.to_string());
    kv("key", key.into());
    kv("sha256", sha.into());
    kv("outcome", outcome.name().into());
    if let Outcome::Unsupported(why) = outcome {
        kv("reason", why.replace('\n', " "));
    }
    kv("original_size", size.to_string());
    if let Outcome::Translated(o) = outcome {
        kv("translated_size", o.len().to_string());
    }
    kv("ctr_el0", format!("{:#x}", r.ctr_el0));
    for k in crate::a64::Kind::ALL {
        kv(k.name(), r.sites[k as usize].to_string());
    }
    kv("brk_fallback", r.brk_fallback.to_string());
    kv("ambiguous", r.ambiguous.to_string());
    kv("data_excluded", r.data_excluded.to_string());
    kv("outside_code", r.outside_code.to_string());
    kv("method", r.method.into());
    kv("oat", (r.is_oat as u8).to_string());
    kv("stub_vaddr", format!("{:#x}", r.stub_vaddr));
    kv("stub_size", r.stub_size.to_string());
    s
}

/// Parse a `meta` file into key/value pairs.
pub fn parse_meta(text: &str) -> Vec<(&str, &str)> {
    text.lines().filter_map(|l| l.split_once('=')).collect()
}

fn set_mode(p: &Path, mode: u32) -> io::Result<()> {
    fs::set_permissions(p, fs::Permissions::from_mode(mode))
}

/// Remove a staging or cache tree even though its contents are read-only.
pub fn remove_tree(p: &Path) -> io::Result<()> {
    if let Ok(meta) = fs::symlink_metadata(p)
        && meta.is_dir()
    {
        let _ = set_mode(p, 0o755);
        for e in fs::read_dir(p)? {
            remove_tree(&e?.path())?;
        }
        return fs::remove_dir(p);
    }
    match fs::remove_file(p) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

impl Cache {
    /// A cache rooted at `dir` (created on first publish).
    pub fn new(dir: impl Into<PathBuf>) -> Cache {
        Cache { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// `~/Library/Caches/DarwinART/translated`, with the home directory
    /// taken from the user database (not the environment).
    pub fn default_dir() -> Option<PathBuf> {
        // SAFETY: getpwuid returns a pointer into static storage or null.
        unsafe {
            let pw = libc::getpwuid(libc::getuid());
            if pw.is_null() || (*pw).pw_dir.is_null() {
                return None;
            }
            let home = std::ffi::CStr::from_ptr((*pw).pw_dir);
            Some(
                Path::new(std::ffi::OsStr::from_bytes(home.to_bytes()))
                    .join("Library/Caches/DarwinART/translated"),
            )
        }
    }

    pub fn entry_dir(&self, key: &str) -> PathBuf {
        self.dir.join(key)
    }

    fn index_path(&self, host_path: &Path) -> PathBuf {
        self.dir
            .join("index")
            .join(xlate::sha256_hex(host_path.as_os_str().as_bytes()))
    }

    /// Whether `p` is a file inside this cache's published entries.
    pub fn contains(&self, p: &Path) -> bool {
        p.starts_with(&self.dir)
    }

    /// The sha256 recorded for the file at `host_path`, if the index has it
    /// and the file has not changed since.
    pub fn lookup_digest(&self, host_path: &Path, st: &FileStat) -> Option<String> {
        let target = fs::read_link(self.index_path(host_path)).ok()?;
        let target = target.to_str()?;
        let (tag, sha) = target.split_once(' ')?;
        (tag == st.tag() && sha.len() == 64).then(|| sha.to_string())
    }

    /// Read a published entry.
    pub fn entry(&self, key: &str) -> Option<Entry> {
        let dir = self.entry_dir(key);
        let text = fs::read_to_string(dir.join("meta")).ok()?;
        let meta = parse_meta(&text);
        let get = |k: &str| meta.iter().find(|(a, _)| *a == k).map(|(_, v)| *v);
        if get("version")? != xlate::VERSION.to_string() {
            return None;
        }
        let ctr_el0 = u32::from_str_radix(get("ctr_el0")?.trim_start_matches("0x"), 16).ok()?;
        let kind = match get("outcome")? {
            "translated" => EntryKind::Translated(dir.join("elf")),
            "identity" => EntryKind::Identity,
            _ => EntryKind::Unsupported(get("reason").unwrap_or("").to_string()),
        };
        Some(Entry {
            key: key.to_string(),
            kind,
            ctr_el0,
        })
    }

    /// The entry for the original file at `host_path`, through the index.
    pub fn lookup(&self, host_path: &Path, st: &FileStat) -> Option<Entry> {
        let sha = self.lookup_digest(host_path, st)?;
        self.entry(&xlate::key_for_digest(&sha))
    }

    /// Publish an entry atomically. Returns false if it already existed.
    pub fn publish(&self, key: &str, meta: &str, output: Option<&Output>) -> io::Result<bool> {
        let final_dir = self.entry_dir(key);
        if final_dir.join("meta").exists() {
            return Ok(false);
        }
        fs::create_dir_all(&self.dir)?;
        let stage = self.dir.join(format!(
            ".tmp-{key}-{}-{}",
            std::process::id(),
            STAGE_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&stage)?;
        let result = (|| -> io::Result<()> {
            if let Some(o) = output {
                let path = stage.join("elf");
                let f = fs::File::create(&path)?;
                f.write_all_at(&o.patched, 0)?;
                // The gap up to the stub segment stays a hole.
                f.set_len(o.stub_offset)?;
                f.write_all_at(&o.stub_segment, o.stub_offset)?;
                f.sync_all()?;
                set_mode(&path, 0o444)?;
            }
            let path = stage.join("meta");
            let mut f = fs::File::create(&path)?;
            f.write_all(meta.as_bytes())?;
            f.sync_all()?;
            set_mode(&path, 0o444)?;
            set_mode(&stage, 0o555)
        })();
        if let Err(e) = result {
            let _ = remove_tree(&stage);
            return Err(e);
        }
        match fs::rename(&stage, &final_dir) {
            Ok(()) => Ok(true),
            Err(_) if final_dir.join("meta").exists() => {
                // Published concurrently with identical content.
                let _ = remove_tree(&stage);
                Ok(false)
            }
            Err(e) => {
                let _ = remove_tree(&stage);
                Err(e)
            }
        }
    }

    /// Point the index entry of `host_path` at `sha256`.
    pub fn record_index(&self, host_path: &Path, st: &FileStat, sha256: &str) -> io::Result<()> {
        let link = self.index_path(host_path);
        let want = format!("{} {sha256}", st.tag());
        if fs::read_link(&link).ok().as_deref() == Some(Path::new(&want)) {
            return Ok(());
        }
        let parent = link.parent().unwrap();
        fs::create_dir_all(parent)?;
        let tmp = parent.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            STAGE_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::os::unix::fs::symlink(&want, &tmp)?;
        fs::rename(&tmp, &link).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })
    }

    /// Translate the file at `host_path` (unless its entry exists), publish
    /// it and index it. Non-AArch64-ELF files return Ok(None).
    pub fn translate_file(
        &self,
        host_path: &Path,
        opts: &xlate::Options,
    ) -> io::Result<Option<FileResult>> {
        let st = FileStat::of_path(host_path)?;
        let bytes = fs::read(host_path)?;
        if !xlate::elf::is_aarch64_elf(&bytes) {
            return Ok(None);
        }
        let sha = xlate::sha256_hex(&bytes);
        let key = xlate::key_for_digest(&sha);
        if let Some(e) = self.entry(&key) {
            self.record_index(host_path, &st, &sha)?;
            let kind = match e.kind {
                EntryKind::Translated(_) => "translated",
                EntryKind::Identity => "identity",
                EntryKind::Unsupported(_) => "unsupported",
            };
            return Ok(Some(FileResult {
                key,
                translated_now: false,
                kind,
                report: None,
            }));
        }
        let t = match xlate::translate(&bytes, opts) {
            Ok(t) => t,
            // An AArch64 ELF the loader cannot use (e.g. ET_REL): no entry.
            Err(_) => return Ok(None),
        };
        let meta = meta_text(&key, &sha, bytes.len() as u64, &t.outcome, &t.report);
        let output = match &t.outcome {
            Outcome::Translated(o) => Some(o),
            _ => None,
        };
        let now = self.publish(&key, &meta, output)?;
        self.record_index(host_path, &st, &sha)?;
        Ok(Some(FileResult {
            key,
            translated_now: now,
            kind: t.outcome.name(),
            report: Some(t.report),
        }))
    }
}
