//! Explicit ownership policy for one host execution.

/// Whether the Android VM belongs to a reusable embedding session or to a
/// one-shot Android application process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionLifetime {
    /// Request reusable in-process teardown. Hosted macOS execution currently
    /// rejects this mode until pump retirement and owned main-task settlement
    /// are supported. It is never silently promoted to AndroidProcess.
    ReusableSession,
    /// Model the Android app process boundary; the process owns final teardown.
    AndroidProcess,
}
