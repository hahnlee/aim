//! Checks the package parser against the original's parser cache
//! (docs/m4-packagemanager.md, D4): parses each package the cache holds an
//! entry for, writes it in the cache's format and compares the bytes.
//! Reports how many are identical and, for each other one, the first field
//! that differs.
//!
//! aim-package-parse --root IMAGE_ROOT --cache CACHE_DIR --features FILE
//!     [--data DATA_DIR] [--locale TAG] [--density DPI] [--package NAME]
//!
//! IMAGE_ROOT is the image's tree (`target/aim/derived/root`), CACHE_DIR
//! `/data/system/package_cache/<fingerprint>` copied from a first boot,
//! FILE the output of `pm list features` there, DATA_DIR a copy of that
//! boot's `/data` for the packages installed in it, TAG the device's
//! locale then (`persist.sys.locale`; the image's default without it),
//! DPI its default display's density (`ro.sf.lcd_density`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use aim_services::package::parse::parcel::java_hash;
use aim_services::package::parse::{Error, Platform, parse};

/// The directories the scan parses packages in, and the APEXes' apps.
const SCAN_DIRS: [&str; 15] = [
    "system/framework",
    "system/app",
    "system/priv-app",
    "system/overlay",
    "system/apex",
    "product/app",
    "product/priv-app",
    "product/overlay",
    "system_ext/app",
    "system_ext/priv-app",
    "system_ext/overlay",
    "vendor/app",
    "vendor/priv-app",
    "vendor/overlay",
    "vendor/apex",
];

struct Args {
    root: PathBuf,
    cache: PathBuf,
    features: PathBuf,
    data: Option<PathBuf>,
    package: Option<String>,
    locale: Option<String>,
    density: Option<i32>,
}

fn args() -> Result<Args, String> {
    let mut a = std::env::args().skip(1);
    let (mut root, mut cache, mut features, mut data, mut package, mut locale, mut density) =
        (None, None, None, None, None, None, None);
    while let Some(k) = a.next() {
        let mut v = || a.next().ok_or(format!("{k} needs a value"));
        match k.as_str() {
            "--root" => root = Some(PathBuf::from(v()?)),
            "--cache" => cache = Some(PathBuf::from(v()?)),
            "--features" => features = Some(PathBuf::from(v()?)),
            "--data" => data = Some(PathBuf::from(v()?)),
            "--package" => package = Some(v()?),
            "--locale" => locale = Some(v()?),
            "--density" => density = Some(v()?.parse().map_err(|_| "--density needs a number")?),
            _ => return Err(format!("unknown argument {k}")),
        }
    }
    Ok(Args {
        root: root.ok_or("--root is required")?,
        cache: cache.ok_or("--cache is required")?,
        features: features.ok_or("--features is required")?,
        data,
        package,
        locale,
        density,
    })
}

/// Every path the scan may have parsed, by the cache's key for it: the
/// file name and `File.getAbsolutePath().hashCode()`.
fn candidates(args: &Args) -> HashMap<(String, i32), (PathBuf, String)> {
    let mut out = HashMap::new();
    let mut add = |host: PathBuf, guest: String| {
        let name = guest.rsplit('/').next().unwrap_or_default().to_owned();
        out.insert((name, java_hash(&guest)), (host, guest));
    };
    let mut dirs: Vec<(PathBuf, String)> = SCAN_DIRS
        .iter()
        .map(|d| (args.root.join(d), format!("/{d}")))
        .collect();
    if let Ok(apexes) = fs::read_dir(args.root.join("apex")) {
        for a in apexes.flatten() {
            let name = a.file_name().to_string_lossy().into_owned();
            for sub in ["app", "priv-app", "overlay"] {
                dirs.push((a.path().join(sub), format!("/apex/{name}/{sub}")));
            }
        }
    }
    if let Some(data) = &args.data
        && let Ok(installs) = fs::read_dir(data.join("app"))
    {
        for i in installs.flatten() {
            let name = i.file_name().to_string_lossy().into_owned();
            dirs.push((i.path(), format!("/data/app/{name}")));
        }
    }
    for (host, guest) in dirs {
        let Ok(entries) = fs::read_dir(&host) else {
            continue;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            add(e.path(), format!("{guest}/{name}"));
        }
    }
    out
}

/// `<name>-<flags>-<hash>`.
fn cache_key(file: &str) -> Option<(String, i32, i32)> {
    let (rest, hash) = file.rsplit_once('-')?;
    let (rest, hash) = match rest.strip_suffix('-') {
        Some(r) => (r, format!("-{hash}")),
        None => (rest, hash.to_owned()),
    };
    let (name, flags) = rest.rsplit_once('-')?;
    Some((name.to_owned(), flags.parse().ok()?, hash.parse().ok()?))
}

fn read_features(path: &Path) -> Result<HashSet<String>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("feature:"))
        .map(|f| f.split('=').next().unwrap_or(f).to_owned())
        .collect())
}

fn int_at(b: &[u8], at: usize) -> Option<i32> {
    b.get(at..at + 4)
        .map(|s| i32::from_le_bytes(s.try_into().unwrap()))
}

/// The cache entry's string pool.
fn pool(b: &[u8]) -> Option<(usize, Vec<Option<String>>)> {
    let at = int_at(b, 0)? as usize;
    let n = int_at(b, at)?;
    let mut p = at + 4;
    let mut out = Vec::new();
    for _ in 0..n {
        let len = int_at(b, p)?;
        p += 4;
        if len < 0 {
            out.push(None);
            continue;
        }
        let units: Vec<u16> = (0..len as usize)
            .map(|i| u16::from_le_bytes([b[p + 2 * i], b[p + 2 * i + 1]]))
            .collect();
        out.push(Some(String::from_utf16_lossy(&units)));
        p += ((len as usize + 1) * 2).next_multiple_of(4);
    }
    Some((at, out))
}

/// The field a position is in.
fn field_at(marks: &[(usize, String)], at: usize) -> String {
    match marks.partition_point(|(p, _)| *p <= at) {
        0 => "(header)".into(),
        i => marks[i - 1].1.clone(),
    }
}

/// The first difference: the field, and what each side has there.
fn first_difference(ours: &aim_services::package::parse::parcel::Entry, theirs: &[u8]) -> String {
    let Some((their_pool_at, their_pool)) = pool(theirs) else {
        return "the original's entry has no string pool".into();
    };
    let body = ours.pool_at.min(their_pool_at);
    let missing: Vec<_> = their_pool
        .iter()
        .filter(|s| !ours.pool.contains(s))
        .take(4)
        .collect();
    let extra: Vec<_> = ours
        .pool
        .iter()
        .filter(|s| !their_pool.contains(s))
        .take(4)
        .collect();
    let strings = if missing.is_empty() && extra.is_empty() {
        String::new()
    } else {
        format!(" [strings only the original has: {missing:?}; only ours: {extra:?}]")
    };
    if let Some(at) = (4..body)
        .step_by(4)
        .find(|&i| int_at(&ours.bytes, i) != int_at(theirs, i))
    {
        return format!(
            "{} (at {at}): ours {:?}, original {:?}{strings}",
            field_at(&ours.marks, at),
            int_at(&ours.bytes, at),
            int_at(theirs, at)
        );
    }
    if ours.pool_at != their_pool_at {
        return format!(
            "{}: ours ends at {}, the original's at {their_pool_at}",
            field_at(&ours.marks, body),
            ours.pool_at
        );
    }
    let n = ours.pool.len().max(their_pool.len());
    if let Some(i) = (0..n).find(|&i| ours.pool.get(i) != their_pool.get(i)) {
        let at = ours
            .strings
            .iter()
            .find(|&&(_, s)| s == i as i32)
            .map(|&(p, _)| p);
        return format!(
            "{}: string {i}: ours {:?}, original {:?}",
            at.map_or("(pool)".into(), |at| field_at(&ours.marks, at)),
            ours.pool.get(i),
            their_pool.get(i)
        );
    }
    "the same fields, different bytes".into()
}

/// A field's name without its list indices, to group differences.
fn cluster(diff: &str) -> String {
    let field = diff.split([' ', ':']).next().unwrap_or(diff);
    let mut out = String::new();
    let mut skip = false;
    for c in field.chars() {
        match c {
            '[' => skip = true,
            ']' => skip = false,
            _ if !skip => out.push(c),
            _ => {}
        }
    }
    out
}

fn run() -> Result<bool, String> {
    let args = args()?;
    let mut platform = Platform::load(&args.root, read_features(&args.features)?)?;
    if let Some(l) = &args.locale {
        platform.locale = aim_services::package::parse::platform::locale(l);
    }
    if args.density.is_some() {
        platform.density_dpi = args.density;
    }
    let candidates = candidates(&args);
    let mut entries: Vec<_> = fs::read_dir(&args.cache)
        .map_err(|e| format!("{}: {e}", args.cache.display()))?
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    let (mut same, mut total) = (0, 0);
    let mut clusters: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in entries {
        let file = entry
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let Some((name, flags, hash)) = cache_key(&file) else {
            continue;
        };
        if args.package.as_ref().is_some_and(|p| *p != name) {
            continue;
        }
        total += 1;
        let Some((host, guest)) = candidates.get(&(name.clone(), hash)) else {
            println!("{file}: no package at a path with this key");
            clusters.entry("(not found)".into()).or_default().push(file);
            continue;
        };
        let theirs = fs::read(&entry).map_err(|e| format!("{}: {e}", entry.display()))?;
        let result = match parse(host, guest, flags, &platform) {
            Err(e) => {
                let c = match &e {
                    Error::Parse(_) => "(parse failed)".to_owned(),
                    Error::Unsupported(s) => format!("(unsupported) {s}"),
                };
                println!("{guest}: {e}");
                clusters.entry(c).or_default().push(guest.clone());
                continue;
            }
            Ok(pkg) => pkg.to_cache_entry(),
        };
        if result.bytes == theirs {
            same += 1;
            continue;
        }
        let diff = first_difference(&result, &theirs);
        println!("{guest}: {diff}");
        clusters
            .entry(cluster(&diff))
            .or_default()
            .push(guest.clone());
    }
    println!();
    for (c, pkgs) in &clusters {
        println!("{:4} {c}", pkgs.len());
    }
    println!("{same} of {total} identical");
    Ok(same == total)
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("aim-package-parse: {e}");
            ExitCode::from(2)
        }
    }
}
