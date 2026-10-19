//! `locale`: LocaleManagerService for the app host (`android-16.0.0_r1`):
//! an app's locales (`getApplicationLocales`) as ActivityTaskManager's
//! `PackageConfigPersister` persisted them (`system_ce/<user>/package_configs`),
//! the system's (the global configuration's), and an app's override
//! `LocaleConfig` as the service persisted it (`system_ce/<user>/locale_configs`).
//! The app host applies no app-specific configuration to an app's
//! processes, so an app with persisted locales, and the writes
//! (`setApplicationLocales`, `setOverrideLocaleConfig`), are not
//! implemented (#1735). With no Android input method (docs/input.md), no
//! caller is the current one.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::path::Path;
use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::android_app_ilocalemanager as lm;

use super::{AppHost, PER_USER_RANGE, reply};
use crate::package::query::Query;

const SYSTEM_UID: i32 = 1000;
const READ_APP_SPECIFIC_LOCALES: &str = "android.permission.READ_APP_SPECIFIC_LOCALES";
/// `LocaleConfig.STATUS_SUCCESS`.
const STATUS_SUCCESS: i32 = 0;

/// A `LocaleList`: its language tags (`writeString8(mStringRepresentation)`).
struct LocaleList(String);

impl aim_service_aidl::WriteParcelable for LocaleList {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string8(Some(&self.0));
    }
}

/// A `LocaleConfig` of its status and locales.
struct LocaleConfig(LocaleList);

impl aim_service_aidl::WriteParcelable for LocaleConfig {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(STATUS_SUCCESS);
        p.write_i32(1);
        self.0.write_to(p);
    }
}

/// `PackageConfigPersister.loadUserPackages`' record of `package`: whether
/// one of its `*_config.xml` files names it with locales or a night mode.
fn package_config(dir: &Path, package: &str) -> Result<bool, String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Ok(false) };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with("_config.xml")) {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let Some(root) = aim_android_xml::read_next_optional(&bytes).map_err(|e| format!("{}: {e}", path.display()))? else { continue };
        if root.name == "config" && root.string("package_name").as_deref() == Some(package) {
            return Ok(root.attr("locale_list").is_some() || root.attr("night_mode").is_some());
        }
    }
    Ok(false)
}

pub struct LocaleManager {
    host: Arc<AppHost>,
}

impl LocaleManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        LocaleManager { host }
    }

    fn unimplemented(&self, what: &str) -> Exception {
        self.host.unimplemented_named("locale", &format!("{what} (#1735)"))
    }

    /// `getPackageUid(package, 0, user)`, or -1.
    fn package_uid(&self, package: &str, user: i32) -> Result<i32, Exception> {
        let (state, resolution) = self.host.packages()?;
        let query = Query { state: &state, filter: &resolution.apps_filter, calling_uid: SYSTEM_UID };
        query.package_uid(package, 0, user).map_err(|e| self.unimplemented(e.0))?
    }

    /// `getApplicationLocales`: the caller's own package's, one it
    /// installed, or any with `READ_APP_SPECIFIC_LOCALES`.
    fn application_locales(&self, uid: i32, package: &str, user: i32) -> Result<LocaleList, Exception> {
        let user = self.host.incoming_user(uid, user, true)?;
        let same_app = |other: i32| other >= 0 && other % PER_USER_RANGE as i32 == uid % PER_USER_RANGE as i32;
        // `isPackageOwnedByCaller`.
        let target = self.package_uid(package, user)?;
        if target < 0 {
            return Err(Exception::illegal_argument(format!("Unknown package: {package} for user {user}")));
        }
        // `isCallerInstaller`.
        let installer = || -> Result<bool, Exception> {
            let (state, _) = self.host.packages()?;
            let Some(installer) = state.packages.get(package).and_then(|ps| ps.install_source.installer.clone()) else { return Ok(false) };
            Ok(same_app(self.package_uid(&installer, user)?))
        };
        if !same_app(target) && !installer()? && self.host.check_permission(Some(READ_APP_SPECIFIC_LOCALES), uid)? != 0 {
            return Err(Exception::security(format!("getApplicationLocales: Neither user {uid} nor current process has {READ_APP_SPECIFIC_LOCALES}.")));
        }
        // `getApplicationLocalesUnchecked`: none without a record.
        let dir = self.host.data()?.join(format!("system_ce/{user}/package_configs"));
        if package_config(&dir, package).map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("locale: {e}")))? {
            return Err(self.unimplemented("an app's own configuration"));
        }
        Ok(LocaleList(String::new()))
    }

    /// `getOverrideLocaleConfig`: the locales of `<locale-config>` in the
    /// app's file, if it has one.
    fn override_locale_config(&self, uid: i32, package: &str, user: i32) -> Result<Option<LocaleConfig>, Exception> {
        if self.host.property("i18n.feature.dynamic_locales_change").is_some_and(|v| matches!(v.as_str(), "0" | "n" | "no" | "false" | "off")) {
            return Ok(None);
        }
        let user = self.host.incoming_user(uid, user, true)?;
        let file = self.host.data()?.join(format!("system_ce/{user}/locale_configs/{package}.xml"));
        let Ok(bytes) = std::fs::read(&file) else { return Ok(None) };
        let root = match aim_android_xml::read_next(&bytes) {
            Ok(root) if root.name == "locale-config" => root,
            Ok(root) => {
                eprintln!("app host: locale: Failed to parse XML configuration from {}: <{}>", file.display(), root.name);
                return Ok(None);
            }
            Err(e) => {
                eprintln!("app host: locale: Failed to parse XML configuration from {}: {e}", file.display());
                return Ok(None);
            }
        };
        let locales: Vec<String> = root.children().filter(|c| c.name == "locale").map(|c| c.string("name").map(|n| n.into_owned()).unwrap_or_default()).collect();
        Ok(Some(LocaleConfig(LocaleList(locales.join(",")))))
    }
}

impl Service for LocaleManager {
    fn descriptor(&self) -> &str {
        lm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            lm::GET_APPLICATION_LOCALES => {
                let a = lm::GetApplicationLocales::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match a.package_name.as_deref() {
                    Some(package) => self.application_locales(uid, package, a.user_id).map(|l| lm::write_get_application_locales_reply(&mut p, Some(&l))),
                    None => Err(Exception::new(EX_NULL_POINTER, "appPackageName")),
                }
            }
            lm::GET_SYSTEM_LOCALES => {
                lm::GetSystemLocales::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.host.global_configuration().map(|c| lm::write_get_system_locales_reply(&mut p, Some(&LocaleList(c.locales))))
            }
            lm::GET_OVERRIDE_LOCALE_CONFIG => {
                let a = lm::GetOverrideLocaleConfig::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match a.package_name.as_deref() {
                    Some(package) => self.override_locale_config(uid, package, a.user_id).map(|c| lm::write_get_override_locale_config_reply(&mut p, c.as_ref())),
                    None => Err(Exception::new(EX_NULL_POINTER, "appPackageName")),
                }
            }
            code => {
                let name = lm::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                Err(self.unimplemented(name))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `loadUserPackages`: a record is found by its package name in any
    /// `*_config.xml` file, and counts only with locales or a night mode.
    #[test]
    fn package_configs_are_found_as_the_persister_loads_them() {
        let dir = std::env::temp_dir().join(format!("aim-package-configs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a_config.xml"), br#"<config package_name="a" locale_list="ko-KR" />"#).unwrap();
        std::fs::write(dir.join("b_config.xml"), br#"<config package_name="b" />"#).unwrap();
        std::fs::write(dir.join("c.xml"), br#"<config package_name="c" night_mode="32" />"#).unwrap();
        let found = ["a", "b", "c", "d"].map(|p| package_config(&dir, p).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(found, [true, false, false, false]);
    }
}
