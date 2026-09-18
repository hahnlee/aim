//! Explicit host execution selection, independent of APK identity or services.

use crate::{HostError, HostOutcome, ProcessCompletionObserver, RunOptions};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub(crate) struct ExecutionImageBinding {
    pub(crate) path: PathBuf,
    pub(crate) run_symbol: String,
    pub(crate) shutdown_symbol: String,
}

fn execute(
    options: &RunOptions,
    image: Option<ExecutionImageBinding>,
    completion: Option<ProcessCompletionObserver>,
) -> Result<HostOutcome, HostError> {
    options.validate()?;
    if completion.is_some() && !options.is_android_process() {
        return Err(HostError::HostService(
            "pre-exit completion requires explicit AndroidProcess lifetime".into(),
        ));
    }
    // The macOS actor currently publishes native callbacks without a reusable
    // retirement/task-settlement contract. Do not start a worker or load either
    // image on a path which cannot safely relinquish those native borrowers.
    // This is an explicit unsupported mode, not successful teardown evidence.
    #[cfg(target_os = "macos")]
    if !options.is_android_process() {
        return Err(HostError::UnsupportedExecutionLifetime(
            options.execution_lifetime,
        ));
    }
    crate::run::run_internal(options.clone(), image, completion)
}

pub fn run(options: &RunOptions) -> Result<HostOutcome, HostError> {
    execute(options, None, None)
}

/// Run an Android process with a synchronous result handoff before OS exit.
/// This does not select process identity or create any service authority.
pub fn run_with_completion(
    options: &RunOptions,
    completion: ProcessCompletionObserver,
) -> Result<HostOutcome, HostError> {
    execute(options, None, Some(completion))
}

/// The product image owns providers/surfaces; the secondary image contributes
/// only the caller-specified run/shutdown entry points.
pub fn run_with_execution_image(
    options: &RunOptions,
    execution_path: &Path,
    run_symbol: &str,
    shutdown_symbol: &str,
) -> Result<HostOutcome, HostError> {
    execute(
        options,
        Some(ExecutionImageBinding {
            path: execution_path.to_path_buf(),
            run_symbol: run_symbol.to_owned(),
            shutdown_symbol: shutdown_symbol.to_owned(),
        }),
        None,
    )
}

/// Select a secondary image without losing pre-exit verification. Assertions
/// and serialization belong to the observer, not the host/product image.
pub fn run_with_execution_image_and_completion(
    options: &RunOptions,
    execution_path: &Path,
    run_symbol: &str,
    shutdown_symbol: &str,
    completion: ProcessCompletionObserver,
) -> Result<HostOutcome, HostError> {
    execute(
        options,
        Some(ExecutionImageBinding {
            path: execution_path.to_path_buf(),
            run_symbol: run_symbol.to_owned(),
            shutdown_symbol: shutdown_symbol.to_owned(),
        }),
        Some(completion),
    )
}
