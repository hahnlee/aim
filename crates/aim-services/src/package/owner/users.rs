//! Native Settings user lifecycle persistence, Android 16 r1.
//! Copyright The Android Open Source Project, Apache License 2.0.
use super::{Store, WriteError, element, prepare, remove, write_resilient};
use crate::package::{Restrictions, User};

impl Store {
    /// Accept an id already allocated by original UserManagerService. User info,
    /// userlist.xml, CE/DE data and permission front ends keep their own owners.
    pub fn register_package_user(&mut self, user: u32) -> Result<(), WriteError> {
        i32::try_from(user).map_err(WriteError::before)?;
        if self.state.users.iter().any(|(id, _)| *id == user) {
            return Err(WriteError::before("package user already registered"));
        }
        self.state.users.push((
            user,
            User {
                restrictions: Restrictions::default(),
                access: None,
                runtime_permissions: None,
            },
        ));
        self.state.users.sort_by_key(|(id, _)| *id);
        self.restrictions
            .insert(user, element("package-restrictions"));
        self.unread_restrictions.insert(user);
        if let Err(error) = self.claim_unread_restrictions(user).and_then(|_|self.claim_runtime_permissions(user)) {
            self.state.users.retain(|(id, _)| *id != user);
            self.restrictions.remove(&user);
            self.unread_restrictions.remove(&user);
            self.unread_claims.remove(&user);
            self.runtime_claims.remove(&user);
            return Err(error);
        }
        Ok(())
    }

    /// Serialize the actual retained resolver and its explicit identity order,
    /// preserving unrelated package/user sections. Never infer identity order
    /// from XML, component names or the target user id.
    pub fn commit_cross_profile_user_filters(
        &mut self,
        user: u32,
        preferred: &crate::package::preferred::Preferred,
        order: &[usize],
    ) -> Result<(), WriteError> {
        use aim_android_xml::{Node, abx};
        if self.unread_restrictions.contains(&user) {
            return Err(WriteError::before("package restrictions not restored"));
        }
        let document = preferred
            .cross_profile_document(order)
            .map_err(WriteError::before)?;
        let mut root = self
            .restrictions
            .get(&user)
            .cloned()
            .ok_or_else(|| WriteError::before("cross-profile source user owner absent"))?;
        root.content.retain(|node| !matches!(node, Node::Element(entry) if entry.name == "crossProfile-intent-filters"));
        root.content.push(Node::Element(document));
        let parsed = Restrictions::parse(&root).map_err(WriteError::before)?;
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

    /// Delete only Settings' resilient restriction files. Original UM data
    /// preparer owns directory removal; permission persistence only cancels its
    /// metadata/queued writes at Settings.removeUserLPw's point in the lifecycle.
    pub fn remove_package_user(&mut self, user: u32) -> Result<(), WriteError> {
        let was_registered = self.state.users.iter().any(|(id, _)| *id == user);
        self.state.users.retain(|(id, _)| *id != user);
        self.restrictions.remove(&user);
        self.unread_restrictions.remove(&user);
        self.unread_claims.remove(&user);
        self.runtime_claims.remove(&user);
        self.preferred_users.remove(&user);
        let dir = self.data.join("system/users").join(user.to_string());
        let mut changed = was_registered;
        let mut errors = Vec::new();
        for name in [
            "package-restrictions.xml",
            "package-restrictions-backup.xml",
            "package-restrictions.xml.reservecopy",
        ] {
            let path = dir.join(name);
            let existed = std::fs::symlink_metadata(&path).is_ok();
            match remove(&path) {
                Ok(()) => changed |= existed,
                Err(error) => errors.push(error.to_string()),
            }
        }
        if !errors.is_empty() {
            return Err(WriteError {
                committed: changed,
                message: errors.join("; "),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::owner::tests::Data;
    #[test]
    fn cross_profile_cleanup_preserves_neighbors_and_requires_real_order() {
        let data = Data::new();
        let path = data.settings();
        std::fs::write(&path, b"<package-restrictions><pkg name='example.app' hidden='true'/><crossProfile-intent-filters><item targetUserId='10' ownerPackage='policy'><filter/></item><item targetUserId='11' ownerPackage='policy'><filter/></item></crossProfile-intent-filters></package-restrictions>").unwrap();
        let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let mut preferred = crate::package::preferred::Preferred::parse(None, Some(&bytes));
        assert!(preferred.remove_cross_profile_target(10));
        assert!(!preferred.remove_cross_profile_target(10));
        assert!(
            store
                .commit_cross_profile_user_filters(0, &preferred, &[])
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        store
            .commit_cross_profile_user_filters(0, &preferred, &[0])
            .unwrap();
        let persisted =
            crate::package::preferred::Preferred::parse(None, Some(&std::fs::read(&path).unwrap()));
        assert_eq!(persisted.cross_profile.entries().len(), 1);
        assert_eq!(persisted.cross_profile.entries()[0].target_user_id, 11);
        assert!(store.state.users[0].1.restrictions.packages[0].1.hidden);
    }
    #[test]
    fn register_and_remove_only_owned_restriction_files() {
        let data = Data::new();
        data.settings();
        let mut store = Store::open(&data.0, &[0]).unwrap().unwrap();
        store.register_package_user(10).unwrap();
        assert!(store.register_package_user(10).is_err());
        let root = aim_android_xml::read(
            b"<package-restrictions><pkg name='example.app' inst='false'/></package-restrictions>",
        )
        .unwrap();
        store.commit_initial_restrictions(10, root).unwrap();
        let dir = data.0.join("system/users/10");
        std::fs::write(dir.join("user-info.xml"), b"UM-owned").unwrap();
        std::fs::create_dir_all(data.0.join("misc_de/10")).unwrap();
        std::fs::write(
            data.0.join("misc_de/10/permissions-owner"),
            b"permission-owned",
        )
        .unwrap();
        assert!(
            !store
                .state
                .users
                .iter()
                .find(|(id, _)| *id == 10)
                .unwrap()
                .1
                .restrictions
                .packages[0]
                .1
                .installed
        );
        store.remove_package_user(10).unwrap();
        assert!(!store.state.users.iter().any(|(id, _)| *id == 10));
        for name in [
            "package-restrictions.xml",
            "package-restrictions-backup.xml",
            "package-restrictions.xml.reservecopy",
        ] {
            assert!(!dir.join(name).exists());
        }
        assert_eq!(
            std::fs::read(dir.join("user-info.xml")).unwrap(),
            b"UM-owned"
        );
        assert!(data.0.join("misc_de/10/permissions-owner").exists());
        store.remove_package_user(10).unwrap();
    }
}
