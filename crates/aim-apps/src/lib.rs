//! The guest's launcher apps as macOS apps (`docs/windows.md`, "App
//! shims"): what a launcher shows of each (label and icon, from its APK),
//! its icon drawn as a macOS app icon, and a shim bundle that opens it in
//! window mode.

pub mod apk;
pub mod app;
pub mod cg;
pub mod drawable;
pub mod icon;
pub mod installed;
pub mod pathdata;
pub mod res;
pub mod restrictions;
pub mod shim;
pub mod table;
