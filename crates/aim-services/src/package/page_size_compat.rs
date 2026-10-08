//! PackageSetting page-size compatibility reads, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{
    apps_filter::{NotModelled, ROOT_UID, SYSTEM_UID, app_id, user_id},
    parse::{
        Platform,
        resources::{Config, Resources},
    },
    query::Query,
};
use aim_binder_host::parcel::Exception;

#[derive(Clone, Debug, PartialEq)]
pub struct Owner {
    apk_warning: String,
    elf_warning: String,
    apk_and_elf_warning: String,
}

impl Owner {
    /// Resolve the original framework text with the applied locale and overlays.
    pub fn load(platform: &Platform, config: Config) -> Result<Self, String> {
        let resources = Resources {
            tables: vec![&platform.framework],
            overlays: &platform.framework_overlays,
            config,
        };
        let text = |name: &str| {
            platform
                .framework
                .id("string", name)
                .and_then(|id| resources.resource_string(id))
                .ok_or_else(|| format!("page-size compatibility resource unavailable: {name}"))
        };
        Ok(Self {
            apk_warning: text("page_size_compat_apk_warning")?,
            elf_warning: text("page_size_compat_elf_warning")?,
            apk_and_elf_warning: text("page_size_compat_apk_and_elf_warning")?,
        })
    }

    pub fn warning(&self, flags: i32) -> Option<String> {
        if flags & (32 | 8 | 16) != 0 {
            return None;
        }
        match flags & (2 | 4) {
            2 => Some(self.apk_warning.clone()),
            4 => Some(self.elf_warning.clone()),
            6 => Some(self.apk_and_elf_warning.clone()),
            _ => None,
        }
    }
}

pub fn enabled(flags: i32) -> bool {
    flags & (64 | 16) == 0 && flags & (4 | 32 | 8) != 0
}

impl Query<'_> {
    fn page_size_flags(
        &self,
        name: Option<&str>,
    ) -> Result<Result<Option<i32>, Exception>, NotModelled> {
        if !matches!(app_id(self.calling_uid), ROOT_UID | SYSTEM_UID) {
            return Ok(Err(Exception::security(
                "Caller must be the system or root.",
            )));
        }
        let package = name.and_then(|name| self.state.packages.get(name));
        if package.is_none()
            || self.filtered_including_uninstalled(package, user_id(self.calling_uid))?
        {
            return Ok(Ok(None));
        }
        let flags = package
            .unwrap()
            .page_size_compat
            .ok_or(NotModelled("page-size compatibility setting unavailable"))?;
        Ok(Ok(Some(flags)))
    }

    pub(crate) fn page_size_compat_enabled(
        &self,
        name: Option<&str>,
    ) -> Result<Result<bool, Exception>, NotModelled> {
        Ok(self
            .page_size_flags(name)?
            .map(|flags| flags.is_some_and(enabled)))
    }

    pub(crate) fn page_size_compat_warning(
        &self,
        name: Option<&str>,
    ) -> Result<Result<Option<String>, Exception>, NotModelled> {
        let flags = match self.page_size_flags(name)? {
            Err(error) => return Ok(Err(error)),
            Ok(None) => return Ok(Ok(None)),
            Ok(Some(flags)) => flags,
        };
        if flags & (32 | 8 | 16) != 0 || flags & (2 | 4) == 0 {
            return Ok(Ok(None));
        }
        let owner = self
            .state
            .system
            .page_size_compat
            .as_ref()
            .ok_or(NotModelled("page-size compatibility resources unavailable"))?;
        Ok(Ok(owner.warning(flags)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        apps_filter::{AppsFilter, Config as FilterConfig},
        model::{PackageState, PackageUserState, State, User},
    };

    #[test]
    fn page_size_flags_preserve_override_and_warning_precedence() {
        let owner = Owner {
            apk_warning: "apk".into(),
            elf_warning: "elf".into(),
            apk_and_elf_warning: "both".into(),
        };
        for flags in 0..128 {
            assert_eq!(enabled(flags), flags & 80 == 0 && flags & 44 != 0);
            let expected = if flags & 56 != 0 {
                None
            } else {
                match flags & 6 {
                    2 => Some("apk"),
                    4 => Some("elf"),
                    6 => Some("both"),
                    _ => None,
                }
            };
            assert_eq!(owner.warning(flags).as_deref(), expected);
        }
        assert!(!enabled(64 | 4));
        assert_eq!(owner.warning(64 | 4).as_deref(), Some("elf"));
        assert!(!enabled(2));
        assert_eq!(owner.warning(2).as_deref(), Some("apk"));
    }

    #[test]
    fn page_size_reads_enforce_identity_and_require_captured_setting() {
        let mut state = State::default();
        state.users.insert(
            0,
            User {
                id: 0,
                ..User::default()
            },
        );
        state.users.insert(
            10,
            User {
                id: 10,
                ..User::default()
            },
        );
        let mut package = PackageState {
            name: "fixture".into(),
            app_id: 10001,
            page_size_compat: Some(4),
            ..PackageState::default()
        };
        package.users.insert(0, PackageUserState::default());
        state.packages.insert("fixture".into(), package);
        let filter = AppsFilter::new(&state, &FilterConfig::default()).unwrap();
        for uid in [0, 1000, 1001000] {
            let query = Query {
                state: &state,
                filter: &filter,
                calling_uid: uid,
            };
            assert_eq!(
                query
                    .page_size_compat_enabled(Some("missing"))
                    .unwrap()
                    .unwrap(),
                false
            );
            assert_eq!(
                query
                    .page_size_compat_warning(Some("missing"))
                    .unwrap()
                    .unwrap(),
                None
            );
        }
        for uid in [2000, 10001, 1010001] {
            let query = Query {
                state: &state,
                filter: &filter,
                calling_uid: uid,
            };
            let error = query
                .page_size_compat_enabled(Some("missing"))
                .unwrap()
                .unwrap_err();
            assert_eq!(error.code, aim_binder_host::parcel::EX_SECURITY);
            assert_eq!(error.message, "Caller must be the system or root.");
        }
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: 1000,
        };
        assert!(
            query
                .page_size_compat_enabled(Some("fixture"))
                .unwrap()
                .unwrap()
        );
        assert!(query.page_size_compat_warning(Some("fixture")).is_err());
        drop(filter);
        state.packages.get_mut("fixture").unwrap().page_size_compat = None;
        let filter = AppsFilter::new(&state, &FilterConfig::default()).unwrap();
        let query = Query {
            state: &state,
            filter: &filter,
            calling_uid: 1000,
        };
        assert!(query.page_size_compat_enabled(Some("fixture")).is_err());
    }
}
