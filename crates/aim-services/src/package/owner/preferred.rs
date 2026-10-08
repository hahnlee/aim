//! Native preferred state belongs to the sole restored Settings disk owner.
use super::*;
use crate::package::preferred::{Preferred, registry::Stage};

impl Store {
    pub fn native_preferred_users(&self) -> Result<Vec<(i32, Preferred)>, String> {
        let mut users = Vec::new();
        for (user, _) in &self.state.users {
            if self.unread_restrictions.contains(user) {
                return Err("package restrictions not restored".into());
            }
            let root = self
                .restrictions
                .get(user)
                .ok_or("native preferred user document unavailable")?;
            let bytes = aim_android_xml::abx::write(root)?;
            users.push((
                i32::try_from(*user).map_err(|_| "preferred user id out of range")?,
                Preferred::parse(Some(&bytes), Some(&bytes)),
            ));
        }
        Ok(users)
    }

    /// Replaces the three native resolver sections with typed owner output.
    /// Unknown package/user sections survive; no preferred section is copied
    /// from the input XML. A committed reserve-copy failure requires publish.
    pub fn commit_preferred_stage(&mut self, stage: &Stage) -> Result<(), WriteError> {
        let user =
            u32::try_from(stage.user).map_err(|_| WriteError::before("invalid preferred user"))?;
        if self.unread_restrictions.contains(&user) {
            return Err(WriteError::before("package restrictions not restored"));
        }
        let (root, parsed, bytes) = self
            .preferred_stage_document(stage)
            .map_err(WriteError::before)?;
        let original = self
            .restrictions
            .get(&user)
            .ok_or_else(|| WriteError::before("native preferred user document unavailable"))?;
        let dir = self.data.join("system/users").join(user.to_string());
        let path = dir.join("package-restrictions.xml");
        let backup = dir.join("package-restrictions-backup.xml");
        prepare(&path, &backup, original).map_err(WriteError::before)?;
        let result = write_resilient(&path, &backup, &bytes);
        if result.is_ok() || result.as_ref().is_err_and(|error| error.committed) {
            self.restrictions.insert(user, root);
            let state = self
                .state
                .users
                .iter_mut()
                .find(|(id, _)| *id == user)
                .expect("restored preferred user state");
            state.1.restrictions = parsed;
            if !stage.preferred.preferred.entries().is_empty() {
                self.preferred_users.insert(user);
            }
        }
        result
    }

    fn preferred_stage_document(
        &self,
        stage: &Stage,
    ) -> Result<(Element, Restrictions, Vec<u8>), String> {
        let user = u32::try_from(stage.user).map_err(|_| "invalid preferred user")?;
        if self.unread_restrictions.contains(&user) {
            return Err("package restrictions not restored".into());
        }
        let documents = [
            stage
                .preferred
                .preferred_document(&stage.preferred_order, true),
            stage.preferred.persistent_document(&stage.persistent_order),
            stage
                .preferred
                .cross_profile_document(&stage.cross_profile_order),
        ]
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
        let mut root = self
            .restrictions
            .get(&user)
            .ok_or("native preferred user document unavailable")?
            .clone();
        root.content.retain(|node| !matches!(node, aim_android_xml::Node::Element(element)
            if matches!(element.name.as_str(), "preferred-activities" | "persistent-preferred-activities" | "crossProfile-intent-filters")));
        for document in documents {
            root.content.push(aim_android_xml::Node::Element(document));
        }
        let parsed = Restrictions::parse(&root)?;
        let bytes = aim_android_xml::abx::write(&root)?;
        Ok((root, parsed, bytes))
    }

    pub fn preview_preferred_stage(&self, stage: &Stage) -> Result<Vec<u8>, String> {
        self.preferred_stage_document(stage)
            .map(|(_, _, bytes)| bytes)
    }

    /// Home fallback changes Settings memory without scheduling its own disk
    /// write. The next ordinary restrictions write sees these native records.
    pub fn apply_preferred_stage_in_memory(&mut self, stage: &Stage) -> Result<(), String> {
        let user = u32::try_from(stage.user).map_err(|_| "invalid preferred user")?;
        let (root, parsed, _) = self.preferred_stage_document(stage)?;
        self.restrictions.insert(user, root);
        self.state
            .users
            .iter_mut()
            .find(|(id, _)| *id == user)
            .ok_or("preferred user state unavailable")?
            .1
            .restrictions = parsed;
        if !stage.preferred.preferred.entries().is_empty() {
            self.preferred_users.insert(user);
        }
        Ok(())
    }

    pub fn preferred_user_document(&self, user: i32) -> Result<Vec<u8>, String> {
        let user = u32::try_from(user).map_err(|_| "invalid preferred user")?;
        if self.unread_restrictions.contains(&user) {
            return Err("package restrictions not restored".into());
        }
        aim_android_xml::abx::write(
            self.restrictions
                .get(&user)
                .ok_or("preferred user document unavailable")?,
        )
    }
}

impl Store {
    pub fn pending_default_browser(&self, user: u32) -> Result<Option<String>, String> {
        if self.unread_restrictions.contains(&user) {
            return Err("package restrictions not restored".into());
        }
        let state = self
            .state
            .users
            .iter()
            .find(|(id, _)| *id == user)
            .ok_or("pending browser user unavailable")?;
        Ok(state.1.restrictions.default_browser.clone())
    }

    /// Original setPendingDefaultBrowserLPw mutates Settings memory; the next
    /// scheduled restrictions write persists this independent default-app state.
    pub fn set_pending_default_browser(
        &mut self,
        user: u32,
        package: Option<&str>,
    ) -> Result<(), String> {
        if self.unread_restrictions.contains(&user) {
            return Err("package restrictions not restored".into());
        }
        let root = self
            .restrictions
            .get_mut(&user)
            .ok_or("pending browser user unavailable")?;
        root.content.retain(|node| !matches!(node, aim_android_xml::Node::Element(section) if section.name == "default-apps"));
        let mut defaults = Element {
            name: "default-apps".into(),
            attrs: Vec::new(),
            content: Vec::new(),
        };
        if let Some(package) = package {
            defaults
                .content
                .push(aim_android_xml::Node::Element(Element {
                    name: "default-browser".into(),
                    attrs: vec![(
                        "packageName".into(),
                        aim_android_xml::Value::String(package.to_owned()),
                    )],
                    content: Vec::new(),
                }));
        }
        root.content.push(aim_android_xml::Node::Element(defaults));
        self.state
            .users
            .iter_mut()
            .find(|(id, _)| *id == user)
            .ok_or("pending browser state unavailable")?
            .1
            .restrictions
            .default_browser = package.map(str::to_owned);
        Ok(())
    }

    pub fn take_matching_pending_browser(
        &mut self,
        user: u32,
        package: &str,
    ) -> Result<bool, String> {
        if self.pending_default_browser(user)?.as_deref() != Some(package) {
            return Ok(false);
        }
        self.set_pending_default_browser(user, None)?;
        Ok(true)
    }
}
