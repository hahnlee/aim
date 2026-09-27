//! Diagnostics that init would log while reading its inputs.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// init logs `LOG(WARNING)` / `LOG(INFO)` and carries on.
    Warning,
    /// init logs `LOG(ERROR)` (and counts a parse error where it does).
    Error,
}

/// One diagnostic, located at a guest path and 1-based line (0 when none).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub file: String,
    pub line: usize,
    pub message: String,
}

impl Diagnostic {
    pub fn warning(file: impl Into<String>, line: usize, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            file: file.into(),
            line,
            message: message.into(),
        }
    }

    pub fn error(file: impl Into<String>, line: usize, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            file: file.into(),
            line,
            message: message.into(),
        }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let level = match self.severity {
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        write!(
            formatter,
            "{level}: {}:{}: {}",
            self.file, self.line, self.message
        )
    }
}
