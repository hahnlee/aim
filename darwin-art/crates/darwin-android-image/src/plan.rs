//! Validates a manifest against the original tree and the source root,
//! producing the plan that `assemble`, `identity` and `diff` share.

use crate::manifest::{Entry, Kind, Manifest};
use crate::problem::{Problem, ProblemKind};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// Root files the tool owns in an image tree. `.identity` names the tree's
/// content identity (the original's extraction writes one too);
/// `.overlay-receipt` is the derived identity's preimage.
pub const IDENTITY_FILE: &str = ".identity";
pub const RECEIPT_FILE: &str = ".overlay-receipt";

/// What a guest path is in the original tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Footprint {
    File { bytes: u64 },
    Symlink,
    Directory { files: u64, bytes: u64 },
    Other,
}

impl Footprint {
    /// Bytes of regular-file content under this path.
    pub fn bytes(self) -> u64 {
        match self {
            Footprint::File { bytes } | Footprint::Directory { bytes, .. } => bytes,
            Footprint::Symlink | Footprint::Other => 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// As written in the manifest, relative to the source root.
    pub declared: String,
    pub resolved: PathBuf,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub kind: Kind,
    pub path: String,
    pub reason: Option<String>,
    pub source: Option<Source>,
    /// The path in the original (`replace` and `remove`).
    pub original: Option<Footprint>,
}

impl Step {
    /// The guest path relative to an image root.
    pub fn relative(&self) -> &str {
        &self.path[1..]
    }
}

/// A validated manifest. Steps are sorted by guest path, which is also their
/// canonical order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
}

/// Validates `manifest` against the original tree `original`; sources
/// resolve against `source_root`.
pub fn validate(
    manifest: &Manifest,
    original: &Path,
    source_root: &Path,
) -> Result<Plan, Vec<Problem>> {
    let mut problems = Vec::new();
    if !original.is_dir() {
        problems.push(Problem::new(
            ProblemKind::MissingInOriginal,
            format!("original tree {} is not a directory", original.display()),
        ));
        return Err(problems);
    }

    // Duplicates and overlaps are checked on the declared paths first; an
    // entry with an invalid path is reported and skipped.
    let mut by_path: BTreeMap<&str, &Entry> = BTreeMap::new();
    for entry in &manifest.entries {
        if let Err(message) = check_guest_path(&entry.path) {
            problems.push(Problem::at(
                ProblemKind::InvalidGuestPath,
                entry.line,
                format!("{}: {message}", entry.path),
            ));
            continue;
        }
        if let Some(first) = by_path.get(entry.path.as_str()) {
            problems.push(Problem::at(
                ProblemKind::Duplicate,
                entry.line,
                format!("{} is already declared on line {}", entry.path, first.line),
            ));
            continue;
        }
        by_path.insert(&entry.path, entry);
    }
    let declared: HashSet<&str> = by_path.keys().copied().collect();
    for (path, entry) in &by_path {
        for ancestor in ancestors(path) {
            if let Some(outer) = declared.get(ancestor).and_then(|a| by_path.get(a)) {
                problems.push(Problem::at(
                    ProblemKind::Overlap,
                    entry.line,
                    format!(
                        "{path} is inside {ancestor}, which line {} already declares ({})",
                        outer.line, outer.kind
                    ),
                ));
            }
        }
    }

    let mut steps = Vec::new();
    for (path, entry) in by_path {
        let before = problems.len();
        if [IDENTITY_FILE, RECEIPT_FILE].contains(&&path[1..]) {
            problems.push(Problem::at(
                ProblemKind::Reserved,
                entry.line,
                format!("{path} is written by android-image itself"),
            ));
            continue;
        }
        let original_footprint = match check_in_original(entry, original) {
            Ok(footprint) => footprint,
            Err(problem) => {
                problems.push(problem);
                None
            }
        };
        let source = entry.source.as_deref().and_then(|declared| {
            match resolve_source(declared, source_root) {
                Ok(source) => Some(source),
                Err(problem) => {
                    problems.push(Problem::at(problem.0, entry.line, problem.1));
                    None
                }
            }
        });
        if problems.len() == before {
            steps.push(Step {
                kind: entry.kind,
                path: path.to_string(),
                reason: entry.reason.clone(),
                source,
                original: original_footprint,
            });
        }
    }

    if problems.is_empty() {
        Ok(Plan { steps })
    } else {
        problems.sort_by_key(|problem| problem.line);
        Err(problems)
    }
}

/// An absolute Android path with no empty, `.` or `..` components.
fn check_guest_path(path: &str) -> Result<(), &'static str> {
    let Some(rest) = path.strip_prefix('/') else {
        return Err("guest paths must be absolute");
    };
    if rest.is_empty() {
        return Err("the image root itself cannot be an entry");
    }
    if path.contains('\0') || path.contains('\\') {
        return Err("guest paths must not contain NUL or backslash");
    }
    for component in rest.split('/') {
        match component {
            "" => return Err("empty path component (`//` or trailing `/`)"),
            "." | ".." => return Err("`.` and `..` components are not allowed"),
            _ => {}
        }
    }
    Ok(())
}

/// Proper ancestors of an absolute guest path, excluding `/`.
fn ancestors(path: &str) -> impl Iterator<Item = &str> {
    path.match_indices('/')
        .map(|(index, _)| &path[..index])
        .filter(|prefix| !prefix.is_empty())
}

/// Checks the entry's path against the original tree: every existing
/// ancestor must be a real directory (a symlink could lead outside the
/// image), and the path itself must exist or not as the kind requires.
fn check_in_original(entry: &Entry, original: &Path) -> Result<Option<Footprint>, Problem> {
    let problem = |kind, message: String| Problem::at(kind, entry.line, message);
    let mut current = original.to_path_buf();
    let components: Vec<&str> = entry.path[1..].split('/').collect();
    for (index, component) in components.iter().enumerate() {
        current.push(component);
        let last = index + 1 == components.len();
        let metadata = match fs::symlink_metadata(&current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return match entry.kind {
                    Kind::Add => Ok(None),
                    Kind::Replace | Kind::Remove => Err(problem(
                        ProblemKind::MissingInOriginal,
                        format!(
                            "{}: the original has no such path; {} needs one",
                            entry.path, entry.kind
                        ),
                    )),
                };
            }
            Err(error) => {
                return Err(problem(
                    ProblemKind::MissingInOriginal,
                    format!("{}: cannot inspect the original: {error}", entry.path),
                ));
            }
        };
        let file_type = metadata.file_type();
        if !last {
            let ancestor = format!("/{}", components[..=index].join("/"));
            if file_type.is_symlink() {
                return Err(problem(
                    ProblemKind::Traversal,
                    format!(
                        "{}: {ancestor} is a symlink in the original; declare the path it resolves to",
                        entry.path
                    ),
                ));
            }
            if !file_type.is_dir() {
                return Err(problem(
                    ProblemKind::NotADirectory,
                    format!(
                        "{}: {ancestor} is not a directory in the original",
                        entry.path
                    ),
                ));
            }
            continue;
        }
        return match entry.kind {
            Kind::Add => Err(problem(
                ProblemKind::Shadows,
                format!(
                    "{}: the original already has this path; use [[replace]] with a reason",
                    entry.path
                ),
            )),
            Kind::Replace if file_type.is_dir() => Err(problem(
                ProblemKind::ReplacesDirectory,
                format!(
                    "{}: is a directory in the original; replace its files, or remove it and add files",
                    entry.path
                ),
            )),
            Kind::Replace | Kind::Remove => footprint(&current).map(Some).map_err(|error| {
                problem(
                    ProblemKind::MissingInOriginal,
                    format!("{}: cannot measure the original: {error}", entry.path),
                )
            }),
        };
    }
    unreachable!("a validated guest path has at least one component")
}

/// Measures a path without following symlinks.
pub fn footprint(path: &Path) -> io::Result<Footprint> {
    let metadata = fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Ok(Footprint::Symlink);
    }
    if file_type.is_file() {
        return Ok(Footprint::File {
            bytes: metadata.len(),
        });
    }
    if !file_type.is_dir() {
        return Ok(Footprint::Other);
    }
    let (mut files, mut bytes) = (0, 0);
    let mut pending = vec![path.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for child in fs::read_dir(&directory)? {
            let child = child?;
            let metadata = fs::symlink_metadata(child.path())?;
            if metadata.is_dir() {
                pending.push(child.path());
            } else {
                files += 1;
                if metadata.is_file() {
                    bytes += metadata.len();
                }
            }
        }
    }
    Ok(Footprint::Directory { files, bytes })
}

fn resolve_source(declared: &str, root: &Path) -> Result<Source, (ProblemKind, String)> {
    let relative = Path::new(declared);
    let normalized = !declared.is_empty()
        && !declared.ends_with('/')
        && relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)));
    if !normalized {
        return Err((
            ProblemKind::InvalidSourcePath,
            format!(
                "source `{declared}` must be a relative path below the source root, without `.` or `..`"
            ),
        ));
    }
    let resolved = root.join(relative);
    match fs::metadata(&resolved) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => {
            return Err((
                ProblemKind::SourceMissing,
                format!("source `{declared}` is not a regular file"),
            ));
        }
        Err(error) => {
            return Err((
                ProblemKind::SourceMissing,
                format!("source `{declared}` ({}): {error}", resolved.display()),
            ));
        }
    }
    let (sha256, bytes) = hash_file(&resolved).map_err(|error| {
        (
            ProblemKind::SourceMissing,
            format!("source `{declared}`: cannot read: {error}"),
        )
    })?;
    Ok(Source {
        declared: declared.to_string(),
        resolved,
        sha256,
        bytes,
    })
}

/// Lowercase hex sha256 and length of a file's contents.
pub fn hash_file(path: &Path) -> io::Result<(String, u64)> {
    let mut hasher = Sha256::new();
    let bytes = io::copy(&mut fs::File::open(path)?, &mut hasher)?;
    Ok((hex(&hasher.finalize()), bytes))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
