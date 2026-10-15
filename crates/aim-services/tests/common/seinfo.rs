//! Explicit controlled compatibility input for native scan mechanics fixtures.
//! This does not verify live original PlatformCompat decisions.
use aim_services::package::{owner::seinfo::Policy, pkg::AndroidPackage, scan::SeInfoScan};
use std::sync::OnceLock;

fn declared_target(package: &AndroidPackage) -> Result<i32, String> {
    Ok(package.target_sdk_version)
}

pub fn scan() -> SeInfoScan<'static> {
    static POLICY: OnceLock<Policy> = OnceLock::new();
    SeInfoScan {
        policy: POLICY.get_or_init(|| Policy::load(&aim_paths::original_image()).unwrap()),
        compatibility: &declared_target,
    }
}
