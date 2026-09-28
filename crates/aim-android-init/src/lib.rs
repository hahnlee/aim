//! Android 16 init, as a typed model driven only by the original inputs.
//!
//! ADR 0012 item 6: aimd plays Android init. This crate reads the
//! pinned image's own inputs and reproduces init's formats and semantics
//! (system/core/init, bionic `libc/system_properties` and
//! `libpropertyinfoserializer` at `android-16.0.0_r1`) without executing
//! anything:
//!
//! - [`rc`]: the `.rc` language (tokenizer, `service` / `on` / `import`
//!   sections, property expansion) and init's file discovery, producing
//!   [`rc::InitScripts`].
//! - [`engine`]: init's `ActionManager` (event queue, `trigger`, property
//!   triggers, the SecondStageMain boot queue) over a [`engine::CommandExecutor`]
//!   trait, with [`engine::DryRunExecutor`].
//! - [`props`]: `property_info` (the serialized context trie), the per-context
//!   `prop_area` files and `properties_serial`, the property service's set
//!   rules, and the boot-time `.prop` loading order.
//! - [`props::protocol`]: the `property_service` socket protocol.
//!
//! The crate performs no process, socket or futex operations itself; those
//! are traits implemented by the profile daemon and the syscall layer.
//!
//! The algorithms are ports of the Apache-2.0 AOSP sources named in each
//! module; `tests/golden` holds upstream fixtures and the byte-exact outputs
//! of the upstream C++ they are checked against.

mod cxx_sort;
pub mod diag;
pub mod engine;
pub mod image;
mod libbase;
pub mod props;
pub mod rc;

pub use diag::{Diagnostic, Severity};
pub use image::ImageRoot;

/// Property reads as init performs them (`android::base::GetProperty`).
///
/// An unset property and an empty one are indistinguishable to init: every
/// caller passes a default and uses it for both.
pub trait PropertyLookup {
    /// The current value, or `None` when the property does not exist.
    fn property(&self, name: &str) -> Option<String>;

    /// `GetProperty(name, default)`: the value, or `default` when unset or empty.
    fn property_or(&self, name: &str, default: &str) -> String {
        match self.property(name) {
            Some(value) if !value.is_empty() => value,
            _ => default.to_string(),
        }
    }
}

impl PropertyLookup for std::collections::BTreeMap<String, String> {
    fn property(&self, name: &str) -> Option<String> {
        self.get(name).cloned()
    }
}

impl PropertyLookup for std::collections::HashMap<String, String> {
    fn property(&self, name: &str) -> Option<String> {
        self.get(name).cloned()
    }
}

impl<T: PropertyLookup + ?Sized> PropertyLookup for &T {
    fn property(&self, name: &str) -> Option<String> {
        (**self).property(name)
    }
}

/// No properties at all.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoProperties;

impl PropertyLookup for NoProperties {
    fn property(&self, _name: &str) -> Option<String> {
        None
    }
}
