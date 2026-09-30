//! Runtime use of the translation cache.
//!
//! - Two caches are consulted, in order: the image's own, beside its tree
//!   (`<root>/../translated`, docs/storage.md), for files of the image, and
//!   the user's (`--cache`, by default `~/Library/Caches/aim/translated`).
//! - When the guest opens an original AArch64 ELF read-only and the cache
//!   has a translated entry for it, the fd is replaced by one on the
//!   translated file ([`on_open`]). The guest linker then reads the
//!   translated program headers and maps the stub segment itself.
//! - Executable mappings of translated files (and of originals with nothing
//!   to rewrite) are file-backed and shared ([`exec_source`]).
//! - Anything else takes the load-time path: an anonymous copy rewritten
//!   before it becomes executable, with sites found by the translator's
//!   code/data identification when the code is an ELF's: a file, or a
//!   library stored in an APK. Those sites are kept in the cache
//!   ([`Cache::publish_sites`]), so only the first process to map an ELF's
//!   code analyzes it.

use std::collections::HashMap;
use std::ffi::CStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::cache::{self, Cache, EntryKind, FileStat};
use crate::xlate::{self, fips};
use crate::zip;

#[derive(Clone, Debug)]
enum FileState {
    /// A published translated file; the guest path of its original.
    Translated { guest: String },
    /// An original with a translated entry at this path.
    HasTranslation(PathBuf),
    /// An original the cache says needs no rewriting.
    Identity,
    /// An original with no usable entry; its sites found once.
    Uncached(Option<Arc<Sites>>),
    /// Not an AArch64 ELF (or not parseable as one).
    Other,
}

/// Where the load-time path rewrites an ELF's code, by file offset.
#[derive(Debug)]
pub struct Sites {
    pub sites: Vec<cache::Site>,
    /// A BoringSSL FIPS module whose hash must follow rewritten bytes.
    pub fips: Option<fips::Module>,
}

struct Runtime {
    /// The image's cache (files under the root), then the user's.
    caches: Vec<Cache>,
    ctr_el0: u32,
    files: Mutex<HashMap<FileStat, FileState>>,
    /// Files holding stored members (APKs): their members, and the sites of
    /// the ELFs found in them by offset.
    archives: Mutex<HashMap<FileStat, Archive>>,
}

struct Archive {
    members: Arc<Vec<zip::Member>>,
    elfs: HashMap<u64, Option<Arc<Sites>>>,
}

static RT: OnceLock<Runtime> = OnceLock::new();

/// Use the translation cache of the image at `root`, if it has one, and
/// `cache` (a translation cache directory) for this process. Without a
/// call, or with neither, every file takes the load-time path.
pub fn init(root: Option<&Path>, cache: Option<PathBuf>) {
    let _ = RT.set(Runtime {
        caches: root
            .and_then(Cache::image_of)
            .into_iter()
            .chain(cache.map(Cache::new))
            .collect(),
        ctr_el0: crate::a64::host_ctr_el0(),
        files: Mutex::new(HashMap::new()),
        archives: Mutex::new(HashMap::new()),
    });
}

fn rt() -> &'static Runtime {
    RT.get_or_init(|| Runtime {
        caches: Vec::new(),
        ctr_el0: crate::a64::host_ctr_el0(),
        files: Mutex::new(HashMap::new()),
        archives: Mutex::new(HashMap::new()),
    })
}

fn state(st: &FileStat) -> Option<FileState> {
    rt().files.lock().unwrap().get(st).cloned()
}

fn set_state(st: FileStat, s: FileState) {
    rt().files.lock().unwrap().insert(st, s);
}

pub(crate) fn fd_path(fd: i32) -> Option<PathBuf> {
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes.
    if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr()) } < 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len])))
}

fn is_elf_fd(fd: i32) -> bool {
    let mut head = [0u8; 20];
    // SAFETY: reading into a local buffer.
    let n = unsafe { libc::pread(fd, head.as_mut_ptr().cast(), head.len(), 0) };
    n == head.len() as isize && xlate::elf::is_aarch64_elf(&head)
}

/// Decide what an original at `host` (with stat `st`) maps as, from the
/// caches' indexes alone: None when no cache knows the file.
fn decide(host: &Path, st: &FileStat) -> Option<FileState> {
    let r = rt();
    let e = r.caches.iter().find_map(|c| c.lookup(host, st))?;
    Some(match e.kind {
        _ if e.ctr_el0 != r.ctr_el0 => FileState::Uncached(None),
        EntryKind::Translated(p) => FileState::HasTranslation(p),
        EntryKind::Identity => FileState::Identity,
        EntryKind::Unsupported(_) => FileState::Uncached(None),
    })
}

/// Open the published translated file `path` of the original at guest path
/// `guest`, with `cloexec` (0 or O_CLOEXEC).
fn open_published(path: &Path, guest: &str, cloexec: i32) -> Option<i32> {
    let cpath = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: opening a published cache file.
    let fd = unsafe { libc::open(cpath.as_ptr(), libc::O_RDONLY | cloexec) };
    if fd < 0 {
        return None;
    }
    if let Ok((tst, _)) = FileStat::of_fd(fd) {
        set_state(
            tst,
            FileState::Translated {
                guest: guest.to_string(),
            },
        );
    }
    Some(fd)
}

/// Before the guest opens `host` with Darwin flags `host_flags`: an
/// original the caches have a translation of, known from its stat and the
/// index, is opened as the translated file right away. That is one host
/// open, and the original's contents (in a compressed image, a
/// decompression) are never read. None: the caller opens `host` and calls
/// [`on_open`].
pub fn open_translated(host: &CStr, guest: &str, host_flags: i32) -> Option<i32> {
    let other = libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CREAT | libc::O_TRUNC;
    if host_flags & libc::O_ACCMODE != libc::O_RDONLY || host_flags & other != 0 {
        return None;
    }
    // SAFETY: stat into a local buffer.
    let mut raw: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::stat(host.as_ptr(), &mut raw) } != 0
        || raw.st_mode & libc::S_IFMT != libc::S_IFREG
        || raw.st_size < 64
    {
        return None;
    }
    let st = FileStat::from_libc(&raw);
    let s = match state(&st) {
        Some(s) => s,
        None => {
            let s = decide(Path::new(std::ffi::OsStr::from_bytes(host.to_bytes())), &st)?;
            set_state(st, s.clone());
            s
        }
    };
    let FileState::HasTranslation(path) = s else {
        return None;
    };
    open_published(&path, guest, host_flags & libc::O_CLOEXEC)
}

/// Called after the guest opened `host` as `fd` with Darwin flags
/// `host_flags`. Substitutes the translated file when there is one.
pub fn on_open(fd: i32, host: &CStr, guest: &str, host_flags: i32) {
    if host_flags & libc::O_ACCMODE != libc::O_RDONLY || host_flags & libc::O_DIRECTORY != 0 {
        return;
    }
    let Ok((st, raw)) = FileStat::of_fd(fd) else {
        return;
    };
    if raw.st_mode & libc::S_IFMT != libc::S_IFREG || st.size < 64 {
        return;
    }
    let s = match state(&st) {
        Some(s) => s,
        None => {
            let s = match decide(Path::new(std::ffi::OsStr::from_bytes(host.to_bytes())), &st) {
                Some(s) => s,
                None if is_elf_fd(fd) => FileState::Uncached(None),
                None => FileState::Other,
            };
            set_state(st, s.clone());
            s
        }
    };
    let FileState::HasTranslation(path) = s else {
        return;
    };
    let Some(nfd) = open_published(&path, guest, libc::O_CLOEXEC) else {
        return; // Keep the original; the load-time path covers it.
    };
    // SAFETY: moving the translated file onto the guest's fd number,
    // keeping FD_CLOEXEC as requested.
    unsafe {
        if libc::dup2(nfd, fd) >= 0 && host_flags & libc::O_CLOEXEC != 0 {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        libc::close(nfd);
    }
}

/// How an executable file mapping of `fd` is made.
pub enum ExecSource {
    /// Map the file itself, shared and read-only, then make it executable.
    Shared,
    /// Copy into anonymous memory and rewrite these sites (file offsets);
    /// None: no metadata, scan every word.
    LoadTime(Option<Arc<Sites>>),
}

fn read_fd(fd: i32, off: u64, size: u64) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; size as usize];
    let mut done = 0usize;
    while done < buf.len() {
        // SAFETY: reading into our buffer.
        let n = unsafe {
            libc::pread(
                fd,
                buf[done..].as_mut_ptr().cast(),
                buf.len() - done,
                (off + done as u64) as i64,
            )
        };
        if n <= 0 {
            return None;
        }
        done += n as usize;
    }
    Some(buf)
}

/// The sites of the ELF at `[start, start+size)` of `fd`, as offsets in
/// the file: from the cache, or found and then recorded there. None when
/// the bytes are no ELF the translator can read. Sites are kept in the
/// user's cache; an image's is read-only.
fn sites_of(fd: i32, st: &FileStat, start: u64, size: u64) -> Option<Arc<Sites>> {
    let cache = rt().caches.iter().find(|c| !c.is_image());
    let member = cache
        .and_then(|_| fd_path(fd))
        .map(|p| cache::member_path(&p, start));
    if let (Some(c), Some(m)) = (cache, &member)
        && let Some(mut sites) = c.lookup_sites(m, st)
    {
        sites.iter_mut().for_each(|s| s.0 += start);
        return Some(Arc::new(Sites { sites, fips: None }));
    }
    let bytes = read_fd(fd, start, size)?;
    // The same content at another path (an app installed again) has its
    // sites recorded already.
    let sha = cache.map(|_| xlate::sha256_hex(&bytes));
    if let (Some(c), Some(m), Some(sha)) = (cache, &member, &sha)
        && let Some(mut sites) = c.sites(sha)
    {
        let _ = c.record_sites_index(m, st, sha);
        sites.iter_mut().for_each(|s| s.0 += start);
        return Some(Arc::new(Sites { sites, fips: None }));
    }
    let elf = xlate::elf::parse(&bytes).ok()?;
    let a = xlate::analyze(&elf);
    let sites: Vec<cache::Site> = a
        .sites
        .iter()
        .map(|s| (start + s.offset, s.kind, s.rt))
        .collect();
    let fips = a.fips.map(|mut m| {
        m.text_offset += start;
        m.rodata_offset += start;
        m.hash_offset += start;
        m
    });
    // A FIPS module's hash must be recomputed from the file each time, so
    // its sites are not recorded.
    if let (Some(c), Some(m), Some(sha), None) = (cache, &member, &sha, &fips) {
        let local: Vec<_> = sites.iter().map(|&(o, k, r)| (o - start, k, r)).collect();
        if let Err(e) = c.publish_sites(m, st, sha, &local) {
            crate::diag!(
                "[linux-abi] cannot record the sites of {}: {e}",
                m.display()
            );
        }
    }
    Some(Arc::new(Sites { sites, fips }))
}

/// The sites of the ELF stored in the archive `fd` (an APK) that holds
/// file offset `off`, if there is one.
fn archive_sites(fd: i32, st: &FileStat, off: u64) -> Option<Arc<Sites>> {
    let read = |at: u64, len: usize| read_fd(fd, at, len as u64);
    let members = {
        let archives = rt().archives.lock().unwrap();
        archives.get(st).map(|a| a.members.clone())
    };
    let members = match members {
        Some(m) => m,
        None => {
            let m = Arc::new(zip::members(st.size, read).unwrap_or_default());
            rt().archives.lock().unwrap().insert(
                *st,
                Archive {
                    members: m.clone(),
                    elfs: HashMap::new(),
                },
            );
            m
        }
    };
    let (start, size) = zip::stored_at(&members, off, read)?;
    if let Some(s) = rt().archives.lock().unwrap().get(st)?.elfs.get(&start) {
        return s.clone();
    }
    let head = read_fd(fd, start, 20)?;
    let sites = if xlate::elf::is_aarch64_elf(&head) {
        sites_of(fd, st, start, size)
    } else {
        None
    };
    if let Some(a) = rt().archives.lock().unwrap().get_mut(st) {
        a.elfs.insert(start, sites.clone());
    }
    sites
}

/// How an executable private mapping of `fd` at file offset `off` is made.
pub fn exec_source(fd: i32, off: u64) -> ExecSource {
    let Ok((st, _)) = FileStat::of_fd(fd) else {
        return ExecSource::LoadTime(None);
    };
    match state(&st) {
        Some(FileState::Translated { .. } | FileState::Identity) => ExecSource::Shared,
        Some(FileState::Uncached(Some(a))) => ExecSource::LoadTime(Some(a)),
        Some(FileState::Other) => ExecSource::LoadTime(archive_sites(fd, &st, off)),
        s => {
            // Not seen at open (or opened writable): a published cache file
            // maps shared; anything else has its sites found once.
            if s.is_none()
                && let Some(p) = fd_path(fd)
                && in_cache(&p)
            {
                return ExecSource::Shared;
            }
            let sites = sites_of(fd, &st, 0, st.size);
            set_state(
                st,
                match (&s, &sites) {
                    (Some(FileState::HasTranslation(p)), _) => FileState::HasTranslation(p.clone()),
                    (_, Some(a)) => FileState::Uncached(Some(a.clone())),
                    (_, None) => FileState::Other,
                },
            );
            match (s, sites) {
                (_, Some(a)) => ExecSource::LoadTime(Some(a)),
                // Not an ELF: maybe an archive.
                (None, None) => ExecSource::LoadTime(archive_sites(fd, &st, off)),
                (_, None) => ExecSource::LoadTime(None),
            }
        }
    }
}

/// Whether `p` is a published file of one of the caches.
fn in_cache(p: &Path) -> bool {
    rt().caches.iter().any(|c| c.contains(p))
}

/// Whether non-executable private mappings of `fd` can stay file-backed
/// (the file never needs rewriting in memory).
pub fn is_shared_source(fd: i32) -> bool {
    let Ok((st, _)) = FileStat::of_fd(fd) else {
        return false;
    };
    matches!(
        state(&st),
        Some(FileState::Translated { .. } | FileState::Identity)
    )
}

/// Guest path of the original behind a substituted fd.
pub fn original_guest_path(fd: i32) -> Option<String> {
    let (st, _) = FileStat::of_fd(fd).ok()?;
    match state(&st)? {
        FileState::Translated { guest } => Some(guest),
        _ => None,
    }
}

/// Guest path of the original behind a translated file mapped from `host`
/// (for `/proc/self/maps`).
pub fn original_guest_path_of_host(host: &Path) -> Option<String> {
    match state(&FileStat::of_path(host).ok()?)? {
        FileState::Translated { guest } => Some(guest),
        _ => None,
    }
}

/// The loader mapped `host` for the guest file `guest`.
pub fn note_mapped(host: &Path, guest: &str) {
    if let Ok(st) = FileStat::of_path(host)
        && in_cache(host)
    {
        set_state(
            st,
            FileState::Translated {
                guest: guest.to_string(),
            },
        );
    }
}

/// How the program loader maps an ELF file.
pub enum LoaderSource {
    /// Map this file (the translated file or an unmodified original)
    /// file-backed; the string says which.
    File(PathBuf, &'static str),
    /// Copy and rewrite at load time.
    LoadTime,
}

pub fn loader_source(host: &Path) -> LoaderSource {
    let Ok(st) = FileStat::of_path(host) else {
        return LoaderSource::LoadTime;
    };
    match decide(host, &st) {
        Some(FileState::HasTranslation(p)) => LoaderSource::File(p, "translation cache"),
        Some(FileState::Identity) => {
            LoaderSource::File(host.to_path_buf(), "original (nothing to rewrite)")
        }
        _ => LoaderSource::LoadTime,
    }
}

/// Fork: what is known about each file, but analyses (redone on demand).
pub(crate) fn fork_save(w: &mut crate::sys::fork_state::Writer) {
    let files = rt().files.lock().unwrap();
    let known: Vec<_> = files
        .iter()
        .filter(|(_, s)| !matches!(s, FileState::Uncached(_)))
        .collect();
    w.seq(known.into_iter(), |w, (st, s)| {
        w.u64(st.dev);
        w.u64(st.ino);
        w.u64(st.size);
        w.i64(st.mtime_s);
        w.i64(st.mtime_ns);
        match s {
            FileState::Translated { guest } => {
                w.u32(0);
                w.str(guest);
            }
            FileState::HasTranslation(p) => {
                w.u32(1);
                w.path(p);
            }
            FileState::Identity => w.u32(2),
            _ => w.u32(3),
        }
    });
}

pub(crate) fn fork_restore(r: &mut crate::sys::fork_state::Reader) {
    let known = r.seq(|r| {
        let st = FileStat {
            dev: r.u64(),
            ino: r.u64(),
            size: r.u64(),
            mtime_s: r.i64(),
            mtime_ns: r.i64(),
        };
        let s = match r.u32() {
            0 => FileState::Translated { guest: r.str() },
            1 => FileState::HasTranslation(r.path()),
            2 => FileState::Identity,
            _ => FileState::Other,
        };
        (st, s)
    });
    rt().files.lock().unwrap().extend(known);
}
