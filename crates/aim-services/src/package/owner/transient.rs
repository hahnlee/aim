//! Runtime PackageSetting fields owned by PackageStateUnserialized, not XML.
//! AOSP android-16.0.0_r1; Copyright AOSP, Apache License 2.0.

/// Fresh original PackageStateUnserialized fields. Copies retain these values;
/// a settings-file read constructs a fresh owner on each boot.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub hidden_until_installed: bool,
    pub updated_system_app: bool,
    pub apk_in_updated_apex: bool,
    pub apex_module_name: Option<String>,
}
