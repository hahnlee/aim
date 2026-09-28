//! Runtime use of the translation cache.
//!
//! - When the guest opens an original AArch64 ELF read-only and the cache
//!   has a translated entry for it, the fd is replaced by one on the
//!   translated file ([`on_open`]). The guest linker then reads the
//!   translated program headers and maps the stub segment itself.
//! - Executable mappings of translated files (and of originals with nothing
//!   to rewrite) are file-backed and shared ([`exec_source`]).
//! - Anything else takes the load-time path: an anonymous copy rewritten
//!   before it becomes executable, with sites found by the translator's
//!   code/data identification when the file is an ELF.

use std::collections::HashMap;
use std::ffi::CStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::cache::{Cache, EntryKind, FileStat};
use crate::xlate;

#[derive(Clone, Debug)]
enum FileState {
    /// A published translated file; the guest path of its original.
    Translated { guest: String },
    /// An original with a translated entry at this path.
    HasTranslation(PathBuf),
    /// An original the cache says needs no rewriting.
    Identity,
    /// An original with no usable entry; analyzed once.
    Uncached(Option<Arc<xlate::Analysis>>),
    /// Not an AArch64 ELF (or not parseable as one).
    Other,
}

struct Runtime {
    cache: Option<Cache>,
    ctr_el0: u32,
    files: Mutex<HashMap<FileStat, FileState>>,
}

static RT: OnceLock<Runtime> = OnceLock::new();

/// Use `cache` (a translation cache directory) for this process. Without a
/// call, or with None, every file takes the load-time path.
pub fn init(cache: Option<PathBuf>) {
    let _ = RT.set(Runtime {
        cache: cache.map(Cache::new),
        ctr_el0: crate::a64::host_ctr_el0(),
        files: Mutex::new(HashMap::new()),
    });
}

fn rt() -> &'static Runtime {
    RT.get_or_init(|| Runtime {
        cache: None,
        ctr_el0: crate::a64::host_ctr_el0(),
        files: Mutex::new(HashMap::new()),
    })
}

fn state(st: &FileStat) -> Option<FileState> {
    rt().files.lock().unwrap().get(st).cloned()
}

fn set_state(st: FileStat, s: FileState) {
    rt().files.lock().unwrap().insert(st, s);
}

fn fd_path(fd: i32) -> Option<PathBuf> {
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

/// Decide what an original at `host` (with stat `st`) maps as.
fn decide(host: &Path, st: &FileStat) -> FileState {
    let r = rt();
    let Some(cache) = &r.cache else {
        return FileState::Uncached(None);
    };
    match cache.lookup(host, st) {
        Some(e) if e.ctr_el0 == r.ctr_el0 => match e.kind {
            EntryKind::Translated(p) => FileState::HasTranslation(p),
            EntryKind::Identity => FileState::Identity,
            EntryKind::Unsupported(_) => FileState::Uncached(None),
        },
        _ => FileState::Uncached(None),
    }
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
            let s = if is_elf_fd(fd) {
                decide(Path::new(std::ffi::OsStr::from_bytes(host.to_bytes())), &st)
            } else {
                FileState::Other
            };
            set_state(st, s.clone());
            s
        }
    };
    let FileState::HasTranslation(path) = s else {
        return;
    };
    let Ok(cpath) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return;
    };
    // SAFETY: opening the published translated file and moving it onto the
    // guest's fd number, keeping FD_CLOEXEC as requested.
    unsafe {
        let nfd = libc::open(cpath.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC);
        if nfd < 0 {
            return; // Keep the original; the load-time path covers it.
        }
        if let Ok((tst, _)) = FileStat::of_fd(nfd) {
            set_state(
                tst,
                FileState::Translated {
                    guest: guest.to_string(),
                },
            );
        }
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
    /// Copy into anonymous memory and rewrite the analysis's sites (file
    /// offsets); None: no metadata, scan every word.
    LoadTime(Option<Arc<xlate::Analysis>>),
}

fn read_fd(fd: i32, size: u64) -> Option<Vec<u8>> {
    let mut buf = vec![0u8; size as usize];
    let mut done = 0usize;
    while done < buf.len() {
        // SAFETY: reading into our buffer.
        let n = unsafe {
            libc::pread(
                fd,
                buf[done..].as_mut_ptr().cast(),
                buf.len() - done,
                done as i64,
            )
        };
        if n <= 0 {
            return None;
        }
        done += n as usize;
    }
    Some(buf)
}

fn analyze_fd(fd: i32, size: u64) -> Option<Arc<xlate::Analysis>> {
    let bytes = read_fd(fd, size)?;
    let elf = xlate::elf::parse(&bytes).ok()?;
    Some(Arc::new(xlate::analyze(&elf)))
}

pub fn exec_source(fd: i32) -> ExecSource {
    let Ok((st, _)) = FileStat::of_fd(fd) else {
        return ExecSource::LoadTime(None);
    };
    match state(&st) {
        Some(FileState::Translated { .. } | FileState::Identity) => ExecSource::Shared,
        Some(FileState::Uncached(Some(a))) => ExecSource::LoadTime(Some(a)),
        Some(FileState::Other) => ExecSource::LoadTime(None),
        s => {
            // Not seen at open (or opened writable): a published cache file
            // maps shared; anything else is analyzed once.
            if s.is_none()
                && let (Some(cache), Some(p)) = (&rt().cache, fd_path(fd))
                && cache.contains(&p)
            {
                return ExecSource::Shared;
            }
            let analysis = analyze_fd(fd, st.size);
            set_state(
                st,
                match (&s, &analysis) {
                    (Some(FileState::HasTranslation(p)), _) => FileState::HasTranslation(p.clone()),
                    (_, Some(a)) => FileState::Uncached(Some(a.clone())),
                    (_, None) => FileState::Other,
                },
            );
            ExecSource::LoadTime(analysis)
        }
    }
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
        && rt().cache.as_ref().is_some_and(|c| c.contains(host))
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
        FileState::HasTranslation(p) => LoaderSource::File(p, "translation cache"),
        FileState::Identity => {
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
