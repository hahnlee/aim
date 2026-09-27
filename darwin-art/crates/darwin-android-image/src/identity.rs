//! Content identity of a derived image.
//!
//! `identity = sha256(receipt)`, where the receipt is the canonical preimage:
//!
//! ```text
//! darwin-android-image identity v1
//! original <original identity>
//! entries <n>
//! <kind> <len>:<path> <len>:<source> <len>:<reason>     (one per entry)
//! sources <m>
//! <len>:<source> <sha256>                               (one per source)
//! ```
//!
//! Entries are in guest-path order and every string is length-prefixed, so
//! the encoding is unambiguous and independent of manifest formatting,
//! comments and entry order. The receipt is written into the derived image,
//! so anyone can check the identity with `shasum -a 256 .overlay-receipt`.

use crate::plan::{IDENTITY_FILE, Plan, hex};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

const HEADER: &str = "darwin-android-image identity v1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Lowercase hex sha256 of `receipt`.
    pub hex: String,
    pub receipt: String,
}

pub fn compute(original_identity: &str, plan: &Plan) -> Identity {
    let mut receipt = String::new();
    let field = |text: &str| format!("{}:{text}", text.len());
    writeln!(receipt, "{HEADER}").unwrap();
    writeln!(receipt, "original {original_identity}").unwrap();
    writeln!(receipt, "entries {}", plan.steps.len()).unwrap();
    for step in &plan.steps {
        let source = step.source.as_ref().map_or("", |source| &source.declared);
        let reason = step.reason.as_deref().unwrap_or("");
        writeln!(
            receipt,
            "{} {} {} {}",
            step.kind,
            field(&step.path),
            field(source),
            field(reason)
        )
        .unwrap();
    }
    let sources: Vec<_> = plan
        .steps
        .iter()
        .filter_map(|s| s.source.as_ref())
        .collect();
    writeln!(receipt, "sources {}", sources.len()).unwrap();
    for source in sources {
        writeln!(receipt, "{} {}", field(&source.declared), source.sha256).unwrap();
    }
    Identity {
        hex: hex(&Sha256::digest(receipt.as_bytes())),
        receipt,
    }
}

/// Checks that `text` is a sha256 in hex; returns it lowercased.
pub fn parse_hex_identity(text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(text.to_ascii_lowercase())
    } else {
        Err(format!("`{text}` is not a 64-digit hex sha256"))
    }
}

/// Reads the identity file at the root of an image tree, if present.
pub fn read_tree_identity(tree: &Path) -> Result<Option<String>, String> {
    let path = tree.join(IDENTITY_FILE);
    match fs::read_to_string(&path) {
        Ok(text) => parse_hex_identity(&text)
            .map(Some)
            .map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// The original's identity, from `--original-identity` and/or
/// `ORIGDIR/.identity`. When both are present they must agree.
pub fn original_identity(original: &Path, flag: Option<&str>) -> Result<Option<String>, String> {
    let flag = flag.map(parse_hex_identity).transpose()?;
    let recorded = read_tree_identity(original)?;
    match (flag, recorded) {
        (Some(flag), Some(recorded)) if flag != recorded => Err(format!(
            "--original-identity {flag} disagrees with {} ({recorded})",
            original.join(IDENTITY_FILE).display()
        )),
        (flag, recorded) => Ok(flag.or(recorded)),
    }
}
