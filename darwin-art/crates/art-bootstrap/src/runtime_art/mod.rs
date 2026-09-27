//! Focused build phases for the ART native bootstrap.
//!
//! Keeping ARM64 assembly generation, interpreter objects, and the small
//! runtime/foundation probes in separate Rust modules prevents a change in one
//! phase from invalidating the command implementation for the others.

use super::*;

mod arm64;
mod boot_image;
mod dex2oat;
mod foundation;
mod interpreter;
mod jit;
mod jit_support;
mod libdexfile;
mod nterp;
mod unwindstack;

pub(crate) use arm64::*;
pub(crate) use boot_image::*;
pub(crate) use dex2oat::*;
pub(crate) use foundation::*;
pub(crate) use interpreter::*;
pub(crate) use jit::*;
pub(crate) use jit_support::*;
pub(crate) use libdexfile::*;
pub(crate) use nterp::*;
pub(crate) use unwindstack::*;
