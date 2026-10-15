//! DomainVerificationLegacySettings.add at the pinned owner boundary.
use super::Owner;
impl Owner {
    pub fn set_legacy_user_state(&mut self, package: Option<&str>, user: i32, status: i32) {
        let index = match self.saved.legacy.iter().position(|(name, _)| name.as_deref() == package) {
            Some(index) => index,
            None => { self.saved.legacy.push((package.map(str::to_owned), vec![])); self.saved.legacy.len() - 1 }
        };
        let users = &mut self.saved.legacy[index].1;
        match users.iter_mut().find(|(id, _)| *id == user) {
            Some((_, value)) => *value = status,
            None => { users.push((user, status)); users.sort_by_key(|(id, _)| *id); }
        }
        self.saved.legacy.sort_by_key(|(name, _)| name.as_deref().map_or(0, crate::package::info::java_hash));
    }
}
