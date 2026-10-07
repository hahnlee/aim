//! Mutation persistence preserves unrelated owner fields and publishes committed failures.
use super::{Store, WriteError, attribute, element, prepare, write_resilient};
use crate::package::write::mutation::{Change, Plan};
use aim_android_xml::{Node, Value, abx};
impl Store {
    pub fn commit_mutation(&mut self, plan: &Plan) -> Result<(), WriteError> {
        match &plan.change {
            Change::None => return Ok(()),
            Change::Enabled(enabled) => {
                return self.commit_enabled(
                    &plan.package,
                    u32::try_from(
                        plan.user
                            .ok_or_else(|| WriteError::before("mutation user absent"))?,
                    )
                    .map_err(WriteError::before)?,
                    enabled,
                );
            }
            Change::UpdateAvailable(_)
            | Change::MimeGroup { .. }
            | Change::CategoryHint(_)
            | Change::RelinquishUpdateOwner => {
                let mut root = self.settings_document.clone();
                let entry = root
                    .content
                    .iter_mut()
                    .find_map(|node| match node {
                        Node::Element(entry)
                            if entry.name == "package"
                                && entry.string("name").as_deref() == Some(&plan.package) =>
                        {
                            Some(entry)
                        }
                        _ => None,
                    })
                    .ok_or_else(|| WriteError::before("mutation package absent"))?;
                match &plan.change {
                    Change::RelinquishUpdateOwner => attribute(entry, "updateOwner", None),
                    Change::CategoryHint(category) => attribute(
                        entry,
                        "categoryHint",
                        (*category != -1).then_some(Value::Int(*category)),
                    ),
                    Change::UpdateAvailable(available) => attribute(
                        entry,
                        "updateAvailable",
                        (*available).then_some(Value::Bool(true)),
                    ),
                    Change::MimeGroup { group, types } => {
                        let group = group.as_ref().ok_or_else(|| {
                            WriteError::before("null MIME group cannot be serialized")
                        })?;
                        let position=entry.content.iter().position(|node|matches!(node,Node::Element(child) if child.name=="mime-group" && child.string("name").as_deref()==Some(group))).ok_or_else(||WriteError::before("mutation MIME group absent"))?;
                        let mut child = element("mime-group");
                        attribute(&mut child, "name", Some(Value::String(group.clone())));
                        for value in types {
                            let mut mime = element("mime-type");
                            attribute(&mut mime, "value", Some(Value::String(value.clone())));
                            child.content.push(Node::Element(mime));
                        }
                        entry.content[position] = Node::Element(child);
                    }
                    _ => unreachable!(),
                }
                return self.commit_package_document(root);
            }
            _ => {}
        }
        let user = u32::try_from(
            plan.user
                .ok_or_else(|| WriteError::before("mutation user absent"))?,
        )
        .map_err(WriteError::before)?;
        if self.unread_restrictions.contains(&user) {
            return Err(WriteError::before("package restrictions not restored"));
        }
        if !self
            .state
            .settings
            .packages
            .iter()
            .any(|package| package.name == plan.package)
        {
            return Err(WriteError::before("mutation package absent"));
        }
        let mut root = self
            .restrictions
            .get(&user)
            .cloned()
            .ok_or_else(|| WriteError::before("mutation user absent"))?;
        let position=root.content.iter().position(|node|matches!(node,Node::Element(entry) if entry.name=="pkg" && entry.string("name").as_deref()==Some(&plan.package))).unwrap_or_else(|| {let mut entry=element("pkg");attribute(&mut entry,"name",Some(Value::String(plan.package.clone())));root.content.push(Node::Element(entry));root.content.len()-1});
        let Node::Element(entry) = &mut root.content[position] else {
            unreachable!()
        };
        match &plan.change {
            Change::Stopped {
                stopped,
                not_launched,
                ..
            } => {
                attribute(entry, "stopped", (*stopped).then_some(Value::Bool(true)));
                attribute(entry, "nl", (*not_launched).then_some(Value::Bool(true)));
            }
            Change::SplashTheme(theme) => attribute(
                entry,
                "splash-screen-theme",
                theme.clone().map(Value::String),
            ),
            Change::HarmfulWarning(warning) => attribute(
                entry,
                "harmful-app-warning",
                warning.clone().map(Value::String),
            ),
            Change::MinAspectRatio(ratio) => attribute(
                entry,
                "min-aspect-ratio",
                (*ratio != 0).then_some(Value::Int(*ratio)),
            ),
            _ => unreachable!(),
        }
        let parsed =
            crate::package::restrictions::Restrictions::parse(&root).map_err(WriteError::before)?;
        let bytes = abx::write(&root).map_err(WriteError::before)?;
        let dir = self.data.join("system/users").join(user.to_string());
        let path = dir.join("package-restrictions.xml");
        let backup = dir.join("package-restrictions-backup.xml");
        prepare(&path, &backup, &self.restrictions[&user]).map_err(WriteError::before)?;
        let result = write_resilient(&path, &backup, &bytes);
        if result.is_ok() || result.as_ref().is_err_and(|error| error.committed) {
            self.restrictions.insert(user, root);
            self.state
                .users
                .iter_mut()
                .find(|(id, _)| *id == user)
                .unwrap()
                .1
                .restrictions = parsed;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::owner::tests::Data;
    #[test]
    fn mutation_reopen_preserves_other_user_and_package_fields() {
        let data = Data::new();
        let path = data.settings();
        std::fs::write(&path,b"<package-restrictions><pkg name='example.app' stopped='true' nl='true' hidden='true' enabled='3' splash-screen-theme='old'/></package-restrictions>").unwrap();
        let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
        for change in [
            Change::SplashTheme(Some("new<&".into())),
            Change::MinAspectRatio(7),
            Change::HarmfulWarning(Some(" ⚠<&😀 ".into())),
            Change::Stopped {
                stopped: false,
                not_launched: false,
                first_launch_installer: Some("installer".into()),
                was_stopped: true,
            },
        ] {
            store
                .commit_mutation(&Plan {
                    package: "example.app".into(),
                    user: Some(0),
                    change,
                })
                .unwrap();
        }
        let opened = Store::open(&data.0, &[0]).unwrap().unwrap();
        let state = &opened.state.users[0].1.restrictions.packages[0].1;
        assert!(!state.stopped && !state.not_launched);
        assert!(state.hidden);
        assert_eq!(state.enabled, 3);
        assert_eq!(state.min_aspect_ratio, 7);
        assert_eq!(state.harmful_app_warning.as_deref(), Some(" ⚠<&😀 "));
        assert_eq!(state.splash_screen_theme.as_deref(), Some("new<&"));
        store
            .commit_mutation(&Plan {
                package: "example.app".into(),
                user: Some(0),
                change: Change::HarmfulWarning(None),
            })
            .unwrap();
        assert!(
            Store::open(&data.0, &[0]).unwrap().unwrap().state.users[0]
                .1
                .restrictions
                .packages[0]
                .1
                .harmful_app_warning
                .is_none()
        );
        store
            .commit_mutation(&Plan {
                package: "example.app".into(),
                user: Some(0),
                change: Change::SplashTheme(None),
            })
            .unwrap();
        assert_eq!(
            Store::open(&data.0, &[0]).unwrap().unwrap().state.users[0]
                .1
                .restrictions
                .packages[0]
                .1
                .splash_screen_theme,
            None
        );
    }
    #[test]
    fn relinquished_update_owner_preserves_installer_and_category_on_reopen() {
        let data = Data::new();
        data.settings();
        std::fs::write(data.0.join("system/packages.xml"), b"<packages><package name='example.app' codePath='/data/app/example' userId='10100' categoryHint='4' installer='store.app' installerUid='10101' updateOwner='owner.app'/></packages>").unwrap();
        let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
        store
            .commit_mutation(&Plan {
                package: "example.app".into(),
                user: None,
                change: Change::RelinquishUpdateOwner,
            })
            .unwrap();
        let reopened = Store::open(&data.0, &[0]).unwrap().unwrap();
        let package = &reopened.state().settings.packages[0];
        assert!(package.install_source.update_owner.is_none());
        assert_eq!(
            package.install_source.installer.as_deref(),
            Some("store.app")
        );
        assert_eq!(package.install_source.installer_uid, 10101);
        assert_eq!(package.category_hint, 4);
    }
    #[test]
    fn category_override_is_durable_and_undefined_removes_only_hint() {
        let data = Data::new();
        data.settings();
        let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
        for value in [7, -123, -1] {
            store
                .commit_mutation(&Plan {
                    package: "example.app".into(),
                    user: None,
                    change: Change::CategoryHint(value),
                })
                .unwrap();
            let reopened = Store::open(&data.0, &[0]).unwrap().unwrap();
            assert_eq!(reopened.state().settings.packages[0].category_hint, value);
            assert_eq!(reopened.state().settings.packages[0].name, "example.app");
        }
    }
    #[test]
    fn package_mutation_preserves_group_neighbors_and_update_state() {
        let data = Data::new();
        data.settings();
        std::fs::write(data.0.join("system/packages.xml"),b"<packages><package name='example.app' codePath='/data/app/example' userId='10100' categoryHint='4'><mime-group name='chosen'><mime-type value='old'/></mime-group><mime-group name='kept'><mime-type value='keep'/></mime-group></package></packages>").unwrap();
        let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
        store
            .commit_mutation(&Plan {
                package: "example.app".into(),
                user: None,
                change: Change::MimeGroup {
                    group: Some("chosen".into()),
                    types: vec!["text/plain".into(), "image/png".into()],
                },
            })
            .unwrap();
        store
            .commit_mutation(&Plan {
                package: "example.app".into(),
                user: None,
                change: Change::UpdateAvailable(true),
            })
            .unwrap();
        let opened = Store::open(&data.0, &[0]).unwrap().unwrap();
        let package = &opened.state.settings.packages[0];
        assert!(package.update_available);
        assert_eq!(package.category_hint, 4);
        assert_eq!(
            package.mime_groups[0].1,
            vec![Some("image/png".into()), Some("text/plain".into())]
        );
        assert_eq!(package.mime_groups[1].1, vec![Some("keep".into())]);
    }
}
