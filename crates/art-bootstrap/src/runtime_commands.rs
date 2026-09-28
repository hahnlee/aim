use super::*;
pub(crate) use crate::native_graph::build_native_graph;
use aim_build_contract::RuntimeFlavor;

pub(crate) fn build_runtime_bootstrap(root: &Path) -> Result<()> {
    build_native_graph(root, "runtime-bootstrap")
}

pub(crate) fn build_runtime_graphics_bootstrap(root: &Path) -> Result<()> {
    build_native_graph(root, "graphics-bootstrap")
}

pub(crate) fn build_graphics_foundation(root: &Path) -> Result<()> {
    build_native_graph(root, "graphics-foundation")
}

pub(crate) fn build_runtime_graphics_bootstrap_inner(root: &Path) -> Result<()> {
    build_runtime_bootstrap_flavor(root, RuntimeFlavor::Graphics)
}

pub(crate) fn build_runtime_bootstrap_inner(root: &Path) -> Result<()> {
    build_runtime_bootstrap_flavor(root, RuntimeFlavor::Headless)
}
