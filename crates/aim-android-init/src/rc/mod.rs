//! init's `.rc` language and script discovery (system/core/init/parser.cpp,
//! service_parser.cpp, action_parser.cpp, import_parser.cpp, init.cpp
//! `LoadBootScripts`, apex_init_util.cpp).
//!
//! Parsing produces a typed model and never executes anything. Where init
//! consults state while parsing (property expansion in `import` and some
//! service options, `getpwnam` for users and groups, the vendor API level),
//! that state is passed in explicitly through [`ParseEnv`].

mod action;
mod builtins;
pub mod caps;
mod expand;
pub mod ids;
mod keywords;
mod loader;
mod parser;
mod service;
mod tokenizer;

pub use action::Action;
pub use builtins::{Builtin, CommandSpec};
pub use expand::{ANDROID_API_P, ANDROID_API_Q, ANDROID_API_R, expand_props};
pub use ids::IdResolver;
pub use loader::{ApexInfo, ScriptLoader, filter_versioned_configs, read_apex_info_list};
pub use parser::{ParsedScripts, Parser, SectionKind};
pub use service::{
    Critical, FileDescriptor, FileMode, Id, IoSchedClass, Namespaces, RawOption, Rlimit, Service,
    SocketDescriptor, SocketType, parse_rlimit,
};

use crate::PropertyLookup;

/// `__ANDROID_API_FUTURE__`, what init assumes when a version is unknown.
pub const ANDROID_API_FUTURE: u32 = 10000;

/// `SelinuxGetVendorAndroidVersion`: `__ANDROID_API_FUTURE__` without
/// `/system/etc/selinux/plat_sepolicy.cil` (not a split-policy device),
/// otherwise the major version in the first line of
/// `/vendor/etc/selinux/plat_sepolicy_vers.txt` (e.g. `202504`).
pub fn vendor_android_version(image: &crate::ImageRoot) -> Result<u32, String> {
    if !image.exists("/system/etc/selinux/plat_sepolicy.cil") {
        return Ok(ANDROID_API_FUTURE);
    }
    let bytes = image
        .read("/vendor/etc/selinux/plat_sepolicy_vers.txt")
        .map_err(|error| format!("Could not read vendor SELinux version: {error}"))?;
    let contents = String::from_utf8_lossy(&bytes);
    let first_line = contents.split('\n').next().unwrap_or("");
    if first_line.is_empty() {
        return Err("No version present in plat_sepolicy_vers.txt".to_string());
    }
    let major = first_line.split('.').next().unwrap_or("");
    crate::libbase::parse_int(major, i32::MIN as i64, i32::MAX as i64)
        .map(|v| v as u32)
        .ok_or_else(|| format!("Failed to parse the vendor sepolicy major version {major}"))
}

/// State init reads while parsing scripts.
pub struct ParseEnv<'a> {
    /// Properties as they are when init parses (after `PropertyInit`).
    pub properties: &'a dyn PropertyLookup,
    /// `SelinuxGetVendorAndroidVersion()`: the vendor image's API level.
    /// Gates several strictness rules that are all on for Android 16 images.
    pub vendor_api_level: u32,
    /// `getpwnam` for `user`, `group` and `socket` owners.
    pub ids: &'a IdResolver,
}

/// Every script init loads for a boot: the boot scripts, and the APEX
/// scripts init adds when `perform_apex_config` runs.
#[derive(Clone, Debug, Default)]
pub struct InitScripts {
    /// `LoadBootScripts`: `/system/etc/init/hw/init.rc`, its imports, and the
    /// partition `etc/init` directories.
    pub boot: ParsedScripts,
    /// `ParseRcScriptsFromAllApexes(bootstrap=true)`: the scripts of the
    /// APEXes active at early-init's `perform_apex_config --bootstrap`.
    pub bootstrap_apex: ParsedScripts,
    /// `ParseRcScriptsFromAllApexes`: `/apex/*/etc/*rc` of the other
    /// APEXes, parsed with the APEX parser (`service` and vendor-APEX `on`
    /// only).
    pub apex: ParsedScripts,
}
