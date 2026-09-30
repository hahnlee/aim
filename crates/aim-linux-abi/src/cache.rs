//! The translation cache: translated files keyed by the sha256 of the
//! original plus the translator version (ADR 0012 decision 8).
//!
//! Layout of a cache directory (for example
//! `~/Library/Caches/aim/translated`):
//!
//! ```text
//! <dir>/<sha256>-v<VERSION>/meta   what the translator found (key=value lines)
//! <dir>/<sha256>-v<VERSION>/elf    the translated file (only when translated)
//! <dir>/<sha256>-v<VERSION>-sites/sites  load-time sites ([`sites_key`])
//! <dir>/index/<sha256 of host path> -> "<dev>:<ino>:<size>:<mtime> <sha256> <summary>"
//! ```
//!
//! The summary repeats what the digest's entry records, so a lookup opens
//! nothing: `v<VERSION> translated|identity <ctr_el0>`, `v<VERSION>
//! unsupported`, or `v<VERSION> sites` when only load-time sites are
//! recorded. A link written before summaries existed has none; its entry's
//! `meta` is read instead.
//!
//! An ELF stored inside another file (an APK's native library) is indexed
//! as `<host path>!<offset>` ([`member_path`]).
//!
//! A system image carries its own cache beside its tree (docs/storage.md):
//! `<volume>/root` is the guest root and `<volume>/translated` a cache
//! whose index is keyed by the path relative to `root`, without the device,
//! since the volume's mount point and device change with every attach:
//!
//! ```text
//! <volume>/translated/paths/<sha256 of relative path> -> "<ino>:<size>:<mtime> <sha256> <summary>"
//! ```
//!
//! - An entry is staged in `<dir>/.tmp-*`, made read-only, and published with
//!   one `rename`, so readers see a complete entry or none. It is never
//!   modified afterwards. A version bump changes every key, which
//!   invalidates all older entries at once.
//! - The index lets the runtime find an original's entry without hashing it
//!   or opening anything: one `readlink` per ELF open. Index links are replaced atomically
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

/// Moves a directory from its pre-rename (darwin-art) location to the new
/// one with a single exclusive `rename`: only when `new` does not exist yet,
/// never copying, replacing or deleting anything. Returns whether it moved.
pub fn migrate_legacy_dir(old: &Path, new: &Path) -> io::Result<bool> {
    let from = CString::new(old.as_os_str().as_bytes())?;
    let to = CString::new(new.as_os_str().as_bytes())?;
    // SAFETY: both paths are NUL-terminated and outlive the call.
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ENOENT | libc::EEXIST) => Ok(false),
        _ => Err(error),
    }
}

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

    /// The tag of a file inside an image, whose device is not stable.
    fn image_tag(&self) -> String {
        format!(
            "{}:{}:{}.{:09}",
            self.ino, self.size, self.mtime_s, self.mtime_ns
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
    /// An image's cache: the tree its index is relative to.
    image_root: Option<PathBuf>,
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
    kv("fips_rehashed", (r.fips_rehashed as u8).to_string());
    kv("stub_vaddr", format!("{:#x}", r.stub_vaddr));
    kv("stub_size", r.stub_size.to_string());
    s
}

/// Parse a `meta` file into key/value pairs.
pub fn parse_meta(text: &str) -> Vec<(&str, &str)> {
    text.lines().filter_map(|l| l.split_once('=')).collect()
}

/// A site the load-time path rewrites: file offset, kind, register.
pub type Site = (u64, crate::a64::Kind, u32);

/// Entries of load-time sites (`<sha256>-v<VERSION>-sites/sites`) are for
/// code that is mapped from a file the runtime cannot substitute (an ELF
/// stored inside an APK) or that has no translated entry: the runtime
/// rewrites a copy at these sites instead of scanning or analyzing it again
/// in every process.
fn sites_key(sha256: &str) -> String {
    format!("{sha256}-v{}-sites", xlate::VERSION)
}

/// The index path of the ELF at `offset` inside the file at `host_path`
/// (the file itself at 0).
pub fn member_path(host_path: &Path, offset: u64) -> PathBuf {
    if offset == 0 {
        return host_path.to_path_buf();
    }
    let mut p = host_path.as_os_str().to_owned();
    p.push(format!("!{offset}"));
    PathBuf::from(p)
}

/// 16 bytes per site: offset, kind and register (little-endian).
fn encode_sites(sites: &[Site]) -> Vec<u8> {
    let mut out = Vec::with_capacity(sites.len() * 16);
    for &(offset, kind, rt) in sites {
        out.extend_from_slice(&offset.to_le_bytes());
        out.extend_from_slice(&(kind as u32).to_le_bytes());
        out.extend_from_slice(&rt.to_le_bytes());
    }
    out
}

fn decode_sites(b: &[u8]) -> Option<Vec<Site>> {
    if b.len() % 16 != 0 {
        return None;
    }
    b.chunks_exact(16)
        .map(|c| {
            let word = |i: usize| u32::from_le_bytes(c[i..i + 4].try_into().unwrap());
            let kind = crate::a64::Kind::from_index(word(8))?;
            Some((
                u64::from_le_bytes(c[..8].try_into().unwrap()),
                kind,
                word(12),
            ))
        })
        .collect()
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
        Cache {
            dir: dir.into(),
            image_root: None,
        }
    }

    /// The cache of the image whose guest root is `root`:
    /// `<root>/../translated`, indexed relative to `root`. `root` must be
    /// spelled as the host paths looked up in it are (the runtime's
    /// canonical root).
    pub fn image(root: &Path) -> Cache {
        Cache {
            dir: root.parent().unwrap_or(root).join("translated"),
            image_root: Some(root.to_path_buf()),
        }
    }

    /// [`Cache::image`] if the image has a translation cache.
    pub fn image_of(root: &Path) -> Option<Cache> {
        let cache = Cache::image(root);
        cache.dir.is_dir().then_some(cache)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether this is an image's own (read-only) cache.
    pub fn is_image(&self) -> bool {
        self.image_root.is_some()
    }

    /// `~/Library/Caches/aim/translated`, with the home directory
    /// taken from the user database (not the environment). Moves the
    /// pre-rename `~/Library/Caches/DarwinART` there first if only it exists.
    pub fn default_dir() -> Option<PathBuf> {
        // SAFETY: getpwuid returns a pointer into static storage or null.
        unsafe {
            let pw = libc::getpwuid(libc::getuid());
            if pw.is_null() || (*pw).pw_dir.is_null() {
                return None;
            }
            let home = std::ffi::CStr::from_ptr((*pw).pw_dir);
            let caches =
                Path::new(std::ffi::OsStr::from_bytes(home.to_bytes())).join("Library/Caches");
            // Best effort: on failure the old tree stays where it was.
            let _ = migrate_legacy_dir(&caches.join("DarwinART"), &caches.join("aim"));
            Some(caches.join("aim/translated"))
        }
    }

    pub fn entry_dir(&self, key: &str) -> PathBuf {
        self.dir.join(key)
    }

    /// The index link of `host_path` and the tag it must carry; None for a
    /// path outside an image cache's tree.
    fn index_link(&self, host_path: &Path, st: &FileStat) -> Option<(PathBuf, String)> {
        match &self.image_root {
            None => Some((
                self.dir
                    .join("index")
                    .join(xlate::sha256_hex(host_path.as_os_str().as_bytes())),
                st.tag(),
            )),
            Some(root) => {
                let relative = host_path.strip_prefix(root).ok()?;
                Some((
                    self.dir
                        .join("paths")
                        .join(xlate::sha256_hex(relative.as_os_str().as_bytes())),
                    st.image_tag(),
                ))
            }
        }
    }

    /// Whether `p` is a file inside this cache's published entries.
    pub fn contains(&self, p: &Path) -> bool {
        p.starts_with(&self.dir)
    }

    /// The sha256 recorded for the file at `host_path`, if the index has it
    /// and the file has not changed since.
    pub fn lookup_digest(&self, host_path: &Path, st: &FileStat) -> Option<String> {
        self.read_index(host_path, st).map(|(sha, _)| sha)
    }

    /// The digest and summary (None in a link written before summaries)
    /// the index records for `host_path`, if the file has not changed since.
    fn read_index(&self, host_path: &Path, st: &FileStat) -> Option<(String, Option<String>)> {
        let (link, want) = self.index_link(host_path, st)?;
        let target = fs::read_link(link).ok()?;
        let mut fields = target.to_str()?.splitn(3, ' ');
        let (tag, sha) = (fields.next()?, fields.next()?);
        (tag == want && sha.len() == 64)
            .then(|| (sha.to_string(), fields.next().map(str::to_string)))
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

    /// The entry for the original file at `host_path`, from the index's
    /// summary (an unsupported entry's reason is only in its `meta`).
    pub fn lookup(&self, host_path: &Path, st: &FileStat) -> Option<Entry> {
        let (sha, summary) = self.read_index(host_path, st)?;
        let key = xlate::key_for_digest(&sha);
        let Some(summary) = summary else {
            return self.entry(&key);
        };
        let mut fields = summary.split(' ');
        if fields.next()? != format!("v{}", xlate::VERSION) {
            return None;
        }
        let kind = match fields.next()? {
            "translated" => EntryKind::Translated(self.entry_dir(&key).join("elf")),
            "identity" => EntryKind::Identity,
            "unsupported" => EntryKind::Unsupported(String::new()),
            _ => return None,
        };
        let ctr_el0 = match fields.next() {
            Some(c) => u32::from_str_radix(c.trim_start_matches("0x"), 16).ok()?,
            None => 0,
        };
        Some(Entry { key, kind, ctr_el0 })
    }

    /// Publish an entry atomically. Returns false if it already existed.
    pub fn publish(&self, key: &str, meta: &str, output: Option<&Output>) -> io::Result<bool> {
        self.publish_with(key, meta, |stage| {
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
            Ok(())
        })
    }

    /// Stage an entry with `meta` and what `fill` writes into the staging
    /// directory, then publish it with one `rename`.
    fn publish_with(
        &self,
        key: &str,
        meta: &str,
        fill: impl FnOnce(&Path) -> io::Result<()>,
    ) -> io::Result<bool> {
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
            fill(&stage)?;
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

    /// The load-time sites recorded for the ELF at `member` (a host path,
    /// see [`member_path`]) if the file has not changed since.
    pub fn lookup_sites(&self, member: &Path, st: &FileStat) -> Option<Vec<Site>> {
        self.sites(&self.lookup_digest(member, st)?)
    }

    /// The load-time sites recorded for content with digest `sha256`.
    pub fn sites(&self, sha256: &str) -> Option<Vec<Site>> {
        let dir = self.entry_dir(&sites_key(sha256));
        decode_sites(&fs::read(dir.join("sites")).ok()?)
    }

    /// Record `sites`, found in the ELF at `member` whose content has
    /// digest `sha256`, for [`Cache::lookup_sites`].
    pub fn publish_sites(
        &self,
        member: &Path,
        st: &FileStat,
        sha256: &str,
        sites: &[Site],
    ) -> io::Result<()> {
        let key = sites_key(sha256);
        let meta = format!(
            "version={}\nkey={key}\nsha256={sha256}\noutcome=sites\nsites={}\n",
            xlate::VERSION,
            sites.len()
        );
        self.publish_with(&key, &meta, |stage| {
            let path = stage.join("sites");
            let mut f = fs::File::create(&path)?;
            f.write_all(&encode_sites(sites))?;
            f.sync_all()?;
            set_mode(&path, 0o444)
        })?;
        self.record_sites_index(member, st, sha256)
    }

    /// Point the index entry of `member` at `sha256`, whose load-time
    /// sites are recorded.
    pub fn record_sites_index(&self, member: &Path, st: &FileStat, sha256: &str) -> io::Result<()> {
        self.record_index(member, st, sha256, "sites", None)
    }

    /// Point the index entry of `host_path` at `sha256`, whose entry has
    /// `outcome` (and `ctr_el0`, for translated and identity entries).
    fn record_index(
        &self,
        host_path: &Path,
        st: &FileStat,
        sha256: &str,
        outcome: &str,
        ctr_el0: Option<u32>,
    ) -> io::Result<()> {
        let (link, tag) = self.index_link(host_path, st).ok_or_else(|| {
            io::Error::other(format!(
                "{}: outside the image this cache belongs to",
                host_path.display()
            ))
        })?;
        let mut want = format!("{tag} {sha256} v{} {outcome}", xlate::VERSION);
        if let Some(c) = ctr_el0 {
            want.push_str(&format!(" {c:#x}"));
        }
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
            let (kind, ctr_el0) = match e.kind {
                EntryKind::Translated(_) => ("translated", Some(e.ctr_el0)),
                EntryKind::Identity => ("identity", Some(e.ctr_el0)),
                EntryKind::Unsupported(_) => ("unsupported", None),
            };
            self.record_index(host_path, &st, &sha, kind, ctr_el0)?;
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
        let ctr_el0 = match &t.outcome {
            Outcome::Unsupported(_) => None,
            _ => Some(t.report.ctr_el0),
        };
        self.record_index(host_path, &st, &sha, t.outcome.name(), ctr_el0)?;
        Ok(Some(FileResult {
            key,
            translated_now: now,
            kind: t.outcome.name(),
            report: Some(t.report),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{Cache, EntryKind, FileStat, migrate_legacy_dir};
    use std::fs;

    #[test]
    fn image_index_is_relative_to_the_root() {
        let vol = std::env::temp_dir().join(format!("aim-image-cache-{}", std::process::id()));
        let _ = super::remove_tree(&vol);
        let root = vol.join("root");
        fs::create_dir_all(root.join("system/lib64")).unwrap();
        let lib = root.join("system/lib64/libx.so");
        fs::write(&lib, b"original").unwrap();
        let outside = vol.join("elsewhere.so");
        fs::write(&outside, b"original").unwrap();
        let sha = "ab".repeat(32);

        assert!(Cache::image_of(&root).is_none(), "no translated/ yet");
        let cache = Cache::image(&root);
        let st = FileStat::of_path(&lib).unwrap();
        cache.record_sites_index(&lib, &st, &sha).unwrap();
        assert!(Cache::image_of(&root).is_some());
        assert_eq!(cache.lookup_digest(&lib, &st), Some(sha.clone()));
        // Another attach: another device and mount point, same relative
        // path, inode, size and mtime.
        let moved = vol.join("moved");
        fs::rename(vol.join("root"), &moved).unwrap();
        let (moved_lib, other) = (moved.join("system/lib64/libx.so"), Cache::image(&moved));
        let st2 = FileStat {
            dev: st.dev + 1,
            ..FileStat::of_path(&moved_lib).unwrap()
        };
        assert_eq!(other.lookup_digest(&moved_lib, &st2), Some(sha.clone()));
        // A changed file is a miss.
        for stale in [
            FileStat {
                ino: st.ino + 1,
                ..st2
            },
            FileStat {
                size: st.size + 1,
                ..st2
            },
            FileStat {
                mtime_ns: (st.mtime_ns + 1) % 1_000_000_000,
                ..st2
            },
        ] {
            assert_eq!(other.lookup_digest(&moved_lib, &stale), None);
        }
        // A path outside the tree is not the image cache's.
        let st3 = FileStat::of_path(&outside).unwrap();
        assert_eq!(other.lookup_digest(&outside, &st3), None);
        assert!(other.record_sites_index(&outside, &st3, &sha).is_err());
        super::remove_tree(&vol).unwrap();
    }

    /// A lookup takes the entry's outcome from the index link and opens
    /// nothing; a link without a summary still finds the entry's `meta`.
    #[test]
    fn lookup_reads_the_index_summary() {
        let dir = std::env::temp_dir().join(format!("aim-index-summary-{}", std::process::id()));
        let _ = super::remove_tree(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("lib.so");
        fs::write(&file, b"original").unwrap();
        let st = FileStat::of_path(&file).unwrap();
        let cache = Cache::new(dir.join("cache"));
        let sha = "cd".repeat(32);
        let key = crate::xlate::key_for_digest(&sha);

        // No entry directory exists: the summary alone answers.
        cache
            .record_index(&file, &st, &sha, "translated", Some(0x8444_c004))
            .unwrap();
        let e = cache.lookup(&file, &st).unwrap();
        assert_eq!(
            e.kind,
            EntryKind::Translated(cache.entry_dir(&key).join("elf"))
        );
        assert_eq!((e.key.as_str(), e.ctr_el0), (key.as_str(), 0x8444_c004));
        cache
            .record_index(&file, &st, &sha, "identity", Some(1))
            .unwrap();
        assert_eq!(cache.lookup(&file, &st).unwrap().kind, EntryKind::Identity);
        cache.record_sites_index(&file, &st, &sha).unwrap();
        assert!(cache.lookup(&file, &st).is_none());
        assert_eq!(cache.lookup_digest(&file, &st), Some(sha.clone()));

        // Another translator version's summary is a miss.
        let (link, tag) = cache.index_link(&file, &st).unwrap();
        let relink = |target: String| {
            fs::remove_file(&link).unwrap();
            std::os::unix::fs::symlink(target, &link).unwrap();
        };
        relink(format!("{tag} {sha} v0 translated 0x1"));
        assert!(cache.lookup(&file, &st).is_none());

        // A link from before summaries: the entry's meta.
        relink(format!("{tag} {sha}"));
        assert!(cache.lookup(&file, &st).is_none());
        fs::create_dir_all(cache.entry_dir(&key)).unwrap();
        fs::write(
            cache.entry_dir(&key).join("meta"),
            format!(
                "version={}\noutcome=identity\nctr_el0=0x5\n",
                crate::xlate::VERSION
            ),
        )
        .unwrap();
        let e = cache.lookup(&file, &st).unwrap();
        assert_eq!((e.kind, e.ctr_el0), (EntryKind::Identity, 5));
        super::remove_tree(&dir).unwrap();
    }

    #[test]
    fn legacy_dir_moves_once_and_never_replaces() {
        let root = std::env::temp_dir().join(format!("aim-migrate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (old, new) = (root.join("DarwinART"), root.join("aim"));
        fs::create_dir_all(old.join("translated")).unwrap();
        fs::write(old.join("translated/entry"), b"kept").unwrap();

        assert!(migrate_legacy_dir(&old, &new).unwrap());
        assert!(!old.exists());
        assert_eq!(fs::read(new.join("translated/entry")).unwrap(), b"kept");
        // Nothing left to move.
        assert!(!migrate_legacy_dir(&old, &new).unwrap());

        // Both present: neither is touched, even an empty new directory.
        fs::create_dir(&old).unwrap();
        fs::write(old.join("stale"), b"old").unwrap();
        assert!(!migrate_legacy_dir(&old, &new).unwrap());
        assert_eq!(fs::read(old.join("stale")).unwrap(), b"old");
        assert_eq!(fs::read(new.join("translated/entry")).unwrap(), b"kept");
        let empty = root.join("empty");
        fs::create_dir(&empty).unwrap();
        assert!(!migrate_legacy_dir(&old, &empty).unwrap());
        assert!(old.join("stale").exists());
        fs::remove_dir_all(&root).unwrap();
    }
}
