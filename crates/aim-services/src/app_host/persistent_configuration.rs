//! AMS/ATMS persistent Configuration ownership, android-16.0.0_r1 (#2555).
use super::*;
use aim_binder_host::parcel::EX_NULL_POINTER;

#[derive(Clone)]
pub(super) struct Locales {
    pub tags: String,
    pub user_set: bool,
    pub direction: i32,
    pub compat: Option<i32>,
}

impl Locales {
    pub fn new(tags: String) -> Self {
        let direction = if crate::locale::right_to_left(&tags) {
            0x80
        } else {
            0x40
        };
        Self {
            tags,
            user_set: false,
            direction,
            compat: None,
        }
    }
}

impl Platform {
    pub(super) fn locale_tags(&self) -> String {
        self.locales.lock().unwrap().tags.clone()
    }
}

impl AppHost {
    pub(super) fn update_persistent_configuration(
        &self,
        uid: i32,
        values: Option<config::Configuration>,
        attribution: Option<(Option<String>, Option<String>)>,
    ) -> Result<(), Exception> {
        if self.check_permission(Some("android.permission.CHANGE_CONFIGURATION"), uid)? != 0 {
            return Err(Exception::security(
                "updatePersistentConfiguration requires android.permission.CHANGE_CONFIGURATION",
            ));
        }
        if uid != 0 {
            let (package, tag) = match attribution {
                Some(attribution) => attribution,
                None => {
                    let (state, resolution) = self.packages()?;
                    let query = crate::package::query::Query {
                        state: &state,
                        filter: &resolution.apps_filter,
                        calling_uid: SYSTEM_UID as i32,
                    };
                    let names = query
                        .packages_for_uid(uid)
                        .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e.0))?;
                    (names.into_iter().flatten().flatten().next(), None)
                }
            };
            let mode = match package.as_deref() {
                Some(package) => appops_service::note_from(
                    self,
                    SYSTEM_UID as i32,
                    appops_service::OP_WRITE_SETTINGS,
                    uid,
                    Some(package),
                    tag.as_deref(),
                )?,
                None => appops_service::MODE_IGNORED,
            };
            if mode != 0
                && !(mode == appops_service::MODE_DEFAULT
                    && self.check_permission(Some("android.permission.WRITE_SETTINGS"), uid)? == 0)
            {
                return Err(Exception::security(
                    "updatePersistentConfiguration requires android.permission.WRITE_SETTINGS",
                ));
            }
        }
        let values = values
            .ok_or_else(|| Exception::new(EX_NULL_POINTER, "Configuration must not be null"))?;
        // Settings persistence currently belongs to user 0; never write another user's settings there.
        if uid / PER_USER_RANGE as i32 != 0 {
            return Err(Exception::new(
                EX_UNSUPPORTED_OPERATION,
                "persistent configuration for secondary users (#2555)",
            ));
        }
        self.apply_persistent_configuration(&values)
    }

    pub(super) fn apply_persistent_configuration(
        &self,
        values: &config::Configuration,
    ) -> Result<(), Exception> {
        let _update = self.configuration_update.lock().unwrap();
        let old = self.global_configuration()?;
        let merged = old.merge_persistent(values)?;
        let changes = old.diff(&merged);
        if changes == 0 {
            return Ok(());
        }
        let persisted_locale = if !values.locales.is_empty() && values.user_set_locale {
            Some(if values.locales.contains(',') {
                self.settings_provider()?
                    .best_persistent_locale(&values.locales)?
            } else {
                values.locales.clone()
            })
        } else {
            None
        };
        let inputs = self.platform()?;
        let mut settings_changes = Vec::new();
        if changes & (config::CONFIG_FONT_SCALE | config::CONFIG_LOCALE) != 0 {
            for (name, value) in [
                ("font_scale", merged.font_scale.to_string()),
                ("system_locales", merged.locales.clone()),
            ] {
                if inputs.settings.insert(
                    settings::Table::System,
                    name,
                    &settings::Change {
                        value: Some(&value),
                        tag: None,
                        make_default: false,
                        package: Some("android"),
                        caller_system: true,
                        overrideable_by_restore: true,
                    },
                    true,
                )? {
                    settings_changes.push(name);
                }
            }
        }
        if let Some(locale) = persisted_locale {
            let configured = self
                .configured
                .get()
                .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host not configured"))?;
            (configured.set_property)("persist.sys.locale", &locale);
            let ready = (configured.wait_property)(
                "persist.sys.locale",
                &locale,
                std::time::Instant::now() + std::time::Duration::from_secs(60),
            )
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
            if !ready {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "timed out setting persist.sys.locale",
                ));
            }
        }
        *inputs.locales.lock().unwrap() = Locales {
            tags: merged.locales,
            user_set: merged.user_set_locale,
            direction: merged.screen_layout & 0xc0,
            compat: Some(merged.screen_layout & 0x1000_0000),
        };
        self.propagate_configuration(&old, &settings_changes)
    }
}

#[cfg(test)]
#[path = "persistent_configuration_tests.rs"]
mod tests;
