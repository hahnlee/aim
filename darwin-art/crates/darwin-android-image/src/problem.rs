//! Manifest problems. Parsing and validation collect every problem so a
//! single run shows all of them.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProblemKind {
    Syntax,
    Schema,
    MissingField,
    UnexpectedField,
    MissingReason,
    /// The guest path is not an absolute, normalized Android path.
    InvalidGuestPath,
    /// The source is not a normalized path below the source root.
    InvalidSourcePath,
    /// The guest path would leave the image: it passes through a symlink in
    /// the original.
    Traversal,
    /// A guest path component that must be a directory is not one.
    NotADirectory,
    /// `add` of a path the original already has.
    Shadows,
    /// `replace` or `remove` of a path the original does not have.
    MissingInOriginal,
    /// `replace` of a directory.
    ReplacesDirectory,
    SourceMissing,
    Duplicate,
    /// One entry's path is inside another entry's path.
    Overlap,
    /// A path the tool itself owns in the derived image.
    Reserved,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub kind: ProblemKind,
    /// Manifest line, when the problem belongs to one.
    pub line: Option<usize>,
    pub message: String,
}

impl Problem {
    pub fn new(kind: ProblemKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            line: None,
            message: message.into(),
        }
    }

    pub fn at(kind: ProblemKind, line: usize, message: impl Into<String>) -> Self {
        Self {
            kind,
            line: Some(line),
            message: message.into(),
        }
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(formatter, "line {line}: {}", self.message),
            None => formatter.write_str(&self.message),
        }
    }
}
