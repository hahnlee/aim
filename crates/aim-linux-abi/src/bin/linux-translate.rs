//! `linux-translate [--cache DIR] [--jobs N] [--verbose] ROOT...`
//! `linux-translate --image VOLUME [--jobs N] [--verbose]`
//!
//! Pre-populates the translation cache for every AArch64 ELF file under each
//! ROOT (an extracted Android image tree, read-only), then reports what was
//! found, how long it took and how large the cache is.
//!
//! `--cache` defaults to `~/Library/Caches/aim/translated`. `--image`
//! translates a system image volume's tree, `VOLUME/root`, into the image's
//! own cache, `VOLUME/translated`, indexed by path relative to the tree
//! (docs/storage.md); linux-run looks there first.
//! Symlinks are not followed: every file is indexed under its real path,
//! which is the host path the runtime resolves guest paths to.

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use aim_linux_abi::a64::Kind;
use aim_linux_abi::cache::Cache;
use aim_linux_abi::xlate;

fn usage() -> ! {
    eprintln!(
        "usage: linux-translate [--cache DIR] [--jobs N] [--verbose] ROOT...\n       linux-translate --image VOLUME [--jobs N] [--verbose]"
    );
    std::process::exit(2);
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>, apks: &mut usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        let p = e.path();
        if ft.is_dir() {
            walk(&p, out, apks);
        } else if ft.is_file() {
            if p.extension().is_some_and(|x| x == "apk") {
                *apks += 1;
            }
            out.push(p);
        }
    }
}

fn is_elf(p: &Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 20];
    std::fs::File::open(p)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && xlate::elf::is_aarch64_elf(&head)
}

/// Allocated bytes and logical bytes of a tree.
fn tree_size(p: &Path) -> (u64, u64, usize) {
    let mut alloc = 0;
    let mut logical = 0;
    let mut files = 0;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(m) = e.path().symlink_metadata() else {
                continue;
            };
            alloc += m.blocks() * 512;
            if m.is_dir() {
                stack.push(e.path());
            } else {
                logical += m.len();
                files += 1;
            }
        }
    }
    (alloc, logical, files)
}

#[derive(Default)]
struct Totals {
    elf: usize,
    translated: usize,
    identity: usize,
    unsupported: usize,
    reused: usize,
    sites: [usize; 6],
    brk_fallback: usize,
    ambiguous: usize,
    ambiguous_files: Vec<(usize, String, &'static str)>,
    data_excluded: usize,
    outside_code: usize,
    oat_files: usize,
    oat_sites: usize,
    fips_rehashed: usize,
    methods: BTreeMap<&'static str, usize>,
    unsupported_reasons: BTreeMap<String, usize>,
    errors: Vec<String>,
    bytes: u64,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut cache_dir = None;
    let mut image = None;
    let mut jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
    let mut verbose = false;
    let mut roots = Vec::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--cache" => {
                cache_dir = Some(args.next().map(PathBuf::from).unwrap_or_else(|| usage()))
            }
            "--jobs" => {
                jobs = args
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| usage())
            }
            "--image" => image = Some(args.next().map(PathBuf::from).unwrap_or_else(|| usage())),
            "--verbose" => verbose = true,
            "--help" | "-h" => usage(),
            _ => roots.push(PathBuf::from(a)),
        }
    }
    let cache = match image {
        Some(volume) if roots.is_empty() && cache_dir.is_none() => {
            let root = volume.join("root").canonicalize().unwrap_or_else(|e| {
                eprintln!("linux-translate: {}/root: {e}", volume.display());
                std::process::exit(1);
            });
            roots.push(root.clone());
            Cache::image(&root)
        }
        Some(_) => usage(),
        None => {
            if roots.is_empty() {
                usage();
            }
            let Some(dir) = cache_dir.or_else(Cache::default_dir) else {
                eprintln!("linux-translate: no cache directory (use --cache)");
                std::process::exit(2);
            };
            Cache::new(dir)
        }
    };
    let cache_dir = cache.dir().to_path_buf();
    let opts = xlate::Options::default();
    let start = Instant::now();

    let mut files = Vec::new();
    let mut apks = 0;
    for r in &roots {
        match r.canonicalize() {
            Ok(r) => walk(&r, &mut files, &mut apks),
            Err(e) => {
                eprintln!("linux-translate: {}: {e}", r.display());
                std::process::exit(1);
            }
        }
    }
    let walked = start.elapsed();

    let next = AtomicUsize::new(0);
    let totals = Mutex::new(Totals::default());
    std::thread::scope(|s| {
        for _ in 0..jobs.max(1) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(p) = files.get(i) else { break };
                    if !is_elf(p) {
                        continue;
                    }
                    let res = cache.translate_file(p, &opts);
                    let size = p.metadata().map(|m| m.len()).unwrap_or(0);
                    let mut t = totals.lock().unwrap();
                    match res {
                        Ok(None) => {}
                        Err(e) => t.errors.push(format!("{}: {e}", p.display())),
                        Ok(Some(r)) => {
                            t.elf += 1;
                            t.bytes += size;
                            match r.kind {
                                "translated" => t.translated += 1,
                                "identity" => t.identity += 1,
                                _ => t.unsupported += 1,
                            }
                            let Some(rep) = r.report else {
                                t.reused += 1;
                                continue;
                            };
                            for k in Kind::ALL {
                                t.sites[k as usize] += rep.sites[k as usize];
                            }
                            t.brk_fallback += rep.brk_fallback;
                            t.ambiguous += rep.ambiguous;
                            t.data_excluded += rep.data_excluded;
                            t.outside_code += rep.outside_code;
                            *t.methods.entry(rep.method).or_default() += 1;
                            t.fips_rehashed += rep.fips_rehashed as usize;
                            if rep.is_oat {
                                t.oat_files += 1;
                                t.oat_sites += rep.total_sites();
                            }
                            if rep.ambiguous > 0 {
                                t.ambiguous_files.push((
                                    rep.ambiguous,
                                    p.display().to_string(),
                                    rep.method,
                                ));
                            }
                            if r.kind == "unsupported" {
                                let why = cache
                                    .entry(&r.key)
                                    .and_then(|e| match e.kind {
                                        aim_linux_abi::cache::EntryKind::Unsupported(w) => Some(w),
                                        _ => None,
                                    })
                                    .unwrap_or_default();
                                *t.unsupported_reasons.entry(why).or_default() += 1;
                            }
                            if verbose {
                                eprintln!(
                                    "{} {} sites={} ambiguous={} data_excluded={} brk={}",
                                    r.kind,
                                    p.display(),
                                    rep.total_sites(),
                                    rep.ambiguous,
                                    rep.data_excluded,
                                    rep.brk_fallback
                                );
                            }
                        }
                    }
                }
            });
        }
    });
    let elapsed = start.elapsed();
    let t = totals.into_inner().unwrap();
    let (alloc, logical, nfiles) = tree_size(&cache_dir);

    println!("cache:           {}", cache_dir.display());
    println!("translator:      v{}", xlate::VERSION);
    println!(
        "scanned:         {} files ({} .apk archives not entered) in {:.2?} walk",
        files.len(),
        apks,
        walked
    );
    println!(
        "AArch64 ELF:     {} ({:.1} MiB): {} translated, {} identity, {} unsupported; {} already cached",
        t.elf,
        t.bytes as f64 / 1048576.0,
        t.translated,
        t.identity,
        t.unsupported,
        t.reused
    );
    let sites: Vec<String> = Kind::ALL
        .iter()
        .map(|k| format!("{} {}", t.sites[*k as usize], k.name()))
        .collect();
    println!("sites rewritten: {}", sites.join(", "));
    println!("brk fallbacks:   {} (site out of ±128 MiB)", t.brk_fallback);
    println!(
        "identification:  {:?}; {} pattern words excluded as data, {} outside executable sections",
        t.methods, t.data_excluded, t.outside_code
    );
    println!(
        "ambiguous sites: {} in {} files (rewritten without symbol evidence of code)",
        t.ambiguous,
        t.ambiguous_files.len()
    );
    let mut amb = t.ambiguous_files;
    amb.sort_by(|a, b| b.cmp(a));
    for (n, p, m) in amb.iter().take(10) {
        println!("                 {n:>6}  [{m}] {p}");
    }
    println!(
        "OAT/ODEX:        {} files, {} sites",
        t.oat_files, t.oat_sites
    );
    println!("FIPS modules:    {} rehashed", t.fips_rehashed);
    for (why, n) in &t.unsupported_reasons {
        println!("unsupported:     {n} x {why}");
    }
    for e in t.errors.iter().take(10) {
        println!("error:           {e}");
    }
    println!("time:            {:.2?} ({} jobs)", elapsed, jobs);
    println!(
        "cache size:      {:.1} MiB allocated, {:.1} MiB logical, {} files",
        alloc as f64 / 1048576.0,
        logical as f64 / 1048576.0,
        nfiles
    );
    if !t.errors.is_empty() {
        std::process::exit(1);
    }
}
