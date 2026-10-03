//! The parser's oracle on installs (docs/m4-packagemanager.md, slice B):
//! the package the native parser makes of an installed APK against the
//! one the original holds for it, up to what the original's scan sets
//! after parsing.

use std::collections::BTreeMap;

use crate::package::pkg::AndroidPackage;
use crate::shadow::{CheckOutcome, Value};

/// The differing lines shown of each side.
const LINES: usize = 40;

/// What the scan and the install set on a parsed package
/// (`ScanPackageUtils`, `PackageAbiHelper`, `getSigningDetails`), which
/// the parser does not: left out on both sides.
fn unscanned(mut p: AndroidPackage) -> AndroidPackage {
    p.primary_cpu_abi = None;
    p.secondary_cpu_abi = None;
    p.native_library_dir = None;
    p.native_library_root_dir = None;
    p.native_library_root_requires_isa = false;
    p.secondary_native_library_dir = None;
    p.signing_details = None;
    p
}

/// The lines of `a`'s debug form that `b`'s lacks, as many times as
/// they are missing.
fn missing(a: &str, b: &str) -> Vec<String> {
    let mut have: BTreeMap<&str, usize> = BTreeMap::new();
    for line in b.lines() {
        *have.entry(line).or_default() += 1;
    }
    a.lines()
        .filter(|line| match have.get_mut(line) {
            Some(n) if *n > 0 => {
                *n -= 1;
                false
            }
            _ => true,
        })
        .map(|line| line.trim().to_string())
        .collect()
}

/// Compares the native parser's package (or why it could not parse) with
/// the original's.
pub(super) fn compare(
    ours: Result<AndroidPackage, String>,
    original: &AndroidPackage,
) -> CheckOutcome {
    let ours = match ours {
        Ok(ours) => ours,
        Err(e) if e.starts_with("unsupported:") => return CheckOutcome::NotModelled(e),
        Err(e) => {
            return CheckOutcome::Differed {
                original: Value::Str("parsed".into()),
                model: Value::Str(e),
            };
        }
    };
    let (ours, original) = (unscanned(ours), unscanned(original.clone()));
    if ours == original {
        return CheckOutcome::Matched;
    }
    let (a, b) = (format!("{original:#?}"), format!("{ours:#?}"));
    let lines = |v: Vec<String>| Value::List(v.into_iter().take(LINES).map(Value::Str).collect());
    CheckOutcome::Differed {
        original: lines(missing(&a, &b)),
        model: lines(missing(&b, &a)),
    }
}
