//! Checks APK signature verification against the original's
//! (docs/m4-packagemanager.md, D4): verifies each package `packages.xml`
//! records as the scan or an install did, and compares the signers, the
//! scheme, the signing lineage and the signers' key set with what the
//! original recorded. Reports how many are identical and, for each other
//! one, the first difference.
//!
//! aim-package-sign --root IMAGE_ROOT --packages PACKAGES_XML
//!     [--data DATA_DIR] [--package NAME] [--verify]
//!
//! IMAGE_ROOT is the image's tree (`target/aim/derived/root`),
//! PACKAGES_XML `/data/system/packages.xml` copied from a boot, DATA_DIR a
//! copy of that boot's `/data` for the packages installed in it. The scan
//! collects a system package's certificates without verifying its
//! contents; `--verify` verifies them too, as an install does.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use aim_apps::apk::Apk as ApkFile;
use aim_services::package::parse::Platform;
use aim_services::package::settings::{Package, Settings};
use aim_services::package::sign::{self, Apk, Build, Lineage, SigningDetails};
use android_image_extract::source::FileSource;

struct Args {
    root: PathBuf,
    packages: PathBuf,
    data: Option<PathBuf>,
    package: Option<String>,
    verify: bool,
}

fn args() -> Result<Args, String> {
    let mut a = std::env::args().skip(1);
    let (mut root, mut packages, mut data, mut package) = (None, None, None, None);
    let mut verify = false;
    while let Some(k) = a.next() {
        let mut v = || a.next().ok_or(format!("{k} needs a value"));
        match k.as_str() {
            "--root" => root = Some(PathBuf::from(v()?)),
            "--packages" => packages = Some(PathBuf::from(v()?)),
            "--data" => data = Some(PathBuf::from(v()?)),
            "--package" => package = Some(v()?),
            "--verify" => verify = true,
            _ => return Err(format!("unknown argument {k}")),
        }
    }
    Ok(Args {
        root: root.ok_or("--root is required")?,
        packages: packages.ok_or("--packages is required")?,
        data,
        package,
        verify,
    })
}

/// Where a guest path is on the host.
fn host(args: &Args, guest: &str) -> Option<PathBuf> {
    match guest.strip_prefix("/data/") {
        Some(rest) => Some(args.data.as_ref()?.join(rest)),
        None => Some(args.root.join(guest.trim_start_matches('/'))),
    }
}

/// A package's APKs at its code path: the file, or a directory's base APK
/// (the one without a split name) and its splits; and whether the base
/// declares a static shared library.
fn apks(dir: &Path, guest: &str) -> Result<(Vec<(PathBuf, String)>, bool), String> {
    let manifest = |p: &Path| {
        ApkFile::open(p)
            .and_then(|a| a.manifest())
            .map_err(|e| format!("{}: {e}", p.display()))
    };
    let mut files = Vec::new();
    if dir.is_dir() {
        for e in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let p = e.map_err(|e| e.to_string())?.path();
            if p.extension().is_some_and(|x| x == "apk") {
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                files.push((p, format!("{guest}/{name}")));
            }
        }
    } else {
        files.push((dir.to_owned(), guest.to_owned()));
    }
    let mut base = None;
    for (i, (p, _)) in files.iter().enumerate() {
        if manifest(p)?.named("split").is_none() {
            base = Some(i);
        }
    }
    let base = base.ok_or_else(|| format!("{guest}: no base APK"))?;
    let b = files.remove(base);
    let static_library = manifest(&b.0)?
        .children
        .iter()
        .filter(|c| c.name == "application")
        .flat_map(|a| &a.children)
        .any(|c| c.name == "static-library");
    files.insert(0, b);
    Ok((files, static_library))
}

fn verify(args: &Args, p: &Package, build: &Build) -> Result<SigningDetails, String> {
    let dir = host(args, &p.code_path).ok_or("no --data for an installed package")?;
    let (files, static_library) = apks(&dir, &p.code_path)?;
    let sources = files
        .iter()
        .map(|(h, _)| FileSource::open(h).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let apk = |i: usize| Apk {
        path: &files[i].1,
        data: &sources[i],
        v4: None,
    };
    let splits: Vec<Apk> = (1..files.len()).map(apk).collect();
    let skip_verify = !args.verify && !p.code_path.starts_with("/data/");
    sign::package_signing_details(
        &apk(0),
        &splits,
        static_library,
        p.target_sdk_version,
        skip_verify,
        build,
    )
    .map_err(|e| e.to_string())
}

/// The first difference between ours and the original's record.
fn difference(ours: &SigningDetails, p: &Package, settings: &Settings) -> Option<String> {
    let Some(theirs) = &p.signatures else {
        return Some("signatures: the original recorded none".into());
    };
    if ours.scheme_version != theirs.scheme_version {
        return Some(format!(
            "scheme: ours {}, original {}",
            ours.scheme_version, theirs.scheme_version
        ));
    }
    if ours.signatures != theirs.signatures {
        return Some(format!(
            "signatures: ours {} certificate(s), original {}",
            ours.signatures.len(),
            theirs.signatures.len()
        ));
    }
    if ours.past_signing_certificates != theirs.past_signatures {
        let n = |l: &Option<Lineage>| l.as_ref().map(Vec::len);
        return Some(format!(
            "past signers: ours {:?}, original {:?}",
            n(&ours.past_signing_certificates),
            n(&theirs.past_signatures)
        ));
    }
    let ks = &settings.key_sets;
    let id = p.key_set_data.proper_signing_key_set;
    if let Some((_, keys)) = ks.key_sets.iter().find(|(k, _)| *k == id) {
        let theirs: HashSet<&[u8]> = keys
            .iter()
            .filter_map(|k| ks.public_keys.iter().find(|(i, _)| i == k))
            .map(|(_, v)| v.as_slice())
            .collect();
        let Some(ours) = &ours.public_keys else {
            return Some("signing key set: null public-key set".into());
        };
        let Some(ours) = ours
            .iter()
            .map(|key| key.as_deref())
            .collect::<Option<HashSet<&[u8]>>>()
        else {
            return Some("signing key set: null public key".into());
        };
        if ours != theirs {
            return Some("signing key set: different public keys".into());
        }
    }
    None
}

fn run() -> Result<bool, String> {
    let args = args()?;
    let bytes =
        fs::read(&args.packages).map_err(|e| format!("{}: {e}", args.packages.display()))?;
    let settings = Settings::parse(&aim_android_xml::read(&bytes)?)?;
    let build = Build::of(&Platform::load(&args.root, HashSet::new())?);
    let (mut same, mut total) = (0, 0);
    let mut clusters: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut schemes: BTreeMap<String, usize> = BTreeMap::new();
    for p in &settings.packages {
        if args.package.as_ref().is_some_and(|n| *n != p.name) {
            continue;
        }
        total += 1;
        let diff = match verify(&args, p, &build) {
            Ok(ours) => {
                let lineage = ours.past_signing_certificates.as_ref().map(Vec::len);
                let kind = match lineage {
                    Some(n) => format!("v{} with a lineage of {n}", ours.scheme_version),
                    None => format!(
                        "v{}, {} signer(s)",
                        ours.scheme_version,
                        ours.signatures.len()
                    ),
                };
                *schemes.entry(kind).or_default() += 1;
                difference(&ours, p, &settings)
            }
            Err(e) => Some(format!("verification failed: {e}")),
        };
        let Some(diff) = diff else {
            same += 1;
            continue;
        };
        println!("{} ({}): {diff}", p.name, p.code_path);
        let cluster = diff.split(':').next().unwrap_or(&diff).to_owned();
        clusters.entry(cluster).or_default().push(p.name.clone());
    }
    println!();
    for (kind, n) in &schemes {
        println!("{n:4} {kind}");
    }
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
            eprintln!("aim-package-sign: {e}");
            ExitCode::from(2)
        }
    }
}
