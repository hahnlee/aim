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
    /// A one-shot fixture run (for example the ART JIT audit): the OS process
    /// owns final teardown as an Android app process does, but it has no
    /// application identity, so no profile authority, Binder endpoint or app
    /// window.
    FixtureProcess,
}

impl ExecutionLifetime {
    /// Whether the OS process boundary, not in-process teardown, ends the
    /// runtime.
    pub fn owns_process_exit(self) -> bool {
        matches!(self, Self::AndroidProcess | Self::FixtureProcess)
    }
}
