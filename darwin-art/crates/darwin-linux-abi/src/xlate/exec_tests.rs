//! End to end: translate a synthetic ELF, publish it in a cache, map it
//! through the program loader (file-backed from the cache) and run it.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::tests::{Synth, TEXT_VADDR};
use crate::cache::{Cache, EntryKind, FileStat};
use crate::{context, loader, xrt};

std::arch::global_asm!(
    ".p2align 2",
    ".globl _xl_f, _xl_f_end",
    // x0 = value for TPIDR_EL0, x1 = u64[4] out. Uses the shadow call stack.
    "_xl_f:",
    "str x30, [x18], #8",
    "stp x29, x30, [sp, #-16]!",
    "msr tpidr_el0, x0",
    "mrs x2, tpidr_el0",
    "str x2, [x1]",
    "mrs x3, ctr_el0",
    "str x3, [x1, #8]",
    "mov x9, x1",
    "mov x8, #172",
    "svc #0",
    "str x0, [x9, #16]",
    "ldp x29, x30, [sp], #16",
    "mov x30, #0",
    "ldr x30, [x18, #-8]!",
    "ret",
    "_xl_f_end:",
);

unsafe extern "C" {
    static xl_f: u32;
    static xl_f_end: u32;
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("darwin-linux-abi-{name}-{}", std::process::id()));
    let _ = crate::cache::remove_tree(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The runtime's cache is process-wide: one directory for this test binary.
fn runtime_cache() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = scratch("rt-cache");
        xrt::init(Some(d.clone()));
        d
    })
}

fn snippet() -> Vec<u32> {
    // SAFETY: symbols defined above, in one section.
    unsafe {
        let (s, e) = (&xl_f as *const u32, &xl_f_end as *const u32);
        std::slice::from_raw_parts(s, e.offset_from(s) as usize).to_vec()
    }
}

#[test]
fn translated_file_runs_from_the_cache() {
    let cache_dir = runtime_cache();
    let work = scratch("exec");
    let text = snippet();
    let mut s = Synth::new(text.clone());
    s.syms.push(("f", 0, text.len(), super::elf::STT_FUNC));
    let orig = work.join("libsynth.so");
    std::fs::write(&orig, s.build()).unwrap();

    let cache = Cache::new(cache_dir);
    let r = cache
        .translate_file(&orig, &super::Options::default())
        .unwrap()
        .unwrap();
    assert_eq!(r.kind, "translated");
    let rep = r.report.unwrap();
    assert_eq!(rep.sites, [1, 1, 1, 1, 1, 1]);

    context::init_thread();
    let host = CString::new(orig.as_os_str().as_bytes()).unwrap();
    let img = loader::load_elf(&host, "/synth").unwrap();
    assert!(
        img.source.starts_with("translation cache"),
        "{}",
        img.source
    );
    assert_eq!(img.stats.total(), 0, "nothing rewritten at load time");

    // The text is a shared mapping of the published file.
    let text_addr = img.bias + TEXT_VADDR;
    let mut buf = [0u8; 1024];
    // SAFETY: querying our own region.
    let n = unsafe {
        libc::proc_regionfilename(libc::getpid(), text_addr, buf.as_mut_ptr().cast(), 1024)
    };
    let path = String::from_utf8_lossy(&buf[..n.max(0) as usize]).into_owned();
    assert!(path.contains(&r.key), "text backed by {path}");

    let scs_before = context::guest_scs();
    let mut out = [0u64; 4];
    // SAFETY: the snippet follows the C ABI (x0, x1).
    let f: extern "C" fn(u64, *mut u64) = unsafe { std::mem::transmute(text_addr) };
    f(0x7100_0000_dead_bee0, out.as_mut_ptr());
    assert_eq!(out[0], 0x7100_0000_dead_bee0, "mrs reads what msr wrote");
    assert_eq!(context::guest_tp(), 0x7100_0000_dead_bee0);
    assert_eq!(out[1], crate::a64::host_ctr_el0() as u64, "ctr_el0");
    assert_eq!(out[2], std::process::id() as u64, "svc getpid");
    assert_eq!(context::guest_scs(), scs_before, "shadow stack balanced");
    // The push stored the return address where the pop found it.
    // SAFETY: the shadow stack is this thread's, and the slot was written.
    let saved = unsafe { (scs_before as *const u64).read() };
    assert_ne!(saved, 0);
    // The mapping stays valid after the files are unlinked.
    crate::cache::remove_tree(&work).unwrap();
    crate::cache::remove_tree(cache_dir).unwrap();
}

#[test]
fn cache_publishes_read_only_entries_and_indexes_by_path() {
    let dir = scratch("publish");
    let work = scratch("publish-src");
    let cache = Cache::new(&dir);
    let mut s = Synth::new(vec![crate::a64::SVC0, 0xd65f_03c0]);
    s.syms.push(("f", 0, 2, super::elf::STT_FUNC));
    let orig = work.join("a.so");
    std::fs::write(&orig, s.build()).unwrap();
    let opts = super::Options::default();

    let first = cache.translate_file(&orig, &opts).unwrap().unwrap();
    assert!(first.translated_now);
    let again = cache.translate_file(&orig, &opts).unwrap().unwrap();
    assert!(!again.translated_now, "published entries are reused");
    assert_eq!(first.key, again.key);

    let entry_dir = cache.entry_dir(&first.key);
    for p in [
        entry_dir.clone(),
        entry_dir.join("elf"),
        entry_dir.join("meta"),
    ] {
        let mode = std::fs::metadata(&p).unwrap().permissions().mode();
        assert_eq!(mode & 0o222, 0, "{} is writable", p.display());
    }
    assert!(
        std::fs::read_dir(&dir).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tmp")),
        "no staging left behind"
    );

    let st = FileStat::of_path(&orig).unwrap();
    let e = cache.lookup(&orig, &st).unwrap();
    assert!(matches!(e.kind, EntryKind::Translated(_)));
    // Same content elsewhere: same key, second index entry.
    let copy = work.join("b.so");
    std::fs::copy(&orig, &copy).unwrap();
    let r = cache.translate_file(&copy, &opts).unwrap().unwrap();
    assert_eq!(r.key, first.key);
    assert!(!r.translated_now);

    // A changed file misses until it is translated again.
    std::fs::write(&orig, Synth::new(vec![0xd65f_03c0]).build()).unwrap();
    let st2 = FileStat::of_path(&orig).unwrap();
    assert!(cache.lookup(&orig, &st2).is_none());
    let r = cache.translate_file(&orig, &opts).unwrap().unwrap();
    assert_eq!(r.kind, "identity");
    assert!(matches!(
        cache.lookup(&orig, &st2).unwrap().kind,
        EntryKind::Identity
    ));

    // A version bump is a different key: old entries are never read.
    let old = format!("{}-v0", first.key.split('-').next().unwrap());
    assert!(cache.entry(&old).is_none());

    crate::cache::remove_tree(&dir).unwrap();
    crate::cache::remove_tree(&work).unwrap();
}
