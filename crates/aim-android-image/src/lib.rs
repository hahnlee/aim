//! Derives the system image from the original Android image
//! (docs/adr/0012-original-android-userspace.md, "Derived system image").
//!
//! The derived image is the extracted original tree plus the deviations a
//! checked-in overlay manifest (`image/overlay.toml`) declares: files added,
//! files replaced with a reason, and paths removed with a reason. It has its
//! own content identity, computed from the original's identity, the canonical
//! manifest and the hash of every overlay source.

pub mod assemble;
pub mod dex;
pub mod diff;
pub mod identity;
pub mod manifest;
pub mod plan;
pub mod problem;
pub mod system_server;

pub use assemble::{Outcome, assemble};
pub use identity::Identity;
pub use manifest::{Entry, Kind, Manifest};
pub use plan::{Plan, validate};
pub use problem::{Problem, ProblemKind};

use std::path::Path;

/// Parses the manifest file at `path` and validates it against the original
/// tree, with sources resolved against `source_root`.
pub fn load(
    path: &Path,
    original: &Path,
    source_root: &Path,
) -> Result<(Manifest, Plan), Vec<Problem>> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        vec![Problem::new(
            ProblemKind::Syntax,
            format!("{}: {error}", path.display()),
        )]
    })?;
    let manifest = manifest::parse(&text)?;
    let plan = validate(&manifest, original, source_root)?;
    Ok((manifest, plan))
}
