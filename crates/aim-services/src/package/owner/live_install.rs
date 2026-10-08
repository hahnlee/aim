//! Live install user restriction persistence after native global settings.
//! Original package/user owners supply values; unrelated sections retain bytes.
use super::{Store, WriteError, initial_user::initial_package, prepare, write_resilient};
use crate::package::{restrictions::Restrictions, scan::SigningScan};
use aim_android_xml::{Node, abx};
use std::collections::BTreeSet;
impl Store {
    /// Replace only this admitted batch's package entries. This is a durable
    /// writer, not a reset of first-boot user state or resolver/domain sections.
    pub fn commit_live_install_restrictions(
        &mut self,
        scan: &SigningScan,
        user: u32,
        packages: &BTreeSet<String>,
        cross_user: bool,
    ) -> Result<(), WriteError> {
        self.validate_committed_scan(scan)?;
        if self.unread_restrictions.contains(&user) {
            return Err(WriteError::before(
                "Live install user restrictions were not restored",
            ));
        }
        let original = self
            .restrictions
            .get(&user)
            .ok_or_else(|| WriteError::before("Live install user owner unavailable"))?;
        let mut root = original.clone();
        let mut updates = Vec::new();
        for name in packages {
            if !scan
                .settings
                .packages
                .iter()
                .any(|package| package.name == *name)
            {
                return Err(WriteError::before(
                    "Live restriction request names an unadmitted package",
                ));
            }
            let state = scan
                .scanned_user_states(name)
                .and_then(|users| users.get(&(user as i32)))
                .ok_or_else(|| {
                    WriteError::before(format!(
                        "Live install user state unavailable: {name}:{user}"
                    ))
                })?;
            updates.push((
                name.clone(),
                initial_package(name, state, user as i32, cross_user)?,
            ));
        }
        let mut seen = BTreeSet::new();
        for node in &mut root.content {
            if let Node::Element(package) = node {
                if package.name == "pkg" {
                    if let Some(name) = package.string("name").map(|name| name.into_owned()) {
                        if let Some((_, replacement)) =
                            updates.iter().find(|(candidate, _)| candidate == &name)
                        {
                            if !seen.insert(name.clone()) {
                                return Err(WriteError::before(
                                    "Duplicate live restriction package",
                                ));
                            }
                            let mut merged = replacement.clone();
                            for (key, value) in &package.attrs {
                                if !matches!(
                                    key.as_str(),
                                    "name"
                                        | "ceDataInode"
                                        | "deDataInode"
                                        | "inst"
                                        | "stopped"
                                        | "nl"
                                        | "hidden"
                                        | "instant-app"
                                        | "virtual-preload"
                                        | "distraction_flags"
                                        | "enabled"
                                        | "install-reason"
                                        | "uninstall-reason"
                                        | "min-aspect-ratio"
                                        | "first-install-time"
                                        | "enabledCaller"
                                        | "harmful-app-warning"
                                        | "splash-screen-theme"
                                        | "suspended"

                                ) {
                                    merged.attrs.push((key.clone(), value.clone()));
                                }
                            }
                            for child in &package.content {
                                if !matches!(child,Node::Element(element) if matches!(element.name.as_str(),"suspend-params"|"enabled-components"|"disabled-components"|"archive-state"))
                                {
                                    merged.content.push(child.clone());
                                }
                            }
                            *package = merged;
                        }
                    }
                }
            }
        }
        for (name, package) in updates {
            if !seen.contains(&name) {
                root.content.push(Node::Element(package));
            }
        }
        let parsed = Restrictions::parse(&root).map_err(WriteError::before)?;
        let bytes = abx::write(&root).map_err(WriteError::before)?;
        let dir = self.data.join("system/users").join(user.to_string());
        let path = dir.join("package-restrictions.xml");
        let backup = dir.join("package-restrictions-backup.xml");
        prepare(&path, &backup, original).map_err(WriteError::before)?;
        let result = write_resilient(&path, &backup, &bytes);
        if result.is_ok() || result.as_ref().is_err_and(|error| error.committed) {
            self.restrictions.insert(user, root);
            let current = self
                .state
                .users
                .iter_mut()
                .find(|(id, _)| *id == user)
                .expect("Owned restriction user exists");
            current.1.restrictions = parsed;
            for name in packages {
                let state = scan
                    .scanned_user_states(name)
                    .and_then(|users| users.get(&(user as i32)))
                    .unwrap()
                    .clone();
                if let Some((_, value)) = current
                    .1
                    .restrictions
                    .packages
                    .iter_mut()
                    .find(|(candidate, _)| candidate == name)
                {
                    *value = state;
                }
            }
        }
        result
    }
}
