use super::*;
use darwin_art_build_contract::RuntimeFlavor;

mod adapter_jobs;
mod archive;
mod compile;
mod manifest;
mod runtime_jobs;
mod seed_jobs;
mod staging;

pub(crate) use archive::finalize;
pub(crate) use compile::{RuntimeBootstrapCompiled, compile};
pub(crate) use staging::{RuntimeBootstrapStaging, prepare, prepare_runtime_shadow};

pub(crate) fn build_runtime_bootstrap_flavor(root: &Path, flavor: RuntimeFlavor) -> Result<()> {
    let staged = prepare(root, flavor)?;
    let compiled = compile(&staged, flavor)?;
    finalize(&staged, flavor, compiled)
}
