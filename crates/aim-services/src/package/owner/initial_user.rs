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
        cross_user_suspension: bool,
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
            packages.push(Node::Element(initial_package(
                &setting.name,
                state,
                id,
                cross_user_suspension,
            )?));
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

fn initial_package(
    name: &str,
    state: &UserState,
    user: i32,
    cross_user: bool,
) -> Result<Element, WriteError> {
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
    let owners = state.resolved_suspensions(user, cross_user);
    if !owners.is_empty() {
        attribute(&mut pkg, "suspended", Some(Value::Bool(true)));
        for (id, owner) in owners {
            let mut entry = element("suspend-params");
            attribute(
                &mut entry,
                "suspending-package",
                Some(Value::String(owner.package.clone())),
            );
            if cross_user {
                attribute(&mut entry, "suspending-user", Some(Value::Int(id)));
            }
            if let Some(params) = &owner.params {
                attribute(
                    &mut entry,
                    "quarantined",
                    Some(Value::Bool(params.quarantined)),
                );
                if let Some(dialog) = &params.dialog {
                    entry
                        .content
                        .push(Node::Element(dialog.save("dialog-info")));
                }
                for (tag, bundle) in [
                    ("app-extras", &params.app_extras),
                    ("launcher-extras", &params.launcher_extras),
                ] {
                    if let Some(bundle) = bundle {
                        entry
                            .content
                            .push(Node::Element(bundle.save(tag).map_err(WriteError::before)?));
                    }
                }
            }
            pkg.content.push(Node::Element(entry));
        }
    }
    if let Some(state) = &state.archive_state {
        let mut archive = element("archive-state");
        attribute(
            &mut archive,
            "installer-title",
            Some(Value::String(state.installer_title.clone())),
        );
        attribute(
            &mut archive,
            "archive-time",
            Some(Value::LongHex(state.archive_time)),
        );
        for activity in &state.activities {
            let mut entry = element("archive-activity-info");
            attribute(
                &mut entry,
                "activity-title",
                Some(Value::String(activity.title.clone())),
            );
            attribute(
                &mut entry,
                "original-component-name",
                Some(Value::String(activity.original_component_name.clone())),
            );
            for (tag, path) in [
                ("icon-path", &activity.icon_path),
                ("monochrome-icon-path", &activity.monochrome_icon_path),
            ] {
                if let Some(path) = path {
                    if !path.starts_with('/') {
                        return Err(WriteError::before(
                            "archive icon path requires guest absolute-path ownership",
                        ));
                    }
                    attribute(&mut entry, tag, Some(Value::String(path.clone())));
                }
            }
            archive.content.push(Node::Element(entry));
        }
        pkg.content.push(Node::Element(archive));
    }
    Ok(pkg)
}
