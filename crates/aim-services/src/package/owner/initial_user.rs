//! Initial Settings.writePackageRestrictions state from a captured native scan.
//! Scalar/component rules ported from android-16.0.0_r1 Settings (Apache 2.0).
use super::{Store, WriteError, attribute, element};
use crate::package::{restrictions::UserState, scan::SigningScan};
use aim_android_xml::{Element, Node, Value};

impl Store {
    /// The scan owns every package/user record; missing dependencies never use
    /// constructor defaults. Resolver/domain sections come from their owners.
    pub fn commit_initial_scan_restrictions(
        &mut self,
        scan: &SigningScan,
        user: u32,
        mut sections: Element,
    ) -> Result<(), WriteError> {
        if super::signing::persisted(self.state.settings.clone())
            != super::signing::persisted(scan.settings.clone())
        {
            return Err(WriteError::before(
                "initial user scan/settings ownership differs",
            ));
        }
        if sections.name != "package-restrictions" || sections.children().any(|e| e.name == "pkg") {
            return Err(WriteError::before(
                "initial user side-owner sections are invalid",
            ));
        }
        let id = i32::try_from(user).map_err(WriteError::before)?;
        let mut packages = Vec::new();
        let mut states = Vec::new();
        for setting in &scan.settings.packages {
            let state = scan
                .scanned_user_states(&setting.name)
                .and_then(|users| users.get(&id))
                .ok_or_else(|| {
                    WriteError::before(format!(
                        "initialized scan user owner is absent: {}:{id}",
                        setting.name
                    ))
                })?;
            packages.push(Node::Element(initial_package(&setting.name, state)?));
            states.push((setting.name.clone(), state.clone()));
        }
        packages.append(&mut sections.content);
        sections.content = packages;
        let result = self.commit_initial_restrictions(user, sections);
        if result.is_ok() || result.as_ref().is_err_and(|error| error.committed) {
            // Serialization/re-read normalization must not mutate live scan
            // object state (null component sets and runtime overlays included).
            self.state
                .users
                .iter_mut()
                .find(|(id, _)| *id == user)
                .unwrap()
                .1
                .restrictions
                .packages = states;
        }
        result
    }
}

fn initial_package(name: &str, state: &UserState) -> Result<Element, WriteError> {
    if state
        .suspensions
        .as_ref()
        .is_some_and(|owners| !owners.is_empty())
        || state.archive_state.is_some()
    {
        return Err(WriteError::before(
            "initial suspended/archive user serialization is unavailable",
        ));
    }
    let mut pkg = element("pkg");
    attribute(&mut pkg, "name", Some(Value::String(name.into())));
    for (key, value) in [
        ("ceDataInode", state.ce_data_inode),
        ("deDataInode", state.de_data_inode),
    ] {
        if value != 0 {
            attribute(&mut pkg, key, Some(Value::Long(value)));
        }
    }
    if !state.installed {
        attribute(&mut pkg, "inst", Some(Value::Bool(false)));
    }
    for (key, value) in [
        ("stopped", state.stopped),
        ("nl", state.not_launched),
        ("hidden", state.hidden),
        ("instant-app", state.instant_app),
        ("virtual-preload", state.virtual_preload),
    ] {
        if value {
            attribute(&mut pkg, key, Some(Value::Bool(true)));
        }
    }
    for (key, value) in [
        ("distraction_flags", state.distraction_flags),
        ("enabled", state.enabled),
        ("install-reason", state.install_reason),
        ("uninstall-reason", state.uninstall_reason),
        ("min-aspect-ratio", state.min_aspect_ratio),
    ] {
        if value != 0 {
            attribute(&mut pkg, key, Some(Value::Int(value)));
        }
    }
    attribute(
        &mut pkg,
        "first-install-time",
        Some(Value::LongHex(state.first_install_time)),
    );
    for (key, value) in [
        ("enabledCaller", &state.last_disable_app_caller),
        ("harmful-app-warning", &state.harmful_app_warning),
        ("splash-screen-theme", &state.splash_screen_theme),
    ] {
        if let Some(value) = value {
            attribute(&mut pkg, key, Some(Value::String(value.clone())));
        }
    }
    for (key, values) in [
        ("enabled-components", &state.enabled_components),
        ("disabled-components", &state.disabled_components),
    ] {
        if let Some(values) = values.as_ref().filter(|v| !v.is_empty()) {
            let mut group = element(key);
            for name in values {
                let mut item = element("item");
                attribute(&mut item, "name", Some(Value::String(name.clone())));
                group.content.push(Node::Element(item));
            }
            pkg.content.push(Node::Element(group));
        }
    }
    Ok(pkg)
}
