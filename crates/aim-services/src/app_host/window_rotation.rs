//! WMS/DisplayRotation/DisplayWindowSettings, android-16.0.0_r1.
//! Policy persists independently of the Mac display's natural rotation (0).
use super::*;
use aim_android_xml::{Element, Node, Value};
use settings::{Change, Table};

pub(super) enum Update {
    User(Option<i32>), // None thaws; -1 freezes at the effective rotation.
    Fixed(i32),
    Ignore(Option<bool>), // None resets the override.
}
pub(super) struct Policy {
    pub frozen: bool,
    pub user: i32,
    pub fixed: i32,
    pub ignore: bool,
}
fn state(message: String) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, message)
}
fn document(path: &Path, atomic: bool) -> Result<Element, Exception> {
    let bytes = if atomic {
        usage_database::atomic_read(path)
    } else {
        std::fs::read(path)
    };
    match bytes {
        Ok(bytes) => aim_android_xml::read(&bytes).map_err(state),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Element {
            name: "display-settings".into(),
            attrs: vec![],
            content: vec![],
        }),
        Err(e) => Err(state(e.to_string())),
    }
}
fn identifier(root: &Element, id: i64) -> String {
    if root
        .children()
        .any(|e| e.name == "config" && e.int("identifier") == Ok(Some(1)))
    {
        format!("port:{}", id & 255)
    } else {
        format!("local:{id}")
    }
}
fn entry<'a>(root: &'a Element, id: &str, name: &str) -> Option<&'a Element> {
    let find = |name: &str| {
        root.children()
            .find(|e| e.name == "display" && e.string("name").as_deref() == Some(name))
    };
    find(id).or_else(|| find(name))
}
impl AppHost {
    pub(super) fn rotation_permission(&self, uid: i32) -> Result<(), Exception> {
        if self.check_permission(Some("android.permission.SET_ORIENTATION"), uid)? != 0 {
            return Err(Exception::security("Requires SET_ORIENTATION permission"));
        }
        Ok(())
    }
    pub(super) fn rotation_policy(
        &self,
        uid: i32,
        update: Option<Update>,
    ) -> Result<Policy, Exception> {
        // resetIgnoreOrientationRequest is a local shell operation without this check in WMS.
        if update
            .as_ref()
            .is_some_and(|u| !matches!(u, Update::Ignore(None)))
        {
            self.rotation_permission(uid)?;
        }
        let provider = if matches!(update, Some(Update::User(_))) {
            Some(self.settings_provider()?)
        } else {
            None
        };
        let _guard = self.windows.rotation.lock().unwrap();
        let platform = self.platform()?;
        if let Some(Update::User(rotation)) = update {
            if rotation.is_some_and(|r| !(-1..=3).contains(&r)) {
                return Err(Exception::illegal_argument(
                    "Rotation argument must be -1 or a valid rotation constant.",
                ));
            }
            let user = rotation
                .map(|r| if r == -1 { 0 } else { r })
                .unwrap_or_else(|| {
                    platform
                        .settings
                        .get(Table::System, "user_rotation")
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0)
                });
            for (name, value) in [
                ("accelerometer_rotation", i32::from(rotation.is_none())),
                ("user_rotation", user),
            ] {
                platform.settings.insert(
                    Table::System,
                    name,
                    &Change {
                        value: Some(&value.to_string()),
                        tag: None,
                        make_default: false,
                        package: Some("android"),
                        caller_system: true,
                        overrideable_by_restore: false,
                    },
                    true,
                )?;
            }
        }
        let display = self.display()?;
        let path = platform.data.join("system/display_settings.xml");
        let mut overrides = document(&path, true)?;
        let id = identifier(&overrides, display.0.id);
        if let Some((key, value)) = match update {
            Some(Update::Fixed(v)) => Some(("fixedToUserRotation", Some(Value::Int(v)))),
            Some(Update::Ignore(v)) => Some(("ignoreOrientationRequest", v.map(Value::Bool))),
            _ => None,
        } {
            let old = entry(&overrides, &id, &display.1.name).cloned();
            let mut row = old.clone().unwrap_or(Element {
                name: "display".into(),
                attrs: vec![],
                content: vec![],
            });
            row.attrs.retain(|(k, _)| k != key && k != "name");
            row.attrs.push(("name".into(), Value::String(id.clone())));
            if let Some(value) = value {
                row.attrs.push((key.into(), value));
            }
            overrides
                .content
                .retain(|n| !matches!(n, Node::Element(e) if Some(e) == old.as_ref()));
            overrides.content.push(Node::Element(row));
            let bytes = aim_android_xml::abx::write(&overrides).map_err(state)?;
            std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| state(e.to_string()))?;
            usage_database::atomic_write(&path, &bytes).map_err(|e| state(e.to_string()))?;
        }
        let product = self.root()?.join("product/etc/display_settings.xml");
        let base = document(
            &if product.exists() {
                product
            } else {
                self.root()?.join("vendor/etc/display_settings.xml")
            },
            false,
        )?;
        let base_id = identifier(&base, display.0.id);
        let base = entry(&base, &base_id, &display.1.name);
        let row = entry(&overrides, &id, &display.1.name);
        let mut merged = base.cloned().unwrap_or(Element {
            name: "display".into(),
            attrs: vec![],
            content: vec![],
        });
        if let Some(row) = row {
            for (key, value) in &row.attrs {
                merged.attrs.retain(|(k, _)| k != key);
                merged.attrs.push((key.clone(), value.clone()));
            }
        }
        let fixed = merged
            .int("fixedToUserRotation")
            .map_err(state)?
            .unwrap_or(0);
        let ignore = merged.bool("ignoreOrientationRequest").map_err(state)?;
        let allowed = platform
            .settings
            .get(
                Table::Config,
                "window_manager/allow_ignore_orientation_request",
            )
            .as_deref()
            != Some("false");
        let default_ignore = if ignore.is_none() {
            let flags = platform.settings.config_flags(None)?;
            let universal = flags.iter().any(|(k, v)| {
                k.ends_with("/com.android.window.flags.universal_resizable_by_default")
                    && v.as_deref() == Some("true")
            });
            let m = self.display_metrics()?;
            universal && allowed && m.width.min(m.height) * 160 / m.density_dpi >= 600
        } else {
            false
        };
        let policy = Policy {
            frozen: platform
                .settings
                .get(Table::System, "accelerometer_rotation")
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(0)
                == 0,
            user: platform
                .settings
                .get(Table::System, "user_rotation")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            fixed,
            ignore: ignore.unwrap_or(default_ignore) && allowed,
        };
        drop(_guard);
        // ContentService can invoke local observers synchronously; let them query this owner.
        if let Some(provider) = provider {
            for name in ["accelerometer_rotation", "user_rotation"] {
                provider.notify(Table::System, 0, name)?;
            }
        }
        Ok(policy)
    }
}
